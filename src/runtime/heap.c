#include "runtime.h"
#include <assert.h>
#include <stdckdint.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
bool rill_runtime_is_object(RillValue v) { return v.kind >= RILL_V_STRING; }
void rill_runtime_root(RillHeap *h, RillRoot *r, RillValue *v, size_t n) {
  *r = (RillRoot){h->roots, v, n};
  h->roots = r;
}
void rill_runtime_unroot(RillHeap *h, RillRoot *r) {
  assert(h->roots == r);
  h->roots = r->previous;
}
static void mark(RillValue v, RillObject **gray) {
  if (!rill_runtime_is_object(v) || !v.as.object || v.as.object->marked)
    return;
  RillObject *o = v.as.object;
  o->marked = true;
  o->gray = *gray;
  *gray = o;
}
void rill_runtime_collect(RillHeap *h) {
  RillObject *gray = nullptr;
  for (RillRoot *r = h->roots; r; r = r->previous)
    for (size_t i = 0; i < r->count; ++i)
      mark(r->values[i], &gray);
  // Intrusive marking needs neither recursive calls nor scratch allocation.
  while (gray) {
    RillObject *o = gray;
    gray = o->gray;
    for (size_t i = 0; i < o->count; ++i)
      mark(o->values[i], &gray);
  }
  RillObject **link = &h->objects;
  while (*link) {
    RillObject *o = *link;
    if (o->marked) {
      o->marked = false;
      link = &o->next;
    } else {
      *link = o->next;
      h->bytes -= o->allocation;
      free(o);
    }
  }
  h->threshold = h->bytes > SIZE_MAX / 2 ? SIZE_MAX : h->bytes * 2;
  if (h->threshold < 65536)
    h->threshold = 65536;
}
RillValue rill_runtime_object(RillHeap *h, RillValueKind kind,
                              const RillValue *v, size_t n, const char *s,
                              size_t len, int64_t tag) {
  if (h->stress || h->bytes >= h->threshold)
    rill_runtime_collect(h);
  size_t edges, bytes, total;
  if (ckd_mul(&edges, n, sizeof(*v)) ||
      ckd_add(&bytes, edges, sizeof(RillObject)) ||
      ckd_add(&bytes, bytes, len) || ckd_add(&bytes, bytes, 1) ||
      ckd_add(&total, bytes, h->bytes))
    return (RillValue){};
  RillObject *o = malloc(bytes);
  if (!o)
    return (RillValue){};
  // Trailing bytes share the allocation without imposing extra alignment.
  char *data = (char *)o + sizeof(*o) + edges;
  if (len)
    memcpy(data, s, len);
  data[len] = '\0';
  *o = (RillObject){.next = h->objects,
                    .allocation = bytes,
                    .bytes = {data, len},
                    .tag = tag,
                    .count = n};
  if (n)
    memcpy(o->values, v, edges);
  h->objects = o;
  h->bytes = total;
  return (RillValue){.kind = kind, .as.object = o};
}
void rill_runtime_heap_clear(RillHeap *h) {
  h->roots = nullptr;
  rill_runtime_collect(h);
  *h = (RillHeap){};
}
