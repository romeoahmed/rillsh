/* Whitespace application needs lookahead past trivia, but no serialized lexer
 * state. Newlines are application gaps only in a grammar-selected soft
 * expression context. mark_end keeps the following atom outside the gap token.
 */
#include "tree_sitter/parser.h"
#include <stddef.h>
#include <string.h>

enum Token {
  APPLICATION_GAP,
  SOFT_APPLICATION_GAP,
  PIPELINE_GAP,
  OPERATOR_GAP,
  COMMAND_WORD,
  COMMAND_GAP,
  PARAMETER_GAP
};

void *tree_sitter_rill_external_scanner_create(void) { return NULL; }
void tree_sitter_rill_external_scanner_destroy(void *payload) { (void)payload; }
unsigned tree_sitter_rill_external_scanner_serialize(void *payload,
                                                     char *buffer) {
  (void)payload;
  (void)buffer;
  return 0;
}
void tree_sitter_rill_external_scanner_deserialize(void *payload,
                                                   const char *buffer,
                                                   unsigned length) {
  (void)payload;
  (void)buffer;
  (void)length;
}
static bool name_start(int32_t c) {
  return (c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z') || c == '_';
}
static bool atom(TSLexer *lexer, bool *operator) {
  int32_t c = lexer->lookahead;
  if (c != 0 && c < 128 && strchr("+-*/=!<>", (char)c) != NULL) {
    *operator = true;
    return false;
  }
  if (c == '(' || c == '[' || c == '{' || c == '\'' || c == '"' ||
      (c >= '0' && c <= '9'))
    return true;
  if (!name_start(c))
    return false;
  char word[16] = {0};
  size_t length = 0;
  while (name_start(lexer->lookahead) ||
         (lexer->lookahead >= '0' && lexer->lookahead <= '9')) {
    if (length + 1 < sizeof word)
      word[length] = (char)lexer->lookahead;
    length++;
    lexer->advance(lexer, false);
  }
  if (length >= sizeof word)
    return true;
  if (strcmp(word, "and") == 0 || strcmp(word, "or") == 0 ||
      strcmp(word, "with") == 0) {
    *operator = true;
    return false;
  }
  const char *reserved[] = {
      "let",  "fn",   "if",     "then", "else",   "match", "of", "struct",
      "enum", "with", "import", "as",   "export", "and",   "or", "not"};
  for (size_t i = 0; i < sizeof reserved / sizeof reserved[0]; i++) {
    if (strcmp(word, reserved[i]) == 0)
      return false;
  }
  return true;
}
bool tree_sitter_rill_external_scanner_scan(void *payload, TSLexer *lexer,
                                            const bool *valid) {
  (void)payload;
  if (valid[COMMAND_WORD]) {
    bool separated = false;
    while (lexer->lookahead == ' ' || lexer->lookahead == '\t' ||
           lexer->lookahead == '\r') {
      separated = true;
      lexer->advance(lexer, true);
    }
    if (separated && lexer->lookahead == '#')
      return false;
    bool found = false;
    if (lexer->lookahead == '2') {
      lexer->advance(lexer, false);
      if (lexer->lookahead == '>')
        return false;
      found = true;
    }
    unsigned dots = 0;
    while (lexer->lookahead == '.') {
      dots++;
      found = true;
      lexer->advance(lexer, false);
    }
    if (dots == 3 && lexer->lookahead == '$')
      return false;
    while (!lexer->eof(lexer) &&
           (lexer->lookahead >= 128 ||
            strchr(" \t\r\n;|<>{}\"'$^", (char)lexer->lookahead) == NULL)) {
      found = true;
      lexer->advance(lexer, false);
    }
    if (found) {
      lexer->mark_end(lexer);
      lexer->result_symbol = COMMAND_WORD;
      return true;
    }
    return false;
  }
  if (!valid[APPLICATION_GAP] && !valid[SOFT_APPLICATION_GAP] &&
      !valid[PIPELINE_GAP] && !valid[OPERATOR_GAP] && !valid[COMMAND_GAP] &&
      !valid[PARAMETER_GAP])
    return false;
  bool soft = valid[SOFT_APPLICATION_GAP] || valid[PIPELINE_GAP] ||
              valid[OPERATOR_GAP] || valid[PARAMETER_GAP];
  bool newline = false;
  bool gap = false;
  while (true) {
    int32_t c = lexer->lookahead;
    if (c == ' ' || c == '\t' || c == '\r' || (soft && c == '\n')) {
      gap = true;
      if (c == '\n')
        newline = true;
      lexer->advance(lexer, false);
    } else if (soft && c == '#') {
      do {
        lexer->advance(lexer, false);
      } while (!lexer->eof(lexer) && lexer->lookahead != '\n');
    } else
      break;
  }
  if (!gap)
    return false;
  lexer->mark_end(lexer);
  if (valid[PIPELINE_GAP] && lexer->lookahead == '|') {
    lexer->result_symbol = PIPELINE_GAP;
    return true;
  }
  if (valid[COMMAND_GAP] && !newline) {
    int32_t c = lexer->lookahead;
    if (c != 0 && c != '#' &&
        (c >= 128 || strchr(";|<>{}^", (char)c) == NULL)) {
      if (c == '2') {
        lexer->advance(lexer, false);
        if (lexer->lookahead == '>')
          return false;
      }
      lexer->result_symbol = COMMAND_GAP;
      return true;
    }
  }
  if (valid[PARAMETER_GAP] && lexer->lookahead == '-') {
    lexer->result_symbol = PARAMETER_GAP;
    return true;
  }
  bool operator = false;
  bool is_atom = atom(lexer, &operator);
  if (operator && valid[OPERATOR_GAP]) {
    lexer->result_symbol = OPERATOR_GAP;
    return true;
  }
  if (newline && !valid[SOFT_APPLICATION_GAP] && !valid[PARAMETER_GAP])
    return false;
  if (!is_atom)
    return false;
  lexer->result_symbol =
      valid[PARAMETER_GAP] ? PARAMETER_GAP
                           : (valid[SOFT_APPLICATION_GAP] ? SOFT_APPLICATION_GAP
                                                          : APPLICATION_GAP);
  return true;
}
