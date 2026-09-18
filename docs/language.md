# Language

This document specifies the first-release target language. [Execution](execution.md)
defines process and stream effects. The non-Stream language is implemented; Stream
lifetimes and cleanup scopes below belong to stage 3. [Current status](status.md)
records coverage.

## Semantic core

The core consists of literals, lexical references, unary functions and application,
bindings and recursive function groups, sequencing, conditional choice, pattern
matching, data construction/projection, and runtime error propagation. Richer syntax
lowers to these operations or to explicit runtime primitives.

Evaluation is strict and ordered, bindings are immutable, and scope is lexical.
Functions may perform effects; purity is a programming discipline. Evaluation has no
implicit laziness, text coercion, truthiness, or string-to-code conversion.

## Values

| Kind | Contract |
| --- | --- |
| Unit | `()`; successful operations with no data result |
| Null | `null`; explicit absence in data, including JSON |
| Bool | `true` or `false`; the only accepted conditional values |
| Int | Signed 64-bit integer; checked arithmetic |
| Float | IEEE binary64; finite results only |
| String | Valid UTF-8, explicit byte length, possibly containing NUL |
| Bytes | Arbitrary bytes with explicit length |
| Path | POSIX path bytes without NUL; no automatic Unicode normalization |
| List | Immutable, ordered sequence of values |
| Record | Immutable mapping from unique String keys to values |
| ADT value | Nominal type identity, constructor identity, immutable payload |
| Function | First-class unary callable, including native and constructor functions |
| JobPlan | Immutable external execution description |
| Job handle | Opaque, session-owned reference to a live or completed job |
| Stream handle | Opaque, scoped, single-consumer resource reference |

Resource handles do not expose OS handles. A Job handle may be retained in persistent
session data; a Stream handle cannot outlive its execution scope. Stream escape rules
also apply through lists, records, ADTs, and captured function environments.

Integer overflow and division by zero are language errors. There is no bit-shift syntax
in the initial language. `/` operates on Float operands; `div` and `rem` are named Int
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
- Bytes are constructed by `bytes([0, 255])`; Paths by `path(string_or_bytes)`.
- A newline or `;` separates statements when the preceding expression is complete.
  Newlines inside lists, records, argument lists, or unfinished operators do not
  terminate them. A following line beginning with `|>` continues an expression;
  the script parser uses this lookahead. In the REPL use a trailing `|>` or explicit
  multiline editing when the previous complete line has already been submitted.
- A completed expression cannot gain an ordinary argument merely because the next
  line begins with `(`. No automatic semicolon insertion beyond these rules occurs.

Keywords are `let`, `fn`, `rec`, `if`, `then`, `else`, `do`, `match`, `struct`, `enum`,
`with`, `job`, `import`, `as`, `export`, `true`, `false`, `null`, `and`, `or`, and
`not`. Builtins such as `run` and `map` are ordinary lexical bindings.

## Functions and application

```rill
fn multiply(factor, value) => factor * value
let double = multiply(2)
let operations = {transform: double, accept: fn(x) => x > 10}

[3, 6, 9] |> map(operations.transform) |> filter(operations.accept)
```

A Function is a first-class unary callable. It can be passed, returned, stored in data,
or created as a lexical closure, independently of the names bound to it. User functions,
native functions, and constructors with fields share one application protocol. Planned
help metadata describes callables without affecting dispatch.

Application follows these lowerings:

```text
fn(p1, p2) => e     = fn(p1) => fn(p2) => e
f(a, b)            = (f(a))(b)
f()                = f(())
fn() => e          = fn(()) => e
record.operation(x) = (record.operation)(x)
```

There is no implicit receiver, variadic application, parameter default, named-call
argument mechanism, or automatic invocation of a returned function. A returned function
can receive further arguments; applying a non-function raises TypeError. In `f(a(),
b())`, evaluate `f`, evaluate `a()`, apply the first argument, evaluate `b()`, then
apply the second argument. Intermediate applications may have effects. Optimizations
cannot evaluate the second argument before the first application.

Each parameter pattern is checked when that parameter is applied. A pattern failure is
immediate even if later curried parameters have not arrived.

Lexical closures capture their resolved free bindings, not an entire surrounding scope
or names to resolve later. This is observable: an unrelated local Stream must not make a
returned closure illegal. Recursive groups retain the bindings needed by their mutually
reachable functions. Immutable values may be shared. Named `fn` declarations bind
themselves recursively. `rec { fn ...; fn ... }` introduces a simultaneous group of
function declarations; arbitrary recursive value initializers are not supported.
Repeated names in one block/module are errors. Each REPL entry introduces a new scope,
so later entries may shadow old bindings without changing existing closures.

Configuration parameters precede the primary data. For example `map(f, items)`,
`filter(predicate, items)`, `take(count, items)`, and `starts_with(prefix, text)`. `x |>
f` evaluates `x` first, then `f`, then applies `f` to the saved value. Its lowering uses
hygienic temporaries; rewriting it literally to `f(x)` would change effect order. Pipes
associate left-to-right.

Proper tail calls cover direct, mutual, and indirect calls, including calls routed
through native higher-order functions. The chosen branch of a tail-position `if` or
`match`, and the final expression of a tail-position `do`, inherit tail position. A call
is not tail-positioned when work remains afterward. No C optimizer is required for this
guarantee. The evaluator limits live continuations to 65,536; syntax nesting is limited
to 256 levels. Exceeding either limit produces a diagnostic.

## Expressions, bindings, and effects

```rill
fn classify(size) => if size > 1_000_000 then "large" else "small"

fn summarize(entries) => do {
  let sizes = entries |> map(fn(entry) => entry.size)
  {count: length(entries), bytes: sum(sizes)}
}
```

`do` evaluates statements in order and returns its final expression. An empty block, or
one ending in a declaration, returns Unit. A trailing semicolon does not discard the
final expression's value. `let pattern = expression` evaluates once, matches atomically,
then binds. Binding failure raises MatchError and installs none of that binding's names.

`if` requires both branches and evaluates only the selected branch. `and` and `or`
short-circuit and require Bool operands; `not` accepts Bool. Sequential effects and
`each(fn(x) => effect(x), items)` support imperative tasks. There are no mutable
variables, user-defined setters, `return`, `break`, or assignment operators in the first
release.

From tightest to loosest: field/index/call suffixes; unary `-` and `not`; `*` and `/`;
`+` and `-`; comparisons; `and`; `or`; `with`; `|>`. Comparisons do not chain. Binary
arithmetic and pipes associate left-to-right. `fn`, `if`, `match`, and `do` are
expression forms; parentheses delimit a larger operand when needed. Operator precedence
is fixed and cannot be redefined. Infix numeric operations have named function
equivalents; short-circuit control remains syntax.

## Records and immutable updates

```rill
let entry = {name: "report.txt", size: 4096}
let larger = entry with {size: 8192}
let name = entry.name
let header = {"content-type": "application/json"}
header["content-type"]
```

Record fields evaluate in source order. Duplicate keys are rejected; expression syntax
does not permit computed field names, but `record(pairs)` constructs dynamic keys with
the same uniqueness rule. Identifier keys are String literals. Missing field access
raises MissingField. `lookup(key, record)` returns Option when absence is expected.
Field order is preserved for deterministic presentation, independently of equality.

`base with {field: expression}` evaluates the base, then replacement expressions
left-to-right, checks that all keys already exist, and returns a new value. It never
changes `base`. Extending an anonymous record uses `extend(additions, base)`, which
rejects collisions; replacement is never silently selected by an insertion API.

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

let p = Point({x: 3, y: 4})
let answer = Outcome.Completed({value: 42})
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
case; case names are unique within that enum.

Nominal identity is never inferred from record keys or JSON. `to_record(value)`
explicitly extracts the payload. Products and variants promise identity and shape, not
sealed representations: field projection and `with` remain available even when a module
exports a checked factory instead of its constructor.

The prelude supplies closed sums:

```rill
enum Option {None, Some {value}}
enum Result {Ok {value}, Err {error}}
```

`some(x)`, `ok(x)`, and `err(e)` are ordinary unary convenience functions. Null, Unit,
`Option.None`, and an empty stream are distinct. Neither Result nor Option has implicit
truthiness or implicit unwrapping.

## Pattern matching

```rill
match answer {
  Outcome.Completed {value} if value > 0 => value,
  Outcome.Completed {value: 0} => 0,
  Outcome.Failed {error} => raise(error),
  _ => -1
}
```

Patterns include literals, `_`, bindings, list patterns, record patterns, nominal
constructor patterns, and nesting. There are no view patterns, regex patterns, implicit
pinning of existing variables, user-defined matchers, or pattern alternatives. The same
pattern language serves `match`, `let`, and function parameters.

| Pattern | Meaning |
| --- | --- |
| `name` | Bind a fresh local name, regardless of an outer binding |
| `[a, b]` | Exactly two elements |
| `[head, ..tail]` | At least one element; bind the remaining list |
| `{name, size}` | Anonymous record with exactly these keys |
| `{name, ..}` | Anonymous record containing `name`; ignore other keys |
| `{name, ..rest}` | Bind the remaining fields as an anonymous record |
| `{name: n}` | Match the key `name`, bind `n` |
| `Point {x, ..}` | Point value containing the declared field `x` |
| `Outcome.Pending` | The fieldless constructor's singleton |

Only one rest pattern is allowed, at the end. Shorthand `name` in a record pattern means
`name: name`. Nominal patterns reject keys not in their descriptor. Anonymous record
patterns do not match nominal data; project explicitly when needed.

Evaluate the subject once. Try branches in source order, completing structural matching
before evaluating the guard. Guards must be Bool. False tries the next branch; an error
propagates. Guard effects are not rolled back. Bindings from an unsuccessful branch do
not escape. If no branch matches, evaluation raises MatchError.

Names cannot occur twice in a pattern. `[x, x]` is invalid; use `[x, y] if x == y`. List
rest views must not copy the full suffix on every recursive call. Matching can allocate
bindings but cannot read a stream, perform field getters, or execute constructor code.
No exhaustiveness guarantee is made for dynamic matches. Diagnose provably redundant
arms when possible; a potentially incomplete match still has the runtime MatchError
contract. Warning analysis must not evaluate guards or constructor expressions.

## Errors and cancellation

Language errors travel through a dedicated evaluator outcome, separate from ordinary
values. They carry stable kind, message, source span, and notes. The prelude exposes an
Error product with `kind: String`, `message: String`, `span: Record or Null`, and
`notes: List[String]`. Evaluator-generated spans contain `source: String` and a
zero-based byte `offset: Int`; `error(kind, message)` supplies the default absent span
and empty notes. `raise` validates the Error shape before propagation. Type errors,
failed matches, invalid arithmetic, I/O errors, and checked process failures use this
channel.

`attempt(thunk)` is a runtime primitive callable as an ordinary unary function:

```rill
match attempt(fn() => read_text(path("settings.json"))) {
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
let p = geometry.Point({x: 3, y: 4})
```

Imports are top-level declarations with literal paths. Relative paths resolve against
the importing source file, or the entry's working directory for a REPL source. `std:`
names resolve only to the bundled standard library. There is no package registry,
network import, automatic current-directory module search, or hot reload.

A module exports declarations explicitly with `export let`, `export fn`, `export
struct`, or `export enum`. The resulting namespace is an immutable record of exports.
The loader caches by resolved file identity for the interpreter's lifetime, rejects
import cycles, and removes failed loads from the cache. Top-level initializers execute
once in source order and may have ordinary effects; importing untrusted code is not a
sandbox. Standard-library initialization must be effect-free.

The standard-library source is embedded in and versioned with the executable. The
prelude is a fixed set of imports, not a second function dispatch path. The initial
library modules are `std:core`, `std:seq`, `std:text`, `std:fs`, `std:process`, and
`std:json`. The prelude exports the names used in these documents; other helpers are
accessed through explicit module namespaces. It does not scan user directories for
commands or replace missing language names with PATH lookup. Private native primitives
are registered in a bootstrapping namespace, then wrapped or re-exported by library
modules. User modules cannot dynamically load native code.

Library signatures and materialization rules are in [execution](execution.md).
Functions, constructors, module exports, and callbacks all use the same application
machinery. There is no special command namespace for language functions.

### Pure library surface

`std:core`, `std:seq`, `std:text`, and `std:process` are bundled modules. The prelude
also exposes their public operations directly. `std:fs` and `std:json` are planned for
stage 3. The table below supplements the sequence/process contracts in
[execution](execution.md#core-library-contracts).

| Operations | Contract |
| --- | --- |
| `identity`, `compose(f, g, value)` | Ordinary unary functions; composition applies `g` before `f` |
| `add`, `subtract`, `multiply`, `divide`, `equal`, `less` | Curried equivalents of the corresponding operators |
| `int`, `float` | Numeric conversion; Float-to-Int truncates toward zero and checks range |
| `text` | Explicit String conversion for String, Bool, Int, and Float |
| `length` | List elements or anonymous Record fields; text requires explicit units |
| `byte_length`, `scalars` | Encoded byte count; String to a List of one-scalar Strings |
| `encode_utf8`, `decode_utf8` | String/Bytes conversion with strict UTF-8 validation |
| `starts_with`, `ends_with` | String prefix/suffix predicates, configuration first |
| `concat`, `reverse`, `drop` | Explicit List/Bytes concatenation; List reversal and shared suffix slicing |

Sequence callbacks and currying are authored in Rill. C primitives handle checked
numeric conversion, representation access, stable sorting, and OS effects. The private
bootstrap namespace is unavailable to user code.

## Parsing contract

The parser returns Complete, Incomplete, or Invalid with source ranges. The lexer has
explicit expression and command modes; these modes are selected by syntax, never by
whether a name happens to exist at runtime. Closure construction resolves free bindings
to fixed captures, including constructor paths used in patterns. Parsing does not need
those values to classify source completeness.

Function bodies and patterns retain source spans after lowering. A script or module is
parsed completely before its statements execute; the REPL parses one submitted entry
completely. EOF converts Incomplete to a syntax error. Parsing and lowering never
execute user code; runtime name/type errors remain distinct from syntax errors.

## Syntax boundaries

The forms above, fixed precedence, and command grammar define the surface language.
Whitespace never implies application. `(a, b)` is not a tuple; `()` is Unit and an empty
call supplies Unit. Parentheses may group expressions or patterns, and negative numeric
literals are allowed in patterns.

Comma-separated lists, arguments, parameters, fields, enum cases, and match arms allow
one trailing comma. Newlines within those forms are whitespace, not commas. A record
expression always uses `key: value`; field shorthand belongs to patterns. `with` takes a
record of replacements. An enum case with empty payload braces or no payload is
fieldless; the enum itself must contain at least one case.

Local `do` blocks admit expressions, command statements, `let`, named `fn`, and `rec`.
Imports, exports, and nominal declarations are top-level forms. A `rec` group contains
one or more named function declarations. A `job` form contains exactly one external
pipeline, with surrounding whitespace; it is not a general statement block.
