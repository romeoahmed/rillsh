#include "diagnostic.h"
#include "private.h"
#include "runtime.h"
#include "text/text.h"
#include <assert.h>
#include <stdckdint.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

// Interior indexes follow the flexible Value array without extra padding.
static_assert(offsetof(RillObject, values) == sizeof(RillObject));
static_assert(alignof(RillValue) % alignof(RillValue *) == 0);
static_assert(alignof(RillValue) % alignof(const char *) == 0);
struct RillBudget {
  RillObject **objects;
  size_t capacity, count, bytes, limit;
};
bool rill_runtime_is_object(RillValue v) { return v.kind >= RILL_V_STRING; }
void rill_runtime_root(RillHeap *h, RillRoot *r, const RillValue *v, size_t n) {
  *r = (RillRoot){h->roots, v, n};
  h->roots = r;
}
void rill_runtime_unroot(RillHeap *h, RillRoot *r) {
  assert(h->roots == r);
  h->roots = r->previous;
}
static void mark(RillValue v, RillObject **gray, bool epoch) {
  if (!rill_runtime_is_object(v) || !v.as.object ||
      v.as.object->marked == epoch)
    return;
  RillObject *o = v.as.object;
  o->marked = epoch;
  // Leaf payloads need no worklist entry or second visit during marking.
  if (!o->count && o->kind != RILL_V_STAGE && o->kind != RILL_V_BUDGET)
    return;
  o->gray = *gray;
  *gray = o;
}
static void trace(RillObject *o, RillObject **gray, bool epoch) {
  if (o->kind == RILL_V_STAGE)
    mark(o->metadata, gray, epoch);
  for (size_t i = 0; i < o->count; ++i)
    mark(o->values[i], gray, epoch);
  if (o->kind == RILL_V_BUDGET && o->budget)
    for (size_t i = 0; i < o->budget->capacity; ++i)
      mark((RillValue){.kind = RILL_V_BUDGET,
                       .as.object = o->budget->objects[i]},
           gray, epoch);
}
void rill_runtime_collect(RillHeap *h) {
  h->epoch = !h->epoch;
  RillObject *gray = nullptr;
  for (RillRoot *r = h->roots; r; r = r->previous)
    for (size_t i = 0; i < r->count; ++i)
      mark(r->values[i], &gray, h->epoch);
  // Intrusive marking needs neither recursive calls nor scratch allocation.
  while (gray) {
    RillObject *o = gray;
    gray = o->gray;
    trace(o, &gray, h->epoch);
  }
  RillObject **link = &h->objects;
  while (*link) {
    RillObject *o = *link;
    if (o->marked == h->epoch) {
      link = &o->next;
    } else {
      *link = o->next;
      h->bytes -= o->allocation;
      if (o->kind == RILL_V_CODE && o->code)
        rill_eval_code_free(o->code);
      if (o->kind == RILL_V_BUDGET && o->budget) {
        free(o->budget->objects);
        free(o->budget);
      }
      free(o);
    }
  }
  h->threshold = h->bytes > SIZE_MAX / 2 ? SIZE_MAX : h->bytes * 2;
  if (h->threshold < 65536)
    h->threshold = 65536;
}
RillValue rill_runtime_object(RillHeap *h, RillValueKind kind,
                              const RillValue *v, size_t n, const char *s,
                              size_t len, int64_t tag) {
  if (h->stress || h->bytes >= h->threshold)
    rill_runtime_collect(h);
  bool text = kind == RILL_V_STRING || kind == RILL_V_BYTES ||
              kind == RILL_V_PATH || kind == RILL_V_ENV ||
              kind == RILL_V_BINDINGS || kind == RILL_V_DESCRIPTOR;
  assert(text || !len);
  bool indexed = kind == RILL_V_RECORD && n >= RECORD_INDEX_MIN_SLOTS;
  size_t edges = {}, index = 0, bytes = {}, total = {};
  size_t keys = kind == RILL_V_BINDINGS ? n : indexed ? n / 2 : 0;
  size_t key_size =
      kind == RILL_V_BINDINGS ? sizeof(const char *) : sizeof(RillValue *);
  if (ckd_mul(&index, keys, key_size))
    return (RillValue){};
  if (ckd_mul(&edges, n, sizeof(*v)) ||
      ckd_add(&bytes, edges, sizeof(RillObject)) ||
      ckd_add(&bytes, bytes, index) || ckd_add(&bytes, bytes, len) ||
      ckd_add(&bytes, bytes, text ? 1 : 0) || ckd_add(&total, bytes, h->bytes))
    return (RillValue){};
  RillObject *o = malloc(bytes);
  if (!o)
    return (RillValue){};
  // Trailing bytes share the allocation without imposing extra alignment.
  char *data = (char *)o + sizeof(*o) + edges + index;
  if (len)
    memcpy(data, s, len);
  if (text)
    data[len] = '\0';
  *o = (RillObject){.next = h->objects,
                    .allocation = bytes,
                    .kind = kind,
                    .marked = h->epoch,
                    .count = n};
  if (text)
    o->bytes = (RillBytes){data, len};
  else if (kind == RILL_V_CODE) {
    o->code = nullptr;
    o->tag = tag;
  } else if (kind == RILL_V_BUDGET) {
    o->budget = nullptr;
    o->tag = tag;
  } else if (kind == RILL_V_SLICE || kind == RILL_V_REDIRECT) {
    o->extent = 0;
    o->tag = tag;
  } else if (kind == RILL_V_STAGE)
    o->metadata = (RillValue){};
  else if (kind == RILL_V_RECORD)
    o->fields = nullptr;
  else if (kind == RILL_V_CLOSURE)
    o->captures = nullptr;
  if (n && v)
    memcpy(o->values, v, edges);
  else
    for (size_t i = 0; i < n; ++i)
      o->values[i] = (RillValue){};
  h->objects = o;
  h->bytes = total;
  RillValue out = {.kind = kind, .as.object = o};
  if (indexed && v && !rill_runtime_record_finish(out))
    return (RillValue){};
  return out;
}
void rill_runtime_heap_clear(RillHeap *h) {
  h->roots = nullptr;
  rill_runtime_collect(h);
  *h = (RillHeap){};
}

RillValue rill_runtime_budget(RillHeap *h, size_t limit) {
  RillValue value =
      rill_runtime_object(h, RILL_V_BUDGET, nullptr, 0, nullptr, 0, 0);
  if (value.kind == RILL_V_UNIT)
    return value;
  size_t total = {};
  if (ckd_add(&total, h->bytes, sizeof(struct RillBudget)))
    return (RillValue){};
  struct RillBudget *budget = malloc(sizeof(*budget));
  if (!budget)
    return (RillValue){};
  *budget = (struct RillBudget){.limit = limit};
  value.as.object->budget = budget;
  value.as.object->allocation += sizeof(*budget);
  h->bytes = total;
  return value;
}
static size_t bucket(const RillObject *object, size_t capacity) {
  uintptr_t key = (uintptr_t)object >> 4;
  key ^= key >> 17;
  return (size_t)key & (capacity - 1);
}
static bool grow_budget(RillHeap *h, RillObject *owner) {
  struct RillBudget *b = owner->budget;
  size_t capacity = {}, bytes = {}, total = {};
  if (ckd_mul(&capacity, b->capacity ? b->capacity : 32, 2) ||
      ckd_mul(&bytes, capacity, sizeof(*b->objects)))
    return false;
  size_t added = bytes - b->capacity * sizeof(*b->objects);
  if (ckd_add(&total, h->bytes, added))
    return false;
  RillObject **objects = calloc(capacity, sizeof(*objects));
  if (!objects)
    return false;
  for (size_t i = 0; i < b->capacity; ++i) {
    RillObject *o = b->objects[i];
    if (!o)
      continue;
    size_t slot = bucket(o, capacity);
    while (objects[slot])
      slot = (slot + 1) & (capacity - 1);
    objects[slot] = o;
  }
  free(b->objects);
  b->objects = objects;
  b->capacity = capacity;
  owner->allocation += added;
  h->bytes = total;
  return true;
}
static RillError charge(RillHeap *h, RillObject *owner, RillObject *o,
                        RillObject **gray) {
  if (!o)
    return RILL_OK;
  struct RillBudget *b = owner->budget;
  if (b->capacity) {
    size_t slot = bucket(o, b->capacity);
    while (b->objects[slot]) {
      if (b->objects[slot] == o)
        return RILL_OK;
      slot = (slot + 1) & (b->capacity - 1);
    }
  }
  size_t total = {};
  if (ckd_add(&total, b->bytes, o->allocation) || total > b->limit)
    return RILL_LIMIT;
  if (b->count >= b->capacity / 2 && !grow_budget(h, owner))
    return RILL_MEMORY;
  size_t slot = bucket(o, b->capacity);
  while (b->objects[slot])
    slot = (slot + 1) & (b->capacity - 1);
  b->objects[slot] = o;
  ++b->count;
  b->bytes = total;
  o->gray = *gray;
  *gray = o;
  return RILL_OK;
}
RillError rill_runtime_charge(RillHeap *h, RillValue budget, RillValue value) {
  assert(budget.kind == RILL_V_BUDGET);
  if (!rill_runtime_is_object(value))
    return RILL_OK;
  RillObject *gray = nullptr, *owner = budget.as.object;
  RillError status = charge(h, owner, value.as.object, &gray);
  // No safepoint runs while the intrusive worklist is in use. The budget's set
  // handles cycles and shared objects independently of the collector's marks.
  while (gray) {
    RillObject *o = gray;
    gray = o->gray;
    o->gray = nullptr;
    if (status)
      continue;
    if (o->kind == RILL_V_STAGE && rill_runtime_is_object(o->metadata))
      status = charge(h, owner, o->metadata.as.object, &gray);
    for (size_t i = 0; i < o->count && !status; ++i)
      if (rill_runtime_is_object(o->values[i]))
        status = charge(h, owner, o->values[i].as.object, &gray);
    if (o->kind == RILL_V_BUDGET && o->budget)
      for (size_t i = 0; i < o->budget->capacity && !status; ++i)
        status = charge(h, owner, o->budget->objects[i], &gray);
  }
  return status;
}
