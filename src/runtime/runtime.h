/**
 * @file
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
  RILL_V_BUDGET,
  RILL_V_STREAM
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
 * The collector owns links, mark state, and allocation lifetime. The kind
 * selects union members and any separately owned storage; callers must not
 * free payload views.
 */
struct RillObject {
  RillObject *next; ///< Private collector ownership chain.
  RillObject
      *gray; ///< Temporary intrusive link for mutually exclusive graph walks.
  RillValueKind
      kind;      ///< Allocation kind, including during tracing and disposal.
  bool marked;   ///< Private last-reached collection epoch.
  bool visiting; ///< Private temporary reachability mark; clear outside walks.
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
 * @brief Caller-owned root registration borrowing initialized Values.
 *
 * Unregister before the frame or Values leave scope. Stable asynchronous owners
 * may unregister independently of evaluator frame order.
 * Tracing borrows a read-only view; the owner may update initialized slots.
 */
typedef struct RillRoot {
  struct RillRoot *previous; ///< Older registered root.
  struct RillRoot *next;     ///< Newer registered root.
  const RillValue *values;   ///< Borrowed contiguous values.
  size_t count;              ///< Number of initialized Values to trace.
} RillRoot;
/** @brief Owned heap; allocation may collect, never moves live objects. */
typedef struct {
  RillObject *objects; ///< Collector list.
  RillRoot *roots;     ///< Borrowed root chain.
  size_t scoped;       ///< Allocated scoped tokens, including unreachable ones.
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
/**
 * @brief Register initialized Values without allocating.
 *
 * The root and Value storage must stay at stable addresses until unregistered.
 * A root must not already be registered, on this heap or another.
 */
void rill_runtime_root(RillHeap *heap, RillRoot *root, const RillValue *values,
                       size_t count);
/**
 * @brief Unregister a root independently of registration order.
 *
 * @pre root is registered on this heap.
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
  RILL_EVAL_MODULE,
  RILL_EVAL_CALLBACK,
  RILL_EVAL_STREAM,
  RILL_EVAL_CLEANUP
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
  int64_t native; ///< Native function ID or CLEANUP resource checkpoint.
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
/** @brief Reserve a monotonic resource identity; zero means exhaustion. */
int64_t rill_runtime_resource_id(RillEval *eval);
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
 * Native/import/module/callback/cleanup requests require host service before
 * evaluation advances. The step limit does not bound collection or individual
 * operation latency.
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
/**
 * @brief Discard pending work and diagnostics, then collect.
 *
 * Committed entries remain rooted. Borrowed event views become invalid.
 */
void rill_runtime_abort(RillEval *eval);
/** @brief Free frames, bindings, and heap; nullptr is allowed. No OS cleanup.
 */
void rill_runtime_free(RillEval *eval);

/** @brief Number of elements in a List or shared List slice. */
size_t rill_runtime_count(RillValue list);
/**
 * @brief Return an element from a List or shared slice without allocating.
 *
 * The index must be below rill_runtime_count(). The copy is not a GC root;
 * retain its owner or register it before a later allocation.
 */
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
 * Objects referenced by the result remain heap-owned; copying a Value does
 * not register a root.
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
 * @brief Copy a finalized Record into private storage, preserving key order.
 *
 * May collect; root the input and retain a root for the result before further
 * allocation. Returns Unit on allocation failure. The copy is already indexed;
 * only its field values may change before publication, never keys or count.
 */
RillValue rill_runtime_record_copy(RillHeap *heap, RillValue record);
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
 * @brief Return the current entry's rooted export namespace, empty when
 * omitted.
 *
 * May collect. Returns Unit and records an evaluator error on failure.
 * Retain a separate root before advancing evaluation if the namespace is
 * needed.
 */
RillValue rill_runtime_exports(RillEval *eval);
/** @brief Current continuation count, for bounded-space verification. */
size_t rill_runtime_depth(const RillEval *eval);

/**
 * @brief Freeze exported bindings (or all bindings without an export table).
 *
 * May collect while constructing the immutable environment. False records an
 * allocation error; the caller must not execute user code after failure.
 */
[[nodiscard]] bool rill_runtime_prelude(RillEval *eval);
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

/**
 * @brief Invoke a language callback for a suspended native operation.
 *
 * Arguments must be rooted. The result is returned as Result in a CALLBACK
 * event; cancellation and allocation failure remain uncatchable. The original
 * native request remains suspended until the host resumes it.
 */
void rill_runtime_callback(RillEval *eval, RillValue function,
                           RillValue argument);
/**
 * @brief Check a rooted graph for scoped handles, including closure captures.
 *
 * Handles sharing and cycles without allocation, collection, or callbacks.
 */
RillError rill_runtime_persistent(RillHeap *heap, RillValue value);

/** @brief Owned suspended continuation, rooted in its original evaluator heap.
 */
typedef struct RillEvaluation RillEvaluation;
/**
 * @brief Detach active work without publishing it.
 *
 * The saved context remains rooted in this evaluator's heap and must be
 * restored or discarded before freeing the evaluator. Allocation failure
 * returns nullptr and leaves active work unchanged. No OS cleanup occurs.
 */
RillEvaluation *rill_runtime_suspend(RillEval *eval);
/** @brief Restore and consume suspended work; the evaluator must have no active
 * work. */
void rill_runtime_restore(RillEval *eval, RillEvaluation *evaluation);
/** @brief Discard suspended work and unregister roots; never performs OS
 * cleanup. */
void rill_runtime_discard(RillEval *eval, RillEvaluation *evaluation);

/** @brief Charge non-object retained storage to a budget; no allocation or
 * collection. */
RillError rill_runtime_charge_storage(RillValue budget, size_t bytes);
