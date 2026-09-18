/**
 * @file
 * @brief Test-owned supervisor cleanup for unreaped helper children.
 */
#pragma once
#include "exec/exec.h"
#include "platform/posix.h"
#include <signal.h>
#include <stddef.h>
#include <stdint.h>

// Failure cleanup owns only unreaped direct children. Test helpers create no
// descendants; SIGKILL avoids depending on their cooperative shutdown behavior.
static inline bool supervisor_cleanup(RillExec *exec) {
  if (!exec)
    return true;
  for (RillJob *job = rill_exec_first(exec); job; job = rill_exec_next(job)) {
    size_t count = {};
    const RillExecStatus *status = rill_exec_status(job, &count);
    for (size_t i = 0; i < count; ++i)
      if (status[i].pid > 0 && !status[i].done)
        (void)kill(status[i].pid, SIGKILL);
  }
  int64_t deadline = rill_platform_now() + 3000;
  while (rill_exec_outstanding(exec, false) && rill_platform_now() < deadline)
    (void)rill_exec_poll(exec, 10, -1);
  if (rill_exec_outstanding(exec, false))
    return false;
  rill_exec_free(exec);
  return true;
}
