<h1 align="center">
    <img width="90px" height="auto" src="https://raw.githubusercontent.com/jamesgober/jamesgober/main/media/icons/hexagon-3.svg" alt="Triple Hexagon">
    <br><b>CHANGELOG</b>
</h1>
<p>
  All notable changes to <code>lower-lang</code> will be documented in this file. The format is based on <a href="https://keepachangelog.com/en/1.1.0/">Keep a Changelog</a>,
  and this project adheres to <a href="https://semver.org/spec/v2.0.0.html/">Semantic Versioning</a>.
</p>

---

## [Unreleased]

---

## [0.3.0] - 2026-10-08

PHP and Python semantics before the Mox demo (ISSUES P15): PHP scoping,
interpolated strings and heredocs, a literal decoding table, text-keyed
operator tables, nested `use` trees, trait conflict rules, and attributes.

### Breaking

- `Arch::MixinUse` has a second field, `rules` (the trait conflict rules);
  mappings that build it must add `rules: Pick::Absent` (or a pick of
  `MixinRule` nodes).
- A hoisted `Arch::Let` (`hoist: true`) with a destructuring pattern is now
  hoisted (temporaries bound by the pattern, then assigned to
  function-level names); 0.2 silently lowered it as an ordinary scoped
  `let`.
- Integer literals beyond 64 bits follow `LangPolicy::overflow`: with
  `Overflow::Promote` (PHP) every integer above `i64::MAX` (including
  `9223372036854775808` and `0xFFFF_FFFF_FFFF_FFFF`) lowers to the nearest
  float instead of an integer or big-integer literal; otherwise binary,
  octal, and hex integers beyond 64 bits are exact big integers (0.2 reported
  them as invalid).
- Number spelling is now a policy (`LangPolicy::literals`). The default
  table keeps 0.2's spelling (separators anywhere, `0777` decimal) and adds
  hexadecimal floats (`0x1.8p3`), which 0.2 rejected.
- Nodes whose kind has an `Arch::Attr` rule (and containers holding only
  such nodes) are no longer children for picks; positional picks of a node
  with attributes count without them.

### Added

- `LangPolicy::php()`, the PHP and Mox preset (promote-to-float, throws
  always, inferred generators, implicit locals, place arguments,
  function-scoped bindings, PHP number spelling).
- `LangPolicy::function_scoped_bindings`: `for-each` template pattern holes
  and `catch` names belong to the enclosing function (PHP). Each binding is
  a hygienic temporary (one `Desugar` expansion named `hoist` per binding)
  assigned to the function-scoped name right after the pattern matches; any
  pattern shape works. A binding that cannot be hoisted (by reference, a hole
  the template binds twice, a binding site cut by the depth budget) is
  reported as `ProblemKind::CannotHoist { reason: HoistBlocker }` and stays
  scoped.
- `Arch::Interp { parts, close, join }` for interpolated strings and
  heredocs delivered in segments (`"Hello {$name}"`, `"$name"`,
  `"{$a['k']}"`), with `Join::{Fold, Call, Template}`; a heredoc's `close`
  marker strips its indentation from every line (PHP 7.3+; a line indented
  less, or starting with an embedded expression, is an `InvalidLiteral`);
  empty text pieces are left out of the join. The expected CST
  shapes are documented in `tests/support/mox.rs`.
- `TokenRole::RawStr` (Rust `r#"…"#`, Python `r'…'` and triple-quoted raw
  strings), `TokenRole::Heredoc(Escapes)` (heredoc and nowdoc tokens, PHP
  7.3 indentation stripping, final newline dropped), and
  `TokenRole::StrPart(Escapes)` (interpolation pieces).
- The literal decoding table `Literals { separators, legacy_octal,
  hex_floats }` with `Separators::{Anywhere, BetweenDigits, None}` and
  `LegacyOctal::{Decimal, Octal, Error}`; hex floats are correctly rounded
  (subnormals included); non-decimal big integers are converted to exact
  decimal digits (up to 1,024).
- Text-keyed operator tables: `MappingBuilder::prefix_op_text`,
  `binary_op_text`, `postfix_op_text` (a kind entry wins over a text entry).
- `Arch::UseTree { path, alias, children }`: nested imports (`use a::{b,
  c::{d as e}, self, *}`, PHP `use App\Models\{User, Post as P}`) flattened
  iteratively into one HIR import per leaf.
- `Arch::MixinRule { from, method, insteadof, alias }`: trait `insteadof`
  exclusions and `as` aliases (new name, visibility, or both), lowered to
  hir-lang's `MixinRule`.
- `Arch::Attr { name, args }` and `Arch::AttrArg { key, value }`: attributes
  attached to the HIR node of the construct they annotate (items, parameters,
  fields, variants, statements, expressions); PHP `#[A, B]` groups splice.
- `ProblemKind::{CannotHoist, AttributeArgument, MisplacedAttribute}`,
  `HoistBlocker::{ByReference, ReusedHole, NoBindingSite}`,
  `MappingError::{BadJoin, EmptyOperatorText}`.
- Tests: `tests/semantics.rs` (10 unit tests and 5 property tests against
  references: function-scoped bindings, interpolation, heredoc indentation,
  integer literals, use trees); 7 new Mox tests (interpolation, heredocs,
  function-scoped `foreach`/`catch`, trait rules, group `use`, attributes,
  PHP number literals); the random-tree properties run under the PHP policy
  too and check that the generator reaches the new paths. Benchmark group
  `php`. Example `php`.

### Fixed

- PHP semantics in Mox lowering: `foreach` key/value and `catch` variables
  were scoped to the loop or handler; they are now visible after it, as in
  PHP. Integers above `i64::MAX` are floats under PHP's policy, as in PHP.

### Changed

- Version 0.3.0. Dependencies unchanged (`syntax-lang` 1, `hir-lang` 0.3,
  `intern-lang` 1; `lang-forge` 1.0.1 as a dev-dependency).

---

## [0.2.0] - 2026-10-08

The foundation: a syntax tree, a mapping that says what its constructs mean,
and one call lower it into validated HIR, with origins, hygienic templates,
budgets, and every problem in half-typed code reported instead of fatal.

### Added

- `lower` (Tier 1) and `Lowerer` (`with_budget`, `with_unit`,
  `with_labeler`): lower any `syntax_lang::Node<K>` (with
  `K: Copy + Ord + TokenKind`) into a `hir_lang::Hir`, validated with
  `finish_lenient`; `Lowered { hir, problems }`.
- `Mapping` and `MappingBuilder`: a language's semantics as data, mirroring
  LSF2's `[semantics]` sections — node kinds to archetypes (`node`,
  `node_when` for grammars that put alternatives in one rule), token roles,
  modifiers, path roots, container and skip kinds, error kinds, type names,
  numeric suffixes, prefix/binary/postfix operator tables, host names,
  templates, and a `LangPolicy`. `build` checks required fields, duplicate
  kinds, template hole counts, and operator specs.
- The archetype catalogue `Arch`: structure (`Module` with items and a
  top-level body, `Pass`, `Splice`, `Unsupported`); binders and statements
  (`Let` with mutability and function-level hoisting, `ExprStmt`, `Defer`,
  `Static`, `GlobalDecl`, `Import`); declarations (`Fn`, `Record`/union,
  `Sum`, `Interface`, `Class` with bases/interfaces/mixins/static members,
  `Impl`, `Alias`, `AssocType`, `Const`, `GlobalVar`, `MixinUse`,
  `FieldDecl`, `VariantDecl`, `GenericParam`, `WherePred`, `Param` with
  by-ref/default/rest/named-only/positional-only/receiver, `Capture`);
  expressions (`Path` with roots and qualified selves, `Segment`,
  `AssocBinding`, `Unary`/`Binary`/`Postfix` through operator specs, `Assign`,
  `RefAssign`, `Call`, `Arg` with named/spread/place arguments, `Member`,
  `MethodCall`, `DynMember`, `Index`, `VarVar`, `Append`, `Tuple`, `Array`,
  `Map`, `MapEntry`, `RecordLit`, `FieldInit`, `Repeat`, `Cast`, `Borrow`,
  `Deref`, `Block`, `If`, `Match`, `Arm`, `Loop`, `Break`/`Continue` with
  labels, `Return`, `Throw`, `Try`, `Catch`, `Await`, `Yield`, `Spawn`,
  `Closure` with explicit and implicit captures, `Template`); patterns and
  types (every HIR pattern and type form). Expression archetypes convert to
  patterns and types where it makes sense (tuples, calls as constructor
  patterns, record literals, paths).
- `Pick`: fields by host label (`Label`, through the `Labeler` trait), by
  position (`Nth`, `Back`, `NthNode`, `Range`), by kind (`Kind`, `KindNth`,
  `AllKind`, `First`), and by neighbour (`After`, `Before`, `AllAfter`,
  `Between`), with container splicing and skip kinds in list picks.
- `TokenRole`, `Escapes`, `TypeName`, `Modifier`, `LangPolicy`
  (`EffectMode`, `IdentPatterns`, `PlaceArgs`, OPS policies), `OpSpec`
  (`Op`, `OpWith`, `Compound`, `CompoundWith`, `Assign`, `RefAssign`,
  `Identity`, `Borrow`, `Deref`, `Await`, `Template`, `Extern`, `Intrinsic`),
  `Fixity`, `NameRef`.
- Templates: `TemplateBuilder` builds a typed, HIR-shaped template AST with
  expression, pattern, type, statement-list, argument-list, and label holes,
  its own binders and labels, marked free names (`global`), host symbols
  (`extern_`), and `apply` of earlier templates; `finish` checks it
  (`TemplateError`). Each instantiation records an HIR expansion, creates its
  binders fresh with the expansion's mark, copies holes used twice, and keeps
  origins at the call site.
- `templates`: `while_loop`, `do_while`, `c_for`, `for_each`, `logical_and`,
  `logical_or`, `and_value`, `or_value`, `coalesce`, `try_operator`,
  `post_increment`.
- `Budget` (`max_nodes`, `max_expansion_depth`); `Problem`, `ProblemKind`,
  `Context`; `LowerError`, `MappingError`, `TemplateError`.
- Tests: the calc worked example forged by lang-forge (end to end, half-typed
  input, every prefix of a program), Mox-flavoured trees built with
  syntax-lang's `Builder`, the archetype catalogue, and property tests (random
  trees, arbitrary spans, budgets, calc token soup, hygiene, determinism, and
  a reference evaluator for calc semantics). Benchmarks at 100k–1.8M
  syntax-tree elements. Examples `calc` and `hygiene`.

### Changed

- Version 0.2.0. Dependencies wired: `syntax-lang` 1, `hir-lang` 0.3,
  `intern-lang` 1; `lang-forge` 1.0.1 as a dev-dependency.

---

## [0.1.0] - 2026-10-08

Initial scaffold and repository bootstrap. No domain logic yet &mdash; this release establishes the structure, tooling, and quality gates the implementation will be built on.

### Added

- `Cargo.toml` with crate metadata, Rust 2024 edition, MSRV 1.85.
- Dual `Apache-2.0 OR MIT` license files.
- `README.md`, `CHANGELOG.md`, and a documentation skeleton.
- `REPS.md` compliance baseline.
- `.github/workflows/ci.yml` CI matrix; `deny.toml`, `clippy.toml`, `rustfmt.toml`.
- `dev/DIRECTIVES.md` and `dev/ROADMAP.md` (committed engineering standards + plan).

[Unreleased]: https://github.com/jamesgober/lower-lang/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/jamesgober/lower-lang/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/jamesgober/lower-lang/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/jamesgober/lower-lang/releases/tag/v0.1.0
