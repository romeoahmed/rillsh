/**
 * @file
 * @brief Measure equality of differently shared, equal DAGs.
 *
 * Layered graphs have equal unfoldings but different transitions, exercising
 * equivalence classes instead of only matching-shape traversal.
 */
#include "../unit/test.h"
#include "diagnostic.h"
#include "runtime/runtime.h"
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>

int main() {
  enum { WIDTH = 32, LEVELS = 96 };
  RillHeap heap = {.threshold = SIZE_MAX};
  RillValue rows[4 * WIDTH] = {};
  RillRoot root = {};
  rill_runtime_root(&heap, &root, rows, sizeof(rows) / sizeof(*rows));
  // Two layered DAGs have equal unfoldings but different transitions. Paired
  // traversal revisits many combinations; equivalence classes collapse them.
  for (size_t level = 0; level < LEVELS; ++level) {
    size_t old = (level % 2) * 2, next = ((level + 1) % 2) * 2;
    for (size_t i = 0; i < WIDTH; ++i) {
      RillValue left[] = {rows[old * WIDTH + i],
                          rows[old * WIDTH + (i + 1) % WIDTH],
                          rows[old * WIDTH + i]};
      RillValue right[] = {rows[(old + 1) * WIDTH + i],
                           rows[(old + 1) * WIDTH + i],
                           rows[(old + 1) * WIDTH + (i + 1) % WIDTH]};
      rows[next * WIDTH + i] =
          rill_runtime_object(&heap, RILL_V_LIST, left, 3, nullptr, 0, 0);
      rows[(next + 1) * WIDTH + i] =
          rill_runtime_object(&heap, RILL_V_LIST, right, 3, nullptr, 0, 0);
      CHECK(rows[next * WIDTH + i].kind == RILL_V_LIST);
      CHECK(rows[(next + 1) * WIDTH + i].kind == RILL_V_LIST);
    }
  }
  rill_runtime_collect(&heap);
  for (size_t i = 0; i < 16; ++i) {
    bool equal = false;
    CHECK(rill_runtime_equal(rows[0], rows[WIDTH], &equal) == RILL_OK);
    CHECK(equal);
  }
  CHECK(printf("{\"retained_bytes\":%zu}\n", heap.bytes) > 0);
  rill_runtime_unroot(&heap, &root);
  rill_runtime_heap_clear(&heap);
}
