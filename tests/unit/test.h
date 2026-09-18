#pragma once
#include <stdio.h>
#include <stdlib.h>
// Always active, evaluates once, and reaches atexit cleanup on failure.
// Sanitizer signal handling remains intact; no longjmp bypasses cleanup.
#define CHECK(condition)                                                       \
  do {                                                                         \
    if (!(condition)) {                                                        \
      (void)fprintf(stderr, "%s:%d: %s\n", __FILE__, __LINE__, #condition);    \
      exit(1);                                                                 \
    }                                                                          \
  } while (0)
