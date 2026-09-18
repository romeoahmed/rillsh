/** @file
 * @brief Allocation-free component diagnostics; presentation belongs to the
 * session.
 */
#pragma once
#include <stddef.h>
/** @brief Stable error categories, distinct from child exit status. */
typedef enum {
  RILL_OK,
  RILL_SYNTAX,
  RILL_TYPE,
  RILL_LAUNCH,
  RILL_PROCESS,
  RILL_IO,
  RILL_LIMIT,
  RILL_CANCELLED,
  RILL_MEMORY
} RillError;
/** @brief Structured failure carrying a byte offset and native error, if any.
 */
typedef struct {
  RillError kind;      ///< Semantic category.
  size_t offset;       ///< Source byte offset.
  int code;            ///< Native errno or selected shell exit status.
  const char *message; ///< Static borrowed explanation.
  bool has_argument;   ///< Argument index is meaningful for this failure.
  size_t argument;     ///< Zero-based argv index when present.
  size_t stage; ///< Pipeline stage responsible for a launch/process error.
} RillDiagnostic;
/** @brief Return the stable printable category name. */
const char *rill_diagnostic_name(RillError kind);
