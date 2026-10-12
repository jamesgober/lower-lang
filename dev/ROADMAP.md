# lower-lang - Roadmap

> Path from scaffold to a stable 1.0. Hard parts are front-loaded; each phase has hard exit criteria.
> Master plan: ../_lexersketch/ROADMAP.md and ../_lexersketch/NEW-LIBS.md
>
> **Anti-deferral rule:** no listed hard task moves to a later phase unless this file records the move and the reason.

## v0.1.0 - Scaffold (DONE)
Compiles, CI green, structure correct, no domain logic.
- [x] Manifest, README, CHANGELOG, REPS, dual license, CI, deny, clippy, rustfmt, DIRECTIVES, ROADMAP.

## v0.2.0 - Foundation (DONE)
- [x] Archetype catalogue and mapping model; lowering driver over syntax-lang trees; error-node handling.
- [x] Wires syntax-lang and hir-lang. Property tests: origins cover every node, no panics on arbitrary trees.

Pulled forward from v0.5.0 into this milestone (the v0.2.0 brief asked for the
full path): programmatic hygienic templates with `${field}` holes, budgets,
diagnostics with spans, and benchmarks. Only the *textual* template form stays
in v0.5.0, because it depends on hir-lang 0.5 (see below).

Delivered:
- `lower` (Tier 1), `Lowerer` (`with_budget`, `with_unit`, `with_labeler`),
  `Lowered`, `Budget`.
- `Mapping` / `MappingBuilder`: node rules (`node`, conditional `node_when`),
  token roles, modifiers, path roots, container/skip/error kinds, type names,
  numeric suffixes, prefix/binary/postfix operator tables (`OpSpec`), host
  names, `LangPolicy`, templates, `build` validation (`MappingError`).
- The archetype catalogue `Arch` (ARCHITECTURE §5): structure, binders and
  statements (let/const/var with mutability and hoisting, static, global,
  imports), declarations (fn, record/union, sum, interface, class with
  bases/interfaces/mixins/static members, impl, alias, associated type,
  const, global, module with top-level body, fields, variants, generic
  parameters, where-predicates, parameters with by-ref/default/rest/
  named-only/positional-only/receiver, captures), control (if, match, loop,
  break/continue with labels, return, defer, try/catch/finally, throw,
  yield, await, spawn), expressions (literals, paths with roots and qualified
  selves, calls with named/spread/place arguments, method calls, fields,
  index, dynamic members, `$$var`, append, tuples, arrays, maps, record
  literals, casts, borrows, closures, intrinsic operators with OPS policies),
  every HIR pattern and type form, and context conversions.
- Tree access: `Pick` (labels via `Labeler`, positions, kinds, neighbours,
  ranges), container splicing, skip kinds; a child picked by two fields is
  lowered once and reported (`AmbiguousChild`).
- Templates: `TemplateBuilder` (typed AST, all hole kinds, own binders and
  labels, marked free names, host symbols, `apply` of earlier templates),
  `TemplateError`; instantiation with one HIR expansion each (nested for
  `apply`), fresh marked binders, copies for repeated holes, call-site
  origins, unused-hole rescue when the depth budget cuts an `apply`.
- `templates`: while, do-while, C for, for-each, `&&`, `||`, value-returning
  `and`/`or`, `??`, `?`, `x++`.
- Effects (declared, inferred, always), PHP implicit locals and JavaScript
  `var` hoisting, or-pattern binder sharing, label scoping.
- Problems with CST spans (`Problem`, `ProblemKind`, `Context`), merged with
  hir-lang's lenient-validation problems in source order; `LowerError` for
  exhausted resources only.
- Tests: calc worked example forged by lang-forge 1.0.1 (end to end, half-typed
  code, every prefix of a program), Mox-flavoured trees via syntax-lang's
  `Builder`, the archetype catalogue, property tests (random trees over a
  kitchen-sink mapping, arbitrary spans, budgets, calc token soup, hygiene,
  determinism, a reference evaluator for calc semantics), a 100,000-deep tree.
  Benchmarks at 100k–1.8M syntax-tree elements. Examples `calc`, `hygiene`.

Dependency wiring (decided here, recorded per the anti-deferral rule):
- **syntax-lang 1: wired.** The input: `Node<K>`, `Element`, `TokenKind`.
- **hir-lang 0.3: wired.** The output: `Builder`, expansions, `copy_subtree`,
  `finish_lenient`.
- **intern-lang 1: wired.** HIR names are the session interner's symbols.
- **span-lang: not wired directly.** `Span` reaches us through hir-lang and
  syntax-lang, which share span-lang 0.4; a direct dependency would add
  nothing.
- **macro-lang: not wired.** ARCHITECTURE §5 has templates expanded by
  macro-lang's hygiene machinery. At HIR level that machinery is hir-lang's
  own `Expansion` records and `Name` marks, which lower-lang uses directly;
  macro-lang works on token trees (L2 EXPAND, before lowering). Revisit only
  if macro-lang 2.0 grows an HIR-level API.
- **diag-lang: not wired.** `Problem` carries a span and a kind; turning it
  into a `diag_lang::Diagnostic` is one line in the host (the LexerSketch
  app) and would make every user of lower-lang depend on diag-lang. Additive
  later if wanted.
- **lang-forge: dev-dependency only** (registry 1.0.1), for the worked example.

## v0.3.0 - PHP and Python semantics (DONE)
ISSUES P15, before the Mox Demo. Breaking changes allowed (0.x) and listed in
the CHANGELOG.

Delivered:
- [x] PHP scoping: `LangPolicy::function_scoped_bindings` (on in the new
      `LangPolicy::php()` preset). `foreach` key/value (every template
      pattern hole) and `catch` variables belong to the function; every
      binding pattern shape is hoisted (hygienic temporary + `Desugar`
      expansion `hoist` + assignment after the match); what cannot be
      (by-reference, a hole bound twice, a cut binding site) is the precise
      problem `CannotHoist { reason: HoistBlocker }`. A hoisted `Let` with a
      destructuring pattern uses the same mechanism.
- [x] String interpolation: `Arch::Interp { parts, close, join }` with
      `Join::{Fold, Call, Template}` (concat fold, one call, or template
      holes); PHP `"Hello {$name}"`, `"$name"`, `"{$a['k']}"`; CST shape
      documented in `tests/support/mox.rs`.
- [x] Literal decoding table: `TokenRole::RawStr`, `TokenRole::Heredoc`
      (heredoc/nowdoc with PHP 7.3 indentation stripping), `TokenRole::StrPart`,
      `Literals { separators, legacy_octal, hex_floats }`; integers beyond 64
      bits follow the overflow policy (PHP: float above `i64::MAX`), exact
      big integers for every radix otherwise.
- [x] Text-keyed operator tables (`*_op_text`).
- [x] Trait `insteadof`/`as` (`Arch::MixinRule`, `MixinUse.rules`) and nested
      use trees (`Arch::UseTree`) to their HIR forms.
- [x] Attributes (`Arch::Attr`, `Arch::AttrArg`) to HIR attributes, with
      `AttributeArgument` / `MisplacedAttribute` problems.
- [x] Mox test support (`LangPolicy::php()`, the new CST shapes) and example
      `php`; property tests against references in `tests/semantics.rs`; the
      random-tree properties run under the PHP policy and check that they
      reach the new paths. Benchmark group `php`.

Dependency wiring (decided here, recorded per the anti-deferral rule):
- **resolve-lang: not wired, not even as a dev-dependency (yet).** The P15
  brief allows it in tests only. resolve-lang 0.3.0 (case-folded PHP names,
  trait rules, C3) is prepared alongside this release but not on crates.io,
  and the family takes first-party crates from the registry by version, never
  by path. The tests resolve locals with a small resolver over hir-lang's
  `lookup_local`, which is enough for the scoping and hygiene properties.
  **Moved to v0.4.0:** add `resolve-lang = "0.3"` as a dev-dependency and
  run the Mox tests through the real resolver once 0.3.0 is published.
- Everything else unchanged from v0.2.0 (syntax-lang 1, hir-lang 0.3,
  intern-lang 1 wired; span-lang, macro-lang, diag-lang not wired;
  lang-forge 1.0.1 dev-dependency).

## v0.4.0 - Resolver in the loop, hir-lang 0.4, textual templates (DONE)
The shortest path to the LexerSketch Demo (G3, "Mox programs print"):
resolve-lang 0.4 and typeck-lang 0.3 are on hir-lang 0.4, so lower-lang had
to move too, and Mox's `echo`, `print`, `&&`, `||`, and `++$i` were blocked on
textual templates (LSF5105). Decision D26 pulled textual templates forward
from v0.5.0.

Delivered:
- [x] `resolve-lang = "0.4"` as a dev-dependency (the item said 0.3, which
      is built on hir-lang 0.3; 0.4 is the release on hir-lang 0.4, which
      lower-lang now uses). The Mox tests (`tests/mox.rs`, `tests/textual.rs`,
      `tests/hir_0_4.rs`), the PHP-scoping property (`tests/semantics.rs`),
      and the random-tree properties (`tests/properties.rs`, under the lexical
      or PHP resolution policy matching the lowering policy) resolve through
      it; the toy `lookup_local` resolver remains only for the calc
      evaluator.
- [x] hir-lang 0.4: keyed `yield` (`Arch::Yield { key, value }`),
      `Arch::YieldFrom`, `Arch::UnionTy` / `Arch::IntersectionTy` (LSF2
      §15/§31), `pow` compounds, logical `not` vs `bit_not`, `shift =
      saturate` through `LangPolicy::shift`; `TemplateBuilder::yield_kv`,
      `yield_from`, `let_place` (`BinderKind::Place`), `union_ty`,
      `intersection_ty`; the behaviour changes (union normalization, orphan
      reporting, lenient repair) tested.
- [x] Compound assignment with non-OPS operators (GAPS B2; LSF2 §15.6
      `{ compound = { intrinsic | extern | template = … } }`):
      `OpSpec::CompoundExtern`, `CompoundIntrinsic`, `CompoundTemplate`,
      through `let_place`, so the place is evaluated once. The built-in
      `post_increment` uses `let_place` too.
- [x] **Textual templates** (D26; HIR spec §20 "Template additions"; LSF2
      §15.7/§15.8): `Template::parse`, `MappingBuilder::parse_template`,
      `SyntaxError`, `MAX_TEMPLATE_TEXT`, `MAX_TEMPLATE_DEPTH`. Every
      `[semantics.templates]` text of `mox.lsf` parses and lowers; the inline
      `[semantics.lower]` form works through `Template::hole_index`. The
      `let_place` forms of the sketch's `pre_increment`, `pre_decrement`,
      `post_increment`, `post_decrement` are in `tests/textual.rs` and
      `examples/textual.rs` (the coordinator updates the sketch).
- [x] Fixed: `Arch::RefAssign` was never finished (orphaned operands).
- [x] Tests: parse-equals-builder differential property, hostile text,
      depth cap on a small stack, every error kind; Mox end to end with
      resolution; benchmarks `template_text/*`.

Dependency wiring (decided here, recorded per the anti-deferral rule):
- **hir-lang 0.4: wired** (was 0.3), shared with resolve-lang 0.4 and
  typeck-lang 0.3 (one `hir_lang` in the tree).
- **resolve-lang 0.4: dev-dependency** (registry). Not a runtime dependency:
  resolution is the next stage (L4), and lower-lang's output must not depend
  on it.
- **No new runtime dependency.** The template text parser is our own
  recursive descent over the text; hir-lang 0.5's textual parser does not
  exist yet (D26), and lower-lang does not wait for it.
- Unchanged: syntax-lang 1, intern-lang 1 wired; span-lang, macro-lang,
  diag-lang not wired; lang-forge 1.0.1 dev-dependency.

## v0.4.1 - Receivers, break levels, per-archetype modifiers, conditional templates (DONE)
An additive patch for the next Mox release: bcgen-lang 0.4 compiles classes,
but Mox methods could not run because lower-lang emitted no receiver
parameter (LSF2 `implicit_receiver` was pending). This patch closes that and
the other pending LSF2 items the release needs (ISSUES P24 rows, P40). No
existing signature changes; every new behaviour is off until a mapping asks
for it.

Delivered:
- [x] **`implicit_receiver`** (LSF2 §15.2): `LangPolicy::implicit_receiver:
      Option<NameRef>`. Non-`static` methods of classes, mixins, and
      interfaces get a first `Receiver` parameter binding the name (unmarked,
      so user code sees it and template binders never capture it); explicit
      receivers win; `impl` members and non-member functions are unchanged.
      Closures follow PHP: explicit-capture closures get a by-value capture
      of the receiver (unless spelled), implicit-capture closures reach it,
      `static` closures and anything nested in them have none (reading it
      there is `ProblemKind::NoReceiver`). `MappingError::UnknownName` for a
      foreign `NameRef`. End to end through resolve-lang 0.4, bcgen-lang 0.4,
      and bvm-lang 2.0.0-alpha.4: constructor setting `$this->x`, a method
      reading it, `parent::__construct`, static methods (`self::`, `K::`),
      mixin methods, closures and arrow functions over `$this`.
- [x] **`break_levels`** (LSF2 §15.2): `LangPolicy::break_levels`,
      `BreakLevels { Labels, Numeric }`. `break N` / `continue N` become
      labelled jumps to the N-th enclosing loop of the function (loops:
      `Arch::Loop` and templates with a label hole); hygienic labels made on
      demand; `ProblemKind::BreakLevel { level, loops }` for 0, non-literals,
      and levels past the enclosing loops. End to end on bvm-lang; a
      reference-model property over random nests.
- [x] **Per-archetype modifiers** (LSF2 §15.5): `MappingBuilder::modifier_in`,
      `Mapping::modifier_in`, `Arch::lsf_name`; `...` is a rest parameter and
      a spread argument (end to end); a spread `map_entry` is reported
      (`Unsupported`), never silently lowered as one element;
      `MappingError::UnknownArchetype`.
- [x] **Conditional templates** (LSF2 §15.4, ISSUES P40):
      `MappingBuilder::template_node_when`; Mox's `include` family.
- [x] **Default member visibility** (LSF2 `default_visibility`):
      `LangPolicy::default_member_visibility` (see "Added in review").
- [x] **Keyword type names** (LSF2 §31): role-less keywords (`array`,
      `callable`) and literal-role keywords (`null`, `false`, `true`) are
      their registered `type_name` in a type.
- [x] Tests (`tests/receivers.rs`, `tests/break_levels.rs`,
      `tests/modifiers_and_conditions.rs`, a fourth random-tree policy),
      benchmarks `oop/*` and `oop_off/*`, docs, LSF2 §31.

Dependency wiring (decided here, recorded per the anti-deferral rule):
- **bcgen-lang 0.4 and bvm-lang `=2.0.0-alpha.4`: dev-dependencies**
  (registry), to run lowered Mox end to end. Not runtime dependencies:
  compiling is a later stage (L5), and lower-lang's output must not depend on
  it. One `hir_lang` 0.4 in the tree.
- Unchanged: syntax-lang 1, hir-lang 0.4, intern-lang 1 wired; span-lang,
  macro-lang, diag-lang not wired; lang-forge 1.0.1 and resolve-lang 0.4
  dev-dependencies (resolve-lang 0.4.1 is what `"0.4"` selects; the
  `new C` test needs it). No new runtime dependency.

Added in review (same patch):
- [x] **Default member visibility** (LSF2 `default_visibility`, added to
      §15.2): `LangPolicy::default_member_visibility`, `Private` by default
      (0.4.0's behaviour); Mox sets `Public`. A class with no visibility
      keyword and `parent::__construct(1)` runs end to end.
- [x] **`new ClassName`** through the sketch's `new_object` template runs end
      to end: resolve-lang 0.4.1 resolves a class name in value position
      under `Policy::php()` (`with_class_values`); no lower-lang change was
      needed, and the test that pinned the 0.4.0 refusal is now a positive
      one.

Found while building 0.4.1 and **not** done here (moved to v0.5.0, reasons
recorded):
- **Spread entries in array literals** need an HIR form (no `MapEntry`
  spread in hir-lang 0.4); reported precisely meanwhile.
- **Precise keyword types**: HIR has no `null`, `false`, `true`, `array`, or
  `callable` type terms, and `TypeName` is not `#[non_exhaustive]`, so they
  map to the nearest `TypeName` (Mox: `any`). `self` / `static` / `parent`
  as types (root-only paths) are not lowered as types.

## v0.5.0 - Implementation
- [x] Quasi-quoted HIR templates with ${field} holes and hygienic expansion — delivered programmatically in v0.2.0.
- [x] Budgets; diagnostics with spans; benchmarks — delivered in v0.2.0.
- [x] **Textual templates** in HIR's surface syntax (`while = "loop { if ${cond} {} else { break }; ${body} }"`):
      **delivered in v0.4.0 per decision D26** (lower-lang parses the
      template subset of HIR spec §20 itself, ahead of hir-lang 0.5).
      Remaining here: check this front end against hir-lang 0.5's textual
      parser when it ships (same syntax, one spec), and fix whichever side
      differs.
- [ ] **LSF2 field labels from lang-forge 2.0**: an adapter implementing
      `Labeler` over lang-forge 2.0's label data (the `Pick::Label` path is
      implemented and tested with a hand-written labeler; lang-forge 1.x trees
      carry no labels).
- [ ] Gaps found while building 0.2 (not in the 0.2 brief; recorded so they are
      not lost): recursive-closure `self_binder`; foreign functions (`abi`);
      template closures that capture and declare effects; `Problem` →
      `diag_lang::Diagnostic` if the app wants it here. (Trait conflict
      rules, nested import trees, string interpolation, and attributes were
      delivered in v0.3.0.)
- [ ] Gaps found while building 0.3: by-reference `foreach` bindings under PHP
      scoping (needs an HIR reference-binding assignment, reported as
      `CannotHoist` today); attribute name resolution (kept as written).
- [ ] Moved from v0.4.1 (see that section for the reasons): spread entries in
      array literals (needs HIR); precise keyword types and `self`/`static`/
      `parent` as types (needs HIR type terms; `TypeName` additions wait for
      a minor release).

## v0.9.0 - Hardening
- [ ] Fuzzing over random trees **and random mappings** (0.2 fuzzes trees over
      fixed mappings only); audit against LexerSketch target-language sketches
      (Mox, HQL scalar expressions, Kraken subset, Iron, Mercury).

## v1.0.0 - Stable
- [ ] Frozen after the LexerSketch app lowers at least three target languages through it (D18).
