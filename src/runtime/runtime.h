/** @file
 * @brief Precise nonmoving values and resumable unary evaluation, with no OS
 * dependency.
 */
#pragma once
#include "diagnostic.h"
#include "text/text.h"
#include <stddef.h>
#include <stdint.h>
/** @brief Syntax owned by the parser and borrowed during evaluation. */
typedef struct RillSyntax RillSyntax;
/** @brief Initial value kinds; internal plan nodes are never public language
 * data. */
typedef enum {
  RILL_V_UNIT,
  RILL_V_INT,
  RILL_V_FUNCTION,
  RILL_V_JOB,
  RILL_V_STRING,
  RILL_V_BYTES,
  RILL_V_PATH,
  RILL_V_LIST,
  RILL_V_RECORD,
  RILL_V_PLAN,
  RILL_V_STAGE,
  RILL_V_REDIRECT,
  RILL_V_REPORT
} RillValueKind;
/** @brief Evaluated plan actions, independent of syntax representation. */
typedef enum {
  RILL_PLAN_INPUT,
  RILL_PLAN_OUTPUT,
  RILL_PLAN_APPEND,
  RILL_PLAN_ERROR_OUTPUT,
  RILL_PLAN_ERROR_APPEND,
  RILL_PLAN_ERROR_TO_OUTPUT
} RillPlanRedirect;
/** @brief Stable traced allocation, owned by its heap. */
typedef struct RillObject RillObject;
/**
 * @brief Tagged value; copying it neither transfers ownership nor creates a GC
 * root.
 */
typedef struct {
  RillValueKind kind; ///< Tag determines the active payload.
  union {
    int64_t integer; ///< Immediate scalar or stable native/job identifier.
    RillObject
        *object; ///< Borrowed traced payload; retain a root across allocation.
  } as;          ///< Payload selected by kind; heap owns object storage.
} RillValue;
/**
 * @brief Heap-owned allocation containing traced Values and immutable bytes.
 *
 * The collector owns the links and mark state. Neither payload view may be
 * freed separately.
 */
struct RillObject {
  RillObject *next;  ///< Private collector ownership chain.
  RillObject *gray;  ///< Private intrusive marking worklist.
  bool marked;       ///< Private mark bit.
  size_t allocation; ///< Retained allocation size.
  RillBytes bytes; ///< NUL-terminated view into this object's trailing storage.
  int64_t tag;     ///< Internal redirection or report metadata.
  size_t count;    ///< Number of traced values.
  RillValue values[]; ///< Values traced as outgoing references.
};
/**
 * @brief Stack-owned root frame borrowing initialized Values.
 *
 * Pop in reverse registration order before the frame or Values leave scope.
 */
typedef struct RillRoot {
  struct RillRoot *previous; ///< Previous frame.
  RillValue *values;         ///< Borrowed contiguous root values.
  size_t count;              ///< Number of initialized Values to trace.
} RillRoot;
/** @brief Owned heap; allocation may collect, never moves live objects. */
typedef struct {
  RillObject *objects; ///< Collector list.
  RillRoot *roots;     ///< Borrowed root chain.
  size_t bytes;        ///< Live allocated bytes after collection.
  size_t threshold;    ///< Next normal collection threshold.
  bool stress;         ///< Collect at every allocation for verification.
} RillHeap;
/**
 * @brief Allocate an object and copy its Values and bytes.
 *
 * May collect before copying: root every heap object backing either input.
 * Input pointers may be null only when their corresponding count is zero.
 * Root the returned object before the next allocating operation.
 * @pre kind denotes a traced object; input Values are initialized.
 * @return The requested object, or Unit on size overflow or allocation failure.
 */
[[nodiscard]] RillValue rill_runtime_object(RillHeap *heap, RillValueKind kind,
                                            const RillValue *values,
                                            size_t count, const char *data,
                                            size_t size, int64_t tag);
/** @brief Collect unreachable objects using only registered roots. */
void rill_runtime_collect(RillHeap *heap);
/** @brief Destroy the heap; no OS resources or finalizers run. */
void rill_runtime_heap_clear(RillHeap *heap);
/** @brief Whether a value has a traced heap payload. */
bool rill_runtime_is_object(RillValue value);
/** @brief Push a stack-owned root frame; no allocation. */
void rill_runtime_root(RillHeap *heap, RillRoot *root, RillValue *values,
                       size_t count);
/**
 * @brief Pop a root frame.
 *
 * @pre root is the most recently registered frame on this heap.
 */
void rill_runtime_unroot(RillHeap *heap, RillRoot *root);
/** @brief Native callable metadata; IDs are interpreted by the library adapter.
 */
typedef struct {
  const char *name; ///< Borrowed static binding name.
  int64_t id;       ///< Library-owned dispatch identifier.
} RillNative;
/** @brief Opaque evaluator with explicit continuations and rooted bindings. */
typedef struct RillEval RillEval;
/** @brief Cooperative evaluator event; native effects are resumed by the host.
 */
typedef enum {
  RILL_EVAL_DONE,
  RILL_EVAL_NATIVE,
  RILL_EVAL_ERROR,
  RILL_EVAL_YIELD
} RillEvalState;
/** @brief Evaluation result or rooted request valid until the next evaluator
 * call. */
typedef struct {
  RillEvalState state; ///< Scheduling outcome.
  int64_t native;      ///< Function identifier for a unary request.
  RillValue
      value; ///< Argument or completed value, borrowed from evaluator roots.
  RillDiagnostic diagnostic; ///< Failure with source offset.
} RillEvalEvent;
/**
 * @brief Create an evaluator, borrowing native metadata for its lifetime.
 *
 * @return An owned evaluator, or nullptr on allocation failure.
 */
[[nodiscard]] RillEval *rill_runtime_new(const RillNative *natives,
                                         size_t count);
/** @brief Access the heap for adapter allocations with explicit roots. */
RillHeap *rill_runtime_heap(RillEval *eval);
/**
 * @brief Discard unfinished work and begin an entry.
 *
 * Syntax must be complete and remain alive until evaluation completes or is
 * aborted. Previously committed bindings survive.
 */
void rill_runtime_begin(RillEval *eval, const RillSyntax *syntax);
/**
 * @brief Execute a bounded quantum and return a rooted result or host request.
 *
 * The host must resume a native request before evaluation can advance.
 */
RillEvalEvent rill_runtime_step(RillEval *eval);
/**
 * @brief Complete an outstanding native request with a value or diagnostic.
 *
 * A non-OK diagnostic takes precedence over the value. This call does not
 * allocate.
 */
void rill_runtime_resume(RillEval *eval, RillValue value,
                         RillDiagnostic diagnostic);
/** @brief Abort pending work; earlier committed entries remain rooted. */
void rill_runtime_abort(RillEval *eval);
/** @brief Release evaluator frames, bindings and heap; no OS cleanup. */
void rill_runtime_free(RillEval *eval);
