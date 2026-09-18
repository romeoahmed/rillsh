/**
 * @file
 * @brief Bridge unary evaluator requests to session and process services.
 *
 * Translate rooted JobPlans into owned launch data, construct reports, and
 * route stream work through its cooperative adapter. Pending operations resume
 * through the session; display never evaluates a function or launches a plan.
 */
#include "library.h"
#include "diagnostic.h"
#include "exec/exec.h"
#include "fs.h"
#include "json.h"
#include "platform/posix.h"
#include "pure.h"
#include "runtime/runtime.h"
#include "stream.h"
#include "text/text.h"
#include "value.h"
#include <assert.h>
#include <errno.h>
#include <inttypes.h>
#include <stdckdint.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
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
  N_CHECK,
  N_PURE,
  N_RAISE,
  N_PROCESS,
  N_DATA,
  N_STREAM,
  N_CAPTURE,
  N_PRESENT
};
static const RillNative natives[] = {{"__data", N_DATA},
                                     {"__stream", N_STREAM},
                                     {"__pure", N_PURE},
                                     {"raise", N_RAISE},
                                     {"__process", N_PROCESS},
                                     {"run", N_RUN},
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
static bool sequence(RillValue v) {
  return v.kind == RILL_V_LIST || v.kind == RILL_V_SLICE;
}
static bool os_scalar(RillValue v, RillBytes *out) {
  return scalar(v, out) && !memchr(out->data, 0, out->size);
}
static bool env_name(RillValue v) {
  return v.kind == RILL_V_STRING && v.as.object->bytes.size &&
         !memchr(v.as.object->bytes.data, 0, v.as.object->bytes.size) &&
         !memchr(v.as.object->bytes.data, '=', v.as.object->bytes.size);
}
static bool process_value(RillLibrary *l, RillValue request) {
  RillHeap *h = rill_runtime_heap(l->eval);
  if (!sequence(request) || !rill_runtime_count(request))
    return fail(l, RILL_TYPE, "invalid process request");
  RillValue key = rill_runtime_at(request, 0);
  if (key.kind != RILL_V_STRING)
    return fail(l, RILL_TYPE, "invalid process operation");
  const char *op = key.as.object->bytes.data;
  size_t count = rill_runtime_count(request);
  RillValue a = count > 1 ? rill_runtime_at(request, 1) : (RillValue){},
            b = count > 2 ? rill_runtime_at(request, 2) : (RillValue){},
            out = {};
  if (!strcmp(op, "args")) {
    out = rill_runtime_object(h, RILL_V_LIST, nullptr, l->argument_count,
                              nullptr, 0, 0);
    if (out.kind == RILL_V_UNIT)
      return fail(l, RILL_MEMORY, "allocation failed");
    RillRoot root = {};
    rill_runtime_root(h, &root, &out, 1);
    RillValue *items = out.as.object->values;
    bool ok = true;
    for (size_t i = 0; i < l->argument_count; ++i) {
      items[i] =
          rill_runtime_object(h, RILL_V_BYTES, nullptr, 0, l->arguments[i],
                              strlen(l->arguments[i]), 0);
      if (items[i].kind == RILL_V_UNIT) {
        ok = false;
        break;
      }
    }
    if (!ok)
      out = (RillValue){};
    rill_runtime_unroot(h, &root);
  } else if (!strcmp(op, "get_env") || !strcmp(op, "set_env") ||
             !strcmp(op, "unset_env")) {
    if (!env_name(a))
      return fail(l, RILL_TYPE,
                  "environment name requires nonempty String without NUL or =");
    if (!strcmp(op, "get_env")) {
      const char *value =
          rill_platform_env_get(l->environment, a.as.object->bytes.data);
      RillValue bytes = {};
      RillRoot root = {};
      rill_runtime_root(h, &root, &bytes, 1);
      if (value)
        bytes = rill_runtime_object(h, RILL_V_BYTES, nullptr, 0, value,
                                    strlen(value), 0);
      if (!value || bytes.kind != RILL_V_UNIT)
        out = rill_runtime_object(h, RILL_V_LIST, &bytes, value ? 1 : 0,
                                  nullptr, 0, 0);
      rill_runtime_unroot(h, &root);
    } else {
      RillBytes value = {};
      bool unset = !strcmp(op, "unset_env");
      if (!unset && ((b.kind != RILL_V_STRING && b.kind != RILL_V_BYTES) ||
                     !os_scalar(b, &value)))
        return fail(l, RILL_TYPE,
                    "environment value requires String or Bytes without NUL");
      if (!rill_platform_env_set(l->environment, a.as.object->bytes.data,
                                 unset ? nullptr : value.data))
        return finish(
            l, (RillValue){},
            (RillDiagnostic){.kind = errno == ENOMEM ? RILL_MEMORY : RILL_IO,
                             .code = errno,
                             .message = "cannot update environment"});
      return finish(l, (RillValue){}, (RillDiagnostic){});
    }
  } else if (!strcmp(op, "command")) {
    RillBytes executable = {};
    if (!os_scalar(a, &executable) || !executable.size)
      return finish(
          l, (RillValue){},
          (RillDiagnostic){
              .kind = RILL_TYPE,
              .has_argument = true,
              .message = "executable requires nonempty NUL-free scalar bytes"});
    if (!sequence(b))
      return fail(l, RILL_TYPE, "command arguments require List");
    if (rill_runtime_count(b) >= RILL_EXEC_MAX_ARGUMENTS)
      return fail(l, RILL_LIMIT, "command argument limit exceeded");
    size_t n = rill_runtime_count(b) + 1;
    RillValue stage =
        rill_runtime_object(h, RILL_V_STAGE, nullptr, n, nullptr, 0, 0);
    if (stage.kind == RILL_V_UNIT)
      return fail(l, RILL_MEMORY, "allocation failed");
    RillValue *items = stage.as.object->values;
    items[0] = a;
    for (size_t i = 1; i < n; ++i) {
      items[i] = rill_runtime_at(b, i - 1);
      RillBytes bytes = {};
      if (!os_scalar(items[i], &bytes)) {
        return finish(
            l, (RillValue){},
            (RillDiagnostic){
                .kind = RILL_TYPE,
                .has_argument = true,
                .argument = i,
                .message = "command arguments require NUL-free scalar bytes"});
      }
    }
    RillRoot root = {};
    rill_runtime_root(h, &root, &stage, 1);
    out = rill_runtime_object(h, RILL_V_PLAN, &stage, 1, nullptr, 0, 0);
    rill_runtime_unroot(h, &root);
  } else if (!strcmp(op, "pipe")) {
    if (a.kind != RILL_V_PLAN || b.kind != RILL_V_PLAN)
      return fail(l, RILL_TYPE, "pipe requires plans");
    size_t n = {};
    if (ckd_add(&n, a.as.object->count, b.as.object->count) ||
        n > RILL_EXEC_MAX_STAGES)
      return fail(l, RILL_LIMIT, "pipeline limit exceeded");
    out = rill_runtime_object(h, RILL_V_PLAN, nullptr, n, nullptr, 0, 0);
    if (out.kind == RILL_V_UNIT)
      return fail(l, RILL_MEMORY, "allocation failed");
    RillValue *stages = out.as.object->values;
    memcpy(stages, a.as.object->values, a.as.object->count * sizeof(*stages));
    memcpy(stages + a.as.object->count, b.as.object->values,
           b.as.object->count * sizeof(*stages));
  } else if (!strcmp(op, "with_cwd") || !strcmp(op, "with_env") ||
             !strcmp(op, "accept_exit")) {
    if (b.kind != RILL_V_PLAN)
      return fail(l, RILL_TYPE, "override requires JobPlan");
    bool cwd = !strcmp(op, "with_cwd"), env = !strcmp(op, "with_env");
    RillValue override = a;
    RillRoot root = {};
    rill_runtime_root(h, &root, &override, 1);
    if (cwd) {
      RillBytes path = {};
      if (!os_scalar(a, &path)) {
        rill_runtime_unroot(h, &root);
        return fail(l, RILL_TYPE, "cwd requires NUL-free path");
      }
      char *absolute = realpath(path.data, nullptr);
      if (!absolute) {
        rill_runtime_unroot(h, &root);
        return finish(l, (RillValue){},
                      (RillDiagnostic){.kind = RILL_IO,
                                       .code = errno,
                                       .message = "cannot resolve plan cwd"});
      }
      struct stat st = {};
      if (stat(absolute, &st) < 0 || !S_ISDIR(st.st_mode)) {
        free(absolute);
        rill_runtime_unroot(h, &root);
        return fail(l, RILL_IO, "plan cwd must be an existing directory");
      }
      override = rill_runtime_object(h, RILL_V_PATH, nullptr, 0, absolute,
                                     strlen(absolute), 0);
      free(absolute);
    } else if (env) {
      if (a.kind != RILL_V_RECORD) {
        rill_runtime_unroot(h, &root);
        return fail(l, RILL_TYPE, "environment override requires Record");
      }
      for (size_t i = 0; i < a.as.object->count; i += 2) {
        RillBytes value = {};
        RillValue item = a.as.object->values[i + 1];
        if (!env_name(a.as.object->values[i]) ||
            (item.kind != RILL_V_STRING && item.kind != RILL_V_BYTES) ||
            !os_scalar(item, &value)) {
          rill_runtime_unroot(h, &root);
          return fail(l, RILL_TYPE, "invalid environment override");
        }
      }
    } else {
      if (!sequence(a) || !rill_runtime_count(a)) {
        rill_runtime_unroot(h, &root);
        return fail(l, RILL_TYPE, "accepted codes require nonempty List");
      }
      for (size_t i = 0; i < rill_runtime_count(a); ++i) {
        RillValue code = rill_runtime_at(a, i);
        if (code.kind != RILL_V_INT || code.as.integer < 0 ||
            code.as.integer > 255) {
          rill_runtime_unroot(h, &root);
          return fail(l, RILL_TYPE, "exit code outside 0 through 255");
        }
      }
    }
    if (override.kind != RILL_V_UNIT) {
      out = rill_runtime_object(h, RILL_V_PLAN, b.as.object->values,
                                b.as.object->count, nullptr, 0, 0);
      RillRoot plan_root = {};
      rill_runtime_root(h, &plan_root, &out, 1);
      if (out.kind != RILL_V_UNIT)
        for (size_t i = 0; i < out.as.object->count; ++i) {
          if (!cwd && !env && i + 1 != out.as.object->count)
            continue;
          RillObject *stage = b.as.object->values[i].as.object;
          RillValue fields[3] = {};
          static const char *const keys[] = {"cwd", "env", "codes"};
          for (size_t j = 0; j < 3; ++j) {
            bool found = rill_runtime_field(
                stage->metadata, (RillBytes){keys[j], strlen(keys[j])},
                &fields[j]);
            (void)found;
          }
          if (env && fields[1].kind == RILL_V_RECORD) {
            RillObject *old = fields[1].as.object;
            size_t n = {};
            if (ckd_add(&n, old->count, override.as.object->count)) {
              out = (RillValue){};
              break;
            }
            for (size_t j = 0; j < override.as.object->count; j += 2)
              if (rill_runtime_field_index(
                      fields[1],
                      override.as.object->values[j].as.object->bytes) <
                  old->count)
                n -= 2;
            fields[1] = rill_runtime_record(h, nullptr, n);
            if (fields[1].kind == RILL_V_UNIT) {
              out = (RillValue){};
              break;
            }
            // The request roots both inputs; finishing this private builder
            // cannot collect. rill_library_record roots the result on entry.
            RillValue *pairs = fields[1].as.object->values;
            size_t used = 0;
            for (size_t j = 0; j + 1 < old->count; j += 2) {
              RillValue ignored = {};
              if (!rill_runtime_field(override, old->values[j].as.object->bytes,
                                      &ignored)) {
                assert(used <= n && n - used >= 2);
                pairs[used++] = old->values[j];
                pairs[used++] = old->values[j + 1];
              }
            }
            memcpy(pairs + used, override.as.object->values,
                   override.as.object->count * sizeof(*pairs));
            used += override.as.object->count;
            assert(used == n);
            if (!rill_runtime_record_finish(fields[1])) {
              out = (RillValue){};
              break;
            }
          } else
            fields[cwd ? 0 : env ? 1 : 2] = override;
          RillValue meta = rill_library_record(h, keys, fields, 3);
          RillRoot mr = {};
          rill_runtime_root(h, &mr, &meta, 1);
          RillValue copy =
              meta.kind != RILL_V_UNIT
                  ? rill_runtime_object(h, RILL_V_STAGE, stage->values,
                                        stage->count, nullptr, 0, 0)
                  : (RillValue){};
          if (copy.kind != RILL_V_UNIT) {
            copy.as.object->metadata = meta;
            out.as.object->values[i] = copy;
          } else
            out = (RillValue){};
          rill_runtime_unroot(h, &mr);
          if (out.kind == RILL_V_UNIT)
            break;
        }
      rill_runtime_unroot(h, &plan_root);
    }
    rill_runtime_unroot(h, &root);
  } else
    return fail(l, RILL_TYPE, "unknown process operation");
  if (out.kind == RILL_V_UNIT)
    return fail(l, RILL_MEMORY, "allocation failed");
  return finish(l, out, (RillDiagnostic){});
}
RillJob *rill_library_launch(RillLibrary *l, RillValue plan, RillExecSpec mode,
                             RillDiagnostic *error) {
  rill_library_collect(l);
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
  RillEnvironment *environments = calloc(o->count, sizeof(*environments));
  bool (*policies)[256] = calloc(o->count, sizeof(*policies));
  bool valid = environments && policies;
  if (!valid)
    *error =
        (RillDiagnostic){.kind = RILL_MEMORY, .message = "allocation failed"};
  size_t bad = 0, used = 0;
  for (size_t i = 0; i < o->count && valid; ++i) {
    bad = i;
    RillObject *stage = o->values[i].as.object;
    RillValue option = {};
    if (rill_runtime_field(stage->metadata, (RillBytes){"cwd", 3}, &option) &&
        option.kind == RILL_V_PATH)
      stages[i].cwd = option.as.object->bytes.data;
    if (rill_runtime_field(stage->metadata, (RillBytes){"codes", 5}, &option) &&
        sequence(option)) {
      for (size_t c = 0; c < rill_runtime_count(option); ++c)
        policies[i][rill_runtime_at(option, c).as.integer] = true;
      stages[i].accepted_codes = policies[i];
    }
    if (rill_runtime_field(stage->metadata, (RillBytes){"env", 3}, &option) &&
        option.kind == RILL_V_RECORD) {
      valid = rill_platform_env_init(&environments[i], l->environment->entries);
      for (size_t c = 0; c < option.as.object->count && valid; c += 2)
        valid = rill_platform_env_set(
            &environments[i], option.as.object->values[c].as.object->bytes.data,
            option.as.object->values[c + 1].as.object->bytes.data);
      if (!valid) {
        *error = (RillDiagnostic){.kind = RILL_MEMORY,
                                  .message = "cannot prepare environment"};
        break;
      }
      stages[i].environment = &environments[i];
    }
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
    RillExecSpec spec = mode;
    spec.stages = stages;
    spec.count = o->count;
    spec.environment = l->environment;
    job = rill_exec_launch(l->exec, &spec, error);
  } else if (!error->kind)
    *error = (RillDiagnostic){
        .kind = RILL_TYPE,
        .stage = bad,
        .message = "command arguments and paths require String, Bytes or Path"};
  if (environments)
    for (size_t i = 0; i < o->count; ++i)
      rill_platform_env_clear(&environments[i]);
  free(environments);
  free(policies);
  free(args);
  free(redirs);
  free(stages);
  return job;
}
static RillValue nominal(RillLibrary *l, const char *type, const char *variant,
                         RillValue payload) {
  RillValue constructor = {};
  if (!rill_runtime_builtin(l->eval, type, &constructor))
    return (RillValue){};
  if (variant &&
      !rill_runtime_field(constructor, (RillBytes){variant, strlen(variant)},
                          &constructor))
    return (RillValue){};
  if (constructor.kind == RILL_V_ADT)
    return constructor;
  if (constructor.kind != RILL_V_CONSTRUCTOR)
    return (RillValue){};
  RillValue values[] = {constructor.as.object->values[0], payload};
  RillHeap *heap = rill_runtime_heap(l->eval);
  RillRoot root = {};
  rill_runtime_root(heap, &root, values, 2);
  RillValue out =
      rill_runtime_object(heap, RILL_V_ADT, values, 2, nullptr, 0, 0);
  rill_runtime_unroot(heap, &root);
  return out;
}
static RillValue tagged_field(RillLibrary *l, const char *type,
                              const char *variant, const char *key,
                              RillValue value) {
  RillHeap *heap = rill_runtime_heap(l->eval);
  RillValue record = rill_library_record(heap, &key, &value, 1);
  if (record.kind == RILL_V_UNIT)
    return record;
  return nominal(l, type, variant, record);
}
static RillValue report(RillLibrary *l, RillJob *job) {
  RillHeap *h = rill_runtime_heap(l->eval);
  size_t count = {};
  const RillExecStatus *statuses = rill_exec_status(job, &count);
  RillValue fields[4] = {
      {.kind = RILL_V_INT, .as.integer = (int64_t)rill_exec_id(job)}};
  RillRoot root = {};
  rill_runtime_root(h, &root, fields, 4);
  fields[1] =
      rill_runtime_object(h, RILL_V_LIST, nullptr, count, nullptr, 0, 0);
  if (fields[1].kind == RILL_V_UNIT) {
    rill_runtime_unroot(h, &root);
    return (RillValue){};
  }
  RillValue *stages = fields[1].as.object->values;
  bool valid = true;
  size_t failure = count;
  for (size_t i = 0; i < count; ++i) {
    const RillExecStatus *s = &statuses[i];
    RillValue values[4] = {
        {.kind = RILL_V_INT, .as.integer = (int64_t)i},
        {},
        {},
        {.kind = RILL_V_BOOL, .as.integer = s->expected_pipe}};
    RillRoot vr = {};
    rill_runtime_root(h, &vr, values, 4);
    values[1] =
        tagged_field(l, "Termination", s->signaled ? "Signaled" : "Exited",
                     s->signaled ? "signal" : "code",
                     (RillValue){.kind = RILL_V_INT, .as.integer = s->status});
    RillValue codes[256];
    size_t used = 0;
    for (size_t code = 0; code < 256; ++code)
      if (s->accepted_codes[code])
        codes[used++] =
            (RillValue){.kind = RILL_V_INT, .as.integer = (int64_t)code};
    values[2] = rill_runtime_object(h, RILL_V_LIST, codes, used, nullptr, 0, 0);
    static const char *const keys[] = {"index", "termination", "accepted_codes",
                                       "expected_cutoff"};
    if (values[1].kind != RILL_V_UNIT && values[2].kind != RILL_V_UNIT)
      stages[i] = rill_library_record(h, keys, values, 4);
    rill_runtime_unroot(h, &vr);
    if (stages[i].kind == RILL_V_UNIT) {
      valid = false;
      break;
    }
    if (!s->expected_pipe && (s->signaled || s->status < 0 || s->status > 255 ||
                              !s->accepted_codes[s->status]))
      failure = i;
  }
  if (valid) {
    if (rill_exec_cancelled(job) || rill_exec_cutoff_requested(job)) {
      const char *why =
          rill_exec_cancelled(job) ? "cancelled" : "consumer cutoff";
      RillValue reason = rill_runtime_object(h, RILL_V_STRING, nullptr, 0, why,
                                             strlen(why), 0);
      if (reason.kind != RILL_V_UNIT)
        fields[2] = tagged_field(
            l, "Completion", rill_exec_cancelled(job) ? "Cancelled" : "Cutoff",
            "reason", reason);
    } else
      fields[2] = nominal(l, "Completion", "Finished", (RillValue){});
    fields[3] = failure == count
                    ? nominal(l, "Option", "None", (RillValue){})
                    : tagged_field(l, "Option", "Some", "value",
                                   (RillValue){.kind = RILL_V_INT,
                                               .as.integer = (int64_t)failure});
  }
  RillValue out = {};
  if (valid && fields[2].kind != RILL_V_UNIT && fields[3].kind != RILL_V_UNIT) {
    static const char *const keys[] = {"id", "stages", "completion", "failure"};
    RillValue payload = rill_library_record(h, keys, fields, 4);
    if (payload.kind != RILL_V_UNIT)
      out = nominal(l, "JobReport", nullptr, payload);
  }
  rill_runtime_unroot(h, &root);
  return out;
}
static bool check_report(RillLibrary *l, RillValue value) {
  RillValue constructor = {}, completion = {}, failure = {};
  if (!rill_runtime_builtin(l->eval, "JobReport", &constructor) ||
      value.kind != RILL_V_ADT || constructor.kind != RILL_V_CONSTRUCTOR ||
      value.as.object->values[0].as.object !=
          constructor.as.object->values[0].as.object ||
      !rill_runtime_field(value, (RillBytes){"completion", 10}, &completion) ||
      !rill_runtime_field(value, (RillBytes){"failure", 7}, &failure))
    return fail(l, RILL_TYPE, "check requires JobReport");
  RillValue finished = nominal(l, "Completion", "Finished", (RillValue){});
  RillValue completion_type = {}, cutoff = {};
  bool has_cutoff =
      rill_runtime_builtin(l->eval, "Completion", &completion_type) &&
      rill_runtime_field(completion_type, (RillBytes){"Cutoff", 6}, &cutoff);
  if (completion.kind != RILL_V_ADT ||
      (completion.as.object->values[0].as.object !=
           finished.as.object->values[0].as.object &&
       (!has_cutoff || completion.as.object->values[0].as.object !=
                           cutoff.as.object->values[0].as.object)))
    return fail(l, RILL_PROCESS, "job did not finish normally");
  RillValue none = nominal(l, "Option", "None", (RillValue){});
  if (failure.kind == RILL_V_ADT && failure.as.object->values[0].as.object ==
                                        none.as.object->values[0].as.object)
    return finish(l, (RillValue){}, (RillDiagnostic){});
  RillValue index = {}, stages = {}, termination = {}, status = {};
  int code = 1;
  if (rill_runtime_field(failure, (RillBytes){"value", 5}, &index) &&
      index.kind == RILL_V_INT && index.as.integer >= 0 &&
      rill_runtime_field(value, (RillBytes){"stages", 6}, &stages) &&
      sequence(stages) &&
      (uint64_t)index.as.integer < rill_runtime_count(stages) &&
      rill_runtime_field(rill_runtime_at(stages, (size_t)index.as.integer),
                         (RillBytes){"termination", 11}, &termination)) {
    bool signaled =
        rill_runtime_field(termination, (RillBytes){"signal", 6}, &status);
    if (signaled ||
        rill_runtime_field(termination, (RillBytes){"code", 4}, &status))
      if (status.kind == RILL_V_INT && status.as.integer >= 0 &&
          status.as.integer <= 255)
        code =
            signaled
                ? (status.as.integer > 127 ? 255 : 128 + (int)status.as.integer)
            : status.as.integer ? (int)status.as.integer
                                : 1;
  }
  return finish(
      l, (RillValue){},
      (RillDiagnostic){.kind = RILL_PROCESS,
                       .code = code,
                       .message = "unacceptable external stage termination"});
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
  if (p->operation == N_CAPTURE) {
    RillValue fields[3] = {};
    RillRoot root = {};
    rill_runtime_root(rill_runtime_heap(l->eval), &root, fields, 3);
    fields[2] = report(l, j);
    for (int i = 0; i < 2; ++i) {
      const RillBuffer *bytes = rill_exec_output(j, i + 1);
      fields[i] = rill_runtime_object(rill_runtime_heap(l->eval), RILL_V_BYTES,
                                      nullptr, 0, bytes->data, bytes->size, 0);
    }
    const char *keys[] = {"stdout", "stderr", "report"};
    RillValue result = {};
    if (fields[0].kind != RILL_V_UNIT && fields[1].kind != RILL_V_UNIT &&
        fields[2].kind != RILL_V_UNIT)
      result = rill_library_record(rill_runtime_heap(l->eval), keys, fields, 3);
    rill_runtime_unroot(rill_runtime_heap(l->eval), &root);
    return result.kind == RILL_V_UNIT
               ? fail(l, RILL_MEMORY, "capture allocation failed")
               : finish(l, result, d);
  }
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
  case N_PRESENT:
    return l->present && l->present(l->context, v)
               ? finish(l, (RillValue){}, d)
               : fail(l, RILL_IO, "cannot display stream item");
  case N_STREAM:
    return rill_stream_call(l, v);
  case N_DATA: {
    if (v.kind != RILL_V_LIST || !v.as.object->count ||
        v.as.object->values[0].kind != RILL_V_STRING)
      return fail(l, RILL_TYPE, "invalid data request");
    const char *op = v.as.object->values[0].as.object->bytes.data;
    RillValue a = v.as.object->count > 1 ? v.as.object->values[1]
                                         : (RillValue){},
              b = v.as.object->count > 2 ? v.as.object->values[2]
                                         : (RillValue){},
              out = {};
    if (!strcmp(op, "from_json") || !strcmp(op, "to_json"))
      out = rill_library_json(h, !strcmp(op, "to_json"), a, b, &d);
    else if (!strcmp(op, "read_text"))
      out = rill_library_read_text(h, a, b, &d);
    else if (!strcmp(op, "glob"))
      out = rill_library_glob(h, a, &d);
    else if (!strcmp(op, "display_path")) {
      if (a.kind != RILL_V_PATH)
        return fail(l, RILL_TYPE, "display_path requires Path");
      RillBytes bytes = a.as.object->bytes;
      [[gnu::cleanup(rill_text_clear)]] RillBuffer text = {};
      if (!rill_text_escape(&text, bytes))
        return fail(l, RILL_MEMORY, "path display allocation failed");
      out = rill_runtime_object(h, RILL_V_STRING, nullptr, 0, text.data,
                                text.size, 0);
      if (out.kind == RILL_V_UNIT)
        return fail(l, RILL_MEMORY, "path display allocation failed");
    } else if (!strcmp(op, "capture")) {
      RillLimit limit = {"max_bytes", (size_t)64 * 1024 * 1024, 0};
      if (!rill_library_limits(a, &limit, 1, &d))
        return finish(l, (RillValue){}, d);
      if (b.kind != RILL_V_PLAN)
        return fail(l, RILL_TYPE, "capture requires JobPlan");
      p->job = rill_library_launch(
          l, b, (RillExecSpec){.capture = true, .capture_limit = limit.value},
          &d);
      p->operation = N_CAPTURE;
      return p->job ? false : finish(l, (RillValue){}, d);
    } else
      return fail(l, RILL_TYPE, "unknown data operation");
    return finish(l, out, d);
  }
  case N_CAPTURE:
    return fail(l, RILL_TYPE, "invalid direct capture request");
  case N_PURE: {
    RillValue out = rill_library_pure(h, v, &d);
    return finish(l, out, d);
  }
  case N_RAISE: {
    RillValue kind = {}, message = {}, span = {}, notes = {}, expected = {};
    if (!rill_runtime_builtin(l->eval, "Error", &expected) ||
        expected.kind != RILL_V_CONSTRUCTOR || v.kind != RILL_V_ADT ||
        v.as.object->values[0].as.object !=
            expected.as.object->values[0].as.object ||
        !rill_runtime_field(v, (RillBytes){"kind", 4}, &kind) ||
        kind.kind != RILL_V_STRING ||
        !rill_runtime_field(v, (RillBytes){"message", 7}, &message) ||
        message.kind != RILL_V_STRING ||
        !rill_runtime_field(v, (RillBytes){"span", 4}, &span) ||
        (span.kind != RILL_V_NULL && span.kind != RILL_V_RECORD) ||
        !rill_runtime_field(v, (RillBytes){"notes", 5}, &notes) ||
        (notes.kind != RILL_V_LIST && notes.kind != RILL_V_SLICE))
      return fail(l, RILL_TYPE, "raise requires Error");
    for (size_t i = 0; i < rill_runtime_count(notes); ++i)
      if (rill_runtime_at(notes, i).kind != RILL_V_STRING)
        return fail(l, RILL_TYPE, "Error notes require Strings");
    return finish(
        l, v,
        (RillDiagnostic){.kind = RILL_TYPE,
                         .label_size = kind.as.object->bytes.size,
                         .message_size = message.as.object->bytes.size,
                         .message = message.as.object->bytes.data,
                         .label = kind.as.object->bytes.data});
  }
  case N_PROCESS:
    return process_value(l, v);
  case N_RUN:
  case N_START:
    if (v.kind != RILL_V_PLAN)
      return fail(l, RILL_TYPE, "expected JobPlan");
    p->job = rill_library_launch(
        l, v, (RillExecSpec){.background = event.native == N_START}, &d);
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
    RillControl action = event.native == N_FG     ? RILL_CONTROL_FG
                         : event.native == N_BG   ? RILL_CONTROL_BG
                         : event.native == N_WAIT ? RILL_CONTROL_WAIT
                                                  : RILL_CONTROL_CANCEL;
    if (l->control && l->control(l->context, action, v))
      return true;
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
    RillValue contexts =
        l->context_jobs ? l->context_jobs(l->context) : (RillValue){};
    if (l->context_jobs && contexts.kind == RILL_V_UNIT)
      return fail(l, RILL_MEMORY, "context snapshot allocation failed");
    RillRoot context_root = {};
    rill_runtime_root(h, &context_root, &contexts, 1);
    size_t n = contexts.kind == RILL_V_UNIT ? 0 : rill_runtime_count(contexts),
           count = n;
    for (RillJob *j = rill_exec_first(l->exec); j; j = rill_exec_next(j))
      if (!l->owned_job || !l->owned_job(l->context, j))
        ++count;
    RillValue list =
        rill_runtime_object(h, RILL_V_LIST, nullptr, count, nullptr, 0, 0);
    if (list.kind == RILL_V_UNIT) {
      rill_runtime_unroot(h, &context_root);
      return fail(l, RILL_MEMORY, "job snapshot allocation failed");
    }
    RillRoot root = {};
    rill_runtime_root(h, &root, &list, 1);
    RillValue *handles = list.as.object->values;
    for (size_t i = 0; i < n; ++i)
      handles[i] = rill_runtime_at(contexts, i);
    for (RillJob *j = rill_exec_first(l->exec); j; j = rill_exec_next(j)) {
      if (l->owned_job && l->owned_job(l->context, j))
        continue;
      size_t id = rill_exec_id(j);
      RillValue fields[4] = {{.kind = RILL_V_INT, .as.integer = (int64_t)id},
                             {.kind = RILL_V_JOB, .as.integer = (int64_t)id}};
      RillRoot fr = {};
      rill_runtime_root(h, &fr, fields, 4);
      static const char *const states[] = {"Launching", "Running", "Stopped",
                                           "Cancelling", "Completed"};
      const char *state = states[rill_exec_state(j)];
      fields[2] =
          rill_runtime_object(h, RILL_V_STRING, nullptr, 0, "external", 8, 0);
      fields[3] = rill_runtime_object(h, RILL_V_STRING, nullptr, 0, state,
                                      strlen(state), 0);
      static const char *const keys[] = {"id", "handle", "kind", "state"};
      handles[n] =
          fields[2].kind != RILL_V_UNIT && fields[3].kind != RILL_V_UNIT
              ? rill_library_record(h, keys, fields, 4)
              : (RillValue){};
      rill_runtime_unroot(h, &fr);
      if (handles[n++].kind == RILL_V_UNIT) {
        rill_runtime_unroot(h, &root);
        rill_runtime_unroot(h, &context_root);
        return fail(l, RILL_MEMORY, "allocation failed");
      }
    }
    rill_runtime_unroot(h, &root);
    rill_runtime_unroot(h, &context_root);
    return finish(l, list, d);
  }
  case N_CD: {
    RillBytes path = {};
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
    if (event.native == N_EXIT && l->control &&
        l->control(l->context, RILL_CONTROL_EXIT, v))
      return true;
    if (event.native == N_EXIT && rill_exec_outstanding(l->exec, false))
      return fail(l, RILL_PROCESS,
                  "live jobs remain; wait, cancel, or exit_force");
    l->exit_requested = true;
    l->exit_code = (int)v.as.integer;
    return finish(l, (RillValue){}, d);
  case N_PATH: {
    RillBytes bytes = {};
    if (!scalar(v, &bytes) || memchr(bytes.data, 0, bytes.size))
      return fail(l, RILL_TYPE, "path expects String or Bytes without NUL");
    RillValue out = rill_runtime_object(h, RILL_V_PATH, nullptr, 0, bytes.data,
                                        bytes.size, 0);
    if (out.kind == RILL_V_UNIT)
      return fail(l, RILL_MEMORY, "allocation failed");
    return finish(l, out, d);
  }
  case N_BYTES: {
    if (!sequence(v))
      return fail(l, RILL_TYPE,
                  "bytes expects a List of integers from 0 through 255");
    [[gnu::cleanup(rill_text_clear)]] RillBuffer b = {};
    for (size_t i = 0; i < rill_runtime_count(v); ++i) {
      RillValue x = rill_runtime_at(v, i);
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
    return check_report(l, v);
  }
  return fail(l, RILL_TYPE, "unknown native function");
}
bool rill_library_display(RillValue v, RillBuffer *out) {
  switch (v.kind) {
  case RILL_V_NULL:
    return rill_text_append(out, "null", 4);
  case RILL_V_BOOL:
    return rill_text_append(out, v.as.integer ? "true" : "false",
                            v.as.integer ? 4 : 5);
  case RILL_V_FLOAT:
    return rill_text_format(out, "%.17g", v.as.real);
  case RILL_V_CLOSURE:
  case RILL_V_CONSTRUCTOR:
    return rill_text_append(out, "<Function>", 10);
  case RILL_V_ADT:
    return rill_text_append(out, v.as.object->values[0].as.object->bytes.data,
                            v.as.object->values[0].as.object->bytes.size);
  case RILL_V_SLICE:
    return rill_text_format(out, "<List %zu>", rill_runtime_count(v));
  case RILL_V_CODE:
  case RILL_V_ENV:
  case RILL_V_BINDINGS:
  case RILL_V_CELL:
  case RILL_V_DESCRIPTOR:
  case RILL_V_BUDGET:
  case RILL_V_STREAM:
    return false;
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
    return rill_text_format(out, "<Record %zu>", v.as.object->count / 2);
  case RILL_V_LIST:
    return rill_text_format(out, "<List %zu>", v.as.object->count);
  case RILL_V_PLAN:
    return rill_text_format(out, "<JobPlan %zu stages>", v.as.object->count);
  case RILL_V_STAGE:
  case RILL_V_REDIRECT:
    return rill_text_append(out, "<internal>", 10);
  }
  return false;
}

static void retain_job(RillExec *exec, RillValue value) {
  if (value.kind == RILL_V_JOB && value.as.integer > 0)
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

bool rill_library_pending_owned(const RillNativePending *pending) {
  return pending->job && pending->operation != N_WAIT &&
         pending->operation != N_START;
}
void rill_library_present(RillLibrary *l, RillValue stream) {
  RillValue values[3] = {
      {}, {.kind = RILL_V_FUNCTION, .as.integer = N_PRESENT}, stream};
  RillRoot root = {};
  rill_runtime_root(rill_runtime_heap(l->eval), &root, values, 3);
  values[0] = rill_runtime_object(rill_runtime_heap(l->eval), RILL_V_STRING,
                                  nullptr, 0, "each", 4, 0);
  RillValue request = {};
  if (values[0].kind != RILL_V_UNIT)
    request = rill_runtime_object(rill_runtime_heap(l->eval), RILL_V_LIST,
                                  values, 3, nullptr, 0, 0);
  RillRoot request_root = {};
  rill_runtime_root(rill_runtime_heap(l->eval), &request_root, &request, 1);
  if (request.kind == RILL_V_UNIT)
    (void)fail(l, RILL_MEMORY, "display allocation failed");
  else
    (void)rill_stream_call(l, request);
  rill_runtime_unroot(rill_runtime_heap(l->eval), &request_root);
  rill_runtime_unroot(rill_runtime_heap(l->eval), &root);
}
