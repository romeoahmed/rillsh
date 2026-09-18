/**
 * @file
 * @brief Fuzz parsing, code preparation, and ownership transfer.
 *
 * Release the original source before preparing complete syntax, then abort
 * and collect. Input is never evaluated, so generated commands cannot execute.
 */
#include "syntax/syntax.h"
#include "diagnostic.h"
#include "runtime/runtime.h"
#include "source.h"
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>

int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size);
int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size) {
  [[gnu::cleanup(rill_source_clear)]] RillSource source = {};
  RillError error = rill_source_init(&source, "fuzz", (const char *)data, size);
  if (error != RILL_OK)
    return 0;
  [[gnu::cleanup(rill_syntax_clear)]] RillSyntax syntax =
      rill_syntax_parse(&source);
  if (syntax.diagnostic.offset > size ||
      (syntax.state == RILL_COMPLETE) != (syntax.diagnostic.kind == RILL_OK))
    abort();
  // Syntax and prepared code must own their source after this boundary.
  rill_source_clear(&source);
  RillEval *eval = rill_runtime_new(nullptr, 0);
  if (!eval)
    return 0;
  rill_runtime_heap(eval)->stress = size <= 1024;
  rill_runtime_begin(eval, &syntax);
  rill_runtime_collect(rill_runtime_heap(eval));
  // Exercise preparation and disposal, never arbitrary loops or host effects.
  rill_runtime_abort(eval);
  rill_runtime_free(eval);
  return 0;
}
