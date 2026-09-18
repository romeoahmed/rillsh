/**
 * @file
 * @brief Fuzz line decoding against a byte-oriented reference scan.
 *
 * A fixed adapter expression processes input as data, never source. Compare
 * line content, CRLF handling, final fragments, and errors; each invocation
 * cleans its stream scope and heap without launching processes.
 */
#include "diagnostic.h"
#include "library/library.h"
#include "library/stream.h"
#include "runtime/runtime.h"
#include "source.h"
#include "syntax/syntax.h"
#include "text/text.h"
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size);
static RillEvalEvent evaluate(RillLibrary *library, const char *text) {
  [[gnu::cleanup(rill_source_clear)]] RillSource source = {};
  if (rill_source_init(&source, "fuzz", text, strlen(text)) != RILL_OK)
    abort();
  [[gnu::cleanup(rill_syntax_clear)]] RillSyntax syntax =
      rill_syntax_parse(&source);
  if (syntax.state != RILL_COMPLETE)
    abort();
  rill_runtime_begin(library->eval, &syntax);
  RillNativePending pending = {};
  for (;;) {
    if (!rill_stream_progress(library))
      continue;
    RillEvalEvent event = rill_runtime_step(library->eval);
    if (event.state == RILL_EVAL_YIELD)
      continue;
    if (event.state == RILL_EVAL_CLEANUP) {
      rill_stream_unwind(library, event.native, event.value);
      continue;
    }
    if (event.state == RILL_EVAL_CALLBACK) {
      rill_stream_callback(library, event.value);
      continue;
    }
    if (event.state != RILL_EVAL_NATIVE)
      return event;
    (void)rill_library_call(library, &pending, event);
  }
}
int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size) {
  if (size > 131072)
    return -1;
  size_t count = {};
  const RillNative *natives = rill_library_natives(&count);
  RillEval *eval = rill_runtime_new(natives, count);
  if (!eval)
    return 0;
  RillLibrary library = {.eval = eval};
  RillHeap *heap = rill_runtime_heap(eval);
  heap->stress = size <= 1024;
  RillValue input = {};
  RillRoot root = {};
  rill_runtime_root(heap, &root, &input, 1);
  input = rill_runtime_object(heap, RILL_V_BYTES, nullptr, 0,
                              (const char *)data, size, 0);
  if (input.kind != RILL_V_BYTES)
    abort();
  if (!rill_runtime_define(eval, "input", input))
    abort();
  RillEvalEvent result = evaluate(
      &library, "__stream(['collect',{max_items:131072,max_bytes:67108864},"
                "__stream(['lines',{max_line_bytes:131072},"
                "__stream(['chunks',input])])])");
  if (result.state == RILL_EVAL_DONE) {
    if (result.value.kind != RILL_V_LIST ||
        !rill_text_valid((const char *)data, size))
      abort();
    size_t offset = 0, index = 0;
    while (offset < size) {
      const uint8_t *lf = memchr(data + offset, '\n', size - offset);
      size_t end = lf ? (size_t)(lf - data) : size;
      size_t length = end - offset;
      if (lf && length && data[end - 1] == '\r')
        --length;
      if (index >= rill_runtime_count(result.value))
        abort();
      RillValue line = rill_runtime_at(result.value, index++);
      if (line.kind != RILL_V_STRING || line.as.object->bytes.size != length ||
          (length &&
           memcmp(line.as.object->bytes.data, data + offset, length) != 0))
        abort();
      offset = end + (lf ? 1 : 0);
    }
    if (index != rill_runtime_count(result.value))
      abort();
  } else if (result.state != RILL_EVAL_ERROR ||
             (result.diagnostic.kind != RILL_MEMORY &&
              (result.diagnostic.kind != RILL_DECODE ||
               rill_text_valid((const char *)data, size))))
    abort();
  rill_stream_cancel(&library);
  rill_stream_clear(&library);
  rill_runtime_unroot(heap, &root);
  rill_runtime_free(eval);
  return 0;
}
