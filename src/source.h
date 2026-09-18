/** @file
 * @brief Owned source identity and byte spans shared across components.
 */
#pragma once
#include "diagnostic.h"
#include "text/text.h"
#include <stddef.h>
/** @brief Owned, immutable source; offsets are bytes, not display cells. */
typedef struct {
  char *name;       ///< Owned logical diagnostic name.
  RillBuffer bytes; ///< Owned source bytes.
} RillSource;
/**
 * @brief Copy a C-string name and UTF-8 source without embedded NUL.
 *
 * The destination must own no storage. A null data pointer is valid only for
 * empty input. Failure leaves an empty, clearable source.
 * @return RILL_OK, RILL_SYNTAX for invalid source, or RILL_MEMORY.
 */
[[nodiscard]] RillError rill_source_init(RillSource *source, const char *name,
                                         const char *data, size_t size);
/** @brief Release the name and bytes, then reset; an empty source is valid. */
void rill_source_clear(RillSource *source);
