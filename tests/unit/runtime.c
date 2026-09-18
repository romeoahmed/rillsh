/**
 * @file
 * @brief Heap, graph, and evaluator-lifecycle contracts.
 *
 * Exercise independent roots, partial builders, shared/cyclic graphs, Record
 * indexes, and transactional entries under collection. Meson selects scenarios
 * separately; assertions cover ownership rather than fixed allocation counts.
 */
#include "runtime/runtime.h"
#include "../support/check.h"
#include "diagnostic.h"
#include "source.h"
#include "syntax/syntax.h"
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

static void independent_roots() {
  RillHeap heap = {.stress = true};
  RillValue values[3] = {};
  RillRoot roots[3] = {};
  for (size_t i = 0; i < 3; ++i) {
    rill_runtime_root(&heap, &roots[i], &values[i], 1);
    values[i] =
        rill_runtime_object(&heap, RILL_V_STRING, nullptr, 0, "retained", 8, 0);
    CHECK(values[i].kind == RILL_V_STRING);
  }
  // Suspended contexts can unregister in any order, then reuse a root frame.
  rill_runtime_unroot(&heap, &roots[1]);
  values[1] = (RillValue){};
  rill_runtime_collect(&heap);
  CHECK(!strcmp(values[0].as.object->bytes.data, "retained"));
  CHECK(!strcmp(values[2].as.object->bytes.data, "retained"));
  rill_runtime_root(&heap, &roots[1], &values[1], 1);
  values[1] =
      rill_runtime_object(&heap, RILL_V_LIST, values + 2, 1, nullptr, 0, 0);
  CHECK(values[1].kind == RILL_V_LIST);
  rill_runtime_unroot(&heap, &roots[0]);
  rill_runtime_unroot(&heap, &roots[2]);
  values[0] = values[2] = (RillValue){};
  rill_runtime_collect(&heap);
  RillValue kept = rill_runtime_at(values[1], 0);
  CHECK(kept.kind == RILL_V_STRING &&
        !strcmp(kept.as.object->bytes.data, "retained"));
  rill_runtime_unroot(&heap, &roots[1]);
  rill_runtime_collect(&heap);
  CHECK(heap.bytes == 0);
  rill_runtime_heap_clear(&heap);
}

static void heap_contracts() {
  RillHeap heap = {.stress = true};
  RillValue roots[2] = {};
  RillRoot frame = {};
  rill_runtime_root(&heap, &frame, roots, 2);
  roots[0] = rill_runtime_object(&heap, RILL_V_STRING, nullptr, 0, "persistent",
                                 10, 0);
  CHECK(roots[0].kind == RILL_V_STRING);
  RillObject *address = roots[0].as.object;
  CHECK(
      rill_runtime_object(&heap, RILL_V_LIST, nullptr, SIZE_MAX, nullptr, 0, 0)
          .kind == RILL_V_UNIT);
  CHECK(rill_runtime_object(&heap, RILL_V_STRING, nullptr, 0, nullptr, SIZE_MAX,
                            0)
            .kind == RILL_V_UNIT);
  for (size_t i = 0; i < 1000; ++i) {
    roots[1] = rill_runtime_object(&heap, RILL_V_LIST, roots, 1, nullptr, 0, 0);
    CHECK(roots[1].kind == RILL_V_LIST);
    CHECK(roots[1].as.object->values[0].as.object == address);
    CHECK(!strcmp(address->bytes.data, "persistent"));
  }
  rill_runtime_collect(&heap);
  size_t plateau = heap.bytes;
  for (size_t i = 0; i < 1000; ++i)
    roots[1] = rill_runtime_object(&heap, RILL_V_LIST, roots, 1, nullptr, 0, 0);
  rill_runtime_collect(&heap);
  CHECK(heap.bytes == plateau);
  // A deep graph must be marked iteratively, including shared edges and cycles.
  heap.stress = false;
  for (size_t i = 0; i < 20000; ++i) {
    roots[1] = rill_runtime_object(&heap, RILL_V_LIST, roots, 2, nullptr, 0, 0);
    CHECK(roots[1].kind == RILL_V_LIST);
  }
  rill_runtime_collect(&heap);
  CHECK(roots[0].as.object == address);
  RillValue cursor = roots[1];
  for (size_t i = 0; i < 20000; ++i) {
    CHECK(cursor.kind == RILL_V_LIST && cursor.as.object->count == 2);
    CHECK(cursor.as.object->values[0].as.object == address);
    cursor = cursor.as.object->values[1];
  }
  CHECK(cursor.as.object->count == 1);
  roots[1].as.object->values[1] = roots[1];
  size_t cyclic =
      roots[0].as.object->allocation + roots[1].as.object->allocation;
  rill_runtime_collect(&heap);
  CHECK(heap.bytes == cyclic);
  rill_runtime_unroot(&heap, &frame);
  rill_runtime_collect(&heap);
  CHECK(heap.bytes == 0);
  rill_runtime_heap_clear(&heap);
}
static void builders_and_slices() {
  RillHeap heap = {.stress = true};
  RillValue values[3] = {};
  RillRoot root = {};
  rill_runtime_root(&heap, &root, values, 3);
  values[0] =
      rill_runtime_object(&heap, RILL_V_LIST, nullptr, 4096, nullptr, 0, 0);
  CHECK(values[0].kind == RILL_V_LIST);
  for (size_t i = 0; i < 4096; ++i)
    CHECK(values[0].as.object->values[i].kind == RILL_V_UNIT);
  // Partially built arrays must be safe at every subsequent safepoint.
  values[0].as.object->values[0] =
      rill_runtime_object(&heap, RILL_V_STRING, nullptr, 0, "kept", 4, 0);
  values[0].as.object->values[2] =
      (RillValue){.kind = RILL_V_INT, .as.integer = 2};
  values[1] = rill_runtime_slice(&heap, values[0], 0, 4096);
  CHECK(values[1].as.object == values[0].as.object);
  values[2] = rill_runtime_slice(&heap, values[0], 4096, 0);
  CHECK(values[2].kind == RILL_V_LIST && rill_runtime_count(values[2]) == 0);
  rill_runtime_collect(&heap);
  CHECK(!strcmp(values[1].as.object->values[0].as.object->bytes.data, "kept"));
  values[1] = rill_runtime_slice(&heap, values[0], 1, 4095);
  values[1] = rill_runtime_slice(&heap, values[1], 1, 4094);
  values[0] = (RillValue){};
  rill_runtime_collect(&heap);
  CHECK(rill_runtime_count(values[1]) == 4094);
  CHECK(rill_runtime_at(values[1], 0).as.integer == 2);
  values[0] = values[1] = (RillValue){};
  rill_runtime_collect(&heap);
  CHECK(heap.bytes == values[2].as.object->allocation);
  rill_runtime_unroot(&heap, &root);
  rill_runtime_heap_clear(&heap);
}
static void scoped_graphs() {
  RillHeap heap = {};
  RillValue values[5] = {};
  RillRoot root = {};
  rill_runtime_root(&heap, &root, values, 5);
  values[0] =
      rill_runtime_object(&heap, RILL_V_STREAM, nullptr, 0, nullptr, 0, 0);
  CHECK(values[0].kind == RILL_V_STREAM);
  for (size_t i = 0; i < 20000; ++i) {
    RillValue edges[] = {values[1], values[1]};
    values[1] =
        rill_runtime_object(&heap, RILL_V_LIST, edges, 2, nullptr, 0, 0);
    CHECK(values[1].kind == RILL_V_LIST);
  }
  values[2] =
      rill_runtime_object(&heap, RILL_V_STAGE, nullptr, 0, nullptr, 0, 0);
  values[3] = rill_runtime_budget(&heap, SIZE_MAX);
  CHECK(values[2].kind == RILL_V_STAGE && values[3].kind == RILL_V_BUDGET);
  CHECK(rill_runtime_charge(&heap, values[3], values[0]) == RILL_OK);
  // Repeated walks must restore temporary state after success and early
  // failure, without changing collection epochs or retaining their visited
  // graphs.
  for (size_t round = 0; round < 4; ++round) {
    CHECK(rill_runtime_persistent(&heap, values[1]) == RILL_OK);
    values[2].as.object->metadata = values[0];
    RillValue edges[] = {values[2], values[1], values[3]};
    values[4] =
        rill_runtime_object(&heap, RILL_V_LIST, edges, 3, nullptr, 0, 0);
    CHECK(values[4].kind == RILL_V_LIST);
    CHECK(rill_runtime_persistent(&heap, values[4]) == RILL_STREAM_ESCAPE);
    values[2].as.object->metadata = values[1];
    values[4].as.object->values[2] = values[4];
    CHECK(rill_runtime_persistent(&heap, values[4]) == RILL_OK);
    CHECK(rill_runtime_persistent(&heap, values[3]) == RILL_STREAM_ESCAPE);
    rill_runtime_collect(&heap);
  }
  rill_runtime_unroot(&heap, &root);
  rill_runtime_collect(&heap);
  CHECK(!heap.bytes && !heap.scoped);
  rill_runtime_heap_clear(&heap);
}
static void deep_equality() {
  RillHeap heap = {};
  RillValue values[2] = {};
  RillRoot root = {};
  rill_runtime_root(&heap, &root, values, 2);
  for (size_t i = 0; i < 65536; ++i) {
    for (size_t j = 0; j < 2; ++j) {
      values[j] =
          rill_runtime_object(&heap, RILL_V_LIST, values + j, 1, nullptr, 0, 0);
      CHECK(values[j].kind == RILL_V_LIST);
    }
    if (i == 127) {
      bool equal = false;
      CHECK(rill_runtime_equal(values[0], values[1], &equal) == RILL_OK &&
            equal);
    }
  }
  bool equal = {};
  CHECK(rill_runtime_equal(values[0], values[1], &equal) == RILL_LIMIT);
  // Identity must not bypass validation of unsupported leaves.
  values[0].as.object->values[0] = (RillValue){.kind = RILL_V_FUNCTION};
  CHECK(rill_runtime_equal(values[0], values[0], &equal) == RILL_TYPE);
  rill_runtime_unroot(&heap, &root);
  rill_runtime_heap_clear(&heap);
}

static void shared_equality() {
  RillHeap heap = {.stress = true};
  RillValue values[4] = {};
  RillRoot root = {};
  rill_runtime_root(&heap, &root, values, 4);
  for (size_t i = 0; i < 80; ++i)
    for (size_t j = 0; j < 2; ++j) {
      RillValue pair[] = {values[j], values[j]};
      values[j] =
          rill_runtime_object(&heap, RILL_V_LIST, pair, 2, nullptr, 0, 0);
      CHECK(values[j].kind == RILL_V_LIST);
    }
  bool equal = false;
  CHECK(rill_runtime_equal(values[0], values[1], &equal) == RILL_OK && equal);
  RillValue bottom = values[1];
  for (size_t i = 1; i < 80; ++i)
    bottom = bottom.as.object->values[0];
  bottom.as.object->values[1] =
      (RillValue){.kind = RILL_V_INT, .as.integer = 1};
  CHECK(rill_runtime_equal(values[0], values[1], &equal) == RILL_OK && !equal);
  bottom.as.object->values[1] = (RillValue){};
  // A later unsupported leaf is not hidden by sharing or an earlier mismatch.
  RillValue bad[] = {values[1], {.kind = RILL_V_FUNCTION}};
  values[2] = rill_runtime_object(&heap, RILL_V_LIST, bad, 2, nullptr, 0, 0);
  CHECK(rill_runtime_equal(values[0], values[2], &equal) == RILL_TYPE);
  // Revalidate on each operation: unpublished builders may have changed.
  values[2].as.object->values[1] = (RillValue){};
  CHECK(rill_runtime_equal(values[2], values[2], &equal) == RILL_OK && equal);
  values[2].as.object->values[1] = values[2];
  CHECK(rill_runtime_equal(values[2], values[2], &equal) == RILL_LIMIT);
  values[2] = (RillValue){};
  heap.stress = false;
  // Visit the shared subtree first, then reach it through a longer path.
  values[3] = values[0];
  for (size_t i = 0; i < 65536 - 80; ++i)
    values[3] =
        rill_runtime_object(&heap, RILL_V_LIST, values + 3, 1, nullptr, 0, 0);
  RillValue pair[] = {values[0], values[3]};
  values[2] = rill_runtime_object(&heap, RILL_V_LIST, pair, 2, nullptr, 0, 0);
  CHECK(rill_runtime_equal(values[2], values[2], &equal) == RILL_LIMIT);
  rill_runtime_unroot(&heap, &root);
  rill_runtime_heap_clear(&heap);
}

static void different_sharing() {
  enum { WIDTH = 12, LEVELS = 40 };
  RillHeap heap = {};
  RillValue rows[4 * WIDTH] = {}, leaf = {};
  RillRoot root = {}, leaf_root = {};
  rill_runtime_root(&heap, &root, rows, sizeof(rows) / sizeof(*rows));
  rill_runtime_root(&heap, &leaf_root, &leaf, 1);
  for (size_t level = 0; level < LEVELS; ++level) {
    size_t old = (level % 2) * 2 * WIDTH;
    size_t next = ((level + 1) % 2) * 2 * WIDTH;
    for (size_t i = 0; i < WIDTH; ++i) {
      RillValue a[] = {rows[old + i], rows[old + (i + 1) % WIDTH],
                       rows[old + i]};
      RillValue b[] = {rows[old + WIDTH + i], rows[old + WIDTH + i],
                       rows[old + WIDTH + (i + 1) % WIDTH]};
      rows[next + i] =
          rill_runtime_object(&heap, RILL_V_LIST, a, 3, nullptr, 0, 0);
      rows[next + WIDTH + i] =
          rill_runtime_object(&heap, RILL_V_LIST, b, 3, nullptr, 0, 0);
      CHECK(rows[next + i].kind == RILL_V_LIST);
      CHECK(rows[next + WIDTH + i].kind == RILL_V_LIST);
      if (!level && !i)
        leaf = rows[next + WIDTH];
    }
  }
  for (size_t i = 0; i < WIDTH; ++i) {
    bool equal = false;
    CHECK(rill_runtime_equal(rows[0], rows[WIDTH + i], &equal) == RILL_OK);
    CHECK(equal);
  }
  // Every root reaches this leaf through a different set of paths. A union
  // cannot discharge untested child obligations or hide an unsupported leaf.
  leaf.as.object->values[1] = (RillValue){.kind = RILL_V_INT, .as.integer = 1};
  bool equal = true;
  CHECK(rill_runtime_equal(rows[0], rows[WIDTH], &equal) == RILL_OK && !equal);
  leaf.as.object->values[1] = (RillValue){.kind = RILL_V_FUNCTION};
  CHECK(rill_runtime_equal(rows[0], rows[WIDTH], &equal) == RILL_TYPE);
  leaf.as.object->values[1] = (RillValue){};
  CHECK(rill_runtime_equal(rows[0], rows[WIDTH], &equal) == RILL_OK && equal);
  rill_runtime_unroot(&heap, &leaf_root);
  rill_runtime_unroot(&heap, &root);
  rill_runtime_heap_clear(&heap);
}

static void equality_boundaries() {
  RillHeap heap = {.stress = true};
  RillValue values[2] = {};
  RillRoot root = {};
  rill_runtime_root(&heap, &root, values, 2);
  for (size_t count = 1; count <= 80; ++count) {
    for (size_t side = 0; side < 2; ++side) {
      values[side] = rill_runtime_object(&heap, RILL_V_LIST, nullptr, count,
                                         nullptr, 0, 0);
      CHECK(values[side].kind == RILL_V_LIST);
    }
    bool equal = false;
    CHECK(rill_runtime_equal(values[0], values[1], &equal) == RILL_OK && equal);
    values[0].as.object->values[0] =
        (RillValue){.kind = RILL_V_INT, .as.integer = 1};
    CHECK(rill_runtime_equal(values[0], values[1], &equal) == RILL_OK &&
          !equal);
    values[1].as.object->values[count - 1] = (RillValue){.kind = RILL_V_JOB};
    CHECK(rill_runtime_equal(values[0], values[1], &equal) == RILL_TYPE);
    CHECK(rill_runtime_equal(values[1], values[1], &equal) == RILL_TYPE);
  }
  rill_runtime_unroot(&heap, &root);
  rill_runtime_heap_clear(&heap);
}

static void copied_records() {
  RillHeap heap = {.stress = true};
  RillValue values[2] = {};
  RillRoot root = {};
  rill_runtime_root(&heap, &root, values, 2);
  const size_t sizes[] = {0, 1, 7, 16, 33, 128};
  for (size_t k = 0; k < sizeof(sizes) / sizeof(*sizes); ++k) {
    size_t count = sizes[k];
    values[0] = rill_runtime_record(&heap, nullptr, count * 2);
    CHECK(values[0].kind == RILL_V_RECORD);
    for (size_t i = 0; i < count; ++i) {
      char name[32];
      int n = snprintf(name, sizeof(name), "key%zu", count - i);
      CHECK(n > 0 && (size_t)n < sizeof(name));
      name[0] = '\0';
      values[0].as.object->values[2 * i] = rill_runtime_object(
          &heap, RILL_V_STRING, nullptr, 0, name, (size_t)n, 0);
      CHECK(values[0].as.object->values[2 * i].kind == RILL_V_STRING);
      values[0].as.object->values[2 * i + 1] =
          (RillValue){.kind = RILL_V_INT, .as.integer = (int64_t)i};
    }
    CHECK(rill_runtime_record_finish(values[0]));
    values[1] = rill_runtime_record_copy(&heap, values[0]);
    CHECK(values[1].kind == RILL_V_RECORD);
    bool equal = false;
    CHECK(rill_runtime_equal(values[0], values[1], &equal) == RILL_OK && equal);
    if (count) {
      values[1].as.object->values[1].as.integer = -1;
      CHECK(values[0].as.object->values[1].as.integer == 0);
    }
    // The copied index must not borrow storage from its discarded source.
    values[0] = (RillValue){};
    rill_runtime_collect(&heap);
    for (size_t i = 0; i < count; ++i) {
      RillValue item = {};
      RillBytes key = values[1].as.object->values[2 * i].as.object->bytes;
      CHECK(rill_runtime_field(values[1], key, &item));
      CHECK(item.kind == RILL_V_INT &&
            item.as.integer == (i ? (int64_t)i : -1));
    }
  }
  rill_runtime_unroot(&heap, &root);
  rill_runtime_heap_clear(&heap);
}
static void indexed_records() {
  RillHeap heap = {.stress = true};
  RillValue records[2] = {};
  RillRoot root = {};
  rill_runtime_root(&heap, &root, records, 2);
  for (size_t j = 0; j < 2; ++j) {
    records[j] =
        rill_runtime_object(&heap, RILL_V_RECORD, nullptr, 256, nullptr, 0, 0);
    CHECK(records[j].kind == RILL_V_RECORD);
    for (size_t i = 0; i < 128; ++i) {
      size_t index = j ? 127 - i : i;
      char name[16];
      int size = snprintf(name, sizeof(name), "key%zu", index);
      CHECK(size > 0 && (size_t)size < sizeof(name));
      // Distinguish length-aware keys even when they share a NUL prefix.
      name[0] = '\0';
      records[j].as.object->values[2 * i] = rill_runtime_object(
          &heap, RILL_V_STRING, nullptr, 0, name, (size_t)size, 0);
      CHECK(records[j].as.object->values[2 * i].kind == RILL_V_STRING);
      records[j].as.object->values[2 * i + 1] =
          (RillValue){.kind = RILL_V_INT, .as.integer = (int64_t)index};
    }
    CHECK(rill_runtime_record_finish(records[j]));
  }
  bool equal = {};
  CHECK(rill_runtime_equal(records[0], records[1], &equal) == RILL_OK && equal);
  RillObject *record = records[0].as.object;
  for (size_t i = 0; i < 128; ++i) {
    RillValue item = {};
    CHECK(rill_runtime_field(records[0], record->values[2 * i].as.object->bytes,
                             &item));
    CHECK(item.as.integer == (int64_t)i);
    CHECK(record->values[2 * i + 1].as.integer == (int64_t)i);
  }
  RillValue item = {};
  CHECK(!rill_runtime_field(records[0], (RillBytes){"", 0}, &item));
  CHECK(!rill_runtime_field(records[0], (RillBytes){"\0ey128", 6}, &item));
  records[1].as.object->values[254] =
      rill_runtime_object(&heap, RILL_V_STRING, nullptr, 0, "other", 5, 0);
  CHECK(records[1].as.object->values[254].kind == RILL_V_STRING);
  CHECK(rill_runtime_record_finish(records[1]));
  CHECK(rill_runtime_equal(records[0], records[1], &equal) == RILL_OK &&
        !equal);
  records[1].as.object->values[255] = (RillValue){.kind = RILL_V_FUNCTION};
  CHECK(rill_runtime_equal(records[0], records[1], &equal) == RILL_TYPE);
  records[1].as.object->values[254] = records[0].as.object->values[0];
  records[1].as.object->values[255] = (RillValue){.kind = RILL_V_INT};
  CHECK(rill_runtime_record_finish(records[1]));
  CHECK(rill_runtime_equal(records[0], records[1], &equal) == RILL_OK && equal);
  records[1].as.object->values[254] = records[1].as.object->values[0];
  CHECK(!rill_runtime_record_finish(records[1]));
  // A shrinking private builder must discard its old index and GC edges.
  record->count = 64;
  CHECK(rill_runtime_record_finish(records[0]));
  rill_runtime_collect(&heap);
  CHECK(rill_runtime_field(records[0], (RillBytes){"\0ey31", 5}, &item));
  CHECK(item.as.integer == 31);
  CHECK(!rill_runtime_field(records[0], (RillBytes){"\0ey32", 5}, &item));
  record->count = 16;
  CHECK(rill_runtime_record_finish(records[0]));
  rill_runtime_collect(&heap);
  CHECK(rill_runtime_field(records[0], (RillBytes){"\0ey7", 4}, &item));
  CHECK(item.as.integer == 7);
  CHECK(!rill_runtime_field(records[0], (RillBytes){"\0ey8", 4}, &item));
  record->values[2] = record->values[0];
  CHECK(!rill_runtime_record_finish(records[0]));
  rill_runtime_unroot(&heap, &root);
  rill_runtime_heap_clear(&heap);
}

static void retained_budget() {
  RillHeap heap = {.stress = true};
  RillValue values[4] = {};
  RillRoot root = {};
  rill_runtime_root(&heap, &root, values, 4);
  values[0] =
      rill_runtime_object(&heap, RILL_V_STRING, nullptr, 0, "shared", 6, 0);
  CHECK(values[0].kind == RILL_V_STRING);
  RillObject *shared = values[0].as.object;
  values[1] = rill_runtime_budget(&heap, shared->allocation);
  CHECK(values[1].kind == RILL_V_BUDGET);
  CHECK(rill_runtime_charge(&heap, values[1], values[0]) == RILL_OK);
  CHECK(rill_runtime_charge(&heap, values[1], values[0]) == RILL_OK);
  values[2] = rill_runtime_budget(&heap, shared->allocation - 1);
  CHECK(values[2].kind == RILL_V_BUDGET);
  CHECK(rill_runtime_charge(&heap, values[2], values[0]) == RILL_LIMIT);
  values[0] = (RillValue){};
  rill_runtime_collect(&heap);
  CHECK(!strcmp(shared->bytes.data, "shared"));
  values[3] = rill_runtime_budget(&heap, SIZE_MAX);
  CHECK(values[3].kind == RILL_V_BUDGET);
  for (size_t i = 0; i < 200; ++i) {
    values[0] =
        rill_runtime_object(&heap, RILL_V_LIST, values + 1, 1, nullptr, 0, 0);
    CHECK(values[0].kind == RILL_V_LIST);
    CHECK(rill_runtime_charge(&heap, values[3], values[0]) == RILL_OK);
  }
  rill_runtime_collect(&heap);
  CHECK(!strcmp(shared->bytes.data, "shared"));
  rill_runtime_unroot(&heap, &root);
  rill_runtime_collect(&heap);
  CHECK(heap.bytes == 0);
  rill_runtime_heap_clear(&heap);
}

static void stage_policy() {
  RillHeap heap = {.stress = true};
  RillValue values[3] = {};
  RillRoot root = {};
  rill_runtime_root(&heap, &root, values, 3);
  values[0] =
      rill_runtime_object(&heap, RILL_V_STRING, nullptr, 0, "policy", 6, 0);
  CHECK(values[0].kind == RILL_V_STRING);
  RillObject *text = values[0].as.object;
  values[1] =
      rill_runtime_object(&heap, RILL_V_STAGE, nullptr, 0, nullptr, 0, 0);
  CHECK(values[1].kind == RILL_V_STAGE);
  CHECK(values[1].as.object->metadata.kind == RILL_V_UNIT);
  values[1].as.object->metadata = values[0];
  size_t retained = text->allocation + values[1].as.object->allocation;
  values[2] = rill_runtime_budget(&heap, retained);
  CHECK(values[2].kind == RILL_V_BUDGET);
  CHECK(rill_runtime_charge(&heap, values[2], values[1]) == RILL_OK);
  values[0] = values[1] = (RillValue){};
  rill_runtime_collect(&heap);
  CHECK(heap.bytes == retained + values[2].as.object->allocation);
  CHECK(!strcmp(text->bytes.data, "policy"));
  rill_runtime_unroot(&heap, &root);
  rill_runtime_collect(&heap);
  CHECK(!heap.bytes);
  rill_runtime_heap_clear(&heap);
}

static RillEvalEvent next_event(RillEval *eval) {
  RillEvalEvent event = {};
  do {
    event = rill_runtime_step(eval);
  } while (event.state == RILL_EVAL_YIELD);
  return event;
}

static RillEvalEvent entry(RillEval *eval, const char *text) {
  RillSource source = {};
  CHECK(rill_source_init(&source, "entry", text, strlen(text)) == RILL_OK);
  RillSyntax syntax = rill_syntax_parse(&source);
  CHECK(syntax.state == RILL_COMPLETE);
  rill_runtime_begin(eval, &syntax);
  RillEvalEvent event = next_event(eval);
  CHECK(event.state == RILL_EVAL_DONE || event.state == RILL_EVAL_ERROR);
  if (event.state == RILL_EVAL_ERROR)
    rill_runtime_abort(eval);
  rill_syntax_clear(&syntax);
  rill_source_clear(&source);
  return event;
}

static void aborted_diagnostic() {
  const RillNative native = {"effect", 42};
  RillEval *eval = rill_runtime_new(&native, 1);
  CHECK(eval);
  RillHeap *heap = rill_runtime_heap(eval);
  heap->stress = true;
  CHECK(entry(eval, "let stable = 7").state == RILL_EVAL_DONE);
  [[gnu::cleanup(rill_source_clear)]] RillSource source = {};
  const char text[] = "let pending = 9; effect 0";
  CHECK(rill_source_init(&source, "abort", text, sizeof(text) - 1) == RILL_OK);
  [[gnu::cleanup(rill_syntax_clear)]] RillSyntax syntax =
      rill_syntax_parse(&source);
  CHECK(syntax.state == RILL_COMPLETE);
  rill_runtime_begin(eval, &syntax);
  CHECK(next_event(eval).state == RILL_EVAL_NATIVE);
  RillValue detail =
      rill_runtime_object(heap, RILL_V_STRING, nullptr, 0, "temporary", 9, 0);
  CHECK(detail.kind == RILL_V_STRING);
  rill_runtime_resume(
      eval, detail,
      (RillDiagnostic){.kind = RILL_TYPE,
                       .message = detail.as.object->bytes.data});
  CHECK(next_event(eval).diagnostic.kind == RILL_TYPE);
  // Abort discards both the error and the root backing its borrowed message.
  rill_runtime_abort(eval);
  rill_runtime_abort(eval);
  CHECK(rill_runtime_define(eval, "fresh",
                            (RillValue){.kind = RILL_V_INT, .as.integer = 11}));
  RillValue value = {};
  CHECK(!rill_runtime_lookup(eval, "pending", &value));
  CHECK(rill_runtime_lookup(eval, "stable", &value) &&
        value.kind == RILL_V_INT && value.as.integer == 7);
  CHECK(next_event(eval).state == RILL_EVAL_DONE);
  CHECK(rill_runtime_lookup(eval, "fresh", &value) &&
        value.kind == RILL_V_INT && value.as.integer == 11);
  rill_runtime_free(eval);
}

static void entries() {
  RillEval *eval = rill_runtime_new(nullptr, 0);
  CHECK(eval);
  rill_runtime_heap(eval)->stress = true;
  CHECK(entry(eval, "let saved = 'original'").state == RILL_EVAL_DONE);
  CHECK(entry(eval, "let saved = 'discarded'; missing").state ==
        RILL_EVAL_ERROR);
  RillEvalEvent result = entry(eval, "saved");
  CHECK(result.value.kind == RILL_V_STRING);
  CHECK(!strcmp(result.value.as.object->bytes.data, "original"));
  const char *invalid[] = {"[1][-1]", "[1][1]",  "[1]['0']",
                           "1 2",     "1.field", "let x = 1; let x = 2"};
  for (size_t i = 0; i < sizeof(invalid) / sizeof(*invalid); ++i) {
    result = entry(eval, invalid[i]);
    CHECK(result.state == RILL_EVAL_ERROR &&
          result.diagnostic.kind == RILL_TYPE);
  }
  result = entry(eval, "[1, [2, 'kept']][1][1]");
  CHECK(result.value.kind == RILL_V_STRING);
  CHECK(!strcmp(result.value.as.object->bytes.data, "kept"));
  for (size_t i = 0; i < 1000; ++i)
    CHECK(entry(eval, "let saved = 'replacement'").state == RILL_EVAL_DONE);
  rill_runtime_collect(rill_runtime_heap(eval));
  size_t retained = rill_runtime_heap(eval)->bytes;
  CHECK(entry(eval, "let saved = 'replacement'").state == RILL_EVAL_DONE);
  rill_runtime_collect(rill_runtime_heap(eval));
  CHECK(rill_runtime_heap(eval)->bytes == retained);
  rill_runtime_free(eval);
}

static void snapshot_merges() {
  RillEval *eval = rill_runtime_new(nullptr, 0);
  CHECK(eval);
  rill_runtime_heap(eval)->stress = true;
  for (size_t round = 0; round < 3; ++round) {
    for (size_t i = 0; i < 32; ++i) {
      char name[32];
      int size = snprintf(name, sizeof(name), "key%zu", 31 - i);
      CHECK(size > 0 && (size_t)size < sizeof(name));
      CHECK(rill_runtime_define(
          eval, name,
          (RillValue){.kind = RILL_V_INT, .as.integer = (int64_t)(i + round)}));
    }
    CHECK(entry(eval, "let middle = 5; let z = 9; let a = 1").state ==
          RILL_EVAL_DONE);
    CHECK(entry(eval, "let key17 = 99; missing").state == RILL_EVAL_ERROR);
    for (size_t i = 0; i < 32; ++i) {
      char name[32];
      int size = snprintf(name, sizeof(name), "key%zu", 31 - i);
      CHECK(size > 0 && (size_t)size < sizeof(name));
      RillValue value = {};
      CHECK(rill_runtime_lookup(eval, name, &value));
      CHECK(value.kind == RILL_V_INT &&
            value.as.integer == (int64_t)(i + round));
    }
    CHECK(entry(eval, "let middle = 6; middle + z + a").value.as.integer == 16);
  }
  rill_runtime_free(eval);
}
static void resumed_publication() {
  const RillNative native = {"pause", 42};
  RillEval *eval = rill_runtime_new(&native, 1);
  CHECK(eval);
  rill_runtime_heap(eval)->stress = true;
  CHECK(entry(eval, "let shared = 1; let old = 2").state == RILL_EVAL_DONE);
  [[gnu::cleanup(rill_source_clear)]] RillSource source = {};
  const char text[] = "let shared = 3; let retained = { () => old }; pause ()";
  CHECK(rill_source_init(&source, "suspended", text, sizeof(text) - 1) ==
        RILL_OK);
  [[gnu::cleanup(rill_syntax_clear)]] RillSyntax syntax =
      rill_syntax_parse(&source);
  CHECK(syntax.state == RILL_COMPLETE);
  rill_runtime_begin(eval, &syntax);
  CHECK(next_event(eval).state == RILL_EVAL_NATIVE);
  RillEvaluation *saved = rill_runtime_suspend(eval);
  CHECK(saved);
  CHECK(entry(eval, "let shared = 4; let old = 5; let fresh = 6").state ==
        RILL_EVAL_DONE);
  // Native definitions may extend the current snapshot while work is stopped.
  CHECK(rill_runtime_define(eval, "native",
                            (RillValue){.kind = RILL_V_INT, .as.integer = 7}));
  rill_runtime_abort(eval);
  rill_runtime_restore(eval, saved);
  rill_runtime_collect(rill_runtime_heap(eval));
  rill_runtime_resume(eval, (RillValue){}, (RillDiagnostic){});
  CHECK(next_event(eval).state == RILL_EVAL_DONE);
  RillEvalEvent event =
      entry(eval, "if shared == 3 and old == 5 and fresh == 6 and native == 7 "
                  "and retained () == 2 then 1 else 0");
  CHECK(event.state == RILL_EVAL_DONE && event.value.kind == RILL_V_INT &&
        event.value.as.integer == 1);
  rill_runtime_free(eval);
}

static void unary_protocol() {
  const RillNative native = {"f", 42};
  RillEval *eval = rill_runtime_new(&native, 1);
  CHECK(eval);
  rill_runtime_heap(eval)->stress = true;
  RillSource source = {};
  const char *text = "let g = [f][0]; g 1 2";
  CHECK(rill_source_init(&source, "unary", text, strlen(text)) == RILL_OK);
  RillSyntax syntax = rill_syntax_parse(&source);
  CHECK(syntax.state == RILL_COMPLETE);
  rill_runtime_begin(eval, &syntax);
  for (int64_t argument = 1; argument <= 2; ++argument) {
    RillEvalEvent event = next_event(eval);
    CHECK(event.state == RILL_EVAL_NATIVE && event.native == 42);
    CHECK(event.value.kind == RILL_V_INT && event.value.as.integer == argument);
    CHECK(rill_runtime_step(eval).state == RILL_EVAL_YIELD);
    rill_runtime_collect(rill_runtime_heap(eval));
    RillValue value =
        argument == 1 ? (RillValue){.kind = RILL_V_FUNCTION, .as.integer = 42}
                      : event.value;
    rill_runtime_resume(eval, value, (RillDiagnostic){});
  }
  RillEvalEvent result = next_event(eval);
  CHECK(result.state == RILL_EVAL_DONE && result.value.as.integer == 2);
  rill_syntax_clear(&syntax);
  rill_source_clear(&source);
  text = "f (f 'payload')";
  CHECK(rill_source_init(&source, "rooted", text, strlen(text)) == RILL_OK);
  syntax = rill_syntax_parse(&source);
  CHECK(syntax.state == RILL_COMPLETE);
  rill_runtime_begin(eval, &syntax);
  for (size_t i = 0; i < 2; ++i) {
    result = next_event(eval);
    CHECK(result.state == RILL_EVAL_NATIVE &&
          result.value.kind == RILL_V_STRING);
    rill_runtime_collect(rill_runtime_heap(eval));
    CHECK(!strcmp(result.value.as.object->bytes.data, "payload"));
    rill_runtime_resume(eval, result.value, (RillDiagnostic){});
  }
  CHECK(next_event(eval).state == RILL_EVAL_DONE);
  rill_syntax_clear(&syntax);
  rill_source_clear(&source);
  // Aborting a suspended effect must discard pending bindings and frames.
  text = "let g = 9; f 1";
  CHECK(rill_source_init(&source, "abort", text, strlen(text)) == RILL_OK);
  syntax = rill_syntax_parse(&source);
  CHECK(syntax.state == RILL_COMPLETE);
  rill_runtime_begin(eval, &syntax);
  CHECK(next_event(eval).state == RILL_EVAL_NATIVE);
  rill_runtime_abort(eval);
  CHECK(entry(eval, "g").value.kind == RILL_V_FUNCTION);
  syntax = rill_syntax_parse(&source);
  CHECK(syntax.state == RILL_COMPLETE);
  rill_runtime_begin(eval, &syntax);
  CHECK(next_event(eval).state == RILL_EVAL_NATIVE);
  rill_runtime_resume(eval, (RillValue){}, (RillDiagnostic){.kind = RILL_IO});
  CHECK(next_event(eval).diagnostic.kind == RILL_IO);
  rill_runtime_abort(eval);
  CHECK(entry(eval, "g").value.kind == RILL_V_FUNCTION);
  rill_syntax_clear(&syntax);
  rill_source_clear(&source);
  rill_runtime_free(eval);
}

int main(int argc, char **argv) {
  CHECK(argc == 2);
  if (!strcmp(argv[1], "heap")) {
    independent_roots();
    copied_records();
    heap_contracts();
    builders_and_slices();
    retained_budget();
    stage_policy();
    scoped_graphs();
  } else if (!strcmp(argv[1], "equality")) {
    deep_equality();
    shared_equality();
    equality_boundaries();
    different_sharing();
    indexed_records();
  } else {
    CHECK(!strcmp(argv[1], "entries"));
    aborted_diagnostic();
    entries();
    snapshot_merges();
    resumed_publication();
    unary_protocol();
  }
}
