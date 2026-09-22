# Language

Rill is a strict, dynamically typed functional language with explicit shell effects.
Functions, immutable data and reusable process plans share one expression language.
[Execution](execution.md) defines command arguments, redirection, jobs, streams and
I/O; [interaction](interaction.md) defines submission and presentation.

```rill
fn double value = value * 2
let total = [1, 2, 3] |> map double |> sum
print (string total)

let listing = plan { ^printf "%s\n" "first" "second" }
stream listing
  |> lines
  |> map { line => {name: line, selected: true} }
  |> collect
```

Evaluation is ordered and strict. Bindings are immutable and lexically scoped;
functions may perform effects. There is no implicit text conversion, truthiness,
laziness or evaluation of String contents as code.

## Source and statements

Source is UTF-8. Identifiers use ASCII `[A-Za-z_][A-Za-z0-9_]*`; `_` alone is reserved
for patterns. Strings and paths support Unicode without automatic normalization.
`#` starts a line comment outside strings; in command words it must occur at a word
boundary. There are no block comments or history expansions.

Keywords are `let`, `fn`, `rec`, `if`, `then`, `else`, `do`, `match`, `of`, `struct`,
`enum`, `with`, `plan`, `import`, `as`, `export`, `true`, `false`, `null`, `and`, `or`
and `not`. Library names such as `map`, `run` and `string` are ordinary bindings.

A newline or semicolon separates complete statements. Indentation has no meaning.
`do { statements }` and closure bodies introduce lexical blocks: statements execute
in order and the final expression supplies the result. An empty block or one ending
in a declaration returns Unit. A trailing semicolon does not discard the result.

```rill
let result = do {
  let base = 20
  base + 22
}
```

Blocks allow expressions, command statements, `let`, `fn` and recursive function
groups. Imports, exports, `struct` and `enum` require module or REPL top level.
There are no mutable variables, assignment, `return` or `break`; use expressions,
recursion and short-circuit sequence operations.

### Newlines and completeness

- Parentheses, list elements, record values, match arms, indexes and `$(...)` allow
  multiline expressions. Nested closure bodies and `do` blocks restore statement
  separation, even inside those contexts.
- An unfinished operator continues on the next line. A following line starting with
  `|>` also continues an expression when the source is already buffered. In the REPL,
  leave `|>` at line end to request continuation before submission.
- Lists, record/type fields, enum cases and match arms require commas and allow one
  trailing comma. Newlines do not replace commas. Function parameters use whitespace.
- A function value or partial application is complete input. `f` and `x` on separate
  lines are separate statements; parenthesize a multiline application.

The whole script, module or submitted entry is parsed and structurally validated
before any statement executes. Results are Complete, Incomplete or Invalid with byte
ranges; EOF makes Incomplete an error. Unused functions and unselected branches still
undergo structural validation. Name resolution, nominal identity and runtime values
are separate concerns. Parsing never evaluates code or consults callable arity.

## Values and literals

| Value    | Meaning                                                   |
| -------- | --------------------------------------------------------- |
| Unit     | `()`; successful operation without a data result          |
| Null     | `null`; explicit absence in data, including JSON          |
| Bool     | `true` or `false`; the only conditional values            |
| Int      | Signed 64-bit integer with checked arithmetic             |
| Float    | Finite IEEE binary64 value                                |
| String   | Valid UTF-8; may contain NUL                              |
| Bytes    | Arbitrary bytes; construct with `bytes [0, 255]`          |
| Path     | POSIX path bytes without NUL; construct with `path value` |
| List     | Immutable ordered values: `[1, "two", null]`              |
| Record   | Immutable unique String keys: `{name: "file", size: 42}`  |
| ADT      | Nominal constructor identity and immutable fields         |
| Function | First-class unary callable                                |
| JobPlan  | Immutable external execution description                  |
| Job      | Session-owned job handle                                  |
| Stream   | Scoped, single-consumer stream handle                     |

Handles expose no OS descriptor or pointer identity. Job handles may persist in session
data. Streams cannot escape their execution scope, including through containers or
captured environments; see [stream ownership](execution.md#stream-ownership-and-scopes).
Null, Unit, `Option.None` and an empty stream are distinct values.

Decimal integer literals allow internal `_` separators; leading zeros remain decimal.
Float literals contain a fractional part or exponent. Double-quoted strings recognize
`\n`, `\r`, `\t`, `\\`, `\"` and `\u{hex}`; escapes must denote Unicode scalars.
Single-quoted strings are literal; use double quotes to include a single quote.
Neither form interpolates. Both may span lines, preserving contents and indentation.
Adjacent literals do not concatenate implicitly.

### Arithmetic, order and equality

`+`, `-` and `*` require operands of the same numeric kind. `/` requires Floats;
`div a b` and `rem a b` operate on Ints, truncating toward zero with the dividend's
sign for the remainder. Overflow, division by zero and non-finite results raise errors.
Use `int` or `float` to cross numeric kinds. `+` also concatenates two Strings without
coercion; List and Bytes concatenation uses `concat`. There is no bit-shift syntax.
`sum` requires a homogeneous numeric sequence and returns Int zero for empty input.

Ordering accepts the same numeric kind or two Strings. Strings use Unicode scalar
lexicographic order, without locale collation. Other sorting requires an appropriate
key function. Lists use zero-based Int indexing; negative or out-of-range indexes fail.
Strings have no indexing syntax: named operations distinguish bytes and scalars.
Terminal display width is a presentation concern.

Equality is structural for ordinary data: records ignore field order, ADTs require the
same type and constructor, and Int and Float remain distinct kinds. Every leaf must
support equality. If either operand contains a Function, JobPlan or resource handle,
`==` and `!=` raise TypeError even when another field already differs. Results cannot
depend on traversal order or shared storage. There is no pointer-identity operator.

## Functions and application

```rill
fn multiply factor value = factor * value
let double = multiply 2
let operations = {transform: double, accept: { x => x > 10 }}

[3, 6, 9]
  |> map operations.transform
  |> filter operations.accept
```

Application uses whitespace and associates left-to-right: `f x y` means `(f x) y`.
Parentheses group expressions; `f(x)` and comma-separated call arguments are invalid.
Functions may be passed, returned, stored in data and partially applied. Closures,
native functions and constructors with fields use the same application protocol.
Field access supplies no implicit receiver; help metadata never affects dispatch.

Anonymous functions use `{ patterns => statements }`. There must be at least one
parameter. `{}` is an empty Record; `{ => ... }` is invalid. Unit is an explicit
argument: `{ () => body }` accepts Unit, `{ _ => body }` accepts anything, and an empty
body returns Unit. Bare `f` remains a value; `f ()` calls it with Unit. Nothing becomes
a thunk or invokes a returned function implicitly.

```rill
let load = { () => read_text "settings.json" }
let summarize = { entry =>
  let name = display_path entry.name
  {name, size: entry.size}
}
```

Parameters are whitespace-separated atomic patterns: bindings, literals, `_`, Unit,
lists, records, qualified fieldless constructors or parenthesized patterns. Group a
constructor with its payload: `{ (Point {x, y}) => x + y }`. A bare name followed by a
record pattern is two parameters. Record/closure classification uses header syntax,
ignoring comments and string contents; an undecided header remains Incomplete.

Multiple parameters behave as nested unary functions. In `f (a ()) (b ())`, evaluate
`f`, evaluate `a ()`, apply the first argument, evaluate `b ()`, then apply the second.
Each parameter pattern is checked as its argument arrives; the body runs after every
header parameter has arrived. Explicitly nested closures may perform effects between
applications. A later parameter may shadow an earlier parameter; each individual pattern
must bind unique names. Applying a non-function fails at that application. There are no variadic,
default or named-call arguments.

Library functions usually place configuration before data: `map f items`, `take n items`
and `starts_with prefix text`. Arithmetic keeps operand order: `subtract a b` is
`a - b`; use `{ x => x - 1 }` to subtract one. Binding pure configuration, such as
`let collect = collect_with {}`, does not perform the eventual operation; this is not
a general license to move effectful applications.

### Lexical scope and recursion

`let pattern = expression` evaluates once, matches atomically and installs the bindings
only on success. A failed match raises MatchError. The initializer cannot see the new
binding. Repeated names in a block/module fail; nested scopes and later REPL entries
may shadow names without changing existing closures.

`fn name patterns = expression` declares a self-recursive function. A simultaneous
`rec { fn ...; fn ... }` group contains one or more named functions. Arbitrary recursive
value initializers are not supported.

```rill
rec {
  fn even n = if n == 0 then true else odd (n - 1)
  fn odd n = if n == 0 then false else even (n - 1)
}

let count = rec { loop total n =>
  if n == 0 then total else loop (total + 1) (n - 1)
}
let from_ten = count 10
```

In a recursive function expression, the first identifier after `rec {` names the root
function; it is not a parameter. It is visible only inside that function and always
denotes the complete curried function, including from partial applications and nested
returned closures. `self` has no special meaning. A recursive expression and a group
of `fn` declarations are distinguished syntactically.

Closures retain resolved free bindings, not an entire scope or names to resolve later.
Recursive groups retain bindings needed by mutually reachable functions. An unrelated
local Stream does not prevent a closure from escaping.

Proper tail calls include direct, mutual and indirect calls and native higher-order
functions. Selected `if`/`match` branches and final block expressions inherit tail
position. Syntax-tree nesting is limited to 256 levels; live continuations to 65,536.
Exceeding a limit produces a diagnostic.

## Operators and control flow

`x |> f` evaluates `x`, then `f`, then applies `f` to the saved value. Pipes associate
left-to-right; `items |> map transform` applies the partial function `map transform`
to `items`. There are no implicit placeholders. Rewriting a pipe as `f x` would reverse
the evaluation order of its operands.

`if condition then yes else no` requires Bool and both branches, evaluating only the
selected branch. `and` and `or` short-circuit and require Bool; `not` negates Bool.
`each { item => effect item } items` sequences effects explicitly.

Precedence, tightest first:

| Level          | Forms                            |
| -------------- | -------------------------------- |
| Projection     | `value.field`, `value[index]`    |
| Application    | `f x`                            |
| Unary          | `-x`, `not x`                    |
| Multiplicative | `*`, `/`                         |
| Additive       | `+`, `-`                         |
| Comparison     | `==`, `!=`, `<`, `<=`, `>`, `>=` |
| Conjunction    | `and`                            |
| Disjunction    | `or`                             |
| Update         | `with`                           |
| Value pipe     | `\|>`                            |

Binary arithmetic and pipes associate left-to-right; comparisons do not chain.
Precedence is fixed. Parenthesize `if` and `match` as arguments; closures, records,
`do` and `plan` have explicit closing boundaries and may be passed directly.

Projection must touch its base: `items[0]` indexes, but `f [0]` passes a List.
`f items[0]` selects before calling; `(f items)[0]` indexes the result. `f -1` is
subtraction regardless of spacing; pass a negative argument as `f (-1)`.
`not p x` means `not (p x)`; `f (not x)` passes a negated Bool.

## Records and immutable updates

```rill
let entry = {name: "report.txt", size: 4096}
let larger = entry with {size: 8192}
let header = {"content-type": "application/json"}
header["content-type"]
```

Record fields evaluate in source order and must have unique keys. Identifier keys are
String literals; `{name}` abbreviates `{name: name}` with ordinary lexical lookup.
Quoted keys require a value. Computed field names use `record pairs`, whose `[key, value]`
pairs must also have unique String keys. Field order is retained for presentation.

`record.name` and `record["name"]` project data without getters, prototypes or method
binding. Missing fields raise MissingField; `lookup key record` returns Option for
expected absence. Nominal values support dot projection; String-key indexing and
`lookup` require anonymous Records. Nominal values remain distinct from
anonymous records in matching and equality.

`base with replacements` evaluates the base, then an expression producing an anonymous
Record. Every replacement key must exist. It returns a new value; shorthand fields
are allowed. `extend additions base` adds fields to an anonymous Record and rejects
collisions instead of silently replacing them.

## ADTs: products and sums

```rill
struct Point {x, y}
enum Outcome {Pending, Completed {value}, Failed {error}}

let point = Point {x: 3, y: 4}
let answer = Outcome.Completed {value: 42}
```

An anonymous Record is structural. `struct` declares a nominal product; `enum` declares
a closed sum of nominal products. Fields are dynamically typed; declarations specify
shape, not static types or business invariants.

- Nonempty constructors are unary functions accepting one anonymous Record with exactly
  the declared keys, in any input order. Declarations have ordered, unique field names.
- Fieldless constructors are singleton values; calling them raises TypeError.
  `struct Empty {}` binds a singleton. An enum name binds a constructor namespace.
- An enum requires at least one case, with unique case names. Omitting a payload and
  writing an empty payload both declare a fieldless case.
- Constructor aliases retain their descriptor. Two independently declared types remain
  distinct even when their names and fields coincide. Identity never comes from JSON
  or record keys.
- Types are declared at top level, not during function evaluation. A nominal declaration
  cannot reuse its name in the same module or REPL session. Module instances are cached.
- `with` preserves the constructor and accepts only existing fields. `to_record value`
  explicitly extracts the payload. Exporting a factory without its constructor does
  not seal field access or prevent updates.

Payloads may contain other ADTs without forward field annotations. Immutable construction
cannot create user-visible cyclic data graphs.

The shared prelude types are:

```rill
enum Option {None, Some {value}}
enum Result {Ok {value}, Err {error}}
enum Control {Continue {value}, Stop {value}}
```

`some x`, `ok x` and `err e` are convenience functions. Option and Result have no
implicit unwrapping or truthiness. Control supports early termination of folds.

## Pattern matching

```rill
match answer of {
  Outcome.Completed {value} if value > 0 => value,
  Outcome.Completed {value: 0} => 0,
  Outcome.Failed {error} => raise error,
  _ => -1
}
```

`match` evaluates the subject once, tries arms in order, completes structural matching,
then evaluates the guard. Guards require Bool. False tries the next arm; errors
propagate. Guard effects are not rolled back. Failed-arm bindings do not escape.
No match raises MatchError; exhaustiveness and redundant arms are not statically checked.

The same patterns serve `match`, `let` and function parameters:

| Pattern           | Meaning                                           |
| ----------------- | ------------------------------------------------- |
| `_`               | Ignore any value                                  |
| `name`            | Bind a new name, regardless of an outer binding   |
| Literal           | Match that literal value                          |
| `[a, b]`          | Exactly two elements                              |
| `[head, ..tail]`  | At least one element; bind the remaining List     |
| `{name, size}`    | Anonymous Record with exactly these keys          |
| `{name, ..}`      | Require `name`, ignore surplus fields             |
| `{name, ..rest}`  | Bind surplus fields as an anonymous Record        |
| `{name: n}`       | Match key `name`, bind `n`                        |
| `Point {x, ..}`   | Match this nominal constructor and declared field |
| `Outcome.Pending` | Match this fieldless singleton                    |

Patterns nest. One rest pattern is permitted, at the end. Record shorthand `name`
means `name: name`. Duplicate bindings/fields and misplaced rests fail before any
entry statement executes. `[x, x]` is invalid; use `[x, y] if x == y` in a match arm.
List rest views do not copy the full suffix on every recursive call.

Constructor paths resolve lexically to immutable descriptors; they are not arbitrary
expressions or user-defined extractors. Matching never invokes a constructor. A
non-constructor in descriptor position is an error. Match an empty nominal product as
`Empty {}`; bare `Empty` binds a name. Fieldless enum cases use qualified paths.
Nominal patterns reject undeclared keys; anonymous Record patterns do not match ADTs.
Use `to_record` for explicit structural matching.

Matching can allocate bindings but cannot consume a stream or invoke getters. There
are no pattern alternatives, implicit pinning, regex patterns or view patterns.

## Commands and plans

```rill
let message = "hello world"
^printf "%s\n" $message

let greeting = plan { ^printf "%s\n" $message | ^cat }
run greeting
```

`^` selects an external executable. Command words are literal; `$name` or `$(expression)`
contributes one String, Path or Bytes argument, and `...$name` or `...$(expression)`
spreads a List. No implicit splitting, globbing or interpolation occurs. The process
pipe `|` connects command stages; the value pipe `|>` applies functions.

A command is a statement. `plan { ... }` makes one external pipeline an expression:
arguments and redirect paths evaluate at construction, but described processes and
redirect files open at launch. Embedded expressions can themselves have effects.
Plans are reusable values; each launch gets a distinct job. Command statements use the
standard checked runner even if a local name shadows `run`.

Use `run` for checked foreground execution, `execute` for a foreground report,
`capture` for bytes and a report, `stream` for stdout Bytes, or `start` for a background
Job handle. Cross between process bytes and values explicitly. The complete grammar,
launch timing, status and cleanup contracts belong to [execution](execution.md).

## Errors and cancellation

Language errors propagate separately from ordinary values. They unwind resource
scopes; previous external effects remain. Failed entries publish no REPL bindings,
but a successful earlier `cd`, file write or process launch is not rolled back.

```rill
match attempt { () => read_text "settings.json" } of {
  Result.Ok {value} => value,
  Result.Err {error} => "unavailable"
}
```

`attempt thunk` calls the thunk with Unit and returns Result. It establishes a cleanup
checkpoint: failure closes resources created inside; success transfers their ownership
to the caller's scope. Previously existing resources consumed by a failed operation
are closed by that operation; unrelated resources remain usable. Returning `Result.Err`
as data does not raise an error or abort a pipeline.

`attempt` catches language errors, including TypeError, MatchError, arithmetic, I/O and
checked process failures. It cannot catch host cancellation or fatal runtime failure.
Ctrl+C follows a separate unforgeable control path, performs cleanup and returns to
the interactive boundary. Scripts report unhandled errors and exit unsuccessfully.

### Error data

The prelude's nominal `Error` product has these fields:

| Field         | Contract                                                          |
| ------------- | ----------------------------------------------------------------- |
| `kind`        | String error category                                             |
| `message`     | String explanation                                                |
| `span`        | Null or Record with `source`, original `text`, `offset`, `length` |
| `notes`       | List of Strings                                                   |
| `exit_status` | Null or Int in 1–255                                              |
| `details`     | Record of String or Int facts                                     |

A span's `source` and `text` are Strings; `offset` and `length` are nonnegative byte
counts within `text` and on UTF-8 boundaries. Generated details include `io_kind`,
available `os_code` for I/O and zero-based `stage` for checked process failures.
Facts are absent when the operation supplies none.

`error kind message` supplies absent span/status and empty notes/details. `raise`
validates Error fields and propagates the error. Catching and rethrowing preserves
source and exit status. Standard-library failures point to the nearest retained user
application when available, with a note identifying the library operation. Proper
tail calls do not retain a historical stack.

## Modules and standard library

```rill
import "./geometry.rill" as geometry
fn normalize value = string value
export {normalize, convert: normalize, Point: geometry.Point}
```

Imports use literal paths at top level. Relative paths resolve against the importing
file, or the entry's working directory for a REPL source. `std:` selects only bundled
modules. File modules must be regular files; directories, FIFOs and devices fail with
IOError. There is no network import, package registry, directory search or hot reload.

A source has at most one top-level export table. An omitted or empty table exports
nothing. Entries select existing bindings or namespace fields; aliases choose public
names without creating local bindings. Arbitrary expressions and duplicate export
keys are invalid. The table evaluates at its position and may precede later private
declarations. The resulting immutable namespace preserves function and nominal identity.

The loader caches resolved file identities for the interpreter's lifetime, rejects
cycles and removes failed loads. Importing a module being initialized by a stopped
evaluation raises ImportBusy; resume or cancel that evaluation before retrying.
Initializers execute once in source order and may perform effects. Imports are not a
sandbox. Bundled standard-library initialization is effect-free.

### Library organization

The embedded library is versioned with the executable. Rill modules own composition,
curried interfaces and exports; runtime primitives own representation access, checked
conversions, codecs and OS effects. The private bootstrap namespace is inaccessible
to user code. There is no dynamic native-code loader or second function dispatch path.

The prelude exposes common functions plus `core`, `seq`, `text`, `fs`, `process` and
`json` namespaces. `std:core` owns the shared Option/Result/Control/Error descriptors.
Configurable `_with` variants and specialized helpers use qualified names, including
`seq.produce`, `seq.CloseReason`, `text.byte_length`, `text.scalars` and `text.path_bytes`.
The predicates `starts_with` and `ends_with` remain common unqualified operations.
Import `std:option`, `std:result` and `std:test` explicitly. Missing language names never
fall back to PATH lookup.

[Execution](execution.md#core-library-contracts) owns sequence, file, process, stream and
JSON contracts and materialization limits. The pure value/text surface is summarized here:

| Operations                                               | Contract                                                                |
| -------------------------------------------------------- | ----------------------------------------------------------------------- |
| `identity`, `compose f g value`                          | Return the input; compose `g` then `f`                                  |
| `add`, `subtract`, `multiply`, `divide`, `equal`, `less` | Curried operator equivalents                                            |
| `int`, `float`                                           | Numeric conversion; Float-to-Int truncates toward zero and checks range |
| `string`                                                 | Explicit conversion of String, Bool, Int or Float to String             |
| `length`                                                 | List elements or anonymous Record fields                                |
| `concat`, `reverse`                                      | List/Bytes concatenation; List reversal                                 |
| `entries record`, `record pairs`                         | Ordered `[key, value]` pairs and reconstruction                         |
| `text.byte_length`                                       | Byte count of String, Bytes or Path                                     |
| `text.scalars`                                           | String to List of one-scalar Strings                                    |
| `encode_utf8`, `decode_utf8`                             | String/Bytes conversion; strict decoding                                |
| `starts_with`, `ends_with`, `contains`                   | Literal predicates, pattern before String                               |
| `split delimiter string`                                 | Nonempty literal delimiter; preserve empty fields and NUL               |
| `join separator strings`                                 | Join a List of Strings without coercion                                 |
| `replace pattern replacement string`                     | Replace literal occurrences, without regex interpretation               |
| `trim string`                                            | Trim ASCII space, HT, LF, CR, FF and VT                                 |
| `text.path_bytes path`                                   | Exact native Path bytes without display escaping                        |

An empty replacement pattern inserts at Unicode scalar boundaries and both ends.
`parse_int` and `parse_float` consume a whole String using JSON decimal syntax, without
trimming or locale conversion. Reject leading `+`, leading zeros, whitespace, trailing
junk, NUL, NaN and infinity. `parse_int` additionally rejects fraction/exponent forms.
Invalid syntax raises DecodeError; Int range or Float overflow raises ArithmeticError.
`parse_float` may round precision. Use `trim` explicitly for line-oriented data.

Both Option and Result modules export `map`, `bind`, `unwrap_or` and `unwrap_or_else`,
with the callback/default before the container. `map` transforms Some/Ok, preserving
None/Err. `bind` requires its callback to return the same nominal Option/Result type;
other returns raise TypeError. None/Err skips the callback and passes through unchanged.
`unwrap_or` takes an eager fallback value. `unwrap_or_else` calls its fallback only on
None/Err, passing Unit for None and the error payload for Err. These are ordinary Rill
functions, with no special call or exception syntax.

`help value` returns a String describing its kind or remaining parameters without
invoking it. Consecutive `##` lines immediately preceding a named function supply help
text; they remain ordinary source comments. Aliases and partial applications preserve
documentation. Anonymous functions still expose their parameters.

`std:test` exports `assert`, `assert_equal` and `assert_error` for ordinary Rill scripts.
Success returns Unit, failed assertions raise AssertionError and cancellation propagates.
There is no separate test syntax or implicit discovery/execution convention.
