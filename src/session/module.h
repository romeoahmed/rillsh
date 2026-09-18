/**
 * @file
 * @brief Session-owned module identity cache and in-flight import stack.
 */
#pragma once
#include "runtime/runtime.h"
/** @brief Private module cache entry. */
typedef struct RillModule RillModule;
/** @brief Owned cache metadata; namespaces are retained by the runtime. */
typedef struct {
  RillModule *entries; ///< Completed and active file identities.
  RillModule *active;  ///< Import stack, for cycle detection and completion.
  char *base;          ///< Entry working directory, owned until the next entry.
} RillModules;
/**
 * @brief Service the evaluator's current import or module-completion request.
 *
 * May block on filesystem I/O and collect during runtime allocation. Resumes
 * the evaluator with cached exports, new module syntax, or an error; does not
 * run evaluation recursively. The cache belongs to this evaluator's session.
 */
void rill_module_event(RillModules *modules, RillEval *eval,
                       RillEvalEvent event);
/** @brief Remove failed in-flight imports; completed identities remain cached.
 */
void rill_module_abort(RillModules *modules);
/** @brief Release cache metadata; no runtime Values are freed here. */
void rill_module_clear(RillModules *modules);
