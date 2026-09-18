/**
 * @file
 * @brief Construct adapter Records and validate explicit size limits.
 *
 * Record construction roots input Values and the private builder across key
 * allocation. Option parsing updates caller-owned defaults without allocating;
 * a failed validation may leave earlier options applied.
 */
#include "value.h"
#include "diagnostic.h"
#include "runtime/runtime.h"
#include "text/text.h"
#include <stdckdint.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>

bool rill_library_limits(RillValue options, RillLimit *limits, size_t count,
                         RillDiagnostic *error) {
  if (options.kind != RILL_V_RECORD) {
    *error = (RillDiagnostic){.kind = RILL_TYPE,
                              .message = "options require Record"};
    return false;
  }
  for (size_t i = 0; i < options.as.object->count; i += 2) {
    RillBytes key = options.as.object->values[i].as.object->bytes;
    RillValue value = options.as.object->values[i + 1];
    size_t j = 0;
    while (j < count && (key.size != strlen(limits[j].name) ||
                         memcmp(key.data, limits[j].name, key.size) != 0))
      ++j;
    if (j == count || value.kind != RILL_V_INT || value.as.integer < 0 ||
        (uint64_t)value.as.integer > SIZE_MAX ||
        (size_t)value.as.integer < limits[j].minimum) {
      *error = (RillDiagnostic){.kind = RILL_TYPE,
                                .message = "unknown or invalid limit option"};
      return false;
    }
    limits[j].value = (size_t)value.as.integer;
  }
  return true;
}
RillValue rill_library_record(RillHeap *h, const char *const *keys,
                              const RillValue *values, size_t count) {
  size_t slots = {};
  if (ckd_mul(&slots, count, 2))
    return (RillValue){};
  RillRoot input_root = {};
  rill_runtime_root(h, &input_root, values, count);
  RillValue out = rill_runtime_record(h, nullptr, slots);
  if (out.kind == RILL_V_UNIT) {
    rill_runtime_unroot(h, &input_root);
    return out;
  }
  RillRoot root = {};
  rill_runtime_root(h, &root, &out, 1);
  RillValue *pairs = out.as.object->values;
  for (size_t i = 0; i < count; ++i)
    pairs[i * 2 + 1] = values[i];
  bool ok = true;
  for (size_t i = 0; i < count; ++i) {
    pairs[i * 2] = rill_runtime_object(h, RILL_V_STRING, nullptr, 0, keys[i],
                                       strlen(keys[i]), 0);
    if (pairs[i * 2].kind == RILL_V_UNIT) {
      ok = false;
      break;
    }
  }
  if (!ok || !rill_runtime_record_finish(out))
    out = (RillValue){};
  rill_runtime_unroot(h, &root);
  rill_runtime_unroot(h, &input_root);
  return out;
}
