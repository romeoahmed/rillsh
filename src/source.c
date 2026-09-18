/**
 * @file
 * @brief Validate and own source text before parsing.
 *
 * Source names and UTF-8 bytes are copied together; invalid input leaves a
 * clearable empty owner. Diagnostics use byte offsets into this storage.
 */
#include "source.h"
#include "diagnostic.h"
#include "text/text.h"
#include <stddef.h>
#include <stdlib.h>
#include <string.h>
RillError rill_source_init(RillSource *s, const char *name, const char *data,
                           size_t n) {
  *s = (RillSource){};
  if ((n && memchr(data, 0, n)) || !rill_text_valid(data, n))
    return RILL_SYNTAX;
  s->name = strdup(name);
  if (!s->name || !rill_text_append(&s->bytes, data, n)) {
    rill_source_clear(s);
    return RILL_MEMORY;
  }
  return RILL_OK;
}
void rill_source_clear(RillSource *s) {
  free(s->name);
  rill_text_clear(&s->bytes);
  *s = (RillSource){};
}
