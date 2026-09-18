/**
 * @file
 * @brief Measure small publications into a large committed environment.
 *
 * Update one binding per entry and verify the resulting snapshot. Retained
 * bytes are observations; elapsed time comes from Meson.
 */
#include "../support/check.h"
#include "diagnostic.h"
#include "runtime/runtime.h"
#include "source.h"
#include "syntax/syntax.h"
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>

int main() {
  RillEval *eval = rill_runtime_new(nullptr, 0);
  CHECK(eval);
  for (size_t i = 0; i < 10000; ++i) {
    char name[32];
    int size = snprintf(name, sizeof(name), "key%zu", i);
    CHECK(size > 0 && (size_t)size < sizeof(name));
    CHECK(rill_runtime_define(
        eval, name, (RillValue){.kind = RILL_V_INT, .as.integer = (int64_t)i}));
  }
  [[gnu::cleanup(rill_source_clear)]] RillSource source = {};
  const char code[] = "let key0 = key0 + 1; key9999";
  CHECK(rill_source_init(&source, "snapshots-benchmark", code,
                         sizeof(code) - 1) == RILL_OK);
  size_t retained = 0;
  for (size_t round = 0; round < 512; ++round) {
    RillSyntax syntax = rill_syntax_parse(&source);
    CHECK(syntax.state == RILL_COMPLETE);
    rill_runtime_begin(eval, &syntax);
    RillEvalEvent event = {};
    do {
      event = rill_runtime_step(eval);
    } while (event.state == RILL_EVAL_YIELD);
    CHECK(event.state == RILL_EVAL_DONE && event.value.kind == RILL_V_INT &&
          event.value.as.integer == 9999);
    rill_runtime_collect(rill_runtime_heap(eval));
    retained = rill_runtime_heap(eval)->bytes;
  }
  RillValue updated = {};
  CHECK(rill_runtime_lookup(eval, "key0", &updated));
  CHECK(updated.kind == RILL_V_INT && updated.as.integer == 512);
  CHECK(printf("{\"retained_bytes\":%zu}\n", retained) > 0);
  rill_runtime_free(eval);
}
