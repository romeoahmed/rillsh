#include "fds.h"
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>
#include <termios.h>
#include <unistd.h>
static bool put(int fd, const void *data, size_t size) {
  const char *p = data;
  while (size) {
    ssize_t n = write(fd, p, size);
    if (n < 0 && errno == EINTR)
      continue;
    if (n <= 0)
      return false;
    p += n;
    size -= (size_t)n;
  }
  return true;
}
static void zero(int) { _exit(0); }
int main(int argc, char **argv) {
  if (argc < 2)
    return 2;
  const char *mode = argv[1];
  if (!strcmp(mode, "terminal-owner")) {
    pid_t foreground = tcgetpgrp(0);
    return foreground > 0 && foreground != getpgrp() &&
                   put(1, "noninteractive\n", 15)
               ? 0
               : 3;
  }
  if (!strcmp(mode, "mark")) {
    if (argc != 3)
      return 2;
    int fd = open(argv[2], O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC, 0600);
    return fd >= 0 && close(fd) == 0 ? 0 : 3;
  }
  if (!strcmp(mode, "exec-closed")) {
    if (argc < 4)
      return 2;
    for (const char *p = argv[2]; *p; ++p) {
      if (*p < '0' || *p > '2')
        return 2;
      (void)close(*p - '0');
    }
    // The test runner supplies the built shell path and literal test arguments.
    // NOLINTNEXTLINE(clang-analyzer-optin.taint.GenericTaint)
    execv(argv[3], argv + 3);
    return 3;
  }
  if (!strcmp(mode, "exit")) {
    if (argc != 3)
      return 2;
    char *end;
    errno = 0;
    long n = strtol(argv[2], &end, 10);
    return errno || end == argv[2] || *end || n < 0 || n > 255 ? 2 : (int)n;
  }
  if (!strcmp(mode, "args")) {
    for (int i = 2; i < argc; ++i) {
      char b[32];
      int n = snprintf(b, sizeof(b), "%zu:", strlen(argv[i]));
      if (n < 0 || (size_t)n >= sizeof(b) || !put(1, b, (size_t)n) ||
          !put(1, argv[i], strlen(argv[i])) || !put(1, "\n", 1))
        return 3;
    }
    return 0;
  }
  if (!strcmp(mode, "echo")) {
    char b[16384];
    for (;;) {
      ssize_t n = read(0, b, sizeof(b));
      if (n == 0)
        return 0;
      if (n < 0) {
        if (errno == EINTR)
          continue;
        return 3;
      }
      if (!put(1, b, (size_t)n))
        return 3;
    }
  }
  if (!strcmp(mode, "both")) {
    return put(1, "out\n", 4) && put(2, "err\n", 4) ? 0 : 3;
  }
  if (!strcmp(mode, "flood")) {
    unsigned char bytes[16384];
    for (size_t i = 0; i < sizeof(bytes); ++i)
      bytes[i] = (unsigned char)(i % 256);
    for (int i = 0; i < 128; ++i)
      if (!put(1, bytes, sizeof(bytes)) || !put(2, bytes, sizeof(bytes)))
        return 3;
    size_t size = 0;
    for (;;) {
      ssize_t n = read(0, bytes, sizeof(bytes));
      if (n == 0)
        break;
      if (n < 0) {
        if (errno == EINTR)
          continue;
        return 3;
      }
      for (size_t i = 0; i < (size_t)n; ++i)
        if (bytes[i] != (size + i) % 256)
          return 4;
      size += (size_t)n;
    }
    return size == 2097152 ? 0 : 4;
  }
  if (!strcmp(mode, "signals")) {
    struct sigaction a;
    sigset_t mask;
    if (sigprocmask(SIG_SETMASK, nullptr, &mask) < 0)
      return 3;
    const int signals[] = {SIGPIPE, SIGINT,  SIGCHLD, SIGTERM, SIGHUP,
                           SIGTSTP, SIGTTIN, SIGTTOU, SIGQUIT};
    for (size_t i = 0; i < sizeof(signals) / sizeof(signals[0]); ++i)
      if (sigaction(signals[i], nullptr, &a) < 0 || a.sa_handler != SIG_DFL ||
          sigismember(&mask, signals[i]) != 0)
        return 5;
    return 0;
  }
  if (!strcmp(mode, "fds")) {
    return descriptor_count(3) == 0 ? 0 : 6;
  }
  if (!strcmp(mode, "cwd")) {
    char *cwd = getcwd(nullptr, 0);
    if (!cwd)
      return 3;
    bool ok = put(1, cwd, strlen(cwd));
    free(cwd);
    return ok ? 0 : 3;
  }
  if (!strcmp(mode, "env")) {
    if (argc != 3)
      return 2;
    const char *v = getenv(argv[2]);
    return v ? (put(1, v, strlen(v)) ? 0 : 3) : 7;
  }
  if (!strcmp(mode, "selfstop")) {
    if (!put(1, "selfstop\n", 9) || raise(SIGSTOP))
      return 3;
    return put(1, "continued\n", 10) ? 0 : 3;
  }
  if (!strcmp(mode, "stop-echo")) {
    if (raise(SIGSTOP))
      return 3;
    char byte;
    return read(0, &byte, 1) == 1 && put(1, &byte, 1) ? 0 : 3;
  }
  if (!strcmp(mode, "stop")) {
    struct termios t;
    if (tcgetattr(0, &t) < 0)
      return 3;
    t.c_lflag &= ~(tcflag_t)ECHO;
    if (tcsetattr(0, TCSANOW, &t) < 0 || !put(1, "stopping\n", 9))
      return 3;
    if (raise(SIGTSTP))
      return 3;
    if (!put(1, "resumed\n", 8))
      return 3;
    return 0;
  }
  if (!strcmp(mode, "readtty")) {
    char line[128];
    size_t size = 0;
    if (!put(1, "reading\n", 8))
      return 3;
    while (size < sizeof(line)) {
      if (read(0, &line[size], 1) != 1)
        return 3;
      if (line[size++] == '\n')
        return put(1, "received:", 9) && put(1, line, size) ? 0 : 3;
    }
    return 4;
  }
  if (!strcmp(mode, "produce")) {
    const char data[16384] = {};
    while (put(1, data, sizeof(data))) {
    }
    return 3;
  }
  if (!strcmp(mode, "take")) {
    char byte;
    return read(0, &byte, 1) == 1 ? 0 : 3;
  }
  if (!strcmp(mode, "term-zero") || !strcmp(mode, "ignore-term")) {
    struct sigaction a = {.sa_handler =
                              !strcmp(mode, "term-zero") ? zero : SIG_IGN};
    if (sigemptyset(&a.sa_mask) < 0 || sigaction(SIGTERM, &a, nullptr) < 0)
      return 3;
    if (!put(1, "ready\n", 6))
      return 3;
    for (;;)
      pause();
  }
  return 2;
}
