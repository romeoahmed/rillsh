/**
 * @file
 * @brief Explicit filesystem value adapters.
 */
#pragma once
#include "diagnostic.h"
#include "runtime/runtime.h"
/**
 * @brief Read bounded strict UTF-8 from a regular file; may block and collect.
 *
 * Inputs must be rooted. Failure returns Unit with a diagnostic; root a
 * successful result before further allocation.
 */
RillValue rill_library_read_text(RillHeap *heap, RillValue options,
                                 RillValue path, RillDiagnostic *error);
/**
 * @brief Expand a rooted String or Bytes pattern to byte-sorted Paths.
 *
 * May block and collect. No matches returns an empty List; failure returns
 * Unit with a diagnostic. Root a successful result before further allocation.
 */
RillValue rill_library_glob(RillHeap *heap, RillValue pattern,
                            RillDiagnostic *error);
