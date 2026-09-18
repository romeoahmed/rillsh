/**
 * @file
 * @brief Select a terminal palette and append trusted styled text.
 *
 * Capability hints and explicit overrides choose RGB, indexed, basic, or plain
 * output. Callers escape untrusted bytes before applying styles.
 */
#include "style.h"
#include "text.h"
#include <stddef.h>
#include <string.h>
RillPalette rill_style_palette(RillColorMode mode, bool tty, const char *term,
                               const char *ct, const char *nc) {
  if (mode == RILL_COLOR_NEVER)
    return RILL_PALETTE_PLAIN;
  static const char *const families[] = {"xterm", "screen", "tmux",     "rxvt",
                                         "foot",  "kitty",  "alacritty"};
  bool known = false;
  if (term)
    for (size_t i = 0; i < sizeof(families) / sizeof(families[0]); ++i)
      if (!strncmp(term, families[i], strlen(families[i])))
        known = true;
  if (mode == RILL_COLOR_AUTO && (!tty || !known || (nc && *nc)))
    return RILL_PALETTE_PLAIN;
  if ((ct && (!strcmp(ct, "truecolor") || !strcmp(ct, "24bit"))) ||
      (term && strstr(term, "-direct")))
    return RILL_PALETTE_RGB;
  if (term && strstr(term, "-256color"))
    return RILL_PALETTE_256;
  return RILL_PALETTE_16;
}
bool rill_style_append(RillBuffer *out, RillPalette p, const char *text,
                       bool error) {
  const char *prefix = "";
  switch (p) {
  case RILL_PALETTE_PLAIN:
    break;
  case RILL_PALETTE_16:
    prefix = error ? "\033[31m" : "\033[36m";
    break;
  case RILL_PALETTE_256:
    prefix = error ? "\033[38;5;203m" : "\033[38;5;80m";
    break;
  case RILL_PALETTE_RGB:
    prefix = error ? "\033[38;2;255;95;95m" : "\033[38;2;80;210;200m";
    break;
  }
  return rill_text_append(out, prefix, strlen(prefix)) &&
         rill_text_append(out, text, strlen(text)) &&
         (p == RILL_PALETTE_PLAIN || rill_text_append(out, "\033[0m", 4));
}
