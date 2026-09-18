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
#include <unistd.h>

static void environment() {
  char original[] = "A=first";
  char *imported[] = {original,
                      (char[]){"broken"},
                      (char[]){"=bad"},
                      (char[]){"A=second"},
                      (char[]){"LC_ALL=C"},
                      nullptr};
  RillEnvironment env;
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
  RillDiagnostic error;
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
  const int signals[] = {SIGSEGV, SIGBUS, SIGINT, SIGTERM, SIGCHLD};
  struct sigaction saved[5];
  for (size_t i = 0; i < 5; ++i)
    CHECK(sigaction(signals[i], nullptr, &saved[i]) == 0);
  RillPlatform platform;
  CHECK(rill_platform_init(&platform, false));
  for (size_t i = 0; i < 2; ++i) {
    struct sigaction current;
    CHECK(sigaction(signals[i], nullptr, &current) == 0);
    CHECK(current.sa_flags == saved[i].sa_flags);
    if (current.sa_flags & SA_SIGINFO)
      CHECK(current.sa_sigaction == saved[i].sa_sigaction);
    else
      CHECK(current.sa_handler == saved[i].sa_handler);
  }
  CHECK(raise(SIGINT) == 0 && raise(SIGTERM) == 0);
  unsigned events = rill_platform_signals(&platform);
  CHECK((events & (RILL_SIG_INT | RILL_SIG_TERM)) ==
        (RILL_SIG_INT | RILL_SIG_TERM));
  CHECK(rill_platform_signals(&platform) == 0);
  rill_platform_clear(&platform);
  for (size_t i = 0; i < 5; ++i) {
    struct sigaction current;
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
