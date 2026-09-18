#include "diagnostic.h"
#include "runtime/runtime.h"
#include "source.h"
#include "syntax/syntax.h"
#include "test.h"
#include "text/text.h"
#include <inttypes.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

static RillEvalEvent evaluate(RillEval *eval, const char *text, size_t *depth) {
  RillSource source = {};
  CHECK(rill_source_init(&source, "language-test", text, strlen(text)) ==
        RILL_OK);
  RillSyntax syntax = rill_syntax_parse(&source);
  if (syntax.state != RILL_COMPLETE)
    (void)fprintf(stderr, "parse at %zu: %s\n", syntax.diagnostic.offset,
                  syntax.diagnostic.message);
  CHECK(syntax.state == RILL_COMPLETE);
  rill_runtime_begin(eval, &syntax);
  CHECK(!syntax.first && !syntax.allocated && !syntax.source.data);
  // Closures own code independently of the caller's parse arena and source.
  rill_syntax_clear(&syntax);
  rill_source_clear(&source);
  RillEvalEvent event = {};
  do {
    event = rill_runtime_step(eval);
    if (depth && rill_runtime_depth(eval) > *depth)
      *depth = rill_runtime_depth(eval);
  } while (event.state == RILL_EVAL_YIELD);
  return event;
}
static void expect(RillEval *eval, const char *source, int64_t expected) {
  RillEvalEvent event = evaluate(eval, source, nullptr);
  if (event.state != RILL_EVAL_DONE)
    (void)fprintf(stderr, "%s\neval at %zu: %s\n", source,
                  event.diagnostic.offset,
                  event.diagnostic.message ? event.diagnostic.message
                                           : "unexpected event");
  CHECK(event.state == RILL_EVAL_DONE);
  if (event.value.kind != RILL_V_INT || event.value.as.integer != expected)
    (void)fprintf(stderr, "%s\nexpected Int %" PRId64 "\n", source, expected);
  CHECK(event.value.kind == RILL_V_INT && event.value.as.integer == expected);
}
static void constants() {
  RillEval *eval = rill_runtime_new(nullptr, 0);
  CHECK(eval);
  RillHeap *heap = rill_runtime_heap(eval);
  heap->stress = true;
  RillEvalEvent event =
      evaluate(eval,
               "fn literal()=>\"a\\u{0}b\"; fn record(x)=>{key:x};"
               "[literal(),literal(),record(1),record(2)]",
               nullptr);
  CHECK(event.state == RILL_EVAL_DONE && event.value.kind == RILL_V_LIST);
  RillValue *items = event.value.as.object->values;
  CHECK(items[0].as.object == items[1].as.object);
  CHECK(items[0].as.object->bytes.size == 3);
  CHECK(!memcmp(items[0].as.object->bytes.data, "a\0b", 3));
  CHECK(items[2].as.object != items[3].as.object);
  CHECK(items[2].as.object->values[0].as.object ==
        items[3].as.object->values[0].as.object);
  CHECK(items[2].as.object->values[1].as.integer == 1);
  CHECK(items[3].as.object->values[1].as.integer == 2);
  // Keeping a literal must not keep its original code or unrelated literals.
  event = evaluate(eval, "let saved=literal(); let literal=0; let record=0",
                   nullptr);
  CHECK(event.state == RILL_EVAL_DONE);
  event = evaluate(eval, "saved", nullptr);
  CHECK(event.state == RILL_EVAL_DONE && event.value.kind == RILL_V_STRING);
  rill_runtime_collect(heap);
  size_t codes = 0;
  for (RillObject *o = heap->objects; o; o = o->next)
    if (o->kind == RILL_V_CODE)
      ++codes;
  CHECK(codes == 1); // Only the current entry's code remains.
  CHECK(event.value.as.object->bytes.size == 3);
  rill_runtime_free(eval);
}

static void pattern_validation_order() {
  const RillNative native = {"effect", 42};
  RillEval *eval = rill_runtime_new(&native, 1);
  CHECK(eval);
  rill_runtime_heap(eval)->stress = true;
  const char *sources[] = {"let [x,x]=effect(7)",
                           "match effect(7) {[x,x]=>0,_=>1}"};
  for (size_t i = 0; i < sizeof(sources) / sizeof(*sources); ++i) {
    RillEvalEvent event = evaluate(eval, sources[i], nullptr);
    CHECK(event.state == RILL_EVAL_NATIVE && event.native == 42);
    CHECK(event.value.kind == RILL_V_INT && event.value.as.integer == 7);
    rill_runtime_collect(rill_runtime_heap(eval));
    rill_runtime_resume(eval, event.value, (RillDiagnostic){});
    do {
      event = rill_runtime_step(eval);
    } while (event.state == RILL_EVAL_YIELD);
    CHECK(event.state == RILL_EVAL_ERROR && event.diagnostic.kind == RILL_TYPE);
    CHECK(!strcmp(event.diagnostic.message, "duplicate name in pattern"));
  }
  rill_runtime_free(eval);
}

static void values_and_patterns() {
  RillEval *eval = rill_runtime_new(nullptr, 0);
  CHECK(eval);
  rill_runtime_heap(eval)->stress = true;
  expect(eval, "let x = 4; let f=fn(y)=>x+y; f(3)", 7);
  expect(eval, "let x=90; f(2)", 6);
  expect(eval, "(fn(x)=>fn(x)=>x)(2,3)", 3);
  expect(eval, "(fn(x)=>do {let x=7; x})(2)", 7);
  expect(eval, "match [2,3] {[head,.._]=>head}", 2);
  expect(eval, "fn accepts('payload')=>11; accepts('payload')", 11);
  expect(eval, "accepts('payload')", 11);
  expect(eval, "match {x:2,y:3} {{x,.._}=>x}", 2);
  expect(
      eval,
      "let g=fn([a,..rest],{b,..})=>a+b+rest[0]; let h=g([2,3]); h({b:4,c:9})",
      9);
  expect(eval,
         "struct Point {x,y}; let p=Point({y:4,x:2}); let q=p with {x:3}; "
         "match q { Point {x,..r} => x+r.y+p.x }",
         9);
  expect(eval,
         "enum E {A, B {value}}; let ctor=E.B; let v=ctor({value:3}); match v "
         "{ E.B {value} if false => 9, E.B {value} => value }",
         3);
  expect(eval, "let fieldless=E.A; match fieldless { E.A => 7, _ => 0 }", 7);
  expect(eval, "if {a:1,b:[2]} == {b:[2],a:1} then 1 else 0", 1);
  expect(eval,
         "let table={\"\\u{0}x\":7,\"\":3}; table[\"\\u{0}x\"]+table[\"\"]",
         10);
  expect(eval,
         "fn make(x) => do {let unused=[1,2,3]; fn(y)=>x+y}; let "
         "saved=make(4); saved(5)",
         9);
  RillValue saved = {};
  CHECK(rill_runtime_lookup(eval, "saved", &saved));
  CHECK(saved.kind == RILL_V_CLOSURE && saved.as.object->count == 2);
  CHECK(saved.as.object->values[0].kind == RILL_V_CODE);
  CHECK(saved.as.object->values[1].kind == RILL_V_INT &&
        saved.as.object->values[1].as.integer == 4);
  expect(eval,
         "let maker=fn(x)=>fn(y)=>x+y; let a=maker(10); let b=maker(20); "
         "a(1)+b(2)",
         33);
  expect(eval, "if false then fn([x,x])=>x else 7", 7);
  expect(eval, "if false then do {let [x,x]=[1,2]; x} else 7", 7);
  expect(eval, "if false then match [1,2] {[x,x]=>x} else 8", 8);
  expect(eval, "if false then fn()=>missing else 8", 8);
  expect(eval,
         "let make_nominal=fn(C)=>fn(C {value})=>value; let "
         "read=make_nominal(E.B); read(E.B({value:19}))",
         19);
  expect(eval,
         "let wide=do {let h=8; let g=7; let f=6; let e=5; let d=4; "
         "let c=3; let b=2; let a=1; fn(a)=>a+b+c+d+e+f+g+h}; wide(10)",
         45);
  expect(eval,
         "let captured=do {let a=1; let b=2; let c=3; let d=4; let e=5; "
         "let f=6; let g=7; let h=8; fn()=>a+b+c+d+e+f+g+h}; captured()",
         36);
  expect(eval, "let a=100; let b=200; captured()", 36);
  RillEvalEvent failed = evaluate(eval, "1 + absent", nullptr);
  CHECK(failed.state == RILL_EVAL_ERROR && failed.diagnostic.offset == 4);
  const struct {
    const char *source;
    RillError error;
  } bad[] = {{"Point({x:1})", RILL_TYPE},
             {"Point({x:1,y:2,z:3})", RILL_TYPE},
             {"Point({x:1,y:2}) with {z:3}", RILL_MISSING_FIELD},
             {"match p { {x,y} => x }", RILL_MATCH_ERROR},
             {"match 0 { Point {missing} => 1, _ => 0 }", RILL_TYPE},
             {"let [a,a]=[1,1]", RILL_TYPE},
             {"g([])", RILL_MATCH_ERROR},
             {"E.A()", RILL_TYPE},
             {"struct Point {x}", RILL_TYPE},
             {"1+1.0", RILL_TYPE},
             {"9223372036854775807+1", RILL_ARITHMETIC},
             {"-(-9223372036854775808)", RILL_ARITHMETIC},
             {"1.0/0.0", RILL_ARITHMETIC},
             {"1.0e308*1.0e308", RILL_ARITHMETIC},
             {"[1][-1]", RILL_TYPE},
             {"if 1 then 1 else 2", RILL_TYPE},
             {"{a:1,b:fn(x)=>x}=={a:2,b:fn(x)=>x}", RILL_TYPE}};
  for (size_t i = 0; i < sizeof(bad) / sizeof(*bad); ++i) {
    RillEvalEvent event = evaluate(eval, bad[i].source, nullptr);
    if (event.state != RILL_EVAL_ERROR || event.diagnostic.kind != bad[i].error)
      (void)fprintf(stderr, "%s\nexpected %s, got %s\n", bad[i].source,
                    rill_diagnostic_name(bad[i].error),
                    rill_diagnostic_name(event.diagnostic.kind));
    CHECK(event.state == RILL_EVAL_ERROR &&
          event.diagnostic.kind == bad[i].error);
  }
  expect(eval, "let before=7; before", 7);
  CHECK(evaluate(eval, "let before=8; missing", nullptr).state ==
        RILL_EVAL_ERROR);
  expect(eval, "before", 7);
  rill_runtime_heap(eval)->stress = false;
  RillEvalEvent limited =
      evaluate(eval, "fn recurse(n)=>1+recurse(n+1); recurse(0)", nullptr);
  CHECK(limited.state == RILL_EVAL_ERROR &&
        limited.diagnostic.kind == RILL_LIMIT);
  rill_runtime_free(eval);
}
static void wide_operands() {
  RillEval *eval = rill_runtime_new(nullptr, 0);
  CHECK(eval);
  rill_runtime_heap(eval)->stress = true;
  RillBuffer source = {};
  CHECK(rill_text_append(&source, "let items=[", 11));
  for (size_t i = 0; i < 64; ++i)
    CHECK(rill_text_format(&source, "%zu,", i));
  CHECK(rill_text_append(&source, "]; let fields={", 15));
  for (size_t i = 0; i < 64; ++i)
    CHECK(rill_text_format(&source, "key%zu:%zu,", i, i));
  CHECK(rill_text_append(&source, "}; enum Wide {", 14));
  for (size_t i = 0; i < 64; ++i)
    CHECK(rill_text_format(&source, "Case%zu,", i));
  CHECK(rill_text_append(&source, "}; fn block()=>do {", 19));
  for (size_t i = 0; i < 64; ++i)
    CHECK(rill_text_append(&source, "();", 3));
  const char *end =
      "items[63]+fields.key63}; match Wide.Case63 {Wide.Case63=>block()}";
  CHECK(rill_text_append(&source, end, strlen(end)));
  expect(eval, source.data, 126);
  rill_text_clear(&source);
  rill_runtime_free(eval);
}

static void tail_calls() {
  static const char *const cases[] = {
      "fn loop(n,acc)=>if n==0 then acc else loop(n-1,acc+1); loop(1000000,0)",
      "rec {fn even(n,acc)=>if n==0 then acc else odd(n-1,acc+1); fn "
      "odd(n,acc)=>if n==0 then acc else even(n-1,acc+1)}; even(1000000,0)",
      "fn bounce(f,x)=>f(x); fn loop(n)=>if n==0 then 1000000 else "
      "bounce(loop,n-1); loop(1000000)"};
  for (size_t i = 0; i < sizeof(cases) / sizeof(*cases); ++i) {
    RillEval *eval = rill_runtime_new(nullptr, 0);
    CHECK(eval);
    size_t depth = 0;
    RillEvalEvent event = evaluate(eval, cases[i], &depth);
    if (event.state != RILL_EVAL_DONE || event.value.kind != RILL_V_INT ||
        event.value.as.integer != 1000000)
      (void)fprintf(stderr, "tail case %zu: %s\n", i, cases[i]);
    CHECK(event.state == RILL_EVAL_DONE && event.value.kind == RILL_V_INT &&
          event.value.as.integer == 1000000);
    CHECK(depth < 32);
    rill_runtime_collect(rill_runtime_heap(eval));
    CHECK(rill_runtime_heap(eval)->bytes < 200000);
    rill_runtime_free(eval);
  }
}
static void collection() {
  RillEval *eval = rill_runtime_new(nullptr, 0);
  CHECK(eval);
  rill_runtime_heap(eval)->stress = true;
  expect(eval, "fn f(n)=>if n==0 then 9 else f(n-1); f(1000)", 9);
  expect(eval,
         "fn churn(n,temporary)=>if n==0 then 9 else "
         "churn(n-1,[n,{value:n},fn()=>n]); churn(2000,[])",
         9);
  rill_runtime_collect(rill_runtime_heap(eval));
  CHECK(rill_runtime_heap(eval)->bytes < 200000);
  expect(eval, "let held=fn(x)=>x+1; held(1)", 2);
  for (size_t i = 0; i < 200; ++i)
    expect(eval, "let held=fn(x)=>x+1; held(1)", 2);
  rill_runtime_collect(rill_runtime_heap(eval));
  size_t before = rill_runtime_heap(eval)->bytes;
  for (size_t i = 0; i < 200; ++i)
    expect(eval, "let held=fn(x)=>x+1; held(1)", 2);
  rill_runtime_collect(rill_runtime_heap(eval));
  CHECK(rill_runtime_heap(eval)->bytes == before);
  rill_runtime_free(eval);
}
static void binding_snapshots() {
  RillEval *eval = rill_runtime_new(nullptr, 0);
  CHECK(eval);
  RillHeap *heap = rill_runtime_heap(eval);
  heap->stress = true;
  for (size_t i = 0; i < 256; ++i) {
    char name[32];
    int size = snprintf(name, sizeof(name), "binding%zu", i);
    CHECK(size > 0 && (size_t)size < sizeof(name));
    CHECK(rill_runtime_define(
        eval, name, (RillValue){.kind = RILL_V_INT, .as.integer = (int64_t)i}));
  }
  expect(eval, "let saved=fn()=>binding0+binding255; saved()", 255);
  rill_runtime_prelude(eval);
  for (size_t pass = 0; pass < 4; ++pass) {
    expect(eval, "let binding0=900; let binding255=100; saved()", 255);
    expect(eval, "binding0+binding255", 1000);
    RillValue original = {};
    CHECK(rill_runtime_builtin(eval, "binding0", &original));
    CHECK(original.kind == RILL_V_INT && original.as.integer == 0);
    CHECK(evaluate(eval, "let binding0=1; absent", nullptr).state ==
          RILL_EVAL_ERROR);
    expect(eval, "binding0", 900);
    CHECK(evaluate(eval, "let duplicate=1; let duplicate=2", nullptr).state ==
          RILL_EVAL_ERROR);
    CHECK(!rill_runtime_lookup(eval, "duplicate", &original));
  }
  rill_runtime_collect(heap);
  size_t snapshots = 0;
  for (RillObject *o = heap->objects; o; o = o->next)
    if (o->kind == RILL_V_BINDINGS)
      ++snapshots;
  CHECK(snapshots == 2); // Current bindings and the explicitly frozen prelude.
  for (size_t i = 1; i < 255; ++i) {
    char name[32];
    int size = snprintf(name, sizeof(name), "binding%zu", i);
    CHECK(size > 0 && (size_t)size < sizeof(name));
    RillValue value = {};
    CHECK(rill_runtime_lookup(eval, name, &value));
    CHECK(value.kind == RILL_V_INT && value.as.integer == (int64_t)i);
  }
  rill_runtime_free(eval);
}
int main(int argc, char **argv) {
  CHECK(argc == 2);
  if (!strcmp(argv[1], "semantics")) {
    values_and_patterns();
    pattern_validation_order();
  } else if (!strcmp(argv[1], "code")) {
    constants();
    wide_operands();
    binding_snapshots();
  } else if (!strcmp(argv[1], "collection"))
    collection();
  else {
    CHECK(!strcmp(argv[1], "tail"));
    tail_calls();
  }
}
