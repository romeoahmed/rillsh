/**
 * @file
 * @brief Compose CLI startup, canonical input, presentation, and shutdown.
 *
 * The session owns the platform, environment, evaluator, and supervisor. Whole
 * entries share one parser/evaluator path across scripts and the prompt.
 * Effects are coordinated in evaluation.c; this file handles input and the
 * user-facing boundary, including terminal restoration on controlled exit.
 */
#include "session.h"
#include "config.h"
#include "diagnostic.h"
#include "exec/exec.h"
#include "library/bundle.h"
#include "library/library.h"
#include "library/stream.h"
#include "module.h"
#include "platform/posix.h"
#include "private.h"
#include "runtime/runtime.h"
#include "source.h"
#include "syntax/syntax.h"
#include "text/style.h"
#include "text/text.h"
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>
#include <unistd.h>

bool rill_session_write(int fd, const char *data, size_t n) {
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
int rill_session_memory(Session *s) {
  s->library.exit_requested = true;
  s->library.exit_code = 1;
  const char message[] = "rillsh: OutOfMemory: allocation failed\n";
  (void)rill_session_write(2, message, sizeof(message) - 1);
  return 1;
}
int rill_session_diagnostic_at(Session *s, const char *source_name,
                               RillBytes source_bytes, RillDiagnostic d) {
  if (d.kind == RILL_MEMORY ||
      ((d.kind == RILL_IO || d.kind == RILL_LAUNCH) && d.code == ENOMEM))
    return rill_session_memory(s);
  size_t line = 1, column = 1;
  if (source_name)
    for (size_t i = 0; i < d.offset && i < source_bytes.size; ++i) {
      if (source_bytes.data[i] == '\n') {
        ++line;
        column = 1;
      } else
        ++column;
    }
  [[gnu::cleanup(rill_text_clear)]] RillBuffer out = {};
  const char *name = source_name ? source_name : "rillsh";
  [[gnu::cleanup(rill_text_clear)]] RillBuffer label = {}, escaped_message = {};
  if (d.label &&
      (!rill_text_escape(&label, (RillBytes){d.label, d.label_size}) ||
       !rill_text_escape(&escaped_message,
                         (RillBytes){d.message, d.message_size})))
    return rill_session_memory(s);
  const char *kind =
      d.label ? (label.data ? label.data : "") : rill_diagnostic_name(d.kind);
  const char *explanation =
      d.label ? (escaped_message.data ? escaped_message.data : "")
              : (d.message ? d.message : "failure");
  bool ok = rill_text_escape(&out, (RillBytes){name, strlen(name)}) &&
            rill_text_format(&out, ":%zu:%zu: ", line, column) &&
            rill_style_append(&out, palette(s, 2), kind, true) &&
            rill_text_append(&out, ": ", 2) &&
            rill_text_append(&out, explanation, strlen(explanation));
  if (ok && (d.kind == RILL_LAUNCH || d.has_argument)) {
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
    return rill_session_memory(s);
  (void)rill_session_write(2, out.data, out.size);
  if (d.kind == RILL_SYNTAX)
    return 2;
  if (d.kind == RILL_CANCELLED)
    return 130;
  if (d.kind == RILL_PROCESS && d.code)
    return d.code;
  return 1;
}
int rill_session_diagnostic(Session *s, const RillSource *source,
                            RillDiagnostic d) {
  return rill_session_diagnostic_at(
      s, source ? source->name : nullptr,
      source ? (RillBytes){source->bytes.data, source->bytes.size}
             : (RillBytes){},
      d);
}
unsigned rill_session_events(Session *s) {
  unsigned bits = rill_platform_signals(&s->platform);
  rill_exec_signal(s->exec, bits);
  rill_stream_signal(&s->library, bits);
  if (bits & (RILL_SIG_TERM | RILL_SIG_HUP)) {
    s->library.exit_requested = true;
    s->library.exit_code = bits & RILL_SIG_TERM ? 143 : 129;
    rill_exec_cancel_all(s->exec);
  }
  return bits;
}
static int execute(Session *s, const char *name, const char *data,
                   size_t size) {
  [[gnu::cleanup(rill_source_clear)]] RillSource source = {};
  char *canonical = name[0] != '<' && strncmp(name, "std:", 4) != 0
                        ? realpath(name, nullptr)
                        : nullptr;
  RillError error =
      rill_source_init(&source, canonical ? canonical : name, data, size);
  free(canonical);
  if (error != RILL_OK)
    return rill_session_diagnostic(
        s, nullptr,
        (RillDiagnostic){.kind = error,
                         .message = "source must be UTF-8 without NUL"});
  [[gnu::cleanup(rill_syntax_clear)]] RillSyntax syntax =
      rill_syntax_parse(&source);
  return rill_session_evaluate(s, &source, &syntax);
}
static bool read_source(Session *s, int fd, RillBuffer *out) {
  char buf[16384];
  for (;;) {
    struct pollfd input[] = {{.fd = fd, .events = POLLIN},
                             {.fd = s->platform.signals[0], .events = POLLIN}};
    int ready = poll(input, 2, -1);
    if (ready < 0 && errno != EINTR)
      return false;
    if (rill_session_events(s) & RILL_SIG_INT) {
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
static void configure(Session *s) {
  const char *base = rill_platform_env_get(&s->env, "XDG_CONFIG_HOME");
  [[gnu::cleanup(rill_text_clear)]] RillBuffer path = {}, source = {};
  bool ok = {};
  if (base && base[0] == '/')
    ok = rill_text_format(&path, "%s/rillsh/init.rill", base);
  else {
    const char *home = rill_platform_env_get(&s->env, "HOME");
    if (!home || home[0] != '/') {
      (void)rill_session_diagnostic(
          s, nullptr,
          (RillDiagnostic){.kind = RILL_IO,
                           .message = "configuration disabled: no absolute "
                                      "HOME or XDG_CONFIG_HOME"});
      return;
    }
    ok = rill_text_format(&path, "%s/.config/rillsh/init.rill", home);
  }
  if (!ok) {
    (void)rill_session_memory(s);
    return;
  }
  [[gnu::cleanup(rill_platform_close)]] int fd =
      rill_platform_internal(open(path.data, O_RDONLY | O_CLOEXEC | O_NOCTTY));
  if (fd < 0) {
    if (errno != ENOENT)
      (void)rill_session_diagnostic(
          s, nullptr,
          (RillDiagnostic){.kind = RILL_IO,
                           .code = errno,
                           .message = "cannot open startup configuration"});
    return;
  }
  if (read_source(s, fd, &source)) {
    s->interactive = false;
    s->status = execute(s, path.data, source.data, source.size);
    s->interactive = true;
  } else
    (void)rill_session_diagnostic(
        s, nullptr,
        (RillDiagnostic){.kind = RILL_IO,
                         .code = errno,
                         .message = "cannot read startup configuration"});
}
static bool prompt(Session *s, bool continuation) {
  [[gnu::cleanup(rill_text_clear)]] RillBuffer out = {};
  return rill_style_append(&out, palette(s, s->platform.tty),
                           continuation ? "... " : "rill> ", false) &&
         rill_session_write(s->platform.tty, out.data, out.size);
}
static int interactive(Session *s) {
  [[gnu::cleanup(rill_text_clear)]] RillBuffer input = {};
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
    unsigned bits = rill_session_events(s);
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
      if (s->contexts || rill_exec_outstanding(s->exec, false)) {
        s->status = rill_session_diagnostic(
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
    [[gnu::cleanup(rill_source_clear)]] RillSource source = {};
    RillError error =
        rill_source_init(&source, "<stdin>", input.data, input.size);
    if (error != RILL_OK) {
      s->status = rill_session_diagnostic(
          s, nullptr,
          (RillDiagnostic){.kind = error,
                           .message = "source must be UTF-8 without NUL"});
      rill_text_clear(&input);
      shown = false;
      continue;
    }
    [[gnu::cleanup(rill_syntax_clear)]] RillSyntax syntax =
        rill_syntax_parse(&source);
    if (syntax.state != RILL_INCOMPLETE) {
      s->status = rill_session_evaluate(s, &source, &syntax);
      rill_text_clear(&input);
    }
    shown = false;
  }
  return s->library.exit_requested ? s->library.exit_code : s->status;
}
int rill_session_main(int argc, char **argv, char **environment) {
  bool force = false, options = true, no_config = false;
  int argument_start = argc;
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
          "[ARG...]]\nFunctional pipelines, pattern matching, modules, and "
          "reusable job plans.\nUse ^command for external commands, | for byte "
          "pipelines, and |> for function application.\n--no-config skips "
          "interactive startup configuration.\n";
      return rill_session_write(1, help, strlen(help)) ? 0 : 1;
    }
    if (options && !strcmp(a, "--version"))
      return rill_session_write(1, "Rill Shell " RILL_VERSION "\n",
                                sizeof("Rill Shell " RILL_VERSION "\n") - 1)
                 ? 0
                 : 1;
    if (options && !strcmp(a, "--no-config")) {
      no_config = true;
      continue;
    }
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
      if (i != argc - 2)
        goto usage;
      command = argv[i + 1];
      break;
    }
    if (options && a[0] == '-')
      goto usage;
    file = a;
    argument_start = i + 1;
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
    return rill_session_memory(&s);
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
    bool wrote = rill_session_write(2, msg, strlen(msg));
    (void)wrote;
    return 1;
  }
  size_t count = {};
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
  s.library = (RillLibrary){.exec = s.exec,
                            .environment = &s.env,
                            .eval = s.eval,
                            .arguments = argv + argument_start,
                            .argument_count = (size_t)(argc - argument_start)};
  rill_session_context_services(&s);
  int status = {};
  bool user_interactive = s.interactive;
  s.interactive = false;
  const char *prelude = rill_library_source("std:prelude");
  int bootstrap = execute(&s, "std:prelude", prelude, strlen(prelude));
  rill_runtime_prelude(s.eval);
  s.interactive = user_interactive;
  if (!bootstrap && s.interactive && !no_config)
    configure(&s);
  if (bootstrap)
    status = bootstrap;
  else if (s.interactive)
    status = interactive(&s);
  else if (command)
    status = execute(&s, "<command>", command, strlen(command));
  else {
    int fd = file ? rill_platform_internal(
                        open(file, O_RDONLY | O_CLOEXEC | O_NOCTTY))
                  : 0;
    [[gnu::cleanup(rill_text_clear)]] RillBuffer source = {};
    if (fd < 0 || !read_source(&s, fd, &source))
      status = s.library.exit_requested
                   ? s.library.exit_code
                   : rill_session_diagnostic(
                         &s, nullptr,
                         (RillDiagnostic){.kind = RILL_IO,
                                          .code = errno,
                                          .message = "cannot read source"});
    else
      status = execute(&s, file ? file : "<stdin>", source.data, source.size);
    if (file)
      rill_platform_close(&fd);
  }
  if (!s.interactive && !s.library.exit_requested &&
      rill_exec_outstanding(s.exec, true)) {
    if (!status)
      status = rill_session_diagnostic(
          &s, nullptr,
          (RillDiagnostic){.kind = RILL_PROCESS,
                           .message = "script ended with unjoined jobs"});
  }
  rill_session_context_clear(&s);
  rill_exec_cancel_all(s.exec);
  while (rill_exec_outstanding(s.exec, false)) {
    (void)rill_exec_poll(s.exec, 20, -1);
    (void)rill_platform_signals(&s.platform);
  }
  rill_module_clear(&s.modules);
  rill_runtime_free(s.eval);
  rill_exec_free(s.exec);
  rill_platform_clear(&s.platform);
  rill_platform_env_clear(&s.env);
  return status;
usage:
  {
    const char *msg = "rillsh: invalid options; use --help\n";
    bool wrote = rill_session_write(2, msg, strlen(msg));
    (void)wrote;
    return 2;
  }
}
