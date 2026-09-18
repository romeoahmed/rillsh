/**
 * @file
 * @brief Strict JSON document and decimal-number conversion.
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
/**
 * @brief Parse a whole String as a JSON decimal token into Float or Int.
 *
 * real selects Float; otherwise require an integer token within Int range.
 * No trimming or extended number syntax. Does not allocate language values or
 * collect. Initialize error to success; failure sets it and returns Unit.
 */
RillValue rill_library_json_number(bool real, RillValue input,
                                   RillDiagnostic *error);
