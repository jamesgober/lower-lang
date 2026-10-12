//! Textual templates end to end: every template text of the Mox sketch
//! (`_lexersketch/sketches/mox.lsf`, `[semantics.templates]`) is parsed and
//! registered the way LexerSketch's `ls-sema` does (LSF2 §15.11: used
//! built-ins first, then the texts in document order), Mox trees using
//! them are lowered, and the HIR is validated and resolved through
//! resolve-lang 0.4: `&&`, `||`, `++`, `--`, `echo`, `print`, `.=`, `??=`,
//! `**=`, `yield from`, keyed `yield`, and a union parameter type.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;
#[path = "support/mox.rs"]
mod mox;

use hir_lang::{BinderKind, Expr, Hir, IdKind, Intrinsic, NodeRef, Prim, Res, Ty};
use intern_lang::Interner;
use lower_lang::{
    Arch, HoleKind, Lowered, Mapping, MappingBuilder, OpSpec, Pick, Template, TemplateId, TypeName,
    templates,
};
use mox::{M, W};

/// `[semantics.templates]` of `mox.lsf` (2026-10-11), verbatim.
const MOX_TEMPLATES: [(&str, &str); 19] = [
    ("bool_and", "if ${lhs} { not(not(${rhs})) } else { false }"),
    ("bool_or", "if ${lhs} { true } else { not(not(${rhs})) }"),
    ("pre_increment", "{ ${place} = add(${place}, 1); ${place} }"),
    ("pre_decrement", "{ ${place} = sub(${place}, 1); ${place} }"),
    (
        "post_decrement",
        "{ let old = ${place}; ${place} = sub(old, 1); old }",
    ),
    ("new_object", "extern:mox.new(${class}, ${...args})"),
    ("echo", "extern:mox.echo(${...values})"),
    ("echo_text", "extern:mox.echo(${...text})"),
    ("isset", "extern:mox.isset(${...args})"),
    ("empty", "extern:mox.empty(${arg})"),
    ("unset", "extern:mox.unset(${...places})"),
    ("exit", "extern:mox.exit(${...value})"),
    ("print", "extern:mox.print(${value})"),
    ("include", "extern:mox.include(${path})"),
    ("include_once", "extern:mox.include_once(${path})"),
    ("require", "extern:mox.require(${path})"),
    ("require_once", "extern:mox.require_once(${path})"),
    ("spawn_task", "spawn(closure[by-value]() => ${operand})"),
    (
        "yield_from",
        "\n{ let g = extern:mox.iter(${from});\n  loop {\n    match extern:mox.next(g) {\n      null => break extern:mox.gen_return(g),\n      (k, v) => { yield (k, v); }\n    }\n  } }\n",
    ),
];

/// How the sketch's `++`/`--` templates should read (GAPS B2): the place is
/// named once with `let_place`, so `++$a[f()]` calls `f` once.
const PLACE_ONCE: [(&str, &str); 3] = [
    ("pre_increment", "let_place p = ${place} in p = add(p, 1)"),
    ("pre_decrement", "let_place p = ${place} in p = sub(p, 1)"),
    (
        "post_decrement",
        "let_place p = ${place} in { let old = p; p = sub(old, 1); old }",
    ),
];

fn text(name: &str) -> &'static str {
    MOX_TEMPLATES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, t)| *t)
        .unwrap()
}

/// The Mox mapping with its sugar from template texts and its operator
/// table from `[semantics.ops]`; `place_once` picks the `let_place` forms of
/// `++`/`--` over the sketch's current ones.
fn mapping(place_once: bool) -> Mapping<M> {
    let mut b = MappingBuilder::new();
    let concat = mox::structure(&mut b);
    // Built-ins the sketch uses by name.
    let coalesce = b.template(templates::coalesce()).unwrap();
    let post_increment = b.template(templates::post_increment()).unwrap();
    // `[semantics.templates]`, in document order.
    let mut ids: Vec<(&str, TemplateId)> = Vec::new();
    for (name, t) in MOX_TEMPLATES {
        let t = if place_once {
            PLACE_ONCE
                .iter()
                .find(|(n, _)| *n == name)
                .map_or(t, |(_, once)| *once)
        } else {
            t
        };
        ids.push((name, b.parse_template(name, t).unwrap()));
    }
    let id = |name: &str| ids.iter().find(|(n, _)| *n == name).unwrap().1;
    let pow = b.name("mox.pow");

    // `[semantics.lower]`: holes take the node's fields of the same name.
    b.template_node(M::Echo, id("echo"), &[Pick::AllAfter(M::EchoKw)]);
    b.template_node(M::Print, id("print"), &[Pick::After(M::PrintKw)]);
    b.template_node(M::YieldFrom, id("yield_from"), &[Pick::After(M::FromKw)]);
    b.node(
        M::YieldKv,
        Arch::Yield {
            key: Pick::Before(M::FatArrow),
            value: Pick::After(M::FatArrow),
        },
    );
    b.node(M::Prefix, Arch::prefix());
    b.node(
        M::Postfix,
        Arch::Postfix {
            operand: Pick::Nth(0),
            op: Pick::Nth(1),
        },
    );
    b.node(
        M::AssignOp,
        Arch::Assign {
            target: Pick::Nth(0),
            op: Pick::Nth(1),
            value: Pick::Nth(2),
        },
    );
    b.node(
        M::TypedParam,
        Arch::Param {
            pat: Pick::Kind(M::Var),
            ty: Pick::NthNode(0),
            default: Pick::Absent,
        },
    );
    b.node(
        M::UnionTy,
        Arch::UnionTy {
            members: Pick::AllKind(M::Ident),
        },
    );
    b.skip(M::Pipe);
    b.type_name("int", TypeName::Prim(Prim::I64));
    b.type_name("string", TypeName::Prim(Prim::Str));
    b.type_name("float", TypeName::Prim(Prim::F64));

    // `[semantics.ops]`.
    b.binary_op(M::Eq, OpSpec::Assign);
    b.binary_op(M::Plus, OpSpec::Op(hir_lang::OpKind::Add));
    b.binary_op(M::Dot, OpSpec::Intrinsic(concat));
    b.binary_op(M::DotEq, OpSpec::CompoundIntrinsic(concat));
    b.binary_op(M::PowEq, OpSpec::CompoundIntrinsic(pow));
    b.binary_op(M::CoalesceEq, OpSpec::CompoundTemplate(coalesce));
    b.binary_op(M::AndAnd, OpSpec::Template(id("bool_and")));
    b.binary_op(M::OrOr, OpSpec::Template(id("bool_or")));
    b.prefix_op(M::PlusPlus, OpSpec::Template(id("pre_increment")));
    b.postfix_op(M::PlusPlus, OpSpec::Template(post_increment));
    b.prefix_op(M::MinusMinus, OpSpec::Template(id("pre_decrement")));
    b.postfix_op(M::MinusMinus, OpSpec::Template(id("post_decrement")));
    b.build().unwrap()
}

fn lower(w: W, mapping: &Mapping<M>) -> (Lowered, Interner, String) {
    let (tree, src) = w.finish();
    let mut names = Interner::new();
    let out = lower_lang::lower(&tree, &src, mapping, &mut names).expect("within budget");
    assert_eq!(out.hir.validate(), Ok(()));
    assert!(common::origins_within(&out.hir, src.len()), "origins");
    (out, names, src)
}

fn exprs(hir: &Hir) -> Vec<Expr> {
    let mut v = Vec::new();
    hir_lang::walk(hir, |n| {
        if let NodeRef::Expr(e) = n {
            v.push(*hir.expr(e));
        }
    });
    v
}

fn stmt(w: &mut W, body: impl FnOnce(&mut W)) {
    w.open(M::ExprStmt);
    body(w);
    w.t(M::Semi, ";").close();
}

/// `$a[$k]`
fn index(w: &mut W, base: &str, key: &str) {
    w.open(M::Index)
        .var(base)
        .t(M::LBracket, "[")
        .var(key)
        .t(M::RBracket, "]")
        .close();
}

/// ```php
/// function f(int|string|float $k, $a) {
///     $a[$k] .= "!";
///     $a[$k] ??= 0;
///     $n = 2; $n **= 3;
///     $i = 0;
///     ++$i; $i++; --$i; $i--;
///     echo $i, $a;
///     print $i && $a || $k;
///     return $i;
/// }
/// function g($xs, $k, $v) { yield from $xs; yield $k => $v; }
/// ```
fn program() -> W {
    let mut w = W::new();
    w.open(M::File);
    w.open(M::FnDecl)
        .t(M::Function, "function")
        .t(M::Ident, "f")
        .t(M::LParen, "(");
    w.open(M::Params);
    w.open(M::TypedParam).open(M::UnionTy);
    w.t(M::Ident, "int")
        .t(M::Pipe, "|")
        .t(M::Ident, "string")
        .t(M::Pipe, "|")
        .t(M::Ident, "float")
        .close();
    w.var("k").close().t(M::Comma, ",");
    w.open(M::Param).var("a").close();
    w.close().t(M::RParen, ")");
    w.open(M::Block).t(M::LBrace, "{");
    // $a[$k] .= "!";
    stmt(&mut w, |w| {
        w.open(M::Assign);
        index(w, "a", "k");
        w.t(M::DotEq, ".=").t(M::Str, "\"!\"").close();
    });
    // $a[$k] ??= 0;   (through the `assign` archetype's operator field)
    stmt(&mut w, |w| {
        w.open(M::AssignOp);
        index(w, "a", "k");
        w.t(M::CoalesceEq, "??=").t(M::Num, "0").close();
    });
    // $n = 2; $n **= 3;
    stmt(&mut w, |w| {
        w.open(M::Assign)
            .var("n")
            .t(M::Eq, "=")
            .t(M::Num, "2")
            .close();
    });
    stmt(&mut w, |w| {
        w.open(M::Assign)
            .var("n")
            .t(M::PowEq, "**=")
            .t(M::Num, "3")
            .close();
    });
    // $i = 0;
    stmt(&mut w, |w| {
        w.open(M::Assign)
            .var("i")
            .t(M::Eq, "=")
            .t(M::Num, "0")
            .close();
    });
    // ++$i; $i++; --$i; $i--;
    stmt(&mut w, |w| {
        w.open(M::Prefix).t(M::PlusPlus, "++").var("i").close();
    });
    stmt(&mut w, |w| {
        w.open(M::Postfix).var("i").t(M::PlusPlus, "++").close();
    });
    stmt(&mut w, |w| {
        w.open(M::Prefix).t(M::MinusMinus, "--").var("i").close();
    });
    stmt(&mut w, |w| {
        w.open(M::Postfix).var("i").t(M::MinusMinus, "--").close();
    });
    // echo $i, $a;
    w.open(M::Echo)
        .t(M::EchoKw, "echo")
        .var("i")
        .t(M::Comma, ",")
        .var("a")
        .t(M::Semi, ";")
        .close();
    // print $i && $a || $k;
    stmt(&mut w, |w| {
        w.open(M::Print).t(M::PrintKw, "print");
        w.open(M::Binary).open(M::Binary);
        w.var("i").t(M::AndAnd, "&&").var("a").close();
        w.t(M::OrOr, "||").var("k").close();
        w.close();
    });
    // return $i;
    w.open(M::Return)
        .t(M::ReturnKw, "return")
        .var("i")
        .t(M::Semi, ";")
        .close();
    w.t(M::RBrace, "}").close().close();

    // function g($xs, $k, $v) { yield from $xs; yield $k => $v; }
    w.open(M::FnDecl)
        .t(M::Function, "function")
        .t(M::Ident, "g")
        .t(M::LParen, "(");
    w.open(M::Params);
    for (i, p) in ["xs", "k", "v"].iter().enumerate() {
        if i > 0 {
            w.t(M::Comma, ",");
        }
        w.open(M::Param).var(p).close();
    }
    w.close().t(M::RParen, ")");
    w.open(M::Block).t(M::LBrace, "{");
    stmt(&mut w, |w| {
        w.open(M::YieldFrom)
            .t(M::YieldKw, "yield")
            .t(M::FromKw, "from")
            .var("xs")
            .close();
    });
    stmt(&mut w, |w| {
        w.open(M::YieldKv)
            .t(M::YieldKw, "yield")
            .var("k")
            .t(M::FatArrow, "=>")
            .var("v")
            .close();
    });
    w.t(M::RBrace, "}").close().close();
    w.close();
    w
}

#[test]
fn test_every_mox_template_text_parses_and_registers() {
    let mut b = MappingBuilder::<M>::new();
    for (name, t) in MOX_TEMPLATES.iter().chain(PLACE_ONCE.iter()) {
        let id = b.parse_template(name, t).unwrap();
        let _ = id;
    }
    let m = b.build().unwrap();
    // Hole kinds follow from position.
    let echo = Template::parse("echo", text("echo")).unwrap();
    assert_eq!(echo.hole(0), Some(("values", HoleKind::Args, true)));
    let yf = Template::parse("yield_from", text("yield_from")).unwrap();
    assert_eq!(yf.hole(0), Some(("from", HoleKind::Expr, false)));
    let new = Template::parse("new_object", text("new_object")).unwrap();
    assert_eq!(new.hole_index("class"), Some(0));
    assert_eq!(new.hole_index("args"), Some(1));
    let _ = m;
}

#[test]
fn test_mox_sugar_lowers_validates_and_resolves() {
    let mapping = mapping(true);
    let (out, mut names, src) = lower(program(), &mapping);
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let es = exprs(&out.hir);

    // Every place is evaluated once: `.=`, `??=`, `**=`, and the four
    // increments each name their place with `let_place`, and `$a[$k]` is
    // indexed exactly as often as the source writes it.
    let let_places = es
        .iter()
        .filter(|e| matches!(e, Expr::LetPlace { .. }))
        .count();
    assert_eq!(let_places, 7);
    let indexes = es
        .iter()
        .filter(|e| matches!(e, Expr::Index { .. }))
        .count();
    assert_eq!(indexes, src.matches("$a [ $k ]").count());
    // `.=` and `**=` go through their intrinsics, `??=` through `coalesce`.
    let concat = names.intern("mox.concat");
    let pow = names.intern("mox.pow");
    let intrinsics: Vec<_> = es
        .iter()
        .filter_map(|e| match e {
            Expr::Intrinsic {
                kind: Intrinsic::Named(s),
                ..
            } => Some(*s),
            _ => None,
        })
        .collect();
    assert!(intrinsics.contains(&concat) && intrinsics.contains(&pow));
    // `&&` and `||` return booleans: `not(not(...))` twice.
    let nots = es
        .iter()
        .filter(|e| matches!(e, Expr::Op { op, .. } if op.kind == hir_lang::OpKind::Not))
        .count();
    assert_eq!(nots, 4);
    // `yield from` through the template's delegation loop, keyed `yield`.
    assert!(es.iter().any(|e| matches!(
        e,
        Expr::Yield {
            key: Some(_),
            value: Some(_)
        }
    )));
    assert!(es.iter().any(|e| matches!(e, Expr::Loop { .. })));

    let printed = hir_lang::print(&out.hir, &names);
    assert!(printed.contains("(let-place p%"), "{printed}");
    assert!(printed.contains("extern mox.echo"), "{printed}");
    assert!(printed.contains("extern mox.print"), "{printed}");
    // The union parameter type, normalized by hir-lang (sorted members).
    assert!(
        (0..out.hir.count(IdKind::Ty)).any(|i| matches!(
            out.hir.ty(hir_lang::TyId::from_index(i).unwrap()),
            Ty::Union(_)
        )),
        "{printed}"
    );

    // The real resolver: clean, valid, and hygienic.
    let (hir, diags) = common::resolve(out.hir, &names, resolve_lang::Policy::php());
    assert!(
        diags.is_empty(),
        "{:?}",
        diags.iter().map(|d| d.message(&names)).collect::<Vec<_>>()
    );
    assert_eq!(hir.validate(), Ok(()));
    assert!(common::hygienic(&hir));
    // Every value path is resolved: locals, the place aliases, externs.
    let mut place_uses = 0;
    for i in 0..hir.count(IdKind::Path) {
        let p = hir.path(hir_lang::PathId::from_index(i).unwrap());
        assert!(!p.res.is_unresolved(), "an unresolved path");
        assert!(!matches!(p.res, Res::Err), "an unresolvable path");
        if let Res::Local(b) = p.res {
            if hir.binder(b).unwrap().kind == BinderKind::Place {
                place_uses += 1;
                assert!(!hir.binder(b).unwrap().name.mark.is_root());
            }
        }
    }
    // Each alias is read and written: 2 per compound, 2 or 3 per increment.
    assert!(place_uses >= 14, "{place_uses}");
}

/// `++$a[f()]`: the sketch's current `pre_increment` copies the place three
/// times (calling `f` three times); the `let_place` form evaluates it once.
#[test]
fn test_let_place_evaluates_the_place_once() {
    let write = || {
        let mut w = W::new();
        w.open(M::File);
        stmt(&mut w, |w| {
            w.open(M::Prefix).t(M::PlusPlus, "++");
            w.open(M::Index).var("a").t(M::LBracket, "[");
            w.open(M::Call)
                .t(M::Ident, "f")
                .t(M::LParen, "(")
                .open(M::Args)
                .close()
                .t(M::RParen, ")")
                .close();
            w.t(M::RBracket, "]").close().close();
        });
        w.close();
        w
    };
    let calls = |out: &Lowered| {
        exprs(&out.hir)
            .iter()
            .filter(|e| matches!(e, Expr::Call { .. }))
            .count()
    };
    let (copied, _, _) = lower(write(), &mapping(false));
    let (once, names, _) = lower(write(), &mapping(true));
    assert_eq!(calls(&copied), 3, "the sketch's text copies the place");
    assert_eq!(calls(&once), 1, "let_place names it once");
    let (hir, diags) = common::resolve(once.hir, &names, resolve_lang::Policy::php());
    assert_eq!(hir.validate(), Ok(()));
    assert!(common::hygienic(&hir));
    // `$a` and `f` are defined nowhere: one diagnostic each, never three.
    let messages: Vec<String> = diags.iter().map(|d| d.message(&names)).collect();
    assert_eq!(
        messages,
        [
            "cannot find value `a` in this scope",
            "cannot find value `f` in this scope"
        ]
    );
}

/// The inline form of `[semantics.lower]` (LSF2 §15.7): a text registered
/// under the node's name, its holes bound to the node's fields by name.
#[test]
fn test_inline_lower_text_binds_holes_by_name() {
    let mut b = MappingBuilder::<M>::new();
    let _ = mox::structure(&mut b);
    let inline = "extern:mox.print(${value})";
    let id = b.parse_template("print_expr", inline).unwrap();
    // The field picks the forge knows, by field name.
    let fields = [("value", Pick::After(M::PrintKw))];
    let t = Template::parse("print_expr", inline).unwrap();
    let picks: Vec<Pick<M>> = (0..t.hole_count())
        .map(|i| {
            let (name, _, _) = t.hole(i).unwrap();
            fields
                .iter()
                .find(|(f, _)| *f == name)
                .map_or(Pick::Absent, |(_, p)| *p)
        })
        .collect();
    b.template_node(M::Print, id, &picks);
    b.binary_op(M::Eq, OpSpec::Assign);
    let m = b.build().unwrap();
    let mut w = W::new();
    w.open(M::File);
    stmt(&mut w, |w| {
        w.open(M::Print)
            .t(M::PrintKw, "print")
            .t(M::Num, "1")
            .close();
    });
    w.close();
    let (out, names, _) = lower(w, &m);
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    assert!(hir_lang::print(&out.hir, &names).contains("extern mox.print"));
}
