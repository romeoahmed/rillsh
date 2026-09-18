/**
 * @file
 * @brief Whole-entry parsing and source-ownership regressions.
 *
 * Matrices distinguish complete, incomplete, and invalid input. Depth and
 * ownership cases exercise malformed nesting and release of the original
 * source.
 */
#include "syntax/syntax.h"
#include "../support/check.h"
#include "diagnostic.h"
#include "source.h"
#include "text/text.h"
#include <stddef.h>
#include <stdio.h>
#include <string.h>

static void nesting() {
  const struct {
    const char *prefix, *suffix;
  } cases[] = {
      {"rec { loop x =>", "}"}, {"do {", "}"}, {"[", "]"}, {"not ", ""}};
  for (size_t i = 0; i < sizeof(cases) / sizeof(*cases); ++i) {
    [[gnu::cleanup(rill_text_clear)]] RillBuffer text = {};
    for (size_t j = 0; j < 1024; ++j)
      CHECK(rill_text_append(&text, cases[i].prefix, strlen(cases[i].prefix)));
    CHECK(rill_text_append(&text, "true", 4));
    for (size_t j = 0; j < 1024; ++j)
      CHECK(rill_text_append(&text, cases[i].suffix, strlen(cases[i].suffix)));
    [[gnu::cleanup(rill_source_clear)]] RillSource source = {};
    CHECK(rill_source_init(&source, "nesting", text.data, text.size) ==
          RILL_OK);
    [[gnu::cleanup(rill_syntax_clear)]] RillSyntax syntax =
        rill_syntax_parse(&source);
    CHECK(syntax.state == RILL_INVALID &&
          syntax.diagnostic.kind == RILL_SYNTAX);
    CHECK(strstr(syntax.diagnostic.message, "nesting limit"));
  }
}

static void wide_record() {
  [[gnu::cleanup(rill_text_clear)]] RillBuffer text = {};
  CHECK(rill_text_append(&text, "{", 1));
  for (size_t i = 0; i < 257; ++i)
    CHECK(rill_text_format(&text, "%skey%zu: %zu", i ? ", " : "", i, i));
  size_t end = text.size;
  CHECK(rill_text_append(&text, "}", 1));
  for (size_t duplicate = 0; duplicate < 2; ++duplicate) {
    [[gnu::cleanup(rill_source_clear)]] RillSource source = {};
    CHECK(rill_source_init(&source, "wide-record", text.data, text.size) ==
          RILL_OK);
    [[gnu::cleanup(rill_syntax_clear)]] RillSyntax syntax =
        rill_syntax_parse(&source);
    CHECK(syntax.state == (duplicate ? RILL_INVALID : RILL_COMPLETE));
    if (!duplicate) {
      size_t i = 0;
      for (RillNode *field = syntax.first->children; field;
           field = field->next, ++i)
        CHECK(field->children->integer == (int64_t)i);
      CHECK(i == 257);
      rill_text_truncate(&text, end);
      const char suffix[] = ", key128: 0}";
      CHECK(rill_text_append(&text, suffix, sizeof(suffix) - 1));
    } else
      CHECK(strstr(syntax.diagnostic.message, "duplicate record key"));
  }
}

int main() {
  wide_record();
  nesting();
  // Compact tokens and incomplete entries deliberately retain their layout.
  const struct {
    const char *source;
    RillParseState state;
  } cases[] = {
      {"", RILL_COMPLETE},
      {"# comment", RILL_COMPLETE},
      {"rec { loop n => loop n }", RILL_COMPLETE},
      {"rec { loop n =>", RILL_INCOMPLETE},
      {"rec { loop(n) => n }", RILL_INVALID},
      {"export {name, alias: module.name}", RILL_COMPLETE},
      {"export {}", RILL_COMPLETE},
      {"export {name}; export {other}", RILL_INVALID},
      {"export {name, name}", RILL_INVALID},
      {"export {name: 1}", RILL_INVALID},
      {"export {name: f x}", RILL_INVALID},
      {"export let name = 1", RILL_INVALID},
      {"do { export {name} }", RILL_INVALID},
      {"^printf '%s' 'a b' | ^cat > out", RILL_COMPLETE},
      {"f 1 2", RILL_COMPLETE},
      {"[1, 'two', ()][0]", RILL_COMPLETE},
      {"9223372036854775807", RILL_COMPLETE},
      {"-9223372036854775808", RILL_COMPLETE},
      {"^cat |", RILL_INCOMPLETE},
      {"job { ^cat", RILL_INCOMPLETE},
      {"^echo ok; ^cat |", RILL_INCOMPLETE},
      {"f \"abc", RILL_INCOMPLETE},
      {"\"\\u{", RILL_INCOMPLETE},
      {"[1,", RILL_INCOMPLETE},
      {"^echo \"a\"b", RILL_INVALID},
      {"^cat |> f", RILL_INVALID},
      {"\"\\q", RILL_INVALID},
      {"\"\\u{110000}", RILL_INVALID},
      {"\"\\u{d800}", RILL_INVALID},
      {"\"\\u{}\"", RILL_INVALID},
      {"9223372036854775808", RILL_INVALID},
      {"-9223372036854775809", RILL_INVALID},
      {"[1,,2]", RILL_INVALID},
      {"fn f x y =x+y", RILL_COMPLETE},
      {"fn f x", RILL_INCOMPLETE},
      {"fn f# header\n x = x", RILL_COMPLETE},
      {"{x =>", RILL_INCOMPLETE},
      {"if true then 1 else", RILL_INCOMPLETE},
      {"do { let x = 1;", RILL_INCOMPLETE},
      {"match x of { [head, ..tail] =>", RILL_INCOMPLETE},
      {"enum E {A, B {value,},}", RILL_COMPLETE},
      {"struct Empty {}", RILL_COMPLETE},
      {"enum Empty {}", RILL_INVALID},
      {"rec {}", RILL_INVALID},
      {"rec { let x = 1 }", RILL_INVALID},
      {"fn if x = x", RILL_INVALID},
      {"let [head, ..else] = [1]", RILL_INVALID},
      {"let {with} = {with: 1}", RILL_INVALID},
      {"struct _ {}", RILL_INVALID},
      {"import 'x' as fn", RILL_INVALID},
      {"{with: 1}.with", RILL_COMPLETE},
      {"do { struct Local {x} }", RILL_INVALID},
      {"{a: 1, a: 2}", RILL_INVALID},
      {"{\"a\\u{0}x\": 1, \"a\\u{0}y\": 2}", RILL_COMPLETE},
      {"{\"a\\u{0}x\": 1, \"a\\u{0}x\": 2}", RILL_INVALID},
      {"{\"\": 1, \"\": 2}", RILL_INVALID},
      {"let {a: x, a: y, ..} = r", RILL_INVALID},
      {"1<2<3", RILL_INVALID},
      {"(1<2)==true", RILL_COMPLETE},
      {"1\n(2)", RILL_COMPLETE},
      {"1\n|> f", RILL_COMPLETE},
      {"let p = job { ^echo ...$([]) }", RILL_COMPLETE},
      {"{ x => x }", RILL_COMPLETE},
      {"{ x y => x }", RILL_COMPLETE},
      {"{ () => }", RILL_COMPLETE},
      {"{ _ => let x = 1; }", RILL_COMPLETE},
      {"{name}", RILL_COMPLETE},
      {"{name, size: 2,}", RILL_COMPLETE},
      {"{ [x, ..tail] {name, ..} => x }", RILL_COMPLETE},
      {"{ (Point {x, y}) => x }", RILL_COMPLETE},
      {"match Point {x: 1, y: 2} of { Point {x, y} => x }", RILL_COMPLETE},
      {"(f\n x)", RILL_COMPLETE},
      {"^echo $(text\n 42)", RILL_COMPLETE},
      {"[f\n x, {y => y\n y}]", RILL_COMPLETE},
      {"{x # header\n => x}", RILL_COMPLETE},
      {"{\"key:=>\": 1}", RILL_COMPLETE},
      {"{name # => is a comment\n}", RILL_COMPLETE},
      {"{x", RILL_INCOMPLETE},
      {"{x y", RILL_INCOMPLETE},
      {"{name:", RILL_INCOMPLETE},
      {"match x of", RILL_INCOMPLETE},
      {"fn f x =", RILL_INCOMPLETE},
      {"f(x)", RILL_INVALID},
      {"f()", RILL_INVALID},
      {"fn(x)=>x", RILL_INVALID},
      {"fn f(x)=x", RILL_INVALID},
      {"fn f x => x", RILL_INVALID},
      {"{=> 1}", RILL_INVALID},
      {"{ x, y => x }", RILL_INVALID},
      {"{x[y] => x}", RILL_INVALID},
      {"{name, name: 1}", RILL_INVALID},
      {"{of}", RILL_INVALID},
      {"f not true", RILL_INVALID},
      {"f if true then 1 else 2", RILL_INVALID},
      {"match x { _ => 1 }", RILL_INCOMPLETE},
      {"let [x, x] = effect ()", RILL_INVALID},
      {"{ [x, {key: x}] => x }", RILL_INVALID},
      {"if false then { [x, x] => x } else 7", RILL_INVALID},
      {"match x of { [a, a] => 1 }", RILL_INVALID},
      {"^...$items", RILL_INVALID},
      {"^echo > ...$paths", RILL_INVALID}};
  for (size_t i = 0; i < sizeof(cases) / sizeof(*cases); ++i) {
    RillSource source = {};
    CHECK(rill_source_init(&source, "syntax", cases[i].source,
                           strlen(cases[i].source)) == RILL_OK);
    RillSyntax syntax = rill_syntax_parse(&source);
    if (syntax.state != cases[i].state)
      (void)fprintf(stderr, "syntax case %zu: %s\n", i, cases[i].source);
    CHECK(syntax.state == cases[i].state);
    CHECK(syntax.diagnostic.kind ==
          (cases[i].state == RILL_COMPLETE ? RILL_OK : RILL_SYNTAX));
    CHECK(syntax.diagnostic.offset <= source.bytes.size);
    rill_syntax_clear(&syntax);
    rill_source_clear(&source);
  }
  RillSource source = {};
  const char *text = "\"a\\u{0}\\u{1f642}\"";
  CHECK(rill_source_init(&source, "literal", text, strlen(text)) == RILL_OK);
  RillSyntax syntax = rill_syntax_parse(&source);
  rill_source_clear(&source);
  CHECK(!strcmp(syntax.name.data, "literal"));
  CHECK(syntax.source.size == strlen(text) &&
        !memcmp(syntax.source.data, text, strlen(text)));
  CHECK(syntax.state == RILL_COMPLETE && syntax.first->kind == RILL_STRING);
  CHECK(syntax.first->text.size == 6);
  CHECK(!memcmp(syntax.first->text.data, "a\0\U0001f642", 6));
  rill_syntax_clear(&syntax);
}
