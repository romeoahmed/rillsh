/**
 * @file
 * @brief Convert strict JSON and runtime values without recursive C walks.
 *
 * yyjson owns parsing and scalar formatting. Explicit stacks convert one child
 * at a time; rooted builders expose only initialized slots. Decoding copies
 * input before in-situ parsing and copies results before freeing the document.
 * Encoding can return a Value that backs a borrowed diagnostic path.
 */
#include "json.h"
#include "diagnostic.h"
#include "runtime/runtime.h"
#include "text/text.h"
#include "value.h"
#include <math.h>
#include <stdckdint.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <yyjson.h>

RillValue rill_library_json_number(bool real, RillValue input,
                                   RillDiagnostic *error) {
  if (input.kind != RILL_V_STRING) {
    *error = (RillDiagnostic){.kind = RILL_TYPE,
                              .message = "numeric parsing requires String"};
    return (RillValue){};
  }
  // Runtime Strings include a trailing NUL; the length check rejects embedded
  // NUL and any unconsumed suffix without copying the input.
  RillBytes data = input.as.object->bytes;
  yyjson_val number = {};
  yyjson_read_err problem = {};
  const char *end =
      yyjson_read_number(data.data, &number, 0, nullptr, &problem);
  // On conversion failure, validate the token without converting its magnitude.
  // This distinguishes overflow from malformed input, including trailing junk.
  if (!end && problem.code != YYJSON_READ_ERROR_MEMORY_ALLOCATION)
    end = yyjson_read_number(data.data, &number, YYJSON_READ_NUMBER_AS_RAW,
                             nullptr, &problem);
  if (!end && problem.code == YYJSON_READ_ERROR_MEMORY_ALLOCATION)
    *error = (RillDiagnostic){.kind = RILL_MEMORY,
                              .message = "numeric parsing allocation failed"};
  else if (!end || end != data.data + data.size)
    *error = (RillDiagnostic){.kind = RILL_DECODE,
                              .message = "expected one JSON decimal number"};
  else if (yyjson_is_raw(&number))
    *error = (RillDiagnostic){.kind = RILL_ARITHMETIC,
                              .message = "number outside Rill numeric range"};
  else if (real) {
    double value = yyjson_get_num(&number);
    if (isfinite(value))
      return (RillValue){.kind = RILL_V_FLOAT, .as.real = value};
    *error = (RillDiagnostic){.kind = RILL_ARITHMETIC,
                              .message = "number outside Float range"};
  } else if (yyjson_is_int(&number) && (!yyjson_is_uint(&number) ||
                                        yyjson_get_uint(&number) <= INT64_MAX))
    return (RillValue){.kind = RILL_V_INT,
                       .as.integer = yyjson_get_sint(&number)};
  else
    *error =
        (RillDiagnostic){.kind = RILL_ARITHMETIC,
                         .message = "expected an integer within Int range"};
  return (RillValue){};
}

typedef struct {
  yyjson_val *input;
  RillValue *output;
  union {
    yyjson_arr_iter array;
    yyjson_obj_iter object;
  } iterator;
} Decode;
static void *reserve(void *old, size_t *capacity, size_t count, size_t width) {
  if (count <= *capacity)
    return old;
  size_t n = *capacity ? *capacity : 32, bytes = {};
  while (n < count)
    if (ckd_mul(&n, n, 2))
      return nullptr;
  if (ckd_mul(&bytes, n, width))
    return nullptr;
  void *next = realloc(old, bytes);
  if (!next)
    return nullptr;
  *capacity = n;
  return next;
}
static RillValue decode(RillHeap *heap, RillValue input, size_t max_depth,
                        RillDiagnostic *error) {
  RillBytes bytes = input.as.object->bytes;
  [[gnu::cleanup(rill_text_clear)]] RillBuffer copy = {};
  const char padding[YYJSON_PADDING_SIZE] = {};
  if (!rill_text_append(&copy, bytes.data, bytes.size) ||
      !rill_text_append(&copy, padding, sizeof(padding))) {
    *error = (RillDiagnostic){.kind = RILL_MEMORY,
                              .message = "JSON allocation failed"};
    return (RillValue){};
  }
  yyjson_read_err problem = {};
  yyjson_doc *doc = yyjson_read_opts(
      copy.data, bytes.size, YYJSON_READ_INSITU | YYJSON_READ_BIGNUM_AS_RAW,
      nullptr, &problem);
  if (!doc) {
    *error = (RillDiagnostic){
        .kind = problem.code == YYJSON_READ_ERROR_MEMORY_ALLOCATION
                    ? RILL_MEMORY
                    : RILL_DECODE,
        .message = "invalid JSON document",
        .offset = problem.pos};
    return (RillValue){};
  }
  RillValue result = {};
  RillRoot root = {};
  rill_runtime_root(heap, &root, &result, 1);
  size_t count = 0, capacity = 0;
  Decode *stack = reserve(nullptr, &capacity, 1, sizeof(*stack));
  if (!stack)
    goto memory;
  stack[count++] =
      (Decode){.input = yyjson_doc_get_root(doc), .output = &result};
  while (count && !error->kind) {
    Decode *task = &stack[count - 1];
    yyjson_val *v = task->input;
    if (count > max_depth) {
      *error = (RillDiagnostic){.kind = RILL_LIMIT,
                                .message = "JSON nesting limit exceeded"};
      break;
    }
    if (yyjson_is_arr(v) || yyjson_is_obj(v)) {
      bool object = yyjson_is_obj(v);
      if (task->output->kind == RILL_V_UNIT) {
        size_t slots = {};
        if (ckd_mul(&slots, yyjson_get_len(v), object ? 2 : 1))
          goto memory;
        *task->output =
            rill_runtime_object(heap, object ? RILL_V_RECORD : RILL_V_LIST,
                                nullptr, slots, nullptr, 0, 0);
        if (task->output->kind == RILL_V_UNIT)
          goto memory;
        // Trace only the initialized prefix while the rooted builder grows.
        task->output->as.object->count = 0;
        if (object)
          task->iterator.object = yyjson_obj_iter_with(v);
        else
          task->iterator.array = yyjson_arr_iter_with(v);
      }
      RillObject *out = task->output->as.object;
      yyjson_val *child = object ? yyjson_obj_iter_next(&task->iterator.object)
                                 : yyjson_arr_iter_next(&task->iterator.array);
      if (!child) {
        if (object && !rill_runtime_record_finish(*task->output))
          *error = (RillDiagnostic){.kind = RILL_DECODE,
                                    .message = "duplicate JSON object key"};
        --count;
        continue;
      }
      size_t index = out->count;
      out->count += object ? 2 : 1;
      if (object) {
        out->values[index] = rill_runtime_object(heap, RILL_V_STRING, nullptr,
                                                 0, yyjson_get_str(child),
                                                 yyjson_get_len(child), 0);
        if (out->values[index].kind == RILL_V_UNIT)
          goto memory;
        ++index;
        child = yyjson_obj_iter_get_val(child);
      }
      size_t needed = {};
      if (ckd_add(&needed, count, 1))
        goto memory;
      Decode *next = reserve(stack, &capacity, needed, sizeof(*stack));
      if (!next)
        goto memory;
      stack = next;
      stack[count++] = (Decode){.input = child, .output = &out->values[index]};
      continue;
    }
    RillValue out = {};
    if (yyjson_is_null(v))
      out.kind = RILL_V_NULL;
    else if (yyjson_is_bool(v))
      out = (RillValue){.kind = RILL_V_BOOL, .as.integer = yyjson_get_bool(v)};
    else if (yyjson_is_sint(v))
      out = (RillValue){.kind = RILL_V_INT, .as.integer = yyjson_get_sint(v)};
    else if (yyjson_is_uint(v) && yyjson_get_uint(v) <= INT64_MAX)
      out = (RillValue){.kind = RILL_V_INT,
                        .as.integer = (int64_t)yyjson_get_uint(v)};
    else if (yyjson_is_real(v) && isfinite(yyjson_get_real(v)))
      out = (RillValue){.kind = RILL_V_FLOAT, .as.real = yyjson_get_real(v)};
    else if (yyjson_is_str(v))
      out = rill_runtime_object(heap, RILL_V_STRING, nullptr, 0,
                                yyjson_get_str(v), yyjson_get_len(v), 0);
    else {
      *error =
          (RillDiagnostic){.kind = RILL_DECODE,
                           .message = "JSON number outside Rill numeric range"};
      break;
    }
    if (out.kind == RILL_V_UNIT)
      goto memory;
    *task->output = out;
    --count;
  }
  goto cleanup;
memory:
  *error = (RillDiagnostic){.kind = RILL_MEMORY,
                            .message = "JSON allocation failed"};
cleanup:
  free(stack);
  yyjson_doc_free(doc);
  rill_runtime_unroot(heap, &root);
  return error->kind ? (RillValue){} : result;
}

typedef struct {
  RillValue value;
  size_t index, depth;
} Encode;
static RillValue unsupported(RillHeap *heap, Encode *stack, size_t count,
                             RillDiagnostic *error) {
  [[gnu::cleanup(rill_text_clear)]] RillBuffer message = {};
  static const char prefix[] = "cannot encode JSON value at $";
  bool ok = rill_text_append(&message, prefix, sizeof(prefix) - 1);
  for (size_t i = 0; i + 1 < count && ok; ++i) {
    Encode parent = stack[i];
    if (parent.value.kind == RILL_V_RECORD) {
      RillBytes key =
          parent.value.as.object->values[parent.index - 2].as.object->bytes;
      yyjson_mut_val scalar = {};
      (void)yyjson_mut_set_strn(&scalar, key.data, key.size);
      size_t size = {};
      char *encoded = yyjson_mut_val_write(&scalar, 0, &size);
      ok = encoded && rill_text_append(&message, "[", 1) &&
           rill_text_append(&message, encoded, size) &&
           rill_text_append(&message, "]", 1);
      free(encoded);
    } else
      ok = rill_text_format(&message, "[%zu]", parent.index - 1);
  }
  RillValue text = {};
  if (ok)
    text = rill_runtime_object(heap, RILL_V_STRING, nullptr, 0, message.data,
                               message.size, 0);
  *error = (RillDiagnostic){
      .kind = text.kind == RILL_V_UNIT ? RILL_MEMORY : RILL_TYPE,
      .message = text.kind == RILL_V_UNIT ? "JSON diagnostic allocation failed"
                                          : text.as.object->bytes.data};
  return text;
}
static RillValue encode(RillHeap *heap, RillValue input, size_t max_bytes,
                        size_t max_depth, RillDiagnostic *error) {
  [[gnu::cleanup(rill_text_clear)]] RillBuffer bytes = {};
  size_t count = 0, capacity = 0;
  Encode *stack = reserve(nullptr, &capacity, 1, sizeof(*stack));
  if (!stack)
    goto memory;
  stack[count++] = (Encode){.value = input, .depth = 1};
  while (count && !error->kind) {
    Encode *task = &stack[count - 1];
    RillValue v = task->value;
    if (task->depth > max_depth) {
      *error = (RillDiagnostic){.kind = RILL_LIMIT,
                                .message = "JSON nesting limit exceeded"};
      break;
    }
    bool object = v.kind == RILL_V_RECORD,
         list = v.kind == RILL_V_LIST || v.kind == RILL_V_SLICE;
    if (object || list) {
      size_t n = object ? v.as.object->count : rill_runtime_count(v);
      if (task->index == 0 && !rill_text_append(&bytes, object ? "{" : "[", 1))
        goto memory;
      if (task->index == n) {
        if (!rill_text_append(&bytes, object ? "}" : "]", 1))
          goto memory;
        --count;
      } else {
        size_t i = task->index++;
        if (i && (!object || i % 2 == 0) && !rill_text_append(&bytes, ",", 1))
          goto memory;
        if (object && i % 2 && !rill_text_append(&bytes, ":", 1))
          goto memory;
        RillValue child =
            object ? v.as.object->values[i] : rill_runtime_at(v, i);
        size_t depth = task->depth + (object && i % 2 == 0 ? 0 : 1),
               needed = {};
        if (ckd_add(&needed, count, 1))
          goto memory;
        Encode *next = reserve(stack, &capacity, needed, sizeof(*stack));
        if (!next)
          goto memory;
        stack = next;
        stack[count++] = (Encode){.value = child, .depth = depth};
      }
    } else {
      // A stack value borrows language bytes; yyjson owns escaping and number
      // formatting.
      yyjson_mut_val scalar = {};
      bool supported = true;
      if (v.kind == RILL_V_NULL)
        (void)yyjson_mut_set_null(&scalar);
      else if (v.kind == RILL_V_BOOL)
        (void)yyjson_mut_set_bool(&scalar, v.as.integer != 0);
      else if (v.kind == RILL_V_INT)
        (void)yyjson_mut_set_sint(&scalar, v.as.integer);
      else if (v.kind == RILL_V_FLOAT)
        (void)yyjson_mut_set_real(&scalar, v.as.real);
      else if (v.kind == RILL_V_STRING)
        (void)yyjson_mut_set_strn(&scalar, v.as.object->bytes.data,
                                  v.as.object->bytes.size);
      else
        supported = false;
      if (!supported) {
        RillValue detail = unsupported(heap, stack, count, error);
        free(stack);
        return detail;
      }
      size_t size = {};
      char *text = yyjson_mut_val_write(&scalar, 0, &size);
      if (!text)
        goto memory;
      bool fits = bytes.size <= max_bytes && size <= max_bytes - bytes.size;
      bool ok = fits && rill_text_append(&bytes, text, size);
      free(text);
      if (!fits) {
        *error = (RillDiagnostic){.kind = RILL_LIMIT,
                                  .message = "JSON output byte limit exceeded"};
        break;
      }
      if (!ok)
        goto memory;
      --count;
    }
    if (bytes.size > max_bytes)
      *error = (RillDiagnostic){.kind = RILL_LIMIT,
                                .message = "JSON output byte limit exceeded"};
  }
  free(stack);
  if (error->kind)
    return (RillValue){};
  RillValue result = rill_runtime_object(heap, RILL_V_BYTES, nullptr, 0,
                                         bytes.data, bytes.size, 0);
  if (result.kind == RILL_V_UNIT)
    *error = (RillDiagnostic){.kind = RILL_MEMORY,
                              .message = "JSON allocation failed"};
  return result;
memory:
  free(stack);
  *error = (RillDiagnostic){.kind = RILL_MEMORY,
                            .message = "JSON allocation failed"};
  return (RillValue){};
}
RillValue rill_library_json(RillHeap *heap, bool encoding, RillValue options,
                            RillValue input, RillDiagnostic *error) {
  RillLimit limits[] = {{"max_bytes", (size_t)64 * 1024 * 1024, 0},
                        {"max_depth", 256, 1}};
  if (!rill_library_limits(options, limits, 2, error))
    return (RillValue){};
  if (encoding)
    return encode(heap, input, limits[0].value, limits[1].value, error);
  if (input.kind != RILL_V_BYTES && input.kind != RILL_V_STRING) {
    *error = (RillDiagnostic){.kind = RILL_TYPE,
                              .message = "JSON input requires Bytes or String"};
    return (RillValue){};
  }
  if (input.as.object->bytes.size > limits[0].value) {
    *error = (RillDiagnostic){.kind = RILL_LIMIT,
                              .message = "JSON input byte limit exceeded"};
    return (RillValue){};
  }
  return decode(heap, input, limits[1].value, error);
}
