/**
 * @file
 * @brief Measure repeated parsing and execution of wide statement sequences.
 *
 * Use the production source, parser, and evaluator with no external effects.
 * Meson reports whole-process duration, including ownership transfer and
 * cleanup.
 */
#include "syntax/syntax.h"
#include "../unit/test.h"
#include "diagnostic.h"
#include "runtime/runtime.h"
#include "source.h"
#include "text/text.h"
#include <stddef.h>

int main() {
  RillBuffer text = {};
  for (size_t i = 0; i < 16384; ++i)
    CHECK(rill_text_append(&text, "();\n", 4));
  RillSource source = {};
  CHECK(rill_source_init(&source, "benchmark", text.data, text.size) ==
        RILL_OK);
  rill_text_clear(&text);
  RillEval *eval = rill_runtime_new(nullptr, 0);
  CHECK(eval);
  for (size_t i = 0; i < 64; ++i) {
    RillSyntax syntax = rill_syntax_parse(&source);
    CHECK(syntax.state == RILL_COMPLETE);
    rill_runtime_begin(eval, &syntax);
    rill_syntax_clear(&syntax);
    RillEvalEvent event = {};
    do {
      event = rill_runtime_step(eval);
    } while (event.state == RILL_EVAL_YIELD);
    CHECK(event.state == RILL_EVAL_DONE && event.value.kind == RILL_V_UNIT);
  }
  rill_runtime_free(eval);
  rill_source_clear(&source);
}
