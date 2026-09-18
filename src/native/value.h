/**
 * @file
 * @brief Shared value construction and explicit library limit validation.
 */
#pragma once
#include "diagnostic.h"
#include "runtime/runtime.h"
#include <stddef.h>
/** @brief A known size limit, initialized to its default before parsing. */
typedef struct {
  const char *name; ///< Static option name.
  size_t value;     ///< Default or validated value.
  size_t minimum;   ///< Smallest accepted value.
} RillLimit;
/**
 * @brief Apply validated options to caller-supplied defaults without
 * allocating.
 *
 * Failure sets error and may leave earlier limits updated; discard that set.
 */
bool rill_library_limits(RillValue options, RillLimit *limits, size_t count,
                         RillDiagnostic *error);
/**
 * @brief Build a Record with copied keys, rooting input Values during the call.
 *
 * May collect. Keys must be unique C strings whose backing storage survives
 * collection. Returns Unit on allocation failure; root a successful result
 * before further allocation.
 */
RillValue rill_library_record(RillHeap *heap, const char *const *keys,
                              const RillValue *values, size_t count);
