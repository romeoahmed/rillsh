#include "session.h"
#include "config.h"
#include "diagnostic.h"
#include "exec/exec.h"
#include "library/library.h"
#include "platform/posix.h"
#include "runtime/runtime.h"
#include "source.h"
#include "syntax/syntax.h"
#include "text/style.h"
#include "text/text.h"
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>
#include <unistd.h>

typedef struct {
  RillPlatform platform;
  RillEnvironment env;
  RillExec *exec;
  RillEval *eval;
  RillLibrary library;
  RillColorMode color;
  bool interactive;
  int status;
} Session;
static bool write_all(int fd, const char *data, size_t n) {
  while (n) {
    ssize_t size = write(fd, data, n);
    if (size < 0 && errno == EINTR)
      continue;
    if (size <= 0)
      return false;
    data += size;
    n -= (size_t)size;
  }
  return true;
}
static RillPalette palette(Session *s, int fd) {
  return rill_style_palette(s->color, isatty(fd) != 0,
                            rill_platform_env_get(&s->env, "TERM"),
                            rill_platform_env_get(&s->env, "COLORTERM"),
                            rill_platform_env_get(&s->env, "NO_COLOR"));
}
static int out_of_memory(Session *s) {
  s->library.exit_requested = true;
  s->library.exit_code = 1;
  const char message[] = "rillsh: OutOfMemory: allocation failed\n";
  (void)write_all(2, message, sizeof(message) - 1);
  return 1;
}
static int diagnostic(Session *s, const RillSource *source, RillDiagnostic d) {
  if (d.kind == RILL_MEMORY ||
      ((d.kind == RILL_IO || d.kind == RILL_LAUNCH) && d.code == ENOMEM))
    return out_of_memory(s);
  size_t line = 1, column = 1;
  if (source)
    for (size_t i = 0; i < d.offset && i < source->bytes.size; ++i) {
      if (source->bytes.data[i] == '\n') {
        ++line;
        column = 1;
      } else
        ++column;
    }
  [[gnu::cleanup(rill_text_clear)]] RillBuffer out = {};
  const char *name = source ? source->name : "rillsh";
  bool ok = rill_text_escape(&out, (RillBytes){name, strlen(name)}) &&
            rill_text_format(&out, ":%zu:%zu: ", line, column) &&
            rill_style_append(&out, palette(s, 2), rill_diagnostic_name(d.kind),
                              true) &&
            rill_text_append(&out, ": ", 2) &&
            rill_text_append(&out, d.message ? d.message : "failure",
                             strlen(d.message ? d.message : "failure"));
  if (ok && d.kind == RILL_LAUNCH) {
    ok = d.has_argument ? rill_text_format(&out, " (stage %zu, argument %zu)",
                                           d.stage, d.argument)
                        : rill_text_format(&out, " (stage %zu)", d.stage);
  }
  if (ok && d.code && (d.kind == RILL_IO || d.kind == RILL_LAUNCH)) {
    const char *message = strerror(d.code);
    ok = rill_text_append(&out, ": ", 2) &&
         rill_text_append(&out, message, strlen(message));
  }
  if (!ok || !rill_text_append(&out, "\n", 1))
    return out_of_memory(s);
  (void)write_all(2, out.data, out.size);
  if (d.kind == RILL_SYNTAX)
    return 2;
  if (d.kind == RILL_CANCELLED)
    return 130;
  if (d.kind == RILL_PROCESS && d.code)
    return d.code;
  return 1;
}
static unsigned events(Session *s) {
  unsigned bits = rill_platform_signals(&s->platform);
  rill_exec_signal(s->exec, bits);
  if (bits & (RILL_SIG_TERM | RILL_SIG_HUP)) {
    s->library.exit_requested = true;
    s->library.exit_code = bits & RILL_SIG_TERM ? 143 : 129;
    rill_exec_cancel_all(s->exec);
  }
  return bits;
}
static int evaluate(Session *s, RillSource *source, RillSyntax *syntax) {
  if (syntax->state != RILL_COMPLETE)
    return diagnostic(s, source, syntax->diagnostic);
  rill_runtime_begin(s->eval, syntax);
  RillNativePending pending = {};
  bool interrupted = false;
  while (!s->library.exit_requested) {
    (void)rill_exec_poll(s->exec, pending.job ? 20 : 0, -1);
    unsigned bits = events(s);
    interrupted |= (bits & RILL_SIG_INT) != 0;
    if (interrupted) {
      // Foreground/cancel effects finish owned cleanup. An independent
      // background job survives interruption of wait, just as it survives other
      // entry errors.
      if (pending.job && rill_exec_cancelled(pending.job)) {
        if (rill_exec_state(pending.job) != RILL_JOB_COMPLETED)
          continue;
        rill_exec_acknowledge(pending.job);
      }
      rill_runtime_abort(s->eval);
      return 130;
    }
    if (pending.job) {
      if (!rill_library_progress(&s->library, &pending))
        continue;
    }
    RillEvalEvent result = rill_runtime_step(s->eval);
    switch (result.state) {
    case RILL_EVAL_NATIVE:
      (void)rill_library_call(&s->library, &pending, result);
      break;
    case RILL_EVAL_YIELD:
      break;
    case RILL_EVAL_ERROR: {
      int code = diagnostic(s, source, result.diagnostic);
      rill_runtime_abort(s->eval);
      return code;
    }
    case RILL_EVAL_DONE:
      if (s->interactive && result.value.kind != RILL_V_UNIT) {
        [[gnu::cleanup(rill_text_clear)]] RillBuffer out = {};
        if (!rill_library_display(result.value, &out) ||
            !rill_text_append(&out, "\n", 1))
          return out_of_memory(s);
        if (!write_all(s->platform.tty, out.data, out.size))
          return 1;
      }
      return 0;
    }
  }
  rill_runtime_abort(s->eval);
  return s->library.exit_code;
}
static int execute(Session *s, const char *name, const char *data,
                   size_t size) {
  RillSource source;
  RillError error = rill_source_init(&source, name, data, size);
  if (error != RILL_OK)
    return diagnostic(
        s, nullptr,
        (RillDiagnostic){.kind = error,
                         .message = "source must be UTF-8 without NUL"});
  RillSyntax syntax = rill_syntax_parse(&source);
  int code = evaluate(s, &source, &syntax);
  rill_syntax_clear(&syntax);
  rill_source_clear(&source);
  return code;
}
static bool read_source(Session *s, int fd, RillBuffer *out) {
  char buf[16384];
  for (;;) {
    struct pollfd input[] = {{.fd = fd, .events = POLLIN},
                             {.fd = s->platform.signals[0], .events = POLLIN}};
    int ready = poll(input, 2, -1);
    if (ready < 0 && errno != EINTR)
      return false;
    if (events(s) & RILL_SIG_INT) {
      s->library.exit_requested = true;
      s->library.exit_code = 130;
    }
    if (s->library.exit_requested) {
      errno = ECANCELED;
      return false;
    }
    if (input[0].revents & POLLNVAL) {
      errno = EBADF;
      return false;
    }
    if (!(input[0].revents & (POLLIN | POLLHUP | POLLERR)))
      continue;
    ssize_t n = read(fd, buf, sizeof(buf));
    if (n == 0)
      return true;
    if (n < 0) {
      if (errno == EINTR)
        continue;
      return false;
    }
    if (out->size > (size_t)64 * 1024 * 1024 - (size_t)n) {
      errno = EFBIG;
      return false;
    }
    if (!rill_text_append(out, buf, (size_t)n))
      return false;
  }
}
static bool prompt(Session *s, bool continuation) {
  [[gnu::cleanup(rill_text_clear)]] RillBuffer out = {};
  return rill_style_append(&out, palette(s, s->platform.tty),
                           continuation ? "... " : "rill> ", false) &&
         write_all(s->platform.tty, out.data, out.size);
}
static int interactive(Session *s) {
  RillBuffer input = {};
  bool shown = false;
  while (!s->library.exit_requested) {
    if (!shown) {
      if (!prompt(s, input.size != 0)) {
        s->status = 1;
        break;
      }
      shown = true;
    }
    bool ready = rill_exec_poll(s->exec, 20, s->platform.tty);
    unsigned bits = events(s);
    if (bits & RILL_SIG_INT) {
      rill_text_clear(&input);
      shown = false;
      continue;
    }
    if (bits & RILL_SIG_STOP) {
      if (!rill_platform_suspend(&s->platform)) {
        s->status = 1;
        break;
      }
      shown = false;
      continue;
    }
    if (!ready)
      continue;
    char line[4096];
    ssize_t n = read(s->platform.tty, line, sizeof(line));
    if (n < 0) {
      if (errno == EINTR)
        continue;
      s->status = 1;
      break;
    }
    if (n == 0) {
      if (input.size) {
        s->status = execute(s, "<stdin>", input.data, input.size);
        rill_text_clear(&input);
        shown = false;
        continue;
      }
      if (rill_exec_outstanding(s->exec, false)) {
        s->status = diagnostic(
            s, nullptr,
            (RillDiagnostic){
                .kind = RILL_PROCESS,
                .message = "live jobs remain; wait, cancel, or exit_force"});
        shown = false;
        continue;
      }
      s->status = 0;
      break;
    }
    if (input.size > (size_t)8 * 1024 * 1024 - (size_t)n ||
        !rill_text_append(&input, line, (size_t)n)) {
      s->status = 1;
      break;
    }
    RillSource source;
    RillError error =
        rill_source_init(&source, "<stdin>", input.data, input.size);
    if (error != RILL_OK) {
      s->status = diagnostic(
          s, nullptr,
          (RillDiagnostic){.kind = error,
                           .message = "source must be UTF-8 without NUL"});
      rill_text_clear(&input);
      shown = false;
      continue;
    }
    RillSyntax syntax = rill_syntax_parse(&source);
    if (syntax.state != RILL_INCOMPLETE) {
      s->status = evaluate(s, &source, &syntax);
      rill_text_clear(&input);
    }
    rill_syntax_clear(&syntax);
    rill_source_clear(&source);
    shown = false;
  }
  rill_text_clear(&input);
  return s->library.exit_requested ? s->library.exit_code : s->status;
}
int rill_session_main(int argc, char **argv, char **environment) {
  bool force = false, options = true;
  const char *command = nullptr, *file = nullptr;
  RillColorMode color = RILL_COLOR_AUTO;
  for (int i = 1; i < argc; ++i) {
    const char *a = argv[i];
    if (options && !strcmp(a, "--")) {
      options = false;
      continue;
    }
    if (options && !strcmp(a, "--help")) {
      const char *help =
          "Rill Shell " RILL_VERSION
          "\nUsage: rillsh [-i] [--color=auto|always|never] [-c SOURCE | FILE "
          "[ARG...]]\nStage 1: literal ^commands, | pipelines, redirection, "
          "job plans and unary native calls.\nUse let j = start(job { ^command "
          "}); wait(j), fg(j), bg(j), cancel(j).\n";
      return write_all(1, help, strlen(help)) ? 0 : 1;
    }
    if (options && !strcmp(a, "--version"))
      return write_all(1, "Rill Shell " RILL_VERSION "\n",
                       sizeof("Rill Shell " RILL_VERSION "\n") - 1)
                 ? 0
                 : 1;
    if (options && !strcmp(a, "--no-config"))
      continue;
    if (options && !strcmp(a, "-i")) {
      force = true;
      continue;
    }
    if (options && !strncmp(a, "--color=", 8)) {
      if (!strcmp(a + 8, "auto"))
        color = RILL_COLOR_AUTO;
      else if (!strcmp(a + 8, "always"))
        color = RILL_COLOR_ALWAYS;
      else if (!strcmp(a + 8, "never"))
        color = RILL_COLOR_NEVER;
      else
        goto usage;
      continue;
    }
    if (options && !strcmp(a, "-c")) {
      if (++i >= argc || i + 1 != argc)
        goto usage;
      command = argv[i];
      break;
    }
    if (options && a[0] == '-')
      goto usage;
    file = a;
    break;
  }
  if (force && (file || command))
    goto usage;
  Session s = {.color = color,
               .interactive = force || (!file && !command && isatty(0))};
  if (!rill_platform_env_init(&s.env, environment))
    return 1;
  char *cwd = getcwd(nullptr, 0);
  if (!cwd && errno == ENOMEM) {
    rill_platform_env_clear(&s.env);
    return out_of_memory(&s);
  }
  bool env_ok = rill_platform_env_set(&s.env, "PWD", cwd);
  free(cwd);
  if (!env_ok) {
    rill_platform_env_clear(&s.env);
    return 1;
  }
  if (!rill_platform_init(&s.platform, s.interactive)) {
    rill_platform_env_clear(&s.env);
    const char *msg = "rillsh: cannot initialize signal/terminal services\n";
    bool wrote = write_all(2, msg, strlen(msg));
    (void)wrote;
    return 1;
  }
  size_t count;
  const RillNative *natives = rill_library_natives(&count);
  s.eval = rill_runtime_new(natives, count);
  s.exec = rill_exec_new(&s.platform);
  if (!s.eval || !s.exec) {
    rill_runtime_free(s.eval);
    rill_exec_free(s.exec);
    rill_platform_clear(&s.platform);
    rill_platform_env_clear(&s.env);
    return 1;
  }
  s.library =
      (RillLibrary){.exec = s.exec, .environment = &s.env, .eval = s.eval};
  int status;
  if (s.interactive)
    status = interactive(&s);
  else if (command)
    status = execute(&s, "<command>", command, strlen(command));
  else {
    int fd =
        file ? rill_platform_internal(open(file, O_RDONLY | O_CLOEXEC)) : 0;
    RillBuffer source = {};
    if (fd < 0 || !read_source(&s, fd, &source))
      status =
          s.library.exit_requested
              ? s.library.exit_code
              : diagnostic(&s, nullptr,
                           (RillDiagnostic){.kind = RILL_IO,
                                            .code = errno,
                                            .message = "cannot read source"});
    else
      status = execute(&s, file ? file : "<stdin>", source.data, source.size);
    if (file)
      rill_platform_close(&fd);
    rill_text_clear(&source);
  }
  if (!s.interactive && !s.library.exit_requested &&
      rill_exec_outstanding(s.exec, true)) {
    if (!status)
      status = diagnostic(
          &s, nullptr,
          (RillDiagnostic){.kind = RILL_PROCESS,
                           .message = "script ended with unjoined jobs"});
  }
  rill_exec_cancel_all(s.exec);
  while (rill_exec_outstanding(s.exec, false)) {
    (void)rill_exec_poll(s.exec, 20, -1);
    (void)rill_platform_signals(&s.platform);
  }
  rill_runtime_free(s.eval);
  rill_exec_free(s.exec);
  rill_platform_clear(&s.platform);
  rill_platform_env_clear(&s.env);
  return status;
usage:
  {
    const char *msg = "rillsh: invalid options; use --help\n";
    bool wrote = write_all(2, msg, strlen(msg));
    (void)wrote;
    return 2;
  }
}
