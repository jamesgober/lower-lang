# lower-lang - Roadmap

> Path from scaffold to a stable 1.0. Hard parts are front-loaded; each phase has hard exit criteria.
> Master plan: ../_lexersketch/ROADMAP.md and ../_lexersketch/NEW-LIBS.md
>
> **Anti-deferral rule:** no listed hard task moves to a later phase unless this file records the move and the reason.

## v0.1.0 - Scaffold (DONE)
Compiles, CI green, structure correct, no domain logic.
- [x] Manifest, README, CHANGELOG, REPS, dual license, CI, deny, clippy, rustfmt, DIRECTIVES, ROADMAP.

## v0.2.0 - Foundation
- [ ] Archetype catalogue and mapping model; lowering driver over syntax-lang trees; error-node handling.
- [ ] Wires syntax-lang and hir-lang. Property tests: origins cover every node, no panics on arbitrary trees.

## v0.5.0 - Implementation
- [ ] Quasi-quoted HIR templates with ${field} holes and hygienic expansion.
- [ ] Budgets; diagnostics with spans; benchmarks.

## v0.9.0 - Hardening
- [ ] Fuzzing over random trees and mappings; audit against LexerSketch target-language sketches.

## v1.0.0 - Stable
- [ ] Frozen after the LexerSketch app lowers at least three target languages through it (D18).
