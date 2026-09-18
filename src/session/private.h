/**
 * @file
 * @brief Session composition and the private evaluation coordinator.
 */
#pragma once
#include "exec/exec.h"
#include "module.h"
#include "native/native.h"
#include "platform/posix.h"
#include "runtime/runtime.h"
#include "session.h"
#include "source.h"
#include "syntax/syntax.h"
#include "text/style.h"
#include <stddef.h>
#include <stdint.h>
typedef struct {
  RillPlatform platform;
  RillEnvironment env;
  RillExec *exec;
  RillEval *eval;
  RillLibrary library;
  RillModules modules;
  RillColorMode color;
  bool interactive;
  int status;
  struct Context *contexts, *callers;
  RillNativePending pending;
  int64_t next_context, active_id, foreground_request;
} Session;

bool rill_session_write(int fd, const char *data, size_t size);
int rill_session_memory(Session *session);
int rill_session_diagnostic(Session *session, const RillSource *source,
                            RillDiagnostic diagnostic);
int rill_session_diagnostic_at(Session *session, const char *name,
                               RillBytes bytes, RillDiagnostic diagnostic);
unsigned rill_session_events(Session *session);
int rill_session_evaluate(Session *session, RillSource *source,
                          RillSyntax *syntax);
void rill_session_context_services(Session *session);
void rill_session_context_clear(Session *session);
