/**
 * @file
 * @brief Measure wide and deeply nested JSON conversion separately.
 *
 * Build input before measurement, verify decoded shape, and report conversion
 * time separately from explicit collection.
 */
#include "library/json.h"
#include "bench.h"
#include "diagnostic.h"
#include "runtime/runtime.h"
#include "text/text.h"
#include <inttypes.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

int main(int argc, char **argv) {
  CHECK(argc == 2);
  bool wide = !strcmp(argv[1], "wide");
  CHECK(wide || !strcmp(argv[1], "deep"));
  [[gnu::cleanup(rill_text_clear)]] RillBuffer input = {};
  if (wide) {
    CHECK(rill_text_append(&input, "[", 1));
    for (size_t i = 0; i < 200000; ++i)
      CHECK(rill_text_append(&input, i ? ",0" : "0", i ? 2 : 1));
    CHECK(rill_text_append(&input, "]", 1));
  } else {
    for (size_t i = 0; i < 128; ++i)
      CHECK(rill_text_append(&input, "[", 1));
    CHECK(rill_text_append(&input, "0", 1));
    for (size_t i = 0; i < 128; ++i)
      CHECK(rill_text_append(&input, "]", 1));
  }
  RillHeap heap = {};
  RillValue values[3] = {};
  RillRoot root = {};
  rill_runtime_root(&heap, &root, values, 3);
  values[0] = rill_runtime_record(&heap, nullptr, 0);
  values[1] = rill_runtime_object(&heap, RILL_V_STRING, nullptr, 0, input.data,
                                  input.size, 0);
  CHECK(values[0].kind == RILL_V_RECORD && values[1].kind == RILL_V_STRING);
  uint64_t total = 0;
  size_t retained = 0, rounds = wide ? 64 : 4096;
  for (size_t round = 0; round < rounds; ++round) {
    RillDiagnostic error = {};
    uint64_t start = nanoseconds();
    values[2] = rill_library_json(&heap, false, values[0], values[1], &error);
    total += nanoseconds() - start;
    CHECK(!error.kind && values[2].kind == RILL_V_LIST);
    if (wide)
      CHECK(rill_runtime_count(values[2]) == 200000 &&
            rill_runtime_at(values[2], 199999).kind == RILL_V_INT &&
            rill_runtime_at(values[2], 199999).as.integer == 0);
    else {
      RillValue at = values[2];
      for (size_t i = 0; i < 128; ++i) {
        CHECK(at.kind == RILL_V_LIST && rill_runtime_count(at) == 1);
        at = rill_runtime_at(at, 0);
      }
      CHECK(at.kind == RILL_V_INT && at.as.integer == 0);
    }
    rill_runtime_collect(&heap);
    retained = heap.bytes;
    values[2] = (RillValue){};
    rill_runtime_collect(&heap);
  }
  CHECK(printf("{\"decode_ns\":%" PRIu64 ",\"retained_bytes\":%zu}\n", total,
               retained) > 0);
  rill_runtime_unroot(&heap, &root);
  rill_runtime_heap_clear(&heap);
}
