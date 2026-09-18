/**
 * @file
 * @brief Implement pure primitives requiring representation access.
 *
 * Validate argument shapes and perform checked arithmetic, conversions, data
 * operations, and sorting. Higher-order composition remains in Rill; this
 * dispatcher never recursively invokes language callbacks.
 */
#include "pure.h"
#include "diagnostic.h"
#include "runtime/runtime.h"
#include "text/text.h"
#include "value.h"
#include <inttypes.h>
#include <math.h>
#include <stdckdint.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

static RillValue fail(RillDiagnostic *d, RillError kind, const char *message) {
  *d = (RillDiagnostic){.kind = kind, .message = message};
  return (RillValue){};
}
static bool list(RillValue v) {
  return v.kind == RILL_V_LIST || v.kind == RILL_V_SLICE;
}
static RillValue alloc(RillHeap *h, RillValueKind kind, const RillValue *v,
                       size_t n, const char *data, size_t len,
                       RillDiagnostic *d) {
  RillValue out = rill_runtime_object(h, kind, v, n, data, len, 0);
  if (out.kind == RILL_V_UNIT)
    return fail(d, RILL_MEMORY, "allocation failed");
  return out;
}
static bool bytes(RillValue v) {
  return v.kind == RILL_V_STRING || v.kind == RILL_V_BYTES ||
         v.kind == RILL_V_PATH;
}
static RillValue integer(size_t n) {
  return (RillValue){.kind = RILL_V_INT, .as.integer = (int64_t)n};
}
static int compare_value(RillValue a, RillValue b) {
  if (a.kind == RILL_V_INT)
    return (a.as.integer > b.as.integer) - (a.as.integer < b.as.integer);
  if (a.kind == RILL_V_FLOAT)
    return (a.as.real > b.as.real) - (a.as.real < b.as.real);
  RillBytes x = a.as.object->bytes, y = b.as.object->bytes;
  size_t n = x.size < y.size ? x.size : y.size;
  int order = n ? memcmp(x.data, y.data, n) : 0;
  return order ? order : (x.size > y.size) - (x.size < y.size);
}
typedef struct {
  RillValue key, value;
  size_t index;
} SortItem;
static int compare_item(const void *a, const void *b) {
  const SortItem *x = a, *y = b;
  int order = compare_value(x->key, y->key);
  return order ? order : (x->index > y->index) - (x->index < y->index);
}
static bool sort_limits(RillValue options, size_t *items, size_t *bytes,
                        RillDiagnostic *error) {
  RillLimit limits[] = {{"max_items", 1'000'000, 0},
                        {"max_bytes", (size_t)64 * 1024 * 1024, 0}};
  if (!rill_library_limits(options, limits, 2, error))
    return false;
  *items = limits[0].value;
  *bytes = limits[1].value;
  return true;
}
RillValue rill_library_pure(RillHeap *h, RillValue input, RillDiagnostic *d) {
  if (!list(input) || !rill_runtime_count(input))
    return fail(d, RILL_TYPE, "invalid primitive request");
  RillValue name = rill_runtime_at(input, 0);
  if (name.kind != RILL_V_STRING)
    return fail(d, RILL_TYPE, "primitive name requires String");
  const char *op = name.as.object->bytes.data;
  size_t argc = rill_runtime_count(input) - 1;
  RillValue a = argc ? rill_runtime_at(input, 1) : (RillValue){},
            b = argc > 1 ? rill_runtime_at(input, 2) : (RillValue){};
  if (!strcmp(op, "is_stream"))
    return (RillValue){.kind = RILL_V_BOOL,
                       .as.integer = a.kind == RILL_V_STREAM};
  if (!strcmp(op, "number")) {
    if (a.kind != RILL_V_INT && a.kind != RILL_V_FLOAT)
      return fail(d, RILL_TYPE, "expected numeric value");
    return a;
  }
  if (!strcmp(op, "length")) {
    if (list(a))
      return integer(rill_runtime_count(a));
    if (a.kind == RILL_V_RECORD)
      return integer(a.as.object->count / 2);
    return fail(d, RILL_TYPE,
                "length requires List or Record; use explicit text units");
  }
  if (!strcmp(op, "byte_length")) {
    if (!bytes(a))
      return fail(d, RILL_TYPE, "byte_length requires String, Bytes or Path");
    return integer(a.as.object->bytes.size);
  }
  if (!strcmp(op, "to_record")) {
    if (a.kind != RILL_V_ADT)
      return fail(d, RILL_TYPE, "to_record requires nominal data");
    return a.as.object->values[1];
  }
  if (!strcmp(op, "lookup")) {
    if (a.kind != RILL_V_STRING || b.kind != RILL_V_RECORD)
      return fail(d, RILL_TYPE, "lookup requires String and Record");
    RillValue result = {};
    if (rill_runtime_field(b, a.as.object->bytes, &result))
      return alloc(h, RILL_V_LIST, &result, 1, nullptr, 0, d);
    return alloc(h, RILL_V_LIST, nullptr, 0, nullptr, 0, d);
  }
  if (!strcmp(op, "take") || !strcmp(op, "drop")) {
    if (a.kind != RILL_V_INT || a.as.integer < 0 || !list(b))
      return fail(d, RILL_TYPE, "slice requires nonnegative Int and List");
    size_t n = rill_runtime_count(b),
           k = (uint64_t)a.as.integer > n ? n : (size_t)a.as.integer;
    RillValue v = rill_runtime_slice(h, b, !strcmp(op, "take") ? 0 : k,
                                     !strcmp(op, "take") ? k : n - k);
    if (v.kind == RILL_V_UNIT)
      return fail(d, RILL_MEMORY, "allocation failed");
    return v;
  }
  if (!strcmp(op, "div") || !strcmp(op, "rem")) {
    if (a.kind != RILL_V_INT || b.kind != RILL_V_INT)
      return fail(d, RILL_TYPE, "div/rem require Int");
    bool divide = !strcmp(op, "div");
    if (!b.as.integer ||
        (divide && a.as.integer == INT64_MIN && b.as.integer == -1))
      return fail(d, RILL_ARITHMETIC,
                  "integer division overflow or zero divisor");
    return (RillValue){.kind = RILL_V_INT,
                       .as.integer = divide ? a.as.integer / b.as.integer
                                     : b.as.integer == -1
                                         ? 0
                                         : a.as.integer % b.as.integer};
  }
  if (!strcmp(op, "float")) {
    if (a.kind == RILL_V_FLOAT)
      return a;
    if (a.kind != RILL_V_INT)
      return fail(d, RILL_TYPE, "float requires Int or Float");
    return (RillValue){.kind = RILL_V_FLOAT, .as.real = (double)a.as.integer};
  }
  if (!strcmp(op, "int")) {
    if (a.kind == RILL_V_INT)
      return a;
    if (a.kind != RILL_V_FLOAT)
      return fail(d, RILL_TYPE, "int requires Int or Float");
    if (!isfinite(a.as.real) || a.as.real < -0x1p63 || a.as.real >= 0x1p63)
      return fail(d, RILL_ARITHMETIC, "Float outside Int range");
    return (RillValue){.kind = RILL_V_INT, .as.integer = (int64_t)a.as.real};
  }
  if (!strcmp(op, "text")) {
    if (a.kind == RILL_V_STRING)
      return a;
    [[gnu::cleanup(rill_text_clear)]] RillBuffer out = {};
    bool ok = false;
    if (a.kind == RILL_V_INT)
      ok = rill_text_format(&out, "%" PRId64, a.as.integer);
    else if (a.kind == RILL_V_FLOAT)
      ok = rill_text_format(&out, "%.17g", a.as.real);
    else if (a.kind == RILL_V_BOOL)
      ok = rill_text_append(&out, a.as.integer ? "true" : "false",
                            a.as.integer ? 4 : 5);
    else
      return fail(d, RILL_TYPE, "text requires String, numeric or Bool data");
    return ok ? alloc(h, RILL_V_STRING, nullptr, 0, out.data, out.size, d)
              : fail(d, RILL_MEMORY, "allocation failed");
  }
  if (!strcmp(op, "encode_utf8") || !strcmp(op, "decode_utf8")) {
    bool encode = !strcmp(op, "encode_utf8");
    if (a.kind != (encode ? RILL_V_STRING : RILL_V_BYTES))
      return fail(d, RILL_TYPE, "invalid UTF-8 conversion input");
    if (!encode &&
        !rill_text_valid(a.as.object->bytes.data, a.as.object->bytes.size))
      return fail(d, RILL_TYPE, "invalid UTF-8");
    return alloc(h, encode ? RILL_V_BYTES : RILL_V_STRING, nullptr, 0,
                 a.as.object->bytes.data, a.as.object->bytes.size, d);
  }
  if (!strcmp(op, "starts_with") || !strcmp(op, "ends_with")) {
    if (a.kind != RILL_V_STRING || b.kind != RILL_V_STRING)
      return fail(d, RILL_TYPE, "text predicate requires Strings");
    RillBytes prefix = a.as.object->bytes, value = b.as.object->bytes;
    bool result =
        prefix.size <= value.size &&
        (!prefix.size ||
         !memcmp(prefix.data,
                 value.data +
                     (!strcmp(op, "ends_with") ? value.size - prefix.size : 0),
                 prefix.size));
    return (RillValue){.kind = RILL_V_BOOL, .as.integer = result};
  }
  if (!strcmp(op, "scalars")) {
    if (a.kind != RILL_V_STRING)
      return fail(d, RILL_TYPE, "scalars requires String");
    RillBytes value = a.as.object->bytes;
    size_t count = 0, at = 0;
    while (at < value.size) {
      uint32_t cp = {};
      if (!rill_text_decode(value.data, value.size, &at, &cp))
        return fail(d, RILL_TYPE, "invalid UTF-8");
      ++count;
    }
    RillValue v = alloc(h, RILL_V_LIST, nullptr, count, nullptr, 0, d);
    if (d->kind)
      return v;
    RillRoot root = {};
    rill_runtime_root(h, &root, &v, 1);
    at = 0;
    for (size_t i = 0; i < count; ++i) {
      uint32_t cp = {};
      size_t start = at;
      bool valid = rill_text_decode(value.data, value.size, &at, &cp);
      (void)valid;
      v.as.object->values[i] = alloc(h, RILL_V_STRING, nullptr, 0,
                                     value.data + start, at - start, d);
      if (d->kind)
        break;
    }
    rill_runtime_unroot(h, &root);
    return v;
  }
  if (!strcmp(op, "record") || !strcmp(op, "extend")) {
    bool extend = !strcmp(op, "extend");
    if (extend ? (a.kind != RILL_V_RECORD || b.kind != RILL_V_RECORD)
               : !list(a))
      return fail(d, RILL_TYPE, "invalid record construction input");
    size_t count = extend ? a.as.object->count : rill_runtime_count(a);
    if (!extend && ckd_mul(&count, count, 2))
      return fail(d, RILL_MEMORY, "record size overflow");
    if (extend && ckd_add(&count, count, b.as.object->count))
      return fail(d, RILL_MEMORY, "record size overflow");
    RillValue v = alloc(h, RILL_V_RECORD, nullptr, count, nullptr, 0, d);
    if (d->kind)
      return v;
    RillValue *pairs = v.as.object->values;
    for (size_t i = 0; i < count; i += 2) {
      if (extend) {
        RillObject *o = i < a.as.object->count ? a.as.object : b.as.object;
        size_t at = i < a.as.object->count ? i : i - a.as.object->count;
        pairs[i] = o->values[at];
        pairs[i + 1] = o->values[at + 1];
      } else {
        RillValue pair = rill_runtime_at(a, i / 2);
        if (!list(pair) || rill_runtime_count(pair) != 2) {
          *d = (RillDiagnostic){.kind = RILL_TYPE,
                                .message = "record requires key/value pairs"};
          break;
        }
        pairs[i] = rill_runtime_at(pair, 0);
        pairs[i + 1] = rill_runtime_at(pair, 1);
      }
      if (pairs[i].kind != RILL_V_STRING) {
        *d = (RillDiagnostic){.kind = RILL_TYPE,
                              .message = "record keys require String"};
        break;
      }
    }
    if (d->kind)
      return (RillValue){};
    if (!rill_runtime_record_finish(v))
      return fail(d, RILL_TYPE, "duplicate record key");
    return v;
  }

  if (!strcmp(op, "concat") || !strcmp(op, "reverse") ||
      !strcmp(op, "unroll")) {
    bool concat = !strcmp(op, "concat"), chain = !strcmp(op, "unroll");
    if (concat && a.kind == RILL_V_BYTES && b.kind == RILL_V_BYTES) {
      [[gnu::cleanup(rill_text_clear)]] RillBuffer out = {};
      bool ok = rill_text_append(&out, a.as.object->bytes.data,
                                 a.as.object->bytes.size) &&
                rill_text_append(&out, b.as.object->bytes.data,
                                 b.as.object->bytes.size);
      return ok ? alloc(h, RILL_V_BYTES, nullptr, 0, out.data, out.size, d)
                : fail(d, RILL_MEMORY, "allocation failed");
    }
    if (!list(a) || (concat && !list(b)))
      return fail(d, RILL_TYPE, "sequence operation requires Lists");
    size_t count = rill_runtime_count(a);
    if (chain) {
      count = 0;
      for (RillValue p = a; list(p) && rill_runtime_count(p) == 2;
           p = rill_runtime_at(p, 1))
        ++count;
    }
    if (concat && ckd_add(&count, count, rill_runtime_count(b)))
      return fail(d, RILL_MEMORY, "List size overflow");
    RillValue v = alloc(h, RILL_V_LIST, nullptr, count, nullptr, 0, d);
    if (d->kind)
      return v;
    RillValue *items = v.as.object->values;
    RillValue p = a;
    for (size_t i = 0; i < count; ++i) {
      if (chain) {
        items[count - i - 1] = rill_runtime_at(p, 0);
        p = rill_runtime_at(p, 1);
      } else if (concat)
        items[i] = i < rill_runtime_count(a)
                       ? rill_runtime_at(a, i)
                       : rill_runtime_at(b, i - rill_runtime_count(a));
      else
        items[i] = rill_runtime_at(a, count - i - 1);
    }
    return v;
  }
  if (!strcmp(op, "sort_input")) {
    size_t max_items = {}, max_bytes = {};
    if (!sort_limits(a, &max_items, &max_bytes, d))
      return (RillValue){};
    if (!list(b))
      return fail(d, RILL_TYPE, "sort requires List");
    if (rill_runtime_count(b) > max_items)
      return fail(d, RILL_LIMIT, "sort input exceeds materialization limits");
    RillValue budget = rill_runtime_budget(h, max_bytes);
    if (budget.kind == RILL_V_UNIT)
      return fail(d, RILL_MEMORY, "allocation failed");
    RillError status =
        rill_runtime_count(b) ? rill_runtime_charge(h, budget, b) : RILL_OK;
    return status ? fail(d, status, "sort input exceeds materialization limits")
                  : budget;
  }
  if (!strcmp(op, "sort_key")) {
    if (a.kind != RILL_V_BUDGET)
      return fail(d, RILL_TYPE, "sort requires retained-value budget");
    if ((b.kind != RILL_V_INT && b.kind != RILL_V_FLOAT &&
         b.kind != RILL_V_STRING) ||
        (a.as.object->tag && a.as.object->tag != b.kind))
      return fail(d, RILL_TYPE,
                  "sort keys must be homogeneous comparable values");
    a.as.object->tag = b.kind;
    RillError status = rill_runtime_charge(h, a, b);
    if (status)
      return fail(d, status, "sort values and keys exceed byte limit");
    return b;
  }
  if (!strcmp(op, "sort")) {
    if (!list(a))
      return fail(d, RILL_TYPE, "sort requires decorated List");
    size_t count = rill_runtime_count(a);
    SortItem *items = calloc(count ? count : 1, sizeof(*items));
    if (!items) {
      return fail(d, RILL_MEMORY, "allocation failed");
    }
    RillValueKind kind = RILL_V_UNIT;
    for (size_t i = 0; i < count; ++i) {
      RillValue pair = rill_runtime_at(a, i);
      if (!list(pair) || rill_runtime_count(pair) != 2) {
        *d = (RillDiagnostic){.kind = RILL_TYPE,
                              .message = "invalid sort decoration"};
        break;
      }
      items[i] =
          (SortItem){rill_runtime_at(pair, 0), rill_runtime_at(pair, 1), i};
      if (!i)
        kind = items[i].key.kind;
      if (items[i].key.kind != kind ||
          (kind != RILL_V_INT && kind != RILL_V_FLOAT &&
           kind != RILL_V_STRING)) {
        *d = (RillDiagnostic){
            .kind = RILL_TYPE,
            .message = "sort keys must be homogeneous comparable values"};
        break;
      }
    }
    RillValue out = d->kind
                        ? (RillValue){}
                        : alloc(h, RILL_V_LIST, nullptr, count, nullptr, 0, d);
    if (!d->kind) {
      qsort(items, count, sizeof(*items), compare_item);
      for (size_t i = 0; i < count; ++i)
        out.as.object->values[i] = items[i].value;
    }
    free(items);
    return out;
  }
  return fail(d, RILL_TYPE, "unknown pure primitive");
}
