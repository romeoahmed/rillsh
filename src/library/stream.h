/**
 * @file
 * @brief Scoped single-consumer streams and cooperative source/transform/sink
 * work.
 */
#pragma once
#include "exec/exec.h"
#include "library.h"
#include "runtime/runtime.h"
#include <stdint.h>
/** @brief Owned resource scope; no OS resource depends on garbage collection.
 */
typedef struct RillStreams RillStreams;
/**
 * @brief Dispatch a rooted private stream request; may collect or suspend.
 *
 * True permits evaluator progress. False retains pending work in the scope;
 * advance it with rill_stream_progress(). Filesystem operations may block.
 */
bool rill_stream_call(RillLibrary *library, RillValue request);
/** @brief Close resources newer than a checkpoint, then resume with result.
 *
 * May suspend for reaping; retains a root for result until completion. */
void rill_stream_unwind(RillLibrary *library, int64_t checkpoint,
                        RillValue result);
/**
 * @brief Advance one stream quantum, allowing evaluation when true.
 *
 * False leaves stream work pending. library->idle distinguishes waiting for
 * I/O or reaping from work that can advance again without a blocking poll.
 * A true result may request a callback rather than finish the original call.
 */
bool rill_stream_progress(RillLibrary *library);
/** @brief Deliver the Result of a requested language callback. */
void rill_stream_callback(RillLibrary *library, RillValue result);
/** @brief Cancel remaining scope resources without blocking or allocating. */
void rill_stream_cancel(RillLibrary *library);
/** @brief Whether scope-owned jobs still require reaping. */
bool rill_stream_live(const RillLibrary *library);
/** @brief Release a cleaned scope and its roots; all owned jobs must be reaped.
 */
void rill_stream_clear(RillLibrary *library);

/** @brief Forward cancellation or stop to every process owned by the active
 * scope. */
void rill_stream_signal(RillLibrary *library, unsigned events);
/** @brief Whether any scope-owned process group is stopped. */
bool rill_stream_stopped(const RillLibrary *library);
/** @brief Whether every scope-owned group has stopped or reaped its children.
 */
bool rill_stream_quiescent(const RillLibrary *library);
/** @brief Resume all scope-owned groups without terminal handoff. */
bool rill_stream_resume(RillLibrary *library);
/** @brief Whether a scope retains the given supervisor-owned job. */
bool rill_stream_owns(const RillStreams *streams, const RillJob *job);
