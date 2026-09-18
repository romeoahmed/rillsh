/** @file
 * @brief Private pure primitive dispatch; callbacks remain language
 * applications.
 */
#pragma once
#include "diagnostic.h"
#include "runtime/runtime.h"
/**
 * @brief Execute a pure primitive without invoking language callbacks.
 *
 * Initialize error to success and keep argument rooted across this call, which
 * may collect. On failure, error describes the outcome; ignore the return
 * value. Root a successful result before further allocation.
 */
RillValue rill_library_pure(RillHeap *heap, RillValue argument,
                            RillDiagnostic *error);
