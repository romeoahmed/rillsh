/**
 * @file
 * @brief Measure parsing separately from preparation and execution.
 *
 * Wide statements, Records, and long literals exercise distinct parser paths.
 * Validate the evaluated result outside the measured parse interval.
 */
#include "syntax/syntax.h"
#include "diagnostic.h"
#include "runtime/runtime.h"
#include "source.h"
#include "text/text.h"
#include "timing.h"
#include <inttypes.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

int main(int argc, char **argv) {
  CHECK(argc <= 2);
  bool records = argc == 2 && !strcmp(argv[1], "records");
  bool strings = argc == 2 && !strcmp(argv[1], "strings");
  CHECK(argc == 1 || records || strings);
  RillBuffer text = {};
  if (records) {
    CHECK(rill_text_append(&text, "{", 1));
    for (size_t i = 0; i < 4096; ++i)
      CHECK(rill_text_format(&text, "%skey%zu: %zu", i ? ", " : "", i, i));
    CHECK(rill_text_append(&text, "}", 1));
  } else if (strings) {
    CHECK(rill_text_append(&text, "\"", 1));
    for (size_t i = 0; i < 16384; ++i)
      CHECK(rill_text_append(&text, "abcdefghijklmnop", 16));
    CHECK(rill_text_append(&text, "\"", 1));
  } else
    for (size_t i = 0; i < 16384; ++i)
      CHECK(rill_text_append(&text, "();\n", 4));
  RillSource source = {};
  CHECK(rill_source_init(&source, "benchmark", text.data, text.size) ==
        RILL_OK);
  rill_text_clear(&text);
  RillEval *eval = rill_runtime_new(nullptr, 0);
  CHECK(eval);
  constexpr size_t iterations = 64;
  uint64_t elapsed = 0;
  for (size_t i = 0; i < iterations; ++i) {
    uint64_t start = nanoseconds();
    RillSyntax syntax = rill_syntax_parse(&source);
    elapsed += nanoseconds() - start;
    CHECK(syntax.state == RILL_COMPLETE);
    rill_runtime_begin(eval, &syntax);
    rill_syntax_clear(&syntax);
    RillEvalEvent event = {};
    do {
      event = rill_runtime_step(eval);
    } while (event.state == RILL_EVAL_YIELD);
    CHECK(event.state == RILL_EVAL_DONE);
    if (records) {
      CHECK(event.value.kind == RILL_V_RECORD);
      for (size_t j = 0; j < 4096; ++j) {
        char key[32];
        int length = snprintf(key, sizeof(key), "key%zu", j);
        CHECK(length > 0 && (size_t)length < sizeof(key));
        RillValue value = {};
        CHECK(rill_runtime_field(event.value, (RillBytes){key, (size_t)length},
                                 &value));
        CHECK(value.kind == RILL_V_INT && value.as.integer == (int64_t)j);
      }
    } else if (strings) {
      CHECK(event.value.kind == RILL_V_STRING);
      CHECK(event.value.as.object->bytes.size == source.bytes.size - 2);
      CHECK(!memcmp(event.value.as.object->bytes.data, source.bytes.data + 1,
                    source.bytes.size - 2));
    } else
      CHECK(event.value.kind == RILL_V_UNIT);
  }
  CHECK(printf("{\"iterations\":%zu,\"input_bytes\":%zu,\"parse_ns\":%" PRIu64
               "}\n",
               iterations, source.bytes.size, elapsed) > 0);
  rill_runtime_free(eval);
  rill_source_clear(&source);
}
