//! Numeric break levels (`LangPolicy::break_levels = Numeric`, LSF2
//! `break_levels = "numeric"`): PHP's `break N` / `continue N` lowered to
//! labelled HIR jumps, checked by shape, by problem, and end to end through
//! resolve-lang 0.4, bcgen-lang 0.4, and bvm-lang 2.0.0-alpha.4.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

#[path = "support/mox_oop.rs"]
mod oop;

use hir_lang::{BinderId, BinderKind, Expr, Hir, NodeRef};
use intern_lang::Interner;
use lower_lang::{BreakLevels, LangPolicy, ProblemKind};
use oop::{W, X, lines};

/// `$name = n;`
fn set(w: &mut W, name: &str, n: i64) {
    w.stmt(|w| {
        w.assign(
            |w| {
                w.var(name);
            },
            |w| {
                w.num(n);
            },
        );
    });
}

/// `$name = $name + 1;`
fn inc(w: &mut W, name: &str) {
    w.stmt(|w| {
        w.assign(
            |w| {
                w.var(name);
            },
            |w| {
                w.bin(
                    X::Plus,
                    |w| {
                        w.var(name);
                    },
                    |w| {
                        w.num(1);
                    },
                );
            },
        );
    });
}

/// `$name < n`.
fn below(w: &mut W, name: &str, n: i64) {
    w.bin(
        X::Lt,
        |w| {
            w.var(name);
        },
        |w| {
            w.num(n);
        },
    );
}

/// `loop { body }` (an `Arch::Loop` node).
fn loop_(w: &mut W, body: impl FnOnce(&mut W)) {
    w.open(X::Loop).t(X::LoopKw, "loop");
    w.block(body);
    w.close();
}

/// The two-loop program: `continue` (plain), `continue 2`, and `break 2`.
///
/// ```php
/// $i = 0;
/// while ($i < 3) {
///     $i = $i + 1; $j = 0;
///     while ($j < 3) {
///         $j = $j + 1;
///         if ($j < 2) { continue; }
///         if ($i < 2) { continue 2; }
///         echo $i; echo $j + 100;
///         break 2;
///     }
///     echo 999;
/// }
/// echo $i + 1000;
/// ```
fn nested(w: &mut W) {
    set(w, "i", 0);
    w.while_(
        |w| below(w, "i", 3),
        |w| {
            inc(w, "i");
            set(w, "j", 0);
            w.while_(
                |w| below(w, "j", 3),
                |w| {
                    inc(w, "j");
                    w.if_(
                        |w| below(w, "j", 2),
                        |w| {
                            w.jump(X::ContinueKw, None);
                        },
                    );
                    w.if_(
                        |w| below(w, "i", 2),
                        |w| {
                            w.jump(X::ContinueKw, Some("2"));
                        },
                    );
                    w.echo(|w| {
                        w.var("i");
                    });
                    w.echo(|w| {
                        w.bin(
                            X::Plus,
                            |w| {
                                w.var("j");
                            },
                            |w| {
                                w.num(100);
                            },
                        );
                    });
                    w.jump(X::BreakKw, Some("2"));
                },
            );
            w.echo(|w| {
                w.num(999);
            });
        },
    );
    w.echo(|w| {
        w.bin(
            X::Plus,
            |w| {
                w.var("i");
            },
            |w| {
                w.num(1000);
            },
        );
    });
}

#[test]
fn test_break_and_continue_levels_run_end_to_end() {
    // i = 1: j = 1 continues, j = 2 continues the outer loop (no 999);
    // i = 2: j = 1 continues, j = 2 prints and breaks both loops.
    assert_eq!(oop::run(nested), lines(&["2", "102", "1002"]));
}

#[test]
fn test_break_one_targets_the_innermost_loop_end_to_end() {
    // $n = 0; loop { while ($n < 5) { $n = $n + 1; if ($n < 3) { continue 1; } break 1; } echo $n; break; }
    let r = oop::run(|w| {
        set(w, "n", 0);
        loop_(w, |w| {
            w.while_(
                |w| below(w, "n", 5),
                |w| {
                    inc(w, "n");
                    w.if_(
                        |w| below(w, "n", 3),
                        |w| {
                            w.jump(X::ContinueKw, Some("1"));
                        },
                    );
                    w.jump(X::BreakKw, Some("1"));
                },
            );
            w.echo(|w| {
                w.var("n");
            });
            w.jump(X::BreakKw, None);
        });
    });
    assert_eq!(r, lines(&["3"]));
}

/// Every `break`/`continue` with its label binder, and every loop with
/// its label, in source order.
fn jumps_and_loops(hir: &Hir) -> (Vec<Option<BinderId>>, Vec<Option<BinderId>>) {
    let mut jumps = Vec::new();
    let mut loops = Vec::new();
    hir_lang::walk(hir, |n| {
        if let NodeRef::Expr(e) = n {
            let at = hir.origin(n).span.start();
            match hir.expr(e) {
                Expr::Break { label, .. } | Expr::Continue { label } => jumps.push((at, *label)),
                Expr::Loop { label, .. } => loops.push((at, *label)),
                _ => {}
            }
        }
    });
    jumps.sort_by_key(|j| j.0);
    loops.sort_by_key(|l| l.0);
    (
        jumps.into_iter().map(|j| j.1).collect(),
        loops.into_iter().map(|l| l.1).collect(),
    )
}

#[test]
fn test_levels_become_hygienic_labels_on_the_targeted_loops() {
    let m = oop::mapping();
    let (out, names, _) = oop::lower(&m, nested);
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let (jumps, loops) = jumps_and_loops(&out.hir);
    // `continue;` stays plain; `continue 2` and `break 2` target the outer
    // loop, which got a label; the inner loop needed none.
    assert_eq!(loops.len(), 2);
    let outer = loops[0].expect("the outer loop is labelled");
    assert_eq!(loops[1], None);
    // The `while` template also writes a `break` (its condition test).
    let user: Vec<_> = jumps.iter().filter(|j| j.is_some()).collect();
    assert_eq!(user, vec![&Some(outer), &Some(outer)]);
    let binder = out.hir.binder(outer).expect("binder");
    assert_eq!(binder.kind, BinderKind::Label);
    assert!(!binder.name.mark.is_root(), "a hygienic label");
    assert_eq!(names.resolve(binder.name.sym), Some("break_level"));
    // And the resolver accepts it.
    let (_, diags) = oop::resolve(&out, &names);
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn test_one_label_per_loop_however_many_jumps_target_it() {
    let m = oop::mapping();
    let (out, _, _) = oop::lower(&m, |w| {
        loop_(w, |w| {
            loop_(w, |w| {
                w.jump(X::BreakKw, Some("2"));
                w.jump(X::ContinueKw, Some("2"));
                w.jump(X::BreakKw, Some("1"));
                w.jump(X::ContinueKw, Some("1"));
            });
        });
    });
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let (jumps, loops) = jumps_and_loops(&out.hir);
    let (outer, inner) = (loops[0].expect("labelled"), loops[1].expect("labelled"));
    assert_ne!(outer, inner);
    assert_eq!(
        jumps,
        vec![Some(outer), Some(outer), Some(inner), Some(inner)]
    );
}

/// The problems of a program, with the text each points at.
fn problems(src: &str, out: &lower_lang::Lowered) -> Vec<(ProblemKind, String)> {
    out.problems
        .iter()
        .map(|p| {
            (
                p.kind,
                src[p.span.start().to_usize()..p.span.end().to_usize()].to_string(),
            )
        })
        .collect()
}

#[test]
fn test_bad_levels_are_precise_problems() {
    let m = oop::mapping();
    let (out, _, src) = oop::lower(&m, |w| {
        // Outside any loop.
        w.jump(X::BreakKw, Some("1"));
        loop_(w, |w| {
            loop_(w, |w| {
                w.jump(X::BreakKw, Some("3"));
                w.jump(X::ContinueKw, Some("0"));
                // Not an integer literal: a variable, a float.
                w.open(X::Break)
                    .t(X::BreakKw, "break")
                    .var("n")
                    .t(X::Semi, ";")
                    .close();
                w.jump(X::BreakKw, Some("1.5"));
                // Beyond u64.
                w.jump(X::BreakKw, Some("99999999999999999999999"));
                // A closure starts a new count.
                w.stmt(|w| {
                    w.closure(false, &[], &[], |w| {
                        w.jump(X::BreakKw, Some("1"));
                    });
                });
            });
        });
    });
    let got = problems(&src, &out);
    assert_eq!(
        got,
        vec![
            (ProblemKind::BreakLevel { level: 1, loops: 0 }, "1".into()),
            (ProblemKind::BreakLevel { level: 3, loops: 2 }, "3".into()),
            (ProblemKind::BreakLevel { level: 0, loops: 2 }, "0".into()),
            (ProblemKind::BreakLevel { level: 0, loops: 2 }, "$n".into()),
            (ProblemKind::BreakLevel { level: 0, loops: 2 }, "1.5".into()),
            // PHP promotes the literal to a float, which is no level.
            (
                ProblemKind::BreakLevel { level: 0, loops: 2 },
                "99999999999999999999999".into()
            ),
            (ProblemKind::BreakLevel { level: 1, loops: 0 }, "1".into()),
        ]
    );
    // The jumps became error expressions; the HIR is still valid.
    assert_eq!(out.hir.validate(), Ok(()));
    assert_eq!(
        ProblemKind::BreakLevel { level: 0, loops: 2 }.to_string(),
        "a break or continue level must be a positive integer literal"
    );
    assert_eq!(
        ProblemKind::BreakLevel { level: 2, loops: 0 }.to_string(),
        "cannot break or continue 2 levels: no loop encloses it in this function"
    );
    assert_eq!(
        ProblemKind::BreakLevel { level: 2, loops: 1 }.to_string(),
        "cannot break or continue 2 levels: only 1 loop encloses it"
    );
}

#[test]
fn test_level_spellings() {
    // `break (2)`, `break 0x2`, `break 0b10`: PHP accepts any integer
    // literal (parentheses are no node in PHP's AST).
    let m = oop::mapping();
    for spelling in ["paren", "0x2", "0b10", "02"] {
        let (out, _, _) = oop::lower(&m, |w| {
            loop_(w, |w| {
                loop_(w, |w| {
                    w.open(X::Break).t(X::BreakKw, "break");
                    if spelling == "paren" {
                        w.open(X::Paren)
                            .t(X::LParen, "(")
                            .num(2)
                            .t(X::RParen, ")")
                            .close();
                    } else {
                        w.t(X::Num, spelling);
                    }
                    w.t(X::Semi, ";").close();
                });
            });
        });
        assert!(out.problems.is_empty(), "{spelling}: {:?}", out.problems);
        let (jumps, loops) = jumps_and_loops(&out.hir);
        assert_eq!(jumps, vec![loops[0]], "{spelling}");
        assert!(loops[0].is_some());
    }
}

#[test]
fn test_labels_mode_is_unchanged() {
    // Under `BreakLevels::Labels` (the default, and 0.4.0's behaviour) a
    // number in the label position is not a label name: `break 2` is a
    // plain `break`, and no loop gets a label.
    let m = oop::mapping_with(true, false);
    let (out, _, _) = oop::lower(&m, |w| {
        loop_(w, |w| {
            loop_(w, |w| {
                w.jump(X::BreakKw, Some("2"));
            });
        });
    });
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let (jumps, loops) = jumps_and_loops(&out.hir);
    assert_eq!(jumps, vec![None]);
    assert_eq!(loops, vec![None, None]);
    assert_eq!(LangPolicy::default().break_levels, BreakLevels::Labels);
    assert_eq!(LangPolicy::php().break_levels, BreakLevels::Labels);
}

#[test]
fn test_numeric_levels_are_deterministic() {
    let m = oop::mapping();
    let (a, na, _) = oop::lower(&m, nested);
    let (b, nb, _) = oop::lower(&m, nested);
    assert_eq!(hir_lang::print(&a.hir, &na), hir_lang::print(&b.hir, &nb));
}

#[test]
fn test_deeply_nested_loops_stay_iterative() {
    // 5,000 nested loops with `break 5000` in the middle: no recursion on
    // the call stack, and the jump reaches the outermost loop.
    let depth = 5_000;
    let mut w = W::new();
    w.open(X::File);
    for _ in 0..depth {
        w.open(X::Loop)
            .t(X::LoopKw, "loop")
            .open(X::Block)
            .t(X::LBrace, "{");
    }
    w.jump(X::BreakKw, Some(&depth.to_string()));
    for _ in 0..depth {
        w.t(X::RBrace, "}").close().close();
    }
    w.close();
    let (tree, src) = w.finish();
    let m = oop::mapping();
    let mut names = Interner::new();
    let out = std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(move || {
            let out = lower_lang::lower(&tree, &src, &m, &mut names).expect("within budget");
            (out.problems.len(), jumps_and_loops(&out.hir))
        })
        .expect("spawns")
        .join()
        .expect("no overflow");
    let (problems, (jumps, loops)) = out;
    assert_eq!(problems, 0);
    assert_eq!(loops.len(), depth);
    assert_eq!(jumps, vec![loops[0]]);
    assert!(loops[1..].iter().all(Option::is_none));
}

// ------------------------------------------------- reference: jump targets

use proptest::prelude::*;

/// A random nest of loops, closures, and jumps.
#[derive(Clone, Debug)]
enum T {
    Loop(Vec<T>),
    Closure(Vec<T>),
    /// `break`/`continue` with an optional level (0..=5).
    Jump(bool, Option<u8>),
}

fn nest() -> impl Strategy<Value = Vec<T>> {
    let leaf = (any::<bool>(), prop::option::of(0u8..=5)).prop_map(|(b, l)| T::Jump(b, l));
    let t = leaf.prop_recursive(6, 64, 4, |inner| {
        prop_oneof![
            3 => prop::collection::vec(inner.clone(), 0..4).prop_map(T::Loop),
            1 => prop::collection::vec(inner, 0..4).prop_map(T::Closure),
        ]
    });
    prop::collection::vec(t, 0..4)
}

/// What the reference expects of each jump, in source order.
#[derive(Debug, PartialEq, Eq)]
enum Want {
    /// No level: a plain jump.
    Plain,
    /// No level and no loop around it in its function: hir-lang's lenient
    /// validation repairs it (and reports it).
    Repaired,
    /// The loop with this source-order index.
    Loop(usize),
    /// A `BreakLevel` problem.
    Bad { level: u64, loops: u32 },
}

/// The reference: loops are numbered in source order; each function body
/// (the file, a closure) starts an empty loop stack.
fn expected(items: &[T]) -> Vec<Want> {
    enum Step<'a> {
        Visit(&'a T),
        Pop,
        Restore(Vec<usize>),
    }
    let mut out = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut next = 0;
    let mut work: Vec<Step<'_>> = items.iter().rev().map(Step::Visit).collect();
    while let Some(step) = work.pop() {
        match step {
            Step::Visit(T::Jump(_, None)) => out.push(if stack.is_empty() {
                Want::Repaired
            } else {
                Want::Plain
            }),
            Step::Visit(T::Jump(_, Some(l))) => {
                let l = usize::from(*l);
                out.push(if l >= 1 && l <= stack.len() {
                    Want::Loop(stack[stack.len() - l])
                } else {
                    Want::Bad {
                        level: l as u64,
                        loops: stack.len() as u32,
                    }
                });
            }
            Step::Visit(T::Loop(body)) => {
                stack.push(next);
                next += 1;
                work.push(Step::Pop);
                work.extend(body.iter().rev().map(Step::Visit));
            }
            Step::Visit(T::Closure(body)) => {
                work.push(Step::Restore(std::mem::take(&mut stack)));
                work.extend(body.iter().rev().map(Step::Visit));
            }
            Step::Pop => {
                stack.pop();
            }
            Step::Restore(s) => stack = s,
        }
    }
    out
}

fn write_nest(w: &mut W, items: &[T]) {
    for t in items {
        match t {
            T::Loop(body) => loop_(w, |w| write_nest(w, body)),
            T::Closure(body) => {
                w.stmt(|w| {
                    w.closure(false, &[], &[], |w| write_nest(w, body));
                });
            }
            T::Jump(is_break, level) => {
                let kind = if *is_break { X::BreakKw } else { X::ContinueKw };
                w.jump(kind, level.map(|l| l.to_string()).as_deref());
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// Every `break N` / `continue N` targets exactly the loop the
    /// reference counts to, or is the problem the reference predicts; plain
    /// jumps stay plain; only targeted loops get labels; the HIR is valid
    /// and resolves.
    #[test]
    fn prop_levels_match_the_reference(items in nest()) {
        let m = oop::mapping();
        let (out, names, _) = oop::lower(&m, |w| write_nest(w, &items));
        let want = expected(&items);
        let (jumps, loops) = jumps_and_loops(&out.hir);
        // Bad levels are error expressions, not jumps: line the HIR jumps
        // and the problems up with the reference in source order.
        let mut hir_jumps = jumps.into_iter();
        let mut problems = out.problems.iter();
        let mut targeted = std::collections::BTreeSet::new();
        for w in &want {
            match w {
                Want::Plain => prop_assert_eq!(hir_jumps.next(), Some(None)),
                Want::Repaired => {
                    let p = problems.next().map(|p| p.kind);
                    prop_assert!(matches!(p, Some(ProblemKind::Hir(_))), "{:?}", p);
                }
                Want::Loop(i) => {
                    let label = hir_jumps.next().flatten();
                    prop_assert!(label.is_some());
                    prop_assert_eq!(label, loops[*i]);
                    targeted.insert(*i);
                }
                Want::Bad { level, loops: n } => {
                    let p = problems.next().map(|p| p.kind);
                    prop_assert_eq!(p, Some(ProblemKind::BreakLevel { level: *level, loops: *n }));
                }
            }
        }
        prop_assert_eq!(hir_jumps.next(), None);
        prop_assert_eq!(problems.next(), None);
        for (i, l) in loops.iter().enumerate() {
            prop_assert_eq!(l.is_some(), targeted.contains(&i));
        }
        prop_assert_eq!(out.hir.validate(), Ok(()));
        let (resolved, _) = oop::resolve(&out, &names);
        prop_assert_eq!(resolved.validate(), Ok(()));
    }
}
