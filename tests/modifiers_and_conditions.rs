//! Per-archetype modifiers (`MappingBuilder::modifier_in`; LSF2 `"..." = {
//! param = "rest", arg = "spread" }`), conditional templates
//! (`MappingBuilder::template_node_when`; LSF2 conditional `{ when = …,
//! template = … }` alternatives, ISSUES P40), and keyword type names.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

#[path = "support/mox_oop.rs"]
mod oop;

use hir_lang::{ArgKind, Expr, Hir, ItemKind, NodeRef, ParamKind, Res, Ty};
use intern_lang::Interner;
use lower_lang::{Arch, MappingBuilder, MappingError, Modifier, Pick, ProblemKind, TypeName};
use oop::{W, X, lines};

/// `function name(params) { return $first; }`
fn returns_first(w: &mut W, name: &str, params: &[&str], first: &str) {
    w.function(X::FnDecl, &[], name, params, |w| {
        w.ret(|w| {
            w.var(first);
        });
    });
}

/// The kinds of every parameter list and every argument list, in order.
fn kinds(hir: &Hir) -> (Vec<Vec<ParamKind>>, Vec<Vec<ArgKind>>) {
    let mut params = Vec::new();
    let mut args = Vec::new();
    hir_lang::walk(hir, |n| match n {
        NodeRef::Item(i) => {
            if let ItemKind::Fn(def) = hir.item(i).kind {
                params.push(
                    hir.list(def.params)
                        .iter()
                        .map(|&p| hir.param(p).kind)
                        .collect(),
                );
            }
        }
        NodeRef::Expr(e) => {
            if let Expr::Call { args: a, .. } = hir.expr(e) {
                args.push(hir.list(*a).iter().map(|a| a.kind).collect());
            }
        }
        _ => {}
    });
    (params, args)
}

#[test]
fn test_one_token_is_a_rest_parameter_and_a_spread_argument() {
    let m = oop::mapping();
    let (out, _, _) = oop::lower(&m, |w| {
        returns_first(w, "f", &["a", "...rest"], "a");
        w.stmt(|w| {
            w.call(
                |w| {
                    w.t(X::Ident, "f");
                },
                2,
                &[false, true],
                |w, i| {
                    if i == 0 {
                        w.num(1);
                    } else {
                        w.var("xs");
                    }
                },
            );
        });
    });
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let (params, args) = kinds(&out.hir);
    assert_eq!(params, vec![vec![ParamKind::Normal, ParamKind::Rest]]);
    assert_eq!(args, vec![vec![ArgKind::Positional, ArgKind::Spread]]);
}

#[test]
fn test_rest_and_spread_run_end_to_end() {
    // function first($a, ...$rest) { return $a; }   echo first(1, 2, 3);
    // function add3($a, $b, $c) { return $a + $b + $c; }
    // $v = [10, 20, 30]; echo add3(...$v);
    let r = oop::run(|w| {
        returns_first(w, "first", &["a", "...rest"], "a");
        w.function(X::FnDecl, &[], "add3", &["a", "b", "c"], |w| {
            w.ret(|w| {
                w.bin(
                    X::Plus,
                    |w| {
                        w.bin(
                            X::Plus,
                            |w| {
                                w.var("a");
                            },
                            |w| {
                                w.var("b");
                            },
                        );
                    },
                    |w| {
                        w.var("c");
                    },
                );
            });
        });
        w.echo(|w| {
            w.call(
                |w| {
                    w.t(X::Ident, "first");
                },
                3,
                &[],
                |w, i| {
                    w.num(i as i64 + 1);
                },
            );
        });
        w.stmt(|w| {
            w.assign(
                |w| {
                    w.var("v");
                },
                |w| {
                    w.open(X::ArrayLit).t(X::LBracket, "[");
                    for (i, n) in [10, 20, 30].into_iter().enumerate() {
                        if i > 0 {
                            w.t(X::Comma, ",");
                        }
                        w.open(X::ArrayItem).num(n).close();
                    }
                    w.t(X::RBracket, "]").close();
                },
            );
        });
        w.echo(|w| {
            w.call(
                |w| {
                    w.t(X::Ident, "add3");
                },
                1,
                &[true],
                |w, _| {
                    w.var("v");
                },
            );
        });
    });
    assert_eq!(r, lines(&["1", "60"]));
}

#[test]
fn test_spread_in_an_array_literal_is_refused_precisely() {
    // `[...$a, 1]`: HIR has no spread entry; dropping the marker would make
    // `$a` one element, so the entry is reported and becomes an error.
    let m = oop::mapping();
    let (out, _, src) = oop::lower(&m, |w| {
        w.stmt(|w| {
            w.open(X::ArrayLit).t(X::LBracket, "[");
            w.open(X::ArrayItem).t(X::Dots, "...").var("a").close();
            w.t(X::Comma, ",");
            w.open(X::ArrayItem).num(1).close();
            w.t(X::RBracket, "]").close();
        });
    });
    let found: Vec<_> = out
        .problems
        .iter()
        .map(|p| {
            (
                p.kind,
                src[p.span.start().to_usize()..p.span.end().to_usize()].trim(),
            )
        })
        .collect();
    assert_eq!(found, vec![(ProblemKind::Unsupported, "... $a")]);
}

#[test]
fn test_per_archetype_entries_win_and_fall_back() {
    let mut b = MappingBuilder::<u8>::new();
    b.modifier(1, Modifier::Ref)
        .modifier_in(1, "param", Modifier::Rest)
        .modifier_in(2, "arg", Modifier::Spread);
    let m = b.build().expect("valid");
    assert_eq!(m.modifier_in(&1, "param"), Some(Modifier::Rest));
    assert_eq!(m.modifier_in(&1, "arg"), Some(Modifier::Ref));
    assert_eq!(m.modifier_for(&1), Some(Modifier::Ref));
    assert_eq!(m.modifier_in(&2, "arg"), Some(Modifier::Spread));
    assert_eq!(m.modifier_in(&2, "param"), None);
    assert_eq!(m.modifier_in(&3, "param"), None);
    // The names are `Arch::lsf_name`s.
    let param: Arch<u8> = Arch::Param {
        pat: Pick::Nth(0),
        ty: Pick::Absent,
        default: Pick::Absent,
    };
    assert_eq!(m.modifier_in(&1, param.lsf_name()), Some(Modifier::Rest));
}

#[test]
fn test_per_archetype_errors() {
    let mut b = MappingBuilder::<u8>::new();
    b.modifier_in(1, "param", Modifier::Rest)
        .modifier_in(1, "parameter", Modifier::Rest);
    assert_eq!(
        b.build().err(),
        Some(MappingError::UnknownArchetype { index: 1 })
    );

    let mut b = MappingBuilder::<u8>::new();
    b.modifier_in(1, "param", Modifier::Rest)
        .modifier_in(2, "param", Modifier::Rest)
        .modifier_in(1, "param", Modifier::Ref);
    assert_eq!(
        b.build().err(),
        Some(MappingError::DuplicateKind {
            table: "archetype modifier",
            index: 2
        })
    );
    // One kind in several archetypes is the point.
    let mut b = MappingBuilder::<u8>::new();
    b.modifier_in(1, "param", Modifier::Rest)
        .modifier_in(1, "arg", Modifier::Spread)
        .modifier_in(1, "map_entry", Modifier::Spread);
    assert!(b.build().is_ok());
    assert_eq!(
        MappingError::UnknownArchetype { index: 1 }.to_string(),
        "per-archetype modifier 1 names no archetype (expected an LSF2 `arch` name such as `param` or `arg`)"
    );
}

/// The host symbol each `include`-family node lowered to, in order.
fn externs(hir: &Hir, names: &Interner) -> Vec<String> {
    let mut out = Vec::new();
    hir_lang::walk(hir, |n| {
        if let NodeRef::Expr(e) = n {
            if let Expr::Call { callee, .. } = hir.expr(e) {
                if let Expr::Path(p) = hir.expr(*callee) {
                    if let Res::Extern(s) = hir.path(*p).res {
                        out.push((
                            hir.origin(n).span.start(),
                            names.resolve(s).unwrap_or("?").to_string(),
                        ));
                    }
                }
            }
        }
    });
    out.sort();
    out.into_iter().map(|(_, s)| s).collect()
}

#[test]
fn test_conditional_templates_pick_the_include_family() {
    let m = oop::mapping();
    let (out, names, _) = oop::lower(&m, |w| {
        for (kw, text) in [
            (X::IncludeKw, "include"),
            (X::IncludeOnceKw, "include_once"),
            (X::RequireKw, "require"),
            (X::RequireOnceKw, "require_once"),
        ] {
            w.stmt(|w| {
                w.open(X::Include)
                    .t(kw, text)
                    .t(X::Str, "'lib.mox'")
                    .close();
            });
        }
    });
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    assert_eq!(
        externs(&out.hir, &names),
        vec![
            "mox.include",
            "mox.include_once",
            "mox.require",
            "mox.require_once"
        ]
    );
    // Each instantiation records its own template expansion.
    let expansions: Vec<String> = (1..=out.hir.count(hir_lang::IdKind::Expansion) as u32)
        .filter_map(|i| out.hir.expansion(hir_lang::ExpnId::from_u32(i)))
        .map(|x| names.resolve(x.name).unwrap_or("?").to_string())
        .collect();
    assert_eq!(
        expansions,
        vec!["include", "include_once", "require", "require_once"]
    );
}

#[test]
fn test_conditional_templates_follow_registration_order_and_check_holes() {
    // The first rule whose `when` finds a child wins; the unconditional one
    // is the fallback, as with `node_when`.
    const NODE: u8 = 10;
    const A: u8 = 1;
    const B: u8 = 2;
    let mut b = MappingBuilder::<u8>::new();
    let ta = b.parse_template("ta", "extern:a(${x})").unwrap();
    let tb = b.parse_template("tb", "extern:b(${x})").unwrap();
    b.template_node_when(NODE, Pick::Kind(A), ta, &[Pick::Back(0)]);
    b.template_node_when(NODE, Pick::Kind(B), tb, &[Pick::Back(0)]);
    let m = b.build().expect("valid");
    let (start, end) = (m.rule_for(&NODE), m.rule_for(&99));
    assert!(matches!(start, Some(Arch::Template(t)) if t.template() == ta));
    assert!(end.is_none());

    // Hole counts are checked for conditional uses too.
    let mut b = MappingBuilder::<u8>::new();
    let ta = b.parse_template("ta", "extern:a(${x}, ${y})").unwrap();
    b.template_node_when(NODE, Pick::Kind(A), ta, &[Pick::Back(0)]);
    assert_eq!(
        b.build().err(),
        Some(MappingError::HoleCount {
            rule: 0,
            expected: 2,
            found: 1
        })
    );
    // With no rule that applies, the node is unmapped.
    let mut b = MappingBuilder::<X>::new();
    let ta = b.parse_template("ta", "extern:a(${x})").unwrap();
    b.node(
        X::File,
        Arch::Module {
            name: Pick::Absent,
            items: Pick::Nodes,
        },
    );
    b.template_node_when(X::Include, Pick::Kind(X::RequireKw), ta, &[Pick::Back(0)]);
    b.token(
        X::Str,
        lower_lang::TokenRole::Str(lower_lang::Escapes::Quotes),
    );
    let m = b.build().expect("valid");
    let (out, _, _) = oop::lower(&m, |w| {
        w.open(X::Include)
            .t(X::IncludeKw, "include")
            .t(X::Str, "'x'")
            .close();
    });
    assert_eq!(
        out.problems.iter().map(|p| p.kind).collect::<Vec<_>>(),
        vec![ProblemKind::Unmapped]
    );
}

/// The type of every parameter, in order.
fn param_tys(hir: &Hir) -> Vec<Option<Ty>> {
    let mut out = Vec::new();
    hir_lang::walk(hir, |n| {
        if let NodeRef::Param(p) = n {
            out.push(hir.param(p).ty.map(|t| *hir.ty(t)));
        }
    });
    out
}

#[test]
fn test_keyword_type_names() {
    // `function f(array $a, callable $c, null $n)`: `array` and `callable`
    // are keyword tokens without a role, `null` has the literal role; all
    // three are type names through `[semantics.type_names]`.
    let m = oop::mapping();
    let (out, _, _) = oop::lower(&m, |w| {
        w.open(X::FnDecl)
            .t(X::FunctionKw, "function")
            .t(X::Ident, "f");
        w.t(X::LParen, "(").open(X::ParamList);
        for (i, (kw, text, var)) in [
            (X::ArrayKw, "array", "a"),
            (X::CallableKw, "callable", "c"),
            (X::NullKw, "null", "n"),
        ]
        .into_iter()
        .enumerate()
        {
            if i > 0 {
                w.t(X::Comma, ",");
            }
            w.open(X::TypedParam)
                .open(X::SpecialType)
                .t(kw, text)
                .close()
                .var(var)
                .close();
        }
        w.close().t(X::RParen, ")");
        w.block(|_| {});
        w.close();
    });
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    assert_eq!(param_tys(&out.hir), vec![Some(Ty::Any); 3]);
}

#[test]
fn test_literal_keywords_without_a_type_name_stay_refused_in_types() {
    // Without a `null` entry, `null` in a type is not a type (0.4.0
    // behaviour).
    let mut b = MappingBuilder::<X>::new();
    b.node(
        X::File,
        Arch::Module {
            name: Pick::Absent,
            items: Pick::Nodes,
        },
    );
    b.node(
        X::SpecialType,
        Arch::Path {
            segments: Pick::All,
            global: Pick::Absent,
            qself: Pick::Absent,
            qtrait: Pick::Absent,
        },
    );
    b.node(
        X::TypedParam,
        Arch::Param {
            pat: Pick::Kind(X::Var),
            ty: Pick::Kind(X::SpecialType),
            default: Pick::Absent,
        },
    );
    b.node(
        X::FnDecl,
        Arch::Fn {
            name: Pick::After(X::FunctionKw),
            generics: Pick::Absent,
            params: Pick::Kind(X::ParamList),
            ret: Pick::Absent,
            throws: Pick::Absent,
            body: Pick::Kind(X::Block),
            where_: Pick::Absent,
        },
    );
    b.container(X::ParamList);
    b.node(
        X::Block,
        Arch::Block {
            stmts: Pick::Nodes,
            tail: Pick::Absent,
            label: Pick::Absent,
            tail_from_last: false,
        },
    );
    b.token(X::NullKw, lower_lang::TokenRole::Null);
    b.token(X::Var, lower_lang::TokenRole::Var { sigil: 1 });
    b.token(X::Ident, lower_lang::TokenRole::Name);
    b.type_name("array", TypeName::Any);
    let m = b.build().expect("valid");
    let (out, _, _) = oop::lower(&m, |w| {
        w.open(X::FnDecl)
            .t(X::FunctionKw, "function")
            .t(X::Ident, "f");
        w.t(X::LParen, "(").open(X::ParamList);
        w.open(X::TypedParam)
            .open(X::SpecialType)
            .t(X::NullKw, "null")
            .close()
            .var("n")
            .close();
        w.close().t(X::RParen, ")");
        w.block(|_| {});
        w.close();
    });
    assert!(
        out.problems
            .iter()
            .any(|p| matches!(p.kind, ProblemKind::NotAllowedHere { .. })),
        "{:?}",
        out.problems
    );
}
