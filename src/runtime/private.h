#pragma once
#include "diagnostic.h"
#include "runtime.h"
#include "syntax/syntax.h"
#include <stddef.h>
// Allocation and finalization must reserve the same Record index storage.
static constexpr size_t RECORD_INDEX_MIN_SLOTS = 32;
enum {
  GLOBAL,
  ENTRY,
  RESULT,
  CODE,
  EXPORTS,
  TYPES,
  PRELUDE,
  MODULES,
  RAISED,
  ERROR_CODE,
  ROOT_COUNT
};
enum { ENV, OWNER, OPERANDS };
enum { CALL_WAIT = 100, ATTEMPT_WAIT, MATCH_GUARD, IMPORT_WAIT, MODULE_WAIT };
// Slots ENV and OWNER retain lexical scope and code; OPERANDS begins the
// initialized operand prefix registered by root. Unused capacity is untraced.
typedef struct Frame {
  struct Frame *parent;
  const RillNode *node, *next, *arm;
  size_t used, capacity;
  unsigned phase;
  RillRoot root;
  RillValue values[];
} Frame;
struct RillEval {
  RillHeap heap;
  const RillNative *natives;
  size_t native_count, depth;
  Frame *frame, *spare_frames;
  size_t spare_count;
  const RillNode *statement;
  RillValue roots[ROOT_COUNT];
  RillRoot root;
  RillDiagnostic error;
  bool waiting, ready;
};
typedef struct Bound {
  struct Bound *parent;
  const char *name;
} Bound;

void rill_eval_error(RillEval *e, RillError kind, const char *message);
RillValue rill_eval_object(RillEval *e, RillValueKind kind,
                           const RillValue *values, size_t count,
                           const char *data, size_t size);
RillValue rill_eval_text(RillEval *e, const char *data, size_t size);
bool rill_eval_lookup_env(RillValue env, const char *name, RillValue *out);
bool rill_eval_lookup(RillEval *e, RillValue env, const char *name,
                      RillValue *out);
RillValue rill_eval_compact(RillEval *e, RillValue env);
RillValue rill_eval_scope(RillEval *e, RillValue parent);
RillValue rill_eval_bind(RillEval *e, RillValue env, const char *name,
                         RillValue value, bool duplicate);
RillValue rill_eval_closure(RillEval *e, const RillNode *node, RillValue env,
                            RillValue code);
RillValue rill_eval_literal(const RillNode *node, RillValue code);
bool rill_eval_bind_pattern(RillEval *e, const RillNode *pattern,
                            RillValue value, RillValue lexical, RillValue code,
                            RillValue *out);
bool rill_eval_pattern_names(RillEval *e, const RillNode *pattern,
                             Bound **names);
void rill_eval_names_free(Bound *names);

// Code owns analysis; names borrow its syntax, never a closure environment.
typedef struct Captures {
  const RillNode *node;
  const char **names;
  size_t count, capacity;
  RillError error;
  const char *message;
} Captures;
typedef struct RillCode {
  RillSyntax syntax;
  RillNode module;
  size_t capacity;
  Captures functions[];
} RillCode;
void rill_eval_analyze(RillEval *eval, Captures *captures);
RillValue rill_eval_code(RillEval *eval, RillSyntax *syntax);
void rill_eval_code_free(RillCode *code);
Captures *rill_eval_captures(RillCode *code, const RillNode *node);
RillValue rill_eval_constant(RillValue code, const RillNode *node);
