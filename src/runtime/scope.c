#include "diagnostic.h"
#include "private.h"
#include "runtime.h"
#include "syntax/syntax.h"
#include "text/text.h"
#include <assert.h>
#include <stdckdint.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>
static bool same(RillBytes a, RillBytes b) {
  return a.size == b.size && (!a.size || !memcmp(a.data, b.data, a.size));
}
static bool bound(Bound *b, const char *name) {
  for (; b; b = b->parent)
    if (!strcmp(b->name, name))
      return true;
  return false;
}
bool rill_eval_lookup_env(RillValue env, const char *name, RillValue *out) {
  while (env.kind == RILL_V_ENV) {
    RillObject *o = env.as.object;
    if (o->count == 2 && !strcmp(o->bytes.data, name)) {
      *out = o->values[1];
      return true;
    }
    env = o->values[0];
  }
  if (env.kind != RILL_V_BINDINGS && env.kind != RILL_V_CLOSURE)
    return false;
  RillObject *o = env.as.object;
  size_t first = env.kind == RILL_V_CLOSURE ? 1 : 0;
  const char *const *names = first ? o->captures->names : o->names;
  size_t count = o->count - first;
  if (count < 8) {
    for (size_t i = 0; i < count; ++i)
      if (!strcmp(names[i], name)) {
        *out = o->values[first + i];
        return true;
      }
  } else {
    size_t lo = 0, hi = count;
    while (lo < hi) {
      size_t mid = lo + (hi - lo) / 2;
      int order = strcmp(name, names[mid]);
      if (order < 0)
        hi = mid;
      else if (order > 0)
        lo = mid + 1;
      else {
        *out = o->values[first + mid];
        return true;
      }
    }
  }
  return false;
}
bool rill_eval_lookup(RillEval *e, RillValue env, const char *name,
                      RillValue *out) {
  if (rill_eval_lookup_env(env, name, out)) {
    if (out->kind == RILL_V_CELL)
      *out = out->as.object->values[0];
    return true;
  }
  if (e->roots[PRELUDE].kind != RILL_V_UNIT && name[0] == '_' && name[1] == '_')
    return false;
  for (size_t i = 0; i < e->native_count; ++i)
    if (!strcmp(e->natives[i].name, name)) {
      *out =
          (RillValue){.kind = RILL_V_FUNCTION, .as.integer = e->natives[i].id};
      return true;
    }
  return false;
}
RillValue rill_eval_scope(RillEval *e, RillValue parent) {
  return rill_eval_object(e, RILL_V_ENV, &parent, 1, nullptr, 0);
}
RillValue rill_eval_bind(RillEval *e, RillValue env, const char *name,
                         RillValue value, bool duplicate) {
  if (!strcmp(name, "_"))
    return env;
  if (duplicate)
    for (RillValue p = env; p.kind == RILL_V_ENV && p.as.object->count == 2;
         p = p.as.object->values[0])
      if (!strcmp(p.as.object->bytes.data, name)) {
        rill_eval_error(e, RILL_TYPE, "duplicate binding in scope");
        return env;
      }
  RillValue pair[] = {env, value};
  return rill_eval_object(e, RILL_V_ENV, pair, 2, name, strlen(name));
}
typedef struct {
  const char *name;
  RillValue value;
  size_t order;
} Binding;
static int binding_order(const void *left, const void *right) {
  const Binding *a = left, *b = right;
  int order = strcmp(a->name, b->name);
  return order ? order : (a->order > b->order) - (a->order < b->order);
}
RillValue rill_eval_compact(RillEval *e, RillValue env) {
  // An entry without bindings shares the existing immutable snapshot.
  if (env.kind != RILL_V_ENV)
    return env;
  if (env.as.object->count == 1)
    return env.as.object->values[0];
  size_t count = 0, bytes = {};
  RillValue base = env;
  for (; base.kind == RILL_V_ENV; base = base.as.object->values[0])
    if (base.as.object->count == 2 && ckd_add(&count, count, 1))
      goto overflow;
  if ((base.kind == RILL_V_BINDINGS &&
       ckd_add(&count, count, base.as.object->count)) ||
      ckd_mul(&bytes, count, sizeof(Binding)))
    goto overflow;
  assert(count && bytes); // The first environment node contains a binding.
  Binding *bindings = malloc(bytes);
  if (!bindings) {
    rill_eval_error(e, RILL_MEMORY, "allocation failed");
    return (RillValue){};
  }
  size_t used = 0;
  for (RillValue p = env; p.kind == RILL_V_ENV; p = p.as.object->values[0]) {
    RillObject *o = p.as.object;
    if (o->count == 2) {
      assert(used < count);
      bindings[used] = (Binding){o->bytes.data, o->values[1], used};
      ++used;
    }
  }
  if (base.kind == RILL_V_BINDINGS)
    for (size_t i = 0; i < base.as.object->count; ++i) {
      assert(used < count);
      bindings[used] =
          (Binding){base.as.object->names[i], base.as.object->values[i], used};
      ++used;
    }
  // Recency breaks equal-name ties without requiring stable qsort. The new
  // snapshot retains neither replaced bindings nor earlier snapshots.
  qsort(bindings, count, sizeof(*bindings), binding_order);
  used = 0;
  for (size_t i = 0; i < count; ++i)
    if (!used || strcmp(bindings[used - 1].name, bindings[i].name) != 0)
      bindings[used++] = bindings[i];
  RillBuffer names = {};
  bool ok = true;
  for (size_t i = 0; i < used && ok; ++i)
    ok = rill_text_append(&names, bindings[i].name,
                          strlen(bindings[i].name) + 1);
  RillValue out = {};
  if (ok) {
    // Rooted env owns the borrowed names and values. After this allocation,
    // finishing the snapshot cannot collect.
    out = rill_eval_object(e, RILL_V_BINDINGS, nullptr, used, names.data,
                           names.size);
    if (!e->error.kind) {
      RillObject *o = out.as.object;
      o->names = (void *)(o->values + used);
      const char *name = o->bytes.data;
      for (size_t i = 0; i < used; ++i) {
        o->names[i] = name;
        o->values[i] = bindings[i].value;
        name += strlen(name) + 1;
      }
    }
  } else
    rill_eval_error(e, RILL_MEMORY, "allocation failed");
  rill_text_clear(&names);
  free(bindings);
  return out;
overflow:
  rill_eval_error(e, RILL_MEMORY, "binding snapshot size overflow");
  return (RillValue){};
}
// Analysis retains only names; values are resolved for each closure instance.
static bool free_names(RillEval *e, const RillNode *n, Bound *locals,
                       Captures *captures);
bool rill_eval_pattern_names(RillEval *e, const RillNode *p, Bound **names) {
  if (!p)
    return true;
  if (p->kind == RILL_NAME && strcmp(p->text.data, "_") != 0) {
    if (bound(*names, p->text.data)) {
      rill_eval_error(e, RILL_TYPE, "duplicate name in pattern");
      return false;
    }
    Bound *b = malloc(sizeof(*b));
    if (!b) {
      rill_eval_error(e, RILL_MEMORY, "allocation failed");
      return false;
    }
    *b = (Bound){*names, p->text.data};
    *names = b;
  }
  for (const RillNode *c = p->children; c; c = c->next)
    if (!rill_eval_pattern_names(e, c, names))
      return false;
  return true;
}
void rill_eval_names_free(Bound *b) {
  while (b) {
    Bound *next = b->parent;
    free(b);
    b = next;
  }
}
static bool capture_name(RillEval *e, const char *name, Bound *locals,
                         Captures *captures) {
  if (bound(locals, name))
    return true;
  for (size_t i = 0; i < captures->count; ++i)
    if (!strcmp(captures->names[i], name))
      return true;
  if (captures->count == captures->capacity) {
    size_t capacity = {}, bytes = {};
    if (ckd_mul(&capacity, captures->capacity ? captures->capacity : 4, 2) ||
        ckd_mul(&bytes, capacity, sizeof(*captures->names))) {
      rill_eval_error(e, RILL_MEMORY, "capture analysis size overflow");
      return false;
    }
    const char **names = realloc(captures->names, bytes);
    if (!names) {
      rill_eval_error(e, RILL_MEMORY, "allocation failed");
      return false;
    }
    captures->names = names;
    captures->capacity = capacity;
  }
  captures->names[captures->count++] = name;
  return true;
}
static bool pattern_references(RillEval *e, const RillNode *pattern,
                               Bound *locals, Captures *captures) {
  if (!pattern)
    return true;
  if (pattern->kind == RILL_NOMINAL &&
      !free_names(e, pattern->pattern, locals, captures))
    return false;
  for (const RillNode *child = pattern->children; child; child = child->next)
    if (!pattern_references(e, child, locals, captures))
      return false;
  return true;
}
static bool declared_names(RillEval *e, const RillNode *node, Bound **names) {
  if (node->kind == RILL_REC) {
    for (const RillNode *c = node->children; c; c = c->next)
      if (!declared_names(e, c, names))
        return false;
    return true;
  }
  Bound *name = malloc(sizeof(*name));
  if (!name) {
    rill_eval_error(e, RILL_MEMORY, "allocation failed");
    return false;
  }
  *name = (Bound){*names, node->text.data};
  *names = name;
  return true;
}
static bool free_names(RillEval *e, const RillNode *n, Bound *locals,
                       Captures *captures) {
  if (!n)
    return true;
  if (n->pattern && !pattern_references(e, n->pattern, locals, captures))
    return false;
  if (n->kind == RILL_NAME)
    return capture_name(e, n->text.data, locals, captures);
  if (n->kind == RILL_FUNCTION || n->kind == RILL_ARM) {
    Bound *names = nullptr;
    if (!rill_eval_pattern_names(e, n->pattern, &names)) {
      rill_eval_names_free(names);
      return false;
    }
    Bound *last = names;
    if (last) {
      while (last->parent)
        last = last->parent;
      last->parent = locals;
    }
    bool ok = true;
    if (n->pattern && n->pattern->kind == RILL_NOMINAL)
      ok = free_names(e, n->pattern->pattern, locals, captures);
    for (const RillNode *c = n->children; c && ok; c = c->next)
      ok = free_names(e, c, names ? names : locals, captures);
    if (last)
      last->parent = nullptr;
    rill_eval_names_free(names);
    return ok;
  }
  if (n->kind == RILL_BLOCK) {
    Bound *names = nullptr;
    bool ok = true;
    for (const RillNode *c = n->children; c && ok; c = c->next) {
      Bound *last = names;
      if (last) {
        while (last->parent)
          last = last->parent;
        last->parent = locals;
      }
      ok = free_names(e, c, names ? names : locals, captures);
      if (last)
        last->parent = nullptr;
      if (c->kind == RILL_BIND)
        ok = ok && rill_eval_pattern_names(e, c->pattern, &names);
      if (c->kind == RILL_DECLARE || c->kind == RILL_REC)
        ok = ok && declared_names(e, c, &names);
    }
    rill_eval_names_free(names);
    return ok;
  }
  if (n->kind == RILL_REC) {
    Bound *names = nullptr;
    if (!declared_names(e, n, &names)) {
      rill_eval_names_free(names);
      return false;
    }
    Bound *last = names;
    while (last && last->parent)
      last = last->parent;
    if (last)
      last->parent = locals;
    bool ok = true;
    for (const RillNode *c = n->children; c && ok; c = c->next)
      ok = free_names(e, c, names ? names : locals, captures);
    if (last)
      last->parent = nullptr;
    rill_eval_names_free(names);
    return ok;
  }
  if (n->kind == RILL_DECLARE) {
    Bound self = {locals, n->text.data};
    return free_names(e, n->children, &self, captures);
  }
  for (const RillNode *c = n->children; c; c = c->next)
    if (!free_names(e, c, locals, captures))
      return false;
  return true;
}
static int name_order(const void *left, const void *right) {
  const char *const *a = left, *const *b = right;
  return strcmp(*a, *b);
}
void rill_eval_analyze(RillEval *e, Captures *captures) {
  RillValue origin = e->roots[ERROR_CODE];
  (void)free_names(e, captures->node, nullptr, captures);
  e->roots[ERROR_CODE] = origin;
  // Small captures use a short linear search; wider layouts share sorted names
  // across all instances. Resolution has no observable per-name effects.
  if (captures->count >= 8)
    qsort(captures->names, captures->count, sizeof(*captures->names),
          name_order);
  captures->error = e->error.kind;
  captures->message = e->error.message;
  if (e->error.kind != RILL_MEMORY)
    e->error = (RillDiagnostic){};
}
RillValue rill_eval_closure(RillEval *e, const RillNode *n, RillValue env,
                            RillValue code) {
  Captures *analysis = rill_eval_captures(code.as.object->code, n);
  size_t count = {};
  if (ckd_add(&count, analysis->count, 1)) {
    rill_eval_error(e, RILL_MEMORY, "closure size overflow");
    return (RillValue){};
  }
  RillValue v = rill_eval_object(e, RILL_V_CLOSURE, nullptr, count, nullptr, 0);
  if (e->error.kind)
    return v;
  v.as.object->captures = analysis;
  v.as.object->values[0] = code;
  // Resolution neither allocates nor calls user code. The flat payload is
  // complete before publication and keeps its borrowed analysis alive via code.
  for (size_t i = 0; i < analysis->count; ++i) {
    const char *name = analysis->names[i];
    RillValue *slot = &v.as.object->values[i + 1];
    // Preserve recursive cells rather than capturing their current contents.
    if (!rill_eval_lookup_env(env, name, slot) &&
        !rill_eval_lookup(e, env, name, slot)) {
      rill_eval_error(e, RILL_TYPE, "unresolved free binding");
      break;
    }
  }
  if (!e->error.kind && analysis->error)
    rill_eval_error(e, analysis->error, analysis->message);
  return v;
}
RillValue rill_eval_literal(const RillNode *n, RillValue code) {
  if (n->kind == RILL_INTEGER)
    return (RillValue){.kind = RILL_V_INT, .as.integer = n->integer};
  if (n->kind == RILL_FLOAT)
    return (RillValue){.kind = RILL_V_FLOAT, .as.real = n->real};
  if (n->kind == RILL_BOOL)
    return (RillValue){.kind = RILL_V_BOOL, .as.integer = n->integer};
  if (n->kind == RILL_NULL)
    return (RillValue){.kind = RILL_V_NULL};
  if (n->kind == RILL_STRING)
    return rill_eval_constant(code, n);
  return (RillValue){};
}
static bool descriptor(RillEval *e, const RillNode *p, RillValue env,
                       RillValue *out) {
  if (p->kind == RILL_NAME) {
    if (!rill_eval_lookup(e, env, p->text.data, out)) {
      rill_eval_error(e, RILL_TYPE, "unknown constructor");
      return false;
    }
  } else {
    RillValue v = {};
    if (!descriptor(e, p->children, env, &v))
      return false;
    if (!rill_runtime_field(v, (RillBytes){p->text.data, p->text.size}, out)) {
      rill_eval_error(e, RILL_TYPE, "unknown constructor field");
      return false;
    }
  }
  return true;
}
static bool match(RillEval *e, const RillNode *p, RillValue v,
                  RillValue lexical, RillValue code, RillValue *env) {
  if (!p)
    return false;
  if (p->kind == RILL_NAME) {
    *env = rill_eval_bind(e, *env, p->text.data, v, true);
    return !e->error.kind;
  }
  if (p->kind == RILL_NOMINAL) {
    RillValue ctor = {};
    if (!descriptor(e, p->pattern, lexical, &ctor))
      return false;
    if (ctor.kind != RILL_V_CONSTRUCTOR && ctor.kind != RILL_V_ADT) {
      rill_eval_error(e, RILL_TYPE, "pattern requires a constructor");
      return false;
    }
    RillValue desc = ctor.as.object->values[0];
    if (!p->children)
      return v.kind == RILL_V_ADT &&
             v.as.object->values[0].as.object == desc.as.object &&
             desc.as.object->count == 0;
    for (const RillNode *field = p->children->children; field;
         field = field->next) {
      if (field->kind == RILL_REST)
        continue;
      bool known = false;
      for (size_t i = 0; i < desc.as.object->count; ++i)
        if (same(desc.as.object->values[i].as.object->bytes,
                 (RillBytes){field->text.data, field->text.size}))
          known = true;
      if (!known) {
        rill_eval_error(e, RILL_TYPE, "unknown nominal pattern field");
        return false;
      }
    }
    if (v.kind != RILL_V_ADT ||
        v.as.object->values[0].as.object != desc.as.object)
      return false;
    return match(e, p->children, v.as.object->values[1], lexical, code, env);
  }
  if (p->kind == RILL_LIST) {
    if (v.kind != RILL_V_LIST && v.kind != RILL_V_SLICE)
      return false;
    size_t i = 0, n = rill_runtime_count(v);
    for (const RillNode *c = p->children; c; c = c->next) {
      if (c->kind == RILL_REST) {
        if (!c->children || (c->children->kind == RILL_NAME &&
                             !strcmp(c->children->text.data, "_")))
          return true;
        RillValue tail = rill_runtime_slice(&e->heap, v, i, n - i);
        if (tail.kind == RILL_V_UNIT) {
          rill_eval_error(e, RILL_MEMORY, "allocation failed");
          return false;
        }
        RillRoot root = {};
        rill_runtime_root(&e->heap, &root, &tail, 1);
        bool ok = match(e, c->children, tail, lexical, code, env);
        rill_runtime_unroot(&e->heap, &root);
        return ok;
      }
      if (i == n || !match(e, c, rill_runtime_at(v, i++), lexical, code, env))
        return false;
    }
    return i == n;
  }
  if (p->kind == RILL_RECORD) {
    if (v.kind != RILL_V_RECORD)
      return false;
    size_t fields = 0;
    for (const RillNode *c = p->children; c; c = c->next) {
      if (c->kind == RILL_REST) {
        if (!c->children || (c->children->kind == RILL_NAME &&
                             !strcmp(c->children->text.data, "_")))
          return true;
        RillValue rest =
            rill_eval_object(e, RILL_V_RECORD, nullptr,
                             v.as.object->count - fields * 2, nullptr, 0);
        if (e->error.kind)
          return false;
        RillRoot root = {};
        rill_runtime_root(&e->heap, &root, &rest, 1);
        size_t used = 0;
        for (size_t i = 0; i < v.as.object->count; i += 2) {
          bool selected = false;
          for (const RillNode *f = p->children; f != c; f = f->next)
            if (same(v.as.object->values[i].as.object->bytes,
                     (RillBytes){f->text.data, f->text.size}))
              selected = true;
          if (!selected) {
            rest.as.object->values[used++] = v.as.object->values[i];
            rest.as.object->values[used++] = v.as.object->values[i + 1];
          }
        }
        rest.as.object->count = used;
        bool valid = rill_runtime_record_finish(rest);
        (void)valid; // A subset of a valid record still has unique String keys.
        bool ok = match(e, c->children, rest, lexical, code, env);
        rill_runtime_unroot(&e->heap, &root);
        return ok;
      }
      RillValue field = {};
      if (!rill_runtime_field(v, (RillBytes){c->text.data, c->text.size},
                              &field) ||
          !match(e, c->children, field, lexical, code, env))
        return false;
      ++fields;
    }
    return fields == v.as.object->count / 2;
  }
  RillValue wanted = rill_eval_literal(p, code);
  if (v.kind != wanted.kind)
    return false;
  bool equal = false;
  RillError status = rill_runtime_equal(v, wanted, &equal);
  if (status)
    rill_eval_error(e, status, "pattern literal is not comparable");
  return equal;
}
bool rill_eval_bind_pattern(RillEval *e, const RillNode *p, RillValue value,
                            RillValue lexical, RillValue code, RillValue *out) {
  if (p && p->repeated) {
    rill_eval_error(e, RILL_TYPE, "duplicate name in pattern");
    return false;
  }
  return match(e, p, value, lexical, code, out);
}
