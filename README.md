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
        <strong>lower-lang</strong> turns a lossless syntax tree into HIR. A language describes what its constructs <em>mean</em> as data &mdash; a mapping from node kinds to a small catalogue of semantic <strong>archetypes</strong> (binders, scopes, declarations, control, expressions, patterns, types), operator tables with their overflow and division policies, and hygienic <strong>templates</strong> for sugar such as <code>while</code>, <code>for-each</code>, <code>&amp;&amp;</code>, and <code>??</code>, written as HIR template text or built in code &mdash; and lower-lang builds the HIR, records where every node came from, and validates the result with <a href="https://crates.io/crates/hir-lang"><code>hir-lang</code></a>.
    </p>
    <p>
        It is the L3 stage of LexerSketch: declaring a language's semantics instead of hand-writing its compiler is what lets a sketch become a running language. Lowering is generic over the tree's kind type, so any <a href="https://crates.io/crates/syntax-lang"><code>syntax-lang</code></a> tree lowers &mdash; one forged by <a href="https://crates.io/crates/lang-forge"><code>lang-forge</code></a>, or one a hand-written parser built. Half-typed code is normal input: every broken construct becomes an HIR error node and a problem with its source span, and the rest of the file is still lowered.
    </p>
    <br>
    <hr>
    <p>
        <strong>MSRV is 1.85+</strong> (Rust 2024 edition). <code>no_std</code>-compatible (needs only <code>alloc</code>), <code>#![forbid(unsafe_code)]</code>, three dependencies, all from the family: <a href="https://crates.io/crates/syntax-lang"><code>syntax-lang</code></a> 1, <a href="https://crates.io/crates/hir-lang"><code>hir-lang</code></a> 0.4, and <a href="https://crates.io/crates/intern-lang"><code>intern-lang</code></a> 1.
    </p>
    <blockquote>
        <strong>Status: 0.4.1 (implicit receivers, numeric break levels, per-archetype modifiers, conditional templates).</strong> New in 0.4.1: methods of classes, mixins, and interfaces bind the <strong>implicit receiver</strong> (PHP's <code>$this</code>) as their HIR receiver parameter, and closures capture it as PHP does, so Mox classes run end to end through resolve-lang 0.4, bcgen-lang 0.4, and bvm-lang (constructors, <code>parent::__construct</code>, static and mixin methods, closures over <code>$this</code>); PHP's <code>break 2</code> / <code>continue 2</code> lower to labelled jumps; one token can be a rest parameter and a spread argument (<code>...</code>); a node can pick its template by a child (<code>include</code> / <code>require_once</code>); and class members without a visibility keyword can default to public, as PHP's do. All additive: nothing changes until a mapping asks for it. The archetype catalogue, the mapping model, hygienic templates, budgets, origins, and lenient validation are implemented, with the PHP semantics the Mox demo needs (function-scoped <code>foreach</code>/<code>catch</code> variables, interpolated strings and heredocs, a literal decoding table, text-keyed operator tables, nested <code>use</code> trees, trait rules, attributes). New in 0.4: <strong>templates written as text</strong> in HIR's template syntax (<code>Template::parse</code>, decision D26, ahead of hir-lang 0.5's full textual parser, which this front end will be checked against), the hir-lang 0.4 forms (place-once <code>let_place</code>, keyed <code>yield</code> and <code>yield from</code>, union and intersection types), and compound assignment with non-OPS operators (PHP <code>.=</code>, <code>**=</code>, <code>??=</code>) that evaluates its place once. The tests resolve lowered HIR through <a href="https://crates.io/crates/resolve-lang"><code>resolve-lang</code></a> 0.4 (a dev-dependency only). Field labels from LSF2 sketches are supported through a <code>Labeler</code> hook (lang-forge 1.x trees have none yet, so mappings address children by kind and position). The API is frozen at 1.0, after the LexerSketch app has lowered at least three target languages through it. See <a href="./CHANGELOG.md"><code>CHANGELOG.md</code></a> and the <a href="./dev/ROADMAP.md"><code>ROADMAP</code></a>.
    </blockquote>
</div>

<hr>
<br>

## The model

- A **[`Mapping`](./docs/API.md#mapping)** is a language's semantics as data, built with a [`MappingBuilder`](./docs/API.md#mappingbuilder). It mirrors LexerSketch's LSF2 `[semantics]` sections: node kinds to **[archetypes](./docs/API.md#arch)** (`[semantics.bind]`), **[templates](./docs/API.md#template)** (`[semantics.lower]`), and **[operators](./docs/API.md#opspec)** with their policies (`[semantics.ops]`), plus the token roles, modifiers (`mut`, `async`, `&`, `...`), and path roots the archetypes read.
- An **[`Arch`](./docs/API.md#arch)** says what a node means and where its parts are: each field is a **[`Pick`](./docs/API.md#pick)** &mdash; a child by host field label, or by position or kind for trees without labels.
- A **[`Template`](./docs/API.md#template)** is a small HIR-shaped AST with `${field}` holes, written as **[text](./docs/API.md#textual-templates)** in HIR's template syntax (`Template::parse("bool_and", "if ${lhs} { not(not(${rhs})) } else { false }")`, `MappingBuilder::parse_template`) or built with a [`TemplateBuilder`](./docs/API.md#templatebuilder). Every instantiation records its own HIR expansion; the binders it introduces carry that expansion's hygiene mark. [`templates`](./docs/API.md#templates) has the common ones: `while`, `do-while`, C `for`, `for-each`, `&&`, `||`, `??`, `?`, `x++`.
- **Places are evaluated once.** `let_place p = ${place} in p = add(p, 1)` names a place (`$a[f()]`) once for reads and writes (HIR `LetPlace`); compound assignment with a host function, an intrinsic, or a template ([`OpSpec::CompoundExtern` / `CompoundIntrinsic` / `CompoundTemplate`](./docs/API.md#place-once-compound-assignment): PHP `.=`, `**=`, `??=`) lowers that way, and so does the built-in `x++`.
- **PHP semantics** are policy and archetypes, not special cases: [`LangPolicy::php()`](./docs/API.md#langpolicy) turns on function-scoped `foreach`/`catch` variables, implicit locals, promote-to-float integers, and PHP number spelling; [`Arch::Interp`](./docs/API.md#interpolated-strings-and-heredocs) joins string segments, [`Arch::UseTree`](./docs/API.md#use-trees-mixin-rules-attributes) flattens group imports, [`Arch::MixinRule`](./docs/API.md#arch) lowers trait `insteadof`/`as`, and [`Arch::Attr`](./docs/API.md#arch) attaches attributes.
- **PHP classes and loops** (0.4.1) are policy and mapping entries too: [`LangPolicy::implicit_receiver`](./docs/API.md#implicit-receiver) gives methods `$this`, [`BreakLevels::Numeric`](./docs/API.md#breaklevels) makes `break 2` a labelled jump, [`MappingBuilder::modifier_in`](./docs/API.md#per-archetype-modifiers) gives `...` one meaning in parameters and another in arguments, and [`template_node_when`](./docs/API.md#conditional-templates) picks a template by a child.
- **[`lower`](./docs/API.md#lower)** is the one-call entry point; [`Lowerer`](./docs/API.md#lowerer) adds a [`Budget`](./docs/API.md#budget), a compilation unit, and field labels. The result is a **[`Lowered`](./docs/API.md#lowered)**: a valid `hir_lang::Hir` and every **[`Problem`](./docs/API.md#problem)** in source order.

<br>

What it guarantees, and how each guarantee is checked:

| Guarantee | How it is held |
|---|---|
| Lowering never panics and always yields valid HIR, on any tree. | Property tests lower 512 random trees per run (random kinds and shapes over a mapping with most archetypes, error nodes, unmapped kinds, every template, four language policies, one with every 0.4.1 feature on) and 512 trees with spans outside the source or off character boundaries; every result re-validates. 256 random token soups per run, parsed by lang-forge into deep half-typed trees, lower the same way. Every prefix of a program (typing it one character at a time) lowers. |
| Every node and binder has an origin inside the source, through template expansions. | Checked on every result of every property test and integration test: each node's span lies in the source and each expansion it names exists. |
| Templates are hygienic: their names never capture or are captured by user names. | Property tests resolve every random result through the real resolver, [`resolve-lang`](https://crates.io/crates/resolve-lang) 0.4 (lexical policy or PHP's, matching the lowering policy), and check that every resolved reference names a binder with its own hygiene mark and that resolution is deterministic; integration tests put user variables named like the templates' temporaries (`it`, `t`, `old`) next to `for-each`, `??`, and `x++`, and the Mox tests resolve under `Policy::php()`. |
| A template text means exactly what the builder would build. | A differential property test generates 512 random templates per run (every expression, statement, pattern, and type form, holes of every kind, binders, labels, closures, `let_place`, keyed `yield`), renders each as text and builds it with `TemplateBuilder` side by side, and checks that parsing the text gives the same template (compared in a canonical form). Every template text of the Mox sketch parses, and the ones that matter for the demo (`&&`, `\|\|`, `++`, `--`, `echo`, `print`, `yield from`) lower into HIR that validates and resolves with no diagnostics. |
| Hostile template text never panics, overflows the stack, or runs long. | Property tests feed 512 arbitrary strings and 512 token soups of the template vocabulary per run; parsing is recursive descent with a nesting cap (`MAX_TEMPLATE_DEPTH` = 32), checked for 16 nesting forms past the cap on a 512 KiB thread, and a length cap (`MAX_TEMPLATE_TEXT` = 1 MiB); every lookup is a sorted-map lookup, so parsing is never quadratic. Every refusal is a `TemplateError::Syntax` with the byte range; tests cover every error kind and pin the range for most. |
| Places are evaluated once. | Tests count the index expressions and calls in lowered Mox code: `$a[$k] .= …`, `??=`, `**=`, `++$i`, `$i++`, `--$i`, `$i--` each evaluate their place once, and `++$a[f()]` calls `f` once with the `let_place` template where the sketch's current text calls it three times. |
| Lowering preserves meaning. | A property test generates random calc programs (assignments, arithmetic, comparisons, `&&`/`||`, `if`/`else`, counted `while` loops), evaluates them directly, renders them as text, parses them with lang-forge, lowers them, and evaluates the HIR: the printed values agree, 256 programs per run. |
| PHP and Python semantics match independent references. | Property tests check function-scoped `foreach`/`catch` bindings (random destructuring `var`, `foreach`, and `catch`, nested, with by-reference bindings) against a reference scope model, interpolated strings against a reference concatenation, heredoc indentation against PHP 7.3's rule, integer literals of every radix and separator policy against a `u128` reference, and nested use trees against a recursive flattener. Random trees reach the 0.3 paths (attributes attached and misplaced, use trees, interpolation joins, decoded texts) on every run, under the PHP policy as well. |
| Methods see `$this`; closures capture it as PHP does. | A property generates random classes (static and instance methods) holding random nests of closures (static or not, explicit or implicit captures, `use ($this)` or not) and named functions that read `$this`, and checks against a reference model: one receiver per instance method, which closures capture `$this`, and which reads are `NoReceiver` problems; every resolved `$this` names a receiver or a capture, and lowering is deterministic. Mox classes (constructors setting `$this->x`, methods reading it, `parent::__construct`, `new C(...)` and `new $name(...)`, static methods through `self::` and `K::`, mixin methods, closures and arrow functions over `$this`, classes with no visibility keyword under `default_member_visibility = Public`) run end to end through resolve-lang 0.4.1, bcgen-lang 0.4, and bvm-lang 2.0.0-alpha.4. A template that binds its own `this` around user code resolves each `this` to its own binder. |
| `break N` / `continue N` reach the right loop. | A property generates random nests of loops, closures, and jumps with levels 0 to 5 and checks every jump against a reference that counts loops per function: the labelled target, a plain jump, or the exact `BreakLevel` problem; only targeted loops get labels. Nested loops with `continue`, `continue 2`, `break 2`, and `break 1` run end to end on bvm-lang; 5,000 nested loops lower on a 256 KiB stack. |
| Lowering is deterministic. | Every random tree is lowered twice with fresh interners; the printed HIR with origins, the binders, and the problems are identical. |
| Budgets stop runaway work. | A node budget and a template-application depth budget; property tests check that a too-small node budget is an error, never a panic or a hang, and integration tests cut a template chain at the depth budget without losing the user's code. |
| Any tree depth is safe. | Lowering is an explicit-stack machine; a test lowers a 100,000-deep operator chain on the default test stack. |

<hr>
<br>

## Installation

```toml
[dependencies]
lower-lang = "0.4"
hir-lang = "0.4"      # the output
intern-lang = "1"     # names
syntax-lang = "1"     # the input
```

Without the standard library:

```toml
[dependencies]
lower-lang = { version = "0.4", default-features = false }
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

### Templates as text

LexerSketch sketches write their sugar as template text (LSF2
`[semantics.templates]`). The text is parsed once into a template, never
substituted; holes take their kind from where they are written:

```rust
use lower_lang::{HoleKind, MappingBuilder, OpSpec, Template};

// PHP's `&&` returns a bool.
let and = Template::parse("bool_and", "if ${lhs} { not(not(${rhs})) } else { false }")?;
assert_eq!(and.hole(0), Some(("lhs", HoleKind::Expr, false)));

// Registered in sketch order; `++$x` evaluates its place once.
let mut b = MappingBuilder::<u8>::new();
let pre_inc = b.parse_template("pre_increment", "let_place p = ${place} in p = add(p, 1)")?;
let echo = b.parse_template("echo", "extern:mox.echo(${...values})")?;
b.prefix_op(1, OpSpec::Template(pre_inc));
let concat = b.name("mox.concat");
b.binary_op(2, OpSpec::CompoundIntrinsic(concat)); // `.=`, place evaluated once
let _ = echo;
assert!(b.build().is_ok());

// Errors carry the byte range.
let err = Template::parse("t", "add(${x})").unwrap_err();
assert_eq!(err.to_string(), "template text, bytes 0..3: the operation takes 2 operands, 1 given");
# Ok::<(), Box<dyn std::error::Error>>(())
```

[`examples/textual.rs`](./examples/textual.rs) lowers a small Mox program
through such templates and prints the HIR.

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
| [`php`](./examples/php.rs) | `LangPolicy::php()` on a Mox function: a group `use`, an attribute, `foreach` variables still visible after the loop, an interpolated string and an indented heredoc, PHP octal and separators. |
| [`hygiene`](./examples/hygiene.rs) | A Mox-style `foreach` through the built-in `for_each` template next to a user variable `$it`; a toy resolver shows the user's `$it` and the template's `it` never meet. The tree is built with syntax-lang's `Builder`. |
| [`textual`](./examples/textual.rs) | Mox's sugar as template text: `&&`, `++`/`--` with `let_place` (the place evaluated once), `echo`, and `.=` through `OpSpec::CompoundIntrinsic`; prints the HIR. |
| [`classes`](./examples/classes.rs) | Mox classes under the 0.4.1 policies: a constructor setting `$this->x`, `parent::__construct`, a closure over `$this`, and `continue 2` / `break 2`; prints the HIR, then resolves, compiles with bcgen-lang 0.4, and runs it on bvm-lang. |

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
| `calc/100k` | 177,101 elements (189 KB of calc) | ~9.3 ms | ~19 M elements/s |
| `calc/1m` | 1,771,001 elements (1.9 MB) → 748,002 HIR nodes | ~122 ms | ~14.5 M elements/s |
| `templates/100k` | 93,001 elements, mostly `while` + `&&`/`\|\|` templates | ~5.5 ms | ~17 M elements/s |
| `templates/1m` | 930,001 elements | ~78 ms | ~11.9 M elements/s |
| `deep/100k_prefix_chain` | one 100,000-deep chain (200,005 elements) | ~28 ms | ~7.2 M elements/s |
| `php/100k` | 169,050 elements (245 KB of Mox): group `use`, `foreach` with function-scoped variables, interpolated strings, heredocs, try/catch | ~15 ms | ~11 M elements/s |
| `php/1m` | 1,690,500 elements (2.5 MB) | ~194 ms | ~8.7 M elements/s |
| `oop/100k` | 114,400 elements of Mox classes under the 0.4.1 policies: receivers, closures over `$this`, `continue 2` / `break 2`, rest and spread, `include_once` | ~7.8 ms | ~14.6 M elements/s |
| `oop/1m` | 1,144,000 elements | ~97 ms | ~11.8 M elements/s |
| `template_text/mox` | the 19 template texts of the Mox sketch (783 bytes), parsed and registered | ~20 µs | ~37 MiB/s |
| `template_text/large` | one 1 MiB template text (12,878 statements, about 155,000 template nodes) | ~23 ms | ~43 MiB/s |

The `oop/*` rows are 0.4.1 (`oop_off/*` lowers the same trees with the
receiver and numeric levels off, about 7 to 13% faster because it builds
fewer HIR nodes); interleaved A/B runs put 0.4.1 within noise of 0.4.0 on
`calc/100k` and `templates/100k`. The other rows are one run of the 0.4.0
benchmarks while other builds shared the
machine; run-to-run noise was up to about 20%, so treat them as rough. In
A/B runs in one session, lowering with the 0.4 code and with the 0.3 code
(both on hir-lang 0.4) was within that noise; moving the 0.3 code from
hir-lang 0.3 to 0.4 measured about 5% slower on `calc/100k` (hir-lang's
validation) and unchanged on `templates/100k` and `deep`. Of the `calc/1m`
time, hir-lang's validation and node building are roughly 50 ms (measured
for 0.3.0); the rest is lowering's traversal, interning, and dispatch. The
PHP policy's per-binding hoisting is the main reason `php/*` is slower per
element than `calc/*`. Template text parsing is linear: the 1 MiB text parses
at about the rate of the small ones. The release gate re-measures on Windows
and Linux.

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
- **Holes used twice are copied; places are named once.** HIR is a tree, so
  the second use of a hole is a `copy_subtree` copy, charged against the node
  budget, and its code runs twice. Where that is wrong (`++`, `--`, compound
  assignment), the template names the place with `let_place`, so its
  operands are evaluated once.
- **Template text is parsed, never substituted** (decision D26). The
  parser builds the same template AST the builder does; holes are filled
  with already-lowered HIR, so a template cannot produce text that re-lexes
  differently. It parses the template subset of HIR spec §20 ahead of
  hir-lang 0.5, which ships the full textual parser; this front end will be
  checked against it then.
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
`$$var`, `$a[]`, classes with mixins and trait rules, try/catch/finally,
generators, map literals, interpolated strings, heredocs, function-scoped
`foreach`/`catch` variables, group `use`, attributes, PHP number literals);
[`tests/semantics.rs`](./tests/semantics.rs) holds the 0.3 semantics tests and
their property tests against references (function-scoped bindings,
interpolation, heredocs, integer literals, use trees);
[`tests/archetypes.rs`](./tests/archetypes.rs) covers the rest of the
catalogue and every problem kind; [`tests/hir_0_4.rs`](./tests/hir_0_4.rs)
covers the hir-lang 0.4 forms (unions and intersections, keyed `yield` and
`yield from`, place-once compounds, lenient repair);
[`tests/textual.rs`](./tests/textual.rs) parses every template text of the Mox
sketch and lowers Mox code through them into HIR that validates and resolves
through resolve-lang; the template text parser's own tests and its
differential property are in `src/text/tests.rs`;
[`tests/receivers.rs`](./tests/receivers.rs),
[`tests/break_levels.rs`](./tests/break_levels.rs), and
[`tests/modifiers_and_conditions.rs`](./tests/modifiers_and_conditions.rs)
cover 0.4.1 (implicit receivers, numeric break levels, per-archetype
modifiers, conditional templates, keyword type names), each with programs run
end to end through resolve-lang 0.4, bcgen-lang 0.4, and bvm-lang
2.0.0-alpha.4 (dev-dependencies) and, for receivers and levels, a property
against a reference model;
[`tests/properties.rs`](./tests/properties.rs) holds the property tests and
the reference evaluator. Every `rust` example in
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
