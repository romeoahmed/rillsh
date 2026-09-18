#include "syntax.h"
#include "diagnostic.h"
#include "source.h"
#include "text/text.h"
#include <errno.h>
#include <math.h>
#include <stdckdint.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

struct RillNodes {
  struct RillNodes *next;
  size_t used, capacity;
  RillNode items[];
};
typedef struct {
  const char *s;
  size_t size, at;
  unsigned depth;
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
          expected(p, "expected { after Unicode escape");
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
            fail(p, "Unicode scalar out of range");
            return nullptr;
          }
          cp = cp * 16 + v;
        }
        if (!take(p, '}')) {
          expected(p, "expected } after Unicode escape");
          return nullptr;
        }
        if (!digits || cp > 0x10ffff || (cp >= 0xd800 && cp <= 0xdfff)) {
          fail(p, "invalid Unicode scalar");
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
  if (n && !append(p, &n->text, p->s + at, p->at - at))
    return nullptr;
  return n;
}
static RillNode *binding_name(Parser *p, RillNode *n, bool wildcard) {
  if (!n)
    return nullptr;
  static const char *const reserved[] = {
      "let",    "fn",     "rec",   "if",   "then", "else",   "do",
      "match",  "struct", "enum",  "with", "job",  "import", "as",
      "export", "true",   "false", "null", "and",  "or",     "not"};
  bool invalid = !wildcard && !strcmp(n->text.data, "_");
  for (size_t i = 0; i < sizeof(reserved) / sizeof(*reserved); ++i)
    invalid |= !strcmp(n->text.data, reserved[i]);
  if (invalid) {
    fail(p, "reserved word is not a binding name");
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
static RillNode *pattern(Parser *p);
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
        fail(p, "invalid numeric separator");
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
    expected(p, "expected numeric digits");
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
        fail(p, "invalid finite Float");
    } else {
      n->integer = strtoll(text.data, &end, 10);
      if (errno == ERANGE || *end)
        fail(p, "integer out of range");
    }
    return n;
  }
  return nullptr;
}
static RillNode *aggregate(Parser *p, bool pat, char closing) {
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
      item = pat ? pattern(p) : expression(p);
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
        item->children = pat ? pattern(p) : expression(p);
      else if (pat && shorthand) {
        if (!binding_name(p, item, true))
          return nullptr;
        item->children = node(p, RILL_NAME, item->offset);
        if (item->children &&
            !append(p, &item->children->text, item->text.data, item->text.size))
          return nullptr;
      } else
        expected(p, "expected : after record key");
      if (!item->children)
        return nullptr;
      for (RillNode *old = n->children; old; old = old->next)
        if (old->text.size == item->text.size &&
            (!item->text.size ||
             !memcmp(old->text.data, item->text.data, item->text.size)))
          fail(p, "duplicate record key");
    }
    if (!item || p->out.diagnostic.kind)
      return nullptr;
    *tail = item;
    tail = &item->next;
    space(p, true);
    if (take(p, closing))
      break;
    if (!require(p, ',', "expected comma or closing delimiter"))
      return nullptr;
    space(p, true);
  }
  return n;
}
static RillNode *pattern(Parser *p) {
  if (++p->depth > 256) {
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
      n = pattern(p);
      (void)require(p, ')', "expected )");
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
    space(p, true);
    if (n && (take(p, '{') || qualified)) {
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
static RillNode *function(Parser *p) {
  RillNode *n = node(p, RILL_FUNCTION, p->at);
  if (!n || !require(p, '(', "expected parameter list"))
    return nullptr;
  RillNode *last = n;
  space(p, true);
  if (take(p, ')'))
    last->pattern = node(p, RILL_UNIT, p->at);
  else
    for (;;) {
      last->pattern = pattern(p);
      if (!last->pattern)
        return nullptr;
      space(p, true);
      if (take(p, ')'))
        break;
      if (!require(p, ',', "expected , or )"))
        return nullptr;
      space(p, true);
      if (take(p, ')'))
        break;
      last->children = node(p, RILL_FUNCTION, p->at);
      last = last->children;
      if (!last)
        return nullptr;
    }
  space(p, true);
  if (!token(p, "=>")) {
    expected(p, "expected =>");
    return nullptr;
  }
  last->children = expression(p);
  return last->children ? n : nullptr;
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
  if (keyword(p, "fn"))
    return function(p);
  if (keyword(p, "do")) {
    RillNode *n = node(p, RILL_BLOCK, at);
    if (!n || !require(p, '{', "expected { after do"))
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
      expected(p, "expected then");
      return nullptr;
    }
    n->children->next = expression(p);
    if (!n->children->next)
      return nullptr;
    space(p, true);
    if (!keyword(p, "else")) {
      expected(p, "expected else");
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
    if (!n->children || !require(p, '{', "expected match arms"))
      return nullptr;
    RillNode **tail = &n->children->next;
    space(p, true);
    while (!take(p, '}')) {
      RillNode *arm = node(p, RILL_ARM, p->at);
      if (!arm)
        return nullptr;
      *tail = arm;
      tail = &arm->next;
      arm->pattern = pattern(p);
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
        expected(p, "expected => in match");
        return nullptr;
      }
      arm->children->next = expression(p);
      if (!arm->children->next)
        return nullptr;
      space(p, true);
      if (take(p, '}'))
        break;
      if (!require(p, ',', "expected , or }"))
        return nullptr;
      space(p, true);
    }
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
    if (!require(p, '{', "expected { after job"))
      return nullptr;
    RillNode *n = pipeline(p);
    (void)require(p, '}', "expected } after pipeline");
    return n;
  }
  if (take(p, '(')) {
    space(p, true);
    if (take(p, ')'))
      return node(p, RILL_UNIT, at);
    RillNode *n = expression(p);
    (void)require(p, ')', "expected )");
    if (n)
      n->grouped = true;
    return n;
  }
  if (take(p, '['))
    return aggregate(p, false, ']');
  if (take(p, '{'))
    return aggregate(p, false, '}');
  return binding_name(p, name(p), false);
}
static RillNode *suffix(Parser *p) {
  space(p, true);
  size_t at = p->at;
  bool neg = p->at < p->size && p->s[p->at] == '-' &&
             !(p->at + 1 < p->size && digit(p->s[p->at + 1]));
  if ((neg && take(p, '-')) || keyword(p, "not")) {
    RillNode *n = node(p, RILL_UNARY, at);
    if (!n || !append(p, &n->text, neg ? "-" : "not", neg ? 1 : 3))
      return nullptr;
    n->children = binary(p, 9);
    return n;
  }
  RillNode *n = atom(p);
  while (n && !p->out.diagnostic.kind) {
    size_t saved = p->at;
    space(p, false);
    if (take(p, '[')) {
      RillNode *index = expression(p), *access = node(p, RILL_INDEX, n->offset);
      (void)require(p, ']', "expected ] after index");
      if (!index || !access)
        return nullptr;
      access->children = n;
      n->next = index;
      n = access;
      continue;
    }
    if (take(p, '.')) {
      RillNode *field = name(p);
      if (!field)
        return nullptr;
      field->kind = RILL_FIELD;
      field->children = n;
      n = field;
      continue;
    }
    if (!take(p, '(')) {
      p->at = saved;
      break;
    }
    space(p, true);
    bool empty = take(p, ')');
    do {
      RillNode *arg = empty ? node(p, RILL_UNIT, p->at) : expression(p);
      RillNode *call = node(p, RILL_CALL, n->offset);
      if (!arg || !call)
        return nullptr;
      call->children = n;
      n->next = arg;
      n = call;
      if (empty)
        break;
      space(p, true);
      if (take(p, ')'))
        break;
      if (!require(p, ',', "expected , or )"))
        return nullptr;
      space(p, true);
      if (take(p, ')'))
        break;
    } while (!p->out.diagnostic.kind);
  }
  return n;
}
static RillNode *binary(Parser *p, unsigned minimum) {
  if (++p->depth > 256) {
    fail(p, "syntax nesting limit exceeded");
    --p->depth;
    return nullptr;
  }
  RillNode *left = suffix(p);
  while (left && !p->out.diagnostic.kind) {
    size_t saved = p->at;
    space(p, false);
    if (p->at < p->size && p->s[p->at] == '\n') {
      space(p, true);
      if (p->size - p->at < 2 || memcmp(p->s + p->at, "|>", 2) != 0) {
        p->at = saved;
        break;
      }
    }
    static const struct {
      const char *text;
      unsigned prec;
      bool word;
    } ops[] = {{"|>", 1, false}, {"with", 2, true}, {"or", 3, true},
               {"and", 4, true}, {"==", 5, false},  {"!=", 5, false},
               {"<=", 5, false}, {">=", 5, false},  {"<", 5, false},
               {">", 5, false},  {"+", 6, false},   {"-", 6, false},
               {"*", 7, false},  {"/", 7, false}};
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
    if (ops[i].prec == 5 && left->kind == RILL_BINARY && left->integer == 5 &&
        !left->grouped) {
      fail(p, "comparisons do not chain");
      break;
    }
    RillNode *n = node(p,
                       i == 0   ? RILL_PIPE
                       : i == 1 ? RILL_WITH
                                : RILL_BINARY,
                       left->offset);
    if (!n || !append(p, &n->text, ops[i].text, strlen(ops[i].text))) {
      left = nullptr;
      break;
    }
    n->integer = ops[i].prec;
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
      expected(p, "spread requires substitution");
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
      n = expression(p);
      space(p, true);
      if (!take(p, ')'))
        expected(p, "expected ) after substitution");
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
    fail(p, "adjacent word fragments are not allowed");
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
      expected(p, "expected ^ before executable");
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
      fail(p, "executable cannot be spread");
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
        part->integer = op;
        if (op != RILL_ERROR_TO_OUTPUT) {
          space(p, false);
          part->children = word(p);
          if (!part->children)
            return nullptr;
          if (part->children->kind == RILL_SPREAD) {
            fail(p, "redirection cannot be spread");
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
      fail(p, "use an explicit stream bridge after commands");
      return nullptr;
    }
  }
  return plan;
}
static RillNode *declaration(Parser *p, bool top) {
  space(p, true);
  bool exported = keyword(p, "export");
  if (exported) {
    if (!top)
      fail(p, "export is top-level only");
    space(p, true);
  }
  RillNode *n = nullptr;
  if (p->at < p->size && p->s[p->at] == '^') {
    RillNode *plan = pipeline(p), *run = node(p, RILL_NAME, p->at);
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
    n->pattern = pattern(p);
    if (!n->pattern || !require(p, '=', "expected = in binding"))
      return nullptr;
    n->children = expression(p);
  } else if (keyword(p, "rec")) {
    n = node(p, RILL_REC, p->at);
    if (!n || !require(p, '{', "expected recursive group"))
      return nullptr;
    // Invalid nested groups still recurse before member validation below.
    if (++p->depth > 256) {
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
        fail(p, "rec accepts named functions only");
  } else if (keyword(p, "struct") || keyword(p, "enum")) {
    bool sum = p->at >= 4 && !memcmp(p->s + p->at - 4, "enum", 4);
    if (!top)
      fail(p, "type declarations are top-level only");
    space(p, true);
    n = binding_name(p, name(p), false);
    if (!n)
      return nullptr;
    n->kind = sum ? RILL_ENUM : RILL_STRUCT;
    if (!require(p, '{', "expected type fields"))
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
          if (!require(p, ',', "expected , or }"))
            return nullptr;
          space(p, true);
        }
      }
      space(p, true);
      if (take(p, '}'))
        break;
      if (!require(p, ',', "expected , or }"))
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
      expected(p, "expected as");
      return nullptr;
    }
    space(p, true);
    n = binding_name(p, name(p), false);
    if (!n)
      return nullptr;
    n->kind = RILL_IMPORT;
    n->children = path;
  } else {
    size_t at = p->at;
    if (keyword(p, "fn")) {
      space(p, true);
      if (p->at < p->size && ident(p->s[p->at])) {
        n = binding_name(p, name(p), false);
        if (!n)
          return nullptr;
        n->kind = RILL_DECLARE;
        n->children = function(p);
      } else {
        p->at = at;
        n = expression(p);
      }
    } else
      n = expression(p);
  }
  if (n) {
    n->exported = exported;
    if (exported && n->kind != RILL_BIND && n->kind != RILL_DECLARE &&
        n->kind != RILL_STRUCT && n->kind != RILL_ENUM)
      fail(p, "expected export declaration");
  }
  return n;
}
static RillNode *statements(Parser *p, bool top, bool block) {
  RillNode *first = nullptr, **tail = &first;
  while (!p->out.diagnostic.kind) {
    space(p, true);
    if (block && take(p, '}'))
      return first;
    if (p->at == p->size) {
      if (block)
        expected(p, "expected }");
      break;
    }
    RillNode *n = declaration(p, top);
    if (!n)
      break;
    *tail = n;
    tail = &n->next;
    space(p, false);
    if (block && take(p, '}'))
      return first;
    if (p->at < p->size && !take(p, ';') && !take(p, '\n')) {
      expected(p, "expected statement separator");
      break;
    }
  }
  return first;
}
static void validate_depth(Parser *p) {
  struct {
    const RillNode *node;
    unsigned phase;
  } stack[257];
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
      if (depth == 256) {
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
RillSyntax rill_syntax_parse(const RillSource *source) {
  Parser p = {.s = source->bytes.data, .size = source->bytes.size};
  p.out.first = statements(&p, true, false);
  validate_depth(&p);
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
