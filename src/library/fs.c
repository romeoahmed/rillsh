/**
 * @file
 * @brief Convert filesystem results into bounded language values.
 *
 * Read regular files as strict UTF-8 and expand explicit glob patterns into
 * byte-sorted Paths. Temporary OS and libc storage is released independently
 * of GC; returned Values copy the data they retain.
 */
#include "fs.h"
#include "diagnostic.h"
#include "platform/posix.h"
#include "runtime/runtime.h"
#include "text/text.h"
#include "value.h"
#include <errno.h>
#include <fcntl.h>
#include <glob.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

static bool path_bytes(RillValue value, RillBytes *bytes,
                       RillDiagnostic *error) {
  if (value.kind != RILL_V_PATH && value.kind != RILL_V_STRING &&
      value.kind != RILL_V_BYTES) {
    *error = (RillDiagnostic){.kind = RILL_TYPE,
                              .message = "path requires Path, String or Bytes"};
    return false;
  }
  *bytes = value.as.object->bytes;
  if (memchr(bytes->data, 0, bytes->size)) {
    *error = (RillDiagnostic){.kind = RILL_TYPE,
                              .message = "path cannot contain NUL"};
    return false;
  }
  return true;
}
RillValue rill_library_read_text(RillHeap *heap, RillValue options,
                                 RillValue path, RillDiagnostic *error) {
  RillLimit limit = {"max_bytes", (size_t)64 * 1024 * 1024, 0};
  RillBytes name = {};
  if (!rill_library_limits(options, &limit, 1, error) ||
      !path_bytes(path, &name, error))
    return (RillValue){};
  [[gnu::cleanup(rill_platform_close)]] int fd = rill_platform_internal(
      open(name.data, O_RDONLY | O_CLOEXEC | O_NOCTTY | O_NONBLOCK));
  struct stat st = {};
  if (fd < 0 || fstat(fd, &st) < 0) {
    *error = (RillDiagnostic){
        .kind = RILL_IO, .code = errno, .message = "cannot open text file"};
    return (RillValue){};
  }
  if (!S_ISREG(st.st_mode)) {
    *error = (RillDiagnostic){.kind = RILL_IO,
                              .message = "read_text requires a regular file"};
    return (RillValue){};
  }
  [[gnu::cleanup(rill_text_clear)]] RillBuffer buffer = {};
  for (;;) {
    char bytes[16384];
    ssize_t n = read(fd, bytes, sizeof(bytes));
    if (n < 0 && errno == EINTR)
      continue;
    if (n < 0) {
      *error = (RillDiagnostic){
          .kind = RILL_IO, .code = errno, .message = "cannot read text file"};
      return (RillValue){};
    }
    if (!n)
      break;
    if ((size_t)n > limit.value - buffer.size) {
      *error = (RillDiagnostic){.kind = RILL_LIMIT,
                                .message = "text file byte limit exceeded"};
      return (RillValue){};
    }
    if (!rill_text_append(&buffer, bytes, (size_t)n)) {
      *error = (RillDiagnostic){.kind = RILL_MEMORY,
                                .message = "text file allocation failed"};
      return (RillValue){};
    }
  }
  if (!rill_text_valid(buffer.data, buffer.size)) {
    *error = (RillDiagnostic){.kind = RILL_DECODE,
                              .message = "text file is not UTF-8"};
    return (RillValue){};
  }
  RillValue out = rill_runtime_object(heap, RILL_V_STRING, nullptr, 0,
                                      buffer.data, buffer.size, 0);
  if (out.kind == RILL_V_UNIT)
    *error = (RillDiagnostic){.kind = RILL_MEMORY,
                              .message = "text file allocation failed"};
  return out;
}
static int compare(const void *a, const void *b) {
  const char *const *x = a, *const *y = b;
  return strcmp(*x, *y);
}
RillValue rill_library_glob(RillHeap *heap, RillValue pattern,
                            RillDiagnostic *error) {
  if (pattern.kind != RILL_V_STRING && pattern.kind != RILL_V_BYTES) {
    *error = (RillDiagnostic){.kind = RILL_TYPE,
                              .message = "glob requires String or Bytes"};
    return (RillValue){};
  }
  RillBytes bytes = {};
  if (!path_bytes(pattern, &bytes, error))
    return (RillValue){};
  [[gnu::cleanup(globfree)]] glob_t matches = {};
  int status = glob(bytes.data, GLOB_ERR | GLOB_NOSORT, nullptr, &matches);
  if (status && status != GLOB_NOMATCH) {
    *error =
        (RillDiagnostic){.kind = status == GLOB_NOSPACE ? RILL_MEMORY : RILL_IO,
                         .message = "filesystem expansion failed"};
    return (RillValue){};
  }
  if (matches.gl_pathc > 1)
    qsort(matches.gl_pathv, matches.gl_pathc, sizeof(*matches.gl_pathv),
          compare);
  RillValue out = rill_runtime_object(heap, RILL_V_LIST, nullptr,
                                      matches.gl_pathc, nullptr, 0, 0);
  RillRoot root = {};
  rill_runtime_root(heap, &root, &out, 1);
  if (out.kind != RILL_V_UNIT)
    for (size_t i = 0; i < matches.gl_pathc; ++i) {
      out.as.object->values[i] = rill_runtime_object(
          heap, RILL_V_PATH, nullptr, 0, matches.gl_pathv[i],
          strlen(matches.gl_pathv[i]), 0);
      if (out.as.object->values[i].kind == RILL_V_UNIT) {
        out = (RillValue){};
        break;
      }
    }
  rill_runtime_unroot(heap, &root);
  if (out.kind == RILL_V_UNIT)
    *error =
        (RillDiagnostic){.kind = RILL_MEMORY,
                         .message = "filesystem expansion allocation failed"};
  return out;
}
