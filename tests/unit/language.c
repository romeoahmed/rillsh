/**
 * @file
 * @brief Language semantics and retained-code behavior through the evaluator.
 *
 * Table cases cover values and errors; separate scenarios verify closure
 * lifetimes, collection, and proper tail calls. Meson owns process isolation
 * and deadlines.
 */
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
static void command_diagnostics() {
  const struct {
    const char *source;
    size_t stage, argument;
    bool has_argument;
  } cases[] = {
      {"job {^printf > out \"\\u{0}\"}", 0, 1, true},
      {"job {^printf > out ...$([\"ok\",\"\\u{0}\"])}", 0, 2, true},
      {"job {^printf ok | ^cat > out $(42)}", 1, 1, true},
      {"job {^printf > out ...$([\"ok\",42])}", 0, 2, true},
      {"job {^printf > \"\\u{0}\"}", 0, 0, false},
      {"job {^printf ok | ^cat > $(42)}", 1, 0, false},
      {"job {^\"\"}", 0, 0, true},
  };
  RillEval *eval = rill_runtime_new(nullptr, 0);
  CHECK(eval);
  rill_runtime_heap(eval)->stress = true;
  for (size_t i = 0; i < sizeof(cases) / sizeof(*cases); ++i) {
    RillEvalEvent event = evaluate(eval, cases[i].source, nullptr);
    CHECK(event.state == RILL_EVAL_ERROR && event.diagnostic.kind == RILL_TYPE);
    CHECK(event.diagnostic.stage == cases[i].stage);
    CHECK(event.diagnostic.has_argument == cases[i].has_argument);
    if (cases[i].has_argument)
      CHECK(event.diagnostic.argument == cases[i].argument);
  }
  CHECK(evaluate(eval, "job {^printf > out \"\"}", nullptr).state ==
        RILL_EVAL_DONE);
  rill_runtime_free(eval);
}
static void constants() {
  RillEval *eval = rill_runtime_new(nullptr, 0);
  CHECK(eval);
  RillHeap *heap = rill_runtime_heap(eval);
  heap->stress = true;
  expect(eval,
         "fn literal()=>\"a\\u{0}b\"; fn record(x)=>{key:x};"
         "let first=record(1); let second=record(2);"
         "if literal()==literal() and first.key==1 and second.key==2 "
         "then 1 else 0",
         1);
  expect(eval,
         "fn pooled()=>['',\"a\\u{0}b\",'key',\"a\\u{0}b\",''];"
         "let values=pooled(); let keyed={key:values[1]};"
         "if values==['',\"a\\u{0}b\",'key',keyed.key,''] "
         "and values[1]!=\"a\\u{0}c\" then 1 else 0",
         1);
  // A retained literal survives replacing its defining functions.
  RillEvalEvent event = evaluate(
      eval, "let saved=literal(); let literal=0; let record=0", nullptr);
  CHECK(event.state == RILL_EVAL_DONE);
  event = evaluate(eval, "saved", nullptr);
  CHECK(event.state == RILL_EVAL_DONE && event.value.kind == RILL_V_STRING);
  rill_runtime_collect(heap);
  CHECK(event.value.as.object->bytes.size == 3);
  CHECK(!memcmp(event.value.as.object->bytes.data, "a\0b", 3));
  rill_runtime_free(eval);
}

static void function_layouts() {
  RillEval *eval = rill_runtime_new(nullptr, 0);
  CHECK(eval);
  rill_runtime_heap(eval)->stress = true;
  [[gnu::cleanup(rill_text_clear)]] RillBuffer source = {};
  CHECK(rill_text_append(&source, "let functions=[", 15));
  for (size_t i = 0; i < 129; ++i)
    CHECK(
        rill_text_format(&source, "%sfn(x)=>fn(y)=>x+y+%zu", i ? "," : "", i));
  CHECK(rill_text_append(&source, "]; 0", 4));
  expect(eval, source.data, 0);
  expect(eval, "functions[0](1)(2)+functions[128](3)(4)", 138);
  // Distinct code owners use independent slots; old closures remain callable.
  expect(eval,
         "let saved=functions[64](10); let functions=0; "
         "let unrelated=fn()=>999; saved(2)",
         76);
  rill_runtime_collect(rill_runtime_heap(eval));
  expect(eval, "saved(3)+unrelated()", 1076);
  expect(eval,
         "let seed=7; let factory=fn(x)=>do {"
         "let shadow=x+1; fn(y)=>do {let x=y+2;"
         "fn(z)=>[seed,shadow,x,z]}};"
         "let first=factory(10)(20); let second=factory(30)(40);"
         "if first(1)==[7,11,22,1] and "
         "second(2)==[7,31,42,2] then 1 else 0",
         1);
  expect(eval,
         "fn recursive_factory(base)=>do {"
         "rec {fn even(n)=>if n==0 then base else odd(n-1);"
         "fn odd(n)=>if n==0 then base+1 else even(n-1)};"
         "fn(n)=>even(n)}; let recursive_saved=recursive_factory(40);"
         "recursive_saved(3)",
         41);
  expect(eval,
         "let seed=900; let factory=0; let recursive_factory=0; first(3)[2]",
         22);
  rill_runtime_collect(rill_runtime_heap(eval));
  expect(eval, "recursive_saved(4)+second(4)[1]", 71);
  rill_runtime_free(eval);
}

static void operators() {
  RillEval *eval = rill_runtime_new(nullptr, 0);
  CHECK(eval);
  rill_runtime_heap(eval)->stress = true;
  expect(eval,
         "if 2+3*4==14 and 8-3==5 and 8.0/2.0==4.0 and -(-7)==7 "
         "and 2<3 and 3>2 and 2<=2 and 3>=3 and 2!=3 "
         "and 'a'+'b'=='ab' and 'a'<'b' and not false then 1 else 0",
         1);
  expect(eval, "if (false and missing) or (true or missing) then 1 else 0", 1);
  const char *const cases[] = {
      "10-3-2==5 and 12.0/3.0/2.0==2.0",
      "(1<1)==false and (1>1)==false and 1<=1 and 1>=1",
      "(1.0<1.0)==false and 1.0<=1.0 and 2.0>1.0 and 2.0>=1.0",
      "''<'a' and 'a'<'aa' and 'aa'>'a' and 'a'<='a' and 'a'>='a'",
      "\"a\\u{0}b\"<\"a\\u{0}c\" and \"a\\u{0}b\"!='a'",
      "not (true and false) and (false or true) and not (false or false)",
  };
  for (size_t i = 0; i < sizeof(cases) / sizeof(*cases); ++i) {
    [[gnu::cleanup(rill_text_clear)]] RillBuffer source = {};
    CHECK(rill_text_format(&source, "if %s then 1 else 0", cases[i]));
    expect(eval, source.data, 1);
  }
  const RillNative native = {"effect", 42};
  rill_runtime_free(eval);
  eval = rill_runtime_new(&native, 1);
  CHECK(eval);
  RillEvalEvent event = evaluate(eval, "effect(1)+effect(2)", nullptr);
  for (int64_t i = 1; i <= 2; ++i) {
    CHECK(event.state == RILL_EVAL_NATIVE && event.native == 42);
    CHECK(event.value.kind == RILL_V_INT && event.value.as.integer == i);
    rill_runtime_collect(rill_runtime_heap(eval));
    rill_runtime_resume(eval, event.value, (RillDiagnostic){});
    do {
      event = rill_runtime_step(eval);
    } while (event.state == RILL_EVAL_YIELD);
  }
  CHECK(event.state == RILL_EVAL_DONE && event.value.kind == RILL_V_INT &&
        event.value.as.integer == 3);
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
  rill_runtime_collect(rill_runtime_heap(eval));
  expect(eval, "saved(5)", 9);
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
             {"-9223372036854775808-1", RILL_ARITHMETIC},
             {"0-(-9223372036854775808)", RILL_ARITHMETIC},
             {"9223372036854775807*2", RILL_ARITHMETIC},
             {"-9223372036854775808*(-1)", RILL_ARITHMETIC},
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
      "fn loop(n,acc)=>if n==0 then acc else loop(n-1,acc+1); "
      "loop(iterations,0)",
      "rec {fn even(n,acc)=>if n==0 then acc else odd(n-1,acc+1); fn "
      "odd(n,acc)=>if n==0 then acc else even(n-1,acc+1)}; even(iterations,0)",
      "fn bounce(f,x)=>f(x); fn loop(n)=>if n==0 then iterations else "
      "bounce(loop,n-1); loop(iterations)"};
  for (size_t i = 0; i < sizeof(cases) / sizeof(*cases); ++i) {
    size_t baseline_depth = 0, baseline_bytes = 0;
    const size_t counts[] = {1000, 1000000};
    for (size_t run = 0; run < 2; ++run) {
      RillEval *eval = rill_runtime_new(nullptr, 0);
      CHECK(eval);
      [[gnu::cleanup(rill_text_clear)]] RillBuffer source = {};
      CHECK(rill_text_format(&source, "let iterations=%zu; %s", counts[run],
                             cases[i]));
      size_t depth = 0;
      RillEvalEvent event = evaluate(eval, source.data, &depth);
      CHECK(event.state == RILL_EVAL_DONE && event.value.kind == RILL_V_INT &&
            event.value.as.integer == (int64_t)counts[run]);
      rill_runtime_collect(rill_runtime_heap(eval));
      size_t bytes = rill_runtime_heap(eval)->bytes;
      if (!run) {
        baseline_depth = depth;
        baseline_bytes = bytes;
      } else {
        // A thousandfold increase in calls must not grow continuation depth.
        CHECK(depth <= baseline_depth);
        CHECK(bytes <= 2 * baseline_bytes);
      }
      rill_runtime_free(eval);
    }
  }
}
static void collection() {
  RillEval *eval = rill_runtime_new(nullptr, 0);
  CHECK(eval);
  rill_runtime_heap(eval)->stress = true;
  expect(eval, "fn f(n)=>if n==0 then 9 else f(n-1); f(1000)", 9);
  expect(eval,
         "fn churn(n,temporary)=>if n==0 then 9 else "
         "churn(n-1,[n,{value:n},fn()=>n]); churn(100,[])",
         9);
  rill_runtime_collect(rill_runtime_heap(eval));
  size_t retained = rill_runtime_heap(eval)->bytes;
  expect(eval, "churn(2000,[])", 9);
  rill_runtime_collect(rill_runtime_heap(eval));
  CHECK(rill_runtime_heap(eval)->bytes <= 2 * retained);
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
  size_t retained = 0;
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
    rill_runtime_collect(heap);
    if (!pass)
      retained = heap->bytes;
    else
      CHECK(heap->bytes <= 2 * retained);
  }
  rill_runtime_collect(heap);
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
    command_diagnostics();
    operators();
  } else if (!strcmp(argv[1], "code")) {
    function_layouts();
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
