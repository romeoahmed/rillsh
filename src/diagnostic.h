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
  RILL_MEMORY,
  RILL_MATCH_ERROR,
  RILL_ARITHMETIC,
  RILL_MISSING_FIELD
} RillError;
/**
 * @brief Borrowed diagnostic with a source byte offset and optional native
 * error.
 *
 * Zero initialization means success. A label identifies a language Error with
 * explicit label/message lengths; otherwise message is a static C string.
 * Consume evaluator-rooted text before resuming or aborting evaluation.
 */
typedef struct {
  size_t label_size;   ///< Byte length when a language Error supplies label.
  size_t message_size; ///< Byte length of a language Error message.
  const char *label;   ///< Optional evaluator-rooted language Error kind.
  RillError kind;      ///< Semantic category.
  size_t offset;       ///< Source byte offset.
  int code;            ///< Native errno or selected shell exit status.
  const char *message; ///< Static or evaluator-rooted explanation.
  bool has_argument;   ///< Argument index is meaningful for this failure.
  size_t argument;     ///< Zero-based argv index when present.
  size_t stage; ///< Zero-based stage responsible for a launch/process error.
} RillDiagnostic;
/** @brief Return the stable printable category name. */
const char *rill_diagnostic_name(RillError kind);
