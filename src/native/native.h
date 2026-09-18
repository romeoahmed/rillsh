/**
 * @file
 * @brief Native unary adapters; the session supplies narrow environment/job
 * services.
 */
#pragma once
#include "exec/exec.h"
#include "platform/posix.h"
#include "runtime/runtime.h"
#include "text/text.h"
#include <stddef.h>
#include <stdint.h>
/** @brief Pending effect borrowing a supervisor-owned job; contains no Values.
 */
typedef struct {
  RillJob *job;      ///< Supervisor-owned pending job.
  int64_t operation; ///< Native operation awaiting completion.
} RillNativePending;
/** @brief Session control operation, independent of native dispatch IDs. */
typedef enum {
  RILL_CONTROL_FG,
  RILL_CONTROL_BG,
  RILL_CONTROL_WAIT,
  RILL_CONTROL_CANCEL,
  RILL_CONTROL_EXIT
} RillControl;
/**
 * @brief Borrowed session services and explicit exit-request state.
 *
 * Keep this object at a stable address while stream scopes exist, including
 * suspended scopes: stdin sources retain its session-wide lease flag.
 */
typedef struct {
  void *context; ///< Borrowed session context for narrow continuation services.
  /** @brief Handle context control; false delegates to external-job control. */
  bool (*control)(void *, RillControl, RillValue);
  /** @brief Build context snapshots; caller roots the result. Unit on failure.
   */
  RillValue (*context_jobs)(void *);
  /** @brief Whether an evaluation owns this external job. */
  bool (*owned_job)(void *, RillJob *);
  char *const *arguments;       ///< Borrowed script argument vector.
  size_t argument_count;        ///< Script arguments excluding the source path.
  RillExec *exec;               ///< Supervisor.
  RillEnvironment *environment; ///< Mutable session environment.
  struct RillStreams *streams;  ///< Active execution resource scope.
  RillEval *eval;               ///< Runtime and heap.
  /** @brief Render one value without invoking language code; false on failure.
   */
  bool (*present)(void *, RillValue);
  bool input_claimed; ///< Session-wide stdin lease, including suspended scopes.
  bool idle;          ///< Pending stream work currently needs I/O readiness.
  bool exit_requested; ///< Successful explicit exit request.
  int exit_code;       ///< Requested 0 through 255.
} RillLibrary;
/**
 * @brief Borrow the static native registry and write its entry count.
 */
const RillNative *rill_library_natives(size_t *count);
/**
 * @brief Dispatch a native request through the session services.
 *
 * The event must be the evaluator's current native request. False leaves a
 * process operation in pending or a stream operation in library state. Drive
 * process work after supervisor progress; drive stream work through
 * rill_stream_progress(). Callbacks use evaluator events.
 * True permits evaluator progress: a result, error, or requested callback.
 * May collect; filesystem effects may block. Pending stream work needs an I/O
 * wait only when library->idle is set.
 */
bool rill_library_call(RillLibrary *library, RillNativePending *pending,
                       RillEvalEvent event);
/**
 * @brief Check a pending operation after supervisor progress, without waiting.
 *
 * May collect while constructing the result or diagnostic. False means still
 * pending; true means finished or no operation remains.
 */
bool rill_library_progress(RillLibrary *library, RillNativePending *pending);
/**
 * @brief Append a display representation without invoking functions or
 * launching plans.
 *
 * Lists, Records, and plans show counts with units; text and path bytes are
 * escaped. Unit appends nothing. Failure may leave a partial append. The
 * output must not alias the value's storage. Does not collect.
 */
[[nodiscard]] bool rill_library_display(RillValue value, RillBuffer *output);

/**
 * @brief Collect Values, then prune unreferenced acknowledged job reports.
 *
 * Keep temporary language Values rooted. Live or unacknowledged jobs survive
 * regardless of whether a language handle remains.
 */
void rill_library_collect(RillLibrary *library);

/**
 * @brief Translate a rooted JobPlan and launch with the supplied I/O mode.
 *
 * Collects before translation and may block during launch preparation.
 * Initialize error to success. A returned job is supervisor-owned and still
 * needs launch-handshake and completion checks; nullptr sets error.
 */
RillJob *rill_library_launch(RillLibrary *library, RillValue plan,
                             RillExecSpec mode, RillDiagnostic *error);

/** @brief Whether interruption of this pending operation owns child cleanup. */
bool rill_library_pending_owned(const RillNativePending *pending);

/** @brief Begin incremental terminal consumption of a rooted Stream result. */
void rill_library_present(RillLibrary *library, RillValue stream);
