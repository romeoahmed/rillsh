/**
 * @file
 * @brief Schedule scoped, single-consumer sources, transforms, and sinks.
 *
 * Tokens identify nodes; transfer claims the old token without cloning work.
 * Consumers propagate demand iteratively, retaining at most one item per edge.
 * Callbacks use evaluator events, and byte transport remains in the supervisor.
 *
 * @verbatim
 * source --item--> transform --item--> sink
 *        <--demand--         <--demand--
 * scope exit / error --> close dependencies --> reap --> resume
 * @endverbatim
 *
 * The scope owns nodes, rooted Values, directories, and attached jobs. Sink
 * frames support nested consumers; checkpoints close only affected resources.
 */
#include "stream.h"
#include "diagnostic.h"
#include "exec/exec.h"
#include "library.h"
#include "platform/posix.h"
#include "runtime/runtime.h"
#include "text/text.h"
#include "value.h"
#include <assert.h>
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <poll.h>
#include <stdckdint.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

enum SourceKind { CHUNKS, FILES, MAP, FILTER, TAKE, LINES, PROCESS };
enum SinkKind { CREATE, COLLECT, COLLECT_BYTES, FOLD, EACH, WRITE, CLOSE };
enum { DATA, FUNCTION, ITEM, NODE_ROOTS };
typedef struct Stream {
  struct Stream *next, *previous, *input;
  enum SourceKind kind;
  int64_t id;
  RillValue values[NODE_ROOTS];
  RillRoot root;
  RillJob *job;
  DIR *directory;
  RillBuffer buffer;
  size_t offset, remaining, limit;
  bool ready, done, demand, claimed, closing, callback;
  RillDiagnostic error;
} Stream;
enum { OUTPUT, CALLBACK, ACCUMULATOR, BUDGET, SINK_ROOTS };
typedef struct Sink {
  struct Sink *parent;
  enum SinkKind kind;
  Stream *source, *callback_source;
  RillValue values[SINK_ROOTS];
  RillRoot root;
  RillBuffer buffer;
  size_t used, capacity, max_items, max_bytes;
  bool awaiting, closing;
  RillValue raised;
  RillRoot raised_root;
  RillDiagnostic error;
} Sink;
struct RillStreams {
  Stream *nodes;
  Sink *sink;
  int64_t checkpoint;
  bool unwinding;
  RillValue result;
  RillRoot root;
};
static RillHeap *heap(RillLibrary *l) { return rill_runtime_heap(l->eval); }
static void error(RillDiagnostic *d, RillError kind, const char *message) {
  if (!d->kind)
    *d = (RillDiagnostic){.kind = kind, .message = message};
}
static bool finish(RillLibrary *l, RillValue value, RillDiagnostic d) {
  rill_runtime_resume(l->eval, value, d);
  return true;
}
static bool fail(RillLibrary *l, RillError kind, const char *message) {
  return finish(l, (RillValue){},
                (RillDiagnostic){.kind = kind, .message = message});
}
static bool named(RillValue value, const char *name) {
  return value.kind == RILL_V_STRING &&
         value.as.object->bytes.size == strlen(name) &&
         !memcmp(value.as.object->bytes.data, name, strlen(name));
}
static bool callable(RillValue v) {
  return v.kind == RILL_V_CLOSURE || v.kind == RILL_V_FUNCTION ||
         v.kind == RILL_V_CONSTRUCTOR;
}
static RillValue object(RillLibrary *l, RillValueKind kind,
                        const RillValue *values, size_t count, const char *data,
                        size_t size, RillDiagnostic *d) {
  RillValue v =
      rill_runtime_object(heap(l), kind, values, count, data, size, 0);
  if (v.kind == RILL_V_UNIT)
    error(d, RILL_MEMORY, "stream allocation failed");
  return v;
}
static Stream *find(RillLibrary *l, RillValue value, RillDiagnostic *d) {
  if (value.kind != RILL_V_STREAM) {
    error(d, RILL_TYPE, "expected Stream");
    return nullptr;
  }
  for (Stream *s = l->streams ? l->streams->nodes : nullptr; s; s = s->next)
    if (s->id == value.as.object->values[0].as.integer) {
      if (s->claimed) {
        error(d, RILL_STREAM_CONSUMED,
              "Stream ownership was transferred or consumed");
        return nullptr;
      }
      return s;
    }
  error(d, RILL_STREAM_CONSUMED, "Stream scope has ended");
  return nullptr;
}
static RillValue token(RillLibrary *l, Stream *s, RillDiagnostic *d) {
  RillValue id = {.kind = RILL_V_INT, .as.integer = s->id};
  return object(l, RILL_V_STREAM, &id, 1, nullptr, 0, d);
}
static Stream *node(RillLibrary *l, enum SourceKind kind, RillDiagnostic *d) {
  if (!l->streams) {
    l->streams = malloc(sizeof(*l->streams));
    if (!l->streams) {
      error(d, RILL_MEMORY, "stream scope allocation failed");
      return nullptr;
    }
    *l->streams = (RillStreams){};
  }
  int64_t id = rill_runtime_resource_id(l->eval);
  if (!id) {
    error(d, RILL_LIMIT, "stream identity limit exceeded");
    return nullptr;
  }
  Stream *s = malloc(sizeof(*s));
  if (!s) {
    error(d, RILL_MEMORY, "stream allocation failed");
    return nullptr;
  }
  *s = (Stream){.kind = kind, .id = id, .next = l->streams->nodes};
  if (s->next)
    s->next->previous = s;
  rill_runtime_root(heap(l), &s->root, s->values, NODE_ROOTS);
  l->streams->nodes = s;
  return s;
}
static void dispose_source(Stream *s, bool cutoff) {
  if (s->directory) {
    (void)closedir(s->directory);
    s->directory = nullptr;
  }
  if (s->job) {
    if (cutoff)
      rill_exec_cutoff(s->job);
    else
      rill_exec_cancel(s->job);
  }
  s->closing = true;
  s->ready = false;
  s->values[ITEM] = (RillValue){};
}
static void close_chain(Stream *s, bool cutoff) {
  for (; s; s = s->input)
    dispose_source(s, cutoff);
}
static bool chain_live(Stream *s) {
  for (; s; s = s->input)
    if (s->job && rill_exec_state(s->job) != RILL_JOB_COMPLETED)
      return true;
  return false;
}
static RillDiagnostic chain_error(Stream *s, bool check) {
  RillDiagnostic result = {};
  for (; s; s = s->input) {
    if (s->error.kind && !result.kind)
      result = s->error;
    if (!s->job)
      continue;
    if (!result.kind && rill_exec_state(s->job) == RILL_JOB_STOPPED)
      result = (RillDiagnostic){
          .kind = RILL_PROCESS,
          .message = "stream producer stopped outside interactive evaluation"};
    RillDiagnostic d = rill_exec_error(s->job);
    if (d.kind && !result.kind)
      result = d;
    if (check && !result.kind &&
        rill_exec_state(s->job) == RILL_JOB_COMPLETED &&
        rill_exec_result(s->job))
      result = (RillDiagnostic){
          .kind = rill_exec_cancelled(s->job) ? RILL_CANCELLED : RILL_PROCESS,
          .code = rill_exec_result(s->job),
          .message = "stream producer did not complete successfully"};
  }
  return result;
}
static void release_item(Stream *s) {
  s->ready = false;
  s->values[ITEM] = (RillValue){};
}
static bool emit_bytes(RillLibrary *l, Stream *s, const char *data, size_t size,
                       RillValueKind kind) {
  s->values[ITEM] = object(l, kind, nullptr, 0, data, size, &s->error);
  s->ready = !s->error.kind;
  return s->ready;
}
static void directory_next(RillLibrary *l, Stream *s) {
  errno = 0;
  struct dirent *entry = readdir(s->directory);
  if (!entry) {
    if (errno)
      s->error = (RillDiagnostic){
          .kind = RILL_IO, .code = errno, .message = "cannot read directory"};
    (void)closedir(s->directory);
    s->directory = nullptr;
    s->done = true;
    return;
  }
  if (!strcmp(entry->d_name, ".") || !strcmp(entry->d_name, ".."))
    return;
  struct stat metadata = {};
  int fd = dirfd(s->directory);
  if (fd < 0 ||
      fstatat(fd, entry->d_name, &metadata, AT_SYMLINK_NOFOLLOW) < 0) {
    s->error =
        (RillDiagnostic){.kind = RILL_IO,
                         .code = errno,
                         .message = "cannot read directory entry metadata"};
    return;
  }
  if (metadata.st_size < 0 || (uintmax_t)metadata.st_size > INT64_MAX) {
    error(&s->error, RILL_LIMIT, "file size outside Int range");
    return;
  }
  const char *kind = S_ISREG(metadata.st_mode)   ? "file"
                     : S_ISDIR(metadata.st_mode) ? "directory"
                     : S_ISLNK(metadata.st_mode) ? "symlink"
                                                 : "other";
  RillValue fields[4] = {
      {},
      {},
      {},
      {.kind = RILL_V_INT, .as.integer = (int64_t)metadata.st_size}};
  RillRoot root = {};
  rill_runtime_root(heap(l), &root, fields, 4);
  fields[0] = object(l, RILL_V_PATH, nullptr, 0, entry->d_name,
                     strlen(entry->d_name), &s->error);
  rill_text_truncate(&s->buffer, 0);
  RillBytes base = s->values[DATA].as.object->bytes;
  if (!rill_text_append(&s->buffer, base.data, base.size) ||
      (base.size && base.data[base.size - 1] != '/' &&
       !rill_text_append(&s->buffer, "/", 1)) ||
      !rill_text_append(&s->buffer, entry->d_name, strlen(entry->d_name)))
    error(&s->error, RILL_MEMORY, "path allocation failed");
  fields[1] = object(l, RILL_V_PATH, nullptr, 0, s->buffer.data, s->buffer.size,
                     &s->error);
  fields[2] =
      object(l, RILL_V_STRING, nullptr, 0, kind, strlen(kind), &s->error);
  const char *keys[] = {"name", "path", "kind", "size"};
  if (!s->error.kind) {
    s->values[ITEM] = rill_library_record(heap(l), keys, fields, 4);
    if (s->values[ITEM].kind == RILL_V_UNIT)
      error(&s->error, RILL_MEMORY, "directory entry allocation failed");
    else
      s->ready = true;
  }
  rill_runtime_unroot(heap(l), &root);
}
static void callback(RillLibrary *l, Sink *sink, Stream *source,
                     RillValue function, RillValue argument) {
  sink->awaiting = true;
  sink->callback_source = source;
  if (source)
    source->callback = true;
  rill_runtime_callback(l->eval, function, argument);
}
static void lines_next(RillLibrary *l, Stream *s) {
  Stream *input = s->input;
  if (!input->ready) {
    if (!input->done) {
      input->demand = true;
      return;
    }
    if (s->buffer.size) {
      if (!rill_text_valid(s->buffer.data, s->buffer.size))
        error(&s->error, RILL_DECODE, "line is not UTF-8");
      else
        (void)emit_bytes(l, s, s->buffer.data, s->buffer.size, RILL_V_STRING);
      rill_text_truncate(&s->buffer, 0);
    } else
      s->done = true;
    return;
  }
  RillValue value = input->values[ITEM];
  if (value.kind != RILL_V_BYTES || !value.as.object->bytes.size) {
    error(&s->error, RILL_TYPE, "byte stream requires nonempty Bytes chunks");
    return;
  }
  RillBytes bytes = value.as.object->bytes;
  const char *start = bytes.data + s->offset;
  const char *lf = memchr(start, '\n', bytes.size - s->offset);
  size_t n = lf ? (size_t)(lf - start) : bytes.size - s->offset;
  if (n > s->limit - s->buffer.size) {
    error(&s->error, RILL_LIMIT, "line byte limit exceeded");
    return;
  }
  if (!rill_text_append(&s->buffer, start, n)) {
    error(&s->error, RILL_MEMORY, "line allocation failed");
    return;
  }
  s->offset += n + (lf ? 1 : 0);
  if (s->offset == bytes.size) {
    release_item(input);
    s->offset = 0;
  }
  if (lf) {
    size_t size = s->buffer.size;
    if (size && s->buffer.data[size - 1] == '\r')
      --size;
    if (!rill_text_valid(s->buffer.data, size))
      error(&s->error, RILL_DECODE, "line is not UTF-8");
    else
      (void)emit_bytes(l, s, s->buffer.data, size, RILL_V_STRING);
    rill_text_truncate(&s->buffer, 0);
  }
}
static void process_next(RillLibrary *l, Stream *s) {
  const RillBuffer *out = rill_exec_output(s->job, 1);
  if (!s->ready && out->size) {
    size_t n = out->size;
    if (emit_bytes(l, s, out->data, n, RILL_V_BYTES))
      rill_exec_consume(s->job, n);
  }
  if (s->input) {
    if (!rill_exec_feed_open(s->job)) {
      if (!s->input->closing && !s->input->done)
        close_chain(s->input, true);
    } else if (rill_exec_feed_ready(s->job)) {
      Stream *input = s->input;
      if (input->ready) {
        RillValue value = input->values[ITEM];
        if (value.kind != RILL_V_BYTES || !value.as.object->bytes.size) {
          error(&s->error, RILL_TYPE, "through requires nonempty Bytes chunks");
          return;
        }
        RillBytes bytes = value.as.object->bytes;
        size_t n = bytes.size - s->offset;
        if (n > RILL_EXEC_QUEUE_BYTES)
          n = RILL_EXEC_QUEUE_BYTES;
        if (!rill_exec_feed(s->job, (RillBytes){bytes.data + s->offset, n}))
          error(&s->error, RILL_MEMORY, "feed allocation failed");
        s->offset += n;
        if (s->offset == bytes.size) {
          release_item(input);
          s->offset = 0;
        }
      } else if (input->done)
        rill_exec_feed_end(s->job);
      else
        input->demand = true;
    }
  }
  if (rill_exec_state(s->job) == RILL_JOB_COMPLETED && !s->ready &&
      !out->size) {
    s->error = chain_error(s, true);
    if (!s->error.kind && !chain_live(s->input))
      s->done = true;
  }
}
static void source_next(RillLibrary *l, Sink *sink, Stream *s) {
  if (s->closing) {
    if (!chain_live(s))
      s->done = true;
    return;
  }
  if (!s->demand || s->done || s->callback || s->error.kind)
    return;
  if (s->kind == PROCESS) {
    process_next(l, s);
    return;
  }
  if (s->ready)
    return;
  switch (s->kind) {
  case CHUNKS: {
    RillBytes bytes = s->values[DATA].as.object->bytes;
    if (s->offset == bytes.size) {
      s->done = true;
      break;
    }
    size_t n = bytes.size - s->offset;
    if (n > RILL_EXEC_QUEUE_BYTES)
      n = RILL_EXEC_QUEUE_BYTES;
    if (emit_bytes(l, s, bytes.data + s->offset, n, RILL_V_BYTES))
      s->offset += n;
    break;
  }
  case FILES:
    directory_next(l, s);
    break;
  case LINES:
    lines_next(l, s);
    break;
  case TAKE:
    if (!s->remaining) {
      close_chain(s->input, true);
      s->closing = true;
      if (!chain_live(s))
        s->done = true;
      break;
    }
    [[fallthrough]];
  case MAP:
  case FILTER:
    if (s->input->ready) {
      if (s->kind == TAKE) {
        s->values[ITEM] = s->input->values[ITEM];
        s->ready = true;
        --s->remaining;
        release_item(s->input);
      } else
        callback(l, sink, s, s->values[FUNCTION], s->input->values[ITEM]);
    } else if (s->input->done)
      s->done = true;
    else
      s->input->demand = true;
    break;
  case PROCESS:
    unreachable();
  }
}
static Sink *sink_new(RillLibrary *l, enum SinkKind kind, Stream *source,
                      RillDiagnostic *d) {
  Sink *s = malloc(sizeof(*s));
  if (!s) {
    error(d, RILL_MEMORY, "sink allocation failed");
    return nullptr;
  }
  *s = (Sink){.kind = kind, .source = source, .parent = l->streams->sink};
  rill_runtime_root(heap(l), &s->root, s->values, SINK_ROOTS);
  rill_runtime_root(heap(l), &s->raised_root, &s->raised, 1);
  l->streams->sink = s;
  return s;
}
static void destroy_chain(RillLibrary *l, Stream *node) {
  while (node) {
    Stream *input = node->input;
    if (l->streams->nodes == node)
      l->streams->nodes = node->next;
    else {
      assert(node->previous);
      node->previous->next = node->next;
    }
    if (node->next)
      node->next->previous = node->previous;
    if (node->directory)
      (void)closedir(node->directory);
    if (node->job)
      rill_exec_acknowledge(node->job);
    rill_text_clear(&node->buffer);
    rill_runtime_unroot(heap(l), &node->root);
    free(node);
    node = input;
  }
}
static bool sink_finish(RillLibrary *l, Sink *s) {
  l->idle = false;
  RillValue result = s->error.kind ? s->raised : s->values[OUTPUT];
  if (!s->error.kind && s->kind == FOLD)
    result = s->values[ACCUMULATOR];
  if (!s->error.kind && s->kind == COLLECT_BYTES)
    result = object(l, RILL_V_BYTES, nullptr, 0, s->buffer.data, s->buffer.size,
                    &s->error);
  if (!s->error.kind && s->kind == COLLECT) {
    if (s->values[OUTPUT].kind == RILL_V_UNIT)
      result = object(l, RILL_V_LIST, nullptr, 0, nullptr, 0, &s->error);
    else if (s->used != s->capacity)
      result = object(l, RILL_V_LIST, s->values[OUTPUT].as.object->values,
                      s->used, nullptr, 0, &s->error);
  }
  RillDiagnostic d = s->error;
  for (Stream *n = s->source; n; n = n->input) {
    n->demand = false;
  }
  rill_runtime_resume(l->eval, result, d);
  if (s->kind != CREATE || d.kind)
    destroy_chain(l, s->source);
  l->streams->sink = s->parent;
  rill_runtime_unroot(heap(l), &s->raised_root);
  rill_runtime_unroot(heap(l), &s->root);
  rill_text_clear(&s->buffer);
  free(s);
  return true;
}
static void collect_item(RillLibrary *l, Sink *s, RillValue value) {
  if (s->used >= s->max_items) {
    error(&s->error, RILL_LIMIT, "collection item limit exceeded");
    return;
  }
  RillError charged = rill_runtime_charge_storage(
      s->values[BUDGET],
      sizeof(RillValue) + (s->used ? 0 : sizeof(RillObject)));
  if (!charged)
    charged = rill_runtime_charge(heap(l), s->values[BUDGET], value);
  if (charged) {
    error(&s->error, charged, "collection retained-byte limit exceeded");
    return;
  }
  if (s->used == s->capacity) {
    size_t capacity = s->capacity ? s->capacity : 16;
    if (s->capacity && ckd_mul(&capacity, capacity, 2)) {
      error(&s->error, RILL_MEMORY, "collection size overflow");
      return;
    }
    if (capacity > s->max_items)
      capacity = s->max_items;
    RillValue next =
        object(l, RILL_V_LIST, nullptr, capacity, nullptr, 0, &s->error);
    if (s->error.kind)
      return;
    if (s->used)
      memcpy(next.as.object->values, s->values[OUTPUT].as.object->values,
             s->used * sizeof(RillValue));
    // Capacity belongs to the sink; GC visits only initialized live items.
    next.as.object->count = s->used;
    s->values[OUTPUT] = next;
    s->capacity = capacity;
  }
  s->values[OUTPUT].as.object->values[s->used++] = value;
  s->values[OUTPUT].as.object->count = s->used;
}
void rill_stream_unwind(RillLibrary *l, int64_t checkpoint, RillValue result) {
  if (!l->streams) {
    (void)finish(l, result, (RillDiagnostic){});
    return;
  }
  RillStreams *scope = l->streams;
  scope->checkpoint = checkpoint;
  scope->result = result;
  scope->unwinding = true;
  rill_runtime_root(heap(l), &scope->root, &scope->result, 1);
  for (Stream *s = scope->nodes; s && s->id > checkpoint; s = s->next) {
    dispose_source(s, false);
    // Newer dependencies are visited by the scope walk itself. Only chains
    // transferred from before this checkpoint need a separate traversal.
    if (s->input && s->input->id <= checkpoint)
      close_chain(s->input, false);
  }
}
static bool pending(const RillLibrary *l) {
  return l->streams && l->streams->sink && !l->streams->sink->awaiting;
}
bool rill_stream_progress(RillLibrary *l) {
  if (l->streams && l->streams->unwinding) {
    RillStreams *scope = l->streams;
    for (Stream *s = scope->nodes; s && s->id > scope->checkpoint; s = s->next)
      if ((s->job && rill_exec_state(s->job) != RILL_JOB_COMPLETED) ||
          (s->input && s->input->id <= scope->checkpoint &&
           chain_live(s->input))) {
        l->idle = true;
        return false;
      }
    while (scope->nodes && scope->nodes->id > scope->checkpoint)
      destroy_chain(l, scope->nodes);
    rill_runtime_resume(l->eval, scope->result, (RillDiagnostic){});
    rill_runtime_unroot(heap(l), &scope->root);
    scope->result = (RillValue){};
    scope->unwinding = false;
    l->idle = false;
    return true;
  }
  if (!pending(l)) {
    l->idle = false;
    return true;
  }
  l->idle = true;
  Sink *s = l->streams->sink;
  Stream *source = s->source;
  if (s->kind == CREATE && !s->error.kind) {
    RillDiagnostic d = rill_exec_error(source->job);
    if (d.kind) {
      s->error = d;
    } else if (rill_exec_launched(source->job))
      return sink_finish(l, s);
    else
      return false;
  }
  if (!s->error.kind)
    s->error = chain_error(source, s->kind != CLOSE);
  if (s->error.kind && !s->closing) {
    close_chain(source, false);
    s->closing = true;
  }
  if (s->closing) {
    if (chain_live(source))
      return false;
    return sink_finish(l, s);
  }
  if (source->done)
    return sink_finish(l, s);
  for (Stream *n = source; n; n = n->input)
    n->demand = false;
  source->demand = true;
  // Newer consumers run before their dependencies. Repeated quanta propagate
  // demand without C recursion; each edge retains at most one language item.
  for (Stream *n = source; n && !s->awaiting; n = n->input) {
    bool ready = n->ready, done = n->done, demand = n->demand;
    size_t offset = n->offset, buffer = n->buffer.size,
           remaining = n->remaining;
    source_next(l, s, n);
    if (ready != n->ready || done != n->done || demand != n->demand ||
        offset != n->offset || buffer != n->buffer.size ||
        remaining != n->remaining || n->error.kind)
      l->idle = false;
  }
  if (s->awaiting)
    return true;
  if (!source->ready)
    return false;
  l->idle = false;
  RillValue value = source->values[ITEM];
  if (s->kind == COLLECT)
    collect_item(l, s, value);
  else if (s->kind == COLLECT_BYTES || s->kind == WRITE) {
    if (value.kind != RILL_V_BYTES || !value.as.object->bytes.size)
      error(&s->error, RILL_TYPE, "byte sink requires nonempty Bytes chunks");
    else {
      RillBytes bytes = value.as.object->bytes;
      if (s->kind == COLLECT_BYTES) {
        if (bytes.size > s->max_bytes - s->buffer.size)
          error(&s->error, RILL_LIMIT, "collection byte limit exceeded");
        else if (!rill_text_append(&s->buffer, bytes.data, bytes.size))
          error(&s->error, RILL_MEMORY, "collection allocation failed");
      } else {
        struct pollfd output = {.fd = STDOUT_FILENO, .events = POLLOUT};
        if (poll(&output, 1, 0) <= 0) {
          l->idle = true;
          return false;
        }
        size_t size = bytes.size - s->used;
        // Bound each write by the minimum POSIX atomic pipe-write size.
        if (size > _POSIX_PIPE_BUF)
          size = _POSIX_PIPE_BUF;
        ssize_t n = write(STDOUT_FILENO, bytes.data + s->used, size);
        if (n < 0 && errno != EINTR && errno != EAGAIN)
          s->error = (RillDiagnostic){
              .kind = RILL_IO, .code = errno, .message = "cannot write stdout"};
        if (n > 0)
          s->used += (size_t)n;
        if (s->used < bytes.size && !s->error.kind)
          return false;
        s->used = 0;
      }
    }
  } else if (s->kind == FOLD || s->kind == EACH) {
    RillValue argument = value;
    if (s->kind == FOLD) {
      RillValue pair[] = {s->values[ACCUMULATOR], value};
      argument = object(l, RILL_V_LIST, pair, 2, nullptr, 0, &s->error);
    }
    if (!s->error.kind)
      callback(l, s, nullptr, s->values[CALLBACK], argument);
  }
  if (!s->awaiting)
    release_item(source);
  return s->awaiting;
}
void rill_stream_callback(RillLibrary *l, RillValue result) {
  assert(l->streams && l->streams->sink && l->streams->sink->awaiting);
  Sink *s = l->streams->sink;
  Stream *n = s->callback_source;
  s->awaiting = false;
  RillValue value = {}, raised = {};
  if (rill_runtime_field(result, (RillBytes){"error", 5}, &raised)) {
    RillValue kind = {}, message = {};
    if (rill_runtime_field(raised, (RillBytes){"kind", 4}, &kind) &&
        rill_runtime_field(raised, (RillBytes){"message", 7}, &message)) {
      s->raised = raised;
      s->error =
          (RillDiagnostic){.kind = RILL_TYPE,
                           .label = kind.as.object->bytes.data,
                           .label_size = kind.as.object->bytes.size,
                           .message = message.as.object->bytes.data,
                           .message_size = message.as.object->bytes.size};
    } else
      error(&s->error, RILL_TYPE, "invalid callback error");
  } else if (!rill_runtime_field(result, (RillBytes){"value", 5}, &value))
    error(&s->error, RILL_TYPE, "invalid callback result");
  else if (n) {
    if (n->kind == MAP) {
      n->values[ITEM] = value;
      n->ready = true;
    } else if (value.kind != RILL_V_BOOL)
      error(&s->error, RILL_TYPE, "filter predicate requires Bool");
    else if (value.as.integer) {
      n->values[ITEM] = n->input->values[ITEM];
      n->ready = true;
    }
  } else if (s->kind == FOLD)
    s->values[ACCUMULATOR] = value;
  if (n) {
    n->callback = false;
    release_item(n->input);
  } else
    release_item(s->source);
}
bool rill_stream_call(RillLibrary *l, RillValue request) {
  if (request.kind != RILL_V_LIST || !request.as.object->count ||
      request.as.object->values[0].kind != RILL_V_STRING)
    return fail(l, RILL_TYPE, "invalid stream request");
  size_t count = request.as.object->count;
  RillValue name = request.as.object->values[0],
            a = count > 1 ? request.as.object->values[1] : (RillValue){},
            b = count > 2 ? request.as.object->values[2] : (RillValue){},
            c = count > 3 ? request.as.object->values[3] : (RillValue){};
  RillDiagnostic d = {};
  Stream *s = nullptr, *input = nullptr;
  if (named(name, "chunks")) {
    if (a.kind != RILL_V_BYTES)
      return fail(l, RILL_TYPE, "chunks requires Bytes");
    s = node(l, CHUNKS, &d);
    if (s)
      s->values[DATA] = a;
  } else if (named(name, "files")) {
    if (a.kind != RILL_V_PATH)
      return fail(l, RILL_TYPE, "files requires Path");
    s = node(l, FILES, &d);
    if (s) {
      s->values[DATA] = a;
      int fd = rill_platform_internal(
          open(a.as.object->bytes.data,
               O_RDONLY | O_DIRECTORY | O_CLOEXEC | O_NOCTTY));
      if (fd >= 0) {
        s->directory = fdopendir(fd);
        if (!s->directory)
          rill_platform_close(&fd);
      }
      if (!s->directory)
        d = (RillDiagnostic){
            .kind = RILL_IO, .code = errno, .message = "cannot open directory"};
    }
  } else if (named(name, "stream") || named(name, "through")) {
    bool through = named(name, "through");
    if (a.kind != RILL_V_PLAN)
      return fail(l, RILL_TYPE, "expected JobPlan");
    if (through)
      input = find(l, b, &d);
    if (through && !input)
      return finish(l, (RillValue){}, d);
    s = node(l, PROCESS, &d);
    if (s) {
      s->job = rill_library_launch(
          l, a, (RillExecSpec){.streaming = true, .feed = through}, &d);
      if (s->job) {
        s->input = input;
        if (input)
          input->claimed = true;
        Sink *pending = sink_new(l, CREATE, s, &d);
        if (pending) {
          pending->values[OUTPUT] = token(l, s, &d);
          pending->error = d;
          return false;
        }
      }
    }
  } else if (named(name, "map") || named(name, "filter") ||
             named(name, "take") || named(name, "lines")) {
    input = find(l, b, &d);
    if (!input)
      return finish(l, (RillValue){}, d);
    enum SourceKind kind = named(name, "map")      ? MAP
                           : named(name, "filter") ? FILTER
                           : named(name, "take")   ? TAKE
                                                   : LINES;
    RillLimit limit = {"max_line_bytes", (size_t)8 * 1024 * 1024, 0};
    if ((kind == MAP || kind == FILTER) && !callable(a))
      return fail(l, RILL_TYPE, "stream callback requires Function");
    if (kind == TAKE && (a.kind != RILL_V_INT || a.as.integer < 0 ||
                         (uint64_t)a.as.integer > SIZE_MAX))
      return fail(l, RILL_TYPE, "take count requires nonnegative Int");
    if (kind == LINES && !rill_library_limits(a, &limit, 1, &d))
      return finish(l, (RillValue){}, d);
    s = node(l, kind, &d);
    if (s) {
      s->input = input;
      input->claimed = true;
      if (kind == TAKE)
        s->remaining = (size_t)a.as.integer;
      if (kind == LINES)
        s->limit = limit.value;
      if (kind == MAP || kind == FILTER)
        s->values[FUNCTION] = a;
    }
  } else {
    enum SinkKind kind;
    RillValue stream = {};
    RillLimit limits[] = {{"max_items", 1'000'000, 0},
                          {"max_bytes", (size_t)64 * 1024 * 1024, 0}};
    if (named(name, "collect")) {
      kind = COLLECT;
      stream = b;
      if (!rill_library_limits(a, limits, 2, &d))
        return finish(l, (RillValue){}, d);
    } else if (named(name, "collect_bytes")) {
      kind = COLLECT_BYTES;
      stream = b;
      if (!rill_library_limits(a, limits + 1, 1, &d))
        return finish(l, (RillValue){}, d);
    } else if (named(name, "fold")) {
      kind = FOLD;
      stream = c;
    } else if (named(name, "each")) {
      kind = EACH;
      stream = b;
    } else if (named(name, "write")) {
      kind = WRITE;
      stream = a;
    } else if (named(name, "close")) {
      kind = CLOSE;
      stream = a;
    } else
      return fail(l, RILL_TYPE, "unknown stream operation");
    if ((kind == FOLD || kind == EACH) && !callable(a))
      return fail(l, RILL_TYPE, "sink callback requires Function");
    input = find(l, stream, &d);
    if (!input)
      return finish(l, (RillValue){}, d);
    Sink *sink = sink_new(l, kind, input, &d);
    if (!sink)
      return finish(l, (RillValue){}, d);
    input->claimed = true;
    sink->max_items = limits[0].value;
    sink->max_bytes = limits[1].value;
    if (kind == COLLECT) {
      sink->values[BUDGET] = rill_runtime_budget(heap(l), sink->max_bytes);
      if (sink->values[BUDGET].kind == RILL_V_UNIT)
        error(&sink->error, RILL_MEMORY, "budget allocation failed");
    }
    if (kind == FOLD || kind == EACH)
      sink->values[CALLBACK] = a;
    if (kind == FOLD)
      sink->values[ACCUMULATOR] = b;
    if (kind == CLOSE) {
      close_chain(input, false);
      sink->closing = true;
    }
    return false;
  }
  RillValue value = {};
  if (s && !d.kind)
    value = token(l, s, &d);
  return finish(l, value, d);
}
void rill_stream_cancel(RillLibrary *l) {
  for (Stream *s = l->streams ? l->streams->nodes : nullptr; s; s = s->next)
    dispose_source(s, false);
}
bool rill_stream_live(const RillLibrary *l) {
  for (Stream *s = l->streams ? l->streams->nodes : nullptr; s; s = s->next)
    if (s->job && rill_exec_state(s->job) != RILL_JOB_COMPLETED)
      return true;
  return false;
}
void rill_stream_clear(RillLibrary *l) {
  if (!l->streams)
    return;
  assert(!rill_stream_live(l));
  if (l->streams->unwinding)
    rill_runtime_unroot(heap(l), &l->streams->root);
  while (l->streams->sink) {
    Sink *s = l->streams->sink;
    l->streams->sink = s->parent;
    rill_runtime_unroot(heap(l), &s->raised_root);
    rill_runtime_unroot(heap(l), &s->root);
    rill_text_clear(&s->buffer);
    free(s);
  }
  while (l->streams->nodes) {
    Stream *s = l->streams->nodes;
    l->streams->nodes = s->next;
    if (s->job)
      rill_exec_acknowledge(s->job);
    if (s->directory)
      (void)closedir(s->directory);
    rill_text_clear(&s->buffer);
    rill_runtime_unroot(heap(l), &s->root);
    free(s);
  }
  free(l->streams);
  l->streams = nullptr;
}

void rill_stream_signal(RillLibrary *l, unsigned events) {
  for (Stream *s = l->streams ? l->streams->nodes : nullptr; s; s = s->next)
    if (s->job) {
      if (events & RILL_SIG_INT)
        rill_exec_cancel(s->job);
      if (events & RILL_SIG_STOP)
        rill_exec_stop(s->job);
    }
}
bool rill_stream_stopped(const RillLibrary *l) {
  for (Stream *s = l->streams ? l->streams->nodes : nullptr; s; s = s->next)
    if (s->job && rill_exec_state(s->job) == RILL_JOB_STOPPED)
      return true;
  return false;
}
bool rill_stream_quiescent(const RillLibrary *l) {
  for (Stream *s = l->streams ? l->streams->nodes : nullptr; s; s = s->next)
    if (s->job && !rill_exec_quiescent(s->job))
      return false;
  return true;
}
bool rill_stream_resume(RillLibrary *l) {
  for (Stream *s = l->streams ? l->streams->nodes : nullptr; s; s = s->next)
    if (s->job && rill_exec_state(s->job) == RILL_JOB_STOPPED &&
        !rill_exec_resume(l->exec, s->job, true))
      return false;
  return true;
}
bool rill_stream_owns(const RillStreams *streams, const RillJob *job) {
  for (Stream *s = streams ? streams->nodes : nullptr; s; s = s->next)
    if (s->job == job)
      return true;
  return false;
}
