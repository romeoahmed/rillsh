#include "diagnostic.h"
#include "runtime.h"
#include "syntax/syntax.h"
#include <stdckdint.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

typedef struct Frame {
  struct Frame *parent;
  const RillNode *node, *next;
  size_t count, used;
  RillRoot root;
  RillValue values[];
} Frame;
typedef struct Binding {
  struct Binding *next;
  char *name;
  RillValue value;
  RillRoot root;
} Binding;
struct RillEval {
  RillHeap heap;
  const RillNative *natives;
  size_t native_count, depth;
  Frame *frame;
  const RillNode *statement;
  Binding *bindings, *pending;
  RillValue result;
  RillRoot result_root;
  RillDiagnostic error;
  bool waiting, ready;
};
static void frame_pop(RillEval *e) {
  Frame *f = e->frame;
  rill_runtime_unroot(&e->heap, &f->root);
  e->frame = f->parent;
  --e->depth;
  free(f);
}
static bool frame_push(RillEval *e, const RillNode *n) {
  if (e->depth == 65536) {
    e->error = (RillDiagnostic){.kind = RILL_LIMIT,
                                .offset = n->offset,
                                .message = "continuation limit exceeded"};
    return false;
  }
  size_t count = 0;
  for (const RillNode *c = n->children; c; c = c->next)
    ++count;
  size_t bytes;
  if (ckd_mul(&bytes, count, sizeof(RillValue)) ||
      ckd_add(&bytes, bytes, sizeof(Frame)))
    return false;
  Frame *f = malloc(bytes);
  if (!f)
    return false;
  *f = (Frame){
      .parent = e->frame, .node = n, .next = n->children, .count = count};
  e->frame = f;
  ++e->depth;
  // Operand slots become roots only after evaluation initializes them.
  rill_runtime_root(&e->heap, &f->root, f->values, 0);
  return true;
}
static void error(RillEval *e, RillError kind, const char *message) {
  if (e->error.kind)
    return;
  e->error = (RillDiagnostic){.kind = kind,
                              .offset = e->frame ? e->frame->node->offset : 0,
                              .message = message};
}
static void binding_clear(Binding *b) {
  while (b) {
    Binding *next = b->next;
    free(b->name);
    free(b);
    b = next;
  }
}
// Rebuild only between statements, when no continuation roots remain.
static void roots_rebuild(RillEval *e) {
  e->heap.roots = nullptr;
  for (Binding *b = e->bindings; b; b = b->next)
    rill_runtime_root(&e->heap, &b->root, &b->value, 1);
  for (Binding *b = e->pending; b; b = b->next)
    rill_runtime_root(&e->heap, &b->root, &b->value, 1);
  rill_runtime_root(&e->heap, &e->result_root, &e->result, 1);
}
RillEval *rill_runtime_new(const RillNative *n, size_t count) {
  RillEval *e = malloc(sizeof(*e));
  if (!e)
    return nullptr;
  *e = (RillEval){.natives = n, .native_count = count};
  roots_rebuild(e);
  return e;
}
RillHeap *rill_runtime_heap(RillEval *e) { return &e->heap; }
void rill_runtime_abort(RillEval *e) {
  while (e->frame)
    frame_pop(e);
  binding_clear(e->pending);
  e->pending = nullptr;
  e->statement = nullptr;
  e->waiting = false;
  e->ready = false;
  e->result = (RillValue){};
  roots_rebuild(e);
  rill_runtime_collect(&e->heap);
}
void rill_runtime_begin(RillEval *e, const RillSyntax *s) {
  rill_runtime_abort(e);
  e->error = (RillDiagnostic){};
  e->statement = s->first;
}
static bool lookup(RillEval *e, const char *name, RillValue *v) {
  for (Binding *b = e->pending; b; b = b->next)
    if (!strcmp(b->name, name)) {
      *v = b->value;
      return true;
    }
  for (Binding *b = e->bindings; b; b = b->next)
    if (!strcmp(b->name, name)) {
      *v = b->value;
      return true;
    }
  for (size_t i = 0; i < e->native_count; ++i)
    if (!strcmp(e->natives[i].name, name)) {
      *v = (RillValue){.kind = RILL_V_FUNCTION, .as.integer = e->natives[i].id};
      return true;
    }
  return false;
}
static bool bind(RillEval *e, const char *name, RillValue value) {
  for (Binding *b = e->pending; b; b = b->next)
    if (!strcmp(b->name, name)) {
      error(e, RILL_TYPE, "duplicate binding in entry");
      return false;
    }
  Binding *b = malloc(sizeof(*b));
  if (!b)
    return false;
  b->name = strdup(name);
  if (!b->name) {
    free(b);
    return false;
  }
  b->value = value;
  b->next = e->pending;
  e->pending = b;
  return true;
}
static void commit(RillEval *e) {
  while (e->pending) {
    Binding *b = e->pending;
    e->pending = b->next;
    Binding **old = &e->bindings;
    while (*old && strcmp((*old)->name, b->name) != 0)
      old = &(*old)->next;
    if (*old) {
      Binding *dead = *old;
      *old = dead->next;
      free(dead->name);
      free(dead);
    }
    b->next = e->bindings;
    e->bindings = b;
  }
  roots_rebuild(e);
}
RillEvalEvent rill_runtime_step(RillEval *e) {
  for (unsigned quantum = 0; quantum < 1024; ++quantum) {
    if (e->error.kind)
      return (RillEvalEvent){.state = RILL_EVAL_ERROR, .diagnostic = e->error};
    if (e->waiting)
      return (RillEvalEvent){.state = RILL_EVAL_YIELD};
    if (!e->frame) {
      if (!e->statement) {
        commit(e);
        return (RillEvalEvent){.state = RILL_EVAL_DONE, .value = e->result};
      }
      roots_rebuild(e);
      if (!frame_push(e, e->statement)) {
        error(e, RILL_MEMORY, "allocation failed");
        continue;
      }
      e->statement = e->statement->next;
    }
    Frame *f = e->frame;
    if (e->ready) {
      e->ready = false;
      if (f->node->kind == RILL_CALL && f->used == f->count) {
        frame_pop(e);
        e->ready = e->frame != nullptr;
        continue;
      }
      f->values[f->used++] = e->result;
      f->root.count = f->used;
    }
    if (f->next) {
      const RillNode *child = f->next;
      f->next = child->next;
      if (!frame_push(e, child))
        error(e, RILL_MEMORY, "allocation failed");
      continue;
    }
    const RillNode *n = f->node;
    RillValue v = {};
    switch (n->kind) {
    case RILL_UNIT:
      break;
    case RILL_INTEGER:
      v = (RillValue){.kind = RILL_V_INT, .as.integer = n->integer};
      break;
    case RILL_NAME:
      if (!lookup(e, n->text.data, &v))
        error(e, RILL_TYPE, "unknown binding");
      break;
    case RILL_CALL:
      if (f->values[0].kind != RILL_V_FUNCTION) {
        error(e, RILL_TYPE, "application requires a function");
        break;
      }
      e->waiting = true;
      return (RillEvalEvent){.state = RILL_EVAL_NATIVE,
                             .native = f->values[0].as.integer,
                             .value = f->values[1]};
    case RILL_INDEX:
      if (f->values[0].kind != RILL_V_LIST || f->values[1].kind != RILL_V_INT ||
          f->values[1].as.integer < 0 ||
          (uint64_t)f->values[1].as.integer >= f->values[0].as.object->count)
        error(e, RILL_TYPE, "invalid List index");
      else
        v = f->values[0].as.object->values[(size_t)f->values[1].as.integer];
      break;
    case RILL_FIELD:
      if (f->values[0].kind != RILL_V_RECORD)
        error(e, RILL_TYPE, "field access requires Record");
      else {
        RillObject *o = f->values[0].as.object;
        const char *key = o->bytes.data;
        bool found = false;
        for (size_t i = 0; i < o->count; ++i) {
          if (!strcmp(key, n->text.data)) {
            v = o->values[i];
            found = true;
            break;
          }
          key += strlen(key) + 1;
        }
        if (!found)
          error(e, RILL_TYPE, "missing record field");
      }
      break;
    case RILL_BIND:
      if (!bind(e, n->text.data, f->values[0]) && !e->error.kind)
        error(e, RILL_MEMORY, "allocation failed");
      break;
    case RILL_STRING:
    case RILL_LIST:
    case RILL_PLAN:
    case RILL_STAGE:
    case RILL_REDIRECT: {
      RillValueKind kind = RILL_V_STRING;
      int64_t tag = 0;
      if (n->kind == RILL_LIST)
        kind = RILL_V_LIST;
      if (n->kind == RILL_PLAN)
        kind = RILL_V_PLAN;
      if (n->kind == RILL_STAGE)
        kind = RILL_V_STAGE;
      if (n->kind == RILL_REDIRECT) {
        static constexpr RillPlanRedirect redirects[] = {
            [RILL_INPUT] = RILL_PLAN_INPUT,
            [RILL_OUTPUT] = RILL_PLAN_OUTPUT,
            [RILL_APPEND] = RILL_PLAN_APPEND,
            [RILL_ERROR_OUTPUT] = RILL_PLAN_ERROR_OUTPUT,
            [RILL_ERROR_APPEND] = RILL_PLAN_ERROR_APPEND,
            [RILL_ERROR_TO_OUTPUT] = RILL_PLAN_ERROR_TO_OUTPUT};
        kind = RILL_V_REDIRECT;
        tag = redirects[n->integer];
      }
      v = rill_runtime_object(&e->heap, kind, f->values, f->count, n->text.data,
                              n->text.size, tag);
      if (v.kind == RILL_V_UNIT)
        error(e, RILL_MEMORY, "allocation failed");
      break;
    }
    }
    e->result = v;
    frame_pop(e);
    e->ready = e->frame != nullptr;
  }
  return (RillEvalEvent){.state = RILL_EVAL_YIELD};
}
void rill_runtime_resume(RillEval *e, RillValue value,
                         RillDiagnostic diagnostic) {
  e->waiting = false;
  e->ready = true;
  e->result = value;
  e->error = diagnostic;
  if (e->frame && diagnostic.kind)
    e->error.offset = e->frame->node->offset;
}
void rill_runtime_free(RillEval *e) {
  if (!e)
    return;
  rill_runtime_abort(e);
  binding_clear(e->bindings);
  rill_runtime_heap_clear(&e->heap);
  free(e);
}
