//! The 0.3 lowering semantics over a small generic language: hoisting for
//! every binding pattern (and the precise problems for the ones that cannot
//! hoist), function-scoped template pattern holes, interpolation joins,
//! text-keyed operator tables, the literal decoding table under non-PHP
//! policies, raw strings and heredoc tokens, use trees with globs and
//! `self`, and the new mapping errors.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod common;
#[path = "support/writer.rs"]
mod writer;

use hir_lang::{BindMode, Expr, IdKind, ItemKind, Lit, OpKind, Pat, PathRoot, Stmt};
use intern_lang::Interner;
use lower_lang::{
    Arch, Escapes, HoistBlocker, Join, LangPolicy, LegacyOctal, Lowered, Mapping, MappingBuilder,
    MappingError, Modifier, OpSpec, Pick, ProblemKind, Separators, TemplateBuilder, TokenRole,
};
use writer::{K, W};

// Kinds: nodes 100+, tokens below.
const FILE: K = K(100);
const FN: K = K(101);
const BLOCK: K = K(102);
const VAR: K = K(103); // `var PAT = INIT;` (hoisted)
const TUPLE: K = K(104);
const BINARY: K = K(105);
const PREFIX: K = K(106);
const STMT: K = K(107);
const REF_BIND: K = K(108);
const TWICE: K = K(109); // a template binding its pattern hole twice
const INTERP_CALL: K = K(110);
const INTERP_TPL: K = K(111);
const INTERP_FOLD_TPL: K = K(112);
const USE: K = K(113);
const PATH: K = K(114);
const GROUP: K = K(115);
const ASSIGN: K = K(116);
const PASS: K = K(117);
const ATTR: K = K(118);
const FOREACH: K = K(119); // `foreach SRC [KEY =>] VALUE BLOCK`
const TRY: K = K(120);
const CATCH: K = K(121);

const IDENT: K = K(1);
const NUM: K = K(2);
const OP: K = K(3); // every operator
const STR_PART: K = K(4);
const RAW: K = K(5);
const HEREDOC: K = K(6);
const STAR: K = K(7);
const SELF_KW: K = K(8);
const CRATE: K = K(9);
const COLONS: K = K(10);
const AS: K = K(11);
const AMP: K = K(12);
const EQ: K = K(13);
const SEMI: K = K(14);
const LBRACE: K = K(15);
const RBRACE: K = K(16);
const COMMA: K = K(17);
const FN_KW: K = K(18);
const VAR_KW: K = K(19);
const PLUS_KIND: K = K(20); // a dedicated `+` kind, mapped by kind
const FATARROW: K = K(22);

fn mapping(policy: LangPolicy) -> Mapping<K> {
    let mut b = MappingBuilder::new();
    b.policy(policy);
    b.node(
        FILE,
        Arch::Module {
            name: Pick::Absent,
            items: Pick::Nodes,
        },
    );
    b.node(
        FN,
        Arch::Fn {
            name: Pick::Kind(IDENT),
            generics: Pick::Absent,
            params: Pick::Absent,
            ret: Pick::Absent,
            throws: Pick::Absent,
            body: Pick::Kind(BLOCK),
            where_: Pick::Absent,
        },
    );
    b.node(
        BLOCK,
        Arch::Block {
            stmts: Pick::Nodes,
            tail: Pick::Absent,
            label: Pick::Absent,
            tail_from_last: false,
        },
    );
    b.node(
        VAR,
        Arch::Let {
            pat: Pick::After(VAR_KW),
            ty: Pick::Absent,
            init: Pick::After(EQ),
            else_: Pick::Absent,
            mutable: true,
            hoist: true,
        },
    );
    b.node(TUPLE, Arch::Tuple { elems: Pick::All });
    b.node(
        REF_BIND,
        Arch::BindPat {
            name: Pick::Kind(IDENT),
            sub: Pick::Absent,
        },
    );
    b.node(BINARY, Arch::binary());
    b.node(PREFIX, Arch::prefix());
    b.node(STMT, Arch::ExprStmt { expr: Pick::Nth(0) });
    b.node(
        ASSIGN,
        Arch::Assign {
            target: Pick::Nth(0),
            op: Pick::Nth(1),
            value: Pick::Nth(2),
        },
    );
    b.node(
        PASS,
        Arch::Pass {
            inner: Pick::Nth(0),
        },
    );
    b.node(
        ATTR,
        Arch::Attr {
            name: Pick::Nth(0),
            args: Pick::Absent,
        },
    );
    // A template that binds its pattern hole in two arms.
    let twice = {
        let mut t = TemplateBuilder::new("twice");
        let v = t.expr_hole("value");
        let p = t.pat_hole("pat", false);
        let scrutinee = t.hole(v);
        let p1 = t.pat(p);
        let p2 = t.pat(p);
        let one = t.lit(lower_lang::TLit::Int(1));
        let two = t.lit(lower_lang::TLit::Int(2));
        let root = t.match_(scrutinee, &[(p1, None, one), (p2, None, two)]);
        b.template(t.finish(root).unwrap()).unwrap()
    };
    b.template_node(TWICE, twice, &[Pick::Nth(0), Pick::Nth(1)]);
    let each = b
        .template(lower_lang::templates::for_each("iter", "next"))
        .unwrap();
    b.template_node(
        FOREACH,
        each,
        &[
            Pick::Nth(0),
            Pick::Before(FATARROW),
            Pick::Before(BLOCK),
            Pick::Kind(BLOCK),
            Pick::Absent,
        ],
    );
    b.node(
        TRY,
        Arch::Try {
            body: Pick::Kind(BLOCK),
            catches: Pick::AllKind(CATCH),
            finally: Pick::Absent,
        },
    );
    b.node(
        CATCH,
        Arch::Catch {
            types: Pick::Absent,
            name: Pick::Kind(IDENT),
            body: Pick::Kind(BLOCK),
        },
    );
    let interp = b.name("interp");
    let concat = b.name("concat");
    let all = {
        let mut t = TemplateBuilder::new("join_all");
        let parts = t.args_hole("parts");
        let f = t.extern_("join_all");
        let s = t.splice(parts);
        let root = t.call(f, &[s]);
        b.template(t.finish(root).unwrap()).unwrap()
    };
    let pair = {
        let mut t = TemplateBuilder::new("join2");
        let (l, r) = (t.expr_hole("l"), t.expr_hole("r"));
        let f = t.extern_("join2");
        let (l, r) = (t.hole(l), t.hole(r));
        let root = t.call(f, &[l, r]);
        b.template(t.finish(root).unwrap()).unwrap()
    };
    b.node(
        INTERP_CALL,
        Arch::Interp {
            parts: Pick::All,
            close: Pick::Absent,
            join: Join::Call(interp),
        },
    );
    b.node(
        INTERP_TPL,
        Arch::Interp {
            parts: Pick::All,
            close: Pick::Absent,
            join: Join::Template(all),
        },
    );
    b.node(
        INTERP_FOLD_TPL,
        Arch::Interp {
            parts: Pick::All,
            close: Pick::Absent,
            join: Join::Template(pair),
        },
    );
    let _ = concat;
    b.node(
        USE,
        Arch::UseTree {
            path: Pick::Nth(0),
            alias: Pick::After(AS),
            children: Pick::Kind(GROUP),
        },
    );
    b.node(
        PATH,
        Arch::Path {
            segments: Pick::All,
            global: Pick::Absent,
            qself: Pick::Absent,
            qtrait: Pick::Absent,
        },
    );
    b.container(GROUP);
    b.token(IDENT, TokenRole::Name)
        .token(NUM, TokenRole::Number)
        .token(STR_PART, TokenRole::StrPart(Escapes::Standard))
        .token(RAW, TokenRole::RawStr)
        .token(HEREDOC, TokenRole::Heredoc(Escapes::Standard));
    b.modifier(AMP, Modifier::Ref)
        .modifier(STAR, Modifier::Glob)
        .modifier(K(21), Modifier::Public);
    b.root(SELF_KW, PathRoot::SelfModule)
        .root(CRATE, PathRoot::Global);
    for k in [
        COLONS, SEMI, LBRACE, RBRACE, COMMA, FN_KW, VAR_KW, EQ, FATARROW,
    ] {
        b.skip(k);
    }
    // One generic operator kind, mapped by text; `+` also has its own kind.
    b.binary_op_text("+", OpSpec::Op(OpKind::Add))
        .binary_op_text("*", OpSpec::Op(OpKind::Mul))
        .binary_op_text("+=", OpSpec::Compound(OpKind::Add))
        .binary_op_text("-", OpSpec::Op(OpKind::Sub))
        .prefix_op_text("-", OpSpec::Op(OpKind::Neg))
        .binary_op(PLUS_KIND, OpSpec::Op(OpKind::Mul));
    b.build().expect("valid")
}

fn lower_with(w: W, policy: LangPolicy) -> (Lowered, Interner, String) {
    let (tree, src) = w.finish();
    let m = mapping(policy);
    let mut names = Interner::new();
    let out = lower_lang::lower(&tree, &src, &m, &mut names).expect("within budget");
    assert_eq!(out.hir.validate(), Ok(()));
    assert!(common::origins_within(&out.hir, src.len()), "origins");
    (out, names, src)
}

/// `fn main { BODY }`.
fn main_fn(w: &mut W, body: impl FnOnce(&mut W)) {
    w.open(FN).t(FN_KW, "fn").t(IDENT, "main");
    w.open(BLOCK).t(LBRACE, "{");
    body(w);
    w.t(RBRACE, "}").close().close();
}

fn kinds(out: &Lowered) -> Vec<ProblemKind> {
    out.problems.iter().map(|p| p.kind).collect()
}

// ------------------------------------------------------------- hoisting

#[test]
fn test_hoisting_works_for_destructuring_patterns() {
    // fn main { var (a, b) = (1, 2); a + b; }
    let mut w = W::new();
    w.open(FILE);
    main_fn(&mut w, |w| {
        w.open(VAR).t(VAR_KW, "var").open(TUPLE);
        w.t(IDENT, "a").t(COMMA, ",").t(IDENT, "b").close();
        w.t(EQ, "=")
            .open(TUPLE)
            .t(NUM, "1")
            .t(COMMA, ",")
            .t(NUM, "2")
            .close();
        w.t(SEMI, ";").close();
        w.open(STMT)
            .open(BINARY)
            .t(IDENT, "a")
            .t(OP, "+")
            .t(IDENT, "b")
            .close()
            .t(SEMI, ";")
            .close();
    });
    w.close();
    let (out, names, _) = lower_with(w, LangPolicy::default());
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let mut hir = out.hir;
    let printed = hir_lang::print(&hir, &names);
    // The function declares `a` and `b` up front; the `var` binds marked
    // temporaries and assigns them.
    let ItemKind::Module { items, .. } = hir.item(hir.root()).kind else {
        panic!()
    };
    let ItemKind::Fn(def) = hir.item(hir.list(items)[0]).kind else {
        panic!()
    };
    let Expr::Block(outer) = *hir.expr(def.body.unwrap()) else {
        panic!()
    };
    assert_eq!(outer.stmts.len(), 2, "{printed}");
    let Expr::Block(inner) = *hir.expr(outer.tail.unwrap()) else {
        panic!()
    };
    // let (t_a, t_b) = (1, 2); a = t_a; b = t_b; a + b;
    let stmts = hir.list(inner.stmts);
    assert_eq!(stmts.len(), 4, "{printed}");
    let Stmt::Let { pat, .. } = *hir.stmt(stmts[0]) else {
        panic!()
    };
    let Pat::Tuple { elems, .. } = *hir.pat(pat) else {
        panic!()
    };
    for &e in hir.list(elems) {
        let Pat::Bind { binder, .. } = *hir.pat(e) else {
            panic!()
        };
        assert!(!hir.binder(binder).unwrap().name.mark.is_root());
    }
    let (resolved, open) = common::resolve_locals(&mut hir);
    assert_eq!(open, 0);
    assert_eq!(resolved, 4, "two assignments, two reads");
}

#[test]
fn test_by_reference_bindings_cannot_hoist_and_are_reported() {
    // fn main { var (&a, b) = t; }
    let mut w = W::new();
    w.open(FILE);
    main_fn(&mut w, |w| {
        w.open(VAR).t(VAR_KW, "var").open(TUPLE);
        w.open(REF_BIND).t(AMP, "&").t(IDENT, "a").close();
        w.t(COMMA, ",").t(IDENT, "b").close();
        w.t(EQ, "=").t(IDENT, "t").t(SEMI, ";").close();
    });
    w.close();
    let (out, _, _) = lower_with(w, LangPolicy::default());
    assert_eq!(
        kinds(&out),
        [ProblemKind::CannotHoist {
            reason: HoistBlocker::ByReference
        }]
    );
    // `&a` stays a by-reference binding of the `let`; `b` is hoisted.
    let hir = &out.hir;
    let mut modes = Vec::new();
    for i in 0..hir.count(IdKind::Pat) {
        if let Pat::Bind { binder, mode, .. } = *hir.pat(hir_lang::PatId::from_index(i).unwrap()) {
            modes.push((mode, hir.binder(binder).unwrap().name.mark.is_root()));
        }
    }
    assert!(modes.contains(&(BindMode::Ref, true)), "{modes:?}");
    assert!(modes.contains(&(BindMode::Value, false)), "{modes:?}");
}

#[test]
fn test_a_pattern_hole_bound_twice_is_reported_under_php_scoping() {
    // fn main { twice(v, x) }  where the template binds `x` in two arms.
    let tree = || {
        let mut w = W::new();
        w.open(FILE);
        main_fn(&mut w, |w| {
            w.open(STMT)
                .open(TWICE)
                .t(IDENT, "v")
                .t(IDENT, "x")
                .close()
                .t(SEMI, ";")
                .close();
        });
        w.close();
        w
    };
    let w = tree();
    let (out, _, _) = lower_with(tree(), LangPolicy::php());
    assert_eq!(
        kinds(&out),
        [ProblemKind::CannotHoist {
            reason: HoistBlocker::ReusedHole
        }]
    );
    // Without PHP scoping the hole is simply copied.
    let (out, _, _) = lower_with(w, LangPolicy::default());
    assert!(out.problems.is_empty(), "{:?}", out.problems);
}

// --------------------------------------------------------- interpolation

#[test]
fn test_interpolation_joins_call_and_templates() {
    // "a{x}b" through Call, an argument-list template, and a two-hole template.
    let mut w = W::new();
    w.open(FILE);
    main_fn(&mut w, |w| {
        for k in [INTERP_CALL, INTERP_TPL, INTERP_FOLD_TPL] {
            w.open(STMT)
                .open(k)
                .t(STR_PART, "a")
                .t(IDENT, "x")
                .t(STR_PART, "b")
                .close()
                .t(SEMI, ";")
                .close();
        }
        // A lone expression, and a lone text.
        w.open(STMT).open(INTERP_CALL).t(IDENT, "y").close().close();
        w.open(STMT)
            .open(INTERP_CALL)
            .t(STR_PART, "only")
            .close()
            .close();
    });
    w.close();
    let (out, names, _) = lower_with(w, LangPolicy::default());
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let p = hir_lang::print(&out.hir, &names);
    // Call: interp("a", x, "b").
    assert!(
        p.contains("(use (path interp → extern interp))\n          (lit \"a\")\n          (use (path x))\n          (lit \"b\"))"),
        "{p}"
    );
    // Argument-list template: join_all("a", x, "b") in an expansion.
    assert!(p.contains("(use (path join_all'e"), "{p}");
    // Two-hole template, folded: join2(join2("a", x), "b").
    assert_eq!(p.matches("join2'e").count(), 2, "{p}");
    // A lone expression: interp("", y); a lone text: the literal.
    assert!(p.contains("(lit \"\")\n          (use (path y))"), "{p}");
    assert!(p.contains("(lit \"only\")"), "{p}");
}

// ------------------------------------------------------------- operators

#[test]
fn test_text_keyed_operator_tables() {
    // a + b * c;  -a;  a += 1;  a % b (unknown);  a P b (P mapped by kind)
    let mut w = W::new();
    w.open(FILE);
    main_fn(&mut w, |w| {
        w.open(STMT).open(BINARY).t(IDENT, "a").t(OP, "+");
        w.open(BINARY)
            .t(IDENT, "b")
            .t(OP, "*")
            .t(IDENT, "c")
            .close()
            .close()
            .t(SEMI, ";")
            .close();
        w.open(STMT)
            .open(PREFIX)
            .t(OP, "-")
            .t(IDENT, "a")
            .close()
            .t(SEMI, ";")
            .close();
        w.open(STMT)
            .open(ASSIGN)
            .t(IDENT, "a")
            .t(OP, "+=")
            .t(NUM, "1")
            .close()
            .t(SEMI, ";")
            .close();
        w.open(STMT)
            .open(BINARY)
            .t(IDENT, "a")
            .t(OP, "%")
            .t(IDENT, "b")
            .close()
            .t(SEMI, ";")
            .close();
        // A kind entry wins over the text entry for the same spelling.
        w.open(STMT)
            .open(BINARY)
            .t(IDENT, "a")
            .t(PLUS_KIND, "+")
            .t(IDENT, "b")
            .close()
            .t(SEMI, ";")
            .close();
    });
    w.close();
    let (out, names, _) = lower_with(w, LangPolicy::default());
    assert_eq!(kinds(&out), [ProblemKind::UnknownOperator]);
    let p = hir_lang::print(&out.hir, &names);
    assert!(p.contains("(op add overflow=error"), "{p}");
    assert_eq!(p.matches("(op mul overflow=error").count(), 2, "{p}");
    assert!(p.contains("(op neg overflow=error"), "{p}");
    assert!(p.contains("(assign add overflow=error"), "{p}");
}

#[test]
fn test_mapping_refuses_bad_text_operators_and_joins() {
    let mut b = MappingBuilder::<K>::new();
    b.binary_op_text("+", OpSpec::Op(OpKind::Add))
        .binary_op_text("+", OpSpec::Op(OpKind::Sub));
    assert!(matches!(
        b.build(),
        Err(MappingError::DuplicateKind {
            table: "binary operator text",
            index: 1
        })
    ));
    let mut b = MappingBuilder::<K>::new();
    b.prefix_op_text("", OpSpec::Op(OpKind::Neg));
    assert!(matches!(
        b.build(),
        Err(MappingError::EmptyOperatorText { .. })
    ));
    let mut b = MappingBuilder::<K>::new();
    b.binary_op_text("!", OpSpec::Op(OpKind::Neg));
    assert!(matches!(b.build(), Err(MappingError::BadOperator { .. })));
    let mut b = MappingBuilder::<K>::new();
    let w = b.template(lower_lang::templates::while_loop()).unwrap();
    b.node(
        K(100),
        Arch::Interp {
            parts: Pick::All,
            close: Pick::Absent,
            join: Join::Template(w),
        },
    );
    assert_eq!(b.build().err(), Some(MappingError::BadJoin { rule: 0 }));
}

// --------------------------------------------------------------- literals

fn literals(texts: &[(K, &str)], policy: LangPolicy) -> (Vec<Option<Lit>>, Lowered, Interner) {
    let mut w = W::new();
    w.open(FILE);
    main_fn(&mut w, |w| {
        for &(k, t) in texts {
            w.open(STMT).t(k, t).t(SEMI, ";").close();
        }
    });
    w.close();
    let (out, names, _) = lower_with(w, policy);
    let ItemKind::Module { items, .. } = out.hir.item(out.hir.root()).kind else {
        panic!()
    };
    let ItemKind::Fn(def) = out.hir.item(out.hir.list(items)[0]).kind else {
        panic!()
    };
    let Expr::Block(body) = *out.hir.expr(def.body.unwrap()) else {
        panic!()
    };
    let lits = out
        .hir
        .list(body.stmts)
        .iter()
        .map(|&s| match *out.hir.stmt(s) {
            Stmt::Expr(e) => match *out.hir.expr(e) {
                Expr::Lit(l) => Some(l),
                _ => None,
            },
            _ => None,
        })
        .collect();
    (lits, out, names)
}

#[test]
fn test_literal_table_defaults_and_options() {
    let texts = [
        (NUM, "0777"),
        (NUM, "1__0_"),
        (NUM, "0x1.8p3"),
        (NUM, "0x1p-1074"),
        (NUM, "0xFFFFFFFFFFFFFFFFFF"),
        (NUM, "99999999999999999999"),
        (NUM, "0x1p99999"),
    ];
    let (lits, out, names) = literals(&texts, LangPolicy::default());
    let hir = &out.hir;
    assert!(
        matches!(lits[0], Some(Lit::Int(i)) if i.value == 777),
        "decimal by default"
    );
    assert!(
        matches!(lits[1], Some(Lit::Int(i)) if i.value == 10),
        "Rust-style separators"
    );
    assert!(matches!(lits[2], Some(Lit::Float(f)) if f.value() == 12.0));
    assert!(matches!(lits[3], Some(Lit::Float(f)) if f.value() == f64::from_bits(1)));
    // Beyond 64 bits without promotion: exact big integers, any radix.
    let big = |l: Option<Lit>| match l {
        Some(Lit::BigInt(t)) => hir.str(t).to_owned(),
        _ => String::new(),
    };
    assert_eq!(big(lits[4]), "4722366482869645213695");
    assert_eq!(big(lits[5]), "99999999999999999999");
    assert_eq!(
        kinds(&out),
        [ProblemKind::InvalidLiteral],
        "the overflowing hex float"
    );
    let _ = names;

    // Python 3: a leading zero is an error (except all zeros); no `_`.
    let mut py = LangPolicy::default();
    py.literals.legacy_octal = LegacyOctal::Error;
    py.literals.separators = Separators::None;
    let (lits, out, _) = literals(&[(NUM, "0777"), (NUM, "00"), (NUM, "1_0")], py);
    assert!(lits[0].is_none() && lits[2].is_none());
    assert!(matches!(lits[1], Some(Lit::Int(i)) if i.value == 0));
    assert_eq!(out.problems.len(), 2);
}

#[test]
fn test_raw_strings_and_heredoc_tokens() {
    let texts = [
        (RAW, r##"r#"a "q" \n"#"##),
        (RAW, r#"r'\d+'"#),
        (RAW, r#"r"open"#),
        (HEREDOC, "<<<EOT\n    one\\t\n      two\n    EOT"),
        (HEREDOC, "<<<\"Q\"\nq\nQ"),
        (HEREDOC, "<<<EOT\nno end"),
        (HEREDOC, "<<<E\nE"),
    ];
    let (lits, out, _) = literals(&texts, LangPolicy::default());
    let hir = &out.hir;
    let s = |l: Option<Lit>| match l {
        Some(Lit::Str(t)) => Some(hir.str(t).to_owned()),
        _ => None,
    };
    assert_eq!(s(lits[0]).as_deref(), Some(r#"a "q" \n"#));
    assert_eq!(s(lits[1]).as_deref(), Some(r"\d+"));
    assert_eq!(s(lits[2]), None);
    assert_eq!(s(lits[3]).as_deref(), Some("one\t\n  two"));
    assert_eq!(s(lits[4]).as_deref(), Some("q"));
    assert_eq!(s(lits[5]), None);
    assert_eq!(s(lits[6]).as_deref(), Some(""));
    assert_eq!(
        kinds(&out),
        [ProblemKind::InvalidLiteral, ProblemKind::InvalidLiteral]
    );
}

// -------------------------------------------------------------- use trees

#[test]
fn test_use_trees_with_globs_self_and_roots() {
    // use crate::a::{self, b::*, c as d};
    let mut w = W::new();
    w.open(FILE).open(USE).open(PATH);
    w.t(CRATE, "crate").t(COLONS, "::").t(IDENT, "a").close();
    w.t(COLONS, "::").open(GROUP).t(LBRACE, "{");
    w.t(SELF_KW, "self").t(COMMA, ",");
    w.open(USE)
        .open(PATH)
        .t(IDENT, "b")
        .close()
        .t(COLONS, "::")
        .t(STAR, "*")
        .close();
    w.t(COMMA, ",");
    w.open(USE).t(IDENT, "c").t(AS, "as").t(IDENT, "d").close();
    w.t(RBRACE, "}").close().close().close();
    let (out, names, _) = lower_with(w, LangPolicy::default());
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let hir = &out.hir;
    let ItemKind::Module { items, .. } = hir.item(hir.root()).kind else {
        panic!()
    };
    let got: Vec<(PathRoot, String, bool, Option<String>)> = hir
        .list(items)
        .iter()
        .filter_map(|&i| {
            let it = hir.item(i);
            let ItemKind::Import { path, glob } = it.kind else {
                return None;
            };
            let p = hir.path(path);
            let segs: Vec<&str> = hir
                .list(p.segments)
                .iter()
                .map(|s| names.resolve(s.name.sym).unwrap())
                .collect();
            Some((
                p.root,
                segs.join("::"),
                glob,
                it.name.map(|n| names.resolve(n.sym).unwrap().to_owned()),
            ))
        })
        .collect();
    assert_eq!(
        got,
        [
            (PathRoot::Global, "a".to_owned(), false, None),
            (PathRoot::Global, "a::b".to_owned(), true, None),
            (
                PathRoot::Global,
                "a::c".to_owned(),
                false,
                Some("d".to_owned())
            ),
        ]
    );
}

#[test]
fn test_an_attribute_on_a_wrapper_is_misplaced() {
    // A pass-through node carrying an attribute has no node to attach it to.
    let mut w = W::new();
    w.open(FILE);
    main_fn(&mut w, |w| {
        w.open(STMT)
            .open(PASS)
            .open(ATTR)
            .t(IDENT, "inline")
            .close()
            .t(IDENT, "x")
            .close()
            .t(SEMI, ";")
            .close();
    });
    w.close();
    let (out, _, _) = lower_with(w, LangPolicy::default());
    assert_eq!(kinds(&out), [ProblemKind::MisplacedAttribute]);
}

// ============================================================ properties

use proptest::prelude::*;

/// A binding pattern: a name, a by-reference name, or a tuple.
#[derive(Clone, Debug)]
enum P {
    Name(u8),
    Ref(u8),
    Tuple(Vec<P>),
}

/// A statement of the function under test.
#[derive(Clone, Debug)]
enum S {
    /// `var PAT = src;`
    Var(P),
    /// `foreach xs [KEY =>] VALUE { BODY }`
    Foreach(Option<P>, P, Vec<S>),
    /// `try {} catch [NAME] { BODY }`
    Catch(Option<u8>, Vec<S>),
}

const NAMES: [&str; 4] = ["a", "b", "c", "d"];

fn pat() -> impl Strategy<Value = P> {
    let leaf = prop_oneof![
        4 => (0u8..4).prop_map(P::Name),
        1 => (0u8..4).prop_map(P::Ref),
    ];
    leaf.prop_recursive(3, 12, 3, |inner| {
        proptest::collection::vec(inner, 1..4).prop_map(P::Tuple)
    })
}

fn stmts() -> impl Strategy<Value = Vec<S>> {
    let leaf = pat().prop_map(S::Var);
    let s = leaf.prop_recursive(3, 16, 3, |inner| {
        prop_oneof![
            (
                proptest::option::of(pat()),
                pat(),
                proptest::collection::vec(inner.clone(), 0..3)
            )
                .prop_map(|(k, v, b)| S::Foreach(k, v, b)),
            (
                proptest::option::of(0u8..4),
                proptest::collection::vec(inner, 0..3)
            )
                .prop_map(|(n, b)| S::Catch(n, b)),
        ]
    });
    proptest::collection::vec(s, 0..5)
}

fn write_pat(w: &mut W, p: &P) {
    match p {
        P::Name(n) => {
            w.t(IDENT, NAMES[*n as usize]);
        }
        P::Ref(n) => {
            w.open(REF_BIND)
                .t(AMP, "&")
                .t(IDENT, NAMES[*n as usize])
                .close();
        }
        P::Tuple(ps) => {
            w.open(TUPLE);
            for (i, q) in ps.iter().enumerate() {
                if i > 0 {
                    w.t(COMMA, ",");
                }
                write_pat(w, q);
            }
            w.close();
        }
    }
}

fn write_stmts(w: &mut W, ss: &[S]) {
    for s in ss {
        match s {
            S::Var(p) => {
                w.open(VAR).t(VAR_KW, "var");
                write_pat(w, p);
                w.t(EQ, "=").t(IDENT, "src").t(SEMI, ";").close();
            }
            S::Foreach(k, v, body) => {
                w.open(FOREACH).t(IDENT, "xs");
                if let Some(k) = k {
                    write_pat(w, k);
                    w.t(FATARROW, "=>");
                }
                write_pat(w, v);
                w.open(BLOCK).t(LBRACE, "{");
                write_stmts(w, body);
                w.t(RBRACE, "}").close().close();
            }
            S::Catch(n, body) => {
                w.open(TRY)
                    .open(BLOCK)
                    .t(LBRACE, "{")
                    .t(RBRACE, "}")
                    .close();
                w.open(CATCH);
                if let Some(n) = n {
                    w.t(IDENT, NAMES[*n as usize]);
                }
                w.open(BLOCK).t(LBRACE, "{");
                write_stmts(w, body);
                w.t(RBRACE, "}").close().close().close();
            }
        }
    }
}

/// What the reference expects: names hoisted to the function, names bound
/// by a by-reference binding of a top-level `var` (visible after it as a
/// scoped binding), and how many by-reference bindings cannot hoist.
struct Expect {
    hoisted: [bool; 4],
    top_ref: [bool; 4],
    by_ref: usize,
}

fn expect(ss: &[S]) -> Expect {
    fn pat(p: &P, e: &mut Expect, top_var: bool) {
        match p {
            P::Name(n) => e.hoisted[*n as usize] = true,
            P::Ref(n) => {
                e.by_ref += 1;
                if top_var {
                    e.top_ref[*n as usize] = true;
                }
            }
            P::Tuple(ps) => ps.iter().for_each(|q| pat(q, e, top_var)),
        }
    }
    fn walk(ss: &[S], e: &mut Expect, top: bool) {
        for s in ss {
            match s {
                S::Var(p) => pat(p, e, top),
                S::Foreach(k, v, body) => {
                    if let Some(k) = k {
                        pat(k, e, false);
                    }
                    pat(v, e, false);
                    walk(body, e, false);
                }
                S::Catch(n, body) => {
                    if let Some(n) = n {
                        e.hoisted[*n as usize] = true;
                    }
                    walk(body, e, false);
                }
            }
        }
    }
    let mut e = Expect {
        hoisted: [false; 4],
        top_ref: [false; 4],
        by_ref: 0,
    };
    walk(ss, &mut e, true);
    e
}

/// The pieces of an interpolated string: text (as written, escapes
/// included) or a variable.
#[derive(Clone, Debug)]
enum Part {
    Text(Vec<usize>),
    Var(u8),
}

/// Text fragments as written, and decoded.
const FRAGS: [(&str, &str); 8] = [
    ("a", "a"),
    ("b", "b"),
    (" ", " "),
    ("\\n", "\n"),
    ("\\t", "\t"),
    ("\\\\", "\\"),
    ("\\$", "$"),
    ("é", "é"),
];

fn parts() -> impl Strategy<Value = Vec<Part>> {
    proptest::collection::vec(
        prop_oneof![
            proptest::collection::vec(0..FRAGS.len(), 1..4).prop_map(Part::Text),
            (0u8..4).prop_map(Part::Var),
        ],
        0..6,
    )
}

/// Evaluates a lowered interpolation: literals as their text, variables as
/// `{name}`, `interp(...)` as the concatenation of its arguments.
fn eval(hir: &hir_lang::Hir, names: &Interner, e: hir_lang::ExprId) -> Option<String> {
    match *hir.expr(e) {
        Expr::Lit(Lit::Str(t)) => Some(hir.str(t).to_owned()),
        Expr::Path(p) => {
            let seg = hir.list(hir.path(p).segments).first()?;
            Some(format!("{{{}}}", names.resolve(seg.name.sym)?))
        }
        Expr::Call { callee, args } => {
            let Expr::Path(p) = *hir.expr(callee) else {
                return None;
            };
            if !matches!(hir.path(p).res, hir_lang::Res::Extern(_)) {
                return None;
            }
            let mut out = String::new();
            for a in hir.list(args) {
                out.push_str(&eval(hir, names, a.value)?);
            }
            Some(out)
        }
        _ => None,
    }
}

/// The statement expressions of `main`'s body, in order.
fn main_exprs(out: &Lowered) -> Vec<hir_lang::ExprId> {
    let hir = &out.hir;
    let ItemKind::Module { items, .. } = hir.item(hir.root()).kind else {
        return Vec::new();
    };
    let ItemKind::Fn(def) = hir.item(hir.list(items)[0]).kind else {
        return Vec::new();
    };
    let mut body = def.body.unwrap();
    // Unwrap the hoisted-declarations block, if any.
    if let Expr::Block(b) = *hir.expr(body) {
        if let Some(t) = b.tail {
            body = t;
        }
    }
    let Expr::Block(b) = *hir.expr(body) else {
        return Vec::new();
    };
    hir.list(b.stmts)
        .iter()
        .filter_map(|&s| match *hir.stmt(s) {
            Stmt::Expr(e) => Some(e),
            _ => None,
        })
        .collect()
}

/// A number literal: a prefix, digits, and `_` positions.
#[derive(Clone, Debug)]
struct Num {
    prefix: &'static str,
    digits: String,
    seps: Vec<usize>,
}

fn num() -> impl Strategy<Value = Num> {
    (
        prop_oneof![Just(""), Just("0x"), Just("0o"), Just("0b"), Just("0")],
        "[0-9a-fA-F]{1,26}",
        proptest::collection::vec(0usize..30, 0..3),
    )
        .prop_map(|(prefix, digits, seps)| Num {
            prefix,
            digits,
            seps,
        })
}

impl Num {
    fn text(&self) -> String {
        let mut d: Vec<char> = self.digits.chars().collect();
        for &s in &self.seps {
            let at = s.min(d.len());
            d.insert(at, '_');
        }
        format!("{}{}", self.prefix, d.into_iter().collect::<String>())
    }
}

/// The reference decoding of an integer literal (no hex floats generated
/// with a `.`/`p`; `e`/`E` digits only after a hex prefix matter).
#[derive(Debug, PartialEq)]
enum NumOut {
    Int(u64),
    Big(String),
    Float(f64),
    Bad,
}

fn reference_number(text: &str, php: bool) -> NumOut {
    // Separators.
    let b = text.as_bytes();
    let (hex, start) = match (b.first(), b.get(1)) {
        (Some(b'0'), Some(b'x')) => (true, 2),
        (Some(b'0'), Some(b'o' | b'b')) => (false, 2),
        _ => (false, 0),
    };
    if php {
        for (i, &c) in b.iter().enumerate() {
            if c == b'_' {
                let ok = |x: Option<&u8>| {
                    x.is_some_and(|&x| {
                        if hex {
                            x.is_ascii_hexdigit()
                        } else {
                            x.is_ascii_digit()
                        }
                    })
                };
                if i <= start || !ok(b.get(i - 1)) || !ok(b.get(i + 1)) {
                    return NumOut::Bad;
                }
            }
        }
    }
    let clean: String = text.chars().filter(|&c| c != '_').collect();
    let (mut radix, mut digits) = match clean.get(..2) {
        Some("0x") => (16, &clean[2..]),
        Some("0o") => (8, &clean[2..]),
        Some("0b") => (2, &clean[2..]),
        _ => (10, &clean[..]),
    };
    // A decimal with `e`/`E` is a float literal: not generated here as such.
    if radix == 10 && digits.bytes().any(|c| c == b'e' || c == b'E') {
        return match digits.parse::<f64>() {
            Ok(v) => NumOut::Float(v),
            Err(_) => NumOut::Bad,
        };
    }
    if radix == 10 && digits.len() > 1 && digits.starts_with('0') && php {
        radix = 8;
        digits = &digits[1..];
    }
    if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
        return NumOut::Bad;
    }
    let Some(v) = u128::from_str_radix(digits, radix).ok() else {
        return NumOut::Bad; // beyond u128: not generated
    };
    if php {
        if v > i64::MAX as u128 {
            #[allow(clippy::cast_precision_loss)]
            return NumOut::Float(v as f64);
        }
        return NumOut::Int(v as u64);
    }
    match u64::try_from(v) {
        Ok(v) => NumOut::Int(v),
        Err(_) => NumOut::Big(v.to_string()),
    }
}

/// A use tree: a prefix of names, then children or an alias.
#[derive(Clone, Debug)]
struct UT {
    prefix: Vec<u8>,
    children: Option<Vec<UT>>,
    alias: Option<u8>,
}

fn use_tree() -> impl Strategy<Value = UT> {
    let leaf = (
        proptest::collection::vec(0u8..4, 1..3),
        proptest::option::of(0u8..4),
    )
        .prop_map(|(prefix, alias)| UT {
            prefix,
            children: None,
            alias,
        });
    leaf.prop_recursive(3, 16, 4, |inner| {
        (
            proptest::collection::vec(0u8..4, 1..3),
            proptest::collection::vec(inner, 0..4),
        )
            .prop_map(|(prefix, children)| UT {
                prefix,
                children: Some(children),
                alias: None,
            })
    })
}

fn flatten(t: &UT, prefix: &mut Vec<u8>, out: &mut Vec<(Vec<u8>, Option<u8>)>) {
    let len = prefix.len();
    prefix.extend_from_slice(&t.prefix);
    match &t.children {
        Some(cs) => {
            for c in cs {
                flatten(c, prefix, out);
            }
        }
        None => out.push((prefix.clone(), t.alias)),
    }
    prefix.truncate(len);
}

fn write_use(w: &mut W, t: &UT) {
    w.open(USE);
    if t.prefix.len() == 1 {
        w.t(IDENT, NAMES[t.prefix[0] as usize]);
    } else {
        w.open(PATH);
        for (i, n) in t.prefix.iter().enumerate() {
            if i > 0 {
                w.t(COLONS, "::");
            }
            w.t(IDENT, NAMES[*n as usize]);
        }
        w.close();
    }
    match &t.children {
        Some(cs) => {
            w.t(COLONS, "::").open(GROUP).t(LBRACE, "{");
            for (i, c) in cs.iter().enumerate() {
                if i > 0 {
                    w.t(COMMA, ",");
                }
                write_use(w, c);
            }
            w.t(RBRACE, "}").close();
        }
        None => {
            if let Some(a) = t.alias {
                w.t(AS, "as").t(IDENT, NAMES[a as usize]);
            }
        }
    }
    w.close();
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    /// PHP scoping against the reference: every name a `var`, `foreach`, or
    /// `catch` binds (except by reference) is declared once for the function
    /// and, after the construct, resolves to that declaration; a
    /// by-reference binding is reported and stays scoped; hygiene holds.
    #[test]
    fn prop_function_scoped_bindings_match_the_reference(ss in stmts()) {
        let mut w = W::new();
        w.open(FILE);
        main_fn(&mut w, |w| {
            write_stmts(w, &ss);
            for n in NAMES {
                w.open(STMT).t(IDENT, n).t(SEMI, ";").close();
            }
        });
        w.close();
        let (out, names, _) = lower_with(w, LangPolicy::php());
        let e = expect(&ss);
        let by_ref = out
            .problems
            .iter()
            .filter(|p| p.kind == ProblemKind::CannotHoist { reason: HoistBlocker::ByReference })
            .count();
        prop_assert_eq!(by_ref, e.by_ref, "{:?}", out.problems);
        prop_assert!(out.problems.iter().all(|p| matches!(p.kind, ProblemKind::CannotHoist { .. })), "{:?}", out.problems);
        // The hoisted declarations: `let` statements of the wrapper block.
        let hir = &out.hir;
        let ItemKind::Module { items, .. } = hir.item(hir.root()).kind else { panic!() };
        let ItemKind::Fn(def) = hir.item(hir.list(items)[0]).kind else { panic!() };
        let mut hoisted_binders = Vec::new();
        if let Expr::Block(b) = *hir.expr(def.body.unwrap()) {
            if b.tail.is_some() {
                for &s in hir.list(b.stmts) {
                    if let Stmt::Let { pat, .. } = *hir.stmt(s) {
                        if let Pat::Bind { binder, .. } = *hir.pat(pat) {
                            hoisted_binders.push(binder);
                        }
                    }
                }
            }
        }
        let declared: Vec<String> = hoisted_binders
            .iter()
            .map(|b| names.resolve(hir.binder(*b).unwrap().name.sym).unwrap().to_owned())
            .collect();
        for (i, n) in NAMES.iter().enumerate() {
            prop_assert_eq!(declared.iter().filter(|d| d.as_str() == *n).count(), usize::from(e.hoisted[i]), "{} in {:?}", n, ss);
        }
        // Resolve through resolve-lang under PHP's policy; the final uses
        // read the hoisted declarations.
        let (hir, _) = common::resolve(out.hir.clone(), &names, resolve_lang::Policy::php());
        prop_assert_eq!(hir.validate(), Ok(()));
        prop_assert!(common::hygienic(&hir));
        let finals = main_exprs(&Lowered::clone(&out));
        let finals = &finals[finals.len() - 4..];
        for (i, &fe) in finals.iter().enumerate() {
            let Expr::Path(p) = *hir.expr(fe) else { panic!() };
            let res = hir.path(p).res;
            let to_hoisted = matches!(res, hir_lang::Res::Local(b) if hoisted_binders.contains(&b));
            if e.top_ref[i] {
                prop_assert!(matches!(res, hir_lang::Res::Local(_)) && !to_hoisted, "{} in {:?}", NAMES[i], ss);
            } else if e.hoisted[i] {
                prop_assert!(to_hoisted, "{} in {:?}", NAMES[i], ss);
            } else {
                // Not declared anywhere: the resolver reports it.
                prop_assert!(matches!(res, hir_lang::Res::Err), "{} in {:?}", NAMES[i], ss);
            }
        }
        for i in 0..hir.count(IdKind::Path) {
            let path = *hir.path(hir_lang::PathId::from_index(i).unwrap());
            if let hir_lang::Res::Local(b) = path.res {
                prop_assert_eq!(hir.binder(b).unwrap().name.mark, hir.list(path.segments)[0].name.mark);
            }
        }
    }

    /// Interpolation against the reference: decoded text pieces (adjacent
    /// ones merged) and variables, joined in order; a lone variable is
    /// joined with "", one text is a literal, none is "".
    #[test]
    fn prop_interpolation_matches_the_reference(ps in parts()) {
        let mut w = W::new();
        w.open(FILE);
        main_fn(&mut w, |w| {
            w.open(STMT).open(INTERP_CALL);
            for p in &ps {
                match p {
                    Part::Text(fs) => {
                        let text: String = fs.iter().map(|&f| FRAGS[f].0).collect();
                        w.t(STR_PART, &text);
                    }
                    Part::Var(n) => {
                        w.t(IDENT, NAMES[*n as usize]);
                    }
                }
            }
            w.close().t(SEMI, ";").close();
        });
        w.close();
        let (out, names, _) = lower_with(w, LangPolicy::default());
        prop_assert!(out.problems.is_empty(), "{:?}", out.problems);
        // The reference: merged pieces.
        let mut merged: Vec<Result<String, u8>> = Vec::new();
        for p in &ps {
            match p {
                Part::Text(fs) => {
                    let text: String = fs.iter().map(|&f| FRAGS[f].1).collect();
                    if let Some(Ok(prev)) = merged.last_mut() {
                        prev.push_str(&text);
                    } else {
                        merged.push(Ok(text));
                    }
                }
                Part::Var(n) => merged.push(Err(*n)),
            }
        }
        let want: String = merged
            .iter()
            .map(|m| match m {
                Ok(t) => t.clone(),
                Err(n) => format!("{{{}}}", NAMES[*n as usize]),
            })
            .collect();
        let e = main_exprs(&out)[0];
        prop_assert_eq!(eval(&out.hir, &names, e), Some(want));
        let args = match *out.hir.expr(e) {
            Expr::Call { args, .. } => Some(args.len()),
            _ => None,
        };
        let expected_args = match merged.as_slice() {
            [] | [Ok(_)] => None,
            [Err(_)] => Some(2),
            m => Some(m.len()),
        };
        prop_assert_eq!(args, expected_args);
    }

    /// Heredoc tokens against PHP 7.3's rule: the closing marker's
    /// indentation leaves every line; a non-empty line indented less is an
    /// error; the last newline is not part of the string.
    #[test]
    fn prop_heredoc_indentation_matches_the_reference(
        indent in 0usize..4,
        lines in proptest::collection::vec((0usize..6, "[ab]{0,3}", any::<bool>()), 0..5),
    ) {
        let mut body = Vec::new();
        for (ind, content, esc) in &lines {
            let mut l = " ".repeat(*ind);
            l.push_str(content);
            if *esc {
                l.push_str("\\n");
            }
            if content.is_empty() && !*esc {
                l.clear();
            }
            body.push(l);
        }
        let mut text = String::from("<<<EOT\n");
        for l in &body {
            text.push_str(l);
            text.push('\n');
        }
        text.push_str(&" ".repeat(indent));
        text.push_str("EOT");
        // The reference.
        let mut bad = false;
        let stripped: Vec<String> = body
            .iter()
            .map(|l| {
                if l.is_empty() {
                    String::new()
                } else if let Some(r) = l.strip_prefix(&" ".repeat(indent)) {
                    r.to_owned()
                } else {
                    bad = true;
                    l.trim_start().to_owned()
                }
            })
            .collect();
        let want = stripped.join("\n").replace("\\n", "\n");
        let (lits, out, _) = literals(&[(HEREDOC, &text)], LangPolicy::default());
        let got = match lits[0] {
            Some(Lit::Str(t)) => Some(out.hir.str(t).to_owned()),
            _ => None,
        };
        prop_assert_eq!(got, Some(want), "{:?}", text);
        prop_assert_eq!(!out.problems.is_empty(), bad, "{:?}", out.problems);
    }

    /// Integer literals against a `u128` reference, under the default table
    /// (`_` anywhere, `0777` decimal, exact big integers) and PHP's (`_`
    /// between digits, `0777` octal, float above `i64::MAX`).
    #[test]
    fn prop_integer_literals_match_the_reference(n in num(), php in any::<bool>()) {
        let text = n.text();
        let policy = if php { LangPolicy::php() } else { LangPolicy::default() };
        let want = reference_number(&text, php);
        let (lits, out, _) = literals(&[(NUM, &text)], policy);
        let got = match lits[0] {
            Some(Lit::Int(i)) => NumOut::Int(i.value),
            Some(Lit::BigInt(t)) => NumOut::Big(out.hir.str(t).to_owned()),
            Some(Lit::Float(f)) => NumOut::Float(f.value()),
            _ => NumOut::Bad,
        };
        prop_assert_eq!(got, want, "{}", text);
    }

    /// Use trees flatten to one import per leaf, prefixes concatenated, in
    /// source order, against a recursive reference.
    #[test]
    fn prop_use_trees_flatten_like_the_reference(t in use_tree()) {
        let mut w = W::new();
        w.open(FILE);
        write_use(&mut w, &t);
        w.close();
        let (out, names, _) = lower_with(w, LangPolicy::default());
        prop_assert!(out.problems.is_empty(), "{:?}", out.problems);
        let mut want = Vec::new();
        flatten(&t, &mut Vec::new(), &mut want);
        let hir = &out.hir;
        let ItemKind::Module { items, .. } = hir.item(hir.root()).kind else { panic!() };
        let got: Vec<(Vec<u8>, Option<u8>)> = hir
            .list(items)
            .iter()
            .filter_map(|&i| {
                let it = hir.item(i);
                let ItemKind::Import { path, .. } = it.kind else { return None };
                let idx = |s: hir_lang::Symbol| {
                    NAMES.iter().position(|n| names.get(n) == Some(s)).unwrap() as u8
                };
                let segs = hir.list(hir.path(path).segments).iter().map(|s| idx(s.name.sym)).collect();
                Some((segs, it.name.map(|n| idx(n.sym))))
            })
            .collect();
        prop_assert_eq!(got, want);
    }
}
