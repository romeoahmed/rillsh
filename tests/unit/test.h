/**
 * @file
 * @brief Always-active assertions that preserve process cleanup handlers.
 */
#pragma once
#include <stdio.h>
#include <stdlib.h>
// Evaluate once even under NDEBUG. exit reaches atexit cleanup without
// intercepting sanitizer signals or unwinding through longjmp.
#define CHECK(condition)                                                       \
  do {                                                                         \
    if (!(condition)) {                                                        \
      (void)fprintf(stderr, "%s:%d: %s\n", __FILE__, __LINE__, #condition);    \
      exit(EXIT_FAILURE);                                                      \
    }                                                                          \
  } while (0)
