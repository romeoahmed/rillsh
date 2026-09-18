#include "../helpers/fds.h"
#include "diagnostic.h"
#include "platform/posix.h"
#include "test.h"
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include <sys/select.h>
#include <unistd.h>

static void readiness() {
  struct rlimit limit = {};
  CHECK(getrlimit(RLIMIT_NOFILE, &limit) == 0);
  const int minimums[] = {3, FD_SETSIZE - 1, FD_SETSIZE, FD_SETSIZE + 17,
                          2 * FD_SETSIZE + 1};
  CHECK(!rill_platform_input_ready(-1));
  for (size_t i = 0; i < sizeof(minimums) / sizeof(*minimums); ++i) {
    if ((rlim_t)minimums[i] >= limit.rlim_cur)
      continue;
    int pipe[2];
    CHECK(rill_platform_pipe(pipe, true));
    int input = fcntl(pipe[0], F_DUPFD_CLOEXEC, minimums[i]);
    CHECK(input >= minimums[i]);
    int flags = fcntl(input, F_GETFL);
    CHECK(flags >= 0 && !rill_platform_input_ready(input));
    CHECK(write(pipe[1], "x", 1) == 1);
    CHECK(rill_platform_input_ready(input));
    char byte = {};
    CHECK(read(input, &byte, 1) == 1 && byte == 'x');
    CHECK(!rill_platform_input_ready(input));
    rill_platform_close(&pipe[1]);
    CHECK(rill_platform_input_ready(input));
    CHECK(read(input, &byte, 1) == 0);
    CHECK(fcntl(input, F_GETFL) == flags);
    CHECK(close(input) == 0);
    CHECK(!rill_platform_input_ready(input));
    rill_platform_close(&pipe[0]);
  }
}

static void environment() {
  char original[] = "A=first";
  char *imported[] = {original,
                      (char[]){"broken"},
                      (char[]){"=bad"},
                      (char[]){"A=second"},
                      (char[]){"LC_ALL=C"},
                      nullptr};
  RillEnvironment env = {};
  CHECK(rill_platform_env_init(&env, imported));
  original[2] = 'X';
  CHECK(env.count == 2 && !strcmp(rill_platform_env_get(&env, "A"), "first"));
  CHECK(!rill_platform_env_get(&env, "absent"));
  CHECK(!rill_platform_env_set(&env, "", "bad") && errno == EINVAL);
  CHECK(!rill_platform_env_set(&env, "A=B", "bad") && errno == EINVAL);
  CHECK(env.count == 2 && !strcmp(rill_platform_env_get(&env, "A"), "first"));
  CHECK(rill_platform_env_set(&env, "A", "\xff=value"));
  CHECK(env.count == 2 &&
        !strcmp(rill_platform_env_get(&env, "A"), "\xff=value"));
  CHECK(rill_platform_env_set(&env, "EMPTY", ""));
  CHECK(env.count == 3 && !strcmp(rill_platform_env_get(&env, "EMPTY"), ""));
  CHECK(rill_platform_env_set(&env, "A", nullptr));
  CHECK(rill_platform_env_set(&env, "absent", nullptr));
  CHECK(env.count == 2 && !rill_platform_env_get(&env, "A"));
  CHECK(env.entries[env.count] == nullptr);
  char *cwd = getcwd(nullptr, 0);
  CHECK(cwd);
  RillDiagnostic error = {};
  CHECK(!rill_platform_cd(&env, "/dev/null/rill-missing", &error));
  char *after = getcwd(nullptr, 0);
  CHECK(after && !strcmp(cwd, after));
  free(after);
  free(cwd);
  rill_platform_env_clear(&env);
}

int main() {
  environment();
  int baseline = descriptor_count(0);
  CHECK(baseline >= 0);
  readiness();
  const int signals[] = {SIGSEGV, SIGBUS,  SIGINT,  SIGTERM,
                         SIGCHLD, SIGQUIT, SIGPIPE, SIGTTOU};
  struct sigaction saved[sizeof(signals) / sizeof(*signals)];
  for (size_t i = 0; i < sizeof(signals) / sizeof(*signals); ++i)
    CHECK(sigaction(signals[i], nullptr, &saved[i]) == 0);
  // Managed signals must work even when exec inherited a blocked mask.
  sigset_t blocked, original, current_mask;
  CHECK(sigemptyset(&blocked) == 0);
  CHECK(sigaddset(&blocked, SIGINT) == 0);
  CHECK(sigaddset(&blocked, SIGTERM) == 0);
  CHECK(sigaddset(&blocked, SIGCHLD) == 0);
  CHECK(sigaddset(&blocked, SIGUSR1) == 0);
  CHECK(sigprocmask(SIG_BLOCK, &blocked, &original) == 0);
  RillPlatform platform = {};
  struct rlimit limit = {};
  CHECK(getrlimit(RLIMIT_NOFILE, &limit) == 0);
  struct rlimit exhausted = limit;
  exhausted.rlim_cur = 0;
  CHECK(setrlimit(RLIMIT_NOFILE, &exhausted) == 0);
  bool initialized = rill_platform_init(&platform, false);
  int failure = errno;
  CHECK(setrlimit(RLIMIT_NOFILE, &limit) == 0);
  CHECK(!initialized && failure == EMFILE);
  CHECK(sigprocmask(SIG_SETMASK, nullptr, &current_mask) == 0);
  CHECK(sigismember(&current_mask, SIGINT) == 1);
  CHECK(sigismember(&current_mask, SIGUSR1) == 1);
  CHECK(sigismember(&current_mask, SIGPIPE) == sigismember(&original, SIGPIPE));
  CHECK(descriptor_count(0) == baseline);
  CHECK(raise(SIGINT) == 0);
  CHECK(rill_platform_init(&platform, false));
  CHECK(sigprocmask(SIG_SETMASK, nullptr, &current_mask) == 0);
  CHECK(sigismember(&current_mask, SIGINT) == 0);
  CHECK(sigismember(&current_mask, SIGTERM) == 0);
  CHECK(sigismember(&current_mask, SIGCHLD) == 0);
  CHECK(sigismember(&current_mask, SIGUSR1) == 1);
  CHECK(rill_platform_signals(&platform) == RILL_SIG_INT);
  const int ignored[] = {SIGQUIT, SIGPIPE, SIGTTOU};
  for (size_t i = 0; i < sizeof(ignored) / sizeof(*ignored); ++i) {
    struct sigaction current = {};
    CHECK(sigaction(ignored[i], nullptr, &current) == 0);
    CHECK(current.sa_handler == SIG_IGN);
  }
  for (size_t i = 0; i < 2; ++i) {
    struct sigaction current = {};
    CHECK(sigaction(signals[i], nullptr, &current) == 0);
    CHECK(current.sa_flags == saved[i].sa_flags);
    if (current.sa_flags & SA_SIGINFO)
      CHECK(current.sa_sigaction == saved[i].sa_sigaction);
    else
      CHECK(current.sa_handler == saved[i].sa_handler);
  }
  CHECK(!rill_platform_input_ready(platform.signals[0]));
  CHECK(raise(SIGCHLD) == 0);
  CHECK(rill_platform_input_ready(platform.signals[0]));
  CHECK(rill_platform_signals(&platform) == 0);
  CHECK(!rill_platform_input_ready(platform.signals[0]));
  // A full pipe must preserve control flags and the interrupted errno.
  const char bytes[1024] = {};
  while (write(platform.signals[1], bytes, sizeof(bytes)) > 0) {
  }
  CHECK(errno == EAGAIN);
  errno = EDOM;
  CHECK(raise(SIGINT) == 0 && raise(SIGTERM) == 0 && raise(SIGHUP) == 0 &&
        raise(SIGTSTP) == 0);
  CHECK(errno == EDOM);
  unsigned events = rill_platform_signals(&platform);
  CHECK(events ==
        (RILL_SIG_INT | RILL_SIG_TERM | RILL_SIG_HUP | RILL_SIG_STOP));
  CHECK(rill_platform_signals(&platform) == 0);
  CHECK(!rill_platform_input_ready(platform.signals[0]));
  rill_platform_clear(&platform);
  CHECK(sigprocmask(SIG_SETMASK, nullptr, &current_mask) == 0);
  CHECK(sigismember(&current_mask, SIGINT) == 1);
  CHECK(sigismember(&current_mask, SIGTERM) == 1);
  CHECK(sigismember(&current_mask, SIGCHLD) == 1);
  CHECK(sigismember(&current_mask, SIGUSR1) == 1);
  CHECK(sigprocmask(SIG_SETMASK, &original, nullptr) == 0);
  for (size_t i = 0; i < sizeof(signals) / sizeof(*signals); ++i) {
    struct sigaction current = {};
    CHECK(sigaction(signals[i], nullptr, &current) == 0);
    if (current.sa_flags & SA_SIGINFO)
      CHECK(current.sa_sigaction == saved[i].sa_sigaction);
    else
      CHECK(current.sa_handler == saved[i].sa_handler);
  }
  for (int nonblocking = 0; nonblocking < 2; ++nonblocking) {
    int fds[2];
    CHECK(rill_platform_pipe(fds, nonblocking != 0));
    for (size_t i = 0; i < 2; ++i) {
      CHECK(fds[i] > 2 && fcntl(fds[i], F_GETFD) == FD_CLOEXEC);
      CHECK(((fcntl(fds[i], F_GETFL) & O_NONBLOCK) != 0) == (nonblocking != 0));
      rill_platform_close(&fds[i]);
      CHECK(fds[i] == -1);
      rill_platform_close(&fds[i]);
    }
  }
  CHECK(descriptor_count(0) == baseline);
}
