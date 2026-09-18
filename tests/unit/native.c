/**
 * @file
 * @brief Standard-library values and scoped streams under stress collection.
 *
 * Each expression has an independent local scope. Cases check results and
 * error categories through the real evaluator and bundled library.
 */
#include "native/native.h"
#include "../support/check.h"
#include "diagnostic.h"
#include "native/bundle.h"
#include "native/json.h"
#include "native/stream.h"
#include "runtime/runtime.h"
#include "session/module.h"
#include "source.h"
#include "syntax/syntax.h"
#include "text/text.h"
#include <errno.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

static RillModules modules;
static RillEvalEvent run(RillEval *eval, const char *code) {
  [[gnu::cleanup(rill_source_clear)]] RillSource source = {};
  CHECK(rill_source_init(&source, "native-test", code, strlen(code)) ==
        RILL_OK);
  [[gnu::cleanup(rill_syntax_clear)]] RillSyntax syntax =
      rill_syntax_parse(&source);
  if (syntax.state != RILL_COMPLETE)
    (void)fprintf(stderr, "%s\nparse at %zu: %s\n", code,
                  syntax.diagnostic.offset, syntax.diagnostic.message);
  CHECK(syntax.state == RILL_COMPLETE);
  rill_runtime_begin(eval, &syntax);
  RillLibrary library = {.eval = eval};
  RillNativePending pending = {};
  for (;;) {
    if (!rill_stream_progress(&library))
      continue;
    // Resumable evaluator work must not ask the session to wait for I/O.
    CHECK(!library.idle);
    RillEvalEvent event = rill_runtime_step(eval);
    if (event.state == RILL_EVAL_YIELD)
      continue;
    if (event.state == RILL_EVAL_IMPORT || event.state == RILL_EVAL_MODULE) {
      rill_module_event(&modules, eval, event);
      continue;
    }
    if (event.state == RILL_EVAL_CALLBACK) {
      rill_stream_callback(&library, event.value);
      continue;
    }
    if (event.state == RILL_EVAL_CLEANUP) {
      rill_stream_unwind(&library, event.native, event.value);
      continue;
    }
    if (event.state != RILL_EVAL_NATIVE) {
      rill_stream_cancel(&library);
      rill_stream_clear(&library);
      return event;
    }
    (void)rill_library_call(&library, &pending, event);
  }
}
static void expect(RillEval *eval, const char *code, RillError error) {
  // Local declarations cannot leak into the next independent table case.
  [[gnu::cleanup(rill_text_clear)]] RillBuffer source = {};
  CHECK(rill_text_format(&source, "do { %s }", code));
  RillEvalEvent event = run(eval, source.data);
  bool ok =
      error ? event.state == RILL_EVAL_ERROR && event.diagnostic.kind == error
            : event.state == RILL_EVAL_DONE &&
                  event.value.kind == RILL_V_BOOL && event.value.as.integer;
  if (!ok)
    (void)fprintf(
        stderr, "%s\nexpected %s, got event %d: %s\n", code,
        error ? rill_diagnostic_name(error) : "true", (int)event.state,
        event.diagnostic.message ? event.diagnostic.message : "no diagnostic");
  CHECK(ok);
}
static void records(RillEval *eval) {
  const size_t sizes[] = {0, 1, 15, 16, 17, 31, 32, 33, 128};
  for (size_t i = 0; i < sizeof(sizes) / sizeof(*sizes); ++i) {
    [[gnu::cleanup(rill_text_clear)]] RillBuffer source = {};
    CHECK(rill_text_append(&source, "let pairs = [", 13));
    for (size_t j = 0; j < sizes[i]; ++j)
      CHECK(rill_text_format(&source, "['key%zu', %zu], ", j, j));
    const char *checks =
        "];\n"
        "let original = record pairs\n"
        "let permuted = record (reverse pairs)\n"
        "original == permuted and length original == length pairs and\n"
        "fold { ok pair => ok and original[pair[0]] == pair[1] } true pairs";
    CHECK(rill_text_append(&source, checks, strlen(checks)));
    expect(eval, source.data, RILL_OK);
  }
}
static void module_errors() {
  const struct {
    const char *source;
    RillError kind;
    int code;
  } cases[] = {
      {"import 'std:missing' as m", RILL_IO, 0},
      {"import 'std:prelude' as m", RILL_IO, 0},
      {"import 'relative.rill' as m", RILL_IO, 0},
      {"import \"bad\\u{0}path\" as m", RILL_TYPE, 0},
      {"import '/dev/null/module.rill' as m", RILL_IO, ENOTDIR},
      {"import '/dev/null' as m", RILL_IO, EINVAL},
  };
  RillEval *eval = rill_runtime_new(nullptr, 0);
  CHECK(eval);
  [[gnu::cleanup(rill_module_clear)]] RillModules cache = {};
  for (size_t i = 0; i < sizeof(cases) / sizeof(*cases); ++i) {
    [[gnu::cleanup(rill_source_clear)]] RillSource source = {};
    CHECK(rill_source_init(&source, "module-test", cases[i].source,
                           strlen(cases[i].source)) == RILL_OK);
    [[gnu::cleanup(rill_syntax_clear)]] RillSyntax syntax =
        rill_syntax_parse(&source);
    CHECK(syntax.state == RILL_COMPLETE);
    rill_runtime_begin(eval, &syntax);
    RillEvalEvent event = {};
    do {
      event = rill_runtime_step(eval);
    } while (event.state == RILL_EVAL_YIELD);
    CHECK(event.state == RILL_EVAL_IMPORT);
    // An earlier allocation failure must not change this error's meaning.
    errno = ENOMEM;
    rill_module_event(&cache, eval, event);
    do {
      event = rill_runtime_step(eval);
    } while (event.state == RILL_EVAL_YIELD);
    CHECK(event.state == RILL_EVAL_ERROR);
    CHECK(event.diagnostic.kind == cases[i].kind);
    CHECK(event.diagnostic.code == cases[i].code);
  }
  rill_runtime_free(eval);
}
static void json_builders() {
  RillHeap heap = {.stress = true};
  RillValue values[4] = {};
  RillRoot root = {};
  rill_runtime_root(&heap, &root, values, 4);
  values[0] = rill_runtime_record(&heap, nullptr, 0);
  CHECK(values[0].kind == RILL_V_RECORD);
  const size_t widths[] = {0, 1, 15, 16, 17, 128};
  for (size_t k = 0; k < sizeof(widths) / sizeof(*widths); ++k) {
    [[gnu::cleanup(rill_text_clear)]] RillBuffer input = {};
    CHECK(rill_text_append(&input, "{", 1));
    for (size_t i = 0; i < widths[k]; ++i)
      CHECK(rill_text_format(&input, "%s\"key%zu\":[%zu,{\"text\":\"kept\"}]",
                             i ? "," : "", i, i));
    CHECK(rill_text_append(&input, "}", 1));
    values[1] = rill_runtime_object(&heap, RILL_V_STRING, nullptr, 0,
                                    input.data, input.size, 0);
    CHECK(values[1].kind == RILL_V_STRING);
    RillDiagnostic error = {};
    values[2] = rill_library_json(&heap, false, values[0], values[1], &error);
    CHECK(!error.kind && values[2].kind == RILL_V_RECORD);
    CHECK(values[2].as.object->count == widths[k] * 2);
    for (size_t i = 0; i < widths[k]; ++i) {
      char name[32];
      int size = snprintf(name, sizeof(name), "key%zu", i);
      CHECK(size > 0 && (size_t)size < sizeof(name));
      RillValue field = {}, text = {};
      CHECK(rill_runtime_field(values[2], (RillBytes){name, (size_t)size},
                               &field));
      CHECK(rill_runtime_count(field) == 2 &&
            rill_runtime_at(field, 0).as.integer == (int64_t)i);
      CHECK(rill_runtime_field(rill_runtime_at(field, 1),
                               (RillBytes){"text", 4}, &text));
      CHECK(!strcmp(text.as.object->bytes.data, "kept"));
    }
    values[3] = rill_library_json(&heap, true, values[0], values[2], &error);
    CHECK(!error.kind && values[3].kind == RILL_V_BYTES);
    values[3] = rill_library_json(&heap, false, values[0], values[3], &error);
    bool equal = false;
    CHECK(!error.kind &&
          rill_runtime_equal(values[2], values[3], &equal) == RILL_OK && equal);
  }
  rill_runtime_unroot(&heap, &root);
  rill_runtime_heap_clear(&heap);
}
int main() {
  module_errors();
  json_builders();
  size_t count = 0;
  const RillNative *primitives = rill_library_natives(&count);
  RillEval *eval = rill_runtime_new(primitives, count);
  CHECK(eval);
  rill_runtime_heap(eval)->stress = true;
  CHECK(run(eval, rill_library_source("std:prelude")).state == RILL_EVAL_DONE);
  CHECK(rill_runtime_prelude(eval));
  CHECK(run(eval, "import \"std:core\" as core\n"
                  "import \"std:seq\" as seq\n"
                  "import \"std:option\" as option\n"
                  "import \"std:result\" as result")
            .state == RILL_EVAL_DONE);
  const char *const cases[] = {
      "split \"aab\" \"aaabaabaab\" == [\"a\", \"\", \"\", \"\"]",
      "split \"aa\" \"aaaaa\" == [\"\", \"\", \"a\"]",
      ("let factorial = rec { loop n =>\n"
       "  if n == 0 then 1 else n * loop (n - 1)\n"
       "}\n"
       "factorial 10 == 3628800"),
      ("let loop = 99\n"
       "let f = rec { loop x y =>\n"
       "  if y == 0 then x else loop (x + 1) (y - 1)\n"
       "}\n"
       "let partial = f 2\n"
       "partial 3 == 5 and partial 0 == 2 and loop == 99"),
      ("let factory = { k => rec { loop n =>\n"
       "  if n == 0 then k else loop (n - 1)\n"
       "} }\n"
       "let a = factory 7\n"
       "let b = factory 9\n"
       "a 50 == 7 and b 50 == 9"),
      ("let f = rec { loop [head, ..tail] =>\n"
       "  if head == 0 then { () => loop [2, 0] } else head\n"
       "}\n"
       "(f [0]) () == 2"),
      ("let f = rec { loop n =>\n"
       "  if n == 0 then 42 else loop (n - 1)\n"
       "}\n"
       "do {\n"
       "  let saved = f\n"
       "  let f = 0\n"
       "  saved 100 == 42\n"
       "}"),
      "core.some 1 == some 1 and seq.find (equal 2) [1, 2, 3] == some 2",
      ("option.map (add 1) (some 2) == some 3 and\n"
       "option.bind some Option.None == Option.None and\n"
       "option.unwrap_or_else { () => 7 } Option.None == 7 and\n"
       "option.unwrap_or_else { () => 1 / 0 } (some 2) == 2"),
      ("result.map (add 1) (ok 2) == ok 3 and\n"
       "result.bind ok (err 5) == err 5 and\n"
       "result.unwrap_or_else identity (err 5) == 5 and\n"
       "result.unwrap_or 9 (ok 2) == 2"),
      ("fold_until { acc x =>\n"
       "  if x > 3 then Control.Stop {value: acc}\n"
       "  else Control.Continue {value: acc + x}\n"
       "} 0 [1, 2, 3, 4, 5] == 6"),
      ("find (equal 2) [1, 2, 3] == some 2 and\n"
       "find (equal 0) [] == Option.None and\n"
       "any (equal 2) [2] and all (equal 2) [] and not any identity []"),
      ("filter_map { x =>\n"
       "  if x > 0 then some (x * 2) else Option.None\n"
       "} [0, 1, 2] == [2, 4]"),
      "range 0 10 |> drop 3 |> take 2 |> collect |> equal [3, 4]",
      ("items [1, 2, 3]\n"
       "  |> filter_map { x => if x > 1 then some x else Option.None }\n"
       "  |> collect\n"
       "  |> equal [2, 3]"),
      ("unfold { x => if x < 3 then some [x, x + 1] else Option.None } 0\n"
       "  |> fold add 0\n"
       "  |> equal 3"),
      ("range 9223372036854775806 9223372036854775807\n"
       "  |> collect\n"
       "  |> equal [9223372036854775806]"),
      ("fold_until { acc x =>\n"
       "  if x == 2 then Control.Stop {value: acc}\n"
       "  else Control.Continue {value: acc + x}\n"
       "} 0 (items [1, 2, 3]) == 1"),
      "range 10 0 |> collect |> equal []",
      ("split \":\" \":a::\" == [\"\", \"a\", \"\", \"\"] and\n"
       "split \":\" \"\" == [\"\"] and split \":\" \"abc\" == [\"abc\"]"),
      ("split \"\\u{0}\" \"a\\u{0}b\" == [\"a\", \"b\"] and\n"
       "join \"\\u{0}\" [\"a\", \"b\"] == \"a\\u{0}b\""),
      ("join \"/\" [\"a\", \"b\", \"\"] == \"a/b/\" and\n"
       "join \"\" [] == \"\" and trim \" \\tHello\\r\\n\" == \"Hello\""),
      ("parse_int \"-9223372036854775808\" == -9223372036854775808 and\n"
       "parse_int \"9223372036854775807\" == 9223372036854775807 and\n"
       "parse_float \"1e2\" == 100.0"),
      "parse_int \"0\" == 0 and parse_float \"-1.25e-2\" == -0.0125",
      "record (entries {a: [1], b: 2}) == {a: [1], b: 2} and entries {} == []",
      ("join_path \"a\" \"b\" == path \"a/b\" and\n"
       "join_path \"a\" \"/b\" == path \"/b\" and\n"
       "join_path (bytes [255]) \"x\" == path (bytes [255, 47, 120])"),
      ("basename \"/a/b/\" == path \"b\" and\n"
       "dirname \"/a/b/\" == path \"/a\" and dirname \"b\" == path \".\" and\n"
       "basename \"///\" == path \"/\" and dirname \"\" == path \".\""),

      "map (multiply 2) [1, 2, 3] == [2, 4, 6]",
      "filter { x => x > 1 } [1, 2, 3] == [2, 3]",
      "fold add 0 [1, 2, 3] == 6",
      "sum [] == 0 and sum [1.0, 2.0] == 3.0",
      ("sort_by { x => x.k } [{k: 2, n: 1}, {k: 1, n: 2}, {k: 2, n: 3}] ==\n"
       "  [{k: 1, n: 2}, {k: 2, n: 1}, {k: 2, n: 3}]"),
      "sort_by { x => (sort_by identity [x, 0])[1] } [3, 1, 2] == [1, 2, 3]",
      "sort_by_with {max_items: 0, max_bytes: 0} identity [] == []",
      "sort_by_with {max_items: 3} identity [3, 2, 1] == [1, 2, 3]",
      "take 0 [1, 2] == [] and take 9 [1, 2] == [1, 2]",
      "drop 0 [1, 2] == [1, 2] and drop 9 [1, 2] == []",
      "take 2 [1, 2, 3] == [1, 2] and drop 2 [1, 2, 3] == [3]",
      "concat [1] [2] == [1, 2] and reverse [1, 2] == [2, 1]",
      "concat [] [] == [] and reverse [] == []",
      ("match attempt { () => 1 + true } of {\n"
       "  Result.Err {error: Error {kind, ..}} => kind == 'TypeError',\n"
       "  _ => false\n"
       "}"),
      ("match attempt { () => 9 } of {\n"
       "  Result.Ok {value} => value == 9,\n"
       "  _ => false\n"
       "}"),
      ("match attempt { () => 1 + true } of {\n"
       "  Result.Err {error} =>\n"
       "    error.span.offset > 0 and byte_length error.span.source > 0,\n"
       "  _ => false\n"
       "}"),
      "let Result = 4; match attempt { () => 9 } of { _ => true }",
      ("fn outer x = do {\n"
       "  rec {\n"
       "    fn a n = if n == 0 then x else b (n - 1)\n"
       "    fn b n = a n\n"
       "  }\n"
       "  a 10\n"
       "}\n"
       "outer 7 == 7"),
      "fn get [Option.Some {value}] = value; get [some 7] == 7",
      "let f = { [head, ..tail] => tail }; f [0, 1, 2] == [1, 2]",
      "match { x => x } of { 1 => false, _ => true }",
      "record [['a', [1, 2]]] == {a: [1, 2]}",
      "extend {b: 2} {a: 1} == {a: 1, b: 2}",
      "match lookup 'missing' {} of { Option.None => true, _ => false }",
      "lookup 'a' {a: null} == some null",
      ("let large = record (map { x => [text x, x] }\n"
       "  [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17])\n"
       "let updated = large with {'17': 19}\n"
       "let rest = match large of { {'0': zero, ..rest} => rest }\n"
       "let small = match large of { {'0': a, '1': b, '2': c, ..rest} => rest "
       "}\n"
       "updated['17'] == 19 and large['17'] == 17 and\n"
       "length rest == 17 and rest['17'] == 17 and\n"
       "length small == 15 and small['17'] == 17"),
      "scalars '\u754ca' == ['\u754c', 'a'] and byte_length '\u754ca' == 4",
      ("scalars '' == [] and scalars \"a\\u{0}\u754c\" == ['a', \"\\u{0}\", "
       "'\u754c']"),
      "decode_utf8 (encode_utf8 '\u754c') == '\u754c'",
      "decode_utf8 (encode_utf8 \"\\u{0}\u754c\") == \"\\u{0}\u754c\"",
      "starts_with '\u754c' '\u754ca' and ends_with 'a' '\u754ca'",
      "starts_with '' '' and ends_with '' '' and not starts_with 'ab' 'a'",
      "div (-7) 3 == -2 and rem (-7) 3 == -1",
      "div 7 (-3) == -2 and rem 7 (-3) == 1",
      "rem (-9223372036854775808) (-1) == 0",
      "int 2.9 == 2 and int (-2.9) == -2 and float 2 == 2.0",
      "int (-9223372036854775808.0) == -9223372036854775808",
      "() != null and null != Option.None and Option.None != []"};
  for (size_t i = 0; i < sizeof(cases) / sizeof(*cases); ++i)
    expect(eval, cases[i], RILL_OK);
  const struct {
    const char *source;
    RillError error;
  } errors[] = {{"record [['a', 1], ['a', 2]]", RILL_TYPE},
                {"record [[1, 2]]", RILL_TYPE},
                {"record [['a']]", RILL_TYPE},
                {"extend {a: 1} {a: 2}", RILL_TYPE},
                {"take (-1) []", RILL_TYPE},
                {"drop 1 true", RILL_TYPE},
                {"sum ['bad']", RILL_TYPE},
                {"int 1e100", RILL_ARITHMETIC},
                {"int 9223372036854775808.0", RILL_ARITHMETIC},
                {"div (-9223372036854775808) (-1)", RILL_ARITHMETIC},
                {"div 1 0", RILL_ARITHMETIC},
                {"rem 1 0", RILL_ARITHMETIC},
                {"sort_by_with {max_items: 1} identity [2, 1]", RILL_LIMIT},
                {"sort_by_with {max_bytes: 0} identity [1]", RILL_LIMIT},
                {"sort_by_with {unknown: 1} identity []", RILL_TYPE},
                {"sort_by_with {max_items: -1} identity []", RILL_TYPE},
                {"sort_by identity [1, 2.0]", RILL_TYPE},
                {"sort_by identity [true]", RILL_TYPE},
                {"parse_int \"1.0\"", RILL_ARITHMETIC},
                {"parse_int \"9223372036854775808\"", RILL_ARITHMETIC},
                {"parse_int \"-9223372036854775809\"", RILL_ARITHMETIC},
                {"parse_int \"1e2\"", RILL_ARITHMETIC},
                {"parse_int \"\"", RILL_DECODE},
                {"parse_int \"+1\"", RILL_DECODE},
                {"parse_int \"01\"", RILL_DECODE},
                {"parse_float \" 1\"", RILL_DECODE},
                {"parse_float \"1 \"", RILL_DECODE},
                {"parse_float \"nan\"", RILL_DECODE},
                {"parse_int (join \"\" (map { _ => \"9999999999\" }\n"
                 "  (range 0 40 |> collect)))",
                 RILL_ARITHMETIC},
                {"parse_float \"1e999\"", RILL_ARITHMETIC},
                {"parse_float \"1e999x\"", RILL_DECODE},
                {"parse_float \"1e\"", RILL_DECODE},
                {"parse_int \"1\\u{0}2\"", RILL_DECODE},
                {"parse_float \"1 trailing\"", RILL_DECODE},
                {"parse_int 1", RILL_TYPE},
                {"parse_float (bytes [49])", RILL_TYPE},
                {"split \"\" \"a\"", RILL_TYPE},
                {"join \":\" [1]", RILL_TYPE},
                {"join_path \"a\" \"\\u{0}\"", RILL_TYPE},
                {"range 0.0 2", RILL_TYPE},
                {"let f = rec { loop n => n }; loop 1", RILL_TYPE}};
  for (size_t i = 0; i < sizeof(errors) / sizeof(*errors); ++i)
    expect(eval, errors[i].source, errors[i].error);
  records(eval);
  rill_module_clear(&modules);
  rill_runtime_free(eval);
}
