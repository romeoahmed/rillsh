/** @file
 * @brief Syntax-owned command and expression tree, shared by scripts and
 * prompts.
 */
#pragma once
#include "diagnostic.h"
#include "source.h"
#include "text/text.h"
#include <stddef.h>
#include <stdint.h>
/** @brief Implemented expression forms; command statements lower to unary run.
 */
typedef enum {
  RILL_STRING,
  RILL_INTEGER,
  RILL_UNIT,
  RILL_NAME,
  RILL_CALL,
  RILL_BIND,
  RILL_PLAN,
  RILL_STAGE,
  RILL_REDIRECT,
  RILL_LIST,
  RILL_INDEX,
  RILL_FIELD
} RillNodeKind;
/** @brief Redirection syntax in source order. */
typedef enum {
  RILL_INPUT,
  RILL_OUTPUT,
  RILL_APPEND,
  RILL_ERROR_OUTPUT,
  RILL_ERROR_APPEND,
  RILL_ERROR_TO_OUTPUT
} RillRedirect;
/**
 * @brief Node owned by a parse result, with copied text and a source byte
 * offset.
 */
typedef struct RillNode {
  RillNodeKind kind;               ///< Syntactic form.
  size_t offset;                   ///< Source byte offset.
  RillBuffer text;                 ///< Decoded literal or name.
  int64_t integer;                 ///< Integer literal or redirection tag.
  struct RillNode *children;       ///< Ordered operands.
  struct RillNode *next;           ///< Next sibling.
  struct RillNode *allocated_next; ///< Parse arena ownership chain.
} RillNode;
/** @brief Whole-entry classification. */
typedef enum { RILL_COMPLETE, RILL_INCOMPLETE, RILL_INVALID } RillParseState;
/**
 * @brief Owned parse result; nodes and decoded text do not borrow source
 * storage.
 */
typedef struct RillSyntax {
  RillNode *first;           ///< First statement.
  RillNode *allocated;       ///< All nodes, freed without recursive traversal.
  RillParseState state;      ///< Classification of the entire source.
  RillDiagnostic diagnostic; ///< Failure or incomplete-input explanation.
} RillSyntax;
/**
 * @brief Classify and parse a whole entry without executing it.
 *
 * Source is borrowed only for this call. Clear the returned result in every
 * state, including incomplete or invalid input.
 */
RillSyntax rill_syntax_parse(const RillSource *source);
/** @brief Release all nodes and decoded literals. */
void rill_syntax_clear(RillSyntax *syntax);
