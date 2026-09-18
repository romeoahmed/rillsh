#include "../unit/test.h"
#include "runtime/runtime.h"
#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <time.h>

static uint64_t nanoseconds() {
  struct timespec now = {};
  CHECK(clock_gettime(CLOCK_MONOTONIC, &now) == 0);
  return (uint64_t)now.tv_sec * UINT64_C(1000000000) + (uint64_t)now.tv_nsec;
}
int main() {
  RillHeap heap = {.threshold = SIZE_MAX};
  RillValue retained = {};
  RillRoot root = {};
  rill_runtime_root(&heap, &root, &retained, 1);
  for (size_t i = 0; i < 100000; ++i) {
    RillValue edges[] = {retained, retained};
    retained = rill_runtime_object(&heap, RILL_V_LIST, edges, 2, nullptr, 0, 0);
    CHECK(retained.kind == RILL_V_LIST);
  }
  rill_runtime_collect(&heap);
  size_t live = heap.bytes;
  uint64_t total = 0, maximum = 0;
  for (size_t round = 0; round < 50; ++round) {
    // Isolate explicit collection from allocation time and automatic triggers.
    heap.threshold = SIZE_MAX;
    for (size_t i = 0; i < 100000; ++i)
      CHECK(rill_runtime_object(&heap, RILL_V_LIST, nullptr, 0, nullptr, 0, 0)
                .kind == RILL_V_LIST);
    uint64_t start = nanoseconds();
    rill_runtime_collect(&heap);
    uint64_t elapsed = nanoseconds() - start;
    total += elapsed;
    if (elapsed > maximum)
      maximum = elapsed;
    CHECK(heap.bytes == live);
  }
  CHECK(printf("{\"retained_bytes\":%zu,\"collections\":50,"
               "\"total_ns\":%" PRIu64 ",\"max_ns\":%" PRIu64 "}\n",
               live, total, maximum) > 0);
  rill_runtime_unroot(&heap, &root);
  rill_runtime_collect(&heap);
  CHECK(heap.bytes == 0);
  rill_runtime_heap_clear(&heap);
}
