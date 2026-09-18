/**
 * @file
 * @brief Validate and own source text before parsing.
 *
 * Copy the diagnostic name and validated UTF-8 bytes into one source owner.
 * Failure leaves it empty and clearable. Diagnostics refer to byte offsets.
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
