/** @file
 * @brief POSIX mechanisms, process-local signal ownership and terminal handoff.
 */
#pragma once
#include "diagnostic.h"
#include <signal.h>
#include <stdint.h>

#include <stddef.h>
#include <sys/types.h>
#include <termios.h>
/** @brief Event bits exchanged while handlers are blocked. */
enum {
  RILL_SIG_INT = 1,
  RILL_SIG_TERM = 2,
  RILL_SIG_HUP = 4,
  RILL_SIG_STOP = 8
};
/** @brief One session's signal channel and optional controlling terminal. */
typedef struct {
  int signals[2];            ///< Owned nonblocking self-pipe.
  int tty;                   ///< Owned controlling-terminal description, or -1.
  pid_t group;               ///< Shell process group.
  struct termios modes;      ///< Modes restored at every prompt/exit.
  bool owns_terminal;        ///< Interactive initialization acquired the tty.
  struct sigaction saved[8]; ///< Original dispositions restored on destruction.
  size_t installed;          ///< Successfully installed handlers.
} RillPlatform;
/**
 * @brief Create a close-on-exec pipe with both descriptors above 2.
 *
 * Optionally set nonblocking mode on both ends. Failure closes all acquired
 * descriptors and leaves both outputs at -1.
 */
[[nodiscard]] bool rill_platform_pipe(int fds[2], bool nonblocking);
/**
 * @brief Consume an FD and return a close-on-exec descriptor above 2.
 *
 * The original may be closed even on success. Failure returns -1 and retains
 * no descriptor; a negative input returns -1 unchanged.
 */
int rill_platform_internal(int fd);
/**
 * @brief Close an owned descriptor, set it to -1, and preserve errno.
 */
void rill_platform_close(int *fd);
/**
 * @brief Initialize the process-wide signal owner and optional controlling
 * terminal.
 *
 * Only one initialized owner may exist; the destination must own no resources.
 * Interactive setup waits for foreground ownership. Failure releases acquired
 * resources and restores dispositions.
 */
[[nodiscard]] bool rill_platform_init(RillPlatform *platform, bool interactive);
/** @brief Restore terminal/dispositions and close descriptors after init. */
void rill_platform_clear(RillPlatform *platform);
/**
 * @brief Drain notifications and atomically exchange pending event bits.
 *
 * SIGCHLD only wakes the supervisor; child state comes from waitpid.
 */
unsigned rill_platform_signals(RillPlatform *platform);
/**
 * @brief Reset shell-managed signal dispositions and unblock all signals.
 *
 * Async-signal-safe; used in the child before exec.
 */
[[nodiscard]] bool rill_platform_child_signals();
/** @brief Block supervisor signals, saving the caller's mask. */
[[nodiscard]] bool rill_platform_block(sigset_t *previous);
/** @brief Give the tty to a process group, applying saved job modes if present.
 */
[[nodiscard]] bool rill_platform_foreground(RillPlatform *platform, pid_t group,
                                            const struct termios *modes);
/** @brief Save job modes when requested and reclaim the shell's terminal. */
[[nodiscard]] bool rill_platform_reclaim(RillPlatform *platform,
                                         struct termios *job_modes);
/** @brief Suspend the prompt shell with restored modes, then reacquire on
 * continue. */
[[nodiscard]] bool rill_platform_suspend(RillPlatform *platform);
/** @brief Monotonic milliseconds; failure yields zero. */
int64_t rill_platform_now();
/** @brief Owned environment map; import preserves the first well-formed
 * duplicate. */
typedef struct {
  char **entries; ///< Owned NUL-terminated name=value vector.
  size_t count;   ///< Entries before the terminating nullptr.
} RillEnvironment;
/**
 * @brief Copy an environment, keeping the first well-formed entry for each
 * name.
 *
 * The destination must own no storage. A null input denotes an empty map;
 * failure leaves an empty, clearable map.
 */
[[nodiscard]] bool rill_platform_env_init(RillEnvironment *env,
                                          char *const *entries);
/** @brief Free all entries and reset the map; an empty map is valid. */
void rill_platform_env_clear(RillEnvironment *env);
/** @brief Borrow a value until the next map mutation; nullptr means unset. */
const char *rill_platform_env_get(const RillEnvironment *env, const char *name);
/**
 * @brief Copy a value, or unset the name when value is nullptr.
 *
 * Names must be nonempty and contain no equals sign. Failure preserves the
 * map and sets errno; mutation invalidates borrowed value pointers.
 */
[[nodiscard]] bool rill_platform_env_set(RillEnvironment *env, const char *name,
                                         const char *value);
/**
 * @brief Change directory and commit physical PWD/OLDPWD.
 *
 * Bookkeeping failure attempts directory rollback. If rollback also fails,
 * PWD is removed and the diagnostic reports the failure; the old cwd is not
 * promised. The error output is written only on failure.
 */
[[nodiscard]] bool rill_platform_cd(RillEnvironment *env, const char *path,
                                    RillDiagnostic *error);

/**
 * @brief Test input readiness without blocking or changing descriptor flags.
 *
 * Returns false for errors and descriptors outside the select range.
 */
bool rill_platform_input_ready(int fd);
