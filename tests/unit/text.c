/**
 * @file
 * @brief Buffer guarantees and Unicode boundary fixtures.
 *
 * Known UTF-8 encodings and malformed byte sequences provide independent
 * checks; grapheme and width cases exercise the implemented text policy.
 */
#include "text/text.h"
#include "../support/check.h"
#include "diagnostic.h"
#include "source.h"
#include <errno.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>

static void utf8() {
  const char *invalid[] = {"\x80",
                           "\xc0\xaf",
                           "\xc1\xbf",
                           "\xe0\x80\xaf",
                           "\xed\xa0\x80",
                           "\xf0\x80\x80\xaf",
                           "\xf4\x90\x80\x80",
                           "\xf5\x80\x80\x80",
                           "\xff",
                           "\xc2",
                           "\xe2\x82",
                           "\xf0\x9f\x99",
                           "\xe2 A"};
  for (size_t i = 0; i < sizeof(invalid) / sizeof(*invalid); ++i) {
    size_t at = 0;
    uint32_t scalar = 123;
    CHECK(!rill_text_decode(invalid[i], strlen(invalid[i]), &at, &scalar));
    CHECK(at == 0 && scalar == 123);
    CHECK(!rill_text_valid(invalid[i], strlen(invalid[i])));
    [[gnu::cleanup(rill_text_clear)]] RillBuffer prefixed = {};
    CHECK(rill_text_append(&prefixed, "a", 1));
    CHECK(rill_text_append(&prefixed, invalid[i], strlen(invalid[i])));
    at = 1;
    CHECK(!rill_text_decode(prefixed.data, prefixed.size, &at, &scalar));
    CHECK(at == 1 && scalar == 123);
  }
  CHECK(rill_text_valid(nullptr, 0));
  // Known encodings are an independent oracle at each UTF-8 length boundary.
  const struct {
    uint32_t scalar;
    const char *bytes;
    size_t size;
  } cases[] = {{0, "\0", 1},
               {0x7f, "\x7f", 1},
               {0x80, "\xc2\x80", 2},
               {0x7ff, "\xdf\xbf", 2},
               {0x800, "\xe0\xa0\x80", 3},
               {0xd7ff, "\xed\x9f\xbf", 3},
               {0xe000, "\xee\x80\x80", 3},
               {0xffff, "\xef\xbf\xbf", 3},
               {0x10000, "\xf0\x90\x80\x80", 4},
               {0x10ffff, "\xf4\x8f\xbf\xbf", 4}};
  for (size_t i = 0; i < sizeof(cases) / sizeof(*cases); ++i) {
    RillBuffer b = {};
    CHECK(rill_text_encode(&b, cases[i].scalar));
    CHECK(b.size == cases[i].size && !memcmp(b.data, cases[i].bytes, b.size));
    size_t at = 0;
    uint32_t scalar = 0;
    CHECK(rill_text_decode(b.data, b.size, &at, &scalar));
    CHECK(scalar == cases[i].scalar && at == b.size);
    CHECK(!rill_text_decode(b.data, b.size, &at, &scalar));
    CHECK(at == b.size);
    rill_text_clear(&b);
  }
  RillBuffer b = {};
  CHECK(!rill_text_encode(&b, 0xd800));
  CHECK(!rill_text_encode(&b, 0xdfff));
  CHECK(!rill_text_encode(&b, 0x110000));
  CHECK(!b.data && !b.size);
  CHECK(rill_text_append(&b, "kept", 4));
  CHECK(!rill_text_encode(&b, 0x110000));
  CHECK(b.size == 4 && !strcmp(b.data, "kept"));
  rill_text_clear(&b);
}

static void buffers() {
  RillBuffer b = {};
  rill_text_truncate(&b, 0);
  CHECK(!b.data && !b.size);
  CHECK(rill_text_append(&b, nullptr, 0));
  CHECK(b.size == 0 && b.data[0] == 0);
  CHECK(rill_text_append(&b, "a\0b", 3));
  CHECK(b.size == 3 && !memcmp(b.data, "a\0b\0", 4));
  CHECK(rill_text_format(&b, "%s:%0100d", "prefix", 7));
  CHECK(b.size == 110 && b.data[109] == '7' && b.data[110] == 0);
  CHECK(rill_text_escape(&b, (RillBytes){"\0\t\n\033\x7f\xff", 6}));
  CHECK(!strcmp(b.data + 110, "\\x00\\x09\\x0a\\x1b\\x7f\\xff"));
  char *storage = b.data;
  size_t size = b.size;
  CHECK(!rill_text_append(&b, nullptr, SIZE_MAX));
  CHECK(errno == ENOMEM && b.data == storage && b.size == size);
  rill_text_truncate(&b, 2);
  CHECK(b.data == storage && b.size == 2 && !memcmp(b.data, "a\0\0", 3));
  rill_text_truncate(&b, 0);
  CHECK(b.data == storage && b.size == 0 && !*b.data);
  CHECK(rill_text_append(&b, "reused", 6));
  CHECK(b.size == 6 && !strcmp(b.data, "reused"));
  rill_text_clear(&b);
  CHECK(!b.data && !b.size && !b.capacity);
  rill_text_clear(&b);
}

static void sources() {
  char name[] = "entry", data[] = "\u754c";
  RillSource source = {};
  CHECK(rill_source_init(&source, name, data, sizeof(data) - 1) == RILL_OK);
  name[0] = data[0] = 'X';
  CHECK(!strcmp(source.name, "entry") && !strcmp(source.bytes.data, "\u754c"));
  rill_source_clear(&source);
  const RillBytes invalid[] = {{"\xff", 1}, {"a\0b", 3}};
  for (size_t i = 0; i < sizeof(invalid) / sizeof(*invalid); ++i) {
    CHECK(rill_source_init(&source, "invalid", invalid[i].data,
                           invalid[i].size) == RILL_SYNTAX);
    CHECK(!source.name && !source.bytes.data);
    rill_source_clear(&source);
  }
}

int main() {
  utf8();
  buffers();
  sources();
  const struct {
    const char *text;
    unsigned cells;
  } widths[] = {{"a", 1},
                {"\u754c", 2},
                {"e\u0301", 1},
                {"\U0001f469\u200d\U0001f4bb", 2},
                {"\u2764\ufe0e", 1},
                {"\u2764\ufe0f", 2}};
  for (size_t i = 0; i < sizeof(widths) / sizeof(*widths); ++i)
    CHECK(rill_text_width(widths[i].text, strlen(widths[i].text)) ==
          widths[i].cells);
}
