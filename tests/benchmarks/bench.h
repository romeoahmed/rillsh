/**
 * @file
 * @brief Monotonic timing for isolated benchmark intervals.
 */
#pragma once
#include "../unit/test.h"
#include <stdint.h>
#include <time.h>

static inline uint64_t nanoseconds() {
  struct timespec now = {};
  CHECK(clock_gettime(CLOCK_MONOTONIC, &now) == 0);
  return (uint64_t)now.tv_sec * UINT64_C(1000000000) + (uint64_t)now.tv_nsec;
}
