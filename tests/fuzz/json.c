/**
 * @file
 * @brief Fuzz strict JSON conversion and supported-value round trips.
 *
 * Each input owns a fresh heap; small inputs enable stress GC. Successful
 * decoding must re-encode without a type error and preserve structural values
 * when it fits the output budget.
 */
#include "native/json.h"
#include "diagnostic.h"
#include "runtime/runtime.h"
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>

int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size);
int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size) {
  RillHeap heap = {.stress = size <= 1024};
  RillValue values[5] = {};
  RillRoot root = {};
  rill_runtime_root(&heap, &root, values, 5);
  values[0] = rill_runtime_record(&heap, nullptr, 0);
  values[1] = rill_runtime_object(&heap, RILL_V_BYTES, nullptr, 0,
                                  (const char *)data, size, 0);
  if (values[0].kind != RILL_V_RECORD || values[1].kind != RILL_V_BYTES)
    goto done;
  RillDiagnostic error = {};
  values[2] = rill_library_json(&heap, false, values[0], values[1], &error);
  if (error.kind) {
    if (error.kind != RILL_DECODE && error.kind != RILL_LIMIT &&
        error.kind != RILL_MEMORY)
      abort();
    goto done;
  }
  values[3] = rill_library_json(&heap, true, values[0], values[2], &error);
  if (error.kind) {
    // Escaping can exceed the output budget; supported decoded values must
    // never become a TypeError on encoding.
    if (error.kind != RILL_LIMIT && error.kind != RILL_MEMORY)
      abort();
    goto done;
  }
  values[4] = rill_library_json(&heap, false, values[0], values[3], &error);
  if (error.kind == RILL_MEMORY)
    goto done;
  bool equal = false;
  RillError compared = rill_runtime_equal(values[2], values[4], &equal);
  if (error.kind || (compared != RILL_MEMORY && (compared || !equal)))
    abort();
done:
  rill_runtime_unroot(&heap, &root);
  rill_runtime_heap_clear(&heap);
  return 0;
}
