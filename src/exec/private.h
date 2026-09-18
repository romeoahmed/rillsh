/**
 * @file
 * @brief Supervisor-owned child state, launch records, and byte queues.
 */
#pragma once
#include "diagnostic.h"
#include "exec.h"
#include "platform/posix.h"
#include "text/text.h"
#include <stddef.h>
#include <stdint.h>
#include <sys/types.h>
#include <termios.h>
typedef struct {
  size_t stage;
  int code;
} LaunchError;
struct RillJob {
  RillJob *next;
  size_t id, count;
  pid_t group;
  pid_t ungrouped; // Last forked child if group registration failed.
  RillExecStatus *stages;
  bool *connected;
  RillJobState state;
  bool streaming, data, feed_end, cutoff;
  int relay;
  bool background, acknowledged, handed, has_modes, cancelled, killed, retained;
  struct termios modes;
  int64_t deadline;
  int errors, input, output[2];
  size_t error_used;
  LaunchError wire_error;
  RillBuffer feed, captured[2];
  size_t fed, limit;
  RillDiagnostic error;
  char error_text[160]; // Bounded numeric limit diagnostics; no error-path
                        // allocation.
};
struct RillExec {
  RillPlatform *platform;
  RillJob *jobs, *attached;
  size_t next_id, count;
  struct pollfd *polls;
};
void rill_exec_job_destroy(RillJob *j);
bool rill_exec_job_live(const RillJob *j);
