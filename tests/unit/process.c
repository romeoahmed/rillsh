/**
 * @file
 * @brief Supervisor contracts using controlled child processes.
 *
 * Scenarios exercise launch, byte transport, descriptor inheritance, signals,
 * cutoff, and cleanup. Each runs in a separate Meson process; harness cleanup
 * retains responsibility for every unreaped helper child.
 */
#include "../support/check.h"
#include "../support/fds.h"
#include "../support/supervisor.h"
#include "diagnostic.h"
#include "exec/exec.h"
#include "platform/posix.h"
#include "text/text.h"
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include <sys/wait.h>
#include <unistd.h>

static RillPlatform platform;
static RillEnvironment environment;
static RillExec *supervisor;
static const char *child;

static void cleanup() {
  if (!supervisor_cleanup(supervisor))
    _Exit(99);
  rill_platform_env_clear(&environment);
  rill_platform_clear(&platform);
}

static void finish(RillJob *job) {
  int64_t deadline = rill_platform_now() + 15000;
  while (rill_exec_state(job) != RILL_JOB_COMPLETED) {
    CHECK(rill_platform_now() < deadline);
    (void)rill_exec_poll(supervisor, 10, -1);
    (void)rill_platform_signals(&platform);
  }
}

static RillJob *launch(const char *mode) {
  RillBytes args[] = {{child, strlen(child)}, {mode, strlen(mode)}};
  RillExecStage stage = {.argv = args, .argc = 2};
  RillExecSpec spec = {.stages = &stage,
                       .count = 1,
                       .environment = &environment,
                       .capture = true,
                       .capture_limit = 8'388'608};
  RillDiagnostic error = {};
  RillJob *job = rill_exec_launch(supervisor, &spec, &error);
  CHECK(job);
  return job;
}

static void streaming() {
  static constexpr size_t size = 2'097'152;
  unsigned char *input = malloc(size);
  CHECK(input);
  for (size_t i = 0; i < size; ++i)
    input[i] = (unsigned char)(i % 256);
  RillBytes args[] = {{child, strlen(child)}, {"flood", 5}};
  RillExecStage stage = {.argv = args, .argc = 2};
  RillExecSpec spec = {.stages = &stage,
                       .count = 1,
                       .environment = &environment,
                       .capture = true,
                       .feed = true,
                       .input = {(const char *)input, size},
                       .capture_limit = 4 * size};
  int aliases[] = {fcntl(0, F_DUPFD_CLOEXEC, 3), fcntl(1, F_DUPFD_CLOEXEC, 3)};
  CHECK(aliases[0] >= 0 && aliases[1] >= 0);
  int flags[] = {fcntl(aliases[0], F_GETFL), fcntl(aliases[1], F_GETFL)};
  RillDiagnostic error = {};
  RillJob *job = rill_exec_launch(supervisor, &spec, &error);
  CHECK(job);
  memset(input, 0xff, size);
  free(input); // Launch must own its feed bytes before returning.
  int64_t deadline = rill_platform_now() + 15000;
  while (rill_exec_state(job) != RILL_JOB_COMPLETED) {
    CHECK(rill_platform_now() < deadline);
    for (size_t i = 0; i < 2; ++i)
      CHECK(fcntl(aliases[i], F_GETFL) == flags[i]);
    (void)rill_exec_poll(supervisor, 10, -1);
    (void)rill_platform_signals(&platform);
  }
  CHECK(rill_exec_result(job) == 0);
  for (int stream = 1; stream <= 2; ++stream) {
    const RillBuffer *output = rill_exec_output(job, stream);
    if (output->size != size)
      (void)fprintf(stderr, "stream %d: got %zu bytes, expected %zu\n", stream,
                    output->size, size);
    CHECK(output->size == size);
    for (size_t i = 0; i < size; ++i)
      CHECK((unsigned char)output->data[i] == i % 256);
  }
  for (size_t i = 0; i < 2; ++i) {
    CHECK(fcntl(aliases[i], F_GETFL) == flags[i]);
    CHECK(fcntl(aliases[i], F_GETFD) == FD_CLOEXEC);
    CHECK(close(aliases[i]) == 0);
  }
  // Empty feed must still close its pipe; the child waits for EOF.
  args[1] = (RillBytes){"echo", 4};
  spec.input = (RillBytes){};
  job = rill_exec_launch(supervisor, &spec, &error);
  CHECK(job);
  finish(job);
  CHECK(rill_exec_result(job) == 0);
  CHECK(rill_exec_output(job, 1)->size == 0 &&
        rill_exec_output(job, 2)->size == 0);

  spec.capture = false;
  spec.streaming = true;
  job = rill_exec_launch(supervisor, &spec, &error);
  CHECK(job);
  // Consuming an untouched queue must not pass null storage to memmove.
  rill_exec_consume(job, 0);
  CHECK(rill_exec_feed(job, (RillBytes){"a\0bc", 4}));
  rill_exec_feed_end(job);
  finish(job);
  CHECK(rill_exec_result(job) == 0);
  const RillBuffer *output = rill_exec_output(job, 1);
  CHECK(output->size == 4 && !memcmp(output->data, "a\0bc\0", 5));
  rill_exec_consume(job, 1);
  CHECK(output->size == 3 && !memcmp(output->data, "\0bc\0", 4));
  rill_exec_consume(job, 3);
  CHECK(!output->size && !*output->data);
  rill_exec_consume(job, 0);
}

static void child_state() {
  int baseline = descriptor_count(3);
  CHECK(baseline >= 0);
  struct rlimit limit = {};
  CHECK(getrlimit(RLIMIT_NOFILE, &limit) == 0 && limit.rlim_cur > 3);
  // Descriptor numbers must be below the inherited soft limit.
  int minimum = limit.rlim_cur > 256 ? 256 : (int)limit.rlim_cur - 1;
  int high = fcntl(platform.signals[0], F_DUPFD_CLOEXEC, minimum);
  CHECK(high >= minimum);
  CHECK(descriptor_count(3) == baseline + 1);
  RillJob *job = launch("fds");
  finish(job);
  CHECK(rill_exec_result(job) == 0);
  CHECK(close(high) == 0);
  job = launch("signals");
  finish(job);
  CHECK(rill_exec_result(job) == 0);
  rill_exec_cancel(job);
  CHECK(!rill_exec_cancelled(job) && rill_exec_result(job) == 0);

  // External SIGCONT must update the job state even without our resume API.
  // Keep the child alive on a borrowed input pipe until the state is observed.
  int input[2];
  CHECK(rill_platform_pipe(input, false));
  int saved = fcntl(0, F_DUPFD_CLOEXEC, 3);
  CHECK(saved >= 0 && dup2(input[0], 0) == 0);
  RillBytes args[] = {{child, strlen(child)}, {"stop-echo", 9}};
  RillExecStage stage = {.argv = args, .argc = 2};
  RillExecSpec spec = {
      .stages = &stage, .count = 1, .environment = &environment};
  RillDiagnostic error = {};
  job = rill_exec_launch(supervisor, &spec, &error);
  CHECK(dup2(saved, 0) == 0 && close(saved) == 0);
  rill_platform_close(&input[0]);
  CHECK(job);
  int64_t deadline = rill_platform_now() + 5000;
  while (rill_exec_state(job) != RILL_JOB_STOPPED) {
    CHECK(rill_platform_now() < deadline);
    (void)rill_exec_poll(supervisor, 10, -1);
  }
  size_t count = {};
  const RillExecStatus *status = rill_exec_status(job, &count);
  CHECK(count == 1 && kill(status[0].pid, SIGCONT) == 0);
  while (rill_exec_state(job) != RILL_JOB_RUNNING) {
    CHECK(rill_platform_now() < deadline);
    (void)rill_exec_poll(supervisor, 10, -1);
  }
  CHECK(!status[0].done && !status[0].stopped);
  CHECK(write(input[1], "x", 1) == 1);
  rill_platform_close(&input[1]);
  finish(job);
  CHECK(rill_exec_result(job) == 0);
}

static void cancellation(const char *mode) {
  RillJob *job = launch(mode);
  int64_t deadline = rill_platform_now() + 5000;
  while (rill_exec_output(job, 1)->size < 6) {
    CHECK(rill_platform_now() < deadline);
    (void)rill_exec_poll(supervisor, 10, -1);
  }
  CHECK(!memcmp(rill_exec_output(job, 1)->data, "ready\n", 6));
  rill_exec_cancel(job);
  finish(job);
  CHECK(rill_exec_cancelled(job) && rill_exec_result(job) == 130);
  size_t count = {};
  const RillExecStatus *status = rill_exec_status(job, &count);
  CHECK(count == 1 && status[0].done);
  bool escalate = !strcmp(mode, "ignore-term");
  CHECK(status[0].signaled == escalate);
  CHECK(status[0].status == (escalate ? SIGKILL : 0));
  // No live child remains to wake poll: an infinite request must still yield
  // to the session loop. Meson's process timeout bounds a regression here.
  (void)rill_platform_signals(&platform);
  (void)rill_exec_poll(supervisor, -1, -1);
}

static void limits() {
  RillBytes args[] = {{child, strlen(child)}, {"flood", 5}};
  RillExecStage stage = {.argv = args, .argc = 2};
  RillExecSpec spec = {.stages = &stage,
                       .count = 1,
                       .environment = &environment,
                       .capture = true,
                       .capture_limit = 128};
  RillDiagnostic error = {};
  spec.count = 0;
  CHECK(!rill_exec_launch(supervisor, &spec, &error));
  CHECK(error.kind == RILL_LIMIT);
  spec.count = RILL_EXEC_MAX_STAGES + 1;
  CHECK(!rill_exec_launch(supervisor, &spec, &error));
  CHECK(error.kind == RILL_LIMIT);
  spec.count = 1;
  stage.argc = 0;
  CHECK(!rill_exec_launch(supervisor, &spec, &error));
  CHECK(error.kind == RILL_LAUNCH && error.code == EINVAL);
  stage.argc = RILL_EXEC_MAX_ARGUMENTS + 1;
  CHECK(!rill_exec_launch(supervisor, &spec, &error));
  CHECK(error.kind == RILL_LAUNCH && error.code == EINVAL);
  stage.argc = 2;
  RillJob *job = rill_exec_launch(supervisor, &spec, &error);
  CHECK(job);
  finish(job);
  CHECK(rill_exec_error(job).kind == RILL_LIMIT && rill_exec_cancelled(job));
  CHECK(rill_exec_output(job, 1)->size + rill_exec_output(job, 2)->size <= 128);
  args[0] = (RillBytes){"bad\0path", 8};
  CHECK(!rill_exec_launch(supervisor, &spec, &error));
  CHECK(error.kind == RILL_LAUNCH && error.code == EINVAL);
  CHECK(error.has_argument && error.argument == 0 && error.stage == 0);
  rill_exec_acknowledge(job);
  size_t id = rill_exec_id(job);
  rill_exec_unmark(supervisor);
  rill_exec_retain(supervisor, id);
  rill_exec_prune(supervisor);
  CHECK(rill_exec_find(supervisor, id) == job);
  rill_exec_unmark(supervisor);
  rill_exec_prune(supervisor);
  CHECK(!rill_exec_first(supervisor) && !rill_exec_find(supervisor, id));
}

static void exhaustion() {
  int baseline = descriptor_count(0);
  CHECK(baseline >= 0);
  struct rlimit original = {};
  CHECK(getrlimit(RLIMIT_NOFILE, &original) == 0);
  struct rlimit low = original;
  low.rlim_cur = (rlim_t)baseline + 10;
  CHECK(setrlimit(RLIMIT_NOFILE, &low) == 0);
  RillBytes args[] = {{child, strlen(child)}, {"signals", 7}};
  RillExecStage stages[8];
  for (size_t i = 0; i < 8; ++i)
    stages[i] = (RillExecStage){.argv = args, .argc = 2};
  RillExecSpec spec = {
      .stages = stages, .count = 8, .environment = &environment};
  RillDiagnostic error = {};
  for (size_t i = 0; i < 2; ++i) {
    CHECK(!rill_exec_launch(supervisor, &spec, &error));
    CHECK(error.code == EMFILE);
    CHECK(descriptor_count(0) == baseline);
  }
  CHECK(setrlimit(RLIMIT_NOFILE, &original) == 0);
}

static void statuses() {
  RillBytes left[] = {{child, strlen(child)}, {"exit", 4}, {"7", 1}};
  RillBytes right[] = {{child, strlen(child)}, {"exit", 4}, {"0", 1}};
  RillExecStage stages[] = {{.argv = left, .argc = 3},
                            {.argv = right, .argc = 3}};
  RillExecSpec spec = {
      .stages = stages, .count = 2, .environment = &environment};
  RillDiagnostic error = {};
  RillJob *job = rill_exec_launch(supervisor, &spec, &error);
  CHECK(job);
  finish(job);
  size_t count = {};
  const RillExecStatus *status = rill_exec_status(job, &count);
  CHECK(count == 2 && status[0].done && status[1].done);
  CHECK(!status[0].signaled && !status[1].signaled);
  CHECK(status[0].status == 7 && status[1].status == 0);
  CHECK(rill_exec_result(job) == 7 && rill_exec_error(job).kind == RILL_OK);
}

int main(int argc, char **argv) {
  CHECK(argc == 3);
  child = argv[1];
  CHECK(rill_platform_init(&platform, false));
  CHECK(atexit(cleanup) == 0);
  extern char **environ;
  CHECK(rill_platform_env_init(&environment, environ));
  supervisor = rill_exec_new(&platform);
  CHECK(supervisor);
  if (!strcmp(argv[2], "streaming"))
    streaming();
  else if (!strcmp(argv[2], "child-state"))
    child_state();
  else if (!strcmp(argv[2], "term-zero") || !strcmp(argv[2], "ignore-term"))
    cancellation(argv[2]);
  else if (!strcmp(argv[2], "cleanup")) {
    CHECK(launch("ignore-term"));
    // Meson distinguishes this deliberate exit from assertion/cleanup failure.
    exit(42);
  } else if (!strcmp(argv[2], "limits"))
    limits();
  else if (!strcmp(argv[2], "exhaustion"))
    exhaustion();
  else {
    CHECK(!strcmp(argv[2], "statuses"));
    statuses();
  }
  CHECK(!rill_exec_outstanding(supervisor, false));
  errno = 0;
  CHECK(waitpid(-1, nullptr, WNOHANG) == -1 && errno == ECHILD);
}
