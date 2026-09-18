#include "library.h"
#include "diagnostic.h"
#include "exec/exec.h"
#include "platform/posix.h"
#include "runtime/runtime.h"
#include "text/text.h"
#include <errno.h>
#include <inttypes.h>
#include <stdckdint.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
enum {
  N_RUN,
  N_START,
  N_WAIT,
  N_FG,
  N_BG,
  N_CANCEL,
  N_JOBS,
  N_CD,
  N_PWD,
  N_EXIT,
  N_EXIT_FORCE,
  N_BYTES,
  N_PATH,
  N_CHECK
};
static const RillNative natives[] = {{"run", N_RUN},
                                     {"start", N_START},
                                     {"wait", N_WAIT},
                                     {"fg", N_FG},
                                     {"bg", N_BG},
                                     {"cancel", N_CANCEL},
                                     {"jobs", N_JOBS},
                                     {"cd", N_CD},
                                     {"pwd", N_PWD},
                                     {"exit", N_EXIT},
                                     {"exit_force", N_EXIT_FORCE},
                                     {"bytes", N_BYTES},
                                     {"path", N_PATH},
                                     {"check", N_CHECK}};
const RillNative *rill_library_natives(size_t *count) {
  *count = sizeof(natives) / sizeof(natives[0]);
  return natives;
}
static bool finish(RillLibrary *l, RillValue v, RillDiagnostic d) {
  rill_runtime_resume(l->eval, v, d);
  return true;
}
static bool fail(RillLibrary *l, RillError k, const char *message) {
  return finish(l, (RillValue){},
                (RillDiagnostic){.kind = k, .message = message});
}
static bool scalar(RillValue v, RillBytes *out) {
  if (v.kind != RILL_V_STRING && v.kind != RILL_V_PATH &&
      v.kind != RILL_V_BYTES)
    return false;
  *out = v.as.object->bytes;
  return true;
}
static RillJob *launch(RillLibrary *l, RillValue plan, bool background,
                       RillDiagnostic *error) {
  RillObject *o = plan.as.object;
  if (!o->count || o->count > RILL_EXEC_MAX_STAGES) {
    *error = (RillDiagnostic){.kind = RILL_LIMIT,
                              .message = "pipeline limit exceeded"};
    return nullptr;
  }
  size_t count = 0;
  for (size_t i = 0; i < o->count; ++i)
    if (ckd_add(&count, count, o->values[i].as.object->count)) {
      *error = (RillDiagnostic){.kind = RILL_MEMORY,
                                .message = "launch size overflow"};
      return nullptr;
    }
  RillExecStage *stages = calloc(o->count, sizeof(*stages));
  RillBytes *args = calloc(count, sizeof(*args));
  RillExecRedirect *redirs = calloc(count, sizeof(*redirs));
  if (!stages || !args || !redirs) {
    free(stages);
    free(args);
    free(redirs);
    *error =
        (RillDiagnostic){.kind = RILL_MEMORY, .message = "allocation failed"};
    return nullptr;
  }
  bool valid = true;
  size_t bad = 0, used = 0;
  for (size_t i = 0; i < o->count && valid; ++i) {
    bad = i;
    RillObject *stage = o->values[i].as.object;
    // Mutable owners stay separate from the borrowed const launch views.
    stages[i].argv = args + used;
    stages[i].redirects = redirs + used;
    for (size_t a = 0; a < stage->count; ++a) {
      RillValue v = stage->values[a];
      if (v.kind != RILL_V_REDIRECT) {
        if (!scalar(v, &args[used + stages[i].argc++])) {
          valid = false;
          break;
        }
      } else {
        RillExecRedirect *r = &redirs[used + stages[i].redirect_count++];
        int64_t tag = v.as.object->tag;
        r->target = tag == RILL_PLAN_INPUT
                        ? 0
                        : (tag >= RILL_PLAN_ERROR_OUTPUT ? 2 : 1);
        r->append = tag == RILL_PLAN_APPEND || tag == RILL_PLAN_ERROR_APPEND;
        r->duplicate_stdout = tag == RILL_PLAN_ERROR_TO_OUTPUT;
        if (!r->duplicate_stdout && !scalar(v.as.object->values[0], &r->path)) {
          valid = false;
          break;
        }
      }
    }
    used += stage->count;
  }
  RillJob *job = nullptr;
  if (valid) {
    RillExecSpec spec = {.stages = stages,
                         .count = o->count,
                         .environment = l->environment,
                         .background = background};
    job = rill_exec_launch(l->exec, &spec, error);
  } else if (!error->kind)
    *error = (RillDiagnostic){
        .kind = RILL_TYPE,
        .stage = bad,
        .message = "command arguments and paths require String, Bytes or Path"};
  free(args);
  free(redirs);
  free(stages);
  return job;
}
static RillValue report(RillLibrary *l, RillJob *job) {
  RillHeap *h = rill_runtime_heap(l->eval);
  size_t n;
  const RillExecStatus *s = rill_exec_status(job, &n);
  RillValue *values = calloc(n, sizeof(*values));
  if (!values)
    return (RillValue){};
  RillRoot root;
  rill_runtime_root(h, &root, values, n);
  bool ok = true;
  for (size_t i = 0; i < n; ++i) {
    RillValue fields[] = {
        {.kind = RILL_V_INT, .as.integer = s[i].status},
        {.kind = RILL_V_INT, .as.integer = s[i].signaled},
        {.kind = RILL_V_INT, .as.integer = s[i].expected_pipe}};
    values[i] = rill_runtime_object(h, RILL_V_LIST, fields, 3, nullptr, 0, 0);
    if (values[i].kind == RILL_V_UNIT) {
      ok = false;
      break;
    }
  }
  RillValue v = {};
  if (ok)
    v = rill_runtime_object(h, RILL_V_REPORT, values, n, nullptr, 0,
                            rill_exec_result(job));
  rill_runtime_unroot(h, &root);
  free(values);
  return v;
}
bool rill_library_progress(RillLibrary *l, RillNativePending *p) {
  RillJob *j = p->job;
  if (!j)
    return true;
  RillJobState state = rill_exec_state(j);
  RillDiagnostic d = rill_exec_error(j);
  if (p->operation == N_START && rill_exec_launched(j) && !d.kind) {
    p->job = nullptr;
    return finish(
        l,
        (RillValue){.kind = RILL_V_JOB, .as.integer = (int64_t)rill_exec_id(j)},
        d);
  }
  if (state != RILL_JOB_COMPLETED && state != RILL_JOB_STOPPED)
    return false;
  p->job = nullptr;
  if (state == RILL_JOB_STOPPED)
    return fail(l, RILL_PROCESS,
                "job stopped; use jobs() and fg(handle) or bg(handle)");
  rill_exec_acknowledge(j);
  if (d.kind)
    return finish(l, (RillValue){}, d);
  if (p->operation == N_WAIT) {
    RillValue v = report(l, j);
    if (v.kind == RILL_V_UNIT)
      return fail(l, RILL_MEMORY, "report allocation failed");
    return finish(l, v, d);
  }
  if (p->operation != N_CANCEL && rill_exec_result(j))
    return finish(
        l, (RillValue){},
        (RillDiagnostic){
            .kind = rill_exec_cancelled(j) ? RILL_CANCELLED : RILL_PROCESS,
            .code = rill_exec_result(j),
            .message = "external pipeline did not complete successfully"});
  return finish(l, (RillValue){}, d);
}
bool rill_library_call(RillLibrary *l, RillNativePending *p,
                       RillEvalEvent event) {
  RillValue v = event.value;
  RillHeap *h = rill_runtime_heap(l->eval);
  RillDiagnostic d = {};
  switch (event.native) {
  case N_RUN:
  case N_START:
    rill_library_collect(l);
    if (v.kind != RILL_V_PLAN)
      return fail(l, RILL_TYPE, "expected JobPlan");
    p->job = launch(l, v, event.native == N_START, &d);
    p->operation = event.native;
    if (!p->job)
      return finish(l, (RillValue){}, d);
    return false;
  case N_WAIT:
  case N_FG:
  case N_BG:
  case N_CANCEL: {
    if (v.kind != RILL_V_JOB)
      return fail(l, RILL_TYPE, "expected session Job handle");
    RillJob *j = rill_exec_find(l->exec, (size_t)v.as.integer);
    if (!j)
      return fail(l, RILL_TYPE, "invalid session Job handle");
    if (event.native == N_BG || event.native == N_FG) {
      if (!rill_exec_resume(l->exec, j, event.native == N_FG))
        return finish(l, (RillValue){},
                      (RillDiagnostic){.kind = RILL_IO,
                                       .code = errno,
                                       .message = "cannot resume job"});
      if (event.native == N_BG)
        return finish(l, (RillValue){}, d);
    }
    if (event.native == N_CANCEL)
      rill_exec_cancel(j);
    p->job = j;
    p->operation = event.native;
    return rill_library_progress(l, p);
  }
  case N_JOBS: {
    if (v.kind != RILL_V_UNIT)
      return fail(l, RILL_TYPE, "jobs expects Unit");
    RillValue handles[RILL_EXEC_MAX_JOBS] = {};
    size_t n = 0;
    RillRoot root;
    rill_runtime_root(h, &root, handles, 0);
    for (RillJob *j = rill_exec_first(l->exec); j; j = rill_exec_next(j)) {
      size_t id = rill_exec_id(j);
      RillValue fields[] = {
          {.kind = RILL_V_INT, .as.integer = (int64_t)id},
          {.kind = RILL_V_JOB, .as.integer = (int64_t)id},
          {.kind = RILL_V_INT, .as.integer = rill_exec_state(j)}};
      handles[n] = rill_runtime_object(h, RILL_V_RECORD, fields, 3,
                                       "id\0handle\0state\0", 16, 0);
      if (handles[n++].kind == RILL_V_UNIT) {
        rill_runtime_unroot(h, &root);
        return fail(l, RILL_MEMORY, "allocation failed");
      }
      root.count = n;
    }
    RillValue list =
        rill_runtime_object(h, RILL_V_LIST, handles, n, nullptr, 0, 0);
    rill_runtime_unroot(h, &root);
    if (list.kind == RILL_V_UNIT)
      return fail(l, RILL_MEMORY, "allocation failed");
    return finish(l, list, d);
  }
  case N_CD: {
    RillBytes path;
    if (!scalar(v, &path) || memchr(path.data, 0, path.size))
      return fail(l, RILL_TYPE, "cd expects a path without NUL");
    bool changed = rill_platform_cd(l->environment, path.data, &d);
    (void)changed;
    return finish(l, (RillValue){}, d);
  }
  case N_PWD: {
    if (v.kind != RILL_V_UNIT)
      return fail(l, RILL_TYPE, "pwd expects Unit");
    char *path = getcwd(nullptr, 0);
    if (!path)
      return finish(
          l, (RillValue){},
          (RillDiagnostic){.kind = RILL_IO,
                           .code = errno,
                           .message = "cannot read working directory"});
    RillValue out =
        rill_runtime_object(h, RILL_V_PATH, nullptr, 0, path, strlen(path), 0);
    free(path);
    if (out.kind == RILL_V_UNIT)
      return fail(l, RILL_MEMORY, "allocation failed");
    return finish(l, out, d);
  }
  case N_EXIT:
  case N_EXIT_FORCE:
    if (v.kind != RILL_V_INT || v.as.integer < 0 || v.as.integer > 255)
      return fail(l, RILL_TYPE, "exit expects an integer from 0 through 255");
    if (event.native == N_EXIT && rill_exec_outstanding(l->exec, false))
      return fail(l, RILL_PROCESS,
                  "live jobs remain; wait, cancel, or exit_force");
    l->exit_requested = true;
    l->exit_code = (int)v.as.integer;
    return finish(l, (RillValue){}, d);
  case N_PATH: {
    RillBytes bytes;
    if (!scalar(v, &bytes) || memchr(bytes.data, 0, bytes.size))
      return fail(l, RILL_TYPE, "path expects String or Bytes without NUL");
    RillValue out = rill_runtime_object(h, RILL_V_PATH, nullptr, 0, bytes.data,
                                        bytes.size, 0);
    if (out.kind == RILL_V_UNIT)
      return fail(l, RILL_MEMORY, "allocation failed");
    return finish(l, out, d);
  }
  case N_BYTES: {
    if (v.kind != RILL_V_LIST)
      return fail(l, RILL_TYPE,
                  "bytes expects a List of integers from 0 through 255");
    [[gnu::cleanup(rill_text_clear)]] RillBuffer b = {};
    for (size_t i = 0; i < v.as.object->count; ++i) {
      RillValue x = v.as.object->values[i];
      if (x.kind != RILL_V_INT || x.as.integer < 0 || x.as.integer > 255) {
        return fail(l, RILL_TYPE, "byte outside 0 through 255");
      }
      unsigned char c = (unsigned char)x.as.integer;
      if (!rill_text_append(&b, &c, 1)) {
        return fail(l, RILL_MEMORY, "allocation failed");
      }
    }
    RillValue out =
        rill_runtime_object(h, RILL_V_BYTES, nullptr, 0, b.data, b.size, 0);
    if (out.kind == RILL_V_UNIT)
      return fail(l, RILL_MEMORY, "allocation failed");
    return finish(l, out, d);
  }
  case N_CHECK:
    if (v.kind != RILL_V_REPORT)
      return fail(l, RILL_TYPE, "check expects JobReport");
    if (v.as.object->tag)
      return finish(l, (RillValue){},
                    (RillDiagnostic){.kind = RILL_PROCESS,
                                     .code = (int)v.as.object->tag,
                                     .message = "unsuccessful job report"});
    return finish(l, (RillValue){}, d);
  default:
    return fail(l, RILL_TYPE, "unknown native function");
  }
}
bool rill_library_display(RillValue v, RillBuffer *out) {
  switch (v.kind) {
  case RILL_V_UNIT:
    return true;
  case RILL_V_INT:
    return rill_text_format(out, "%" PRId64, v.as.integer);
  case RILL_V_FUNCTION:
    return rill_text_append(out, "<Function>", 10);
  case RILL_V_JOB:
    return rill_text_format(out, "<Job %" PRId64 ">", v.as.integer);
  case RILL_V_STRING:
  case RILL_V_BYTES:
  case RILL_V_PATH:
    return rill_text_escape(out, v.as.object->bytes);
  case RILL_V_RECORD:
    return rill_text_format(out, "<Record %zu>", v.as.object->count);
  case RILL_V_LIST:
    return rill_text_format(out, "<List %zu>", v.as.object->count);
  case RILL_V_PLAN:
    return rill_text_format(out, "<JobPlan %zu stages>", v.as.object->count);
  case RILL_V_REPORT:
    return rill_text_format(out, "<JobReport status=%" PRId64 ">",
                            v.as.object->tag);
  case RILL_V_STAGE:
  case RILL_V_REDIRECT:
    return rill_text_append(out, "<internal>", 10);
  }
  return false;
}

static void retain_job(RillExec *exec, RillValue value) {
  if (value.kind == RILL_V_JOB)
    rill_exec_retain(exec, (size_t)value.as.integer);
}
void rill_library_collect(RillLibrary *l) {
  RillHeap *heap = rill_runtime_heap(l->eval);
  // Only reachable heap objects may retain reports after this collection.
  rill_runtime_collect(heap);
  rill_exec_unmark(l->exec);
  for (RillRoot *root = heap->roots; root; root = root->previous)
    for (size_t i = 0; i < root->count; ++i)
      retain_job(l->exec, root->values[i]);
  for (RillObject *object = heap->objects; object; object = object->next)
    for (size_t i = 0; i < object->count; ++i)
      retain_job(l->exec, object->values[i]);
  rill_exec_prune(l->exec);
}
