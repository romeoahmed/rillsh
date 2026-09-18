#include "posix.h"
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stddef.h>
#include <stdint.h>
#include <sys/select.h>
#include <sys/types.h>
#include <termios.h>
#include <time.h>
#include <unistd.h>
static volatile sig_atomic_t pending;
static volatile sig_atomic_t notify_fd = -1;
static const int handled[] = {SIGCHLD, SIGINT,  SIGTERM, SIGHUP,
                              SIGTSTP, SIGQUIT, SIGPIPE, SIGTTOU};
static void handler(int sig) {
  int saved = errno;
  unsigned char byte = 0;
  switch (sig) {
  case SIGCHLD:
    pending |= RILL_SIG_CHILD;
    break;
  case SIGINT:
    pending |= RILL_SIG_INT;
    break;
  case SIGTERM:
    pending |= RILL_SIG_TERM;
    break;
  case SIGHUP:
    pending |= RILL_SIG_HUP;
    break;
  case SIGTSTP:
    pending |= RILL_SIG_STOP;
    break;
  default:
    break;
  }
  // Flags retain events even when the nonblocking wakeup pipe is full.
  if (notify_fd >= 0) {
    ssize_t ignored = write(notify_fd, &byte, 1);
    (void)ignored;
  }
  errno = saved;
}
void rill_platform_close(int *fd) {
  if (*fd >= 0) {
    int saved = errno;
    (void)close(*fd);
    *fd = -1;
    errno = saved;
  }
}
int rill_platform_internal(int fd) {
  if (fd < 0)
    return -1;
  if (fd < 3) {
    int moved = fcntl(fd, F_DUPFD_CLOEXEC, 3);
    int saved = errno;
    (void)close(fd);
    errno = saved;
    return moved;
  }
  if (fcntl(fd, F_SETFD, FD_CLOEXEC) < 0) {
    int saved = errno;
    (void)close(fd);
    errno = saved;
    return -1;
  }
  return fd;
}
bool rill_platform_pipe(int fds[2], bool nonblocking) {
  fds[0] = -1;
  fds[1] = -1;
  if (pipe(fds) < 0)
    return false;
  for (size_t i = 0; i < 2; ++i) {
    fds[i] = rill_platform_internal(fds[i]);
    if (fds[i] < 0 || (nonblocking && fcntl(fds[i], F_SETFL, O_NONBLOCK) < 0)) {
      rill_platform_close(&fds[0]);
      rill_platform_close(&fds[1]);
      return false;
    }
  }
  return true;
}
bool rill_platform_block(sigset_t *previous) {
  sigset_t set;
  if (sigemptyset(&set) < 0)
    return false;
  for (size_t i = 0; i < sizeof(handled) / sizeof(handled[0]); ++i)
    if (sigaddset(&set, handled[i]) < 0)
      return false;
  return sigprocmask(SIG_BLOCK, &set, previous) == 0;
}
bool rill_platform_init(RillPlatform *p, bool interactive) {
  *p = (RillPlatform){.signals = {-1, -1}, .tty = -1, .group = getpgrp()};
  if (interactive) {
    p->tty = rill_platform_internal(open("/dev/tty", O_RDWR | O_CLOEXEC));
    if (p->tty < 0)
      return false;
    pid_t foreground;
    while ((foreground = tcgetpgrp(p->tty)) != p->group) {
      if (foreground < 0)
        goto fail;
      struct sigaction action = {.sa_handler = SIG_DFL}, previous;
      if (sigemptyset(&action.sa_mask) < 0 ||
          sigaction(SIGTTIN, &action, &previous) < 0)
        goto fail;
      int stopped = kill(-p->group, SIGTTIN);
      int saved = errno;
      if (sigaction(SIGTTIN, &previous, nullptr) < 0)
        goto fail;
      if (stopped < 0) {
        errno = saved;
        goto fail;
      }
    }
    if (p->group != getpid() && setpgid(0, 0) < 0)
      goto fail;
    p->group = getpgrp();
    if (tcgetattr(p->tty, &p->modes) < 0)
      goto fail;
  }
  if (!rill_platform_pipe(p->signals, true))
    goto fail;
  pending = 0;
  notify_fd = p->signals[1];
  for (size_t i = 0; i < sizeof(handled) / sizeof(handled[0]); ++i) {
    struct sigaction action = {.sa_handler = i >= 5 ? SIG_IGN : handler};
    if (sigfillset(&action.sa_mask) < 0 ||
        sigaction(handled[i], &action, &p->saved[i]) < 0)
      goto fail;
    ++p->installed;
  }
  if (p->tty >= 0 && tcsetpgrp(p->tty, p->group) < 0)
    goto fail;
  p->owns_terminal = p->tty >= 0;
  return true;
fail:
  {
    int saved = errno;
    rill_platform_clear(p);
    errno = saved;
    return false;
  }
}
void rill_platform_clear(RillPlatform *p) {
  if (p->owns_terminal)
    (void)rill_platform_reclaim(p, nullptr);
  notify_fd = -1;
  for (size_t i = 0; i < p->installed; ++i)
    (void)sigaction(handled[i], &p->saved[i], nullptr);
  p->installed = 0;
  rill_platform_close(&p->signals[0]);
  rill_platform_close(&p->signals[1]);
  rill_platform_close(&p->tty);
  p->owns_terminal = false;
}
unsigned rill_platform_signals(RillPlatform *p) {
  sigset_t old;
  if (!rill_platform_block(&old))
    return 0;
  char bytes[128];
  while (read(p->signals[0], bytes, sizeof(bytes)) > 0) {
  }
  unsigned result = (unsigned)pending;
  // Handlers remain blocked until both the pipe and flags have been drained.
  pending = 0;
  (void)sigprocmask(SIG_SETMASK, &old, nullptr);
  return result;
}
bool rill_platform_child_signals() {
  struct sigaction action = {.sa_handler = SIG_DFL};
  if (sigemptyset(&action.sa_mask) < 0)
    return false;
  for (size_t i = 0; i < sizeof(handled) / sizeof(handled[0]); ++i)
    if (sigaction(handled[i], &action, nullptr) < 0)
      return false;
  if (sigaction(SIGTTIN, &action, nullptr) < 0)
    return false;
  return sigprocmask(SIG_SETMASK, &action.sa_mask, nullptr) == 0;
}
bool rill_platform_foreground(RillPlatform *p, pid_t group,
                              const struct termios *modes) {
  if (p->tty < 0)
    return true;
  if (tcsetpgrp(p->tty, group) < 0)
    return false;
  if (!modes || tcsetattr(p->tty, TCSADRAIN, modes) == 0)
    return true;
  int saved = errno;
  (void)rill_platform_reclaim(p, nullptr);
  errno = saved;
  return false;
}
bool rill_platform_reclaim(RillPlatform *p, struct termios *modes) {
  if (p->tty < 0)
    return true;
  bool ok = true;
  if (modes && tcgetattr(p->tty, modes) < 0)
    ok = false;
  if (tcsetpgrp(p->tty, p->group) < 0)
    ok = false;
  if (tcsetattr(p->tty, TCSADRAIN, &p->modes) < 0)
    ok = false;
  return ok;
}
bool rill_platform_suspend(RillPlatform *p) {
  if (!rill_platform_reclaim(p, nullptr))
    return false;
  struct sigaction action = {.sa_handler = SIG_DFL}, old;
  if (sigemptyset(&action.sa_mask) < 0 || sigaction(SIGTSTP, &action, &old) < 0)
    return false;
  int result = raise(SIGTSTP);
  if (sigaction(SIGTSTP, &old, nullptr) < 0)
    return false;
  return result == 0 && rill_platform_reclaim(p, nullptr);
}
int64_t rill_platform_now() {
  struct timespec ts;
  if (clock_gettime(CLOCK_MONOTONIC, &ts) < 0)
    return 0;
  return (int64_t)ts.tv_sec * 1000 + ts.tv_nsec / 1000000;
}

bool rill_platform_input_ready(int fd) {
  if (fd < 0 || fd >= FD_SETSIZE)
    return false;
  fd_set set;
  FD_ZERO(&set);
  FD_SET(fd, &set);
  struct timeval timeout = {};
  // Darwin's /dev/tty returns POLLNVAL from poll; select supports this device.
  return select(fd + 1, &set, nullptr, nullptr, &timeout) > 0;
}
