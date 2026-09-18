/**
 * @file
 * @brief Measure scoped-handle checks over a shared data graph.
 *
 * Keep an unrelated Stream token live so the graph walk cannot be skipped.
 * Report traversal time and additional charged heap storage.
 */
#include "diagnostic.h"
#include "runtime/runtime.h"
#include "timing.h"
#include <inttypes.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>

int main() {
  RillHeap heap = {};
  RillValue values[2] = {};
  RillRoot root = {};
  rill_runtime_root(&heap, &root, values, 2);
  values[0] =
      rill_runtime_object(&heap, RILL_V_STREAM, nullptr, 0, nullptr, 0, 0);
  CHECK(values[0].kind == RILL_V_STREAM);
  for (size_t i = 0; i < 20000; ++i) {
    RillValue edges[] = {values[1], values[1]};
    values[1] =
        rill_runtime_object(&heap, RILL_V_LIST, edges, 2, nullptr, 0, 0);
    CHECK(values[1].kind == RILL_V_LIST);
  }
  rill_runtime_collect(&heap);
  size_t retained = heap.bytes, extra = 0;
  uint64_t total = 0;
  constexpr size_t iterations = 200;
  for (size_t round = 0; round < iterations; ++round) {
    uint64_t start = nanoseconds();
    RillError error = rill_runtime_persistent(&heap, values[1]);
    total += nanoseconds() - start;
    CHECK(error == RILL_OK);
    if (heap.bytes > retained && heap.bytes - retained > extra)
      extra = heap.bytes - retained;
    rill_runtime_collect(&heap);
  }
  CHECK(printf("{\"iterations\":%zu,\"walk_ns\":%" PRIu64
               ",\"retained_bytes\":%zu,\"extra_heap_bytes\":%zu}\n",
               iterations, total, retained, extra) > 0);
  rill_runtime_unroot(&heap, &root);
  rill_runtime_heap_clear(&heap);
}
