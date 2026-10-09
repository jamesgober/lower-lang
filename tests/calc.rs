//! The worked example end to end: a calc language forged by lang-forge from an
//! `.lsf` schematic, parsed into a lossless tree, lowered through a
//! lower-lang mapping, validated by hir-lang, and resolved by a toy resolver.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{calc_language, calc_mapping, lower_calc, origins_within, resolve_locals};
use hir_lang::{ExpnKind, IdKind, ItemKind};
use intern_lang::Interner;
use lower_lang::ProblemKind;

const FACT: &str = "\
fn fact(n) {
    let acc = 1;
    while n > 1 {
        acc = acc * n;
        n = n - 1;
    }
    return acc;
}
let r = fact(5);
if r > 100 && r < 1000 { print(r); } else { print(0 - r); }
";

#[test]
fn test_calc_program_lowers_validates_and_resolves() {
    let lang = calc_language();
    let mapping = calc_mapping(&lang);
    let mut names = Interner::new();
    let out = lower_calc(&lang, &mapping, FACT, &mut names);
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    assert_eq!(out.hir.validate(), Ok(()));
    assert!(origins_within(&out.hir, FACT.len()));

    let printed = hir_lang::print(&out.hir, &names);
    assert!(printed.contains("(fn fact"), "{printed}");
    assert!(printed.contains("(loop"), "{printed}");
    assert!(printed.contains("(op mul overflow=error"), "{printed}");
    assert!(printed.contains("(op gt"), "{printed}");
    assert!(printed.contains("(return"), "{printed}");

    // The module has one item (`fact`) and a top-level body (`let r`, `if`).
    let ItemKind::Module { items, body, .. } = out.hir.item(out.hir.root()).kind else {
        panic!("root is a module");
    };
    assert_eq!(items.len(), 1);
    assert!(body.is_some());

    // `while` and `&&` were expanded through templates: one expansion each.
    let kinds: Vec<ExpnKind> = (1..=out.hir.count(IdKind::Expansion) as u32)
        .filter_map(|i| {
            out.hir
                .expansion(hir_lang::ExpnId::from_u32(i))
                .map(|e| e.kind)
        })
        .collect();
    assert_eq!(kinds, [ExpnKind::Desugar, ExpnKind::Desugar]);

    // Every local reference resolves by the HIR scope rules; only `fact`
    // (an item) and `print` (a host function) stay for the real resolver.
    let mut hir = out.hir;
    let (resolved, open) = resolve_locals(&mut hir);
    assert_eq!(open, 3, "fact, print, print");
    assert_eq!(resolved, 11);
    assert_eq!(hir.validate(), Ok(()));
}

#[test]
fn test_half_typed_calc_still_lowers_with_every_problem() {
    let lang = calc_language();
    let mapping = calc_mapping(&lang);
    let mut names = Interner::new();
    // A missing operand, a stray `)`, a `while` with no condition, an `if`
    // with no block, and a `break` that the language does not have.
    let src = "let a = ;\nlet b = 1 + );\nwhile { a = 1; }\nfn f(x) { return x * ; }\nif a\n";
    let out = lower_calc(&lang, &mapping, src, &mut names);
    assert_eq!(out.hir.validate(), Ok(()));
    assert!(origins_within(&out.hir, src.len()));
    assert!(!out.problems.is_empty());
    // Problems are in source order and inside the source.
    let starts: Vec<u32> = out
        .problems
        .iter()
        .map(|p| p.span.start().to_u32())
        .collect();
    let mut sorted = starts.clone();
    sorted.sort_unstable();
    assert_eq!(starts, sorted);
    assert!(
        out.problems
            .iter()
            .all(|p| p.span.end().to_usize() <= src.len())
    );
    // The well-formed parts are still there: `f` is an item.
    let printed = hir_lang::print(&out.hir, &names);
    assert!(printed.contains("(fn f"), "{printed}");
    assert!(out.problems.iter().any(|p| matches!(
        p.kind,
        ProblemKind::SyntaxError
            | ProblemKind::MissingField { .. }
            | ProblemKind::MissingHole { .. }
    )));
}

#[test]
fn test_every_prefix_of_a_program_lowers() {
    // Typing the program one character at a time: every prefix is half-typed
    // code, and every one must lower to valid HIR with in-source origins.
    let lang = calc_language();
    let mapping = calc_mapping(&lang);
    for end in 0..=FACT.len() {
        let src = &FACT[..end];
        let mut names = Interner::new();
        let out = lower_calc(&lang, &mapping, src, &mut names);
        assert_eq!(out.hir.validate(), Ok(()), "prefix {end}");
        assert!(origins_within(&out.hir, src.len()), "prefix {end}");
    }
}
