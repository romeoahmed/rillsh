#include "../unit/test.h"
#include "diagnostic.h"
#include "runtime/runtime.h"
#include "source.h"
#include "syntax/syntax.h"
#include <inttypes.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <time.h>

static uint64_t nanoseconds() {
  struct timespec now = {};
  CHECK(clock_gettime(CLOCK_MONOTONIC, &now) == 0);
  return (uint64_t)now.tv_sec * UINT64_C(1000000000) + (uint64_t)now.tv_nsec;
}
int main() {
  RillEval *eval = rill_runtime_new(nullptr, 0);
  CHECK(eval);
  const char text[] =
      "fn build(n, output)=>if n==0 then output else do {"
      "let a=n; let b=n+1; let c=n+2; let d=n+3; let e=n+4; let f=n+5;"
      "let g=n+6; let h=n+7;"
      "build(n-1,[fn()=>a+b+c+d+e+f+g+h,output])}; build(10000,[])";
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
  RillValue cursor = event.value;
  for (size_t i = 0; i < 10000; ++i) {
    CHECK(cursor.kind == RILL_V_LIST && cursor.as.object->count == 2);
    CHECK(cursor.as.object->values[0].kind == RILL_V_CLOSURE);
    cursor = cursor.as.object->values[1];
  }
  CHECK(cursor.kind == RILL_V_LIST && cursor.as.object->count == 0);
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
    CHECK(heap->bytes == retained);
  }
  CHECK(printf("{\"retained_bytes\":%zu,\"objects\":%zu,\"collections\":50,"
               "\"total_ns\":%" PRIu64 ",\"max_ns\":%" PRIu64 "}\n",
               retained, objects, total, maximum) > 0);
  rill_runtime_free(eval);
}
