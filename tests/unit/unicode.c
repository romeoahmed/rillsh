#include "test.h"
#include "text/text.h"
#include <errno.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int main(int argc, char **argv) {
  CHECK(argc == 2);
  FILE *f = fopen(argv[1], "r");
  CHECK(f);
  char *line = nullptr;
  size_t cap = 0, cases = 0;
  while (getline(&line, &cap, f) >= 0) {
    char *comment = strchr(line, '#');
    if (comment)
      *comment = 0;
    char *save = nullptr, *token = strtok_r(line, " \t\r\n", &save);
    if (!token)
      continue;
    RillBuffer b = {};
    size_t boundaries[1024], n = 0;
    do {
      if (!strcmp(token, "\u00f7")) {
        CHECK(n < 1024);
        boundaries[n++] = b.size;
      } else if (strcmp(token, "\u00d7") != 0) {
        errno = 0;
        char *end = {};
        unsigned long cp = strtoul(token, &end, 16);
        CHECK(!errno && !*end && cp <= 0x10ffff);
        CHECK(rill_text_encode(&b, (uint32_t)cp));
      }
    } while ((token = strtok_r(nullptr, " \t\r\n", &save)));
    CHECK(n >= 2 && boundaries[0] == 0);
    size_t at = 0;
    for (size_t i = 1; i < n; ++i) {
      at = rill_text_next(b.data, b.size, at);
      if (at != boundaries[i])
        (void)fprintf(stderr, "case %zu boundary %zu got %zu expected %zu\n",
                      cases, i, at, boundaries[i]);
      CHECK(at == boundaries[i]);
    }
    CHECK(at == b.size);
    rill_text_clear(&b);
    ++cases;
  }
  // The upstream corpus is independent of generated property tables.
  CHECK(cases == 853);
  CHECK(!ferror(f));
  free(line);
  CHECK(fclose(f) == 0);
}
