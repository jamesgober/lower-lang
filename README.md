<h1 align="center">
    <img width="99" alt="Rust logo" src="https://raw.githubusercontent.com/jamesgober/rust-collection/72baabd71f00e14aa9184efcb16fa3deddda3a0a/assets/rust-logo.svg">
    <br>
    <b>lower-lang</b>
    <br>
    <sub><sup>SYNTAX TREE TO HIR LOWERING</sup></sub>
</h1>

<div align="center">
    <a href="https://crates.io/crates/lower-lang"><img alt="Crates.io" src="https://img.shields.io/crates/v/lower-lang"></a>
    <a href="https://crates.io/crates/lower-lang"><img alt="Downloads" src="https://img.shields.io/crates/d/lower-lang?color=%230099ff"></a>
    <a href="https://docs.rs/lower-lang"><img alt="docs.rs" src="https://img.shields.io/docsrs/lower-lang"></a>
    <a href="https://github.com/jamesgober/lower-lang/actions"><img alt="CI" src="https://github.com/jamesgober/lower-lang/actions/workflows/ci.yml/badge.svg"></a>
    <a href="https://github.com/rust-lang/rfcs/blob/master/text/2495-min-rust-version.md"><img alt="MSRV" src="https://img.shields.io/badge/MSRV-1.85%2B-blue"></a>
</div>

<br>

<div align="left">
    <p>
        <strong>lower-lang</strong> turns a lossless syntax tree into HIR. A language describes what its constructs <em>mean</em> as data &mdash; a mapping from node kinds to a small catalogue of semantic <strong>archetypes</strong> (binders, scopes, declarations, control, expressions, patterns, types), operator tables with their overflow and division policies, and hygienic <strong>templates</strong> for sugar such as <code>while</code>, <code>for-each</code>, <code>&amp;&amp;</code>, and <code>??</code> &mdash; and lower-lang builds the HIR, records where every node came from, and validates the result with <a href="https://crates.io/crates/hir-lang"><code>hir-lang</code></a>.
    </p>
    <p>
        It is the L3 stage of LexerSketch: declaring a language's semantics instead of hand-writing its compiler is what lets a sketch become a running language. Lowering is generic over the tree's kind type, so any <a href="https://crates.io/crates/syntax-lang"><code>syntax-lang</code></a> tree lowers &mdash; one forged by <a href="https://crates.io/crates/lang-forge"><code>lang-forge</code></a>, or one a hand-written parser built. Half-typed code is normal input: every broken construct becomes an HIR error node and a problem with its source span, and the rest of the file is still lowered.
    </p>
    <br>
    <hr>
    <p>
        <strong>MSRV is 1.85+</strong> (Rust 2024 edition). <code>no_std</code>-compatible (needs only <code>alloc</code>), <code>#![forbid(unsafe_code)]</code>, three dependencies, all from the family: <a href="https://crates.io/crates/syntax-lang"><code>syntax-lang</code></a> 1, <a href="https://crates.io/crates/hir-lang"><code>hir-lang</code></a> 0.3, and <a href="https://crates.io/crates/intern-lang"><code>intern-lang</code></a> 1.
    </p>
    <blockquote>
        <strong>Status: 0.2.0 (Foundation).</strong> The archetype catalogue, the mapping model, programmatic hygienic templates, budgets, origins, and lenient validation are implemented. Field labels from LSF2 sketches are supported through a <code>Labeler</code> hook (lang-forge 1.x trees have none yet, so mappings address children by kind and position), and textual templates arrive with hir-lang's textual syntax. The API is frozen at 1.0, after the LexerSketch app has lowered at least three target languages through it. See <a href="./CHANGELOG.md"><code>CHANGELOG.md</code></a> and the <a href="./dev/ROADMAP.md"><code>ROADMAP</code></a>.
    </blockquote>
</div>

<hr>
<br>

## The model

- A **[`Mapping`](./docs/API.md#mapping)** is a language's semantics as data, built with a [`MappingBuilder`](./docs/API.md#mappingbuilder). It mirrors LexerSketch's LSF2 `[semantics]` sections: node kinds to **[archetypes](./docs/API.md#arch)** (`[semantics.bind]`), **[templates](./docs/API.md#template)** (`[semantics.lower]`), and **[operators](./docs/API.md#opspec)** with their policies (`[semantics.ops]`), plus the token roles, modifiers (`mut`, `async`, `&`, `...`), and path roots the archetypes read.
- An **[`Arch`](./docs/API.md#arch)** says what a node means and where its parts are: each field is a **[`Pick`](./docs/API.md#pick)** &mdash; a child by host field label, or by position or kind for trees without labels.
- A **[`Template`](./docs/API.md#template)** is a small HIR-shaped AST with `${field}` holes, built with a [`TemplateBuilder`](./docs/API.md#templatebuilder). Every instantiation records its own HIR expansion; the binders it introduces carry that expansion's hygiene mark. [`templates`](./docs/API.md#templates) has the common ones: `while`, `do-while`, C `for`, `for-each`, `&&`, `||`, `??`, `?`, `x++`.
- **[`lower`](./docs/API.md#lower)** is the one-call entry point; [`Lowerer`](./docs/API.md#lowerer) adds a [`Budget`](./docs/API.md#budget), a compilation unit, and field labels. The result is a **[`Lowered`](./docs/API.md#lowered)**: a valid `hir_lang::Hir` and every **[`Problem`](./docs/API.md#problem)** in source order.

<br>

What it guarantees, and how each guarantee is checked:

| Guarantee | How it is held |
|---|---|
| Lowering never panics and always yields valid HIR, on any tree. | Property tests lower 512 random trees per run (random kinds and shapes over a mapping with most archetypes, error nodes, unmapped kinds, every template, two language policies) and 512 trees with spans outside the source or off character boundaries; every result re-validates. 256 random token soups per run, parsed by lang-forge into deep half-typed trees, lower the same way. Every prefix of a program (typing it one character at a time) lowers. |
| Every node and binder has an origin inside the source, through template expansions. | Checked on every result of every property test and integration test: each node's span lies in the source and each expansion it names exists. |
| Templates are hygienic: their names never capture or are captured by user names. | Property tests run a resolver (`lookup_local` + `resolve`) over every random result and check that every resolved reference names a binder with its own hygiene mark; integration tests put user variables named like the templates' temporaries (`it`, `t`, `old`) next to `for-each`, `??`, and `x++`. |
| Lowering preserves meaning. | A property test generates random calc programs (assignments, arithmetic, comparisons, `&&`/`||`, `if`/`else`, counted `while` loops), evaluates them directly, renders them as text, parses them with lang-forge, lowers them, and evaluates the HIR: the printed values agree, 256 programs per run. |
| Lowering is deterministic. | Every random tree is lowered twice with fresh interners; the printed HIR with origins, the binders, and the problems are identical. |
| Budgets stop runaway work. | A node budget and a template-application depth budget; property tests check that a too-small node budget is an error, never a panic or a hang, and integration tests cut a template chain at the depth budget without losing the user's code. |
| Any tree depth is safe. | Lowering is an explicit-stack machine; a test lowers a 100,000-deep operator chain on the default test stack. |

<hr>
<br>

## Installation

```toml
[dependencies]
lower-lang = "0.2"
hir-lang = "0.3"      # the output
intern-lang = "1"     # names
syntax-lang = "1"     # the input
```

Without the standard library:

```toml
[dependencies]
lower-lang = { version = "0.2", default-features = false }
```

<hr>
<br>

## Quick start

A mapping for `1 + 2`, a tree built with syntax-lang's builder, and one call:

```rust
use hir_lang::OpKind;
use intern_lang::Interner;
use lower_lang::{lower, Arch, MappingBuilder, OpSpec, Pick, TokenRole};
use syntax_lang::{Builder, Span, Token, TokenKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum K { File, Add, Num, Plus, Space }
impl TokenKind for K {
    fn is_trivia(&self) -> bool { matches!(self, K::Space) }
}

// The semantics, as data.
let mut b = MappingBuilder::new();
b.node(K::File, Arch::Module { name: Pick::Absent, items: Pick::Nodes });
b.node(K::Add, Arch::binary()); // lhs = 1st child, op = 2nd, rhs = 3rd
b.token(K::Num, TokenRole::Int);
b.binary_op(K::Plus, OpSpec::Op(OpKind::Add));
let mapping = b.build()?;

// `1 + 2`
let mut t = Builder::new();
t.start_node(K::File);
t.start_node(K::Add);
t.token(Token::new(K::Num, Span::new(0, 1)));
t.token(Token::new(K::Space, Span::new(1, 2)));
t.token(Token::new(K::Plus, Span::new(2, 3)));
t.token(Token::new(K::Space, Span::new(3, 4)));
t.token(Token::new(K::Num, Span::new(4, 5)));
t.finish_node();
t.finish_node();
let tree = t.finish().expect("balanced");

let mut names = Interner::new();
let out = lower(&tree, "1 + 2", &mapping, &mut names).expect("within budget");
assert!(out.problems.is_empty());
assert_eq!(
    hir_lang::print(&out.hir, &names),
    "(module\n  (block\n    (do\n      (op add overflow=error\n        (lit 1)\n        (lit 2)))))"
);
# Ok::<(), lower_lang::MappingError>(())
```

### Sugar through hygienic templates

`while c { b }` is a template over HIR's one loop. The template's own names
are marked with its expansion, so they can never meet the user's:

```rust
use hir_lang::{ExpnKind, OpKind};
use intern_lang::Interner;
use lower_lang::{lower, templates, Arch, MappingBuilder, OpSpec, Pick, TokenRole};
use syntax_lang::{Builder, Span, Token, TokenKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum K { File, While, Block, Name, WhileKw }
impl TokenKind for K {}

let mut b = MappingBuilder::new();
let while_loop = b.template(templates::while_loop())?;
b.node(K::File, Arch::Module { name: Pick::Absent, items: Pick::Nodes });
// holes: cond, body, label
b.template_node(K::While, while_loop, &[Pick::Kind(K::Name), Pick::Kind(K::Block), Pick::Absent]);
b.node(K::Block, Arch::Block { stmts: Pick::Nodes, tail: Pick::Absent, label: Pick::Absent, tail_from_last: false });
b.token(K::Name, TokenRole::Name);
let mapping = b.build()?;

// `while go {}` — with the half-typed `while {}` next to it.
let mut t = Builder::new();
t.start_node(K::File);
t.start_node(K::While);
t.token(Token::new(K::WhileKw, Span::new(0, 5)));
t.token(Token::new(K::Name, Span::new(6, 8)));
t.start_node(K::Block);
t.finish_node();
t.finish_node();
t.start_node(K::While);
t.token(Token::new(K::WhileKw, Span::new(9, 14)));
t.start_node(K::Block);
t.finish_node();
t.finish_node();
t.finish_node();
let tree = t.finish().expect("balanced");

let mut names = Interner::new();
let out = lower(&tree, "while go while", &mapping, &mut names).expect("within budget");
// The second `while` has no condition: one problem, and the HIR is still valid.
assert_eq!(out.problems.len(), 1);
assert_eq!(out.hir.validate(), Ok(()));
let first = out.hir.expansion(hir_lang::ExpnId::from_u32(1)).expect("recorded");
assert_eq!(first.kind, ExpnKind::Desugar);
assert_eq!(names.resolve(first.name), Some("while"));
# Ok::<(), lower_lang::MappingError>(())
```

### Trees from lang-forge

lang-forge 1.x trees have no field labels yet, so a mapping addresses a
node's children by kind (`Pick::Kind`), position (`Pick::Nth`), or neighbour
(`Pick::After`, `Pick::Before`). [`examples/calc.rs`](./examples/calc.rs) forges
a small language from an `.lsf` schematic and lowers a program and a half-typed
one; [`tests/calc.rs`](./tests/calc.rs) checks the result end to end, and a
resolver resolves every local reference by HIR's scope rules.

<hr>
<br>

## Examples

Runnable programs in [`examples/`](./examples):

| Example | What it shows |
|---|---|
| [`calc`](./examples/calc.rs) | The Tier-1 path end to end: an `.lsf` schematic forged by lang-forge, a mapping, a program and a half-typed one lowered, printed with origins and problems. |
| [`hygiene`](./examples/hygiene.rs) | A Mox-style `foreach` through the built-in `for_each` template next to a user variable `$it`; a toy resolver shows the user's `$it` and the template's `it` never meet. The tree is built with syntax-lang's `Builder`. |

<hr>
<br>

## Performance

Lowering is one explicit-stack pass over the tree that builds HIR bottom-up
with hir-lang's builder, then hir-lang's lenient validation. Picks are
resolved against a node's significant children with linear scans; kind tables
are sorted vectors (binary search); scratch buffers (children, values, lists,
template-instantiation state) are pooled and reused, so steady-state lowering
allocates only what the HIR itself stores.

Measured with [`benches/bench.rs`](./benches/bench.rs) (library only: the
trees are parsed once, outside the measurement; the measurement includes
`finish_lenient` validation), Windows 11, x86_64, Rust stable, release
profile. Elements are syntax-tree nodes plus tokens (trivia included).

| Benchmark | Input | Time | Throughput |
|---|---|---:|---:|
| `calc/100k` | 177,101 elements (189 KB of calc) | ~12.6 ms | ~14 M elements/s |
| `calc/1m` | 1,771,001 elements (1.9 MB) → 748,002 HIR nodes | ~157 ms | ~11 M elements/s |
| `templates/100k` | 93,001 elements, mostly `while` + `&&`/`\|\|` templates | ~12 ms | ~7.8 M elements/s |
| `templates/1m` | 930,001 elements | ~129 ms | ~7.2 M elements/s |
| `deep/100k_prefix_chain` | one 100,000-deep chain (200,004 elements) | ~37 ms | ~5.4 M elements/s |

Of the `calc/1m` time, measured separately: hir-lang's validation is about
20 ms and hir-lang's node building about 30 ms (its own published figure for a
similar size); the rest is lowering's traversal, interning, and dispatch.
These figures were taken on an otherwise idle machine just before the final
change of this release (the check that a child is not picked by two fields);
an interleaved A/B of that check on a heavily loaded machine was dominated by
noise and puts its cost at no more than about 10%. The release gate re-measures
on Windows and Linux.

```bash
cargo bench --bench bench
```

<hr>
<br>

## Design notes

- **Archetypes, not code.** Most constructs of real languages are instances of
  a small set of forms (LexerSketch ARCHITECTURE §5). A mapping says which node
  kind is which form and where its parts are; nothing about a language is
  hard-coded, and nothing requires a closure.
- **Fields by label or by position.** LSF2 grammars name their fields
  (`name:expr`); a host that has labels supplies a `Labeler` and the mapping
  uses `Pick::Label`. Trees without labels use kind and position picks. A child
  picked by two fields (positional picks alias in half-typed code) is lowered
  once and reported.
- **One loop, templates for the rest.** HIR has one loop and no short-circuit
  operators; `while`, `do-while`, C `for`, `for-each`, `&&`, `||`, `??`, `?`,
  and `x++` are templates. Each instantiation is an HIR expansion
  (`Desugar` for the built-ins), so diagnostics and tools can see the surface
  form.
- **Hygiene by construction.** A template's binders are created fresh per
  instantiation with the expansion's mark and referenced by binder id; its free
  names carry the mark too and resolve at the definition site; host symbols
  (`extern_`) are pre-resolved. User code in holes keeps its own (root) marks.
- **Holes used twice are copied.** HIR is a tree, so the second use of a hole
  (`x++` reads and writes `x`) is a `copy_subtree` copy, charged against the
  node budget.
- **Templates are acyclic by construction.** A template may apply only
  templates registered before it, so expansion terminates; a depth budget
  bounds long chains anyway, and a cut chain keeps the user's code attached.
- **Half-typed code is normal input.** Error nodes, unmapped kinds, missing
  parts, wrong contexts, unknown operators and labels, bad literals and escapes
  each become an HIR error form plus a `Problem`; `finish_lenient` repairs and
  reports the rest.

<hr>
<br>

## Testing

```bash
cargo test                       # unit + integration + property + doctests
cargo test --no-default-features # no_std + alloc
cargo clippy --all-targets --all-features -- -D warnings
cargo bench --bench bench
```

[`tests/calc.rs`](./tests/calc.rs) is the worked example end to end (lang-forge
→ CST → lower-lang → hir-lang → resolve), including every prefix of a program;
[`tests/mox.rs`](./tests/mox.rs) covers Mox-flavoured constructs built with
syntax-lang's `Builder` (`$vars`, `foreach`, by-reference parameters and place
arguments, implicit locals, `global`/`static`, `use` captures, `->` members,
`$$var`, `$a[]`, classes with mixins, try/catch/finally, generators, map
literals); [`tests/archetypes.rs`](./tests/archetypes.rs) covers the rest of the
catalogue and every problem kind; [`tests/properties.rs`](./tests/properties.rs)
holds the property tests and the reference evaluator. Every `rust` example in
this README and in [`docs/API.md`](./docs/API.md) is compiled and run as a
doctest.

<hr>
<br>

## Cross-platform support

- Linux (x86_64, aarch64)
- macOS (x86_64, Apple Silicon)
- Windows (x86_64)

The crate uses no operating-system facilities and no platform-specific code;
the same tree lowers to the same HIR everywhere.

<hr>
<br>

## Contributing

See [`REPS.md`](./REPS.md) for the engineering standards every change is held to, [`dev/DIRECTIVES.md`](./dev/DIRECTIVES.md) for the invariants and the definition of done, and [`dev/ROADMAP.md`](./dev/ROADMAP.md) for what comes next. Before a PR: `cargo fmt --all`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test --all-features` must be clean.

<br>

<div id="license">
    <h2>License</h2>
    <p>Licensed under either of</p>
    <ul>
        <li><b>Apache License, Version 2.0</b> &mdash; <a href="./LICENSE-APACHE">LICENSE-APACHE</a></li>
        <li><b>MIT License</b> &mdash; <a href="./LICENSE-MIT">LICENSE-MIT</a></li>
    </ul>
    <p>at your option.</p>
</div>

<div align="center">
  <h2></h2>
  <sup>COPYRIGHT <small>&copy;</small> 2026 <strong>James Gober.</strong></sup>
</div>
