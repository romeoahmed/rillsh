/** @file
 * @brief Traced values and resumable unary evaluation, independent of OS
 * services.
 */
#pragma once
#include "diagnostic.h"
#include "text/text.h"
#include <stddef.h>
#include <stdint.h>
/** @brief Owned parse result consumed when an entry or module begins. */
typedef struct RillSyntax RillSyntax;
/** @brief Language values and internal traced representations. */
typedef enum {
  RILL_V_UNIT,
  RILL_V_INT,
  RILL_V_FUNCTION,
  RILL_V_JOB,
  RILL_V_BOOL,
  RILL_V_NULL,
  RILL_V_FLOAT,
  RILL_V_STRING,
  RILL_V_BYTES,
  RILL_V_PATH,
  RILL_V_LIST,
  RILL_V_RECORD,
  RILL_V_PLAN,
  RILL_V_STAGE,
  RILL_V_REDIRECT,
  RILL_V_CLOSURE,
  RILL_V_CODE,
  RILL_V_ENV,
  RILL_V_BINDINGS,
  RILL_V_CELL,
  RILL_V_DESCRIPTOR,
  RILL_V_CONSTRUCTOR,
  RILL_V_ADT,
  RILL_V_SLICE,
  RILL_V_BUDGET
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
    double real;     ///< Finite binary64 scalar.
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
  RillObject *next; ///< Private collector ownership chain.
  RillObject *gray; ///< Private intrusive marking worklist.
  RillValueKind
      kind;    ///< Allocation kind, including during tracing and disposal.
  bool marked; ///< Private last-reached collection epoch.
  size_t allocation; ///< Bytes charged to the heap, including owned storage.
  size_t count;      ///< Number of traced values.
  union {
    struct {
      RillBytes bytes; ///< Text, environment/descriptor name, or binding names.
      const char **names; ///< Interior name index, only for committed bindings.
    };
    struct {
      union {
        struct RillCode *code; ///< Owned syntax and analysis, only for code.
        struct RillBudget *budget; ///< Retained-graph budget, only for budgets.
        size_t extent; ///< Logical length, only for shared List slices.
      };
      int64_t
          tag; ///< Code origin, budget key kind, slice offset, or redirection.
    };
    const struct Captures *captures; ///< Closure layout, retained by code.
    RillValue **fields; ///< Sorted interior key pointers, only for Records.
    RillValue metadata; ///< Traced policy, only for stages.
  }; ///< Kind selects the active member; never inspect other views.
  RillValue values[]; ///< Values traced as outgoing references.
};
/**
 * @brief Stack-owned root frame borrowing initialized Values.
 *
 * Pop in reverse registration order before the frame or Values leave scope.
 * Tracing borrows a read-only view; the owner may update initialized slots.
 */
typedef struct RillRoot {
  struct RillRoot *previous; ///< Previous frame.
  const RillValue *values;   ///< Borrowed contiguous values.
  size_t count;              ///< Number of initialized Values to trace.
} RillRoot;
/** @brief Owned heap; allocation may collect, never moves live objects. */
typedef struct {
  RillObject *objects; ///< Collector list.
  RillRoot *roots;     ///< Borrowed root chain.
  size_t bytes;        ///< Charged bytes; includes garbage until collection.
  size_t threshold;    ///< Next normal collection threshold.
  bool epoch;  ///< Alternates each full collection; survivors need no reset.
  bool stress; ///< Collect at every allocation for verification.
} RillHeap;
/**
 * @brief Allocate copied bytes and copied or Unit-initialized Values.
 *
 * May collect before copying: root every heap object backing either input.
 * A null values pointer initializes count Unit slots for a rooted builder.
 * The byte pointer may be null only when size is zero. Fill new slots before
 * publication; root the result across any further allocation. Supplied Record
 * arrays have unique String keys; finalize Record builders before publication.
 * Bytes are supported by String, Bytes, Path, ENV, BINDINGS, and DESCRIPTOR.
 * Other kinds require size zero. Only CODE, BUDGET, SLICE, and REDIRECT use
 * tag.
 * @pre kind denotes a traced object; supplied input Values are initialized.
 * @return The requested object, or Unit on size overflow or allocation failure.
 */
[[nodiscard]] RillValue rill_runtime_object(RillHeap *heap, RillValueKind kind,
                                            const RillValue *values,
                                            size_t count, const char *data,
                                            size_t size, int64_t tag);
/** @brief Collect unreachable objects using only registered roots. */
void rill_runtime_collect(RillHeap *heap);
/** @brief Free all objects and reset the heap, invalidating every root and
 * view. */
void rill_runtime_heap_clear(RillHeap *heap);
/** @brief Whether a value has a traced heap payload. */
bool rill_runtime_is_object(RillValue value);
/** @brief Push a stack-owned root frame; no allocation. */
void rill_runtime_root(RillHeap *heap, RillRoot *root, const RillValue *values,
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
  int64_t id;       ///< Nonnegative library-owned dispatch identifier.
} RillNative;
/** @brief Opaque evaluator with explicit continuations and rooted bindings. */
typedef struct RillEval RillEval;
/** @brief Cooperative evaluator event; native effects are resumed by the host.
 */
typedef enum {
  RILL_EVAL_DONE,
  RILL_EVAL_NATIVE,
  RILL_EVAL_ERROR,
  RILL_EVAL_YIELD,
  RILL_EVAL_IMPORT,
  RILL_EVAL_MODULE
} RillEvalState;
/**
 * @brief Rooted result or request borrowed from the evaluator.
 *
 * Consume source and diagnostic views before advancing, resuming, or aborting.
 * Root any Value that must survive those operations and later allocation.
 */
typedef struct {
  const char *source;     ///< Borrowed source identity.
  RillBytes source_bytes; ///< Borrowed source for diagnostic coordinates.
  RillEvalState state;    ///< Scheduling outcome.
  int64_t native;         ///< Function identifier for a unary request.
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
 * Syntax is consumed and reset, including on failure. Traced code retains
 * source and nodes for reachable closures. Committed bindings survive.
 */
void rill_runtime_begin(RillEval *eval, RillSyntax *syntax);
/**
 * @brief Execute a step-limited quantum and return a result or host request.
 *
 * Native/import/module requests require host service before evaluation
 * advances. The step limit does not bound collection or individual operation
 * latency.
 */
RillEvalEvent rill_runtime_step(RillEval *eval);
/**
 * @brief Complete an outstanding host request with a value or diagnostic.
 *
 * A non-OK diagnostic takes precedence over the value. This call does not
 * allocate.
 */
void rill_runtime_resume(RillEval *eval, RillValue value,
                         RillDiagnostic diagnostic);
/** @brief Discard pending work and collect; committed entries remain rooted. */
void rill_runtime_abort(RillEval *eval);
/** @brief Free frames, bindings, and heap; nullptr is allowed. No OS cleanup.
 */
void rill_runtime_free(RillEval *eval);

/** @brief Number of elements in a List or shared List slice. */
size_t rill_runtime_count(RillValue list);
/** @brief Borrow an element; index must be below rill_runtime_count(). */
RillValue rill_runtime_at(RillValue list, size_t index);
/**
 * @brief Share a List range, reusing full ranges and detaching empty ones.
 *
 * May allocate and collect. The input must be rooted and the bounds valid.
 * Returns Unit on allocation failure; root the result before further
 * allocation.
 */
RillValue rill_runtime_slice(RillHeap *heap, RillValue list, size_t start,
                             size_t count);
/**
 * @brief Borrow a field from a Record or nominal payload without allocating.
 *
 * False leaves out unchanged when the key is absent or the value has no fields.
 */
bool rill_runtime_field(RillValue value, RillBytes key, RillValue *out);
/**
 * @brief Copy alternating String keys and values into a Record.
 *
 * count is the even number of Value slots, not fields. Root all inputs across
 * this allocation; keys must be unique. A null pairs pointer creates a private
 * Unit-filled builder that must be filled and finalized before publication.
 * Returns Unit on allocation failure; root the result before further
 * allocation.
 */
RillValue rill_runtime_record(RillHeap *heap, const RillValue *pairs,
                              size_t count);
/**
 * @brief Validate and index a private Record builder without a GC safepoint.
 *
 * Keys must be Strings and unique. Call after filling or shrinking the value
 * array, before publication. Values may subsequently change only during private
 * construction; published records are immutable. False rejects invalid keys.
 */
[[nodiscard]] bool rill_runtime_record_finish(RillValue record);
/**
 * @brief Find a key slot in a finalized Record, or its slot count if absent.
 *
 * The field value follows the returned key slot. No allocation occurs.
 */
size_t rill_runtime_field_index(RillValue record, RillBytes key);

/**
 * @brief Compare data structurally after validating both complete graphs.
 *
 * May allocate scratch storage but cannot collect or invoke language code.
 * Read equal only on RILL_OK; unsupported data, depth, and allocation failures
 * return RILL_TYPE, RILL_LIMIT, or RILL_MEMORY.
 */
RillError rill_runtime_equal(RillValue left, RillValue right, bool *equal);
/**
 * @brief Copy a name and bind a rooted value in the committed environment.
 *
 * May collect. False preserves existing bindings and records an evaluator
 * error.
 */
bool rill_runtime_define(RillEval *eval, const char *name, RillValue value);
/** @brief Borrow a committed binding, including registered native functions. */
bool rill_runtime_lookup(RillEval *eval, const char *name, RillValue *value);
/**
 * @brief Materialize the current entry's exports in the evaluator result root.
 *
 * May collect. Returns Unit and records an evaluator error on failure.
 * Retain a separate root before advancing evaluation if the namespace is
 * needed.
 */
RillValue rill_runtime_exports(RillEval *eval);
/** @brief Current continuation count, for bounded-space verification. */
size_t rill_runtime_depth(const RillEval *eval);

/** @brief Freeze the current committed bindings as the environment of new
 * modules. */
void rill_runtime_prelude(RillEval *eval);
/**
 * @brief Consume and reset module syntax for the outstanding import.
 *
 * Caller continuations survive; syntax is consumed even on failure.
 */
void rill_runtime_module(RillEval *eval, RillSyntax *syntax);

/**
 * @brief Retain a completed namespace for the evaluator lifetime.
 *
 * Roots value internally across allocation. False records an evaluator error.
 */
bool rill_runtime_retain_module(RillEval *eval, RillValue value);

/**
 * @brief Borrow a prelude binding, unaffected by later user shadowing.
 *
 * During bootstrap, before freezing the prelude, searches the current entry.
 */
bool rill_runtime_builtin(RillEval *eval, const char *name, RillValue *value);

/**
 * @brief Allocate a private retained-byte budget; Unit on allocation failure.
 *
 * May collect. Root the result before further allocation.
 */
RillValue rill_runtime_budget(RillHeap *heap, size_t limit);
/**
 * @brief Retain and charge previously unseen backing objects, including code.
 *
 * No GC safepoint occurs. The rooted budget keeps charged objects alive and
 * counts shared storage once across calls. Discard the budget after an error.
 */
RillError rill_runtime_charge(RillHeap *heap, RillValue budget,
                              RillValue value);
