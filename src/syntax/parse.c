/**
 * @file
 * @brief Parse complete entries into stable, owned syntax trees.
 *
 * Expression and command modes share source offsets but have distinct word
 * rules. Precedence parsing preserves effect order; syntax depth is checked
 * before evaluation. Complete, incomplete, and invalid results all own storage
 * and must be cleared or transferred to code preparation.
 */
#include "diagnostic.h"
#include "source.h"
#include "syntax.h"
#include "text/text.h"
#include <errno.h>
#include <math.h>
#include <stdckdint.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

static constexpr size_t MAX_SYNTAX_DEPTH = 256;
struct RillNodes {
  struct RillNodes *next;
  size_t used, capacity;
  RillNode items[];
};
typedef struct {
  const char *s;
  size_t size, at;
  unsigned depth;
  bool soft_lines;
  RillSyntax out;
} Parser;
static void fail(Parser *p, const char *message) {
  if (p->out.diagnostic.kind)
    return;
  p->out.state = RILL_INVALID;
  p->out.diagnostic = (RillDiagnostic){
      .kind = RILL_SYNTAX, .offset = p->at, .message = message};
}
static void expected(Parser *p, const char *message) {
  if (p->out.diagnostic.kind)
    return;
  fail(p, message);
  if (p->at == p->size)
    p->out.state = RILL_INCOMPLETE;
}
static void memory_error(Parser *p) {
  p->out.state = RILL_INVALID;
  p->out.diagnostic = (RillDiagnostic){
      .kind = RILL_MEMORY, .offset = p->at, .message = "allocation failed"};
}
static RillNode *node(Parser *p, RillNodeKind kind, size_t at) {
  struct RillNodes *block = p->out.nodes;
  if (!block || block->used == block->capacity) {
    size_t capacity = !block                  ? 8
                      : block->capacity < 128 ? block->capacity * 2
                                              : 128;
    // Fixed upper capacity bounds this allocation; nodes never move.
    struct RillNodes *next =
        malloc(sizeof(*next) + capacity * sizeof(RillNode));
    if (!next) {
      memory_error(p);
      return nullptr;
    }
    *next = (struct RillNodes){.next = block, .capacity = capacity};
    p->out.nodes = block = next;
  }
  RillNode *n = &block->items[block->used++];
  *n = (RillNode){
      .kind = kind, .offset = at, .allocated_next = p->out.allocated};
  p->out.allocated = n;
  return n;
}
static bool append(Parser *p, RillBuffer *b, const char *s, size_t n) {
  if (rill_text_append(b, s, n))
    return true;
  memory_error(p);
  return false;
}
static bool ident(char c) {
  return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || c == '_';
}
static bool digit(char c) { return c >= '0' && c <= '9'; }
static void space(Parser *p, bool lines) {
  while (p->at < p->size) {
    char c = p->s[p->at];
    if (c == ' ' || c == '\t' || c == '\r' || (lines && c == '\n')) {
      ++p->at;
      continue;
    }
    if (c == '#') {
      while (p->at < p->size && p->s[p->at] != '\n')
        ++p->at;
      continue;
    }
    break;
  }
}
static bool take(Parser *p, char c) {
  if (p->at < p->size && p->s[p->at] == c) {
    ++p->at;
    return true;
  }
  return false;
}
static bool keyword(Parser *p, const char *s) {
  size_t n = strlen(s);
  if (n > p->size - p->at || memcmp(p->s + p->at, s, n) != 0)
    return false;
  if (p->at + n < p->size && (ident(p->s[p->at + n]) || digit(p->s[p->at + n])))
    return false;
  p->at += n;
  return true;
}
static RillNode *expression(Parser *p);
static RillNode *pipeline(Parser *p);
static RillNode *string(Parser *p) {
  RillNode *n = node(p, RILL_STRING, p->at);
  if (!n)
    return nullptr;
  char quote = p->s[p->at++];
  while (p->at < p->size) {
    size_t start = p->at;
    while (p->at < p->size && p->s[p->at] != quote &&
           !(quote == '"' && p->s[p->at] == '\\'))
      ++p->at;
    if (p->at != start && !append(p, &n->text, p->s + start, p->at - start))
      return nullptr;
    if (p->at == p->size)
      break;
    char c = p->s[p->at++];
    if (c == quote)
      return n;
    if (c == '\\' && quote == '"') {
      if (p->at == p->size)
        break;
      c = p->s[p->at++];
      switch (c) {
      case 'n':
        c = '\n';
        break;
      case 'r':
        c = '\r';
        break;
      case 't':
        c = '\t';
        break;
      case '\\':
      case '"':
        break;
      case 'u': {
        if (!take(p, '{')) {
          expected(p, "expected '{' after Unicode escape");
          return nullptr;
        }
        uint32_t cp = 0;
        unsigned digits = 0;
        while (p->at < p->size && p->s[p->at] != '}') {
          char d = p->s[p->at++];
          unsigned v = {};
          if (digit(d))
            v = (unsigned)(d - '0');
          else if (d >= 'a' && d <= 'f')
            v = (unsigned)(d - 'a') + 10;
          else if (d >= 'A' && d <= 'F')
            v = (unsigned)(d - 'A') + 10;
          else {
            fail(p, "invalid Unicode escape");
            return nullptr;
          }
          if (++digits > 6) {
            fail(p, "Unicode escape exceeds U+10FFFF");
            return nullptr;
          }
          cp = cp * 16 + v;
        }
        if (!take(p, '}')) {
          expected(p, "expected '}' after Unicode escape");
          return nullptr;
        }
        if (!digits || cp > 0x10ffff || (cp >= 0xd800 && cp <= 0xdfff)) {
          fail(p, "Unicode escape must name a Unicode scalar value");
          return nullptr;
        }
        if (!rill_text_encode(&n->text, cp)) {
          memory_error(p);
          return nullptr;
        }
        continue;
      }
      default:
        fail(p, "unknown string escape");
        return nullptr;
      }
    }
    if (!append(p, &n->text, &c, 1))
      return nullptr;
  }
  expected(p, "unterminated string");
  return nullptr;
}
static RillNode *name(Parser *p) {
  size_t at = p->at;
  if (at == p->size || !ident(p->s[at])) {
    expected(p, "expected identifier");
    return nullptr;
  }
  while (p->at < p->size && (ident(p->s[p->at]) || digit(p->s[p->at])))
    ++p->at;
  RillNode *n = node(p, RILL_NAME, at);
  if (!n)
    return nullptr;
  // Identifier spelling is complete and never grows. Do not retain a
  // geometrically sized editing buffer for every name in long-lived code.
  size_t size = p->at - at;
  char *text = strndup(p->s + at, size);
  if (!text) {
    memory_error(p);
    return nullptr;
  }
  n->text = (RillBuffer){.data = text, .size = size, .capacity = size + 1};
  return n;
}
static RillNode *binding_name(Parser *p, RillNode *n, bool wildcard) {
  if (!n)
    return nullptr;
  static const char *const reserved[] = {
      "let",    "fn",   "rec",  "if",  "then",   "else", "do",     "match",
      "struct", "enum", "with", "job", "import", "as",   "export", "true",
      "false",  "null", "and",  "or",  "not",    "of"};
  bool invalid = !wildcard && !strcmp(n->text.data, "_");
  for (size_t i = 0; i < sizeof(reserved) / sizeof(*reserved); ++i)
    invalid |= !strcmp(n->text.data, reserved[i]);
  if (invalid) {
    fail(p, "reserved word cannot be used as a binding name");
    return nullptr;
  }
  return n;
}
static bool token(Parser *p, const char *text) {
  size_t n = strlen(text);
  if (n > p->size - p->at || memcmp(p->s + p->at, text, n) != 0)
    return false;
  p->at += n;
  return true;
}
static bool require(Parser *p, char c, const char *message) {
  space(p, true);
  if (take(p, c))
    return true;
  expected(p, message);
  return false;
}
static RillNode *pattern(Parser *p, bool parameter);
static RillNode *statements(Parser *p, bool top, bool block);
static RillNode *binary(Parser *p, unsigned precedence);
static RillNode *number(Parser *p) {
  size_t at = p->at;
  [[gnu::cleanup(rill_text_clear)]] RillBuffer text = {};
  bool fractional = false, last_digit = false;
  if (take(p, '-')) {
    if (!append(p, &text, "-", 1))
      return nullptr;
  }
  while (p->at < p->size) {
    char c = p->s[p->at];
    if (digit(c))
      last_digit = true;
    else if (c == '_') {
      if (!last_digit || p->at + 1 == p->size || !digit(p->s[p->at + 1])) {
        fail(p, "numeric separators must occur between digits");
        break;
      }
      ++p->at;
      last_digit = false;
      continue;
    } else if (c == '.' && p->at + 1 < p->size && digit(p->s[p->at + 1]) &&
               !fractional) {
      fractional = true;
      last_digit = false;
    } else if ((c == 'e' || c == 'E') && last_digit) {
      fractional = true;
      if (!append(p, &text, &c, 1))
        break;
      ++p->at;
      if (p->at < p->size && (p->s[p->at] == '+' || p->s[p->at] == '-')) {
        if (!append(p, &text, p->s + p->at, 1))
          break;
        ++p->at;
      }
      last_digit = false;
      while (p->at < p->size && digit(p->s[p->at])) {
        if (!append(p, &text, p->s + p->at, 1))
          break;
        ++p->at;
        last_digit = true;
      }
      break;
    } else
      break;
    ++p->at;
    if (!append(p, &text, &c, 1))
      break;
  }
  if (!last_digit) {
    expected(p, "expected digits in number");
    return nullptr;
  }
  if (!p->out.diagnostic.kind) {
    RillNode *n = node(p, fractional ? RILL_FLOAT : RILL_INTEGER, at);
    if (!n)
      return nullptr;
    errno = 0;
    char *end = {};
    if (fractional) {
      n->real = strtod(text.data, &end);
      if (!isfinite(n->real) || *end)
        fail(p, "expected a finite Float literal");
    } else {
      n->integer = strtoll(text.data, &end, 10);
      if (errno == ERANGE || *end)
        fail(p, "integer out of range");
    }
    return n;
  }
  return nullptr;
}
static RillNode *aggregate_items(Parser *p, bool pat, char closing) {
  RillNode *n = node(p, closing == ']' ? RILL_LIST : RILL_RECORD, p->at);
  if (!n)
    return nullptr;
  RillNode **tail = &n->children;
  space(p, true);
  while (!take(p, closing)) {
    RillNode *item = {};
    if (pat && token(p, "..")) {
      item = node(p, RILL_REST, p->at);
      space(p, true);
      if (item && p->at < p->size && ident(p->s[p->at]))
        item->children = binding_name(p, name(p), true);
      if (!item)
        return nullptr;
      *tail = item;
      space(p, true);
      (void)take(p, ',');
      if (!require(p, closing, "rest pattern must be last"))
        return nullptr;
      return n;
    }
    if (closing == ']')
      item = pat ? pattern(p, false) : expression(p);
    else {
      space(p, true);
      item = p->at < p->size && (p->s[p->at] == '\'' || p->s[p->at] == '"')
                 ? string(p)
                 : name(p);
      if (!item)
        return nullptr;
      bool shorthand = item->kind == RILL_NAME;
      item->kind = RILL_PAIR;
      space(p, true);
      if (take(p, ':'))
        item->children = pat ? pattern(p, false) : expression(p);
      else if (shorthand) {
        if (!binding_name(p, item, pat))
          return nullptr;
        item->children = node(p, RILL_NAME, item->offset);
        if (item->children &&
            !append(p, &item->children->text, item->text.data, item->text.size))
          return nullptr;
      } else
        expected(p, "expected ':' after record key");
      if (!item->children)
        return nullptr;
    }
    if (!item || p->out.diagnostic.kind)
      return nullptr;
    *tail = item;
    tail = &item->next;
    space(p, true);
    if (take(p, closing))
      break;
    if (!require(p, ',', "expected ',' or closing delimiter"))
      return nullptr;
    space(p, true);
  }
  return n;
}
static int field_order(const void *left, const void *right) {
  const RillNode *const *a = left, *const *b = right;
  RillBuffer x = (*a)->text, y = (*b)->text;
  size_t size = x.size < y.size ? x.size : y.size;
  int order = size ? memcmp(x.data, y.data, size) : 0;
  return order ? order : (x.size > y.size) - (x.size < y.size);
}
static void unique_fields(Parser *p, const RillNode *record) {
  size_t count = 0;
  for (RillNode *n = record->children; n && n->kind == RILL_PAIR; n = n->next)
    ++count;
  if (count < 2)
    return;
  RillNode *local[32], **fields = local;
  if (count > sizeof(local) / sizeof(*local)) {
    size_t bytes = {};
    if (ckd_mul(&bytes, count, sizeof(*fields))) {
      memory_error(p);
      return;
    }
    fields = malloc(bytes);
    if (!fields) {
      memory_error(p);
      return;
    }
  }
  size_t i = 0;
  for (RillNode *n = record->children; i < count; n = n->next)
    fields[i++] = n;
  // Sort only borrowed pointers: source order remains the evaluation order.
  qsort(fields, count, sizeof(*fields), field_order);
  for (i = 1; i < count; ++i)
    if (!field_order(fields + i - 1, fields + i)) {
      p->at = fields[i]->offset;
      fail(p, "duplicate record key");
      break;
    }
  if (fields != local)
    free(fields);
}
static RillNode *aggregate(Parser *p, bool pat, char closing) {
  bool outer = p->soft_lines;
  p->soft_lines = true;
  RillNode *n = aggregate_items(p, pat, closing);
  if (n && n->kind == RILL_RECORD && !p->out.diagnostic.kind)
    unique_fields(p, n);
  p->soft_lines = outer;
  return n;
}
static RillNode *pattern(Parser *p, bool parameter) {
  if (++p->depth > MAX_SYNTAX_DEPTH) {
    fail(p, "pattern nesting limit exceeded");
    --p->depth;
    return nullptr;
  }
  space(p, true);
  RillNode *n = nullptr;
  if (take(p, '['))
    n = aggregate(p, true, ']');
  else if (take(p, '{'))
    n = aggregate(p, true, '}');
  else if (take(p, '(')) {
    space(p, true);
    if (take(p, ')'))
      n = node(p, RILL_UNIT, p->at);
    else {
      n = pattern(p, false);
      (void)require(p, ')', "expected ')'");
    }
  } else if (keyword(p, "true") || keyword(p, "false")) {
    n = node(p, RILL_BOOL, p->at);
    if (n)
      n->integer = p->s[p->at - 1] == 'e' && p->s[p->at - 2] == 'u';
  } else if (keyword(p, "null"))
    n = node(p, RILL_NULL, p->at);
  else if (p->at < p->size && (digit(p->s[p->at]) || p->s[p->at] == '-'))
    n = number(p);
  else if (p->at < p->size && (p->s[p->at] == '\'' || p->s[p->at] == '"'))
    n = string(p);
  else {
    n = binding_name(p, name(p), true);
    bool qualified = false;
    while (n && take(p, '.')) {
      RillNode *field = name(p);
      if (!field) {
        n = nullptr;
        break;
      }
      field->kind = RILL_FIELD;
      field->children = n;
      n = field;
      qualified = true;
    }
    if (!parameter)
      space(p, true);
    if (n && ((!parameter && take(p, '{')) || qualified)) {
      bool fields = p->at && p->s[p->at - 1] == '{';
      RillNode *nominal = node(p, RILL_NOMINAL, n->offset);
      if (!nominal)
        n = nullptr;
      else {
        nominal->pattern = n;
        nominal->children = fields ? aggregate(p, true, '}') : nullptr;
        n = nominal;
      }
    }
  }
  --p->depth;
  return n;
}
// Only the header is inspected: nested patterns and quoted keys are skipped.
// A colon, comma, or closing brace selects a Record; => selects a closure.
static bool closure_header(const Parser *p) {
  size_t depth = 0;
  char quote = 0;
  for (size_t i = p->at; i < p->size; ++i) {
    char c = p->s[i];
    if (quote) {
      if (c == '\\' && quote == '"' && i + 1 < p->size)
        ++i;
      else if (c == quote)
        quote = 0;
    } else if (c == '"' || c == '\'')
      quote = c;
    else if (c == '#') {
      while (i < p->size && p->s[i] != '\n')
        ++i;
    } else if (!depth && c == '=' && i + 1 < p->size && p->s[i + 1] == '>')
      return true;
    else if (!depth && (c == ':' || c == ',' || c == '}'))
      return false;
    else if (c == '(' || c == '[' || c == '{')
      ++depth;
    else if (c == ')' || c == ']' || c == '}') {
      if (!depth)
        return false;
      --depth;
    }
  }
  // An unfinished header without Record punctuation remains incomplete.
  return true;
}
static RillNode *function(Parser *p, bool closure) {
  RillNode *first = nullptr, **body = &first;
  for (;;) {
    space(p, true);
    bool end = closure ? token(p, "=>") : take(p, '=');
    if (end) {
      if (!first) {
        fail(p, "expected parameter; use () for a Unit parameter");
        return nullptr;
      }
      break;
    }
    size_t start = p->at;
    RillNode *parameter = pattern(p, true);
    RillNode *n = node(p, RILL_FUNCTION, start);
    if (!parameter || !n)
      return nullptr;
    n->pattern = parameter;
    *body = n;
    body = &n->children;
    if (p->at < p->size) {
      char c = p->s[p->at];
      if (c != ' ' && c != '\t' && c != '\r' && c != '\n' && c != '#' &&
          c != '=') {
        fail(p, "expected whitespace between parameters");
        return nullptr;
      }
    }
  }
  if (closure) {
    size_t at = p->at;
    RillNode *items = statements(p, false, true);
    if (items && !items->next && items->kind != RILL_BIND &&
        items->kind != RILL_DECLARE && items->kind != RILL_REC)
      *body = items;
    else {
      *body = node(p, RILL_BLOCK, at);
      if (!*body)
        return nullptr;
      (*body)->children = items;
    }
  } else
    *body = expression(p);
  return *body ? first : nullptr;
}
// Lower a recursive expression to a private declaration and its value.
static RillNode *recursive_function(Parser *p, size_t at) {
  if (!require(p, '{', "expected '{' after 'rec'"))
    return nullptr;
  space(p, true);
  RillNode *decl = binding_name(p, name(p), false);
  RillNode *block = node(p, RILL_BLOCK, at);
  RillNode *value = node(p, RILL_NAME, at);
  if (!decl || !block || !value ||
      !append(p, &value->text, decl->text.data, decl->text.size))
    return nullptr;
  if (p->at < p->size && p->s[p->at] != ' ' && p->s[p->at] != '\t' &&
      p->s[p->at] != '\r' && p->s[p->at] != '\n' && p->s[p->at] != '#') {
    fail(p, "expected whitespace before parameters");
    return nullptr;
  }
  decl->kind = RILL_DECLARE;
  decl->children = function(p, true);
  if (!decl->children)
    return nullptr;
  decl->next = value;
  block->children = decl;
  return block;
}
static bool recursive_group(Parser *p) {
  size_t at = p->at;
  bool group = false;
  if (keyword(p, "rec")) {
    space(p, true);
    if (take(p, '{')) {
      space(p, true);
      group = keyword(p, "fn");
    }
  }
  p->at = at;
  return group;
}
static RillNode *delimited_expression(Parser *p, char closing) {
  bool outer = p->soft_lines;
  p->soft_lines = true;
  RillNode *n = expression(p);
  (void)require(p, closing, "expected closing delimiter");
  p->soft_lines = outer;
  return n;
}
static RillNode *match_arms(Parser *p, RillNode *n) {
  RillNode **tail = &n->children->next;
  space(p, true);
  while (!take(p, '}')) {
    RillNode *arm = node(p, RILL_ARM, p->at);
    if (!arm)
      return nullptr;
    *tail = arm;
    tail = &arm->next;
    arm->pattern = pattern(p, false);
    if (!arm->pattern)
      return nullptr;
    space(p, true);
    if (keyword(p, "if"))
      arm->children = expression(p);
    else {
      arm->children = node(p, RILL_BOOL, p->at);
      if (arm->children)
        arm->children->integer = 1;
    }
    if (!arm->children)
      return nullptr;
    space(p, true);
    if (!token(p, "=>")) {
      expected(p, "expected '=>' after match pattern");
      return nullptr;
    }
    arm->children->next = expression(p);
    if (!arm->children->next)
      return nullptr;
    space(p, true);
    if (take(p, '}'))
      break;
    if (!require(p, ',', "expected ',' or '}'"))
      return nullptr;
    space(p, true);
  }
  return n;
}
static RillNode *atom(Parser *p) {
  space(p, true);
  if (p->at == p->size) {
    expected(p, "expected expression");
    return nullptr;
  }
  size_t at = p->at;
  char c = p->s[at];
  if (c == '\'' || c == '"')
    return string(p);
  if (digit(c) || (c == '-' && at + 1 < p->size && digit(p->s[at + 1])))
    return number(p);
  if (keyword(p, "rec"))
    return recursive_function(p, at);
  if (keyword(p, "do")) {
    RillNode *n = node(p, RILL_BLOCK, at);
    if (!n || !require(p, '{', "expected '{' after 'do'"))
      return nullptr;
    n->children = statements(p, false, true);
    return n;
  }
  if (keyword(p, "if")) {
    RillNode *n = node(p, RILL_IF, at);
    if (!n)
      return nullptr;
    n->children = expression(p);
    if (!n->children)
      return nullptr;
    space(p, true);
    if (!keyword(p, "then")) {
      expected(p, "expected 'then' after condition");
      return nullptr;
    }
    n->children->next = expression(p);
    if (!n->children->next)
      return nullptr;
    space(p, true);
    if (!keyword(p, "else")) {
      expected(p, "expected 'else' after then-branch");
      return nullptr;
    }
    n->children->next->next = expression(p);
    return n;
  }
  if (keyword(p, "match")) {
    RillNode *n = node(p, RILL_MATCH, at);
    if (!n)
      return nullptr;
    n->children = expression(p);
    space(p, true);
    if (!keyword(p, "of")) {
      expected(p, "expected 'of' before match arms");
      return nullptr;
    }
    if (!n->children || !require(p, '{', "expected '{' before match arms"))
      return nullptr;
    bool outer = p->soft_lines;
    p->soft_lines = true;
    n = match_arms(p, n);
    p->soft_lines = outer;
    return n;
  }
  if (keyword(p, "true")) {
    RillNode *n = node(p, RILL_BOOL, at);
    if (n)
      n->integer = 1;
    return n;
  }
  if (keyword(p, "false"))
    return node(p, RILL_BOOL, at);
  if (keyword(p, "null"))
    return node(p, RILL_NULL, at);
  if (keyword(p, "job")) {
    if (!require(p, '{', "expected '{' after 'job'"))
      return nullptr;
    RillNode *n = pipeline(p);
    (void)require(p, '}', "expected '}' after pipeline");
    return n;
  }
  if (take(p, '(')) {
    space(p, true);
    if (take(p, ')'))
      return node(p, RILL_UNIT, at);
    RillNode *n = delimited_expression(p, ')');
    if (n)
      n->grouped = true;
    return n;
  }
  if (take(p, '['))
    return aggregate(p, false, ']');
  if (take(p, '{'))
    return closure_header(p) ? function(p, true) : aggregate(p, false, '}');
  return binding_name(p, name(p), false);
}
static RillNode *suffix(Parser *p) {
  RillNode *n = atom(p);
  while (n && !p->out.diagnostic.kind) {
    if (take(p, '[')) {
      RillNode *index = delimited_expression(p, ']');
      RillNode *access = node(p, RILL_INDEX, n->offset);
      if (!index || !access)
        return nullptr;
      access->children = n;
      n->next = index;
      n = access;
    } else if (take(p, '.')) {
      RillNode *field = name(p);
      if (!field)
        return nullptr;
      field->kind = RILL_FIELD;
      field->children = n;
      n = field;
    } else {
      if (p->at < p->size && p->s[p->at] == '(')
        fail(p, "function calls require whitespace; write 'f x' or 'f "
                "(expression)'");
      break;
    }
  }
  return n;
}
static bool argument_start(Parser *p) {
  if (p->at == p->size)
    return false;
  char c = p->s[p->at];
  if (c == '(' || c == '[' || c == '{' || c == '\'' || c == '"' || digit(c))
    return true;
  if (!ident(c))
    return false;
  size_t saved = p->at;
  bool stop = keyword(p, "of") || keyword(p, "then") || keyword(p, "else") ||
              keyword(p, "with") || keyword(p, "and") || keyword(p, "or") ||
              keyword(p, "not") || keyword(p, "if") || keyword(p, "match") ||
              keyword(p, "fn");
  p->at = saved;
  return !stop;
}
static RillNode *application(Parser *p) {
  RillNode *left = suffix(p);
  while (left && !p->out.diagnostic.kind) {
    size_t saved = p->at;
    space(p, p->soft_lines);
    if (saved == p->at || !argument_start(p)) {
      p->at = saved;
      break;
    }
    size_t at = p->at;
    RillNode *arg = suffix(p), *call = node(p, RILL_CALL, at);
    if (!arg || !call)
      return nullptr;
    call->children = left;
    left->next = arg;
    left = call;
  }
  return left;
}
static RillNode *unary(Parser *p) {
  space(p, true);
  size_t at = p->at;
  bool neg = p->at < p->size && p->s[p->at] == '-' &&
             !(p->at + 1 < p->size && digit(p->s[p->at + 1]));
  if ((neg && take(p, '-')) || keyword(p, "not")) {
    RillNode *n = node(p, RILL_UNARY, at);
    if (!n)
      return nullptr;
    n->op = neg ? RILL_OP_SUB : RILL_OP_NOT;
    n->children = binary(p, 9);
    return n;
  }
  return application(p);
}
static RillNode *binary(Parser *p, unsigned minimum) {
  if (++p->depth > MAX_SYNTAX_DEPTH) {
    fail(p, "syntax nesting limit exceeded");
    --p->depth;
    return nullptr;
  }
  RillNode *left = unary(p);
  while (left && !p->out.diagnostic.kind) {
    size_t saved = p->at;
    space(p, p->soft_lines);
    if (p->at < p->size && p->s[p->at] == '\n') {
      space(p, true);
      if (p->size - p->at < 2 || memcmp(p->s + p->at, "|>", 2) != 0) {
        p->at = saved;
        break;
      }
    }
    static const struct {
      const char *text;
      RillOperator op;
      unsigned prec;
      bool word;
    } ops[] = {{"|>", RILL_OP_PIPE, 1, false}, {"with", RILL_OP_WITH, 2, true},
               {"or", RILL_OP_OR, 3, true},    {"and", RILL_OP_AND, 4, true},
               {"==", RILL_OP_EQ, 5, false},   {"!=", RILL_OP_NE, 5, false},
               {"<=", RILL_OP_LE, 5, false},   {">=", RILL_OP_GE, 5, false},
               {"<", RILL_OP_LT, 5, false},    {">", RILL_OP_GT, 5, false},
               {"+", RILL_OP_ADD, 6, false},   {"-", RILL_OP_SUB, 6, false},
               {"*", RILL_OP_MUL, 7, false},   {"/", RILL_OP_DIV, 7, false}};
    size_t i = 0;
    for (; i < sizeof(ops) / sizeof(*ops); ++i) {
      if (ops[i].prec < minimum)
        continue;
      if (ops[i].word ? keyword(p, ops[i].text) : token(p, ops[i].text))
        break;
    }
    if (i == sizeof(ops) / sizeof(*ops)) {
      p->at = saved;
      break;
    }
    if (ops[i].prec == 5 && left->kind == RILL_BINARY &&
        left->op >= RILL_OP_EQ && left->op <= RILL_OP_GT && !left->grouped) {
      fail(p, "comparisons cannot be chained; combine them with 'and'");
      break;
    }
    RillOperator op = ops[i].op;
    RillNode *n = node(p,
                       op == RILL_OP_PIPE   ? RILL_PIPE
                       : op == RILL_OP_WITH ? RILL_WITH
                                            : RILL_BINARY,
                       left->offset);
    if (!n) {
      left = nullptr;
      break;
    }
    n->op = op;
    n->children = left;
    left->next = binary(p, ops[i].prec + 1);
    if (!left->next) {
      left = nullptr;
      break;
    }
    left = n;
  }
  --p->depth;
  return left;
}
static RillNode *expression(Parser *p) { return binary(p, 1); }
static bool delimiter(char c) {
  return c == ' ' || c == '\t' || c == '\r' || c == '\n' || c == '|' ||
         c == ';' || c == '{' || c == '}' || c == '<' || c == '>';
}
static RillNode *word(Parser *p) {
  if (p->at == p->size || delimiter(p->s[p->at])) {
    expected(p, "expected command word");
    return nullptr;
  }
  if (token(p, "...")) {
    RillNode *spread = node(p, RILL_SPREAD, p->at - 3);
    if (!spread)
      return nullptr;
    if (p->at == p->size || p->s[p->at] != '$') {
      expected(p, "command spread requires a substitution: ...$(expression)");
      return nullptr;
    }
    spread->children = word(p);
    return spread->children ? spread : nullptr;
  }
  RillNode *n = {};
  char c = p->s[p->at];
  if (c == '\'' || c == '"')
    n = string(p);
  else if (take(p, '$')) {
    if (take(p, '(')) {
      n = delimited_expression(p, ')');
    } else
      n = binding_name(p, name(p), false);
  } else {
    size_t at = p->at;
    while (p->at < p->size && !delimiter(p->s[p->at])) {
      char d = p->s[p->at];
      if (d == '\'' || d == '"' || d == '$' || d == '`' || d == '\\') {
        fail(p, "quote special characters; word fragments do not concatenate");
        return nullptr;
      }
      ++p->at;
    }
    n = node(p, RILL_STRING, at);
    if (n && !append(p, &n->text, p->s + at, p->at - at))
      return nullptr;
  }
  if (p->at < p->size && !delimiter(p->s[p->at]))
    fail(p, "command word fragments cannot be joined; quote the whole word");
  return n;
}
static RillNode *pipeline(Parser *p) {
  RillNode *plan = node(p, RILL_PLAN, p->at);
  if (!plan)
    return nullptr;
  RillNode **stage_tail = &plan->children;
  for (;;) {
    space(p, true);
    if (!take(p, '^')) {
      expected(p, "expected '^' before command name");
      return nullptr;
    }
    RillNode *stage = node(p, RILL_STAGE, p->at);
    if (!stage)
      return nullptr;
    *stage_tail = stage;
    stage_tail = &stage->next;
    space(p, false);
    stage->children = word(p);
    if (!stage->children)
      return nullptr;
    if (stage->children->kind == RILL_SPREAD) {
      fail(p, "command name cannot use list spread");
      return nullptr;
    }
    RillNode **tail = &stage->children->next;
    for (;;) {
      space(p, false);
      if (p->at == p->size)
        break;
      char c = p->s[p->at];
      if (c == '\n' || c == ';' || c == '}' || c == '|')
        break;
      size_t at = p->at;
      if (digit(c)) {
        size_t end = at;
        while (end < p->size && digit(p->s[end]))
          ++end;
        if (end < p->size && (p->s[end] == '>' || p->s[end] == '<') &&
            !(end == at + 1 && c == '2' && p->s[end] == '>')) {
          fail(p, "unsupported descriptor redirection");
          return nullptr;
        }
      }
      int op = -1;
      if (p->size - p->at >= 4 && !memcmp(p->s + p->at, "2>&1", 4)) {
        op = RILL_ERROR_TO_OUTPUT;
        p->at += 4;
      } else {
        bool err = p->size - p->at >= 2 && p->s[p->at] == '2' &&
                   p->s[p->at + 1] == '>';
        if (err)
          ++p->at;
        if (take(p, '>'))
          op = take(p, '>') ? (err ? RILL_ERROR_APPEND : RILL_APPEND)
                            : (err ? RILL_ERROR_OUTPUT : RILL_OUTPUT);
        else if (take(p, '<'))
          op = RILL_INPUT;
      }
      if (op >= 0 && op != RILL_ERROR_TO_OUTPUT && p->at < p->size &&
          p->s[p->at] == '&') {
        fail(p, "unsupported descriptor duplication");
        return nullptr;
      }
      RillNode *part = {};
      if (op >= 0) {
        part = node(p, RILL_REDIRECT, at);
        if (!part)
          return nullptr;
        part->redirect = (RillRedirect)op;
        if (op != RILL_ERROR_TO_OUTPUT) {
          space(p, false);
          part->children = word(p);
          if (!part->children)
            return nullptr;
          if (part->children->kind == RILL_SPREAD) {
            fail(p, "redirection path cannot use list spread");
            return nullptr;
          }
        } else if (p->at < p->size && !delimiter(p->s[p->at])) {
          fail(p, "unsupported descriptor redirection");
          return nullptr;
        }
      } else
        part = word(p);
      if (!part)
        return nullptr;
      *tail = part;
      tail = &part->next;
    }
    size_t saved = p->at;
    space(p, true);
    if (!take(p, '|')) {
      p->at = saved;
      break;
    }
    if (take(p, '>')) {
      fail(p,
           "use 'stream' or 'through' to connect commands to a value pipeline");
      return nullptr;
    }
  }
  return plan;
}
static RillNode *declaration(Parser *p, bool top) {
  space(p, true);
  RillNode *n = nullptr;
  if (keyword(p, "export")) {
    if (!top || !require(p, '{', "expected '{' after 'export'")) {
      fail(p, "'export' is only allowed at the top level");
      return nullptr;
    }
    n = node(p, RILL_EXPORT, p->at);
    if (!n)
      return nullptr;
    n->children = aggregate(p, false, '}');
    if (!n->children)
      return nullptr;
    for (RillNode *field = n->children->children; field; field = field->next) {
      const RillNode *value = field->children;
      while (value && value->kind == RILL_FIELD)
        value = value->children;
      if (!value || value->kind != RILL_NAME)
        fail(p, "exports require existing bindings or namespace fields");
    }
  } else if (p->at < p->size && p->s[p->at] == '^') {
    RillNode *plan = pipeline(p), *run = node(p, RILL_BUILTIN, p->at);
    n = node(p, RILL_CALL, p->at);
    if (!plan || !run || !n || !append(p, &run->text, "run", 3))
      return nullptr;
    n->offset = plan->offset;
    n->children = run;
    run->next = plan;
  } else if (keyword(p, "let")) {
    n = node(p, RILL_BIND, p->at);
    if (!n)
      return nullptr;
    n->pattern = pattern(p, false);
    if (!n->pattern || !require(p, '=', "expected '=' after binding pattern"))
      return nullptr;
    n->children = expression(p);
  } else if (recursive_group(p)) {
    (void)keyword(p, "rec");
    n = node(p, RILL_REC, p->at);
    if (!n || !require(p, '{', "expected '{' after 'rec'"))
      return nullptr;
    // Invalid nested groups still recurse before member validation below.
    if (++p->depth > MAX_SYNTAX_DEPTH) {
      fail(p, "syntax nesting limit exceeded");
      --p->depth;
      return nullptr;
    }
    n->children = statements(p, false, true);
    --p->depth;
    if (!n->children)
      fail(p, "empty recursive group");
    for (RillNode *c = n->children; c; c = c->next)
      if (c->kind != RILL_DECLARE)
        fail(p, "recursive groups can contain only named functions");
  } else if (keyword(p, "struct") || keyword(p, "enum")) {
    bool sum = p->at >= 4 && !memcmp(p->s + p->at - 4, "enum", 4);
    if (!top)
      fail(p, "type declarations are top-level only");
    space(p, true);
    n = binding_name(p, name(p), false);
    if (!n)
      return nullptr;
    n->kind = sum ? RILL_ENUM : RILL_STRUCT;
    if (!require(p, '{', "expected '{' after type name"))
      return nullptr;
    RillNode **tail = &n->children;
    space(p, true);
    while (!take(p, '}')) {
      *tail = name(p);
      if (!*tail)
        return nullptr;
      RillNode *field = *tail;
      tail = &field->next;
      space(p, true);
      if (sum && take(p, '{')) {
        RillNode **ft = &field->children;
        space(p, true);
        while (!take(p, '}')) {
          *ft = name(p);
          if (!*ft)
            return nullptr;
          ft = &(*ft)->next;
          space(p, true);
          if (take(p, '}'))
            break;
          if (!require(p, ',', "expected ',' or '}'"))
            return nullptr;
          space(p, true);
        }
      }
      space(p, true);
      if (take(p, '}'))
        break;
      if (!require(p, ',', "expected ',' or '}'"))
        return nullptr;
      space(p, true);
    }
    if (sum && !n->children)
      fail(p, "enum requires at least one case");
  } else if (keyword(p, "import")) {
    if (!top)
      fail(p, "imports are top-level only");
    space(p, true);
    if (p->at == p->size || (p->s[p->at] != '\'' && p->s[p->at] != '"')) {
      expected(p, "expected literal module path");
      return nullptr;
    }
    RillNode *path = string(p);
    space(p, true);
    if (!keyword(p, "as")) {
      expected(p, "expected 'as' after module path");
      return nullptr;
    }
    space(p, true);
    n = binding_name(p, name(p), false);
    if (!n)
      return nullptr;
    n->kind = RILL_IMPORT;
    n->children = path;
  } else if (keyword(p, "fn")) {
    space(p, true);
    n = binding_name(p, name(p), false);
    if (!n)
      return nullptr;
    n->kind = RILL_DECLARE;
    if (p->at < p->size && p->s[p->at] != ' ' && p->s[p->at] != '\t' &&
        p->s[p->at] != '\r' && p->s[p->at] != '\n' && p->s[p->at] != '#') {
      fail(p, "expected whitespace before function parameters");
      return nullptr;
    }
    n->children = function(p, false);
  } else
    n = expression(p);
  return n;
}
static RillNode *statement_items(Parser *p, bool top, bool block) {
  RillNode *first = nullptr, **tail = &first;
  bool exported = false;
  while (!p->out.diagnostic.kind) {
    space(p, true);
    if (block && take(p, '}'))
      return first;
    if (p->at == p->size) {
      if (block)
        expected(p, "expected '}'");
      break;
    }
    RillNode *n = declaration(p, top);
    if (!n)
      break;
    if (n->kind == RILL_EXPORT) {
      if (exported)
        fail(p, "only one export table is allowed");
      exported = true;
    }
    *tail = n;
    tail = &n->next;
    space(p, false);
    if (block && take(p, '}'))
      return first;
    if (p->at < p->size && !take(p, ';') && !take(p, '\n')) {
      expected(p, "expected a newline or ';' between statements");
      break;
    }
  }
  return first;
}
static RillNode *statements(Parser *p, bool top, bool block) {
  bool outer = p->soft_lines;
  p->soft_lines = false;
  RillNode *n = statement_items(p, top, block);
  p->soft_lines = outer;
  return n;
}
static void validate_depth(Parser *p) {
  struct {
    const RillNode *node;
    unsigned phase;
  } stack[MAX_SYNTAX_DEPTH];
  size_t depth = 1;
  stack[0].node = p->out.first;
  stack[0].phase = 0;
  while (depth && !p->out.diagnostic.kind) {
    const RillNode *n = stack[depth - 1].node;
    if (!n) {
      --depth;
      continue;
    }
    const RillNode *child = nullptr;
    if (stack[depth - 1].phase == 0) {
      stack[depth - 1].phase = 1;
      child = n->pattern;
    } else if (stack[depth - 1].phase == 1) {
      stack[depth - 1].phase = 2;
      child = n->children;
    } else {
      stack[depth - 1].node = n->next;
      stack[depth - 1].phase = 0;
      continue;
    }
    if (child) {
      if (depth == MAX_SYNTAX_DEPTH) {
        p->at = n->offset;
        fail(p, "syntax nesting limit exceeded");
        return;
      }
      stack[depth].node = child;
      stack[depth].phase = 0;
      ++depth;
    }
  }
}
// Collect binding leaves only; nominal descriptor paths are lexical references.
static void pattern_names(RillNode *n, RillNode **names, size_t *count) {
  if (!n)
    return;
  if (n->kind == RILL_NAME && strcmp(n->text.data, "_") != 0) {
    if (names)
      names[*count] = n;
    ++*count;
  }
  for (RillNode *c = n->children; c; c = c->next)
    pattern_names(c, names, count);
}
static int pattern_name_order(const void *left, const void *right) {
  const RillNode *const *a = left, *const *b = right;
  return strcmp((*a)->text.data, (*b)->text.data);
}
static void validate_patterns(Parser *p) {
  size_t capacity = 0, bytes = {};
  for (RillNode *n = p->out.allocated; n; n = n->allocated_next) {
    if (n->kind != RILL_BIND && n->kind != RILL_FUNCTION && n->kind != RILL_ARM)
      continue;
    size_t count = 0;
    pattern_names(n->pattern, nullptr, &count);
    if (count > capacity)
      capacity = count;
  }
  if (capacity < 2)
    return;
  if (ckd_mul(&bytes, capacity, sizeof(RillNode *))) {
    memory_error(p);
    return;
  }
  RillNode **names = malloc(bytes);
  if (!names) {
    memory_error(p);
    return;
  }
  for (RillNode *n = p->out.allocated; n && !p->out.diagnostic.kind;
       n = n->allocated_next) {
    if (n->kind != RILL_BIND && n->kind != RILL_FUNCTION && n->kind != RILL_ARM)
      continue;
    size_t count = 0;
    pattern_names(n->pattern, names, &count);
    qsort(names, count, sizeof(*names), pattern_name_order);
    for (size_t i = 1; i < count; ++i)
      if (!strcmp(names[i - 1]->text.data, names[i]->text.data)) {
        p->at = names[i]->offset;
        fail(p, "duplicate name in pattern");
        break;
      }
  }
  free(names);
}
RillSyntax rill_syntax_parse(const RillSource *source) {
  Parser p = {.s = source->bytes.data, .size = source->bytes.size};
  p.out.first = statements(&p, true, false);
  validate_depth(&p);
  if (!p.out.diagnostic.kind)
    validate_patterns(&p);
  if (source->name &&
      !append(&p, &p.out.name, source->name, strlen(source->name)))
    memory_error(&p);
  if (!append(&p, &p.out.source, p.s, p.size))
    memory_error(&p);
  size_t bytes = sizeof(RillSyntax);
  bool overflow = ckd_add(&bytes, bytes, p.out.source.capacity) ||
                  ckd_add(&bytes, bytes, p.out.name.capacity);
  for (RillNode *n = p.out.allocated; n && !overflow; n = n->allocated_next)
    overflow = ckd_add(&bytes, bytes, n->text.capacity);
  for (struct RillNodes *b = p.out.nodes; b && !overflow; b = b->next)
    overflow =
        ckd_add(&bytes, bytes, sizeof(*b) + b->capacity * sizeof(RillNode));
  if (overflow)
    memory_error(&p);
  else
    p.out.allocation = bytes;
  return p.out;
}
void rill_syntax_clear(RillSyntax *s) {
  RillNode *n = s->allocated;
  while (n) {
    RillNode *next = n->allocated_next;
    rill_text_clear(&n->text);
    n = next;
  }
  while (s->nodes) {
    struct RillNodes *next = s->nodes->next;
    free(s->nodes);
    s->nodes = next;
  }
  rill_text_clear(&s->name);
  rill_text_clear(&s->source);
  *s = (RillSyntax){};
}
