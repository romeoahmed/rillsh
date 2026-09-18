/**
 * @file
 * @brief Allocation and launch failure recovery using production code.
 *
 * Meson compiles selected subject files with test-only allocation/POSIX
 * aliases. These wrappers fail controlled operations while the harness keeps
 * real cleanup services. Scenarios verify errors, retained state, and child
 * reaping.
 */
#include "../helpers/fds.h"
#include "../helpers/supervisor.h"
#include "diagnostic.h"
#include "exec/exec.h"
#include "library/bundle.h"
#include "library/library.h"
#include "library/stream.h"
#include "platform/posix.h"
#include "runtime/runtime.h"
#include "source.h"
#include "syntax/syntax.h"
#include "test.h"
#include "text/text.h"
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
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
static bool fail_once, allocation_failed;
static int cwd_calls, fork_calls, group_calls;
static int fail_fork, fail_group, fail_cwd;
static bool fail_rollback, kill_before_exec, stop_before_group_failure;

static bool allocation_fails() {
  if (allocation_budget < 0)
    return false;
  if (allocation_budget-- > 0)
    return false;
  allocation_budget = fail_once ? -1 : 0;
  allocation_failed = true;
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
      siginfo_t info = {};
      CHECK(waitid(P_PID, (id_t)pid, &info, WSTOPPED | WNOWAIT) == 0);
      CHECK(info.si_code == CLD_STOPPED);
    }
    errno = EPERM;
    return -1;
  }
  int result = setpgid(pid, group);
  if (result == 0 && kill_before_exec) {
    CHECK(kill(pid, SIGKILL) == 0);
    siginfo_t info = {};
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
  RillEnvironment env = {};
  RillDiagnostic error = {};
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
      struct stat actual = {}, target = {};
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
  for (int budget = 0; !completed; ++budget) {
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
  RillPlatform platform = {};
  CHECK(rill_platform_init(&platform, false));
  RillExec *exec = rill_exec_new(&platform);
  CHECK(exec);
  active_exec = exec;
  RillEnvironment env = {};
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
    RillDiagnostic error = {};
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
      size_t count = {};
      const RillExecStatus *status = rill_exec_status(job, &count);
      CHECK(count == 1 && status[0].done && status[0].signaled);
      CHECK(status[0].status == SIGKILL);
      // Channel EOF also occurs when a child is killed before exec.
      CHECK(rill_exec_launched(job));
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
  for (int budget = 0; !completed; ++budget) {
    allocation_budget = budget;
    RillDiagnostic error = {};
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
  active_exec = nullptr;
  rill_exec_free(exec);
  rill_platform_env_clear(&env);
  rill_platform_clear(&platform);
}
static void check_text_and_syntax() {
  RillBuffer buffer = {};
  CHECK(rill_text_append(&buffer, "kept", 4));
  CHECK(buffer.capacity < INT_MAX);
  allocation_budget = 0;
  CHECK(!rill_text_format(&buffer, "%*s", (int)buffer.capacity, ""));
  allocation_budget = -1;
  CHECK(buffer.size == 4);
  CHECK(!strcmp(buffer.data, "kept"));
  size_t size = buffer.capacity;
  char *controls = calloc(size, 1);
  CHECK(controls);
  char *storage = buffer.data;
  allocation_budget = 0;
  CHECK(!rill_text_escape(&buffer, (RillBytes){controls, size}));
  allocation_budget = -1;
  free(controls);
  CHECK(buffer.data == storage && buffer.size == 4);
  CHECK(!strcmp(buffer.data, "kept"));
  CHECK(!rill_text_append(&buffer, nullptr, SIZE_MAX));
  CHECK(errno == ENOMEM && buffer.data == storage && buffer.size == 4);
  rill_text_clear(&buffer);

  RillBuffer many = {};
  for (size_t i = 0; i < 1024; ++i)
    CHECK(rill_text_append(&many, "();", 3));
  const char *cases[] = {many.data, "\"\\u{1f642}\"", "1_234.5e-2",
                         "let x = [1, 'two']; x[0]",
                         "job { ^cat 'argument' | ^cat > result 2>&1 }"};
  for (size_t i = 0; i < sizeof(cases) / sizeof(*cases); ++i) {
    RillSource source = {};
    CHECK(rill_source_init(&source, "fault", cases[i], strlen(cases[i])) ==
          RILL_OK);
    bool complete = false;
    for (int budget = 0; !complete; ++budget) {
      allocation_budget = budget;
      RillSyntax syntax = rill_syntax_parse(&source);
      allocation_budget = -1;
      complete = syntax.state == RILL_COMPLETE;
      if (!complete)
        CHECK(syntax.state == RILL_INVALID &&
              syntax.diagnostic.kind == RILL_MEMORY);
      rill_syntax_clear(&syntax);
    }
    rill_source_clear(&source);
  }
  rill_text_clear(&many);
  bool complete = false;
  for (int budget = 0; !complete; ++budget) {
    RillSource source = {};
    allocation_budget = budget;
    RillError error = rill_source_init(&source, "input", "valid", 5);
    allocation_budget = -1;
    complete = error == RILL_OK;
    if (!complete)
      CHECK(error == RILL_MEMORY && !source.name && !source.bytes.data);
    rill_source_clear(&source);
  }
}

static void check_equality_allocations() {
  RillHeap heap = {};
  RillValue values[2] = {};
  RillRoot root = {};
  rill_runtime_root(&heap, &root, values, 2);
  for (size_t j = 0; j < 2; ++j)
    values[j] =
        rill_runtime_object(&heap, RILL_V_LIST, values + j, 1, nullptr, 0, 0);
  bool equal = false;
  allocation_budget = 0;
  RillError status = rill_runtime_equal(values[0], values[1], &equal);
  allocation_budget = -1;
  CHECK(status == RILL_OK ? equal : status == RILL_MEMORY);
  for (size_t i = 0; i < 128; ++i)
    for (size_t j = 0; j < 2; ++j) {
      values[j] =
          rill_runtime_object(&heap, RILL_V_LIST, values + j, 1, nullptr, 0, 0);
      CHECK(values[j].kind == RILL_V_LIST);
    }
  bool complete = false;
  for (int budget = 0; !complete; ++budget) {
    allocation_budget = budget;
    status = rill_runtime_equal(values[0], values[1], &equal);
    allocation_budget = -1;
    complete = status == RILL_OK;
    CHECK(complete ? equal : status == RILL_MEMORY);
  }
  rill_runtime_unroot(&heap, &root);
  rill_runtime_heap_clear(&heap);
}
static void check_runtime() {
  const char *program =
      "enum Box {Empty,Full {value}}; fn make(x)=>Box.Full({value:x}); let "
      "saved=make([1,2]); match saved {Box.Full {value:[x,..tail]}=>if "
      "tail==[2] then x else 0,_=>0}";
  RillSource source = {};
  CHECK(rill_source_init(&source, "fault-runtime", program, strlen(program)) ==
        RILL_OK);
  bool complete = false;
  for (int budget = 0; !complete; ++budget) {
    RillEval *eval = rill_runtime_new(nullptr, 0);
    CHECK(eval);
    CHECK(rill_runtime_define(
        eval, "saved", (RillValue){.kind = RILL_V_INT, .as.integer = 7}));
    RillSource initial = {};
    CHECK(rill_source_init(&initial, "initial", "let stable=12", 13) ==
          RILL_OK);
    RillSyntax base = rill_syntax_parse(&initial);
    CHECK(base.state == RILL_COMPLETE);
    rill_runtime_heap(eval)->stress = true;
    rill_runtime_begin(eval, &base);
    RillEvalEvent initialized = {};
    do {
      initialized = rill_runtime_step(eval);
    } while (initialized.state == RILL_EVAL_YIELD);
    CHECK(initialized.state == RILL_EVAL_DONE);
    rill_syntax_clear(&base);
    rill_source_clear(&initial);
    RillSyntax syntax = rill_syntax_parse(&source);
    CHECK(syntax.state == RILL_COMPLETE);
    allocation_budget = budget;
    rill_runtime_begin(eval, &syntax);
    RillEvalEvent event = {};
    do {
      event = rill_runtime_step(eval);
    } while (event.state == RILL_EVAL_YIELD);
    complete = event.state == RILL_EVAL_DONE;
    if (!complete) {
      CHECK(event.state == RILL_EVAL_ERROR &&
            event.diagnostic.kind == RILL_MEMORY);
      RillValue prior = {};
      CHECK(rill_runtime_lookup(eval, "saved", &prior));
      CHECK(prior.kind == RILL_V_INT && prior.as.integer == 7);
      CHECK(rill_runtime_lookup(eval, "stable", &prior));
      CHECK(prior.kind == RILL_V_INT && prior.as.integer == 12);
    } else
      CHECK(event.value.kind == RILL_V_INT && event.value.as.integer == 1);
    allocation_budget = -1;
    rill_runtime_free(eval);
    rill_syntax_clear(&syntax);
  }
  rill_source_clear(&source);
  RillEval *eval = rill_runtime_new(nullptr, 0);
  CHECK(eval);
  RillValue cached = rill_runtime_object(rill_runtime_heap(eval), RILL_V_STRING,
                                         nullptr, 0, "cached", 6, 0);
  CHECK(cached.kind == RILL_V_STRING);
  CHECK(rill_runtime_retain_module(eval, cached));
  // Retention may reuse capacity; either outcome must preserve earlier values.
  allocation_budget = 0;
  (void)rill_runtime_retain_module(eval, (RillValue){});
  allocation_budget = -1;
  rill_runtime_abort(eval);
  CHECK(cached.as.object->bytes.size == 6);
  CHECK(!memcmp(cached.as.object->bytes.data, "cached", 6));
  rill_runtime_free(eval);
  complete = false;
  for (int budget = 0; !complete; ++budget) {
    RillHeap heap = {.stress = true};
    RillValue items[200] = {}, retained = {};
    RillRoot ir = {}, br = {};
    rill_runtime_root(&heap, &ir, items, 200);
    rill_runtime_root(&heap, &br, &retained, 1);
    for (size_t i = 0; i < 200; ++i) {
      items[i] =
          rill_runtime_object(&heap, RILL_V_STRING, nullptr, 0, "item", 4, 0);
      CHECK(items[i].kind == RILL_V_STRING);
    }
    allocation_budget = budget;
    retained = rill_runtime_budget(&heap, SIZE_MAX);
    RillError status = retained.kind == RILL_V_UNIT ? RILL_MEMORY : RILL_OK;
    for (size_t i = 0; i < 200 && !status; ++i)
      status = rill_runtime_charge(&heap, retained, items[i]);
    complete = status == RILL_OK;
    CHECK(complete || status == RILL_MEMORY);
    allocation_budget = -1;
    rill_runtime_unroot(&heap, &br);
    rill_runtime_unroot(&heap, &ir);
    rill_runtime_heap_clear(&heap);
  }
}
static RillSyntax parse(const char *text) {
  [[gnu::cleanup(rill_source_clear)]] RillSource source = {};
  CHECK(rill_source_init(&source, "fault", text, strlen(text)) == RILL_OK);
  RillSyntax syntax = rill_syntax_parse(&source);
  CHECK(syntax.state == RILL_COMPLETE);
  return syntax;
}
static void check_modules() {
  bool complete = false;
  for (int budget = 0; !complete; ++budget) {
    RillEval *eval = rill_runtime_new(nullptr, 0);
    CHECK(eval);
    CHECK(rill_runtime_define(
        eval, "stable", (RillValue){.kind = RILL_V_INT, .as.integer = 7}));
    rill_runtime_prelude(eval);
    rill_runtime_heap(eval)->stress = true;
    [[gnu::cleanup(rill_syntax_clear)]] RillSyntax entry =
        parse("import 'module' as m; m.identity(m.saved)");
    rill_runtime_begin(eval, &entry);
    RillEvalEvent request = {};
    do {
      request = rill_runtime_step(eval);
    } while (request.state == RILL_EVAL_YIELD);
    CHECK(request.state == RILL_EVAL_IMPORT);
    [[gnu::cleanup(rill_syntax_clear)]] RillSyntax module =
        parse("export let saved='kept'; export fn identity(x)=>x");
    fail_once = true;
    allocation_failed = false;
    allocation_budget = budget;
    rill_runtime_module(eval, &module);
    RillEvalEvent event = {};
    do {
      event = rill_runtime_step(eval);
    } while (event.state == RILL_EVAL_YIELD);
    allocation_budget = -1;
    fail_once = false;
    complete = !allocation_failed;
    if (complete) {
      CHECK(event.state == RILL_EVAL_MODULE &&
            event.value.kind == RILL_V_RECORD);
      CHECK(rill_runtime_retain_module(eval, event.value));
      rill_runtime_resume(eval, event.value, (RillDiagnostic){});
      do {
        event = rill_runtime_step(eval);
      } while (event.state == RILL_EVAL_YIELD);
      CHECK(event.state == RILL_EVAL_DONE && event.value.kind == RILL_V_STRING);
      CHECK(!strcmp(event.value.as.object->bytes.data, "kept"));
    } else {
      if (event.state != RILL_EVAL_ERROR)
        (void)fprintf(stderr, "module fault budget %d\n", budget);
      CHECK(event.state == RILL_EVAL_ERROR &&
            event.diagnostic.kind == RILL_MEMORY);
      rill_runtime_abort(eval);
      RillValue prior = {};
      CHECK(rill_runtime_lookup(eval, "stable", &prior) &&
            prior.as.integer == 7);
      CHECK(!rill_runtime_lookup(eval, "m", &prior));
    }
    CHECK(!module.allocated && !module.source.data);
    rill_runtime_free(eval);
  }
}
static RillEvalEvent evaluate_library(RillLibrary *library,
                                      RillSyntax *syntax) {
  rill_runtime_begin(library->eval, syntax);
  RillNativePending pending = {};
  for (;;) {
    if (pending.job) {
      (void)rill_exec_poll(library->exec, 0, -1);
      if (!rill_library_progress(library, &pending))
        continue;
    }
    if (!rill_stream_progress(library))
      continue;
    RillEvalEvent event = rill_runtime_step(library->eval);
    if (event.state == RILL_EVAL_CLEANUP) {
      rill_stream_unwind(library, event.native, event.value);
      continue;
    }
    if (event.state == RILL_EVAL_CALLBACK) {
      rill_stream_callback(library, event.value);
      continue;
    }
    if (event.state == RILL_EVAL_YIELD)
      continue;
    if (event.state != RILL_EVAL_NATIVE)
      return event;
    (void)rill_library_call(library, &pending, event);
  }
}
static void check_library(const char *child) {
  RillPlatform platform = {};
  CHECK(rill_platform_init(&platform, false));
  RillExec *exec = rill_exec_new(&platform);
  CHECK(exec);
  active_exec = exec;
  RillEnvironment environment = {};
  CHECK(rill_platform_env_init(&environment, nullptr));
  RillBytes arguments[] = {{child, strlen(child)}, {"exit", 4}, {"0", 1}};
  RillExecStage stage = {.argv = arguments, .argc = 3};
  RillExecSpec spec = {.stages = &stage,
                       .count = 1,
                       .environment = &environment,
                       .background = true};
  RillDiagnostic diagnostic = {};
  RillJob *job = rill_exec_launch(exec, &spec, &diagnostic);
  CHECK(job);
  rill_exec_cancel(job);
  finish(exec, &platform, job);
  CHECK(rill_exec_cancelled(job));
  size_t native_count = {};
  const RillNative *natives = rill_library_natives(&native_count);
  [[gnu::cleanup(rill_text_clear)]] RillBuffer nested = {}, deep_json = {};
  for (size_t i = 0; i < 40; ++i)
    CHECK(rill_text_append(&nested, "{\"x\":", 5));
  CHECK(rill_text_append(&nested, "0", 1));
  for (size_t i = 0; i < 40; ++i)
    CHECK(rill_text_append(&nested, "}", 1));
  CHECK(rill_text_format(&deep_json,
                         "from_json(to_json(from_json('%s')))==from_json('%s')",
                         nested.data, nested.data));
  const struct {
    const char *name, *source;
    bool exports;
  } cases[] = {
      {"arguments", "args()==[bytes([97]),bytes([]),bytes([255])]", false},
      {"environment overrides",
       "let base=with_env({KEEP:'old',KEY:'first'},"
       "pipe(command('printf',['text']),job {^cat}));"
       "let plan=with_env({KEY:'value',NEW:'added'},base); true",
       false},
      {"byte conversion",
       "concat(encode_utf8(text(42)),bytes([33]))==bytes([52,50,33])", false},
      {"exports", "export let first='one'; export let second='two'; true",
       true},
      {"cancelled report",
       "match wait(handle) {JobReport {completion: Completion.Cancelled "
       "{reason},..}=>reason=='cancelled',_=>false}",
       false},
      {"error conversion",
       "match attempt(fn()=>1+true) {Result.Err {error:Error {kind,..}}"
       "=>kind=='TypeError',_=>false}",
       false},
      {"record rest", "match {a:1,b:[2,3],c:4} {{a,..rest}=>rest.b==[2,3]}",
       false},
      {"nested captures and constants",
       "let factory=fn(x)=>fn(y)=>fn(z)=>[x,y,z,'same','same'];"
       "let saved=factory(1)(2); saved(3)==[1,2,3,'same','same']",
       false},
      {"record update",
       "let base={a:1,b:2,c:3,d:4,e:5,f:6,g:7,h:8,i:9,j:10,k:11,l:12,"
       "m:13,n:14,o:15,p:16,q:17}; let copy=base with {a:[42],q:0};"
       "copy.a==[42] and copy.q==0 and base.a==1 and base.q==17",
       false},
      {"sort",
       "sort_by(fn(x)=>x.k,[{k:2,v:'a'},{k:1,v:'b'},{k:2,v:'c'}])[2].v=='c'",
       false},
      {"stream callbacks",
       "do {let value=chunks(encode_utf8(\"a\\nb\\n\")) |> lines |> "
       "map(fn(x)=>x+'!') |> collect; value==['a!','b!']}",
       false},
      {"filled stream collection",
       "do {let value=chunks(encode_utf8(\"a\\nb\\nc\\n\")) |> lines |> "
       "map(fn(x)=>{item:x}) |> collect_with({max_items:3});"
       "value==[{item:'a'},{item:'b'},{item:'c'}]}",
       false},
      {"nested stream callbacks",
       "do {let value=chunks(encode_utf8('x')) |> map(fn(x)=>chunks(x) |> "
       "collect_bytes) |> collect; value==[encode_utf8('x')]}",
       false},
      {"resource checkpoint",
       "do {let kept=chunks(encode_utf8('x')); "
       "let result=attempt(fn()=>do {let lost=chunks(encode_utf8('y')); "
       "raise(error('Example','failed'))}); "
       "match result {Result.Err {error}=>error.kind=='Example' and "
       "collect_bytes(kept)==encode_utf8('x'),_=>false}}",
       false},
      {"JSON conversion",
       "from_json(to_json({a:[1,1.0,null,true,'x']}))=={a:[1,1.0,null,true,'x']"
       "}",
       false},
      {"deep JSON conversion", deep_json.data, false},
      {"Unicode scalars", "scalars('\u754ca')==['\u754c','a']", false}};
  for (size_t i = 0; i < sizeof(cases) / sizeof(*cases); ++i) {
    bool complete = false;
    for (int budget = 0; !complete; ++budget) {
      RillEval *eval = rill_runtime_new(natives, native_count);
      CHECK(eval);
      char *args[] = {(char[]){"a"}, (char[]){""}, (char[]){"\xff"}};
      RillLibrary library = {.eval = eval,
                             .exec = exec,
                             .environment = &environment,
                             .arguments = args,
                             .argument_count = 3};
      [[gnu::cleanup(rill_syntax_clear)]] RillSyntax prelude =
          parse(rill_library_source("std:prelude"));
      CHECK(evaluate_library(&library, &prelude).state == RILL_EVAL_DONE);
      rill_runtime_prelude(eval);
      CHECK(rill_runtime_define(
          eval, "handle",
          (RillValue){.kind = RILL_V_JOB,
                      .as.integer = (int64_t)rill_exec_id(job)}));
      [[gnu::cleanup(rill_syntax_clear)]] RillSyntax syntax =
          parse(cases[i].source);
      rill_runtime_heap(eval)->stress = true;
      // A single failed allocation followed by recovery exposes swallowed
      // errors that a permanently exhausted allocator would conceal.
      fail_once = true;
      allocation_failed = false;
      allocation_budget = budget;
      RillEvalEvent event = evaluate_library(&library, &syntax);
      if (event.state == RILL_EVAL_DONE) {
        RillValue exports = rill_runtime_exports(eval);
        if (exports.kind == RILL_V_UNIT)
          event = rill_runtime_step(eval);
        else if (cases[i].exports) {
          RillValue first = {}, second = {};
          CHECK(rill_runtime_field(exports, (RillBytes){"first", 5}, &first));
          CHECK(rill_runtime_field(exports, (RillBytes){"second", 6}, &second));
          CHECK(first.kind == RILL_V_STRING && second.kind == RILL_V_STRING);
          CHECK(!strcmp(first.as.object->bytes.data, "one"));
          CHECK(!strcmp(second.as.object->bytes.data, "two"));
        }
      }
      allocation_budget = -1;
      fail_once = false;
      complete = !allocation_failed;
      if (complete) {
        if (event.state != RILL_EVAL_DONE || event.value.kind != RILL_V_BOOL ||
            !event.value.as.integer)
          (void)fprintf(
              stderr, "library case %s: event %d, value %d, error %s\n",
              cases[i].name, (int)event.state, (int)event.value.kind,
              event.diagnostic.message ? event.diagnostic.message : "none");
        CHECK(event.state == RILL_EVAL_DONE &&
              event.value.kind == RILL_V_BOOL && event.value.as.integer);
      } else {
        if (event.state != RILL_EVAL_ERROR ||
            event.diagnostic.kind != RILL_MEMORY)
          (void)fprintf(stderr, "library fault %s budget %d\n", cases[i].name,
                        budget);
        CHECK(event.state == RILL_EVAL_ERROR &&
              event.diagnostic.kind == RILL_MEMORY);
      }
      rill_stream_cancel(&library);
      CHECK(!rill_stream_live(&library));
      rill_stream_clear(&library);
      rill_runtime_free(eval);
    }
  }
  active_exec = nullptr;
  rill_exec_free(exec);
  rill_platform_env_clear(&environment);
  rill_platform_clear(&platform);
}
static void check_escape_without_allocation() {
  RillHeap heap = {.stress = true};
  RillValue values[2] = {};
  RillRoot root = {};
  rill_runtime_root(&heap, &root, values, 2);
  values[0] =
      rill_runtime_object(&heap, RILL_V_STREAM, nullptr, 0, nullptr, 0, 0);
  values[1] =
      rill_runtime_object(&heap, RILL_V_CELL, nullptr, 1, nullptr, 0, 0);
  CHECK(values[0].kind == RILL_V_STREAM && values[1].kind == RILL_V_CELL);
  allocation_budget = 0;
  allocation_failed = false;
  CHECK(rill_runtime_persistent(&heap, values[1]) == RILL_OK);
  values[1].as.object->values[0] = values[0];
  CHECK(rill_runtime_persistent(&heap, values[1]) == RILL_STREAM_ESCAPE);
  values[1].as.object->values[0] = values[1];
  CHECK(rill_runtime_persistent(&heap, values[1]) == RILL_OK);
  CHECK(!allocation_failed);
  allocation_budget = -1;
  rill_runtime_unroot(&heap, &root);
  rill_runtime_heap_clear(&heap);
}
int main(int argc, char **argv) {
  CHECK(argc == 3);
  if (!strcmp(argv[2], "memory")) {
    check_text_and_syntax();
    check_runtime();
    check_modules();
    check_equality_allocations();
    check_escape_without_allocation();
    return 0;
  }
  CHECK(atexit(cleanup_launch) == 0);
  if (!strcmp(argv[2], "library")) {
    check_library(argv[1]);
    return 0;
  }
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
