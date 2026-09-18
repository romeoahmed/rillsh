#pragma once
#include <dirent.h>
#include <errno.h>
#include <limits.h>
#include <stdlib.h>

// Both supported systems expose /dev/fd. Exclude this enumeration's own FD.
static inline int descriptor_count(int first) {
  DIR *directory = opendir("/dev/fd");
  if (!directory)
    return -1;
  int count = 0;
  for (;;) {
    errno = 0;
    struct dirent *entry = readdir(directory);
    if (!entry) {
      if (errno)
        count = -1;
      break;
    }
    if (entry->d_name[0] == '.')
      continue;
    char *end = {};
    errno = 0;
    long fd = strtol(entry->d_name, &end, 10);
    if (errno || end == entry->d_name || *end || fd < 0 || fd > INT_MAX) {
      count = -1;
      break;
    }
    if (fd >= first && fd != dirfd(directory))
      ++count;
  }
  return closedir(directory) == 0 ? count : -1;
}
