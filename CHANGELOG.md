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

[Unreleased]: https://github.com/jamesgober/lower-lang/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/jamesgober/lower-lang/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/jamesgober/lower-lang/releases/tag/v0.1.0
