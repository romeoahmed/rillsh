/**
 * @file
 * @brief Cooperative evaluation over explicit, rooted continuations.
 *
 * Each frame retains scope, code, and initialized operands. Tail calls replace
 * frames; host effects yield events instead of nesting a C evaluator. Entry
 * bindings publish atomically, and cleanup checkpoints remain live until the
 * host has released their resources.
 *
 * @verbatim
 * step --> value / error / yield
 *      --> host request --> session service --> resume --> step
 *      --> callback     --> ordinary frames --> callback event
 * @endverbatim
 *
 * Suspension transfers frames and roots into a saved context. Abort discards
 * pending work and diagnostics while preserving committed bindings.
 */
#include "diagnostic.h"
#include "private.h"
#include "runtime.h"
#include "syntax/syntax.h"
#include "text/text.h"
#include <assert.h>
#include <math.h>
#include <stdckdint.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

void rill_eval_error(RillEval *e, RillError kind, const char *message) {
  if (!e->error.kind) {
    if (e->frame)
      e->roots[ERROR_CODE] = e->frame->values[OWNER];
    e->error = (RillDiagnostic){
        .kind = kind,
        .offset = e->frame && e->frame->node ? e->frame->node->offset : 0,
        .message = message};
  }
}
RillValue rill_eval_object(RillEval *e, RillValueKind kind, const RillValue *v,
                           size_t n, const char *s, size_t len) {
  if (e->error.kind)
    return (RillValue){};
  RillValue out = rill_runtime_object(&e->heap, kind, v, n, s, len, 0);
  if (out.kind == RILL_V_UNIT)
    rill_eval_error(e, RILL_MEMORY, "allocation failed");
  return out;
}
RillValue rill_eval_text(RillEval *e, const char *s, size_t n) {
  return rill_eval_object(e, RILL_V_STRING, nullptr, 0, s, n);
}
static bool same(RillBytes a, RillBytes b) {
  return a.size == b.size && (!a.size || !memcmp(a.data, b.data, a.size));
}
enum { SMALL_FRAME_VALUES = 16, SPARE_FRAMES = 32 };
static constexpr size_t MAX_CONTINUATIONS = 65'536;
static constexpr size_t MAX_STAGE_ITEMS = 65'536;
static void pop(RillEval *e) {
  Frame *f = e->frame;
  rill_runtime_unroot(&e->heap, &f->root);
  e->frame = f->parent;
  --e->depth;
  // Unregistered slots may contain stale bits, but never keep values alive.
  if (f->capacity == SMALL_FRAME_VALUES && e->spare_count < SPARE_FRAMES) {
    f->parent = e->spare_frames;
    e->spare_frames = f;
    ++e->spare_count;
  } else
    free(f);
}
static size_t frame_slots(RillEval *e, const RillNode *node) {
  size_t count = OPERANDS;
  // Only aggregates retain all operands. Sequential forms need fixed-size
  // frames regardless of syntax width, keeping them within the frame cache.
  if (node && (node->kind == RILL_LIST || node->kind == RILL_RECORD ||
               node->kind == RILL_ENUM || node->kind == RILL_PLAN ||
               node->kind == RILL_STAGE))
    for (const RillNode *c = node->children; c; c = c->next) {
      size_t slots =
          node->kind == RILL_RECORD || node->kind == RILL_ENUM ? 2 : 1;
      if (ckd_add(&count, count, slots)) {
        rill_eval_error(e, RILL_MEMORY, "frame size overflow");
        return 0;
      }
    }
  return count < SMALL_FRAME_VALUES ? SMALL_FRAME_VALUES : count;
}
static bool push(RillEval *e, const RillNode *node, RillValue env,
                 RillValue code) {
  if (e->depth == MAX_CONTINUATIONS) {
    rill_eval_error(e, RILL_LIMIT, "continuation limit exceeded");
    return false;
  }
  size_t count = frame_slots(e, node), bytes = {};
  if (!count)
    return false;
  if (ckd_mul(&bytes, count, sizeof(RillValue)) ||
      ckd_add(&bytes, bytes, sizeof(Frame))) {
    rill_eval_error(e, RILL_MEMORY, "frame size overflow");
    return false;
  }
  Frame *f = {};
  if (count == SMALL_FRAME_VALUES && e->spare_frames) {
    f = e->spare_frames;
    e->spare_frames = f->parent;
    --e->spare_count;
  } else
    f = malloc(bytes);
  if (!f) {
    rill_eval_error(e, RILL_MEMORY, "allocation failed");
    return false;
  }
  *f = (Frame){.parent = e->frame,
               .node = node,
               .next = node ? node->children : nullptr,
               .used = OPERANDS,
               .capacity = count};
  f->values[ENV] = env;
  f->values[OWNER] = code;
  rill_runtime_root(&e->heap, &f->root, f->values, OPERANDS);
  e->frame = f;
  ++e->depth;
  return true;
}
static void save(Frame *f, RillValue v) {
  assert(f->used < f->capacity);
  f->values[f->used++] = v;
  f->root.count = f->used;
}
static bool atomic_operand(RillEval *e, Frame *f, const RillNode *n) {
  if (e->error.kind)
    return true;
  RillValue value = {};
  if (n->kind == RILL_NAME) {
    if (!rill_eval_lookup(e, f->values[ENV], n->text.data, &value)) {
      rill_eval_error(e, RILL_TYPE, "unknown binding");
      e->error.offset = n->offset;
      return true;
    }
  } else if (n->kind == RILL_UNIT || n->kind == RILL_INTEGER ||
             n->kind == RILL_FLOAT || n->kind == RILL_BOOL ||
             n->kind == RILL_NULL || n->kind == RILL_STRING) {
    value = rill_eval_literal(n, f->values[OWNER]);
  } else
    return false;
  // Atomic operands cannot suspend or call user code; the parent roots their
  // scope and code. Store the result before the next safepoint.
  save(f, value);
  e->roots[RESULT] = (RillValue){};
  return true;
}
static void done(RillEval *e, RillValue v) {
  e->roots[RESULT] = v;
  pop(e);
  e->ready = e->frame != nullptr;
}
static void replace(RillEval *e, const RillNode *node, RillValue env,
                    RillValue code) {
  Frame *f = e->frame;
  size_t count = frame_slots(e, node);
  if (!count)
    return;
  if (count == f->capacity) {
    // Keep root registration stable. Reset control state and the live prefix;
    // discarded operands must not survive the next allocation or suspension.
    *f = (Frame){.parent = f->parent,
                 .node = node,
                 .next = node ? node->children : nullptr,
                 .used = OPERANDS,
                 .capacity = count,
                 .root = f->root};
    f->values[ENV] = env;
    f->values[OWNER] = code;
    f->root.count = OPERANDS;
  } else {
    // Shrink wide frames too: tail calls must not retain a wide high-water
    // mark. push() cannot collect while the old frame roots are absent.
    pop(e);
    (void)push(e, node, env, code);
  }
  e->ready = false;
  e->roots[RESULT] = (RillValue){};
}
static void publish(RillEval *e, Frame *f, RillValue env) {
  if (f->parent && f->parent->node && f->parent->node->kind == RILL_BLOCK)
    f->parent->values[ENV] = env;
  else
    e->roots[ENTRY] = env;
}
static RillValue numeric(RillEval *e, RillOperator op, RillValue a,
                         RillValue b) {
  if (op == RILL_OP_EQ || op == RILL_OP_NE) {
    bool eq = {};
    RillError status = rill_runtime_equal(a, b, &eq);
    if (status) {
      rill_eval_error(e, status, "equality requires data values");
      return (RillValue){};
    }
    return (RillValue){.kind = RILL_V_BOOL,
                       .as.integer = op == RILL_OP_EQ ? eq : !eq};
  }
  bool order = op == RILL_OP_LT || op == RILL_OP_GT || op == RILL_OP_LE ||
               op == RILL_OP_GE;
  if (a.kind != b.kind) {
    rill_eval_error(e, RILL_TYPE, "operands must have the same kind");
    return (RillValue){};
  }
  int cmp = 0;
  if (a.kind == RILL_V_STRING) {
    RillBytes x = a.as.object->bytes, y = b.as.object->bytes;
    if (op == RILL_OP_ADD) {
      [[gnu::cleanup(rill_text_clear)]] RillBuffer joined = {};
      bool ok = rill_text_append(&joined, x.data, x.size) &&
                rill_text_append(&joined, y.data, y.size);
      RillValue v =
          ok ? rill_eval_text(e, joined.data, joined.size) : (RillValue){};
      if (!ok)
        rill_eval_error(e, RILL_MEMORY, "allocation failed");
      return v;
    }
    if (!order) {
      rill_eval_error(e, RILL_TYPE, "invalid String operation");
      return (RillValue){};
    }
    size_t n = x.size < y.size ? x.size : y.size;
    cmp = n ? memcmp(x.data, y.data, n) : 0;
    if (!cmp)
      cmp = (x.size > y.size) - (x.size < y.size);
  } else if (a.kind == RILL_V_INT) {
    int64_t x = a.as.integer, y = b.as.integer, z = 0;
    bool overflow = false;
    cmp = (x > y) - (x < y);
    if (!order) {
      if (op == RILL_OP_ADD)
        overflow = ckd_add(&z, x, y);
      else if (op == RILL_OP_SUB)
        overflow = ckd_sub(&z, x, y);
      else if (op == RILL_OP_MUL)
        overflow = ckd_mul(&z, x, y);
      else {
        rill_eval_error(e, RILL_TYPE, "/ requires Float operands");
        return (RillValue){};
      }
      if (overflow)
        rill_eval_error(e, RILL_ARITHMETIC, "integer overflow");
      return (RillValue){.kind = RILL_V_INT, .as.integer = z};
    }
  } else if (a.kind == RILL_V_FLOAT) {
    double x = a.as.real, y = b.as.real, z = 0;
    cmp = (x > y) - (x < y);
    if (!order) {
      if (op == RILL_OP_ADD)
        z = x + y;
      else if (op == RILL_OP_SUB)
        z = x - y;
      else if (op == RILL_OP_MUL)
        z = x * y;
      else if (op == RILL_OP_DIV && y != 0)
        z = x / y;
      else {
        rill_eval_error(e, RILL_ARITHMETIC, "division by zero");
        return (RillValue){};
      }
      if (!isfinite(z))
        rill_eval_error(e, RILL_ARITHMETIC, "non-finite Float result");
      return (RillValue){.kind = RILL_V_FLOAT, .as.real = z};
    }
  } else {
    rill_eval_error(e, RILL_TYPE,
                    "operation requires numeric or String operands");
    return (RillValue){};
  }
  return (RillValue){.kind = RILL_V_BOOL,
                     .as.integer = op == RILL_OP_LT   ? cmp < 0
                                   : op == RILL_OP_GT ? cmp > 0
                                   : op == RILL_OP_LE ? cmp <= 0
                                                      : cmp >= 0};
}
static RillValue construct(RillEval *e, RillValue ctor, RillValue arg) {
  RillValue desc = ctor.as.object->values[0];
  if (arg.kind != RILL_V_RECORD ||
      arg.as.object->count != desc.as.object->count * 2) {
    rill_eval_error(e, RILL_TYPE,
                    "constructor requires its exact anonymous record shape");
    return (RillValue){};
  }
  for (size_t i = 0; i < desc.as.object->count; ++i) {
    RillValue ignored = {};
    if (!rill_runtime_field(arg, desc.as.object->values[i].as.object->bytes,
                            &ignored)) {
      rill_eval_error(e, RILL_TYPE, "constructor field mismatch");
      return (RillValue){};
    }
  }
  RillValue values[] = {desc, arg};
  return rill_eval_object(e, RILL_V_ADT, values, 2, nullptr, 0);
}
static RillValue type_case(RillEval *e, const RillNode *fields,
                           const char *name) {
  size_t n = 0;
  for (const RillNode *c = fields; c; c = c->next)
    ++n;
  RillValue desc =
      rill_eval_object(e, RILL_V_DESCRIPTOR, nullptr, n, name, strlen(name));
  if (e->error.kind)
    return (RillValue){};
  RillRoot root = {};
  rill_runtime_root(&e->heap, &root, &desc, 1);
  RillValue *keys = desc.as.object->values;
  size_t i = 0;
  for (const RillNode *c = fields; c && !e->error.kind; c = c->next) {
    for (size_t j = 0; j < i; ++j)
      if (same(keys[j].as.object->bytes,
               (RillBytes){c->text.data, c->text.size}))
        rill_eval_error(e, RILL_TYPE, "duplicate type field");
    keys[i++] = rill_eval_text(e, c->text.data, c->text.size);
  }
  RillValue v = {};
  if (!e->error.kind) {
    if (n)
      v = rill_eval_object(e, RILL_V_CONSTRUCTOR, &desc, 1, nullptr, 0);
    else {
      RillValue payload =
          rill_eval_object(e, RILL_V_RECORD, nullptr, 0, nullptr, 0);
      RillValue pair[] = {desc, payload};
      RillRoot pr = {};
      rill_runtime_root(&e->heap, &pr, pair, 2);
      v = rill_eval_object(e, RILL_V_ADT, pair, 2, nullptr, 0);
      rill_runtime_unroot(&e->heap, &pr);
    }
  }
  rill_runtime_unroot(&e->heap, &root);
  return v;
}
static RillValue variant(RillEval *e, const char *type, const char *case_name,
                         const char *field, RillValue value) {
  RillValue values[5] = {value};
  RillRoot root = {};
  rill_runtime_root(&e->heap, &root, values, 5);
  if (!rill_runtime_builtin(e, type, &values[1]) ||
      !rill_runtime_field(values[1], (RillBytes){case_name, strlen(case_name)},
                          &values[2])) {
    rill_eval_error(e, RILL_TYPE, "prelude type unavailable");
    goto done;
  }
  if (!field) {
    values[4] = values[2];
    goto done;
  }
  values[3] = rill_eval_text(e, field, strlen(field));
  RillValue pairs[] = {values[3], value};
  values[4] = rill_eval_object(e, RILL_V_RECORD, pairs, 2, nullptr, 0);
  if (!e->error.kind)
    values[4] = construct(e, values[2], values[4]);
done:
  rill_runtime_unroot(&e->heap, &root);
  return values[4];
}
static RillValue error_value(RillEval *e, RillDiagnostic d) {
  RillValue pairs[8] = {};
  RillRoot root = {};
  rill_runtime_root(&e->heap, &root, pairs, 8);
  static const char *const keys[] = {"kind", "message", "span", "notes"};
  for (size_t i = 0; i < 4; ++i)
    pairs[i * 2] = rill_eval_text(e, keys[i], strlen(keys[i]));
  pairs[1] = rill_eval_text(e, rill_diagnostic_name(d.kind),
                            strlen(rill_diagnostic_name(d.kind)));
  pairs[3] = rill_eval_text(e, d.message ? d.message : "failure",
                            strlen(d.message ? d.message : "failure"));
  pairs[5] = (RillValue){.kind = RILL_V_NULL};
  if (e->roots[ERROR_CODE].kind == RILL_V_CODE && d.offset <= INT64_MAX) {
    RillSyntax *origin = &e->roots[ERROR_CODE].as.object->code->syntax;
    RillValue location[4] = {};
    RillRoot sr = {};
    rill_runtime_root(&e->heap, &sr, location, 4);
    location[0] = rill_eval_text(e, "source", 6);
    location[1] = rill_eval_text(e, origin->name.data, origin->name.size);
    location[2] = rill_eval_text(e, "offset", 6);
    location[3] =
        (RillValue){.kind = RILL_V_INT, .as.integer = (int64_t)d.offset};
    pairs[5] = rill_eval_object(e, RILL_V_RECORD, location, 4, nullptr, 0);
    rill_runtime_unroot(&e->heap, &sr);
  }
  pairs[7] = rill_eval_object(e, RILL_V_LIST, nullptr, 0, nullptr, 0);
  RillValue record = rill_eval_object(e, RILL_V_RECORD, pairs, 8, nullptr, 0);
  RillValue ctor = {};
  if (!e->error.kind && rill_runtime_builtin(e, "Error", &ctor)) {
    RillRoot rr = {};
    rill_runtime_root(&e->heap, &rr, &record, 1);
    record = construct(e, ctor, record);
    rill_runtime_unroot(&e->heap, &rr);
  }
  rill_runtime_unroot(&e->heap, &root);
  return record;
}
static bool catch_error(RillEval *e, int64_t *checkpoint) {
  if (e->error.kind == RILL_MEMORY || e->error.kind == RILL_CANCELLED)
    return false;
  Frame *handler = e->frame;
  while (handler && handler->phase != ATTEMPT_WAIT)
    handler = handler->parent;
  if (!handler)
    return false;
  *checkpoint = handler->checkpoint;
  RillDiagnostic d = e->error;
  while (e->frame != handler)
    pop(e);
  e->error = (RillDiagnostic){};
  e->waiting = false;
  RillValue v = e->roots[RAISED].kind == RILL_V_ADT ? e->roots[RAISED]
                                                    : error_value(e, d);
  e->roots[RESULT] = v;
  if (!e->error.kind)
    v = variant(e, "Result", "Err", "error", v);
  e->roots[RAISED] = (RillValue){};
  done(e, v);
  return true;
}
static void call(RillEval *e, Frame *f, RillValue function_value,
                 RillValue argument) {
  if (function_value.kind == RILL_V_CLOSURE) {
    RillObject *o = function_value.as.object;
    const RillNode *pattern = o->captures->node->pattern;
    // A single parameter cannot duplicate a sibling binding. Blocks introduce
    // their own scope boundary; no empty environment is needed for this call.
    bool simple = pattern && pattern->kind == RILL_NAME;
    RillValue env =
        simple ? function_value : rill_eval_scope(e, function_value);
    RillRoot root = {};
    rill_runtime_root(&e->heap, &root, &env, 1);
    bool ok = {};
    if (simple) {
      env = rill_eval_bind(e, env, pattern->text.data, argument, false);
      ok = !e->error.kind;
    } else
      ok = rill_eval_bind_pattern(e, pattern, argument, function_value,
                                  o->values[0], &env);
    rill_runtime_unroot(&e->heap, &root);
    if (!ok) {
      if (!e->error.kind)
        rill_eval_error(e, RILL_MATCH_ERROR, "parameter pattern did not match");
      return;
    }
    replace(e, o->captures->node->children, env, o->values[0]);
    return;
  }
  if (function_value.kind == RILL_V_CONSTRUCTOR) {
    RillValue v = construct(e, function_value, argument);
    done(e, v);
    return;
  }
  if (function_value.kind != RILL_V_FUNCTION) {
    rill_eval_error(e, RILL_TYPE, "application requires a function");
    return;
  }
  if (function_value.as.integer == -1) {
    // The checkpoint remains while the thunk's own calls are tail-eliminated.
    f->phase = ATTEMPT_WAIT;
    f->checkpoint = e->resource_serial;
    f->next = nullptr;
    if (push(e, nullptr, f->values[ENV], f->values[OWNER])) {
      save(e->frame, argument);
      save(e->frame, (RillValue){});
    }
    return;
  }
  f->phase = CALL_WAIT;
  e->waiting = true;
}
static void export_bindings(RillEval *e, const RillNode *n, RillValue env) {
  if (!n->exported || (e->frame && e->frame->values[OWNER].as.object->tag))
    return;
  if (n->kind == RILL_BIND) {
    Bound *names = nullptr;
    if (rill_eval_pattern_names(e, n->pattern, &names))
      for (Bound *b = names; b; b = b->parent) {
        RillValue value = {};
        if (rill_eval_lookup(e, env, b->name, &value))
          e->roots[EXPORTS] =
              rill_eval_bind(e, e->roots[EXPORTS], b->name, value, false);
      }
    rill_eval_names_free(names);
  } else {
    RillValue value = {};
    if (rill_eval_lookup(e, env, n->text.data, &value))
      e->roots[EXPORTS] =
          rill_eval_bind(e, e->roots[EXPORTS], n->text.data, value, false);
  }
}
RillEval *rill_runtime_new(const RillNative *n, size_t count) {
  RillEval *e = malloc(sizeof(*e));
  if (!e)
    return nullptr;
  *e = (RillEval){.natives = n, .native_count = count};
  rill_runtime_root(&e->heap, &e->root, e->roots, ROOT_COUNT);
  if (!rill_runtime_define(
          e, "attempt",
          (RillValue){.kind = RILL_V_FUNCTION, .as.integer = -1})) {
    rill_runtime_free(e);
    return nullptr;
  }
  return e;
}
RillHeap *rill_runtime_heap(RillEval *e) { return &e->heap; }
size_t rill_runtime_depth(const RillEval *e) { return e->depth; }
bool rill_runtime_define(RillEval *e, const char *name, RillValue value) {
  RillValue next = rill_eval_bind(e, e->roots[GLOBAL], name, value, false);
  if (!e->error.kind)
    e->roots[GLOBAL] = next;
  return !e->error.kind;
}
bool rill_runtime_lookup(RillEval *e, const char *name, RillValue *value) {
  return rill_eval_lookup(e, e->roots[GLOBAL], name, value);
}
RillValue rill_runtime_exports(RillEval *e) {
  RillValue env = e->roots[EXPORTS];
  size_t count = 0;
  for (RillValue p = env; p.kind == RILL_V_ENV; p = p.as.object->values[0])
    count += 2;
  RillValue v = rill_eval_object(e, RILL_V_RECORD, nullptr, count, nullptr, 0);
  if (e->error.kind)
    return (RillValue){};
  RillRoot root = {};
  rill_runtime_root(&e->heap, &root, &v, 1);
  RillValue *pairs = v.as.object->values;
  size_t i = 0;
  for (RillValue p = env; p.kind == RILL_V_ENV && !e->error.kind;
       p = p.as.object->values[0]) {
    RillObject *o = p.as.object;
    pairs[i++] = rill_eval_text(e, o->bytes.data, o->bytes.size);
    RillValue value = o->values[1];
    if (value.kind == RILL_V_CELL)
      value = value.as.object->values[0];
    pairs[i++] = value;
  }
  if (!e->error.kind && !rill_runtime_record_finish(v))
    rill_eval_error(e, RILL_TYPE, "duplicate export name");
  if (e->error.kind)
    v = (RillValue){};
  rill_runtime_unroot(&e->heap, &root);
  e->roots[RESULT] = v;
  return v;
}
void rill_runtime_abort(RillEval *e) {
  while (e->frame)
    pop(e);
  for (size_t i = ENTRY; i < ROOT_COUNT; ++i)
    if (i != TYPES && i != PRELUDE && i != MODULES)
      e->roots[i] = (RillValue){};
  e->statement = nullptr;
  e->waiting = false;
  e->ready = false;
  e->error = (RillDiagnostic){};
  rill_runtime_collect(&e->heap);
}
void rill_runtime_begin(RillEval *e, RillSyntax *s) {
  rill_runtime_abort(e);
  e->roots[CODE] = rill_eval_code(e, s);
  if (e->error.kind)
    return;
  e->roots[ENTRY] = rill_eval_scope(e, e->roots[GLOBAL]);
  e->statement = e->roots[CODE].as.object->code->syntax.first;
  e->resource_boundary = e->resource_serial;
}
static void recursive(RillEval *e, Frame *f) {
  const RillNode *first =
      f->node->kind == RILL_REC ? f->node->children : f->node;
  RillValue env = f->values[ENV];
  save(f, env);
  for (const RillNode *n = first; n;
       n = f->node->kind == RILL_REC ? n->next : nullptr) {
    RillValue unit = {};
    RillValue cell = rill_eval_object(e, RILL_V_CELL, &unit, 1, nullptr, 0);
    RillRoot root = {};
    rill_runtime_root(&e->heap, &root, &cell, 1);
    f->values[OPERANDS] =
        rill_eval_bind(e, f->values[OPERANDS], n->text.data, cell, true);
    rill_runtime_unroot(&e->heap, &root);
    if (e->error.kind)
      return;
  }
  for (const RillNode *n = first; n;
       n = f->node->kind == RILL_REC ? n->next : nullptr) {
    RillValue cell = {};
    if (!rill_eval_lookup_env(f->values[OPERANDS], n->text.data, &cell)) {
      rill_eval_error(e, RILL_TYPE, "missing recursive binding");
      return;
    }
    cell.as.object->values[0] = rill_eval_closure(
        e, n->children, f->values[OPERANDS], f->values[OWNER]);
    if (e->error.kind)
      return;
  }
  publish(e, f, f->values[OPERANDS]);
  export_bindings(e, f->node, f->values[OPERANDS]);
}
static void nominal_declaration(RillEval *e, Frame *f) {
  const RillNode *n = f->node;
  RillValue prior = {};
  if (!f->values[OWNER].as.object->tag &&
      rill_eval_lookup_env(e->roots[TYPES], n->text.data, &prior)) {
    rill_eval_error(e, RILL_TYPE, "nominal declaration already exists");
    return;
  }
  RillValue value = {};
  if (n->kind == RILL_STRUCT) {
    value = type_case(e, n->children, n->text.data);
    save(f, value);
  } else {
    for (const RillNode *c = n->children; c && !e->error.kind; c = c->next) {
      for (const RillNode *old = n->children; old != c; old = old->next)
        if (!strcmp(old->text.data, c->text.data))
          rill_eval_error(e, RILL_TYPE, "duplicate enum case");
      save(f, rill_eval_text(e, c->text.data, c->text.size));
      save(f, type_case(e, c->children, c->text.data));
    }
    if (!e->error.kind) {
      value = rill_eval_object(e, RILL_V_RECORD, f->values + OPERANDS,
                               f->used - OPERANDS, nullptr, 0);
      f->values[OPERANDS] = value;
      f->used = OPERANDS + 1;
      f->root.count = f->used;
    }
  }
  if (e->error.kind)
    return;
  f->values[ENV] = rill_eval_bind(e, f->values[ENV], n->text.data, value, true);
  if (e->error.kind)
    return;
  // The registry is published only with the entry, below.
  publish(e, f, f->values[ENV]);
  export_bindings(e, n, f->values[ENV]);
}
static RillPlanRedirect lower_redirect(RillRedirect redirect) {
  switch (redirect) {
  case RILL_INPUT:
    return RILL_PLAN_INPUT;
  case RILL_OUTPUT:
    return RILL_PLAN_OUTPUT;
  case RILL_APPEND:
    return RILL_PLAN_APPEND;
  case RILL_ERROR_OUTPUT:
    return RILL_PLAN_ERROR_OUTPUT;
  case RILL_ERROR_APPEND:
    return RILL_PLAN_ERROR_APPEND;
  case RILL_ERROR_TO_OUTPUT:
    return RILL_PLAN_ERROR_TO_OUTPUT;
  }
  unreachable();
}
RillEvalEvent rill_runtime_step(RillEval *e) {
  for (unsigned quantum = 0; quantum < 1024; ++quantum) {
    if (e->error.kind) {
      int64_t checkpoint = 0;
      if (catch_error(e, &checkpoint)) {
        if (!e->error.kind && e->resource_serial > checkpoint) {
          e->waiting = true;
          return (RillEvalEvent){.state = RILL_EVAL_CLEANUP,
                                 .native = checkpoint,
                                 .value = e->roots[RESULT]};
        }
        continue;
      }
      RillSyntax *origin = e->roots[ERROR_CODE].kind == RILL_V_CODE
                               ? &e->roots[ERROR_CODE].as.object->code->syntax
                               : nullptr;
      return (RillEvalEvent){
          .state = RILL_EVAL_ERROR,
          .value = e->roots[RAISED],
          .diagnostic = e->error,
          .source = origin ? origin->name.data : nullptr,
          .source_bytes =
              origin ? (RillBytes){origin->source.data, origin->source.size}
                     : (RillBytes){}};
    }
    if (e->waiting)
      return (RillEvalEvent){.state = RILL_EVAL_YIELD};
    if (!e->frame) {
      if (e->roots[RESULT].kind == RILL_V_STREAM) {
        e->waiting = true;
        return (RillEvalEvent){.state = RILL_EVAL_STREAM,
                               .value = e->roots[RESULT]};
      }
      if (e->resource_boundary != e->resource_serial) {
        e->resource_boundary = e->resource_serial;
        RillError escape = rill_runtime_persistent(&e->heap, e->roots[ENTRY]);
        if (!escape)
          escape = rill_runtime_persistent(&e->heap, e->roots[RESULT]);
        if (escape) {
          rill_eval_error(e, escape,
                          "scoped Stream cannot escape a top-level statement");
          continue;
        }
        e->waiting = true;
        return (RillEvalEvent){.state = RILL_EVAL_CLEANUP,
                               .value = e->roots[RESULT]};
      }
      if (!e->statement) {
        RillValue merged = e->roots[GLOBAL];
        RillRoot merge_root = {};
        rill_runtime_root(&e->heap, &merge_root, &merged, 1);
        for (RillValue p = e->roots[ENTRY];
             p.kind == RILL_V_ENV && p.as.object->count == 2 && !e->error.kind;
             p = p.as.object->values[0])
          merged = rill_eval_bind(e, merged, p.as.object->bytes.data,
                                  p.as.object->values[1], false);
        RillValue pending[] = {rill_eval_compact(e, merged), e->roots[TYPES]};
        rill_runtime_unroot(&e->heap, &merge_root);
        RillRoot root = {};
        rill_runtime_root(&e->heap, &root, pending, 2);
        RillSyntax *syntax = e->roots[CODE].kind == RILL_V_CODE
                                 ? &e->roots[CODE].as.object->code->syntax
                                 : nullptr;
        for (const RillNode *n = syntax ? syntax->first : nullptr;
             n && !e->error.kind; n = n->next)
          if (n->kind == RILL_STRUCT || n->kind == RILL_ENUM) {
            RillValue v = {};
            if (rill_eval_lookup_env(e->roots[TYPES], n->text.data, &v)) {
              rill_eval_error(
                  e, RILL_TYPE,
                  "nominal declaration already exists at publication");
              break;
            }
            if (rill_eval_lookup_env(pending[0], n->text.data, &v))
              pending[1] =
                  rill_eval_bind(e, pending[1], n->text.data, v, false);
          }
        if (!e->error.kind) {
          RillError escape = rill_runtime_persistent(&e->heap, pending[0]);
          if (!escape)
            escape = rill_runtime_persistent(&e->heap, e->roots[RESULT]);
          if (escape)
            rill_eval_error(e, escape,
                            "scoped Stream cannot escape an execution entry");
        }
        // Publish bindings and nominal names together only after both succeed.
        if (!e->error.kind) {
          e->roots[GLOBAL] = pending[0];
          e->roots[TYPES] = pending[1];
        }
        rill_runtime_unroot(&e->heap, &root);
        if (e->error.kind)
          continue;
        e->roots[ENTRY] = e->roots[GLOBAL];
        return (RillEvalEvent){.state = RILL_EVAL_DONE,
                               .value = e->roots[RESULT]};
      }
      const RillNode *n = e->statement;
      e->statement = n->next;
      if (!push(e, n, e->roots[ENTRY], e->roots[CODE]))
        continue;
    }
    Frame *f = e->frame;
    const RillNode *n = f->node;
    RillNodeKind kind = n ? n->kind : RILL_CALL;
    if (e->ready) {
      e->ready = false;
      if (f->phase == IMPORT_WAIT && n) {
        f->values[ENV] = rill_eval_bind(e, f->values[ENV], n->text.data,
                                        e->roots[RESULT], true);
        publish(e, f, f->values[ENV]);
        done(e, (RillValue){});
        continue;
      }
      if (f->phase == HOST_WAIT) {
        pop(e);
        e->waiting = true;
        return (RillEvalEvent){.state = RILL_EVAL_CALLBACK,
                               .value = e->roots[RESULT]};
      }
      if (f->phase == MODULE_WAIT || f->phase == CALL_WAIT) {
        done(e, e->roots[RESULT]);
        continue;
      }
      if (f->phase == ATTEMPT_WAIT) {
        RillValue v = variant(e, "Result", "Ok", "value", e->roots[RESULT]);
        done(e, v);
        continue;
      }
      if (kind ==
          RILL_BLOCK) { /* The result slot already holds the last statement. */
      } else if (kind == RILL_MATCH && f->phase == MATCH_GUARD) {
        RillValue guard = e->roots[RESULT];
        if (guard.kind != RILL_V_BOOL) {
          rill_eval_error(e, RILL_TYPE, "guard requires Bool");
          continue;
        }
        if (guard.as.integer) {
          replace(e, f->arm->children->next, f->values[OPERANDS + 1],
                  f->values[OWNER]);
          continue;
        }
        f->arm = f->arm->next;
        f->phase = 1;
      } else
        save(f, e->roots[RESULT]);
    }
    if (kind == RILL_FUNCTION) {
      RillValue v = rill_eval_closure(e, n, f->values[ENV], f->values[OWNER]);
      done(e, v);
      continue;
    }
    if (kind == RILL_DECLARE || kind == RILL_REC) {
      recursive(e, f);
      done(e, (RillValue){});
      continue;
    }
    if (kind == RILL_STRUCT || kind == RILL_ENUM) {
      nominal_declaration(e, f);
      done(e, (RillValue){});
      continue;
    }
    if (kind == RILL_BLOCK) {
      if (!f->phase) {
        f->values[ENV] = rill_eval_scope(e, f->values[ENV]);
        f->phase = 1;
        e->roots[RESULT] = (RillValue){};
      }
      if (!f->next) {
        if (n->integer) {
          RillValue exported = {};
          RillRoot root = {};
          rill_runtime_root(&e->heap, &root, &exported, 1);
          for (const RillNode *decl = n->children; decl && !e->error.kind;
               decl = decl->next)
            if (decl->exported) {
              Bound *names = nullptr;
              if (decl->kind == RILL_BIND) {
                bool valid = rill_eval_pattern_names(e, decl->pattern, &names);
                (void)valid;
              } else {
                names = malloc(sizeof(*names));
                if (names)
                  *names = (Bound){nullptr, decl->text.data};
                else
                  rill_eval_error(e, RILL_MEMORY, "allocation failed");
              }
              for (Bound *b = names; b && !e->error.kind; b = b->parent) {
                RillValue item = {};
                if (rill_eval_lookup(e, f->values[ENV], b->name, &item))
                  exported = rill_eval_bind(e, exported, b->name, item, false);
              }
              rill_eval_names_free(names);
            }
          RillValue old = e->roots[EXPORTS];
          save(f, old);
          e->roots[EXPORTS] = exported;
          RillValue ns = rill_runtime_exports(e);
          e->roots[EXPORTS] = old;
          rill_runtime_unroot(&e->heap, &root);
          e->roots[RESULT] = ns;
          if (e->error.kind)
            continue;
          f->phase = MODULE_WAIT;
          e->waiting = true;
          return (RillEvalEvent){.state = RILL_EVAL_MODULE, .value = ns};
        }
        done(e, e->roots[RESULT]);
        continue;
      }
      const RillNode *child = f->next;
      f->next = child->next;
      if (!n->integer && !f->next && child->kind != RILL_BIND &&
          child->kind != RILL_DECLARE && child->kind != RILL_REC)
        replace(e, child, f->values[ENV], f->values[OWNER]);
      else
        (void)push(e, child, f->values[ENV], f->values[OWNER]);
      continue;
    }
    if (kind == RILL_IF && f->used > OPERANDS) {
      RillValue condition = f->values[OPERANDS];
      if (condition.kind != RILL_V_BOOL) {
        rill_eval_error(e, RILL_TYPE, "if requires Bool");
        continue;
      }
      replace(
          e, condition.as.integer ? n->children->next : n->children->next->next,
          f->values[ENV], f->values[OWNER]);
      continue;
    }
    if (kind == RILL_MATCH && f->used > OPERANDS) {
      if (!f->phase) {
        f->arm = n->children->next;
        f->phase = 1;
        save(f, (RillValue){});
      }
      if (!f->arm) {
        rill_eval_error(e, RILL_MATCH_ERROR, "no matching arm");
        continue;
      }
      f->values[OPERANDS + 1] = rill_eval_scope(e, f->values[ENV]);
      if (rill_eval_bind_pattern(e, f->arm->pattern, f->values[OPERANDS],
                                 f->values[ENV], f->values[OWNER],
                                 &f->values[OPERANDS + 1])) {
        f->phase = MATCH_GUARD;
        (void)push(e, f->arm->children, f->values[OPERANDS + 1],
                   f->values[OWNER]);
      } else
        f->arm = f->arm->next;
      continue;
    }
    if (kind == RILL_BINARY && f->used == OPERANDS + 1 &&
        (n->op == RILL_OP_AND || n->op == RILL_OP_OR)) {
      RillValue left = f->values[OPERANDS];
      if (left.kind != RILL_V_BOOL) {
        rill_eval_error(e, RILL_TYPE, "logical operands require Bool");
        continue;
      }
      if ((n->op == RILL_OP_AND && !left.as.integer) ||
          (n->op == RILL_OP_OR && left.as.integer)) {
        done(e, left);
        continue;
      }
    }
    if (f->next) {
      const RillNode *child = f->next;
      f->next = child->next;
      if (kind == RILL_RECORD) {
        save(f, rill_eval_constant(f->values[OWNER], child));
        child = child->children;
      }
      if (!atomic_operand(e, f, child))
        (void)push(e, child, f->values[ENV], f->values[OWNER]);
      continue;
    }
    RillValue v = {};
    RillValue *args = f->values + OPERANDS;
    size_t count = f->used - OPERANDS;
    switch (kind) {
    case RILL_UNIT:
    case RILL_INTEGER:
    case RILL_FLOAT:
    case RILL_BOOL:
    case RILL_NULL:
    case RILL_STRING:
      v = rill_eval_literal(n, f->values[OWNER]);
      break;
    case RILL_NAME:
      if (!rill_eval_lookup(e, f->values[ENV], n->text.data, &v))
        rill_eval_error(e, RILL_TYPE, "unknown binding");
      break;
    case RILL_CALL:
    case RILL_PIPE: {
      RillValue fn = args[kind == RILL_PIPE ? 1 : 0],
                arg = args[kind == RILL_PIPE ? 0 : 1];
      call(e, f, fn, arg);
      if (e->waiting)
        return (RillEvalEvent){
            .state = RILL_EVAL_NATIVE, .native = fn.as.integer, .value = arg};
      continue;
    }
    case RILL_BIND:
      if (rill_eval_bind_pattern(e, n->pattern, args[0], f->values[ENV],
                                 f->values[OWNER], &f->values[ENV])) {
        publish(e, f, f->values[ENV]);
        export_bindings(e, n, f->values[ENV]);
      } else if (!e->error.kind)
        rill_eval_error(e, RILL_MATCH_ERROR, "binding pattern did not match");
      break;
    case RILL_INDEX:
      if (args[0].kind == RILL_V_RECORD && args[1].kind == RILL_V_STRING) {
        if (!rill_runtime_field(args[0], args[1].as.object->bytes, &v))
          rill_eval_error(e, RILL_MISSING_FIELD, "missing record field");
      } else if ((args[0].kind != RILL_V_LIST &&
                  args[0].kind != RILL_V_SLICE) ||
                 args[1].kind != RILL_V_INT || args[1].as.integer < 0 ||
                 (uint64_t)args[1].as.integer >= rill_runtime_count(args[0]))
        rill_eval_error(e, RILL_TYPE, "invalid List index");
      else
        v = rill_runtime_at(args[0], (size_t)args[1].as.integer);
      break;
    case RILL_FIELD:
      if (args[0].kind != RILL_V_RECORD && args[0].kind != RILL_V_ADT)
        rill_eval_error(e, RILL_TYPE,
                        "field access requires Record or nominal data");
      else if (!rill_runtime_field(args[0],
                                   (RillBytes){n->text.data, n->text.size}, &v))
        rill_eval_error(e, RILL_MISSING_FIELD, "missing record field");
      break;
    case RILL_BINARY:
      if (n->op == RILL_OP_AND || n->op == RILL_OP_OR) {
        if (args[1].kind != RILL_V_BOOL)
          rill_eval_error(e, RILL_TYPE, "logical operands require Bool");
        else
          v = args[1];
      } else
        v = numeric(e, n->op, args[0], args[1]);
      break;
    case RILL_UNARY:
      if (n->op == RILL_OP_NOT) {
        if (args[0].kind != RILL_V_BOOL)
          rill_eval_error(e, RILL_TYPE, "not requires Bool");
        else
          v = (RillValue){.kind = RILL_V_BOOL,
                          .as.integer = !args[0].as.integer};
      } else if (args[0].kind == RILL_V_INT) {
        int64_t result = {};
        if (ckd_sub(&result, 0, args[0].as.integer))
          rill_eval_error(e, RILL_ARITHMETIC, "integer overflow");
        else
          v = (RillValue){.kind = RILL_V_INT, .as.integer = result};
      } else if (args[0].kind == RILL_V_FLOAT)
        v = (RillValue){.kind = RILL_V_FLOAT, .as.real = -args[0].as.real};
      else
        rill_eval_error(e, RILL_TYPE, "negation requires a number");
      break;
    case RILL_WITH: {
      RillValue base =
          args[0].kind == RILL_V_ADT ? args[0].as.object->values[1] : args[0];
      if (base.kind != RILL_V_RECORD || args[1].kind != RILL_V_RECORD) {
        rill_eval_error(e, RILL_TYPE, "with requires records");
        break;
      }
      v = rill_runtime_record_copy(&e->heap, base);
      if (v.kind == RILL_V_UNIT)
        rill_eval_error(e, RILL_MEMORY, "allocation failed");
      save(f, v);
      if (e->error.kind)
        break;
      for (size_t i = 0; i < args[1].as.object->count; i += 2) {
        RillValue key = args[1].as.object->values[i];
        size_t index = rill_runtime_field_index(v, key.as.object->bytes);
        if (index == v.as.object->count) {
          rill_eval_error(e, RILL_MISSING_FIELD, "with cannot insert fields");
          break;
        }
        v.as.object->values[index + 1] = args[1].as.object->values[i + 1];
      }
      if (args[0].kind == RILL_V_ADT) {
        RillValue pair[] = {args[0].as.object->values[0], v};
        v = rill_eval_object(e, RILL_V_ADT, pair, 2, nullptr, 0);
      }
      break;
    }
    case RILL_SPREAD:
      if (args[0].kind != RILL_V_LIST && args[0].kind != RILL_V_SLICE)
        rill_eval_error(e, RILL_TYPE, "spread requires List");
      else
        v = args[0];
      break;
    case RILL_STAGE: {
      size_t total = 0, index = 0;
      for (const RillNode *c = n->children; c; c = c->next, ++index) {
        size_t added =
            c->kind == RILL_SPREAD ? rill_runtime_count(args[index]) : 1;
        if (ckd_add(&total, total, added) || total > MAX_STAGE_ITEMS) {
          rill_eval_error(e, RILL_LIMIT, "command payload limit exceeded");
          break;
        }
      }
      if (e->error.kind)
        break;
      v = rill_eval_object(e, RILL_V_STAGE, nullptr, total, nullptr, 0);
      if (e->error.kind)
        break;
      // Filling and validating the private payload has no further safepoint.
      RillValue *items = v.as.object->values;
      size_t used = 0;
      index = 0;
      for (const RillNode *c = n->children; c; c = c->next, ++index) {
        if (c->kind == RILL_SPREAD)
          for (size_t i = 0; i < rill_runtime_count(args[index]); ++i) {
            assert(used < total);
            items[used++] = rill_runtime_at(args[index], i);
          }
        else {
          assert(used < total);
          items[used++] = args[index];
        }
      }
      size_t argument = 0;
      for (size_t i = 0; i < total; ++i) {
        RillValue item = items[i];
        bool redirect = item.kind == RILL_V_REDIRECT;
        if (redirect) {
          if (!item.as.object->count)
            continue;
          item = item.as.object->values[0];
        }
        const char *message = nullptr;
        if (item.kind != RILL_V_STRING && item.kind != RILL_V_BYTES &&
            item.kind != RILL_V_PATH) {
          message = redirect
                        ? "redirection paths require String, Bytes or Path"
                        : "command arguments require String, Bytes or Path";
        } else if (memchr(item.as.object->bytes.data, 0,
                          item.as.object->bytes.size)) {
          message = redirect ? "redirection path contains NUL"
                             : "command argument contains NUL";
        } else if (!redirect && argument == 0 && !item.as.object->bytes.size)
          message = "executable must not be empty";
        if (message) {
          rill_eval_error(e, RILL_TYPE, message);
          e->error.has_argument = !redirect;
          e->error.argument = argument;
          e->error.stage = f->parent ? f->parent->used - OPERANDS : 0;
          break;
        }
        if (!redirect)
          ++argument;
      }
      break;
    }
    case RILL_LIST:
    case RILL_RECORD:
    case RILL_PLAN:
    case RILL_REDIRECT:
      v = rill_eval_object(e,
                           kind == RILL_LIST     ? RILL_V_LIST
                           : kind == RILL_RECORD ? RILL_V_RECORD
                           : kind == RILL_PLAN   ? RILL_V_PLAN
                                                 : RILL_V_REDIRECT,
                           args, count, nullptr, 0);
      if (kind == RILL_REDIRECT && v.kind != RILL_V_UNIT)
        v.as.object->tag = lower_redirect(n->redirect);
      break;
    case RILL_IMPORT:
      f->phase = IMPORT_WAIT;
      e->waiting = true;
      return (RillEvalEvent){
          .state = RILL_EVAL_IMPORT,
          .value = args[0],
          .source = f->values[OWNER].as.object->code->syntax.name.data};
    case RILL_FUNCTION:
    case RILL_DECLARE:
    case RILL_BLOCK:
    case RILL_IF:
    case RILL_MATCH:
    case RILL_STRUCT:
    case RILL_ENUM:
    case RILL_REC:
    case RILL_PAIR:
    case RILL_ARM:
    case RILL_REST:
    case RILL_NOMINAL:
      rill_eval_error(e, RILL_TYPE, "invalid evaluator form");
      break;
    }
    done(e, v);
  }
  return (RillEvalEvent){.state = RILL_EVAL_YIELD};
}
void rill_runtime_resume(RillEval *e, RillValue value,
                         RillDiagnostic diagnostic) {
  e->waiting = false;
  e->ready = e->frame != nullptr;
  e->roots[RESULT] = value;
  e->error = diagnostic;
  if (diagnostic.kind && value.kind == RILL_V_ADT)
    e->roots[RAISED] = value;
  if (e->frame && diagnostic.kind)
    e->roots[ERROR_CODE] = e->frame->values[OWNER];
  if (e->frame && e->frame->node && diagnostic.kind)
    e->error.offset = e->frame->node->offset;
}
void rill_runtime_free(RillEval *e) {
  if (!e)
    return;
  rill_runtime_abort(e);
  rill_runtime_heap_clear(&e->heap);
  while (e->spare_frames) {
    Frame *next = e->spare_frames->parent;
    free(e->spare_frames);
    e->spare_frames = next;
  }
  free(e);
}

void rill_runtime_prelude(RillEval *e) { e->roots[PRELUDE] = e->roots[GLOBAL]; }
void rill_runtime_module(RillEval *e, RillSyntax *s) {
  e->waiting = false;
  RillValue values[2] = {e->roots[PRELUDE], rill_eval_code(e, s)};
  if (e->error.kind)
    return;
  RillCode *code = values[1].as.object->code;
  code->module = (RillNode){
      .kind = RILL_BLOCK, .integer = 1, .children = code->syntax.first};
  values[1].as.object->tag = 1;
  (void)push(e, &code->module, values[0], values[1]);
}
bool rill_runtime_retain_module(RillEval *e, RillValue value) {
  RillValue values[] = {e->roots[MODULES], value};
  RillRoot root = {};
  rill_runtime_root(&e->heap, &root, values, 2);
  RillValue next = rill_eval_object(e, RILL_V_LIST, values, 2, nullptr, 0);
  if (!e->error.kind)
    e->roots[MODULES] = next;
  rill_runtime_unroot(&e->heap, &root);
  return !e->error.kind;
}

bool rill_runtime_builtin(RillEval *e, const char *name, RillValue *value) {
  return rill_eval_lookup(e,
                          e->roots[PRELUDE].kind == RILL_V_UNIT
                              ? e->roots[ENTRY]
                              : e->roots[PRELUDE],
                          name, value);
}

void rill_runtime_callback(RillEval *e, RillValue function,
                           RillValue argument) {
  assert(e->waiting);
  RillValue env = e->frame ? e->frame->values[ENV] : e->roots[ENTRY],
            code = e->frame ? e->frame->values[OWNER] : e->roots[CODE];
  e->waiting = false;
  e->ready = false;
  if (!push(e, nullptr, env, code))
    return;
  e->frame->phase = HOST_WAIT;
  if (!push(e, nullptr, env, code))
    return;
  e->frame->phase = ATTEMPT_WAIT;
  e->frame->checkpoint = e->resource_serial;
  if (!push(e, nullptr, env, code))
    return;
  save(e->frame, function);
  save(e->frame, argument);
}

int64_t rill_runtime_resource_id(RillEval *e) {
  return e->resource_serial == INT64_MAX ? 0 : ++e->resource_serial;
}
struct RillEvaluation {
  Frame *frame;
  const RillNode *statement;
  size_t depth;
  int64_t resource_boundary;
  RillValue roots[ROOT_COUNT];
  RillRoot root;
  RillDiagnostic error;
  bool waiting, ready;
};
static bool shared_root(size_t i) {
  return i == GLOBAL || i == TYPES || i == PRELUDE || i == MODULES;
}
RillEvaluation *rill_runtime_suspend(RillEval *e) {
  RillEvaluation *saved = malloc(sizeof(*saved));
  if (!saved)
    return nullptr;
  *saved = (RillEvaluation){.frame = e->frame,
                            .statement = e->statement,
                            .depth = e->depth,
                            .resource_boundary = e->resource_boundary,
                            .error = e->error,
                            .waiting = e->waiting,
                            .ready = e->ready};
  for (size_t i = 0; i < ROOT_COUNT; ++i)
    if (!shared_root(i)) {
      saved->roots[i] = e->roots[i];
      e->roots[i] = (RillValue){};
    }
  rill_runtime_root(&e->heap, &saved->root, saved->roots, ROOT_COUNT);
  e->frame = nullptr;
  e->statement = nullptr;
  e->depth = 0;
  e->waiting = false;
  e->ready = false;
  e->error = (RillDiagnostic){};
  return saved;
}
void rill_runtime_restore(RillEval *e, RillEvaluation *saved) {
  assert(!e->frame && !e->statement);
  e->frame = saved->frame;
  e->statement = saved->statement;
  e->depth = saved->depth;
  e->resource_boundary = saved->resource_boundary;
  e->error = saved->error;
  e->waiting = saved->waiting;
  e->ready = saved->ready;
  for (size_t i = 0; i < ROOT_COUNT; ++i)
    if (!shared_root(i))
      e->roots[i] = saved->roots[i];
  rill_runtime_unroot(&e->heap, &saved->root);
  free(saved);
}
void rill_runtime_discard(RillEval *e, RillEvaluation *saved) {
  if (!saved)
    return;
  for (Frame *frame = saved->frame; frame;) {
    Frame *next = frame->parent;
    rill_runtime_unroot(&e->heap, &frame->root);
    free(frame);
    frame = next;
  }
  rill_runtime_unroot(&e->heap, &saved->root);
  free(saved);
}
