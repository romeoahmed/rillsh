/**
 * @file
 * @brief Advance process jobs, bounded byte queues, and cleanup deadlines.
 *
 * One supervisor owns child identities and captured storage. Polling pumps all
 * channels, observes wait statuses, and aggregates completion policy. Pipe EOF
 * does not prove process success; signaling authority ends with the last owned
 * member, even if output or a retained report remains.
 */
#include "diagnostic.h"
#include "exec.h"
#include "platform/posix.h"
#include "private.h"
#include "text/text.h"
#include <assert.h>
#include <errno.h>
#include <poll.h>
#include <signal.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>
RillExec *rill_exec_new(RillPlatform *p) {
  RillExec *e = malloc(sizeof(*e));
  if (e) {
    *e = (RillExec){.platform = p, .polls = malloc(sizeof(*e->polls))};
    if (!e->polls) {
      free(e);
      return nullptr;
    }
  }
  return e;
}
bool rill_exec_job_live(const RillJob *j) {
  for (size_t i = 0; i < j->count; ++i)
    if (j->stages[i].pid > 0 && !j->stages[i].done)
      return true;
  return false;
}
void rill_exec_job_destroy(RillJob *j) {
  if (!j)
    return;
  rill_platform_close(&j->errors);
  rill_platform_close(&j->input);
  rill_platform_close(&j->relay);
  for (int i = 0; i < 2; ++i) {
    rill_platform_close(&j->output[i]);
    rill_text_clear(&j->captured[i]);
  }
  rill_text_clear(&j->feed);
  free(j->stages);
  free(j->connected);
  free(j);
}
static void signal_job(RillJob *j, int signal) {
  bool grouped = false;
  for (size_t i = 0; i < j->count; ++i) {
    const RillExecStatus *s = &j->stages[i];
    if (s->pid <= 0 || s->done)
      continue;
    if (s->pid == j->ungrouped)
      (void)kill(s->pid, signal);
    else
      grouped = true;
  }
  // An unreaped registered child prevents reuse of this process-group ID.
  if (grouped)
    (void)kill(-j->group, signal);
}
static void terminate(RillJob *j) {
  if (j->state == RILL_JOB_COMPLETED || j->state == RILL_JOB_CANCELLING)
    return;
  j->state = RILL_JOB_CANCELLING;
  j->deadline = rill_platform_now() + 1000;
  if (j->relay >= 0)
    rill_text_truncate(&j->captured[1], 0);
  rill_platform_close(&j->input);
  for (int i = 0; i < 2; ++i)
    rill_platform_close(&j->output[i]);
  signal_job(j, SIGTERM);
  signal_job(j, SIGCONT);
}
void rill_exec_cancel(RillJob *j) {
  if (j->state == RILL_JOB_COMPLETED)
    return;
  j->cancelled = true;
  terminate(j);
}
void rill_exec_cutoff(RillJob *j) {
  if (j->cancelled)
    return;
  j->cutoff = true;
  for (size_t i = 0; i < j->count; ++i)
    if (!j->stages[i].done)
      j->stages[i].cutoff_signal = true;
  terminate(j);
}
static void io_failure(RillJob *j, const char *message) {
  if (!j->error.kind)
    j->error =
        (RillDiagnostic){.kind = RILL_IO, .code = errno, .message = message};
  rill_exec_cancel(j);
}
// One bounded operation per channel keeps a busy job from starving others.
static void pump(RillJob *j) {
  if (j->relay >= 0 && j->captured[1].size) {
    struct pollfd fd = {.fd = j->relay, .events = POLLOUT};
    if (poll(&fd, 1, 0) > 0) {
      ssize_t n = write(j->relay, j->captured[1].data, j->captured[1].size);
      if (n > 0) {
        memmove(j->captured[1].data, j->captured[1].data + n,
                j->captured[1].size - (size_t)n);
        rill_text_truncate(&j->captured[1], j->captured[1].size - (size_t)n);
      } else if (n < 0 && errno != EINTR && errno != EAGAIN)
        io_failure(j, "cannot relay child stderr");
    }
  }
  if (j->errors >= 0) {
    ssize_t n = read(j->errors, (char *)&j->wire_error + j->error_used,
                     sizeof(j->wire_error) - j->error_used);
    if (n > 0) {
      j->error_used += (size_t)n;
      if (j->error_used == sizeof(j->wire_error)) {
        if (!j->error.kind)
          j->error = (RillDiagnostic){.kind = RILL_LAUNCH,
                                      .code = j->wire_error.code,
                                      .stage = j->wire_error.stage,
                                      .message = "child setup or exec failed"};
        j->error_used = 0;
        rill_exec_cancel(j);
      }
    } else if (n == 0) {
      rill_platform_close(&j->errors);
      if (j->error_used) {
        errno = EIO;
        io_failure(j, "incomplete child error record");
      }
      if (j->state == RILL_JOB_LAUNCHING)
        j->state = RILL_JOB_RUNNING;
    } else if (errno != EINTR && errno != EAGAIN) {
      rill_platform_close(&j->errors);
      io_failure(j, "cannot read child launch channel");
    }
  }
  if (j->input >= 0) {
    if (j->fed == j->feed.size) {
      j->fed = 0;
      rill_text_truncate(&j->feed, 0);
      if (j->feed_end)
        rill_platform_close(&j->input);
    } else {
      size_t size = j->feed.size - j->fed;
      if (size > RILL_EXEC_QUEUE_BYTES)
        size = RILL_EXEC_QUEUE_BYTES;
      ssize_t n = write(j->input, j->feed.data + j->fed, size);
      if (n > 0)
        j->fed += (size_t)n;
      else if (n < 0 && errno == EPIPE)
        rill_platform_close(&j->input);
      else if (n < 0 && errno != EINTR && errno != EAGAIN)
        io_failure(j, "cannot feed child input");
    }
  }
  for (int i = 0; i < 2; ++i)
    if (j->output[i] >= 0) {
      char bytes[16384];
      size_t room = sizeof(bytes);
      if (j->streaming) {
        size_t available = RILL_EXEC_QUEUE_BYTES - j->captured[i].size;
        if (room > available)
          room = available;
      }
      if (!room)
        continue;
      ssize_t n = read(j->output[i], bytes, room);
      if (n > 0) {
        size_t current = j->captured[0].size + j->captured[1].size;
        if (!j->streaming && (size_t)n > j->limit - current) {
          int length = snprintf(
              j->error_text, sizeof(j->error_text),
              "capture byte limit %zu exceeded: %zu retained, %zu additional",
              j->limit, current, (size_t)n);
          j->error = (RillDiagnostic){
              .kind = RILL_LIMIT,
              .message = length >= 0 && (size_t)length < sizeof(j->error_text)
                             ? j->error_text
                             : "combined capture byte limit exceeded"};
          rill_exec_cancel(j);
        } else if (!rill_text_append(&j->captured[i], bytes, (size_t)n)) {
          j->error = (RillDiagnostic){.kind = RILL_MEMORY,
                                      .message = "capture allocation failed"};
          rill_exec_cancel(j);
        }
      } else if (n == 0)
        rill_platform_close(&j->output[i]);
      else if (errno != EINTR && errno != EAGAIN)
        io_failure(j, "cannot read child output");
    }
}
static void reap(RillJob *j) {
  for (size_t i = 0; i < j->count; ++i) {
    RillExecStatus *s = &j->stages[i];
    if (s->done || s->pid <= 0)
      continue;
    int status = {};
    pid_t pid = {};
    while ((pid = waitpid(s->pid, &status, WNOHANG | WUNTRACED | WCONTINUED)) >
           0) {
      if (WIFSTOPPED(status)) {
        s->stopped = true;
        s->status = WSTOPSIG(status);
      } else if (WIFCONTINUED(status))
        s->stopped = false;
      else {
        s->done = true;
        s->stopped = false;
        s->signaled = WIFSIGNALED(status);
        s->status = s->signaled ? WTERMSIG(status) : WEXITSTATUS(status);
        if (j->handed && s->signaled && s->status == SIGINT)
          rill_exec_cancel(j);
        break;
      }
    }
    if (pid < 0 && errno != EINTR) {
      s->done = true;
      io_failure(j, "owned child could not be reaped");
    }
  }
  bool live = false, all_stopped = true;
  for (size_t i = 0; i < j->count; ++i)
    if (!j->stages[i].done) {
      live = true;
      if (!j->stages[i].stopped)
        all_stopped = false;
    }
  if (!j->cancelled && !j->cutoff) {
    if (live && all_stopped)
      j->state = RILL_JOB_STOPPED;
    else if (j->state == RILL_JOB_STOPPED)
      j->state = j->errors >= 0 ? RILL_JOB_LAUNCHING : RILL_JOB_RUNNING;
  }
  if (live && (j->cancelled || j->cutoff) && !j->killed &&
      rill_platform_now() >= j->deadline) {
    signal_job(j, SIGKILL);
    j->killed = true;
  }
  if (!live && j->errors < 0 && j->output[0] < 0 && j->output[1] < 0 &&
      (j->relay < 0 || !j->captured[1].size)) {
    j->state = RILL_JOB_COMPLETED;
    rill_platform_close(&j->input);
    bool downstream = true;
    for (size_t i = j->count; i > 0; --i) {
      RillExecStatus *s = &j->stages[i - 1];
      s->expected_pipe =
          s->signaled && !j->cancelled &&
          ((s->status == SIGPIPE &&
            ((j->connected[i - 1] && downstream) || j->cutoff)) ||
           (j->cutoff && s->cutoff_signal &&
            (s->status == SIGTERM || s->status == SIGKILL)));
      downstream =
          downstream && ((!s->signaled && s->status >= 0 && s->status < 256 &&
                          s->accepted_codes[s->status]) ||
                         s->expected_pipe);
    }
  }
}
bool rill_exec_poll(RillExec *e, int timeout, int extra_fd) {
  size_t count = 0;
  struct pollfd *fds = e->polls;
  fds[count++] =
      (struct pollfd){.fd = e->platform->signals[0], .events = POLLIN};
  for (RillJob *j = e->jobs; j; j = j->next) {
    pump(j);
    reap(j);
    if (j->state == RILL_JOB_COMPLETED || j->state == RILL_JOB_STOPPED) {
      if (j->handed) {
        if (!rill_platform_reclaim(e->platform, j->state == RILL_JOB_STOPPED
                                                    ? &j->modes
                                                    : nullptr))
          io_failure(j, "cannot restore terminal");
        j->has_modes = j->state == RILL_JOB_STOPPED;
        j->handed = false;
      }
      if (e->attached == j)
        e->attached = nullptr;
    }
    fds[count++] = (struct pollfd){.fd = j->errors, .events = POLLIN};
    fds[count++] = (struct pollfd){
        .fd = j->input >= 0 && j->fed < j->feed.size ? j->input : -1,
        .events = POLLOUT};
    fds[count++] = (struct pollfd){
        .fd = !j->streaming || j->captured[0].size < RILL_EXEC_QUEUE_BYTES
                  ? j->output[0]
                  : -1,
        .events = POLLIN};
    fds[count++] = (struct pollfd){
        .fd = !j->streaming || j->captured[1].size < RILL_EXEC_QUEUE_BYTES
                  ? j->output[1]
                  : -1,
        .events = POLLIN};
  }
  if (timeout < 0 || timeout > 20)
    timeout = 20; // Also service cancellation deadlines without a timer FD.
  if (rill_platform_input_ready(extra_fd))
    timeout = 0;
  (void)poll(fds, (nfds_t)count, timeout);
  return rill_platform_input_ready(extra_fd);
}
void rill_exec_signal(RillExec *e, unsigned events) {
  RillJob *j = e->attached;
  if (!j || !rill_exec_job_live(j))
    return;
  if (events & RILL_SIG_INT)
    rill_exec_cancel(j);
  if (events & RILL_SIG_STOP)
    signal_job(j, SIGTSTP);
}
size_t rill_exec_id(const RillJob *j) { return j->id; }
RillJob *rill_exec_find(RillExec *e, size_t id) {
  for (RillJob *j = e->jobs; j; j = j->next)
    if (j->id == id)
      return j;
  return nullptr;
}
const RillExecStatus *rill_exec_status(const RillJob *j, size_t *n) {
  *n = j->count;
  return j->stages;
}
RillJobState rill_exec_state(const RillJob *j) { return j->state; }
bool rill_exec_launched(const RillJob *j) { return j->errors < 0; }
RillDiagnostic rill_exec_error(const RillJob *j) { return j->error; }
bool rill_exec_cancelled(const RillJob *j) { return j->cancelled; }
int rill_exec_result(const RillJob *j) {
  if (j->error.kind)
    return 1;
  if (j->cancelled)
    return 130;
  if (j->state != RILL_JOB_COMPLETED)
    return 1;
  for (size_t i = j->count; i > 0; --i) {
    const RillExecStatus *s = &j->stages[i - 1];
    if (s->expected_pipe)
      continue;
    if (s->signaled)
      return s->status > 127 ? 255 : 128 + s->status;
    if (s->status < 0 || s->status > 255 || !s->accepted_codes[s->status])
      return s->status ? s->status : 1;
  }
  return 0;
}
const RillBuffer *rill_exec_output(const RillJob *j, int stream) {
  return &j->captured[stream == 2 ? 1 : 0];
}
bool rill_exec_resume(RillExec *e, RillJob *j, bool foreground) {
  if (j->state == RILL_JOB_COMPLETED)
    return true;
  if (j->state == RILL_JOB_CANCELLING) {
    errno = EINVAL;
    return false;
  }
  // Captured output can outlive the direct children. A historical group ID
  // does not authorize terminal handoff or SIGCONT after the last reap.
  if (!rill_exec_job_live(j))
    return true;
  if (foreground) {
    if (!j->data &&
        !rill_platform_foreground(e->platform, j->group,
                                  j->has_modes ? &j->modes : nullptr))
      return false;
    j->handed = !j->data && e->platform->tty >= 0;
    e->attached = j;
  }
  if (kill(-j->group, SIGCONT) < 0) {
    int saved = errno;
    if (j->handed)
      (void)rill_platform_reclaim(e->platform, nullptr);
    j->handed = false;
    if (e->attached == j)
      e->attached = nullptr;
    errno = saved;
    return false;
  }
  for (size_t i = 0; i < j->count; ++i)
    j->stages[i].stopped = false;
  j->state = RILL_JOB_RUNNING;
  return true;
}
void rill_exec_acknowledge(RillJob *j) { j->acknowledged = true; }
bool rill_exec_outstanding(const RillExec *e, bool unjoined) {
  for (RillJob *j = e->jobs; j; j = j->next)
    if (j->state != RILL_JOB_COMPLETED ||
        (unjoined && j->background && !j->acknowledged))
      return true;
  return false;
}
void rill_exec_cancel_all(RillExec *e) {
  for (RillJob *j = e->jobs; j; j = j->next)
    rill_exec_cancel(j);
}
void rill_exec_free(RillExec *e) {
  if (!e)
    return;
  RillJob *j = e->jobs;
  while (j) {
    RillJob *next = j->next;
    assert(!rill_exec_job_live(j));
    rill_exec_job_destroy(j);
    j = next;
  }
  free(e->polls);
  free(e);
}

RillJob *rill_exec_first(RillExec *e) { return e->jobs; }
RillJob *rill_exec_next(RillJob *j) { return j->next; }
void rill_exec_unmark(RillExec *e) {
  for (RillJob *j = e->jobs; j; j = j->next)
    j->retained = false;
}
void rill_exec_retain(RillExec *e, size_t id) {
  RillJob *j = rill_exec_find(e, id);
  if (j)
    j->retained = true;
}
void rill_exec_prune(RillExec *e) {
  RillJob **at = &e->jobs;
  while (*at) {
    RillJob *j = *at;
    if (j->state == RILL_JOB_COMPLETED && j->acknowledged && !j->retained &&
        j != e->attached) {
      *at = j->next;
      --e->count;
      rill_exec_job_destroy(j);
    } else
      at = &j->next;
  }
}

void rill_exec_consume(RillJob *j, size_t count) {
  assert(j->streaming && count <= j->captured[0].size);
  if (!count)
    return;
  memmove(j->captured[0].data, j->captured[0].data + count,
          j->captured[0].size - count);
  rill_text_truncate(&j->captured[0], j->captured[0].size - count);
}
bool rill_exec_feed_ready(const RillJob *j) {
  return j->input >= 0 && !j->feed_end && j->fed == j->feed.size;
}
bool rill_exec_feed(RillJob *j, RillBytes bytes) {
  assert(rill_exec_feed_ready(j) && bytes.size <= RILL_EXEC_QUEUE_BYTES);
  j->fed = 0;
  rill_text_truncate(&j->feed, 0);
  return rill_text_append(&j->feed, bytes.data, bytes.size);
}
void rill_exec_feed_end(RillJob *j) { j->feed_end = true; }
bool rill_exec_feed_open(const RillJob *j) { return j->input >= 0; }
bool rill_exec_cutoff_requested(const RillJob *j) { return j->cutoff; }

void rill_exec_stop(RillJob *j) {
  if (rill_exec_job_live(j) && !j->cancelled && !j->cutoff)
    signal_job(j, SIGSTOP);
}
bool rill_exec_quiescent(const RillJob *j) {
  return !rill_exec_job_live(j) || j->state == RILL_JOB_STOPPED;
}
