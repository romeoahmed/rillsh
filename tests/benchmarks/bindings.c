/**
 * @file
 * @brief Measure full binding-snapshot publication and lookup.
 *
 * Populate through the C adapter to isolate snapshot work from parsing and
 * duplicate-name validation; verify retained values after each publication.
 */
#include "../unit/test.h"
#include "diagnostic.h"
#include "runtime/runtime.h"
#include "source.h"
#include "syntax/syntax.h"
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

int main() {
  RillEval *eval = rill_runtime_new(nullptr, 0);
  CHECK(eval);
  RillSource source = {};
  const char text[] = "let committed=1; committed";
  CHECK(rill_source_init(&source, "bindings-benchmark", text,
                         sizeof(text) - 1) == RILL_OK);
  size_t retained = 0;
  for (size_t round = 0; round < 8; ++round) {
    // Populate through the adapter API to isolate snapshot construction and
    // lookup from parser and same-scope duplicate-name validation costs.
    for (size_t i = 0; i < 4096; ++i) {
      char name[32];
      int size = snprintf(name, sizeof(name), "binding%zu", i);
      CHECK(size > 0 && (size_t)size < sizeof(name));
      CHECK(rill_runtime_define(
          eval, name,
          (RillValue){.kind = RILL_V_INT, .as.integer = (int64_t)(i + round)}));
    }
    RillSyntax syntax = rill_syntax_parse(&source);
    CHECK(syntax.state == RILL_COMPLETE);
    rill_runtime_begin(eval, &syntax);
    rill_syntax_clear(&syntax);
    RillEvalEvent event = {};
    do {
      event = rill_runtime_step(eval);
    } while (event.state == RILL_EVAL_YIELD);
    CHECK(event.state == RILL_EVAL_DONE && event.value.kind == RILL_V_INT &&
          event.value.as.integer == 1);
    for (size_t i = 0; i < 4096; ++i) {
      char name[32];
      int size = snprintf(name, sizeof(name), "binding%zu", i);
      CHECK(size > 0 && (size_t)size < sizeof(name));
      RillValue value = {};
      CHECK(rill_runtime_lookup(eval, name, &value));
      CHECK(value.kind == RILL_V_INT &&
            value.as.integer == (int64_t)(i + round));
    }
    rill_runtime_collect(rill_runtime_heap(eval));
    size_t bytes = rill_runtime_heap(eval)->bytes;
    retained = bytes;
  }
  CHECK(printf("{\"retained_bytes\":%zu}\n", retained) > 0);
  rill_source_clear(&source);
  rill_runtime_free(eval);
}
