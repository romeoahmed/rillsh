#include "runtime/runtime.h"
#include "diagnostic.h"
#include "source.h"
#include "syntax/syntax.h"
#include "test.h"
#include <stddef.h>
#include <stdint.h>
#include <string.h>

static void heap_contracts() {
  RillHeap heap = {.stress = true};
  RillValue roots[2] = {};
  RillRoot frame;
  rill_runtime_root(&heap, &frame, roots, 2);
  roots[0] = rill_runtime_object(&heap, RILL_V_STRING, nullptr, 0, "persistent",
                                 10, 0);
  CHECK(roots[0].kind == RILL_V_STRING);
  RillObject *address = roots[0].as.object;
  CHECK(
      rill_runtime_object(&heap, RILL_V_LIST, nullptr, SIZE_MAX, nullptr, 0, 0)
          .kind == RILL_V_UNIT);
  CHECK(rill_runtime_object(&heap, RILL_V_STRING, nullptr, 0, nullptr, SIZE_MAX,
                            0)
            .kind == RILL_V_UNIT);
  for (size_t i = 0; i < 1000; ++i) {
    roots[1] = rill_runtime_object(&heap, RILL_V_LIST, roots, 1, nullptr, 0, 0);
    CHECK(roots[1].kind == RILL_V_LIST);
    CHECK(roots[1].as.object->values[0].as.object == address);
    CHECK(!strcmp(address->bytes.data, "persistent"));
  }
  rill_runtime_collect(&heap);
  size_t plateau = heap.bytes;
  for (size_t i = 0; i < 1000; ++i)
    roots[1] = rill_runtime_object(&heap, RILL_V_LIST, roots, 1, nullptr, 0, 0);
  rill_runtime_collect(&heap);
  CHECK(heap.bytes == plateau);
  // A deep graph must be marked iteratively, including shared edges and cycles.
  heap.stress = false;
  for (size_t i = 0; i < 20000; ++i) {
    roots[1] = rill_runtime_object(&heap, RILL_V_LIST, roots, 2, nullptr, 0, 0);
    CHECK(roots[1].kind == RILL_V_LIST);
  }
  rill_runtime_collect(&heap);
  CHECK(roots[0].as.object == address);
  RillValue cursor = roots[1];
  for (size_t i = 0; i < 20000; ++i) {
    CHECK(cursor.kind == RILL_V_LIST && cursor.as.object->count == 2);
    CHECK(cursor.as.object->values[0].as.object == address);
    cursor = cursor.as.object->values[1];
  }
  CHECK(cursor.as.object->count == 1);
  roots[1].as.object->values[1] = roots[1];
  rill_runtime_collect(&heap);
  CHECK(heap.bytes == plateau + sizeof(RillValue));
  rill_runtime_unroot(&heap, &frame);
  rill_runtime_collect(&heap);
  CHECK(heap.bytes == 0);
  rill_runtime_heap_clear(&heap);
}

static RillEvalEvent entry(RillEval *eval, const char *text) {
  RillSource source;
  CHECK(rill_source_init(&source, "entry", text, strlen(text)) == RILL_OK);
  RillSyntax syntax = rill_syntax_parse(&source);
  CHECK(syntax.state == RILL_COMPLETE);
  rill_runtime_begin(eval, &syntax);
  RillEvalEvent event;
  size_t steps = 0;
  do {
    CHECK(++steps < 100);
    event = rill_runtime_step(eval);
  } while (event.state == RILL_EVAL_YIELD);
  CHECK(event.state == RILL_EVAL_DONE || event.state == RILL_EVAL_ERROR);
  if (event.state == RILL_EVAL_ERROR)
    rill_runtime_abort(eval);
  rill_syntax_clear(&syntax);
  rill_source_clear(&source);
  return event;
}

static void entries() {
  RillEval *eval = rill_runtime_new(nullptr, 0);
  CHECK(eval);
  rill_runtime_heap(eval)->stress = true;
  CHECK(entry(eval, "let saved = 'original'").state == RILL_EVAL_DONE);
  CHECK(entry(eval, "let saved = 'discarded'; missing").state ==
        RILL_EVAL_ERROR);
  RillEvalEvent result = entry(eval, "saved");
  CHECK(result.value.kind == RILL_V_STRING);
  CHECK(!strcmp(result.value.as.object->bytes.data, "original"));
  const char *invalid[] = {"[1][-1]", "[1][1]",  "[1]['0']",
                           "1(2)",    "1.field", "let x = 1; let x = 2"};
  for (size_t i = 0; i < sizeof(invalid) / sizeof(*invalid); ++i) {
    result = entry(eval, invalid[i]);
    CHECK(result.state == RILL_EVAL_ERROR &&
          result.diagnostic.kind == RILL_TYPE);
  }
  result = entry(eval, "[1, [2, 'kept']][1][1]");
  CHECK(result.value.kind == RILL_V_STRING);
  CHECK(!strcmp(result.value.as.object->bytes.data, "kept"));
  for (size_t i = 0; i < 1000; ++i)
    CHECK(entry(eval, "let saved = 'replacement'").state == RILL_EVAL_DONE);
  rill_runtime_collect(rill_runtime_heap(eval));
  size_t retained = rill_runtime_heap(eval)->bytes;
  CHECK(entry(eval, "let saved = 'replacement'").state == RILL_EVAL_DONE);
  rill_runtime_collect(rill_runtime_heap(eval));
  CHECK(rill_runtime_heap(eval)->bytes == retained);
  rill_runtime_free(eval);
}

static void unary_protocol() {
  const RillNative native = {"f", 42};
  RillEval *eval = rill_runtime_new(&native, 1);
  CHECK(eval);
  rill_runtime_heap(eval)->stress = true;
  RillSource source;
  const char *text = "let g = [f][0]; g(1, 2)";
  CHECK(rill_source_init(&source, "unary", text, strlen(text)) == RILL_OK);
  RillSyntax syntax = rill_syntax_parse(&source);
  CHECK(syntax.state == RILL_COMPLETE);
  rill_runtime_begin(eval, &syntax);
  for (int64_t argument = 1; argument <= 2; ++argument) {
    RillEvalEvent event = rill_runtime_step(eval);
    CHECK(event.state == RILL_EVAL_NATIVE && event.native == 42);
    CHECK(event.value.kind == RILL_V_INT && event.value.as.integer == argument);
    CHECK(rill_runtime_step(eval).state == RILL_EVAL_YIELD);
    rill_runtime_collect(rill_runtime_heap(eval));
    RillValue value =
        argument == 1 ? (RillValue){.kind = RILL_V_FUNCTION, .as.integer = 42}
                      : event.value;
    rill_runtime_resume(eval, value, (RillDiagnostic){});
  }
  RillEvalEvent result = rill_runtime_step(eval);
  CHECK(result.state == RILL_EVAL_DONE && result.value.as.integer == 2);
  rill_syntax_clear(&syntax);
  rill_source_clear(&source);
  text = "f(f('payload'))";
  CHECK(rill_source_init(&source, "rooted", text, strlen(text)) == RILL_OK);
  syntax = rill_syntax_parse(&source);
  CHECK(syntax.state == RILL_COMPLETE);
  rill_runtime_begin(eval, &syntax);
  for (size_t i = 0; i < 2; ++i) {
    result = rill_runtime_step(eval);
    CHECK(result.state == RILL_EVAL_NATIVE &&
          result.value.kind == RILL_V_STRING);
    rill_runtime_collect(rill_runtime_heap(eval));
    CHECK(!strcmp(result.value.as.object->bytes.data, "payload"));
    rill_runtime_resume(eval, result.value, (RillDiagnostic){});
  }
  CHECK(rill_runtime_step(eval).state == RILL_EVAL_DONE);
  rill_syntax_clear(&syntax);
  rill_source_clear(&source);
  // Aborting a suspended effect must discard pending bindings and frames.
  text = "let g = 9; f(1)";
  CHECK(rill_source_init(&source, "abort", text, strlen(text)) == RILL_OK);
  syntax = rill_syntax_parse(&source);
  CHECK(syntax.state == RILL_COMPLETE);
  rill_runtime_begin(eval, &syntax);
  CHECK(rill_runtime_step(eval).state == RILL_EVAL_NATIVE);
  rill_runtime_abort(eval);
  CHECK(entry(eval, "g").value.kind == RILL_V_FUNCTION);
  rill_runtime_begin(eval, &syntax);
  CHECK(rill_runtime_step(eval).state == RILL_EVAL_NATIVE);
  rill_runtime_resume(eval, (RillValue){}, (RillDiagnostic){.kind = RILL_IO});
  CHECK(rill_runtime_step(eval).diagnostic.kind == RILL_IO);
  rill_runtime_abort(eval);
  CHECK(entry(eval, "g").value.kind == RILL_V_FUNCTION);
  rill_syntax_clear(&syntax);
  rill_source_clear(&source);
  rill_runtime_free(eval);
}

int main() {
  heap_contracts();
  entries();
  unary_protocol();
}
