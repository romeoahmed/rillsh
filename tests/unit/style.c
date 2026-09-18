/**
 * @file
 * @brief Palette selection and safe style emission.
 *
 * Cases cover destination capability, color overrides, and deterministic
 * escape sequences without requiring a live terminal.
 */
#include "text/style.h"
#include "../support/check.h"
#include "text/text.h"
#include <stddef.h>
#include <string.h>

int main() {
  const struct {
    RillColorMode mode;
    bool tty;
    const char *term, *colorterm, *no_color;
    RillPalette palette;
  } cases[] = {
      {RILL_COLOR_AUTO, true, "xterm-256color", nullptr, nullptr,
       RILL_PALETTE_256},
      {RILL_COLOR_AUTO, true, "xterm", "24bit", "", RILL_PALETTE_RGB},
      {RILL_COLOR_AUTO, true, "xterm-direct", nullptr, nullptr,
       RILL_PALETTE_RGB},
      {RILL_COLOR_AUTO, true, "xterm", nullptr, nullptr, RILL_PALETTE_16},
      {RILL_COLOR_AUTO, false, "xterm", "truecolor", nullptr,
       RILL_PALETTE_PLAIN},
      {RILL_COLOR_AUTO, true, "dumb", "truecolor", nullptr, RILL_PALETTE_PLAIN},
      {RILL_COLOR_AUTO, true, nullptr, nullptr, nullptr, RILL_PALETTE_PLAIN},
      {RILL_COLOR_AUTO, true, "unknown", "truecolor", nullptr,
       RILL_PALETTE_PLAIN},
      {RILL_COLOR_AUTO, true, "xterm", "truecolor", "1", RILL_PALETTE_PLAIN},
      {RILL_COLOR_ALWAYS, false, "dumb", "truecolor", "1", RILL_PALETTE_RGB},
      {RILL_COLOR_ALWAYS, false, nullptr, nullptr, "1", RILL_PALETTE_16},
      {RILL_COLOR_NEVER, true, "xterm", "truecolor", nullptr,
       RILL_PALETTE_PLAIN}};
  for (size_t i = 0; i < sizeof(cases) / sizeof(*cases); ++i)
    CHECK(rill_style_palette(cases[i].mode, cases[i].tty, cases[i].term,
                             cases[i].colorterm,
                             cases[i].no_color) == cases[i].palette);
  const RillPalette palettes[] = {RILL_PALETTE_PLAIN, RILL_PALETTE_16,
                                  RILL_PALETTE_256, RILL_PALETTE_RGB};
  for (size_t i = 0; i < sizeof(palettes) / sizeof(*palettes); ++i) {
    [[gnu::cleanup(rill_text_clear)]] RillBuffer b = {}, error = {};
    CHECK(rill_style_append(&b, palettes[i], "content", false));
    CHECK(rill_style_append(&error, palettes[i], "content", true));
    CHECK(strstr(b.data, "content"));
    CHECK(strstr(error.data, "content"));
    if (palettes[i] == RILL_PALETTE_PLAIN) {
      CHECK(!strcmp(b.data, "content"));
      CHECK(!strcmp(error.data, "content"));
    } else {
      CHECK(b.size >= 4 && error.size >= 4);
      CHECK(b.data[0] == '\033');
      CHECK(!strcmp(b.data + b.size - 4, "\033[0m"));
      CHECK(error.data[0] == '\033');
      CHECK(!strcmp(error.data + error.size - 4, "\033[0m"));
      CHECK(strcmp(b.data, error.data));
    }
  }
}
