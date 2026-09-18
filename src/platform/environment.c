#include "diagnostic.h"
#include "posix.h"
#include <errno.h>
#include <fcntl.h>
#include <stdckdint.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
static size_t index_of(const RillEnvironment *e, const char *name, size_t n) {
  for (size_t i = 0; i < e->count; ++i)
    if (!strncmp(e->entries[i], name, n) && e->entries[i][n] == '=')
      return i;
  return e->count;
}
const char *rill_platform_env_get(const RillEnvironment *e, const char *name) {
  size_t i = index_of(e, name, strlen(name));
  return i < e->count ? strchr(e->entries[i], '=') + 1 : nullptr;
}
bool rill_platform_env_set(RillEnvironment *e, const char *name,
                           const char *value) {
  if (!*name || strchr(name, '=')) {
    errno = EINVAL;
    return false;
  }
  size_t n = strlen(name);
  size_t i = index_of(e, name, n);
  if (!value) {
    if (i < e->count) {
      free(e->entries[i]);
      memmove(e->entries + i, e->entries + i + 1,
              (e->count - i) * sizeof(char *));
      --e->count;
    }
    return true;
  }
  size_t value_size = strlen(value), length = {};
  if (ckd_add(&length, n, value_size) || ckd_add(&length, length, 2)) {
    errno = ENOMEM;
    return false;
  }
  char *entry = malloc(length);
  if (!entry)
    return false;
  // The name is a prefix; the value copy below writes the final terminator.
  // NOLINTNEXTLINE(bugprone-not-null-terminated-result)
  memcpy(entry, name, n);
  entry[n] = '=';
  memcpy(entry + n + 1, value, value_size + 1);
  if (i == e->count) {
    size_t bytes = {};
    if (ckd_add(&bytes, e->count, 2) ||
        ckd_mul(&bytes, bytes, sizeof(char *))) {
      free(entry);
      errno = ENOMEM;
      return false;
    }
    char **entries = realloc(e->entries, bytes);
    if (!entries) {
      free(entry);
      return false;
    }
    e->entries = entries;
    e->entries[++e->count] = nullptr;
  } else
    free(e->entries[i]);
  e->entries[i] = entry;
  return true;
}
bool rill_platform_env_init(RillEnvironment *e, char *const *entries) {
  *e = (RillEnvironment){};
  size_t count = 0;
  while (entries && entries[count])
    ++count;
  // A valid input vector already fits count entries plus its sentinel.
  e->entries = calloc(count + 1, sizeof(*e->entries));
  if (!e->entries)
    return false;
  for (size_t i = 0; i < count; ++i) {
    const char *eq = strchr(entries[i], '=');
    if (!eq || eq == entries[i])
      continue;
    size_t n = (size_t)(eq - entries[i]);
    if (index_of(e, entries[i], n) < e->count)
      continue;
    char *entry = strdup(entries[i]);
    if (!entry)
      goto fail;
    e->entries[e->count++] = entry;
  }
  return true;
fail:
  rill_platform_env_clear(e);
  return false;
}
void rill_platform_env_clear(RillEnvironment *e) {
  for (size_t i = 0; i < e->count; ++i)
    free(e->entries[i]);
  free(e->entries);
  *e = (RillEnvironment){};
}
bool rill_platform_cd(RillEnvironment *e, const char *path,
                      RillDiagnostic *error) {
  RillEnvironment next = {};
  [[gnu::cleanup(rill_platform_close)]] int fd =
      rill_platform_internal(open(".", O_RDONLY | O_DIRECTORY | O_CLOEXEC));
  if (fd < 0)
    goto fail;
  char *old = getcwd(nullptr, 0);
  if (!old && errno == ENOMEM)
    goto fail;
  // Prepare a separate map so a failed directory transaction cannot publish it.
  if (!rill_platform_env_init(&next, e->entries)) {
    free(old);
    goto fail;
  }
  if (chdir(path) < 0) {
    free(old);
    goto fail;
  }
  char *cwd = getcwd(nullptr, 0);
  if (!cwd || !rill_platform_env_set(&next, "PWD", cwd) ||
      !rill_platform_env_set(&next, "OLDPWD", old)) {
    int saved = errno;
    if (fchdir(fd) < 0) {
      bool unset = rill_platform_env_set(e, "PWD", nullptr);
      (void)unset;
      *error = (RillDiagnostic){
          .kind = RILL_IO,
          .code = errno,
          .message =
              "cd bookkeeping failed and rollback failed; PWD invalidated"};
    } else
      *error = (RillDiagnostic){
          .kind = RILL_IO,
          .code = saved,
          .message = "cd bookkeeping failed; directory restored"};
    free(cwd);
    free(old);
    rill_platform_env_clear(&next);
    return false;
  }
  free(cwd);
  free(old);
  rill_platform_env_clear(e);
  *e = next;
  return true;
fail:
  *error = (RillDiagnostic){
      .kind = RILL_IO, .code = errno, .message = "cannot change directory"};
  rill_platform_env_clear(&next);
  return false;
}
