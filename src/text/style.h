/**
 * @file
 * @brief Semantic RGB styles with deterministic terminal palette selection.
 */
#pragma once
#include "text.h"
/** @brief User color override, independent of cursor-addressed editing. */
typedef enum {
  RILL_COLOR_AUTO,
  RILL_COLOR_ALWAYS,
  RILL_COLOR_NEVER
} RillColorMode;
/** @brief Destination capability selected from explicit hints. */
typedef enum {
  RILL_PALETTE_PLAIN,
  RILL_PALETTE_16,
  RILL_PALETTE_256,
  RILL_PALETTE_RGB
} RillPalette;
/**
 * @brief Select a palette from destination capability and user overrides.
 *
 * Hints are borrowed nullable C strings. No terminal queries or I/O occur.
 */
RillPalette rill_style_palette(RillColorMode mode, bool tty, const char *term,
                               const char *colorterm, const char *no_color);
/**
 * @brief Append trusted NUL-terminated text with the selected palette.
 *
 * Text must not alias out storage. Escape untrusted display bytes first.
 * Failure may leave a partial append; discard it rather than emit a broken
 * style sequence.
 */
[[nodiscard]] bool rill_style_append(RillBuffer *out, RillPalette palette,
                                     const char *text, bool error);
