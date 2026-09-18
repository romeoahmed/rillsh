#include "syntax/syntax.h"
#include "diagnostic.h"
#include "source.h"
#include "test.h"
#include <stddef.h>
#include <stdio.h>
#include <string.h>

int main() {
  const struct {
    const char *source;
    RillParseState state;
  } cases[] = {{"", RILL_COMPLETE},
               {"# comment", RILL_COMPLETE},
               {"^printf '%s' 'a b' | ^cat > out", RILL_COMPLETE},
               {"f(1, 2)", RILL_COMPLETE},
               {"[1, 'two', ()][0]", RILL_COMPLETE},
               {"9223372036854775807", RILL_COMPLETE},
               {"-9223372036854775808", RILL_COMPLETE},
               {"^cat |", RILL_INCOMPLETE},
               {"job { ^cat", RILL_INCOMPLETE},
               {"^echo ok; ^cat |", RILL_INCOMPLETE},
               {"f(\"abc", RILL_INCOMPLETE},
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
               {"[1,,2]", RILL_INVALID}};
  for (size_t i = 0; i < sizeof(cases) / sizeof(*cases); ++i) {
    RillSource source;
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
  RillSource source;
  const char *text = "\"a\\u{0}\\u{1f642}\"";
  CHECK(rill_source_init(&source, "literal", text, strlen(text)) == RILL_OK);
  RillSyntax syntax = rill_syntax_parse(&source);
  CHECK(syntax.state == RILL_COMPLETE && syntax.first->kind == RILL_STRING);
  CHECK(syntax.first->text.size == 6);
  CHECK(!memcmp(syntax.first->text.data, "a\0🙂", 6));
  rill_syntax_clear(&syntax);
  rill_source_clear(&source);
}
