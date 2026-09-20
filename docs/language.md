# Language

This document defines Rill syntax, values, and evaluation. [Execution](execution.md)
owns process and stream effects.

## Semantic core

The core consists of literals, lexical references, unary functions and application,
bindings and recursive function groups, sequencing, conditional choice, pattern
matching, data construction/projection, and runtime error propagation. Richer syntax
lowers to these operations or to explicit runtime primitives.

Evaluation is strict and ordered, bindings are immutable, and scope is lexical.
Functions may perform effects; purity is a programming discipline. Evaluation has no
implicit laziness, text coercion, truthiness, or string-to-code conversion.

## Values

| Kind          | Contract                                                               |
| ------------- | ---------------------------------------------------------------------- |
| Unit          | `()`; successful operations with no data result                        |
| Null          | `null`; explicit absence in data, including JSON                       |
| Bool          | `true` or `false`; the only accepted conditional values                |
| Int           | Signed 64-bit integer; checked arithmetic                              |
| Float         | IEEE binary64; finite results only                                     |
| String        | Valid UTF-8, explicit byte length, possibly containing NUL             |
| Bytes         | Arbitrary bytes with explicit length                                   |
| Path          | POSIX path bytes without NUL; no automatic Unicode normalization       |
| List          | Immutable, ordered sequence of values                                  |
| Record        | Immutable mapping from unique String keys to values                    |
| ADT value     | Nominal type identity, constructor identity, immutable payload         |
| Function      | First-class unary callable, including native and constructor functions |
| JobPlan       | Immutable external execution description                               |
| Job handle    | Opaque, session-owned reference to a live or completed job             |
| Stream handle | Opaque, scoped, single-consumer resource reference                     |

Resource handles do not expose OS handles. A Job handle may be retained in persistent
session data; a Stream handle cannot outlive its execution scope. Stream escape rules
also apply through lists, records, ADTs, and captured function environments.

Integer overflow and division by zero are language errors. There is no bit-shift syntax.
`/` operates on Float operands; `div` and `rem` are named Int
functions, with truncation toward zero and the dividend's sign for the remainder. Mixed
Int/Float arithmetic requires explicit conversion. Float overflow and non-finite
conversion results are errors. `+` also concatenates two Strings; it never converts
another value to String. List/Bytes concatenation uses named library operations.
Zero-element `sum` returns Int zero.

Ordering is defined for values of the same numeric kind and for Strings using Unicode
scalar lexicographic order, without locale collation. Other orderings require a key
function. String indexing is not provided; named operations distinguish bytes, Unicode
scalars, and terminal display cells. List indexing is zero-based, requires an Int, and
rejects negative or out-of-range indices.

Structural equality requires equality-capable values. Records ignore key order; ADTs
require the same type and constructor before comparing payloads. Numeric kinds remain
distinct. Unit, Null, Bool, String, Bytes, Path, List, Record, and ADT data support
equality. If either operand contains a Function, JobPlan, or resource handle, `==`
raises a TypeError, even when another field already differs. Thus equality must not
depend on traversal order. There is no public pointer-identity operator.

## Lexical conventions

- Source is UTF-8. Identifiers are ASCII `[A-Za-z_][A-Za-z0-9_]*`; `_` alone is
  reserved for patterns. Unicode remains fully supported in strings and paths.
- `#` starts a comment outside strings; in command mode it starts a comment at
  a word boundary. There are no block comments or history expansions.
- Decimal integer literals allow internal `_` separators. Float literals contain
  a fractional part or exponent. Leading zeros are decimal, not octal.
- Double-quoted strings support `\n`, `\r`, `\t`, `\\`, `\"`, and `\u{hex}`.
  Escapes must decode to Unicode scalar values. Single-quoted strings are literal;
  to include a single quote, use a double-quoted string. Neither form interpolates.
- Both string forms may span lines and preserve their contents without indentation
  stripping. There is no implicit concatenation of adjacent literals.
- Bytes are constructed by `bytes [0, 255]`; Paths by `path string_or_bytes`.
- Application uses whitespace between a function and each argument. It associates
  left-to-right: `f x y` means `(f x) y`. Parentheses group expressions; `f(x)` is
  invalid. There are no comma-separated call arguments or tuples.
- A newline or `;` separates complete statements. Parentheses, List elements,
  Record field values, match arms, indexes, and command substitutions permit multiline
  expressions. Closure bodies and `do` reset this context to statement separation,
  even inside an enclosing List or parentheses. Indentation has no semantic role.
- An unfinished operator continues on the next line. A following line beginning with
  `|>` also continues an expression while the source is buffered. In the REPL, leave
  `|>` at the end of a line to request continuation before submission.
- Lists, Record fields, type fields, enum cases and match arms use commas and allow
  one trailing comma. Newlines do not replace commas. Function parameters use whitespace.
- A bare function or partial application is Complete. Neither callable metadata nor
  a following line supplies implicit arguments. `f` followed by a newline and `x`
  is two statements; enclose a multiline application in parentheses.

Keywords are `let`, `fn`, `rec`, `if`, `then`, `else`, `do`, `match`, `struct`, `enum`,
`with`, `job`, `import`, `as`, `export`, `true`, `false`, `null`, `and`, `or`, `not`,
and `of`. Builtins such as `run` and `map` are ordinary lexical bindings.

## Functions and application

```rill
fn multiply factor value = factor * value
let double = multiply 2
let operations = {transform: double, accept: { x => x > 10 }}

[3, 6, 9] |> map operations.transform |> filter operations.accept
```

A Function is a first-class unary callable. User closures, native functions, and
constructors with fields share one application protocol. Functions can be passed,
returned, stored in data, and partially applied. Names and help metadata do not affect
dispatch. Field access supplies no implicit receiver.

Ordinary anonymous functions use `{ patterns => statements }`. Parameters are
whitespace-separated atomic patterns: literals, bindings, `_`, Unit, Lists, Records,
qualified fieldless constructors, or parenthesized patterns. Enclose a constructor and
its payload together: `{ (Point {x, y}) => x + y }`. A bare name followed by a Record
pattern denotes two parameters, not a constructor pattern.

```rill
let multiply = { factor value => factor * value }
let load = { () => read_text (path "settings.json") }
let ignore = { _ => () }
let summarize = { entry =>
  let name = display_path entry.name
  {name, size: entry.size}
}
```

A closure body is a lexical block with the same sequencing and result rules as `do`.
`{}` is an empty Record, not an empty closure. An empty closure body returns Unit; `{ ()
=> }` accepts only Unit, whereas `{ _ => }` accepts and ignores any value. A closure
must have at least one parameter; `{ => ... }` is invalid. No expression is implicitly
converted to a thunk. Record/closure classification depends only on header syntax;
comments and strings do not contribute header punctuation. An unfinished header stays
Incomplete until its syntax determines otherwise.

Multiple parameters lower to nested unary functions:

```text
{ p1 p2 => body }     = { p1 => { p2 => body } }
f x y                = (f x) y
record.operation x   = (record.operation) x
```

In `f (a ()) (b ())`, evaluate `f`, then `a ()`, apply the first argument, then `b ()`,
and apply the second argument. Each parameter pattern is checked when its argument
arrives. The body executes after all header parameters have arrived. Explicitly nested
closures may perform effects between applications; optimizations must preserve these
stages. Applying a non-function raises TypeError at that application. There is no
implicit receiver, variadic application, default parameter, named-call argument
mechanism, or automatic invocation of a returned function. `f ()` explicitly passes
Unit; bare `f` remains a value.

`fn name patterns = expression` declares a self-recursive function. `rec { fn ...; fn
... }` declares a simultaneous group. `let name = { ... }` is an ordinary nonrecursive
binding; the initializer does not gain access to its new name. Arbitrary recursive value
initializers are not supported.

A recursive function expression names its own root function:

```rill
let factorial = rec { loop n =>
  if n == 0 then 1 else n * loop (n - 1)
}
let count = rec { loop total n =>
  if n == 0 then total else loop (total + 1) (n - 1)
}
let from_ten = count 10
```

The first identifier after `rec {` is a local recursive binder, not a parameter. It is
visible only inside the function and denotes the complete curried function, including
from a partial application or a nested returned closure. It does not change when an
outer binding is shadowed. `self` is an ordinary identifier. Parameters, pattern checks,
captures, and tail calls use the same rules as every other function. A `rec` expression
and a mutual `rec { fn ... }` group are distinguished syntactically. A group contains
one or more named function declarations.

Closures capture resolved free bindings rather than an entire surrounding scope or names
to resolve later. An unrelated local Stream must not make a returned closure illegal.
Recursive groups retain bindings needed by mutually reachable functions. Repeated names
in one block/module are errors; nested scopes and later REPL entries may shadow bindings
without changing existing closures.

Library operations generally put configuration before primary data: `map f items`,
`filter predicate items`, `take count items`, and `starts_with prefix text`. Arithmetic
functions preserve operand order: `subtract a b` means `a - b`; use `{ x => x - 1 }` to
subtract one. Pure configuration binding can replace forwarding wrappers, as in `let
collect = collect_with {}`; this is not a general transformation for effectful partial
applications.

`x |> f` evaluates x first, then f, then applies f to the saved value. Pipes associate
left-to-right and have lower precedence than application, so `items |> map transform`
applies `map transform` to items. There are no implicit placeholders or callback
arguments. Literal substitution with `f x` would change effect order.

Proper tail calls cover direct, mutual, and indirect calls, including native
higher-order functions. The selected branch of a tail-position `if` or `match`, and the
final expression of a tail-position closure body or `do`, inherit tail position. Live
continuations are limited to 65,536; syntax-tree nesting is limited to 256 levels.
Exceeding either limit produces a diagnostic.

## Expressions, bindings, and effects

```rill
fn classify size = if size > 1_000_000 then "large" else "small"

fn summarize entries = do {
  let sizes = entries |> map { entry => entry.size }
  {count: length entries, bytes: sum sizes}
}
```

`do` evaluates statements in order and returns its final expression. An empty block, or
one ending in a declaration, returns Unit. A trailing semicolon does not discard the
final expression's value. `let pattern = expression` evaluates once, matches atomically,
then binds. Binding failure raises MatchError and installs none of that binding's names.

`if` requires both branches and evaluates only the selected branch. `and` and `or`
short-circuit and require Bool operands; `not` accepts Bool. Sequential effects and
`each { x => effect x } items` support imperative tasks. There are no mutable variables,
user-defined setters, `return`, `break`, or assignment operators. Closure bodies and
`do` admit expressions, commands, `let`, `fn` and `rec`; imports, exports and nominal
declarations require top level.

From tightest to loosest: field/index suffixes; application; unary `-` and `not`; `*`
and `/`; `+` and `-`; comparisons; `and`; `or`; `with`; `|>`. Comparisons do not chain.
Binary arithmetic and pipes associate left-to-right. Parenthesize `if` and `match` when
passing them as arguments. Closures, Records, `do`, and `job` have explicit closing
boundaries and may be passed directly. Operator precedence is fixed and cannot be
redefined. Infix numeric operations have named function equivalents; short-circuit
control remains syntax.

Field and index suffixes must touch their base: `items[0]` is indexing, whereas `f [0]`
passes a List. `f items[0]` applies f to the selected element; `(f items)[0]` indexes
the result. `f -1` is subtraction regardless of spacing; `f (-1)` passes a negative
argument. Application binds more tightly than prefix negation: `not p x` means `not (p
x)`, and `f (not x)` passes a negated Bool.

## Records and immutable updates

```rill
let entry = {name: "report.txt", size: 4096}
let larger = entry with {size: 8192}
let name = entry.name
let header = {"content-type": "application/json"}
header["content-type"]
```

Record fields evaluate in source order. Duplicate keys are rejected; expression syntax
does not permit computed field names, but `record pairs` constructs dynamic keys with
the same uniqueness rule. Identifier keys are String literals. A bare identifier field
`{name}` expands to `{name: name}` using ordinary lexical lookup; quoted keys require a
value. Missing field access raises MissingField. `lookup key record` returns Option when
absence is expected. Field order is preserved for deterministic presentation,
independently of equality.

`base with replacements` evaluates the base, then an ordinary expression producing an
anonymous Record, checks that all keys already exist, and returns a new value. Literal
replacement fields evaluate left-to-right; shorthand also applies, as in `base with
{size}`. It never changes `base`. Extending an anonymous record uses `extend additions
base`, which rejects collisions; replacement is never silently selected by an insertion
API.

Field access is data access: no getter, prototype lookup, or method binding runs.
Anonymous records and nominal records share field projection but remain distinct in
patterns and equality.

## ADTs: products and sums

```rill
struct Point {x, y}

enum Outcome {
  Pending,
  Completed {value},
  Failed {error}
}

let p = Point {x: 3, y: 4}
let answer = Outcome.Completed {value: 42}
let pending = Outcome.Pending
```

An anonymous Record is a structural product, identified by its fields. `struct` declares
a nominal product with its own type identity; `enum` declares a closed sum of such
products. Each constructor fixes ordered, unique field names. Nominal identity is
independent of JSON representations and diagnostic names.

- Fields are dynamically typed; declarations impose shape, not static field types.
- A nonempty constructor is a first-class unary function accepting exactly one
  anonymous record with exactly the declared keys. Input key order is irrelevant.
- A fieldless constructor is a singleton value. Calling it is a TypeError.
- `struct Empty {}` likewise binds a singleton; `struct Point {x, y}` binds its
  constructor function. An enum name binds an immutable constructor namespace.
- Constructor functions carry immutable descriptor metadata used by nominal
  patterns. Matching never calls the function or a user-defined extractor.
- Type declarations occur at module/REPL top level, not during ordinary function
  evaluation. A nominal declaration cannot be redefined under the same name in
  the same module or REPL session. Modules are instantiated once per interpreter.
- Two independently declared types remain distinct even if names and fields match.
- Recursive values can contain other ADT values without forward field annotations.
  Immutable construction does not create user-visible cyclic data graphs.
- `with` updates a product or a variant payload without changing its constructor;
  unknown fields are rejected. It does not enforce undeclared business invariants.

Pattern syntax names the descriptor, for example `Point {x, y}` or `Outcome.Completed
{value}`. The constructor path is lexically resolved and cannot be an arbitrary
expression. An alias to a constructor preserves its descriptor. A non-constructor in
this position is a diagnostic, not a function invocation. An empty nominal product is
matched as `Empty {}`; a bare identifier always binds a pattern variable. Fieldless enum
cases use a qualified path such as `Option.None`. An enum must declare at least one
case; case names are unique within that enum. Empty payload braces and an omitted
payload both declare a fieldless case.

Nominal identity is never inferred from record keys or JSON. `to_record value`
explicitly extracts the payload. Products and variants promise identity and shape, not
sealed representations: field projection and `with` remain available even when a module
exports a checked factory instead of its constructor.

The prelude supplies closed sums:

```rill
enum Option {None, Some {value}}
enum Result {Ok {value}, Err {error}}
enum Control {Continue {value}, Stop {value}}
```

`some x`, `ok x`, and `err e` are ordinary unary convenience functions. Null, Unit,
`Option.None`, and an empty stream are distinct. Neither Result nor Option has implicit
truthiness or implicit unwrapping.

## Pattern matching

```rill
match answer of {
  Outcome.Completed {value} if value > 0 => value,
  Outcome.Completed {value: 0} => 0,
  Outcome.Failed {error} => raise error,
  _ => -1
}
```

Patterns include literals, `_`, bindings, list patterns, record patterns, nominal
constructor patterns, and nesting. There are no view patterns, regex patterns, implicit
pinning of existing variables, user-defined matchers, or pattern alternatives. The same
pattern language serves `match`, `let`, and function parameters.

| Pattern           | Meaning                                                 |
| ----------------- | ------------------------------------------------------- |
| `name`            | Bind a fresh local name, regardless of an outer binding |
| `[a, b]`          | Exactly two elements                                    |
| `[head, ..tail]`  | At least one element; bind the remaining list           |
| `{name, size}`    | Anonymous record with exactly these keys                |
| `{name, ..}`      | Anonymous record containing `name`; ignore other keys   |
| `{name, ..rest}`  | Bind the remaining fields as an anonymous record        |
| `{name: n}`       | Match the key `name`, bind `n`                          |
| `Point {x, ..}`   | Point value containing the declared field `x`           |
| `Outcome.Pending` | The fieldless constructor's singleton                   |

Only one rest pattern is allowed, at the end. Shorthand `name` in a record pattern means
`name: name`. Nominal patterns reject keys not in their descriptor. Anonymous record
patterns do not match nominal data; project explicitly when needed.

Evaluate the subject once. Try branches in source order, completing structural matching
before evaluating the guard. Guards must be Bool. False tries the next branch; an error
propagates. Guard effects are not rolled back. Bindings from an unsuccessful branch do
not escape. If no branch matches, evaluation raises MatchError.

Names cannot occur twice in a pattern. Duplicate bindings/fields and misplaced rest
patterns are rejected before any statement in the entry executes, including in
unselected branches or unused functions. `[x, x]` is invalid; use `[x, y] if x == y`.
List rest views must not copy the full suffix on every recursive call. Matching can
allocate bindings but cannot read a stream, perform field getters, or execute
constructor code. Exhaustiveness and redundancy are not statically checked; an
unmatched value raises MatchError.

## Errors and cancellation

Language errors travel through a dedicated evaluator outcome, separate from ordinary
values. They carry stable kind, message, source span, and notes. The prelude exposes an
Error product with `kind: String`, `message: String`, `span: Record or Null`, and
`notes: List[String]`. Evaluator-generated spans contain `source: String` and a
zero-based byte `offset: Int`; `error kind message` supplies the default absent span and
empty notes. `raise` validates the Error shape before propagation. Type errors, failed
matches, invalid arithmetic, I/O errors, and checked process failures use this channel.

`attempt thunk` is a runtime primitive callable as an ordinary unary function:

```rill
match attempt { () => read_text (path "settings.json") } of {
  Result.Ok {value} => value,
  Result.Err {error} => "unavailable"
}
```

It calls the Unit-accepting thunk and converts a normal result or language error to
Result. It establishes a cleanup checkpoint: failure closes resources created inside the
thunk; success transfers their ownership to the caller's execution scope. Previously
existing resources consumed by a failed operation are closed by that operation, while
unrelated resources remain valid. It does not catch user cancellation or fatal runtime
failure. Returning `Result.Err` is ordinary data; it does not implicitly abort a
pipeline. Option is used for expected absence. This keeps matching data separate from
control transfer.

Errors unwind resource scopes. The REPL boundary reports an error and accepts the next
entry; the script boundary reports it and exits unsuccessfully. Earlier external effects
are not rolled back. REPL bindings from a failed entry are not published, but effects
such as successful `cd` remain. Ctrl-C uses the separate cancellation path, performs
cleanup, and returns to the interactive boundary.

## Modules and standard library

```rill
import "./geometry.rill" as geometry
let p = geometry.Point {x: 3, y: 4}
```

Imports are top-level declarations with literal paths. Relative paths resolve against
the importing source file, or the entry's working directory for a REPL source. `std:`
names resolve only to the bundled standard library. There is no package registry,
network import, automatic current-directory module search, or hot reload. File modules
must be regular files; directories, FIFOs, and devices are rejected with `IOError`.

A module exposes one explicit export table of existing bindings or namespace fields:

```rill
import "std:core" as core
fn normalize value = core.text value
export {normalize, convert: normalize, Option: core.Option}
```

An omitted or empty table exports nothing. The table is top-level, occurs at most once
per source, and is evaluated at its position; referenced bindings must already exist.
Aliases select public names without creating extra local bindings. Duplicate keys and
arbitrary expressions in table entries are syntax errors; exports use tables, not
declaration modifiers. A table can precede later private declarations. Its namespace is
an immutable Record; exports preserve function and nominal identities.

The loader caches by resolved file identity for the interpreter's lifetime, rejects
import cycles, and removes failed loads from the cache. Importing a module whose
initializer belongs to a stopped evaluation raises `ImportBusy`; resume or cancel that
evaluation before retrying. Top-level initializers execute once in source order and may
have ordinary effects; importing untrusted code is not a sandbox. Standard-library
initialization must be effect-free.

The standard-library source is embedded in and versioned with the executable. The
prelude is a fixed set of imports, not a second function dispatch path. The
library modules are `std:core`, `std:seq`, `std:text`, `std:fs`, `std:process`,
`std:json`, `std:option`, and `std:result`. Each module owns its definitions and
explicit imports; the prelude only selects public bindings. `std:core` owns the shared
Option/Result/Control/Error descriptors, so imports never manufacture replacement
identities. The prelude exports the names used in these documents; other helpers are
accessed through explicit module namespaces. It does not scan user directories for
commands or replace missing language names with PATH lookup. Private native primitives
are registered in a bootstrapping namespace, then wrapped or re-exported by library
modules. User modules cannot dynamically load native code.

Library signatures and materialization rules are in [execution](execution.md).
Functions, constructors, module exports, and callbacks all use the same application
machinery. There is no special command namespace for language functions.

### Pure library surface

The prelude exposes the core, sequence, text, filesystem, process, and JSON operations.
Option and Result combinators require explicit module imports to avoid name collisions.
These pure operations supplement the [sequence and process
contracts](execution.md#core-library-contracts).

| Operations                                               | Contract                                                                                                 |
| -------------------------------------------------------- | -------------------------------------------------------------------------------------------------------- |
| `identity`, `compose f g value`                          | Ordinary unary functions; composition applies `g` before `f`                                             |
| `add`, `subtract`, `multiply`, `divide`, `equal`, `less` | Curried equivalents of the corresponding operators                                                       |
| `int`, `float`                                           | Numeric conversion; Float-to-Int truncates toward zero and checks range                                  |
| `text`                                                   | Explicit String conversion for String, Bool, Int, and Float                                              |
| `length`                                                 | List elements or anonymous Record fields; text requires explicit units                                   |
| `byte_length`, `scalars`                                 | Encoded byte count; String to a List of one-scalar Strings                                               |
| `encode_utf8`, `decode_utf8`                             | String/Bytes conversion with strict UTF-8 validation                                                     |
| `starts_with`, `ends_with`                               | String prefix/suffix predicates, configuration first                                                     |
| `concat`, `reverse`                                      | Explicit List/Bytes concatenation; List reversal                                                         |
| `split delimiter string`                                 | Literal nonempty String delimiter; preserve empty fields and embedded NUL                                |
| `join separator strings`                                 | List of Strings to String; no implicit conversion                                                        |
| `trim string`                                            | Remove leading/trailing ASCII space, HT, LF, CR, FF, and VT; no locale dependence                        |
| `parse_int string`                                       | One signed 64-bit integer token using JSON decimal syntax; reject fractional/exponent forms and overflow |
| `parse_float string`                                     | One finite JSON decimal number, converted to Float; precision may round                                  |
| `entries record`                                         | List of `[key, value]` pairs in field order; reconstruct with `record`                                   |

Numeric parsers consume the whole String without trimming. They reject leading `+`,
leading zeroes, surrounding whitespace, trailing junk, NUL, NaN, and infinity. Use
`trim` explicitly for line-oriented input. Invalid syntax raises DecodeError; an integer
token outside Int range or a number overflowing Float raises ArithmeticError. Neither
parser performs locale-specific conversion.

Both `std:option` and `std:result` expose `map`, `bind`, `unwrap_or`, and
`unwrap_or_else`, with the callback/default before the wrapped value. `map` transforms
Some/Ok and preserves None/Err; `bind` returns the callback's result directly. The eager
`unwrap_or` accepts an ordinary fallback value. `unwrap_or_else` calls its fallback only
on None/Err, passing Unit for None and the error payload for Err. These are ordinary
Rill functions; they introduce no special call, exception, or short-circuit syntax.

Sequence callbacks and currying are authored in Rill. Runtime primitives handle checked
numeric conversion, representation access, stable sorting, and OS effects. The private
bootstrap namespace is unavailable to user code.

## Parsing contract

Parse the whole script, module or submitted entry before executing any statement. The
result is Complete, Incomplete or Invalid with byte ranges; EOF turns Incomplete into a
syntax error. Lexical modes depend only on syntax, never runtime bindings or parameter
counts. Parsing and lowering execute no user code.

Structural validation covers unused functions and unselected branches. Name, nominal
identity and value errors remain distinct from syntax errors. Lowering retains locations
for function bodies and patterns so diagnostics identify the failing application and its
original source. The editor uses this same completeness decision.
