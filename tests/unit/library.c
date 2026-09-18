/**
 * @file
 * @brief Pure standard-library behavior under stress collection.
 *
 * Each expression has an independent local scope. Cases check results and
 * error categories through the real evaluator and bundled library.
 */
#include "diagnostic.h"
#include "library/bundle.h"
#include "library/json.h"
#include "library/pure.h"
#include "runtime/runtime.h"
#include "source.h"
#include "syntax/syntax.h"
#include "test.h"
#include "text/text.h"
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

static RillEvalEvent run(RillEval *eval, const char *code) {
  [[gnu::cleanup(rill_source_clear)]] RillSource source = {};
  CHECK(rill_source_init(&source, "library-test", code, strlen(code)) ==
        RILL_OK);
  [[gnu::cleanup(rill_syntax_clear)]] RillSyntax syntax =
      rill_syntax_parse(&source);
  if (syntax.state != RILL_COMPLETE)
    (void)fprintf(stderr, "%s\nparse at %zu: %s\n", code,
                  syntax.diagnostic.offset, syntax.diagnostic.message);
  CHECK(syntax.state == RILL_COMPLETE);
  rill_runtime_begin(eval, &syntax);
  for (;;) {
    RillEvalEvent event = rill_runtime_step(eval);
    if (event.state == RILL_EVAL_YIELD)
      continue;
    if (event.state != RILL_EVAL_NATIVE)
      return event;
    CHECK(event.native == 1);
    RillDiagnostic diagnostic = {};
    RillValue value =
        rill_library_pure(rill_runtime_heap(eval), event.value, &diagnostic);
    rill_runtime_resume(eval, value, diagnostic);
  }
}
static void expect(RillEval *eval, const char *code, RillError error) {
  // Local declarations cannot leak into the next independent table case.
  [[gnu::cleanup(rill_text_clear)]] RillBuffer source = {};
  CHECK(rill_text_format(&source, "do {%s}", code));
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
    CHECK(rill_text_append(&source, "let pairs=[", 11));
    for (size_t j = 0; j < sizes[i]; ++j)
      CHECK(rill_text_format(&source, "['key%zu',%zu],", j, j));
    const char *checks =
        "]; let original=record(pairs); let permuted=record(reverse(pairs));"
        "original==permuted and length(original)==length(pairs) and "
        "fold(fn(ok,pair)=>ok and original[pair[0]]==pair[1],true,pairs)";
    CHECK(rill_text_append(&source, checks, strlen(checks)));
    expect(eval, source.data, RILL_OK);
  }
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
  json_builders();
  const RillNative primitives[] = {
      {"__pure", 1}, {"__process", 2}, {"__stream", 3}, {"__data", 4}};
  RillEval *eval =
      rill_runtime_new(primitives, sizeof(primitives) / sizeof(*primitives));
  CHECK(eval);
  rill_runtime_heap(eval)->stress = true;
  CHECK(run(eval, rill_library_source("std:prelude")).state == RILL_EVAL_DONE);
  rill_runtime_prelude(eval);
  const char *const cases[] = {
      "map(multiply(2),[1,2,3])==[2,4,6]",
      "filter(fn(x)=>x>1,[1,2,3])==[2,3]",
      "fold(add,0,[1,2,3])==6",
      "sum([])==0 and sum([1.0,2.0])==3.0",
      ("sort_by(fn(x)=>x.k,[{k:2,n:1},{k:1,n:2},{k:2,n:3}])=="
       "[{k:1,n:2},{k:2,n:1},{k:2,n:3}]"),
      "sort_by(fn(x)=>sort_by(identity,[x,0])[1],[3,1,2])==[1,2,3]",
      "sort_by_with({max_items:0,max_bytes:0},identity,[])==[]",
      "sort_by_with({max_items:3},identity,[3,2,1])==[1,2,3]",
      "take(0,[1,2])==[] and take(9,[1,2])==[1,2]",
      "drop(0,[1,2])==[1,2] and drop(9,[1,2])==[]",
      "take(2,[1,2,3])==[1,2] and drop(2,[1,2,3])==[3]",
      "concat([1],[2])==[1,2] and reverse([1,2])==[2,1]",
      "concat([],[])==[] and reverse([])==[]",
      ("match attempt(fn()=>1+true) {Result.Err {error:Error "
       "{kind,..}}=>kind=='TypeError',_=>false}"),
      "match attempt(fn()=>9) {Result.Ok {value}=>value==9,_=>false}",
      ("match attempt(fn()=>1+true) {Result.Err {error}=>error.span.offset>0 "
       "and byte_length(error.span.source)>0,_=>false}"),
      "let Result=4; match attempt(fn()=>9) {_=>true}",
      ("fn outer(x)=>do {rec {fn a(n)=>if n==0 then x else b(n-1); fn "
       "b(n)=>a(n)}; a(10)}; outer(7)==7"),
      "fn get([Option.Some {value}])=>value; get([some(7)])==7",
      "let f=fn([head,..tail])=>tail; f([0,1,2])==[1,2]",
      "match fn(x)=>x {1=>false,_=>true}",
      "record([['a',[1,2]]])=={a:[1,2]}",
      "extend({b:2},{a:1})=={a:1,b:2}",
      "match lookup('missing',{}) {Option.None=>true,_=>false}",
      "lookup('a',{a:null})==some(null)",
      ("let "
       "large=record(map(fn(x)=>[text(x),x],[0,1,2,3,4,5,6,7,8,9,10,11,12,13,"
       "14,"
       "15,16,17]));"
       "let updated=large with {'17':19};"
       "let rest=match large {{'0':zero,..rest}=>rest};"
       "let small=match large {{'0':a,'1':b,'2':c,..rest}=>rest};"
       "updated['17']==19 and large['17']==17 and length(rest)==17 and "
       "rest['17']==17 and "
       "length(small)==15 and small['17']==17"),
      "scalars('\u754ca')==['\u754c','a'] and byte_length('\u754ca')==4",
      ("scalars('')==[] and "
       "scalars(\"a\\u{0}\u754c\")==['a',\"\\u{0}\",'\u754c']"),
      "decode_utf8(encode_utf8('\u754c'))=='\u754c'",
      "decode_utf8(encode_utf8(\"\\u{0}\u754c\"))==\"\\u{0}\u754c\"",
      "starts_with('\u754c','\u754ca') and ends_with('a','\u754ca')",
      "starts_with('','') and ends_with('','') and not starts_with('ab','a')",
      "div(-7,3)==-2 and rem(-7,3)==-1",
      "div(7,-3)==-2 and rem(7,-3)==1",
      "rem(-9223372036854775808,-1)==0",
      "int(2.9)==2 and int(-2.9)==-2 and float(2)==2.0",
      "int(-9223372036854775808.0)==-9223372036854775808",
      "() != null and null != Option.None and Option.None != []"};
  for (size_t i = 0; i < sizeof(cases) / sizeof(*cases); ++i)
    expect(eval, cases[i], RILL_OK);
  const struct {
    const char *source;
    RillError error;
  } errors[] = {{"record([['a',1],['a',2]])", RILL_TYPE},
                {"record([[1,2]])", RILL_TYPE},
                {"record([['a']])", RILL_TYPE},
                {"extend({a:1},{a:2})", RILL_TYPE},
                {"take(-1,[])", RILL_TYPE},
                {"drop(1,true)", RILL_TYPE},
                {"sum(['bad'])", RILL_TYPE},
                {"int(1e100)", RILL_ARITHMETIC},
                {"int(9223372036854775808.0)", RILL_ARITHMETIC},
                {"div(-9223372036854775808,-1)", RILL_ARITHMETIC},
                {"div(1,0)", RILL_ARITHMETIC},
                {"rem(1,0)", RILL_ARITHMETIC},
                {"sort_by_with({max_items:1},identity,[2,1])", RILL_LIMIT},
                {"sort_by_with({max_bytes:0},identity,[1])", RILL_LIMIT},
                {"sort_by_with({unknown:1},identity,[])", RILL_TYPE},
                {"sort_by_with({max_items:-1},identity,[])", RILL_TYPE},
                {"sort_by(identity,[1,2.0])", RILL_TYPE},
                {"sort_by(identity,[true])", RILL_TYPE}};
  for (size_t i = 0; i < sizeof(errors) / sizeof(*errors); ++i)
    expect(eval, errors[i].source, errors[i].error);
  records(eval);
  rill_runtime_free(eval);
}
