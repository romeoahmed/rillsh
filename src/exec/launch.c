/**
 * @file
 * @brief Prepare and register a pipeline before releasing its children.
 *
 * Copy launch inputs and open descriptor actions before fork. Children perform
 * only async-signal-safe setup and wait at a gate while the parent registers
 * the process group and hands off the terminal when required. Partial launch
 * failure retains every child identity until cancellation and reaping finish.
 */
#include "diagnostic.h"
#include "exec.h"
#include "platform/posix.h"
#include "private.h"
#include "text/text.h"
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <poll.h>
#include <signal.h>
#include <stdckdint.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>
#include <unistd.h>

static_assert(RILL_EXEC_MAX_STAGES <= _POSIX_PIPE_BUF);
static_assert(sizeof(LaunchError) <= _POSIX_PIPE_BUF);

typedef struct {
  char **argv, **paths;
  size_t argc, path_count;
  RillEnvironment env;
  int cwd;
  int streams[3];
  int pipe_input, pipe_output;
} Prepared;
typedef struct {
  int *fds;
  size_t count, capacity;
  Prepared *stages;
  size_t count_stages;
} Launch;
static char *copy_bytes(RillBytes b) {
  if (b.size == SIZE_MAX ||
      (b.size && (!b.data || memchr(b.data, 0, b.size)))) {
    errno = EINVAL;
    return nullptr;
  }
  char *s = malloc(b.size + 1);
  if (!s)
    return nullptr;
  if (b.size)
    memcpy(s, b.data, b.size);
  s[b.size] = 0;
  return s;
}
static int own(Launch *l, int fd) {
  if (fd < 0)
    return -1;
  if (l->count == l->capacity) {
    size_t cap = 32, bytes = {};
    if ((l->capacity && ckd_mul(&cap, l->capacity, 2)) ||
        ckd_mul(&bytes, cap, sizeof(*l->fds))) {
      (void)close(fd);
      errno = ENOMEM;
      return -1;
    }
    int *fds = realloc(l->fds, bytes);
    if (!fds) {
      int saved = errno;
      (void)close(fd);
      errno = saved;
      return -1;
    }
    l->fds = fds;
    l->capacity = cap;
  }
  l->fds[l->count++] = fd;
  return fd;
}
static bool pipe_owned(Launch *l, int fds[2]) {
  if (!rill_platform_pipe(fds, false))
    return false;
  if (own(l, fds[0]) < 0) {
    rill_platform_close(&fds[1]);
    return false;
  }
  return own(l, fds[1]) >= 0;
}
static void release(Launch *l) {
  for (size_t i = 0; i < l->count; ++i)
    rill_platform_close(&l->fds[i]);
  free(l->fds);
  for (size_t i = 0; l->stages && i < l->count_stages; ++i) {
    Prepared *s = &l->stages[i];
    for (size_t a = 0; s->argv && a < s->argc; ++a)
      free(s->argv[a]);
    free(s->argv);
    for (size_t a = 0; a < s->path_count; ++a)
      free(s->paths[a]);
    free(s->paths);
    rill_platform_env_clear(&s->env);
  }
  free(l->stages);
}
static int detach(Launch *l, int fd) {
  for (size_t i = 0; i < l->count; ++i)
    if (l->fds[i] == fd) {
      l->fds[i] = -1;
      break;
    }
  return fd;
}
static bool paths(Prepared *s, const char *path) {
  if (strchr(s->argv[0], '/')) {
    s->paths = calloc(1, sizeof(char *));
    if (!s->paths)
      return false;
    s->path_count = 1;
    s->paths[0] = strdup(s->argv[0]);
    return s->paths[0] != nullptr;
  }
  if (!path)
    path = "/usr/bin:/bin";
  size_t count = 1;
  for (const char *p = path; *p; ++p)
    if (*p == ':')
      ++count;
  s->paths = calloc(count, sizeof(char *));
  if (!s->paths)
    return false;
  s->path_count = count;
  for (size_t i = 0; i < count; ++i) {
    const char *end = strchr(path, ':');
    size_t n = end ? (size_t)(end - path) : strlen(path);
    [[gnu::cleanup(rill_text_clear)]] RillBuffer b = {};
    if ((n &&
         (!rill_text_append(&b, path, n) || !rill_text_append(&b, "/", 1))) ||
        !rill_text_append(&b, s->argv[0], strlen(s->argv[0])))
      return false;
    s->paths[i] = b.data;
    b = (RillBuffer){};
    path = end ? end + 1 : path + n;
  }
  return true;
}
[[noreturn]] static void child_error(int fd, size_t stage, int code) {
  LaunchError e = {.stage = stage, .code = code};
  const char *p = (const char *)&e;
  size_t n = sizeof(e);
  while (n) {
    ssize_t wrote = write(fd, p, n);
    if (wrote > 0) {
      p += wrote;
      n -= (size_t)wrote;
    } else if (wrote < 0 && errno == EINTR)
      continue;
    else
      break;
  }
  _exit(126);
}
// Post-fork code uses prepared storage and async-signal-safe calls only.
[[noreturn]] static void child(Launch *l, size_t index, int gate,
                               int error_fd) {
  Prepared *s = &l->stages[index];
  if (fchdir(s->cwd) < 0)
    child_error(error_fd, index, errno);
  for (int i = 0; i < 3; ++i) {
    if (s->streams[i] < 0) {
      (void)close(i);
      continue;
    }
    if (dup2(s->streams[i], i) < 0)
      child_error(error_fd, index, errno);
  }
  for (size_t i = 0; i < l->count; ++i)
    if (l->fds[i] != gate && l->fds[i] != error_fd)
      (void)close(l->fds[i]);
  char byte = {};
  ssize_t got = {};
  do {
    got = read(gate, &byte, 1);
  } while (got < 0 && errno == EINTR);
  if (got < 0)
    child_error(error_fd, index, errno);
  if (got == 0)
    _exit(126); // No permit: the parent abandoned launch before exec.
  (void)close(gate);
  if (!rill_platform_child_signals())
    child_error(error_fd, index, errno);
  int denied = 0;
  for (size_t i = 0; i < s->path_count; ++i) {
    execve(s->paths[i], s->argv, s->env.entries);
    if (errno == EACCES)
      denied = EACCES;
    else if (errno != ENOENT && errno != ENOTDIR)
      child_error(error_fd, index, errno);
  }
  child_error(error_fd, index, denied ? denied : ENOENT);
}
RillJob *rill_exec_launch(RillExec *e, const RillExecSpec *spec,
                          RillDiagnostic *error) {
  *error = (RillDiagnostic){};
  const size_t stage_count = spec->count;
  size_t stage = 0;
  if (!stage_count || stage_count > RILL_EXEC_MAX_STAGES ||
      e->count >= RILL_EXEC_MAX_JOBS || e->next_id == INT64_MAX) {
    *error = (RillDiagnostic){.kind = RILL_LIMIT,
                              .message = "job or pipeline limit exceeded"};
    return nullptr;
  }
  if (spec->streaming && spec->feed) {
    for (size_t i = 0; i < stage_count; ++i)
      for (size_t k = 0; k < spec->stages[i].redirect_count; ++k) {
        int target = spec->stages[i].redirects[k].target;
        if ((i == 0 && target == 0) || (i + 1 == stage_count && target == 1)) {
          *error = (RillDiagnostic){
              .kind = RILL_TYPE,
              .message = "through conflicts with endpoint redirection"};
          return nullptr;
        }
      }
  }
  RillJob *j = malloc(sizeof(*j));
  Launch l = {};
  if (!j)
    goto fail;
  *j = (RillJob){.errors = -1,
                 .input = -1,
                 .relay = -1,
                 .streaming = spec->streaming,
                 .data = spec->streaming || spec->capture,
                 .feed_end = !spec->streaming,
                 .output = {-1, -1},
                 .count = stage_count,
                 .background = spec->background,
                 .limit = spec->capture_limit};
  j->stages = calloc(stage_count, sizeof(*j->stages));
  j->connected = calloc(stage_count, sizeof(bool));
  l.count_stages = stage_count;
  l.stages = calloc(stage_count, sizeof(*l.stages));
  if (!j->stages || !j->connected || !l.stages)
    goto fail;
  struct pollfd *polls =
      realloc(e->polls, (1 + (e->count + 1) * 4) * sizeof(*polls));
  if (!polls)
    goto fail;
  e->polls = polls;
  int defaults[3];
  for (int i = 0; i < 3; ++i) {
    defaults[i] = fcntl(i, F_DUPFD_CLOEXEC, 3);
    if (defaults[i] < 0 && errno != EBADF)
      goto fail;
    if (defaults[i] >= 0 && own(&l, defaults[i]) < 0)
      goto fail;
  }
  if (spec->background || spec->capture || spec->streaming) {
    defaults[0] = own(
        &l, rill_platform_internal(open("/dev/null", O_RDONLY | O_CLOEXEC)));
    if (defaults[0] < 0)
      goto fail;
  }
  for (stage = 0; stage < stage_count; ++stage) {
    const RillExecStage *in = &spec->stages[stage];
    Prepared *out = &l.stages[stage];
    if (!in->argc || in->argc > RILL_EXEC_MAX_ARGUMENTS) {
      errno = EINVAL;
      goto fail;
    }
    if (!rill_platform_env_init(
            &out->env,
            (in->environment ? in->environment : spec->environment)->entries))
      goto fail;
    out->cwd = own(
        &l, rill_platform_internal(open(in->cwd ? in->cwd : ".",
                                        O_RDONLY | O_DIRECTORY | O_CLOEXEC)));
    if (out->cwd < 0)
      goto fail;
    if (in->accepted_codes)
      memcpy(j->stages[stage].accepted_codes, in->accepted_codes,
             sizeof(j->stages[stage].accepted_codes));
    else
      j->stages[stage].accepted_codes[0] = true;
    out->argc = in->argc;
    out->argv = calloc(in->argc + 1, sizeof(char *));
    if (!out->argv)
      goto fail;
    for (size_t a = 0; a < in->argc; ++a) {
      out->argv[a] = copy_bytes(in->argv[a]);
      if (!out->argv[a]) {
        error->has_argument = true;
        error->argument = a;
        goto fail;
      }
    }
    if (!paths(out, rill_platform_env_get(&out->env, "PATH")))
      goto fail;
    memcpy(out->streams, defaults, sizeof(defaults));
    out->pipe_input = out->pipe_output = -1;
  }
  for (stage = 0; stage + 1 < stage_count; ++stage) {
    int fds[2];
    if (!pipe_owned(&l, fds))
      goto fail;
    l.stages[stage].streams[1] = fds[1];
    l.stages[stage + 1].streams[0] = fds[0];
    l.stages[stage].pipe_output = fds[1];
    l.stages[stage + 1].pipe_input = fds[0];
  }
  if (spec->capture || spec->streaming)
    for (int i = 0; i < 2; ++i) {
      if (spec->streaming && i == 1 && !isatty(defaults[2]))
        continue;
      if (spec->streaming && i == 1)
        j->relay = defaults[2];
      int fds[2];
      if (!pipe_owned(&l, fds))
        goto fail;
      j->output[i] = fds[0];
      if (fcntl(fds[0], F_SETFL, O_NONBLOCK) < 0)
        goto fail;
      if (i == 0)
        l.stages[j->count - 1].streams[1] = fds[1];
      else
        for (stage = 0; stage < stage_count; ++stage)
          l.stages[stage].streams[2] = fds[1];
    }
  if (spec->feed) {
    int fds[2];
    if (!pipe_owned(&l, fds) ||
        !rill_text_append(&j->feed, spec->input.data, spec->input.size))
      goto fail;
    j->input = fds[1];
    if (fcntl(fds[1], F_SETFL, O_NONBLOCK) < 0)
      goto fail;
    l.stages[0].streams[0] = fds[0];
  }
  for (stage = 0; stage < stage_count; ++stage) {
    const RillExecStage *in = &spec->stages[stage];
    Prepared *out = &l.stages[stage];
    for (size_t a = 0; a < in->redirect_count; ++a) {
      const RillExecRedirect *r = &in->redirects[a];
      if (r->target < 0 || r->target > 2 ||
          (r->duplicate_stdout && r->target != 2)) {
        errno = EINVAL;
        goto fail;
      }

      if (r->duplicate_stdout) {
        out->streams[2] = out->streams[1];
        continue;
      }
      char *path = copy_bytes(r->path);
      if (!path)
        goto fail;
      int flags = r->target == 0
                      ? O_RDONLY
                      : O_WRONLY | O_CREAT | (r->append ? O_APPEND : O_TRUNC);
      int fd = rill_platform_internal(
          openat(out->cwd, path, flags | O_CLOEXEC | O_NOCTTY, 0666));
      int saved = errno;
      free(path);
      errno = saved;
      out->streams[r->target] = own(&l, fd);
      if (out->streams[r->target] < 0)
        goto fail;
    }
  }
  for (size_t i = 0; i + 1 < stage_count; ++i) {
    Prepared *left = &l.stages[i], *right = &l.stages[i + 1];
    j->connected[i] = (left->streams[1] == left->pipe_output ||
                       left->streams[2] == left->pipe_output) &&
                      right->streams[0] == right->pipe_input;
  }
  int gate[2], errors[2];
  if (!pipe_owned(&l, gate) || !pipe_owned(&l, errors))
    goto fail;
  if (fcntl(errors[0], F_SETFL, O_NONBLOCK) < 0)
    goto fail;
  j->errors = errors[0];
  sigset_t old;
  if (!rill_platform_block(&old))
    goto fail;
  j->state = RILL_JOB_LAUNCHING;
  j->id = ++e->next_id;
  for (stage = 0; stage < stage_count; ++stage) {
    pid_t pid = fork();
    if (pid == 0)
      child(&l, stage, gate[0], errors[1]);
    if (pid < 0) {
      j->error = (RillDiagnostic){.kind = RILL_LAUNCH,
                                  .code = errno,
                                  .stage = stage,
                                  .message = "fork failed"};
      break;
    }
    if (!j->group)
      j->group = pid;
    j->stages[stage].pid = pid;
    if (setpgid(pid, j->group) < 0) {
      j->ungrouped = pid;
      j->error = (RillDiagnostic){.kind = RILL_LAUNCH,
                                  .code = errno,
                                  .stage = stage,
                                  .message = "process group setup failed"};
      ++stage;
      break;
    }
  }
  for (size_t i = stage; i < stage_count; ++i)
    j->stages[i].done = true;
  j->next = e->jobs;
  e->jobs = j;
  ++e->count;
  if (!spec->background) {
    e->attached = j;
    if (!j->error.kind && !spec->capture && !spec->streaming &&
        e->platform->tty >= 0) {
      if (rill_platform_foreground(e->platform, j->group, nullptr))
        j->handed = true;
      else
        j->error = (RillDiagnostic){.kind = RILL_LAUNCH,
                                    .code = errno,
                                    .message = "terminal handoff failed"};
    }
  }
  j->errors = detach(&l, j->errors);
  j->input = detach(&l, j->input);
  j->relay = detach(&l, j->relay);
  for (int i = 0; i < 2; ++i)
    j->output[i] = detach(&l, j->output[i]);
  if (!j->error.kind) {
    // The gate prevents exec while the parent alone registers process groups.
    // The stage limit keeps all permits in one atomic pipe write.
    char permits[RILL_EXEC_MAX_STAGES] = {};
    ssize_t wrote = {};
    do {
      wrote = write(gate[1], permits, j->count);
    } while (wrote < 0 && errno == EINTR);
    if (wrote != (ssize_t)j->count)
      j->error = (RillDiagnostic){.kind = RILL_LAUNCH,
                                  .code = wrote < 0 ? errno : EIO,
                                  .message = "cannot release launch gate"};
  }
  if (j->error.kind)
    rill_exec_cancel(j);
  release(&l);
  (void)sigprocmask(SIG_SETMASK, &old, nullptr);
  return j;
fail:
  {
    int saved = errno;
    release(&l);
    if (j) {
      j->errors = j->input = j->relay = j->output[0] = j->output[1] = -1;
      rill_exec_job_destroy(j);
    }
    *error = (RillDiagnostic){
        .kind = saved == ENOMEM ? RILL_MEMORY : RILL_LAUNCH,
        .code = saved,
        .stage = stage,
        .has_argument = error->has_argument,
        .argument = error->argument,
        .message = saved == EINVAL
                       ? "invalid argument or path (embedded NUL is forbidden)"
                       : "launch preparation failed"};
    return nullptr;
  }
}
