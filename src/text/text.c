#include "text.h"
#include <errno.h>
#include <stdarg.h>
#include <stdckdint.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static bool reserve(RillBuffer *b, size_t n) {
  size_t needed;
  if (ckd_add(&needed, b->size, n) || ckd_add(&needed, needed, 1)) {
    errno = ENOMEM;
    return false;
  }
  if (needed > b->capacity) {
    size_t cap = b->capacity ? b->capacity : 64;
    while (cap < needed) {
      if (ckd_mul(&cap, cap, 2)) {
        cap = needed;
        break;
      }
    }
    char *p = realloc(b->data, cap);
    if (!p)
      return false;
    b->data = p;
    b->capacity = cap;
  }
  return true;
}
bool rill_text_append(RillBuffer *b, const void *data, size_t n) {
  if (!reserve(b, n))
    return false;
  if (n)
    memcpy(b->data + b->size, data, n);
  b->size += n;
  b->data[b->size] = '\0';
  return true;
}
bool rill_text_format(RillBuffer *b, const char *format, ...) {
  va_list args, copy;
  va_start(args);
  va_copy(copy, args);
  int n = vsnprintf(nullptr, 0, format, copy);
  va_end(copy);
  bool ok = n >= 0 && reserve(b, (size_t)n);
  if (ok) {
    int wrote = vsnprintf(b->data + b->size, (size_t)n + 1, format, args);
    ok = wrote == n;
    if (ok)
      b->size += (size_t)n;
    b->data[b->size] = '\0';
  }
  va_end(args);
  return ok;
}
void rill_text_clear(RillBuffer *b) {
  free(b->data);
  *b = (RillBuffer){};
}
bool rill_text_escape(RillBuffer *b, RillBytes bytes) {
  static constexpr char hex[] = "0123456789abcdef";
  // Reserve the complete expansion before mutating the destination.
  size_t size = bytes.size;
  for (size_t i = 0; i < bytes.size; ++i) {
    unsigned char c = (unsigned char)bytes.data[i];
    if ((c < 32 || c >= 127) && ckd_add(&size, size, 3)) {
      errno = ENOMEM;
      return false;
    }
  }
  if (!reserve(b, size))
    return false;
  char *out = b->data + b->size;
  for (size_t i = 0; i < bytes.size; ++i) {
    unsigned char c = (unsigned char)bytes.data[i];
    if (c < 32 || c >= 127) {
      *out++ = '\\';
      *out++ = 'x';
      *out++ = hex[c >> 4];
      *out++ = hex[c & 15];
    } else
      *out++ = (char)c;
  }
  *out = '\0';
  b->size += size;
  return true;
}
bool rill_text_decode(const char *s, size_t n, size_t *at, uint32_t *cp) {
  if (*at >= n)
    return false;
  size_t p = *at;
  uint32_t c = (unsigned char)s[p++];
  unsigned rest = 0;
  uint32_t min = 0;
  if (c >= 0xc2 && c <= 0xdf) {
    rest = 1;
    min = 0x80;
    c &= 31;
  } else if (c >= 0xe0 && c <= 0xef) {
    rest = 2;
    min = 0x800;
    c &= 15;
  } else if (c >= 0xf0 && c <= 0xf4) {
    rest = 3;
    min = 0x10000;
    c &= 7;
  } else if (c >= 0x80)
    return false;
  if (n - p < rest)
    return false;
  for (unsigned i = 0; i < rest; ++i) {
    unsigned char d = (unsigned char)s[p++];
    if ((d & 0xc0) != 0x80)
      return false;
    c = (c << 6) | (d & 63U);
  }
  if (c < min || c > 0x10ffff || (c >= 0xd800 && c <= 0xdfff))
    return false;
  *at = p;
  *cp = c;
  return true;
}
bool rill_text_valid(const char *s, size_t n) {
  size_t at = 0;
  uint32_t c;
  while (at < n)
    if (!rill_text_decode(s, n, &at, &c))
      return false;
  return true;
}
bool rill_text_encode(RillBuffer *b, uint32_t c) {
  unsigned char s[4];
  size_t n;
  if (c <= 0x7f) {
    s[0] = (unsigned char)c;
    n = 1;
  } else if (c <= 0x7ff) {
    s[0] = (unsigned char)(0xc0 | (c >> 6));
    s[1] = (unsigned char)(0x80 | (c & 63));
    n = 2;
  } else if (c >= 0xd800 && c <= 0xdfff)
    return false;
  else if (c <= 0xffff) {
    s[0] = (unsigned char)(0xe0 | (c >> 12));
    s[1] = (unsigned char)(0x80 | ((c >> 6) & 63));
    s[2] = (unsigned char)(0x80 | (c & 63));
    n = 3;
  } else if (c <= 0x10ffff) {
    s[0] = (unsigned char)(0xf0 | (c >> 18));
    s[1] = (unsigned char)(0x80 | ((c >> 12) & 63));
    s[2] = (unsigned char)(0x80 | ((c >> 6) & 63));
    s[3] = (unsigned char)(0x80 | (c & 63));
    n = 4;
  } else
    return false;
  return rill_text_append(b, s, n);
}

typedef struct {
  uint32_t first, last;
  unsigned property;
} Range;
#include "unicode_tables.inc"
static unsigned property(uint32_t cp, const Range *table, size_t size) {
  size_t lo = 0, hi = size;
  while (lo < hi) {
    size_t mid = lo + (hi - lo) / 2;
    if (cp < table[mid].first)
      hi = mid;
    else if (cp > table[mid].last)
      lo = mid + 1;
    else
      return table[mid].property;
  }
  return 0;
}
#define PROP(c, table)                                                         \
  property((c), (table), sizeof(table) / sizeof((table)[0]))
size_t rill_text_next(const char *s, size_t n, size_t at) {
  uint32_t c;
  if (!rill_text_decode(s, n, &at, &c))
    return n;
  unsigned prev = PROP(c, gcb), ri = prev == G_RI ? 1U : 0U;
  bool ep = PROP(c, pictographic) != 0, zwj_ep = false;
  bool linker = PROP(c, incb) == I_Linker;
  while (at < n) {
    size_t next = at;
    if (!rill_text_decode(s, n, &next, &c))
      return n;
    unsigned curr = PROP(c, gcb), in = PROP(c, incb);
    bool join = false;
    if (prev == G_CR && curr == G_LF)
      join = true;
    else if (prev == G_CR || prev == G_LF || prev == G_Control ||
             curr == G_CR || curr == G_LF || curr == G_Control)
      join = false;
    else if (prev == G_L &&
             (curr == G_L || curr == G_V || curr == G_LV || curr == G_LVT))
      join = true;
    else if ((prev == G_LV || prev == G_V) && (curr == G_V || curr == G_T))
      join = true;
    else if ((prev == G_LVT || prev == G_T) && curr == G_T)
      join = true;
    else if (curr == G_Extend || curr == G_ZWJ || curr == G_SpacingMark ||
             prev == G_Prepend)
      join = true;
    else if (linker && in == I_Consonant)
      join = true;
    else if (zwj_ep && PROP(c, pictographic))
      join = true;
    else if (prev == G_RI && curr == G_RI && ri % 2 == 1)
      join = true;
    if (!join)
      break;
    zwj_ep = curr == G_ZWJ && ep;
    ep = PROP(c, pictographic) != 0 || (curr == G_Extend && ep);
    // Unicode 18 GB9c: Linker Extend* × Consonant (no leading consonant).
    if (in == I_Linker)
      linker = true;
    else if (in != I_Extend)
      linker = false;
    ri = curr == G_RI ? ri + 1 : 0;
    prev = curr;
    at = next;
  }
  return at;
}
unsigned rill_text_width(const char *s, size_t n) {
  size_t at = 0;
  uint32_t c;
  unsigned width = 0;
  bool emoji = false, text = false;
  while (at < n && rill_text_decode(s, n, &at, &c)) {
    if (c == 0xfe0e)
      text = true;
    if (c == 0xfe0f)
      emoji = true;
    if (PROP(c, emoji_presentation))
      emoji = true;
    if (!PROP(c, invisible)) {
      unsigned w = PROP(c, wide) ? 2U : 1U;
      if (w > width)
        width = w;
    }
  }
  return emoji && !text ? 2 : (width ? width : 1);
}
