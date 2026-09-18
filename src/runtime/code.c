/**
 * @file
 * @brief Transfer parsed syntax into traced, reusable code.
 *
 * Prepare exact free-name layouts and a code-local pool of immutable Strings.
 * Closures retain this owner; escaped literals do not retain code. Preparation
 * runs no user code and defers pattern errors until their expression is
 * reached.
 */
#include "diagnostic.h"
#include "private.h"
#include "runtime.h"
#include "syntax/syntax.h"
#include "text/text.h"
#include <stdckdint.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>

static bool constant(const RillNode *node) {
  return node->kind == RILL_STRING || node->kind == RILL_PAIR;
}
static int literal_order(const void *left, const void *right) {
  const RillNode *const *a = left, *const *b = right;
  RillBuffer x = (*a)->text, y = (*b)->text;
  size_t size = x.size < y.size ? x.size : y.size;
  int order = size ? memcmp(x.data, y.data, size) : 0;
  return order ? order : (x.size > y.size) - (x.size < y.size);
}
RillValue rill_eval_constant(RillValue owner, const RillNode *node) {
  return owner.as.object->values[node->constant];
}
RillValue rill_eval_code(RillEval *e, RillSyntax *syntax) {
  RillNode **literals = nullptr;
  if (syntax->allocation < sizeof(RillSyntax))
    goto failure;
  size_t count = 0, constants = 0, bytes = {}, allocation = {}, total = {};
  for (const RillNode *n = syntax->allocated; n; n = n->allocated_next) {
    if (n->kind == RILL_FUNCTION)
      ++count;
    if (syntax->state == RILL_COMPLETE && constant(n))
      ++constants;
  }
  size_t unique = 0;
  if (constants) {
    if (ckd_mul(&bytes, constants, sizeof(*literals)))
      goto failure;
    literals = malloc(bytes);
    if (!literals)
      goto failure;
    size_t i = 0;
    for (RillNode *n = syntax->allocated; n; n = n->allocated_next)
      if (constant(n))
        literals[i++] = n;
    qsort(literals, constants, sizeof(*literals), literal_order);
    // Share immutable bytes only within this code owner. No global intern table
    // or back edge may extend a literal's lifetime to unrelated code.
    for (i = 0; i < constants; ++i) {
      if (!i || literal_order(literals + i - 1, literals + i))
        ++unique;
      literals[i]->constant = unique - 1;
    }
  }
  if (ckd_mul(&bytes, count, sizeof(Captures)) ||
      ckd_add(&bytes, bytes, sizeof(RillCode)))
    goto failure;
  RillValue value =
      rill_eval_object(e, RILL_V_CODE, nullptr, unique, nullptr, 0);
  if (e->error.kind)
    goto failure;
  // Analysis uses only C storage; constant construction roots code below.
  RillCode *code = malloc(bytes);
  if (!code)
    goto failure;
  *code = (RillCode){.syntax = *syntax, .count = count};
  *syntax = (RillSyntax){};
  size_t function = 0;
  for (RillNode *n = code->syntax.allocated; n; n = n->allocated_next) {
    if (n->kind != RILL_FUNCTION)
      continue;
    // The complete function set is known; node slots need no pointer index.
    n->function = function;
    code->functions[function++] = (Captures){.node = n};
  }
  if (code->syntax.state == RILL_COMPLETE)
    for (RillNode *n = code->syntax.allocated; n; n = n->allocated_next) {
      if (n->kind != RILL_BIND && n->kind != RILL_FUNCTION &&
          n->kind != RILL_ARM)
        continue;
      Bound *names = nullptr;
      RillValue origin = e->roots[ERROR_CODE];
      bool valid = rill_eval_pattern_names(e, n->pattern, &names);
      rill_eval_names_free(names);
      if (!valid && e->error.kind == RILL_MEMORY) {
        rill_eval_code_free(code);
        goto failure;
      }
      if (n->pattern)
        n->pattern->repeated = !valid;
      // Defer diagnosis until matching, after subject/argument effects.
      // Preparation must not raise pattern errors in unexecuted branches.
      e->error = (RillDiagnostic){};
      e->roots[ERROR_CODE] = origin;
    }
  size_t extra = {};
  if (ckd_add(&extra, bytes, code->syntax.allocation - sizeof(RillSyntax))) {
    rill_eval_code_free(code);
    goto failure;
  }
  for (size_t i = 0; i < count; ++i) {
    Captures *analysis = &code->functions[i];
    if (code->syntax.state != RILL_COMPLETE)
      continue;
    rill_eval_analyze(e, code, analysis);
    size_t names = {};
    if (e->error.kind ||
        ckd_mul(&names, analysis->capacity, sizeof(*analysis->names)) ||
        ckd_add(&extra, extra, names)) {
      rill_eval_code_free(code);
      goto failure;
    }
  }
  if (ckd_add(&allocation, value.as.object->allocation, extra) ||
      ckd_add(&total, e->heap.bytes, extra)) {
    rill_eval_code_free(code);
    goto failure;
  }
  value.as.object->code = code;
  value.as.object->allocation = allocation;
  e->heap.bytes = total;
  if (code->syntax.state == RILL_COMPLETE) {
    // Complete constant edges before budgets can charge code. Strings have
    // no back edge, so an escaped literal does not retain its syntax.
    RillRoot root = {};
    rill_runtime_root(&e->heap, &root, &value, 1);
    for (size_t i = 0; i < constants && !e->error.kind; ++i) {
      RillNode *n = literals[i];
      RillValue *slot = &value.as.object->values[n->constant];
      if (slot->kind == RILL_V_UNIT)
        *slot = rill_eval_text(e, n->text.data, n->text.size);
      if (n->kind == RILL_STRING && !e->error.kind) {
        // Lowered literals read the pool. Record keys still need their decoded
        // syntax text for pattern matching; Strings no longer need two copies.
        size_t released = n->text.capacity;
        rill_text_clear(&n->text);
        code->syntax.allocation -= released;
        value.as.object->allocation -= released;
        e->heap.bytes -= released;
      }
    }
    rill_runtime_unroot(&e->heap, &root);
    if (e->error.kind)
      goto failure;
  }
  if (code->syntax.state != RILL_COMPLETE) {
    e->error = code->syntax.diagnostic;
    e->roots[ERROR_CODE] = value;
  }
  free(literals);
  return value;
failure:
  free(literals);
  rill_syntax_clear(syntax);
  rill_eval_error(e, RILL_MEMORY, "cannot retain code");
  return (RillValue){};
}
void rill_eval_code_free(RillCode *code) {
  for (size_t i = 0; i < code->count; ++i)
    free(code->functions[i].names);
  rill_syntax_clear(&code->syntax);
  free(code);
}
