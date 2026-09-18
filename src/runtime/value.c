/**
 * @file
 * @brief Immutable List/Record operations and graph-aware equality.
 *
 * Slices retain backing Lists; Record indexes borrow keys in their own payload.
 * Equality validates supported data before identity or mismatch shortcuts. A
 * bounded tree walk handles small values; graph validation and union/find avoid
 * repeated expansion of shared data. Scratch never crosses a GC safepoint.
 */
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

static constexpr size_t MAX_DATA_DEPTH = 65'536;
size_t rill_runtime_count(RillValue list) {
  return list.kind == RILL_V_SLICE ? list.as.object->extent
                                   : list.as.object->count;
}
RillValue rill_runtime_at(RillValue list, size_t index) {
  if (list.kind == RILL_V_SLICE) {
    index += (size_t)list.as.object->tag;
    list = list.as.object->values[0];
  }
  return list.as.object->values[index];
}
RillValue rill_runtime_slice(RillHeap *heap, RillValue list, size_t start,
                             size_t count) {
  if (start == 0 && count == rill_runtime_count(list))
    return list;
  if (count == 0)
    return rill_runtime_object(heap, RILL_V_LIST, nullptr, 0, nullptr, 0, 0);
  if (list.kind == RILL_V_SLICE) {
    start += (size_t)list.as.object->tag;
    list = list.as.object->values[0];
  }
  RillValue slice = rill_runtime_object(heap, RILL_V_SLICE, &list, 1, nullptr,
                                        0, (int64_t)start);
  if (slice.kind != RILL_V_UNIT)
    slice.as.object->extent = count;
  return slice;
}
static int compare_key(RillBytes a, RillBytes b) {
  size_t count = a.size < b.size ? a.size : b.size;
  int order = count ? memcmp(a.data, b.data, count) : 0;
  return order ? order : (a.size > b.size) - (a.size < b.size);
}
static int compare_field(const void *a, const void *b) {
  RillValue *const *x = a, *const *y = b;
  return compare_key((*x)->as.object->bytes, (*y)->as.object->bytes);
}
bool rill_runtime_record_finish(RillValue record) {
  RillObject *o = record.as.object;
  o->fields = nullptr;
  if (o->count % 2)
    return false;
  for (size_t i = 0; i < o->count; i += 2)
    if (o->values[i].kind != RILL_V_STRING)
      return false;
  if (o->count < RECORD_INDEX_MIN_SLOTS) {
    for (size_t i = 0; i < o->count; i += 2)
      for (size_t j = 0; j < i; j += 2)
        if (!compare_key(o->values[i].as.object->bytes,
                         o->values[j].as.object->bytes))
          return false;
    return true;
  }
  // Value-array alignment also satisfies pointer alignment on the target ABIs.
  RillValue **fields = (void *)(o->values + o->count);
  for (size_t i = 0; i < o->count / 2; ++i)
    fields[i] = &o->values[i * 2];
  qsort(fields, o->count / 2, sizeof(*fields), compare_field);
  for (size_t i = 1; i < o->count / 2; ++i)
    if (!compare_field(fields + i - 1, fields + i))
      return false;
  o->fields = fields;
  return true;
}
size_t rill_runtime_field_index(RillValue record, RillBytes key) {
  RillObject *o = record.as.object;
  if (o->fields) {
    size_t lo = 0, hi = o->count / 2;
    while (lo < hi) {
      size_t mid = lo + (hi - lo) / 2;
      RillValue *field = o->fields[mid];
      int order = compare_key(key, field->as.object->bytes);
      if (order < 0)
        hi = mid;
      else if (order > 0)
        lo = mid + 1;
      else
        return (size_t)(field - o->values);
    }
  } else
    for (size_t i = 0; i < o->count; i += 2)
      if (!compare_key(key, o->values[i].as.object->bytes))
        return i;
  return o->count;
}
bool rill_runtime_field(RillValue value, RillBytes key, RillValue *out) {
  if (value.kind == RILL_V_ADT)
    value = value.as.object->values[1];
  if (value.kind != RILL_V_RECORD)
    return false;
  size_t index = rill_runtime_field_index(value, key);
  if (index == value.as.object->count)
    return false;
  *out = value.as.object->values[index + 1];
  return true;
}
RillValue rill_runtime_record(RillHeap *heap, const RillValue *pairs,
                              size_t count) {
  return rill_runtime_object(heap, RILL_V_RECORD, pairs, count, nullptr, 0, 0);
}
RillValue rill_runtime_record_copy(RillHeap *heap, RillValue record) {
  assert(record.kind == RILL_V_RECORD);
  RillObject *source = record.as.object;
  RillValue copy = rill_runtime_record(heap, nullptr, source->count);
  if (copy.kind == RILL_V_UNIT)
    return copy;
  RillObject *out = copy.as.object;
  memcpy(out->values, source->values, source->count * sizeof(RillValue));
  if (source->fields) {
    out->fields = (void *)(out->values + out->count);
    for (size_t i = 0; i < out->count / 2; ++i)
      out->fields[i] = out->values + (source->fields[i] - source->values);
  }
  return copy;
}
// Per-comparison scratch never survives the call or crosses a GC safepoint.
// A completed validation memo stores subtree height, preserving the depth limit
// when the same DAG node is reached through a longer path.
typedef struct Memo {
  const RillObject *object;
  struct Memo *parent;
  size_t height;
} Memo;
typedef struct {
  Memo *entries;
  size_t count, capacity;
  Memo local[64];
} Seen;
static size_t memo_bucket(const RillObject *object, size_t capacity) {
  uintptr_t key = (uintptr_t)object >> 3;
  key ^= key >> 17;
  return (size_t)key & (capacity - 1);
}
static Memo *find_memo(Seen *seen, const RillObject *object) {
  size_t slot = memo_bucket(object, seen->capacity);
  while (seen->entries[slot].object && seen->entries[slot].object != object)
    slot = (slot + 1) & (seen->capacity - 1);
  return &seen->entries[slot];
}
static bool add_memo(Seen *seen, const RillObject *object) {
  if (seen->count == seen->capacity / 2) {
    size_t capacity = {}, bytes = {};
    if (ckd_mul(&capacity, seen->capacity, 2) ||
        ckd_mul(&bytes, capacity, sizeof(Memo)))
      return false;
    Memo *entries = malloc(bytes);
    if (!entries)
      return false;
    for (size_t i = 0; i < capacity; ++i)
      entries[i] = (Memo){};
    Memo *old = seen->entries;
    size_t old_capacity = seen->capacity;
    seen->entries = entries;
    seen->capacity = capacity;
    for (size_t i = 0; i < old_capacity; ++i)
      if (old[i].object)
        *find_memo(seen, old[i].object) = old[i];
    if (old != seen->local)
      free(old);
  }
  *find_memo(seen, object) = (Memo){.object = object, .height = SIZE_MAX};
  ++seen->count;
  return true;
}
static Memo *representative(Memo *memo) {
  if (!memo->parent) {
    memo->parent = memo;
    memo->height = 0;
  }
  while (memo->parent != memo) {
    memo->parent = memo->parent->parent;
    memo = memo->parent;
  }
  return memo;
}
static bool equated(Seen *seen, const RillObject *a, const RillObject *b) {
  // Validation registered every aggregate. The table cannot grow now, so
  // interior parent pointers stay stable. Reuse completed heights as ranks.
  Memo *x = representative(find_memo(seen, a));
  Memo *y = representative(find_memo(seen, b));
  if (x == y)
    return true;
  if (x->height < y->height)
    x->parent = y;
  else {
    y->parent = x;
    if (x->height == y->height)
      ++x->height;
  }
  // Joining equivalence classes obliges the caller to compare their children
  // before reporting equality. Transitivity avoids a Cartesian product of
  // pairs when equal DAGs have different sharing shapes.
  return false;
}
static bool aggregate(RillValue v) {
  return v.kind == RILL_V_LIST || v.kind == RILL_V_SLICE ||
         v.kind == RILL_V_RECORD || v.kind == RILL_V_ADT;
}
static bool scalar(RillValue v) {
  return v.kind == RILL_V_UNIT || v.kind == RILL_V_NULL ||
         v.kind == RILL_V_INT || v.kind == RILL_V_FLOAT ||
         v.kind == RILL_V_BOOL || v.kind == RILL_V_STRING ||
         v.kind == RILL_V_BYTES || v.kind == RILL_V_PATH;
}
static bool equal_scalar(RillValue a, RillValue b) {
  if (a.kind != b.kind)
    return false;
  if (a.kind == RILL_V_FLOAT)
    return a.as.real == b.as.real;
  if (rill_runtime_is_object(a))
    return !compare_key(a.as.object->bytes, b.as.object->bytes);
  return a.kind == RILL_V_INT || a.kind == RILL_V_BOOL
             ? a.as.integer == b.as.integer
             : true;
}
typedef struct {
  RillValue value;
  size_t next, height;
} Visit;
static RillError comparable(RillValue value, Seen *seen) {
  if (!aggregate(value))
    return scalar(value) ? RILL_OK : RILL_TYPE;
  Visit local[64], *stack = local;
  size_t depth = 1, capacity = 64;
  stack[0] = (Visit){.value = value};
  RillError status = RILL_OK;
  size_t fuel = 64;
  while (depth && !status) {
    // A bounded tree walk avoids memo setup for small values. Exhaustion is
    // internal: the caller restarts with graph validation before returning.
    if (!seen && !fuel--) {
      status = RILL_LIMIT;
      break;
    }
    Visit *v = &stack[depth - 1];
    RillValue x = v->value;
    if (aggregate(x)) {
      if (seen && !v->next) {
        Memo *memo = find_memo(seen, x.as.object);
        if (memo->object) {
          if (memo->height > MAX_DATA_DEPTH - depth) {
            status = RILL_LIMIT; // Includes an active (cyclic) object.
            break;
          }
          v->height = memo->height;
          goto complete;
        }
        if (!add_memo(seen, x.as.object)) {
          status = RILL_MEMORY;
          break;
        }
      }
      if (x.kind == RILL_V_ADT && !v->next)
        v->next = 1; // Nominal identity is not a data leaf.
      if (v->next == rill_runtime_count(x)) {
        if (seen)
          find_memo(seen, x.as.object)->height = v->height;
        goto complete;
      }
      RillValue child = rill_runtime_at(x, v->next++);
      if (depth == MAX_DATA_DEPTH) {
        status = RILL_LIMIT;
        break;
      }
      if (scalar(child)) {
        if (!v->height)
          v->height = 1;
        continue;
      }
      if (depth == capacity) {
        if (!seen) {
          status = RILL_LIMIT;
          break;
        }
        size_t next = capacity * 2;
        Visit *grown = malloc(next * sizeof(*grown));
        if (!grown) {
          status = RILL_MEMORY;
          break;
        }
        memcpy(grown, stack, depth * sizeof(*grown));
        if (stack != local)
          free(stack);
        stack = grown;
        capacity = next;
      }
      stack[depth++] = (Visit){.value = child};
      continue;
    }
    if (!scalar(x)) {
      status = RILL_TYPE;
      break;
    }
  complete:
    size_t height = stack[--depth].height + 1;
    if (depth && stack[depth - 1].height < height)
      stack[depth - 1].height = height;
  }
  if (stack != local)
    free(stack);
  return status;
}
typedef struct {
  RillValue a, b;
  size_t next;
} Compare;
static RillError compare(RillValue a, RillValue b, bool *equal, Seen *seen) {
  RillError status = RILL_OK;
  if (!rill_runtime_is_object(a) && !rill_runtime_is_object(b)) {
    *equal = equal_scalar(a, b);
    return RILL_OK;
  }
  Compare local[64], *stack = local;
  size_t depth = 1, capacity = 64;
  stack[0] = (Compare){a, b, 0};
  *equal = true;
  size_t fuel = 64;
  while (depth && *equal) {
    if (!seen && !fuel--) {
      status = RILL_LIMIT;
      break;
    }
    Compare *f = &stack[depth - 1];
    a = f->a;
    b = f->b;
    bool lists = (a.kind == RILL_V_LIST || a.kind == RILL_V_SLICE) &&
                 (b.kind == RILL_V_LIST || b.kind == RILL_V_SLICE);
    if (a.kind != b.kind && !lists) {
      *equal = false;
      break;
    }
    // Validation above must precede identity: a shared function-containing
    // record is still not comparable.
    if (rill_runtime_is_object(a) && a.as.object == b.as.object) {
      --depth;
      continue;
    }
    if (lists || a.kind == RILL_V_RECORD || a.kind == RILL_V_ADT) {
      if (seen && !f->next && equated(seen, a.as.object, b.as.object)) {
        --depth;
        continue;
      }
      size_t n = rill_runtime_count(a);
      if (n != rill_runtime_count(b)) {
        *equal = false;
        break;
      }
      if (a.kind == RILL_V_ADT && f->next == 0) {
        if (a.as.object->values[0].as.object !=
            b.as.object->values[0].as.object) {
          *equal = false;
          break;
        }
        f->next = 1;
      }
      if (f->next == n) {
        --depth;
        continue;
      }
      RillValue x = {}, y = {};
      if (a.kind == RILL_V_RECORD) {
        if (a.as.object->fields && b.as.object->fields) {
          // Both indexes have the same sorted key order. Zip them instead of
          // doing a binary lookup for every field; presentation is untouched.
          RillValue *left = a.as.object->fields[f->next / 2];
          RillValue *right = b.as.object->fields[f->next / 2];
          if (compare_key(left->as.object->bytes, right->as.object->bytes)) {
            *equal = false;
            break;
          }
          x = left[1];
          y = right[1];
        } else {
          RillValue key = a.as.object->values[f->next];
          x = a.as.object->values[f->next + 1];
          if (!rill_runtime_field(b, key.as.object->bytes, &y)) {
            *equal = false;
            break;
          }
        }
        f->next += 2;
      } else {
        x = rill_runtime_at(a, f->next);
        y = rill_runtime_at(b, f->next++);
      }
      if (depth == MAX_DATA_DEPTH) {
        status = RILL_LIMIT;
        break;
      }
      if (scalar(x) && scalar(y)) {
        *equal = equal_scalar(x, y);
        continue;
      }
      if (depth == capacity) {
        if (!seen) {
          status = RILL_LIMIT;
          break;
        }
        size_t next = capacity * 2;
        Compare *grown = malloc(next * sizeof(*grown));
        if (!grown) {
          status = RILL_MEMORY;
          break;
        }
        memcpy(grown, stack, depth * sizeof(*grown));
        if (stack != local)
          free(stack);
        stack = grown;
        capacity = next;
      }
      stack[depth++] = (Compare){x, y, 0};
    } else {
      *equal = equal_scalar(a, b);
      --depth;
    }
  }
  if (stack != local)
    free(stack);
  return status;
}

RillError rill_runtime_equal(RillValue a, RillValue b, bool *equal) {
  if (scalar(a) && scalar(b)) {
    *equal = equal_scalar(a, b);
    return RILL_OK;
  }
  RillError status = comparable(a, nullptr);
  if (!status)
    status = comparable(b, nullptr);
  if (!status)
    status = compare(a, b, equal, nullptr);
  if (status != RILL_LIMIT)
    return status;
  Seen seen = {.capacity = 64};
  seen.entries = seen.local;
  status = comparable(a, &seen);
  if (!status)
    status = comparable(b, &seen);
  if (!status)
    status = compare(a, b, equal, &seen);
  if (seen.entries != seen.local)
    free(seen.entries);
  return status;
}
