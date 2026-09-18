#include "diagnostic.h"
#include "private.h"
#include "runtime.h"
#include "syntax/syntax.h"
#include "text/text.h"
#include <stdckdint.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>

static size_t bucket(const RillNode *node, size_t capacity) {
  uintptr_t key = (uintptr_t)node >> 3;
  key ^= key >> 17;
  return (size_t)key & (capacity - 1);
}
Captures *rill_eval_captures(RillCode *code, const RillNode *node) {
  size_t slot = bucket(node, code->capacity);
  while (code->functions[slot].node != node)
    slot = (slot + 1) & (code->capacity - 1);
  return &code->functions[slot];
}
static bool constant(const RillNode *node) {
  return node->kind == RILL_STRING || node->kind == RILL_PAIR;
}
RillValue rill_eval_constant(RillValue owner, const RillNode *node) {
  return owner.as.object->values[node->constant];
}
RillValue rill_eval_code(RillEval *e, RillSyntax *syntax) {
  if (syntax->allocation < sizeof(RillSyntax))
    goto failure;
  size_t count = 0, constants = 0, capacity = 1, bytes = {}, allocation = {},
         total = {};
  for (const RillNode *n = syntax->allocated; n; n = n->allocated_next) {
    if (n->kind == RILL_FUNCTION)
      ++count;
    if (constant(n))
      ++constants;
  }
  while (capacity / 2 < count)
    if (ckd_mul(&capacity, capacity, 2))
      goto failure;
  if (ckd_mul(&bytes, capacity, sizeof(Captures)) ||
      ckd_add(&bytes, bytes, sizeof(RillCode)))
    goto failure;
  RillValue value =
      rill_eval_object(e, RILL_V_CODE, nullptr, constants, nullptr, 0);
  if (e->error.kind)
    goto failure;
  // Analysis uses only C storage; constant construction roots code below.
  RillCode *code = malloc(bytes);
  if (!code)
    goto failure;
  *code = (RillCode){.syntax = *syntax, .capacity = capacity};
  *syntax = (RillSyntax){};
  for (size_t i = 0; i < capacity; ++i)
    code->functions[i] = (Captures){};
  for (const RillNode *n = code->syntax.allocated; n; n = n->allocated_next) {
    if (n->kind != RILL_FUNCTION)
      continue;
    size_t slot = bucket(n, capacity);
    while (code->functions[slot].node)
      slot = (slot + 1) & (capacity - 1);
    code->functions[slot].node = n;
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
  for (size_t i = 0; i < capacity; ++i) {
    Captures *analysis = &code->functions[i];
    if (!analysis->node || code->syntax.state != RILL_COMPLETE)
      continue;
    rill_eval_analyze(e, analysis);
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
    size_t slot = 0;
    for (RillNode *n = code->syntax.allocated; n && !e->error.kind;
         n = n->allocated_next) {
      if (!constant(n))
        continue;
      n->constant = slot;
      value.as.object->values[slot++] =
          rill_eval_text(e, n->text.data, n->text.size);
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
  return value;
failure:
  rill_syntax_clear(syntax);
  rill_eval_error(e, RILL_MEMORY, "cannot retain code");
  return (RillValue){};
}
void rill_eval_code_free(RillCode *code) {
  for (size_t i = 0; i < code->capacity; ++i)
    free(code->functions[i].names);
  rill_syntax_clear(&code->syntax);
  free(code);
}
