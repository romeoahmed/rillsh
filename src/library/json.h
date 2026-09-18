/**
 * @file
 * @brief Strict, bounded conversion between JSON documents and language data.
 */
#pragma once
#include "diagnostic.h"
#include "runtime/runtime.h"
/**
 * @brief Decode or encode one document with validated options.
 *
 * Initialize error to success and keep input and options rooted; may collect.
 * On error the returned Value may own diagnostic text: keep it rooted until
 * the diagnostic is consumed.
 * A successful result also needs a root before further allocation. yyjson
 * representations remain private to this adapter.
 */
RillValue rill_library_json(RillHeap *heap, bool encode, RillValue options,
                            RillValue input, RillDiagnostic *error);
