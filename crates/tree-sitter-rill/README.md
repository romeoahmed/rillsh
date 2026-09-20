# Tree-sitter Rill

Incremental syntax trees and editor queries for Rill Shell. The execution parser and
REPL completeness checks remain in `rill-syntax`.

The Rust binding exposes `LANGUAGE`, `NODE_TYPES`, `HIGHLIGHTS_QUERY` and
`LOCALS_QUERY`. Building the package compiles the checked-in parser and its stateless
scanner through `cc`; it needs neither Node.js nor the Tree-sitter CLI.

## Source and generated files

Maintain `grammar.js`, the external `src/scanner.c`, `queries/`, tests and
package/binding configuration by hand. The stateless scanner is necessary for command
words and context-sensitive whitespace; it is not generated from the grammar.

`tree-sitter generate` owns `src/parser.c`, `src/grammar.json`, `src/node-types.json`
and the headers under `src/tree_sitter/`. Never patch these outputs. Fix the grammar or
scanner, regenerate, and review the resulting diff. The bindings are initially
scaffolded by `tree-sitter init`; ordinary generation does not rewrite them.

## Regenerate and verify

Run inside this package with Tree-sitter CLI 0.27.0:

```sh
cargo install tree-sitter-cli --version 0.27.0 --locked
tree-sitter generate --js-runtime native
tree-sitter test
```

The native JavaScript runtime is bundled with the CLI. Corpus cases assert tree
structure; Rust tests additionally cover standard-library parsing, query captures,
meaningful newlines, and incremental edits against fresh parsing. Run them from the
repository with `cargo test -p tree-sitter-rill --locked`.

The scanner recognizes command words and distinguishes horizontal application gaps from
permitted expression and pipeline continuation. It retains no state between calls, so
its serialized state is empty. Newlines inside blocks continue to delimit statements
even when the block is nested in a list or grouping expression.
