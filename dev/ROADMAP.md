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

## v0.5.0 - Implementation
- [x] Quasi-quoted HIR templates with ${field} holes and hygienic expansion — delivered programmatically in v0.2.0.
- [x] Budgets; diagnostics with spans; benchmarks — delivered in v0.2.0.
- [ ] **Textual templates** in HIR's surface syntax (`while = "loop { if ${cond} {} else { break }; ${body} }"`):
      a pure front end that parses the text into the existing `TemplateBuilder`
      AST. **Moved here from the brief's v0.2.0 scope** because the textual
      syntax (HIR spec §20) ships with hir-lang 0.5; the template AST was
      designed so this needs no change to `Template`.
- [ ] **LSF2 field labels from lang-forge 2.0**: an adapter implementing
      `Labeler` over lang-forge 2.0's label data (the `Pick::Label` path is
      implemented and tested with a hand-written labeler; lang-forge 1.x trees
      carry no labels).
- [ ] Gaps found while building 0.2 (not in the 0.2 brief; recorded so they are
      not lost): PHP trait conflict rules (`insteadof` / `as`, HIR `MixinRule`);
      nested import trees (`use a::{b, c}`); string interpolation; attributes
      (HIR's attribute side table); recursive-closure `self_binder`; foreign
      functions (`abi`); template closures that capture and declare effects;
      `Problem` → `diag_lang::Diagnostic` if the app wants it here.

## v0.9.0 - Hardening
- [ ] Fuzzing over random trees **and random mappings** (0.2 fuzzes trees over
      fixed mappings only); audit against LexerSketch target-language sketches
      (Mox, HQL scalar expressions, Kraken subset, Iron, Mercury).

## v1.0.0 - Stable
- [ ] Frozen after the LexerSketch app lowers at least three target languages through it (D18).
