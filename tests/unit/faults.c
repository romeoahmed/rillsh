#include "../helpers/fds.h"
#include "../helpers/supervisor.h"
#include "diagnostic.h"
#include "exec/exec.h"
#include "platform/posix.h"
#include "source.h"
#include "syntax/syntax.h"
#include "test.h"
#include "text/text.h"
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

// Only the subject translation units receive the syscall/allocator aliases.
// These wrappers and the rest of the supervisor still call the real POSIX APIs.
char *rill_test_getcwd(char *buffer, size_t size);
int rill_test_fchdir(int fd);
pid_t rill_test_fork();
int rill_test_setpgid(pid_t pid, pid_t group);
void *rill_test_malloc(size_t size);
void *rill_test_calloc(size_t count, size_t size);
void *rill_test_realloc(void *pointer, size_t size);
char *rill_test_strdup(const char *text);

static int allocation_budget = -1;
static int cwd_calls, fork_calls, group_calls;
static int fail_fork, fail_group, fail_cwd;
static bool fail_rollback, kill_before_exec, stop_before_group_failure;

static bool allocation_fails() {
  if (allocation_budget < 0)
    return false;
  if (allocation_budget-- > 0)
    return false;
  allocation_budget = 0;
  errno = ENOMEM;
  return true;
}
void *rill_test_malloc(size_t size) {
  return allocation_fails() ? nullptr : malloc(size);
}
void *rill_test_calloc(size_t count, size_t size) {
  return allocation_fails() ? nullptr : calloc(count, size);
}
void *rill_test_realloc(void *pointer, size_t size) {
  return allocation_fails() ? nullptr : realloc(pointer, size);
}
char *rill_test_strdup(const char *text) {
  return allocation_fails() ? nullptr : strdup(text);
}
char *rill_test_getcwd(char *buffer, size_t size) {
  if (++cwd_calls == fail_cwd) {
    errno = ENOMEM;
    return nullptr;
  }
  return getcwd(buffer, size);
}
int rill_test_fchdir(int fd) {
  if (fail_rollback) {
    errno = EACCES;
    return -1;
  }
  return fchdir(fd);
}
pid_t rill_test_fork() {
  if (++fork_calls == fail_fork) {
    errno = EAGAIN;
    return -1;
  }
  return fork();
}
int rill_test_setpgid(pid_t pid, pid_t group) {
  if (++group_calls == fail_group) {
    if (stop_before_group_failure) {
      CHECK(kill(pid, SIGSTOP) == 0);
      siginfo_t info;
      CHECK(waitid(P_PID, (id_t)pid, &info, WSTOPPED | WNOWAIT) == 0);
      CHECK(info.si_code == CLD_STOPPED);
    }
    errno = EPERM;
    return -1;
  }
  int result = setpgid(pid, group);
  if (result == 0 && kill_before_exec) {
    CHECK(kill(pid, SIGKILL) == 0);
    siginfo_t info;
    // Observe termination without reaping: ownership remains with the
    // supervisor.
    CHECK(waitid(P_PID, (id_t)pid, &info, WEXITED | WNOWAIT) == 0);
    CHECK(info.si_code == CLD_KILLED && info.si_status == SIGKILL);
  }
  return result;
}
static void assert_cwd(const char *expected) {
  char *actual = getcwd(nullptr, 0);
  CHECK(actual && !strcmp(actual, expected));
  free(actual);
}
static void check_directory(int base, const char *cwd) {
  char *entries[] = {(char[]){"OLDPWD=kept"}, nullptr};
  RillEnvironment env;
  RillDiagnostic error;
  int baseline = descriptor_count(0);
  CHECK(rill_platform_env_init(&env, entries));
  CHECK(rill_platform_env_set(&env, "PWD", cwd));
  cwd_calls = 0;
  fail_cwd = 1;
  CHECK(!rill_platform_cd(&env, "target", &error));
  fail_cwd = 0;
  CHECK(error.kind == RILL_IO && error.code == ENOMEM);
  assert_cwd(cwd);
  CHECK(!strcmp(rill_platform_env_get(&env, "PWD"), cwd));
  rill_platform_env_clear(&env);
  CHECK(descriptor_count(0) == baseline);
  for (int rollback = 0; rollback < 2; ++rollback) {
    CHECK(rill_platform_env_init(&env, entries));
    CHECK(rill_platform_env_set(&env, "PWD", cwd));
    cwd_calls = 0;
    fail_cwd = 2;
    fail_rollback = rollback != 0;
    CHECK(!rill_platform_cd(&env, "target", &error));
    fail_cwd = 0;
    fail_rollback = false;
    CHECK(error.kind == RILL_IO);
    CHECK(error.code == (rollback ? EACCES : ENOMEM));
    CHECK(!strcmp(rill_platform_env_get(&env, "OLDPWD"), "kept"));
    if (rollback) {
      CHECK(!rill_platform_env_get(&env, "PWD"));
      struct stat actual, target;
      CHECK(stat(".", &actual) == 0 &&
            fstatat(base, "target", &target, 0) == 0);
      CHECK(actual.st_dev == target.st_dev && actual.st_ino == target.st_ino);
    } else {
      assert_cwd(cwd);
      CHECK(!strcmp(rill_platform_env_get(&env, "PWD"), cwd));
    }
    CHECK(fchdir(base) == 0);
    rill_platform_env_clear(&env);
    CHECK(descriptor_count(0) == baseline);
  }
  // Fail each successive allocation in this transaction, including PWD commit.
  bool completed = false;
  for (int budget = 0; budget < 100 && !completed; ++budget) {
    CHECK(rill_platform_env_init(&env, entries));
    CHECK(rill_platform_env_set(&env, "PWD", cwd));
    allocation_budget = budget;
    completed = rill_platform_cd(&env, "target", &error);
    allocation_budget = -1;
    if (!completed) {
      CHECK(error.kind == RILL_IO && error.code == ENOMEM);
      assert_cwd(cwd);
      CHECK(!strcmp(rill_platform_env_get(&env, "PWD"), cwd));
      CHECK(!strcmp(rill_platform_env_get(&env, "OLDPWD"), "kept"));
    } else {
      char *actual = getcwd(nullptr, 0);
      CHECK(actual && !strcmp(rill_platform_env_get(&env, "PWD"), actual));
      CHECK(!strcmp(rill_platform_env_get(&env, "OLDPWD"), cwd));
      free(actual);
    }
    CHECK(fchdir(base) == 0);
    rill_platform_env_clear(&env);
    CHECK(descriptor_count(0) == baseline);
  }
  CHECK(completed);
}
static void finish(RillExec *exec, RillPlatform *platform, RillJob *job) {
  int64_t deadline = rill_platform_now() + 5000;
  while (rill_exec_state(job) != RILL_JOB_COMPLETED) {
    CHECK(rill_platform_now() < deadline);
    (void)rill_exec_poll(exec, 10, -1);
    (void)rill_platform_signals(platform);
  }
  errno = 0;
  CHECK(waitpid(-1, nullptr, WNOHANG) == -1 && errno == ECHILD);
}
static void discard(RillExec *exec, RillJob *job, int baseline) {
  rill_exec_acknowledge(job);
  rill_exec_prune(exec);
  CHECK(!rill_exec_first(exec));
  CHECK(descriptor_count(0) == baseline);
}
static RillExec *active_exec;
static void cleanup_launch() {
  if (!supervisor_cleanup(active_exec))
    _Exit(99);
}

static void check_launch(const char *child) {
  RillPlatform platform;
  CHECK(rill_platform_init(&platform, false));
  RillExec *exec = rill_exec_new(&platform);
  CHECK(exec);
  active_exec = exec;
  RillEnvironment env;
  extern char **environ;
  CHECK(rill_platform_env_init(&env, environ));
  RillBytes args[] = {{child, strlen(child)}, {"mark", 4}, {"launched", 8}};
  RillExecStage stages[3];
  for (size_t i = 0; i < 3; ++i)
    stages[i] = (RillExecStage){.argv = args, .argc = 3};
  RillExecSpec spec = {.stages = stages,
                       .count = 3,
                       .environment = &env,
                       .capture = true,
                       .capture_limit = 1024};
  int baseline = descriptor_count(0);
  const struct {
    const char *name;
    int fork_call, group_call;
    bool stopped, killed;
    size_t stage;
  } cases[] = {{"first fork", 1, 0, false, false, 0},
               {"later fork", 2, 0, false, false, 1},
               {"first group", 0, 1, false, false, 0},
               {"later group", 0, 2, false, false, 1},
               {"before exec", 0, 0, false, true, 0},
               {"first stopped group", 0, 1, true, false, 0},
               {"later stopped group", 0, 2, true, false, 1}};
  for (size_t i = 0; i < sizeof(cases) / sizeof(*cases); ++i) {
    (void)fprintf(stderr, "launch fault: %s\n", cases[i].name);
    fork_calls = group_calls = 0;
    fail_fork = cases[i].fork_call;
    fail_group = cases[i].group_call;
    stop_before_group_failure = cases[i].stopped;
    kill_before_exec = cases[i].killed;
    spec.count = cases[i].killed ? 1 : 3;
    sigset_t before, after;
    CHECK(sigprocmask(SIG_SETMASK, nullptr, &before) == 0);
    RillDiagnostic error;
    RillJob *job = rill_exec_launch(exec, &spec, &error);
    fail_fork = fail_group = 0;
    kill_before_exec = false;
    stop_before_group_failure = false;
    CHECK(job);
    CHECK(sigprocmask(SIG_SETMASK, nullptr, &after) == 0);
    CHECK(sigismember(&before, SIGCHLD) == sigismember(&after, SIGCHLD));
    CHECK(sigismember(&before, SIGINT) == sigismember(&after, SIGINT));
    finish(exec, &platform, job);
    CHECK(rill_exec_result(job) != 0);
    if (cases[i].killed) {
      size_t count;
      const RillExecStatus *status = rill_exec_status(job, &count);
      CHECK(count == 1 && status[0].done && status[0].signaled);
      CHECK(status[0].status == SIGKILL);
      CHECK(rill_exec_launched(job)); // A child killed before exec also closes
                                      // the handshake channel.
    } else {
      error = rill_exec_error(job);
      CHECK(error.kind == RILL_LAUNCH);
      CHECK(error.code == (cases[i].fork_call ? EAGAIN : EPERM));
      CHECK(error.stage == cases[i].stage);
    }
    CHECK(access("launched", F_OK) == -1 && errno == ENOENT);
    discard(exec, job, baseline);
  }
  args[1] = (RillBytes){"signals", 7};
  stages[0].argc = 2;
  spec.count = 1;
  bool completed = false;
  for (int budget = 0; budget < 100 && !completed; ++budget) {
    allocation_budget = budget;
    RillDiagnostic error;
    RillJob *job = rill_exec_launch(exec, &spec, &error);
    allocation_budget = -1;
    if (job) {
      finish(exec, &platform, job);
      CHECK(!rill_exec_result(job));
      discard(exec, job, baseline);
      completed = true;
    } else {
      CHECK(error.kind == RILL_MEMORY);
      CHECK(descriptor_count(0) == baseline);
      CHECK(!rill_exec_first(exec));
      CHECK(waitpid(-1, nullptr, WNOHANG) == -1 && errno == ECHILD);
    }
  }
  CHECK(completed);
  active_exec = nullptr;
  rill_exec_free(exec);
  rill_platform_env_clear(&env);
  rill_platform_clear(&platform);
}
static void check_text_and_syntax() {
  RillBuffer buffer = {};
  CHECK(rill_text_append(&buffer, "kept", 4));
  char *storage = buffer.data;
  allocation_budget = 0;
  CHECK(!rill_text_format(&buffer, "%0100d", 1));
  allocation_budget = -1;
  CHECK(buffer.data == storage && buffer.size == 4);
  CHECK(!strcmp(buffer.data, "kept"));
  const char controls[64] = {};
  allocation_budget = 0;
  CHECK(!rill_text_escape(&buffer, (RillBytes){controls, sizeof(controls)}));
  allocation_budget = -1;
  CHECK(buffer.data == storage && buffer.size == 4);
  CHECK(!strcmp(buffer.data, "kept"));
  CHECK(!rill_text_append(&buffer, nullptr, SIZE_MAX));
  CHECK(errno == ENOMEM && buffer.data == storage && buffer.size == 4);
  rill_text_clear(&buffer);

  const char *cases[] = {"\"\\u{1f642}\"", "let x = [1, 'two']; x[0]",
                         "job { ^cat 'argument' | ^cat > result 2>&1 }"};
  for (size_t i = 0; i < sizeof(cases) / sizeof(*cases); ++i) {
    RillSource source;
    CHECK(rill_source_init(&source, "fault", cases[i], strlen(cases[i])) ==
          RILL_OK);
    bool complete = false;
    for (int budget = 0; budget < 256 && !complete; ++budget) {
      allocation_budget = budget;
      RillSyntax syntax = rill_syntax_parse(&source);
      allocation_budget = -1;
      complete = syntax.state == RILL_COMPLETE;
      if (!complete)
        CHECK(syntax.state == RILL_INVALID &&
              syntax.diagnostic.kind == RILL_MEMORY);
      rill_syntax_clear(&syntax);
    }
    CHECK(complete);
    rill_source_clear(&source);
  }
  bool complete = false;
  for (int budget = 0; budget < 20 && !complete; ++budget) {
    RillSource source;
    allocation_budget = budget;
    RillError error = rill_source_init(&source, "input", "valid", 5);
    allocation_budget = -1;
    complete = error == RILL_OK;
    if (!complete)
      CHECK(error == RILL_MEMORY && !source.name && !source.bytes.data);
    rill_source_clear(&source);
  }
  CHECK(complete);
}

int main(int argc, char **argv) {
  CHECK(argc == 3);
  if (!strcmp(argv[2], "memory")) {
    check_text_and_syntax();
    return 0;
  }
  CHECK(atexit(cleanup_launch) == 0);
  char *child = realpath(argv[1], nullptr);
  CHECK(child);
  int original = open(".", O_RDONLY | O_DIRECTORY | O_CLOEXEC);
  CHECK(original >= 0);
  char temporary[] = "rill-faults-XXXXXX";
  CHECK(mkdtemp(temporary));
  CHECK(chdir(temporary) == 0 && mkdir("target", 0700) == 0);
  int base = open(".", O_RDONLY | O_DIRECTORY | O_CLOEXEC);
  char *cwd = getcwd(nullptr, 0);
  CHECK(base >= 0 && cwd);
  if (!strcmp(argv[2], "directory"))
    check_directory(base, cwd);
  else {
    CHECK(!strcmp(argv[2], "launch"));
    check_launch(child);
  }
  CHECK(close(base) == 0);
  CHECK(rmdir("target") == 0 && fchdir(original) == 0);
  CHECK(rmdir(temporary) == 0 && close(original) == 0);
  free(cwd);
  free(child);
  return 0;
}
