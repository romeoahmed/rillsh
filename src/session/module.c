/**
 * @file
 * @brief Load modules by identity and track unfinished imports.
 *
 * Completed exports are runtime-rooted; the cache owns paths and load state.
 * Relative imports use the importing directory, cycles fail explicitly, and
 * failed entries are removed so a later import can retry. Loading resumes the
 * evaluator without running it recursively.
 */
#include "module.h"
#include "diagnostic.h"
#include "native/bundle.h"
#include "platform/posix.h"
#include "runtime/runtime.h"
#include "source.h"
#include "syntax/syntax.h"
#include "text/text.h"
#include <errno.h>
#include <fcntl.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>

struct RillModule {
  RillModule *next, *parent;
  char *name;
  dev_t device;
  ino_t inode;
  bool bundled, complete;
  RillValue value;
};
static void resume_error(RillEval *eval, RillError kind, int code,
                         const char *message) {
  rill_runtime_resume(
      eval, (RillValue){},
      (RillDiagnostic){.kind = kind, .code = code, .message = message});
}
void rill_module_event(RillModules *m, RillEval *eval, RillEvalEvent event) {
  if (event.state == RILL_EVAL_MODULE) {
    RillModule *entry = m->active;
    if (!entry) {
      resume_error(eval, RILL_TYPE, 0, "module completion without import");
      return;
    }
    RillError escape =
        rill_runtime_persistent(rill_runtime_heap(eval), event.value);
    if (escape) {
      resume_error(eval, escape, 0, "module cannot export a scoped Stream");
      return;
    }
    entry->value = event.value;
    if (!rill_runtime_retain_module(eval, event.value)) {
      resume_error(eval, RILL_MEMORY, 0, "cannot retain module");
      return;
    }
    entry->complete = true;
    m->active = entry->parent;
    rill_runtime_resume(eval, event.value, (RillDiagnostic){});
    return;
  }
  RillBytes path = event.value.as.object->bytes;
  if (memchr(path.data, 0, path.size)) {
    resume_error(eval, RILL_TYPE, 0, "module path contains NUL");
    return;
  }
  bool bundled = path.size >= 4 && !memcmp(path.data, "std:", 4);
  const char *embedded = bundled ? rill_library_source(path.data) : nullptr;
  if (bundled && (!embedded || !strcmp(path.data, "std:prelude"))) {
    resume_error(eval, RILL_IO, 0, "unknown standard module");
    return;
  }
  [[gnu::cleanup(rill_text_clear)]] RillBuffer resolved = {}, contents = {};
  if (!bundled && path.size && path.data[0] != '/') {
    const char *source = event.source ? event.source : "";
    const char *slash = strrchr(source, '/');
    if (source[0] == '/' && slash) {
      if (!rill_text_append(&resolved, source, (size_t)(slash - source) + 1)) {
        resume_error(eval, RILL_MEMORY, 0, "allocation failed");
        return;
      }
    } else if (m->base) {
      if (!rill_text_format(&resolved, "%s/", m->base)) {
        resume_error(eval, RILL_MEMORY, 0, "allocation failed");
        return;
      }
    } else {
      resume_error(eval, RILL_IO, 0, "entry directory unavailable");
      return;
    }
  }
  if (!rill_text_append(&resolved, path.data, path.size)) {
    resume_error(eval, RILL_MEMORY, 0, "allocation failed");
    return;
  }
  struct stat identity = {};
  char *canonical =
      bundled ? strdup(path.data) : realpath(resolved.data, nullptr);
  if (!canonical) {
    resume_error(eval, errno == ENOMEM ? RILL_MEMORY : RILL_IO, errno,
                 "cannot resolve module");
    return;
  }
  [[gnu::cleanup(rill_platform_close)]] int fd =
      bundled ? -1
              : rill_platform_internal(open(
                    canonical, O_RDONLY | O_CLOEXEC | O_NONBLOCK | O_NOCTTY));
  if (!bundled) {
    // Opening a FIFO must not wait for a writer before type validation.
    if (fd < 0 || fstat(fd, &identity) < 0) {
      free(canonical);
      resume_error(eval, RILL_IO, errno, "cannot open module source");
      return;
    }
    if (!S_ISREG(identity.st_mode)) {
      free(canonical);
      resume_error(eval, RILL_IO, EINVAL,
                   "module source must be a regular file");
      return;
    }
  }
  for (RillModule *entry = m->entries; entry; entry = entry->next) {
    bool same = bundled ? entry->bundled && !strcmp(entry->name, canonical)
                        : !entry->bundled && entry->device == identity.st_dev &&
                              entry->inode == identity.st_ino;
    if (!same)
      continue;
    free(canonical);
    if (!entry->complete)
      resume_error(eval, RILL_TYPE, 0, "cyclic module import");
    else
      rill_runtime_resume(eval, entry->value, (RillDiagnostic){});
    return;
  }
  bool read_ok = true;
  if (bundled)
    read_ok = rill_text_append(&contents, embedded, strlen(embedded));
  else
    for (;;) {
      char block[16384];
      ssize_t n = read(fd, block, sizeof(block));
      if (n < 0 && errno == EINTR)
        continue;
      if (n <= 0) {
        read_ok = n == 0;
        break;
      }
      if (contents.size > (size_t)64 * 1024 * 1024 - (size_t)n) {
        errno = EFBIG;
        read_ok = false;
        break;
      }
      if (!rill_text_append(&contents, block, (size_t)n)) {
        read_ok = false;
        break;
      }
    }
  rill_platform_close(&fd);
  if (!read_ok) {
    free(canonical);
    resume_error(eval, errno == ENOMEM ? RILL_MEMORY : RILL_IO, errno,
                 "cannot read module");
    return;
  }
  [[gnu::cleanup(rill_source_clear)]] RillSource source = {};
  RillError failure =
      rill_source_init(&source, canonical, contents.data, contents.size);
  if (failure) {
    free(canonical);
    resume_error(eval, failure, 0, "invalid module source");
    return;
  }
  [[gnu::cleanup(rill_syntax_clear)]] RillSyntax syntax =
      rill_syntax_parse(&source);
  rill_source_clear(&source);
  RillModule *entry = malloc(sizeof(*entry));
  if (!entry) {
    free(canonical);
    resume_error(eval, RILL_MEMORY, 0, "allocation failed");
    return;
  }
  *entry = (RillModule){.next = m->entries,
                        .parent = m->active,
                        .name = canonical,
                        .device = identity.st_dev,
                        .inode = identity.st_ino,
                        .bundled = bundled};
  m->entries = entry;
  m->active = entry;
  rill_runtime_module(eval, &syntax);
}
void rill_module_abort(RillModules *m) {
  while (m->active) {
    RillModule *entry = m->active;
    m->active = entry->parent;
    RillModule **link = &m->entries;
    while (*link && *link != entry)
      link = &(*link)->next;
    if (*link)
      *link = entry->next;
    free(entry->name);
    free(entry);
  }
}
void rill_module_clear(RillModules *m) {
  while (m->entries) {
    RillModule *entry = m->entries;
    m->entries = entry->next;
    free(entry->name);
    free(entry);
  }
  free(m->base);
  *m = (RillModules){};
}
