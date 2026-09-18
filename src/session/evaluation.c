/**
 * @file
 * @brief Coordinate evaluator events, host effects, and suspended work.
 *
 * Exactly one language context runs at a time. A stopped context retains its
 * continuations, stream scope, pending operation, and import stack. Foreground
 * resumption restores that state; cancellation releases owned resources before
 * discarding roots or displaying borrowed diagnostics.
 */
#include "diagnostic.h"
#include "exec/exec.h"
#include "library/library.h"
#include "library/stream.h"
#include "library/value.h"
#include "module.h"
#include "platform/posix.h"
#include "private.h"
#include "runtime/runtime.h"
#include "source.h"
#include "syntax/syntax.h"
#include "text/text.h"
#include <errno.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

typedef struct Context {
  struct Context *next, *caller;
  RillEvaluation *evaluation;
  RillStreams *streams;
  RillNativePending pending;
  RillModule *module;
  char *base;
  int64_t id;
} Context;
static bool present(void *data, RillValue value) {
  Session *s = data;
  [[gnu::cleanup(rill_text_clear)]] RillBuffer out = {};
  return rill_library_display(value, &out) && rill_text_append(&out, "\n", 1) &&
         rill_session_write(s->platform.tty, out.data, out.size);
}
static Context *save_context(Session *s) {
  Context *context = malloc(sizeof(*context));
  if (!context)
    return nullptr;
  *context = (Context){.evaluation = rill_runtime_suspend(s->eval),
                       .streams = s->library.streams,
                       .pending = s->pending,
                       .module = s->modules.active,
                       .base = s->modules.base,
                       .id = s->active_id};
  if (!context->evaluation) {
    free(context);
    return nullptr;
  }
  s->library.streams = nullptr;
  s->pending = (RillNativePending){};
  s->modules.active = nullptr;
  s->modules.base = nullptr;
  s->active_id = 0;
  s->library.idle = false;
  return context;
}
static void restore_context(Session *s, Context *context) {
  rill_runtime_restore(s->eval, context->evaluation);
  s->library.streams = context->streams;
  s->pending = context->pending;
  s->modules.active = context->module;
  free(s->modules.base);
  s->modules.base = context->base;
  s->active_id = context->id;
  free(context);
}
static void cleanup_streams(Session *s) {
  rill_stream_cancel(&s->library);
  while (rill_stream_live(&s->library)) {
    (void)rill_exec_poll(s->exec, 20, -1);
    (void)rill_session_events(s);
  }
  rill_stream_clear(&s->library);
}
static bool context_owns(Context *context, RillJob *job) {
  for (; context; context = context->caller)
    if ((rill_library_pending_owned(&context->pending) &&
         context->pending.job == job) ||
        rill_stream_owns(context->streams, job))
      return true;
  return false;
}
static bool owned_job(void *data, RillJob *job) {
  Session *s = data;
  if ((rill_library_pending_owned(&s->pending) && s->pending.job == job) ||
      rill_stream_owns(s->library.streams, job) ||
      context_owns(s->callers, job))
    return true;
  for (Context *c = s->contexts; c; c = c->next)
    if (context_owns(c, job))
      return true;
  return false;
}
static void discard_context(Session *s, Context *context) {
  RillStreams *streams = s->library.streams;
  RillModule *module = s->modules.active;
  while (context) {
    Context *caller = context->caller;
    s->library.streams = context->streams;
    if (rill_library_pending_owned(&context->pending))
      rill_exec_cancel(context->pending.job);
    cleanup_streams(s);
    if (rill_library_pending_owned(&context->pending)) {
      while (rill_exec_state(context->pending.job) != RILL_JOB_COMPLETED)
        (void)rill_exec_poll(s->exec, 20, -1);
      rill_exec_acknowledge(context->pending.job);
    }
    s->modules.active = context->module;
    rill_module_abort(&s->modules);
    rill_runtime_discard(s->eval, context->evaluation);
    free(context->base);
    free(context);
    context = caller;
  }
  s->library.streams = streams;
  s->modules.active = module;
}
static bool control(void *data, RillControl action, RillValue value) {
  Session *s = data;
  const char *message = nullptr;
  if (action == RILL_CONTROL_EXIT) {
    if (!s->contexts && !s->callers)
      return false;
    message = "suspended evaluations remain; cancel them or exit_force";
  } else if (value.as.integer >= 0) {
    RillJob *job = rill_exec_find(s->exec, (size_t)value.as.integer);
    if (!job || !owned_job(s, job))
      return false;
    message = "job belongs to an evaluation; control its evaluation handle";
  } else {
    Context **at = &s->contexts;
    while (*at && (*at)->id != value.as.integer)
      at = &(*at)->next;
    if (!*at)
      message = "evaluation is active or no longer available";
    else if (action == RILL_CONTROL_BG || action == RILL_CONTROL_WAIT)
      message = "evaluation requires foreground resumption with fg";
    else if (action == RILL_CONTROL_FG) {
      s->foreground_request = value.as.integer;
      return true;
    } else {
      Context *context = *at;
      *at = context->next;
      discard_context(s, context);
      rill_runtime_resume(s->eval, (RillValue){}, (RillDiagnostic){});
      return true;
    }
  }
  rill_runtime_resume(
      s->eval, (RillValue){},
      (RillDiagnostic){.kind = RILL_PROCESS, .message = message});
  return true;
}
static RillValue context_jobs(void *data) {
  Session *s = data;
  size_t count = 0;
  for (Context *c = s->contexts; c; c = c->next)
    ++count;
  RillHeap *heap = rill_runtime_heap(s->eval);
  RillValue list =
      rill_runtime_object(heap, RILL_V_LIST, nullptr, count, nullptr, 0, 0);
  RillRoot root = {};
  if (list.kind == RILL_V_UNIT)
    return list;
  rill_runtime_root(heap, &root, &list, 1);
  size_t i = 0;
  for (Context *c = s->contexts; c; c = c->next) {
    RillValue fields[] = {{.kind = RILL_V_INT, .as.integer = c->id},
                          {.kind = RILL_V_JOB, .as.integer = c->id},
                          {},
                          {}};
    RillRoot values = {};
    rill_runtime_root(heap, &values, fields, 4);
    fields[2] = rill_runtime_object(heap, RILL_V_STRING, nullptr, 0,
                                    "evaluation", 10, 0);
    fields[3] =
        rill_runtime_object(heap, RILL_V_STRING, nullptr, 0, "Stopped", 7, 0);
    const char *keys[] = {"id", "handle", "kind", "state"};
    if (fields[2].kind != RILL_V_UNIT && fields[3].kind != RILL_V_UNIT)
      list.as.object->values[i] = rill_library_record(heap, keys, fields, 4);
    rill_runtime_unroot(heap, &values);
    if (list.as.object->values[i++].kind == RILL_V_UNIT) {
      list = (RillValue){};
      break;
    }
  }
  rill_runtime_unroot(heap, &root);
  return list;
}
static bool foreground_context(Session *s) {
  Context **at = &s->contexts;
  while (*at && (*at)->id != s->foreground_request)
    at = &(*at)->next;
  s->foreground_request = 0;
  if (!*at)
    return false;
  Context *caller = save_context(s);
  if (!caller)
    return false;
  caller->caller = s->callers;
  Context *target = *at;
  *at = target->next;
  Context **tail = &target->caller;
  while (*tail)
    tail = &(*tail)->caller;
  *tail = caller;
  s->callers = target->caller;
  restore_context(s, target);
  if ((s->pending.job && !rill_exec_resume(s->exec, s->pending.job, true)) ||
      !rill_stream_resume(&s->library)) {
    int code = errno;
    if (rill_library_pending_owned(&s->pending)) {
      rill_exec_cancel(s->pending.job);
      while (rill_exec_state(s->pending.job) != RILL_JOB_COMPLETED)
        (void)rill_exec_poll(s->exec, 20, -1);
      rill_exec_acknowledge(s->pending.job);
    }
    s->pending = (RillNativePending){};
    cleanup_streams(s);
    rill_runtime_resume(
        s->eval, (RillValue){},
        (RillDiagnostic){.kind = RILL_IO,
                         .code = code,
                         .message = "cannot resume evaluation"});
  }
  return true;
}
static int evaluate_entry(Session *s, RillSource *source, RillSyntax *syntax) {
  if (syntax->state != RILL_COMPLETE)
    return rill_session_diagnostic(s, source, syntax->diagnostic);
  rill_module_abort(&s->modules);
  free(s->modules.base);
  s->modules.base = getcwd(nullptr, 0);
  rill_runtime_begin(s->eval, syntax);
  s->pending = (RillNativePending){};
  s->active_id = 0;
  bool interrupted = false, stopping = false;
  while (!s->library.exit_requested) {
    (void)rill_exec_poll(
        s->exec, s->pending.job || s->library.idle || stopping ? 20 : 0, -1);
    unsigned bits = rill_session_events(s);
    interrupted |= (bits & RILL_SIG_INT) != 0;
    if (s->interactive && !interrupted &&
        (stopping || (bits & RILL_SIG_STOP) ||
         (rill_library_pending_owned(&s->pending) &&
          rill_exec_state(s->pending.job) == RILL_JOB_STOPPED) ||
         rill_stream_stopped(&s->library))) {
      if (!stopping) {
        // Freeze every dependency before exposing a resumable evaluation.
        rill_stream_signal(&s->library, RILL_SIG_STOP);
        if (rill_library_pending_owned(&s->pending))
          rill_exec_stop(s->pending.job);
        stopping = true;
      }
      if (!rill_stream_quiescent(&s->library) ||
          (rill_library_pending_owned(&s->pending) &&
           !rill_exec_quiescent(s->pending.job)))
        continue;
      Context *saved = save_context(s);
      if (!saved)
        return rill_session_memory(s);
      if (!saved->id) {
        if (s->next_context == INT64_MAX) {
          restore_context(s, saved);
          return 1;
        }
        saved->id = -(++s->next_context);
      }
      saved->caller = s->callers;
      s->callers = nullptr;
      saved->next = s->contexts;
      s->contexts = saved;
      const char *message = "evaluation stopped; use jobs() and fg(handle)\n";
      (void)rill_session_write(s->platform.tty, message, strlen(message));
      return 0;
    }
    if (interrupted) {
      // Finish foreground/cancel cleanup before leaving the entry. Interrupted
      // wait leaves its independent background job alive.
      if (s->pending.job && rill_exec_cancelled(s->pending.job)) {
        if (rill_exec_state(s->pending.job) != RILL_JOB_COMPLETED)
          continue;
        rill_exec_acknowledge(s->pending.job);
      }
      rill_runtime_abort(s->eval);
      return 130;
    }
    if (s->pending.job) {
      if (!rill_library_progress(&s->library, &s->pending))
        continue;
    }
    if (!rill_stream_progress(&s->library))
      continue;
    RillEvalEvent result = rill_runtime_step(s->eval);
    switch (result.state) {
    case RILL_EVAL_CLEANUP:
      rill_stream_unwind(&s->library, result.native, result.value);
      break;
    case RILL_EVAL_STREAM:
      if (s->interactive)
        rill_library_present(&s->library, result.value);
      else
        rill_runtime_resume(
            s->eval, (RillValue){},
            (RillDiagnostic){
                .kind = RILL_UNCONSUMED_STREAM,
                .message =
                    "script result Stream requires an explicit consumer"});
      break;
    case RILL_EVAL_CALLBACK:
      rill_stream_callback(&s->library, result.value);
      break;
    case RILL_EVAL_IMPORT:
    case RILL_EVAL_MODULE:
      rill_module_event(&s->modules, s->eval, result);
      break;
    case RILL_EVAL_NATIVE:
      (void)rill_library_call(&s->library, &s->pending, result);
      if (s->foreground_request && !foreground_context(s))
        return rill_session_memory(s);
      break;
    case RILL_EVAL_YIELD:
      break;
    case RILL_EVAL_ERROR: {
      if (s->callers) {
        RillDiagnostic failure = result.diagnostic;
        // The failed context remains rooted until its diagnostic is consumed.
        Context *failed = save_context(s);
        if (!failed)
          return rill_session_memory(s);
        Context *caller = s->callers;
        s->callers = caller->caller;
        restore_context(s, caller);
        rill_runtime_resume(s->eval, result.value, failure);
        // Runtime roots the resumed diagnostic's originating error when needed.
        discard_context(s, failed);
        break;
      }
      int code = result.source
                     ? rill_session_diagnostic_at(s, result.source,
                                                  result.source_bytes,
                                                  result.diagnostic)
                     : rill_session_diagnostic(s, source, result.diagnostic);
      rill_runtime_abort(s->eval);
      return code;
    }
    case RILL_EVAL_DONE:
      if (s->callers) {
        RillValue value = result.value;
        RillRoot root = {};
        rill_runtime_root(rill_runtime_heap(s->eval), &root, &value, 1);
        cleanup_streams(s);
        rill_runtime_abort(s->eval);
        Context *caller = s->callers;
        s->callers = caller->caller;
        restore_context(s, caller);
        rill_runtime_resume(s->eval, value, (RillDiagnostic){});
        rill_runtime_unroot(rill_runtime_heap(s->eval), &root);
        break;
      }
      if (s->interactive && result.value.kind != RILL_V_UNIT) {
        [[gnu::cleanup(rill_text_clear)]] RillBuffer out = {};
        if (!rill_library_display(result.value, &out) ||
            !rill_text_append(&out, "\n", 1))
          return rill_session_memory(s);
        if (!rill_session_write(s->platform.tty, out.data, out.size))
          return 1;
      }
      return 0;
    }
  }
  rill_runtime_abort(s->eval);
  return s->library.exit_code;
}
int rill_session_evaluate(Session *s, RillSource *source, RillSyntax *syntax) {
  int status = evaluate_entry(s, source, syntax);
  cleanup_streams(s);
  if (s->callers) {
    discard_context(s, s->callers);
    s->callers = nullptr;
  }
  return status;
}

void rill_session_context_services(Session *s) {
  s->library.context = s;
  s->library.control = control;
  s->library.context_jobs = context_jobs;
  s->library.owned_job = owned_job;
  s->library.present = present;
}
void rill_session_context_clear(Session *s) {
  while (s->contexts) {
    Context *c = s->contexts;
    s->contexts = c->next;
    discard_context(s, c);
  }
}
