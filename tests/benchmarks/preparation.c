/**
 * @file
 * @brief Measure nested capture analysis and repeated or unique constants.
 *
 * Time entry preparation separately from parsing and execution. Invoke and
 * check the retained function result after measurement to validate its body.
 */
#include "diagnostic.h"
#include "runtime/runtime.h"
#include "source.h"
#include "syntax/syntax.h"
#include "text/text.h"
#include "timing.h"
#include <inttypes.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

int main(int argc, char **argv) {
  CHECK(argc == 2);
  bool nested = !strcmp(argv[1], "nested");
  bool unique = !strcmp(argv[1], "unique");
  CHECK(nested || unique || !strcmp(argv[1], "literals"));
  [[gnu::cleanup(rill_text_clear)]] RillBuffer text = {};
  const char prefix[] = "let seed = 7; let saved = ";
  CHECK(rill_text_append(&text, prefix, sizeof(prefix) - 1));
  for (size_t i = 0; i < (nested ? 96u : 1u); ++i)
    CHECK(rill_text_append(&text, "{ () => ", 8));
  CHECK(rill_text_append(&text, "[", 1));
  for (size_t i = 0; i < 4096; ++i) {
    if (unique)
      CHECK(rill_text_format(&text, "%s'payload %zu'", i ? ", " : "", i));
    else
      CHECK(rill_text_format(&text, "%s%s", i ? ", " : "",
                             nested ? "seed" : "'repeated literal payload'"));
  }
  CHECK(rill_text_append(&text, "]", 1));
  for (size_t i = 0; i < (nested ? 96u : 1u); ++i)
    CHECK(rill_text_append(&text, " }", 2));
  CHECK(rill_text_append(&text, "; saved", 7));
  [[gnu::cleanup(rill_source_clear)]] RillSource source = {};
  CHECK(rill_source_init(&source, "preparation-benchmark", text.data,
                         text.size) == RILL_OK);
  RillEval *eval = rill_runtime_new(nullptr, 0);
  CHECK(eval);
  uint64_t elapsed = 0;
  size_t retained = 0;
  constexpr size_t iterations = 64;
  for (size_t round = 0; round < iterations; ++round) {
    RillSyntax syntax = rill_syntax_parse(&source);
    CHECK(syntax.state == RILL_COMPLETE);
    uint64_t start = nanoseconds();
    rill_runtime_begin(eval, &syntax);
    elapsed += nanoseconds() - start;
    RillEvalEvent event = {};
    do {
      event = rill_runtime_step(eval);
    } while (event.state == RILL_EVAL_YIELD);
    CHECK(event.state == RILL_EVAL_DONE && event.value.kind == RILL_V_CLOSURE);
    rill_runtime_collect(rill_runtime_heap(eval));
    retained = rill_runtime_heap(eval)->bytes;
  }
  CHECK(printf("{\"iterations\":%zu,\"input_bytes\":%zu,\"prepare_ns\":%" PRIu64
               ",\"retained_bytes\":%zu}\n",
               iterations, source.bytes.size, elapsed, retained) > 0);
  // Verify the prepared body too, outside the reported preparation interval.
  rill_text_truncate(&text, 0);
  CHECK(rill_text_append(&text, "saved", 5));
  for (size_t i = 0; i < (nested ? 96u : 1u); ++i)
    CHECK(rill_text_append(&text, " ()", 3));
  rill_source_clear(&source);
  CHECK(rill_source_init(&source, "preparation-result", text.data, text.size) ==
        RILL_OK);
  RillSyntax syntax = rill_syntax_parse(&source);
  CHECK(syntax.state == RILL_COMPLETE);
  rill_runtime_begin(eval, &syntax);
  RillEvalEvent event = {};
  do {
    event = rill_runtime_step(eval);
  } while (event.state == RILL_EVAL_YIELD);
  CHECK(event.state == RILL_EVAL_DONE && event.value.kind == RILL_V_LIST &&
        rill_runtime_count(event.value) == 4096);
  for (size_t i = 0; i < 4096; ++i) {
    RillValue value = rill_runtime_at(event.value, i);
    if (nested)
      CHECK(value.kind == RILL_V_INT && value.as.integer == 7);
    else {
      rill_text_truncate(&text, 0);
      if (unique)
        CHECK(rill_text_format(&text, "payload %zu", i));
      else
        CHECK(rill_text_append(&text, "repeated literal payload", 24));
      CHECK(value.kind == RILL_V_STRING &&
            value.as.object->bytes.size == text.size &&
            !memcmp(value.as.object->bytes.data, text.data, text.size));
    }
  }
  rill_runtime_free(eval);
}
