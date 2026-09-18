/** @file
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
/** @brief Pending native effect, owning no runtime values outside explicit
 * roots. */
typedef struct {
  RillJob *job;      ///< Supervisor-owned pending job.
  int64_t operation; ///< Native operation awaiting completion.
} RillNativePending;
/**
 * @brief Borrowed session services and explicit exit-request state.
 */
typedef struct {
  RillExec *exec;               ///< Supervisor.
  RillEnvironment *environment; ///< Mutable session environment.
  RillEval *eval;               ///< Runtime and heap.
  bool exit_requested;          ///< Successful explicit exit request.
  int exit_code;                ///< Requested 0 through 255.
} RillLibrary;
/**
 * @brief Borrow the static native registry and write its entry count.
 */
const RillNative *rill_library_natives(size_t *count);
/**
 * @brief Dispatch a native request through the session services.
 *
 * The event must be the evaluator's current native request. False leaves an
 * operation in pending; true means the evaluator was resumed, possibly with
 * an error. Filesystem effects may block.
 */
bool rill_library_call(RillLibrary *library, RillNativePending *pending,
                       RillEvalEvent event);
/**
 * @brief Check a pending operation after supervisor progress, without waiting.
 *
 * False means still pending; true means finished or no operation remains.
 */
bool rill_library_progress(RillLibrary *library, RillNativePending *pending);
/**
 * @brief Append a display representation without invoking functions or
 * launching plans.
 *
 * Containers use summaries; scalar bytes are escaped. Failure may leave a
 * partial append. The output must not alias the value's storage.
 */
[[nodiscard]] bool rill_library_display(RillValue value, RillBuffer *output);

/**
 * @brief Collect Values, then prune unreferenced acknowledged job reports.
 *
 * Keep temporary language Values rooted. Live or unacknowledged jobs survive
 * regardless of whether a language handle remains.
 */
void rill_library_collect(RillLibrary *library);
