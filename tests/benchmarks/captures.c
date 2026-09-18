/**
 * @file
 * @brief Measure retained flat captures and collection over live closures.
 *
 * Keep closures reachable after preparation, report their retained graph, and
 * time explicit collections separately from construction.
 */
#include "diagnostic.h"
#include "runtime/runtime.h"
#include "source.h"
#include "syntax/syntax.h"
#include "timing.h"
#include <inttypes.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>

int main() {
  RillEval *eval = rill_runtime_new(nullptr, 0);
  CHECK(eval);
  const char text[] =
      "fn build n output =\n"
      "  if n == 0 then output else do {\n"
      "    let a = n; let b = n + 1; let c = n + 2; let d = n + 3\n"
      "    let e = n + 4; let f = n + 5; let g = n + 6; let h = n + 7\n"
      "    build (n - 1) [{ () => a + b + c + d + e + f + g + h }, output]\n"
      "  }\n"
      "build 10000 []";
  RillSource source = {};
  CHECK(rill_source_init(&source, "captures-benchmark", text,
                         sizeof(text) - 1) == RILL_OK);
  RillSyntax syntax = rill_syntax_parse(&source);
  CHECK(syntax.state == RILL_COMPLETE);
  rill_runtime_begin(eval, &syntax);
  rill_syntax_clear(&syntax);
  rill_source_clear(&source);
  RillEvalEvent event = {};
  do {
    event = rill_runtime_step(eval);
  } while (event.state == RILL_EVAL_YIELD);
  CHECK(event.state == RILL_EVAL_DONE);
  RillHeap *heap = rill_runtime_heap(eval);
  rill_runtime_collect(heap);
  size_t retained = heap->bytes, objects = 0;
  for (RillObject *o = heap->objects; o; o = o->next)
    ++objects;
  uint64_t total = 0, maximum = 0;
  for (size_t i = 0; i < 50; ++i) {
    uint64_t start = nanoseconds();
    rill_runtime_collect(heap);
    uint64_t elapsed = nanoseconds() - start;
    total += elapsed;
    if (elapsed > maximum)
      maximum = elapsed;
  }
  CHECK(printf("{\"retained_bytes\":%zu,\"objects\":%zu,\"collections\":50,"
               "\"total_ns\":%" PRIu64 ",\"max_ns\":%" PRIu64 "}\n",
               retained, objects, total, maximum) > 0);
  RillValue cursor = event.value;
  for (size_t i = 0; i < 10000; ++i) {
    CHECK(cursor.kind == RILL_V_LIST && rill_runtime_count(cursor) == 2);
    CHECK(rill_runtime_at(cursor, 0).kind == RILL_V_CLOSURE);
    cursor = rill_runtime_at(cursor, 1);
  }
  CHECK(cursor.kind == RILL_V_LIST && rill_runtime_count(cursor) == 0);
  rill_runtime_free(eval);
}
