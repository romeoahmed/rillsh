/**
 * @file
 * @brief Measure code with many independently prepared function layouts.
 *
 * Repeated entries retain a function List and verify an invocation; report
 * heap storage after collection. Meson measures whole-process duration.
 */
#include "../support/check.h"
#include "diagnostic.h"
#include "runtime/runtime.h"
#include "source.h"
#include "syntax/syntax.h"
#include "text/text.h"
#include <stddef.h>
#include <stdio.h>

int main() {
  [[gnu::cleanup(rill_text_clear)]] RillBuffer text = {};
  CHECK(rill_text_append(&text, "let functions = [", 17));
  for (size_t i = 0; i < 2048; ++i)
    CHECK(rill_text_format(&text, "%s{ x => x + %zu }", i ? ", " : "", i));
  const char result[] = "]; functions[2047] 1";
  CHECK(rill_text_append(&text, result, sizeof(result) - 1));
  [[gnu::cleanup(rill_source_clear)]] RillSource source = {};
  CHECK(rill_source_init(&source, "functions-benchmark", text.data,
                         text.size) == RILL_OK);
  RillEval *eval = rill_runtime_new(nullptr, 0);
  CHECK(eval);
  size_t retained = 0;
  for (size_t round = 0; round < 64; ++round) {
    RillSyntax syntax = rill_syntax_parse(&source);
    CHECK(syntax.state == RILL_COMPLETE);
    rill_runtime_begin(eval, &syntax);
    RillEvalEvent event = {};
    do {
      event = rill_runtime_step(eval);
    } while (event.state == RILL_EVAL_YIELD);
    CHECK(event.state == RILL_EVAL_DONE && event.value.kind == RILL_V_INT &&
          event.value.as.integer == 2048);
    rill_runtime_collect(rill_runtime_heap(eval));
    retained = rill_runtime_heap(eval)->bytes;
  }
  CHECK(printf("{\"retained_bytes\":%zu}\n", retained) > 0);
  rill_runtime_free(eval);
}
