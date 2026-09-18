#include "syntax.h"
#include "diagnostic.h"
#include "source.h"
#include "text/text.h"
#include <stdckdint.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

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
  RillNode *n = malloc(sizeof(*n));
  if (!n) {
    memory_error(p);
    return nullptr;
  }
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
          unsigned v;
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
  if (keyword(p, "job")) {
    space(p, true);
    if (!take(p, '{')) {
      expected(p, "expected { after job");
      return nullptr;
    }
    space(p, true);
    RillNode *n = pipeline(p);
    space(p, true);
    if (!take(p, '}'))
      expected(p, "expected } after pipeline");
    return n;
  }
  if (take(p, '(')) {
    space(p, true);
    if (take(p, ')'))
      return node(p, RILL_UNIT, at);
    RillNode *n = expression(p);
    space(p, true);
    if (!take(p, ')'))
      expected(p, "expected )");
    return n;
  }
  if (take(p, '[')) {
    RillNode *n = node(p, RILL_LIST, at);
    if (!n)
      return nullptr;
    RillNode **tail = &n->children;
    space(p, true);
    while (!take(p, ']')) {
      *tail = expression(p);
      if (!*tail)
        return nullptr;
      tail = &(*tail)->next;
      space(p, true);
      if (take(p, ']'))
        break;
      if (!take(p, ',')) {
        expected(p, "expected , or ]");
        return nullptr;
      }
      space(p, true);
    }
    return n;
  }
  if (digit(c) || c == '-') {
    bool negative = take(p, '-');
    int64_t value = 0;
    bool any = false, underscore = false;
    while (p->at < p->size && (digit(p->s[p->at]) || p->s[p->at] == '_')) {
      char d = p->s[p->at++];
      if (d == '_') {
        if (!any || underscore) {
          fail(p, "invalid numeric separator");
          return nullptr;
        }
        underscore = true;
        continue;
      }
      any = true;
      underscore = false;
      if (ckd_mul(&value, value, 10) || ckd_sub(&value, value, d - '0')) {
        fail(p, "integer out of range");
        return nullptr;
      }
    }
    if (!any || underscore || (!negative && value == INT64_MIN)) {
      fail(p, "invalid integer");
      return nullptr;
    }
    RillNode *n = node(p, RILL_INTEGER, at);
    if (n)
      n->integer = negative ? value : -value;
    return n;
  }
  return name(p);
}
static RillNode *expression(Parser *p) {
  if (++p->depth > 256) {
    fail(p, "syntax nesting limit exceeded");
    --p->depth;
    return nullptr;
  }
  RillNode *n = atom(p);
  while (n && !p->out.diagnostic.kind) {
    size_t saved = p->at;
    space(p, false);
    if (take(p, '[')) {
      RillNode *index = expression(p), *access = node(p, RILL_INDEX, n->offset);
      space(p, true);
      if (!take(p, ']'))
        expected(p, "expected ] after index");
      if (!index || !access) {
        n = nullptr;
        break;
      }
      access->children = n;
      n->next = index;
      n = access;
      continue;
    }
    if (take(p, '.')) {
      RillNode *field = name(p);
      if (!field) {
        n = nullptr;
        break;
      }
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
      if (!arg || !call) {
        n = nullptr;
        break;
      }
      call->children = n;
      n->next = arg;
      n = call;
      if (empty)
        break;
      space(p, true);
      if (take(p, ')'))
        break;
      if (!take(p, ',')) {
        expected(p, "expected , or )");
        break;
      }
      space(p, true);
      if (take(p, ')'))
        break;
    } while (!p->out.diagnostic.kind);
  }
  --p->depth;
  return n;
}
static bool delimiter(char c) {
  return c == ' ' || c == '\t' || c == '\r' || c == '\n' || c == '|' ||
         c == ';' || c == '{' || c == '}' || c == '<' || c == '>';
}
static RillNode *word(Parser *p) {
  if (p->at == p->size || delimiter(p->s[p->at])) {
    expected(p, "expected command word");
    return nullptr;
  }
  RillNode *n;
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
      n = name(p);
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
      RillNode *part;
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
RillSyntax rill_syntax_parse(const RillSource *source) {
  Parser p = {.s = source->bytes.data, .size = source->bytes.size};
  RillNode **tail = &p.out.first;
  while (!p.out.diagnostic.kind) {
    space(&p, true);
    if (p.at == p.size)
      break;
    RillNode *n;
    if (p.s[p.at] == '^') {
      RillNode *plan = pipeline(&p), *run = node(&p, RILL_NAME, p.at);
      n = node(&p, RILL_CALL, p.at);
      if (!plan || !run || !n || !append(&p, &run->text, "run", 3))
        break;
      n->offset = plan->offset;
      n->children = run;
      run->next = plan;
    } else if (keyword(&p, "let")) {
      space(&p, false);
      n = name(&p);
      if (!n)
        break;
      n->kind = RILL_BIND;
      space(&p, false);
      if (!take(&p, '=')) {
        expected(&p, "expected = in binding");
        break;
      }
      n->children = expression(&p);
      if (!n->children)
        break;
    } else
      n = expression(&p);
    if (!n)
      break;
    *tail = n;
    tail = &n->next;
    space(&p, false);
    if (p.at < p.size && !take(&p, ';') && !take(&p, '\n')) {
      expected(&p, "expected statement separator");
      break;
    }
  }
  return p.out;
}
void rill_syntax_clear(RillSyntax *s) {
  RillNode *n = s->allocated;
  while (n) {
    RillNode *next = n->allocated_next;
    rill_text_clear(&n->text);
    free(n);
    n = next;
  }
  *s = (RillSyntax){};
}
