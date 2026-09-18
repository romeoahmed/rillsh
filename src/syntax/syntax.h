/**
 * @file
 * @brief Syntax-owned command and expression tree, shared by scripts and
 * prompts.
 */
#pragma once
#include "diagnostic.h"
#include "source.h"
#include "text/text.h"
#include <stddef.h>
#include <stdint.h>
/** @brief Syntax forms; command statements lower to unary run calls. */
typedef enum {
  RILL_STRING,
  RILL_INTEGER,
  RILL_UNIT,
  RILL_NAME,
  RILL_BUILTIN,
  RILL_EXPORT,
  RILL_CALL,
  RILL_BIND,
  RILL_PLAN,
  RILL_STAGE,
  RILL_REDIRECT,
  RILL_LIST,
  RILL_INDEX,
  RILL_FIELD,
  RILL_FLOAT,
  RILL_BOOL,
  RILL_NULL,
  RILL_RECORD,
  RILL_PAIR,
  RILL_FUNCTION,
  RILL_DECLARE,
  RILL_BLOCK,
  RILL_IF,
  RILL_BINARY,
  RILL_UNARY,
  RILL_PIPE,
  RILL_WITH,
  RILL_MATCH,
  RILL_ARM,
  RILL_REST,
  RILL_NOMINAL,
  RILL_STRUCT,
  RILL_ENUM,
  RILL_REC,
  RILL_IMPORT,
  RILL_SPREAD
} RillNodeKind;
/** @brief Parsed operators; spelling is resolved once, before evaluation. */
typedef enum {
  RILL_OP_PIPE,
  RILL_OP_WITH,
  RILL_OP_OR,
  RILL_OP_AND,
  RILL_OP_EQ,
  RILL_OP_NE,
  RILL_OP_LE,
  RILL_OP_GE,
  RILL_OP_LT,
  RILL_OP_GT,
  RILL_OP_ADD,
  RILL_OP_SUB,
  RILL_OP_MUL,
  RILL_OP_DIV,
  RILL_OP_NOT
} RillOperator;
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
 * @brief Stable node owned by syntax, then by prepared code after transfer.
 *
 * Text is owned, not a view of caller source. Preparation may replace decoded
 * literals with constant slots; inspect payloads according to kind and flags.
 */
typedef struct RillNode {
  RillNodeKind kind; ///< Syntactic form.
  bool grouped;      ///< Explicit parentheses permit nested comparisons.
  bool captured;     ///< Name resolves to a prepared closure slot.
  size_t offset;     ///< Source byte offset.
  RillBuffer text; ///< Decoded text; code preparation replaces String text with
                   ///< a slot.
  struct RillNode *pattern; ///< Parameter or binding pattern.
  union {
    double real;           ///< Finite floating literal.
    int64_t integer;       ///< Integer literal or module flag.
    RillOperator op;       ///< Unary, binary, pipe, or update operation.
    RillRedirect redirect; ///< Syntax redirection operation.
    size_t constant; ///< String/key slot assigned after code consumes syntax.
    size_t function; ///< Capture-layout slot assigned during code preparation.
    size_t capture;  ///< Closure Value slot for a captured Name.
    size_t
        slots; ///< Aggregate frame capacity assigned during code preparation.
  }; ///< Kind selects the payload; the parser leaves preparation slots unset.
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
  size_t
      allocation; ///< Arena, owned source, and decoded-buffer allocation bytes.
  RillBuffer name;           ///< Logical source identity retained by closures.
  RillBuffer source;         ///< Owned input for code lifetime transfer.
  RillNode *first;           ///< First statement.
  RillNode *allocated;       ///< All nodes, traversed without recursion.
  struct RillNodes *nodes;   ///< Stable node blocks owned by this parse.
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
/** @brief Free nodes, text, and copied source, then reset the parse result. */
void rill_syntax_clear(RillSyntax *syntax);
