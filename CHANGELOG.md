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

## [0.4.1] - 2026-10-12

PHP classes and loops for the Mox demo: methods get the implicit receiver
`$this` that bcgen-lang 0.4 compiles as an instance method's receiver, numeric
`break N` / `continue N` lower to labelled jumps, one token can be a rest
parameter and a spread argument, and a node can pick its template by a child
(the `include` family). An additive patch: no existing signature changes, and
every new behaviour is off until a mapping asks for it.

### Added

- **Implicit receiver** (LSF2 `implicit_receiver`; ISSUES P24):
  `LangPolicy::implicit_receiver: Option<NameRef>`. Every non-`static` method
  of a class, mixin, or interface without an explicit receiver gets a first
  parameter of kind `Receiver` binding that name (an ordinary, unmarked user
  name; its origin is the method's name). Closures inside methods follow PHP:
  an explicit-captures closure (`implicit: None`) gets a by-value capture of
  the receiver, unless it captures it itself; implicit-capture closures reach
  it through their captures; a `static` closure (and anything nested in one)
  has none, and reading the name there is the new `ProblemKind::NoReceiver`.
  An assignment to the receiver never declares an implicit local.
  `MappingBuilder::build` refuses a `NameRef` it did not issue
  (`MappingError::UnknownName`).
- **Default member visibility** (LSF2 `default_visibility`):
  `LangPolicy::default_member_visibility: hir_lang::Vis`. A method,
  property, static, or constant of a class, mixin, or interface without a
  visibility modifier gets it (PHP and Mox: `Public`); the default `Private`
  keeps 0.4.0's behaviour. Declarations outside class bodies, `impl` members,
  and members with a keyword are unchanged.
- **Numeric break levels** (LSF2 `break_levels = "numeric"`; ISSUES P24, P40):
  `LangPolicy::break_levels` and the new `#[non_exhaustive]` enum
  `BreakLevels { Labels, Numeric }`. Under `Numeric`, `break N` / `continue N`
  target the N-th loop around the jump in its function (an `Arch::Loop`, or a
  template node whose template has a label hole); the target keeps its own
  label or gets a hygienic one (marked with a `Desugar` expansion named
  `break_level`), made only when a jump needs it. Levels may be any integer
  literal, through `Pass` wrappers (`break (2)`). A level of `0`, a
  non-literal, or a level past the enclosing loops is the new
  `ProblemKind::BreakLevel { level, loops }`, and the jump becomes an error
  expression.
- **Per-archetype modifiers** (LSF2 `"..." = { param = "rest", arg = "spread" }`;
  ISSUES P24, P40): `MappingBuilder::modifier_in(kind, archetype, modifier)`
  and `Mapping::modifier_in(kind, archetype)`. Inside the named archetype the
  entry wins over the plain `modifier` one. `Spread` on a `map_entry`
  (`[...$a]`) is reported as `Unsupported` (HIR has no spread entry) instead
  of silently lowering `$a` as one element. Unknown archetype names are
  `MappingError::UnknownArchetype`; two entries for one kind and archetype are
  `MappingError::DuplicateKind { table: "archetype modifier", .. }`.
- `Arch::lsf_name()`: an archetype's LSF2 name (`param`, `arg`, `map_entry`,
  ...), the key `modifier_in` takes.
- **Conditional templates** (LSF2 conditional `{ when = …, template = … }`
  alternatives; ISSUES P40): `MappingBuilder::template_node_when(kind, when,
  template, holes)`, tried in registration order with the other rules for the
  kind. Mox's `include` / `include_once` / `require` / `require_once` lower
  through it.
- **Keyword type names** (LSF2 §31): in a type, a keyword token without a role
  whose text is a registered `type_name` (`array`, `callable`), and a token
  with the literal role `Null` / `True` / `False` whose text is one (`null`,
  `false`, `true`), is that type. Without a `type_name` entry they are refused
  as before.
- Tests: `tests/receivers.rs` (constructors setting `$this->x`, methods
  reading it, `parent::__construct`, static methods, mixin methods, and
  closures over `$this`, `new C(...)` through the `new_object` template and
  `new $name(...)`, and classes with no visibility keyword, run end to end
  through resolve-lang 0.4.1, bcgen-lang 0.4, and bvm-lang 2.0.0-alpha.4;
  default member visibility; capture rules; hygiene against a template
  binding `this`; a reference-model property over random methods and
  closures), `tests/break_levels.rs` (nested loops with `continue`,
  `continue 2`, `break 2`, and `break 1` run end to end; a reference-model
  property over random nests of loops, closures, and jumps; every bad level;
  a 5,000-deep nest on a 256 KiB stack), `tests/modifiers_and_conditions.rs`
  (rest and spread through one token, run end to end; the spread entry; the
  include family; registration order; keyword type names). The random-tree
  properties run under a fourth policy with every 0.4.1 feature on.
- Dev-dependencies `bcgen-lang = "0.4"` and `bvm-lang = "=2.0.0-alpha.4"`
  (registry), to run lowered Mox end to end.
- Benchmark groups `oop/*` and `oop_off/*`.

### Changed

Behaviour a 0.4.0 mapping can notice (each was an error or a silent
miscompile before; none changes a correct program's HIR):

- A `map_entry` carrying `Modifier::Spread` (from a plain `modifier` entry
  too) is now an `Unsupported` problem and an error entry. 0.4.0 dropped the
  marker and lowered `[...$a]` as `[$a]`.
- In a type position, a keyword token without a token role whose text is a
  registered `type_name` is that type (0.4.0: an `Unmapped` problem), and a
  `Null`/`True`/`False`-role token whose text is one is that type (0.4.0: a
  `NotAllowedHere` problem).
- The modifier scan reads per-archetype entries when a mapping has any;
  mappings without them take the 0.4.0 path.

---

## [0.4.0] - 2026-10-12

Textual templates (decision D26), hir-lang 0.4, and the resolver in the loop:
the step that lets LexerSketch lower Mox's `echo`, `print`, `&&`, `||`, and
`++$i` (LSF5105) on one `hir_lang` shared with resolve-lang 0.4 and
typeck-lang 0.3.

### Breaking

- **Depends on `hir-lang` 0.4** (was 0.3). `Lowered::hir` is hir-lang 0.4's
  `Hir`; a caller still on hir-lang 0.3 gets two incompatible `hir_lang`
  crates. hir-lang 0.4's own breaking changes reach callers through the HIR:
  `Expr::Yield { key, value }`, the new `Expr::LetPlace`/`YieldFrom` and
  `Ty::Union`/`Intersection` forms, `BinderKind::Place`, logical-only
  `OpKind::Not` (`BitNot` is new), and normalized unions.
- **`Arch::Yield` has a `key` field** (`yield $k => $v`); mappings that build
  it add `key: Pick::Absent`.
- **The built-in `templates::post_increment()` evaluates its place once**:
  it is now `let_place p = x in { let old = p; p = old + 1; old }`, so the
  HIR has a `LetPlace` (and a `Place` binder) where 0.3 had two copies of the
  place.
- New `OpSpec` variants (`CompoundExtern`, `CompoundIntrinsic`,
  `CompoundTemplate`), `Arch` variants (`YieldFrom`, `UnionTy`,
  `IntersectionTy`), and the `TemplateError::Syntax` variant; all three
  enums are `#[non_exhaustive]`, so only code that listed every variant
  changes.

### Added

- **Textual templates** (HIR spec §20 template additions, LSF2 §15.7/§15.8):
  `Template::parse(name, text)` (the lazy path) and
  `MappingBuilder::parse_template(name, text)` (parse and register, with
  `apply:` naming templates registered before; built-ins also by their LSF2
  names) parse the template subset of HIR's textual syntax straight into the
  template AST: expressions, blocks and statements, `let`, `if`/`else`,
  `loop` with `[step: …]` and labels, `break`/`continue`, `match` with
  patterns and guards, closures with capture modes, ops by OPS name with the
  language's policies or exact ones (`add<overflow=wrap>(a, b)`,
  `int_cast<i32>(x)`), literals, tuples, arrays, calls, methods, fields,
  indexes, `yield` (keyed), `yield from`, `let_place`, `return`, `throw`,
  `await`, `spawn`, `defer`, union and intersection types; holes `${name}`
  (kind from position), `${name?}`, `${name?=<literal>}`, `${...name}`;
  template binders and labels without ids, fresh per instantiation;
  `extern:`, `global:`, `apply:`. The parser is recursive descent with a
  nesting cap (`MAX_TEMPLATE_DEPTH` = 32) and a length cap
  (`MAX_TEMPLATE_TEXT` = 1 MiB), never panics, is never quadratic, and
  reports `TemplateError::Syntax { at, error: SyntaxError }` with the byte
  range for every refusal. Every template text of `mox.lsf` parses.
- `SyntaxError` (`#[non_exhaustive]`), `MAX_TEMPLATE_TEXT`,
  `MAX_TEMPLATE_DEPTH`; `Template::desugar` (LSF2 `desugar = true`) and
  `Template::hole_index` (bind node fields to holes by name, the inline
  `[semantics.lower]` form).
- **Compound assignment with non-OPS operators** (GAPS B2, LSF2 §15.6
  `{ compound = { extern | intrinsic | template = … } }`):
  `OpSpec::CompoundExtern`, `CompoundIntrinsic`, and `CompoundTemplate`
  lower `target op= value` to `let_place p = target in p = op(p, value)`,
  so the target's operands are evaluated once (`$a[f()] .= "!"` calls `f`
  once). The alias is a `Place` binder marked with a `Desugar` expansion
  named `compound`. Through `Arch::Assign`'s operator field or a
  binary-operator node alike.
- `TemplateBuilder::let_place`, `yield_kv`, `yield_from`, `union_ty`,
  `intersection_ty`.
- `Arch::YieldFrom { inner }`, `Arch::UnionTy { members }`,
  `Arch::IntersectionTy { members }` (LSF2 `union_ty` / `intersection_ty`);
  no members is a `MissingField` problem and an error type.
- `pow` is a valid `Compound` op (hir-lang 0.4).
- `resolve-lang` 0.4 as a dev-dependency: the Mox tests, the PHP-scoping
  property, and the random-tree properties resolve through it.
- Tests: `src/text/tests.rs` (every Mox template, the builder forms of four
  of them, every `SyntaxError` kind, most with their byte range checked, a differential property
  of parse against `TemplateBuilder` over random templates, arbitrary text
  and token soup, every nesting form past the cap on a 512 KiB thread);
  `tests/textual.rs` (Mox `&&`, `||`, `++`, `--`, `echo`, `print`, `.=`,
  `??=`, `**=`, `yield from`, keyed `yield`, and a union parameter type,
  lowered, validated, and resolved clean; `++$a[f()]` calls `f` three times
  with the sketch's current `pre_increment` and once with the `let_place`
  one; the inline `[semantics.lower]` form); `tests/hir_0_4.rs` (the 0.4
  archetypes and op specs, union normalization, lenient repair). Random
  trees now reach `let_place`, unions, intersections, keyed yields, and
  `yield from` (checked). Example `textual`; benchmark group
  `template_text`.

### Changed

- hir-lang 0.4's behaviour reaches lowering as follows, each covered by a
  test: union and intersection types come back normalized from
  `finish_lenient` (sorted, flattened, repeats dropped, one member
  collapsed); lenient repair reports a duplicate binding at every repeat (an
  or-pattern's alternatives sharing a binder report each one); orphan
  reporting changed in hir-lang but lowering produces no orphans.
- `OpKind::Not` in templates and `OpSpec::Op` is logical negation (hir-lang
  0.4); use `OpKind::BitNot` for the bitwise complement.
- A template's `int_cast` takes `overflow = error` under a `promote`
  language policy, as casts already did (HIR refuses `promote` on a static
  result).

### Fixed

- **`Arch::RefAssign` was never finished**: its operands were lowered and
  then dropped, so `$b = &$a` through the archetype gave orphaned HIR
  (internal validation problems) instead of `Expr::RefAssign`. It now
  lowers like the `=&` operator spec.

### Migration

| 0.3 | 0.4 |
|---|---|
| `hir-lang = "0.3"` | `hir-lang = "0.4"` |
| `Arch::Yield { value }` | `Arch::Yield { key: Pick::Absent, value }` |
| `match expr { Expr::Yield(v) => … }` on the output | `Expr::Yield { key, value }`, plus `Expr::YieldFrom` and `Expr::LetPlace` arms |
| `OpSpec::Op(OpKind::Not)` for a bitwise `~` | `OpSpec::Op(OpKind::BitNot)` |
| `.=` as a hook or an `Extern` that evaluates the place twice | `OpSpec::CompoundExtern(name)` / `CompoundIntrinsic(name)` |
| `??=` | `OpSpec::CompoundTemplate(coalesce)` |
| a template built with `TemplateBuilder` from a sketch's text | `MappingBuilder::parse_template(name, text)` |
| `{ ${place} = add(${place}, 1); ${place} }` (place evaluated three times) | `let_place p = ${place} in p = add(p, 1)` |
| a test resolver over `lookup_local` | `resolve_lang::Resolver::new(Policy::php()).resolve(hir, &names)` |

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

[Unreleased]: https://github.com/jamesgober/lower-lang/compare/v0.4.1...HEAD
[0.4.1]: https://github.com/jamesgober/lower-lang/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/jamesgober/lower-lang/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/jamesgober/lower-lang/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/jamesgober/lower-lang/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/jamesgober/lower-lang/releases/tag/v0.1.0
