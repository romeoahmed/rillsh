/** @file
 * @brief Asynchronous process supervision over byte-oriented launch
 * specifications.
 *
 * One session thread owns each supervisor. Callers drive progress explicitly
 * and reap live children before freeing it.
 */
#pragma once
#include "diagnostic.h"
#include "platform/posix.h"
#include "text/text.h"
#include <stddef.h>
#include <sys/types.h>
/** @brief Maximum stages in one pipeline, bounded for atomic gate release. */
static constexpr size_t RILL_EXEC_MAX_STAGES = 256;
/** @brief Maximum live or retained jobs in one session. */
static constexpr size_t RILL_EXEC_MAX_JOBS = 1024;
/** @brief Maximum arguments per stage, including the executable. */
static constexpr size_t RILL_EXEC_MAX_ARGUMENTS = 65'536;
/**
 * @brief Descriptor action applied in source order after pipeline connections.
 */
typedef struct {
  int target;            ///< Standard descriptor 0, 1, or 2.
  bool append;           ///< Append instead of truncate for output.
  bool duplicate_stdout; ///< Copy the current stdout mapping into stderr.
  RillBytes path;        ///< Borrowed pathname bytes.
} RillExecRedirect;
/** @brief Borrowed stage specification; launch copies all data it retains. */
typedef struct {
  const RillBytes *argv;             ///< Borrowed argument vector.
  size_t argc;                       ///< Argument count, including argv[0].
  const RillExecRedirect *redirects; ///< Borrowed descriptor actions.
  size_t redirect_count;             ///< Number of descriptor actions.
} RillExecStage;
/** @brief One launch, not a persistent language plan. */
typedef struct {
  const RillExecStage *stages; ///< Borrowed ordered stages.
  size_t count;                ///< Stage count; must be nonzero.
  const RillEnvironment
      *environment; ///< Borrowed environment copied during preparation.
  bool background;  ///< Default stdin is /dev/null.
  bool capture; ///< Concurrent stdout/stderr byte capture, no terminal handoff.
  bool feed;    ///< Feed input through an owned nonblocking pipe.
  RillBytes input;      ///< Copied input, only used with feed.
  size_t capture_limit; ///< Combined stdout/stderr byte limit when capturing.
} RillExecSpec;
/** @brief Observable job lifecycle. */
typedef enum {
  RILL_JOB_LAUNCHING,
  RILL_JOB_RUNNING,
  RILL_JOB_STOPPED,
  RILL_JOB_CANCELLING,
  RILL_JOB_COMPLETED
} RillJobState;
/** @brief Raw stage termination retained after aggregation. */
typedef struct {
  pid_t pid;          ///< Owned child identity until done.
  bool done;          ///< Reaped termination, not merely pipe EOF.
  bool stopped;       ///< Latest wait observation.
  bool signaled;      ///< Status denotes a signal instead of exit code.
  bool expected_pipe; ///< Connected downstream success permits SIGPIPE.
  int status; ///< Exit code, terminating signal, or current stop signal.
} RillExecStatus;
/**
 * @brief Supervisor-owned job, valid until pruned or the supervisor is freed.
 */
typedef struct RillJob RillJob;
/** @brief One supervisor, borrowing its session platform until destruction. */
typedef struct RillExec RillExec;
/**
 * @brief Allocate a supervisor borrowing platform until destruction.
 *
 * @return An owned supervisor, or nullptr on allocation failure.
 */
[[nodiscard]] RillExec *rill_exec_new(RillPlatform *platform);
/**
 * @brief Prepare a launch and register its children for supervision.
 *
 * Copies retained specification data before returning. Preparation may perform
 * blocking filesystem operations and truncate redirection files.
 * @return nullptr with error on preparation failure; otherwise a borrowed job.
 * A returned job may still fail launch: poll it, inspect its diagnostic, and
 * reap its children. It is not proof that exec succeeded.
 */
[[nodiscard]] RillJob *rill_exec_launch(RillExec *exec,
                                        const RillExecSpec *spec,
                                        RillDiagnostic *error);
/**
 * @brief Service owned I/O and child state, then poll for progress.
 *
 * The wait is capped at 20 ms, including negative timeout requests. Pass -1
 * for no extra input descriptor. A true result means extra_fd is readable;
 * it does not report job completion or drain session signal flags.
 */
bool rill_exec_poll(RillExec *exec, int timeout_ms, int extra_fd);
/** @brief Forward session signal events to the currently attached job. */
void rill_exec_signal(RillExec *exec, unsigned events);
/** @brief Return a stable session ID, never a PID authority. */
size_t rill_exec_id(const RillJob *job);
/** @brief Look up a session-owned job; absent IDs return nullptr. */
RillJob *rill_exec_find(RillExec *exec, size_t id);
/**
 * @brief Borrow stage statuses and write their count.
 *
 * The view lasts until job destruction; subsequent polling may update it.
 */
const RillExecStatus *rill_exec_status(const RillJob *job, size_t *count);
/** @brief Current state; query is side-effect free. */
RillJobState rill_exec_state(const RillJob *job);
/**
 * @brief Whether the launch error channel has closed.
 *
 * This does not prove successful exec or child termination. Inspect the
 * job diagnostic and continue supervision through completion.
 */
bool rill_exec_launched(const RillJob *job);
/**
 * @brief Return a diagnostic snapshot, independent of process exit status.
 */
RillDiagnostic rill_exec_error(const RillJob *job);
/**
 * @brief Return aggregate shell status; zero means completed successfully.
 *
 * Incomplete or erroneous jobs return failure. Cancellation returns 130;
 * otherwise the rightmost failure wins, excluding expected downstream SIGPIPE.
 */
int rill_exec_result(const RillJob *job);
/** @brief Whether cancellation was requested, even if all children exited zero.
 */
bool rill_exec_cancelled(const RillJob *job);
/**
 * @brief Borrow stdout (1) or stderr (2) capture.
 *
 * The buffer lasts until job destruction; polling may move its byte storage.
 */
const RillBuffer *rill_exec_output(const RillJob *job, int stream);
/**
 * @brief Request cancellation with TERM/CONT and timed KILL escalation.
 *
 * Idempotent; the caller must keep polling until cleanup completes.
 */
void rill_exec_cancel(RillJob *job);
/** @brief Resume or foreground a live external job; completed jobs are
 * unchanged. */
[[nodiscard]] bool rill_exec_resume(RillExec *exec, RillJob *job,
                                    bool foreground);
/** @brief Acknowledge explicit wait/fg/cancel for shutdown accounting. */
void rill_exec_acknowledge(RillJob *job);
/** @brief Check for live jobs, or unacknowledged background jobs when
 * requested. */
bool rill_exec_outstanding(const RillExec *exec, bool unjoined);
/** @brief Cancel all live jobs; caller continues polling until none remain. */
void rill_exec_cancel_all(RillExec *exec);
/**
 * @brief Free the supervisor and retained jobs; nullptr is allowed.
 *
 * @pre All owned children have been reaped. This call does not cancel or wait.
 */
void rill_exec_free(RillExec *exec);

/** @brief Iterate session jobs without interpreting numeric IDs as PIDs. */
RillJob *rill_exec_first(RillExec *exec);
/**
 * @brief Borrow the next job, or nullptr at the end.
 *
 * Do not prune jobs while traversing this chain.
 */
RillJob *rill_exec_next(RillJob *job);
/** @brief Clear retention marks before enumerating reachable language handles.
 */
void rill_exec_unmark(RillExec *exec);
/** @brief Retain a completed report named by a reachable language handle. */
void rill_exec_retain(RillExec *exec, size_t id);
/** @brief Release acknowledged completed jobs with no retained handle; never
 * signal. */
void rill_exec_prune(RillExec *exec);
