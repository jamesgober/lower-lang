//! The hir-lang 0.4 forms lower-lang 0.4 produces: union and intersection
//! types (normalized by hir-lang), keyed `yield` and `yield from`,
//! compound assignment with non-OPS operators through `let_place` (place
//! evaluated once), the `ref_assign` archetype, `pow` compounds, and the
//! template forms behind them.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod common;
#[path = "support/writer.rs"]
mod writer;

use hir_lang::{
    BinderKind, ExpnKind, Expr, Hir, IdKind, Intrinsic, Lit, NodeRef, OpKind, Overflow, Prim, Res,
    Ty,
};
use intern_lang::Interner;
use lower_lang::{
    Arch, Escapes, Fixity, LangPolicy, Lowered, MappingBuilder, MappingError, OpSpec, Pick,
    ProblemKind, Template, TemplateBuilder, TokenRole, TypeName, templates,
};
use writer::{K, W};

const IDENT: K = K(1);
const NUM: K = K(2);
const STR: K = K(3);
const EQ: K = K(4);
const COLON: K = K(5);
const PIPE: K = K(6);
const AMP: K = K(7);
const SEMI: K = K(8);
const FAT: K = K(9);
const YIELD: K = K(10);
const FROM: K = K(11);
const DOTEQ: K = K(12);
const POWEQ: K = K(13);
const COALEQ: K = K(14);
const STAREQ: K = K(15);
const LBRACK: K = K(16);
const RBRACK: K = K(17);
const REF: K = K(18);

const FILE: K = K(100);
const LET: K = K(101);
const UNION: K = K(102);
const INTER: K = K(103);
const YIELD_KV: K = K(104);
const YIELD_FROM: K = K(105);
const ASSIGN: K = K(106);
const ASSIGN_OP: K = K(107);
const INDEX: K = K(108);
const REF_ASSIGN: K = K(109);
const STMT: K = K(110);
const TPL: K = K(111);

fn base(policy: LangPolicy) -> MappingBuilder<K> {
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
        LET,
        Arch::Let {
            pat: Pick::Nth(1),
            ty: Pick::After(COLON),
            init: Pick::After(EQ),
            else_: Pick::Absent,
            mutable: true,
            hoist: false,
        },
    );
    b.node(
        UNION,
        Arch::UnionTy {
            members: Pick::AllKind(IDENT),
        },
    );
    b.node(
        INTER,
        Arch::IntersectionTy {
            members: Pick::AllKind(IDENT),
        },
    );
    b.node(
        YIELD_KV,
        Arch::Yield {
            key: Pick::Before(FAT),
            value: Pick::After(FAT),
        },
    );
    b.node(
        YIELD_FROM,
        Arch::YieldFrom {
            inner: Pick::After(FROM),
        },
    );
    b.node(ASSIGN, Arch::binary());
    b.node(
        ASSIGN_OP,
        Arch::Assign {
            target: Pick::Nth(0),
            op: Pick::Nth(1),
            value: Pick::Nth(2),
        },
    );
    b.node(
        INDEX,
        Arch::Index {
            base: Pick::Nth(0),
            index: Pick::Nth(2),
        },
    );
    b.node(
        REF_ASSIGN,
        Arch::RefAssign {
            target: Pick::Nth(0),
            source: Pick::After(REF),
        },
    );
    b.node(STMT, Arch::ExprStmt { expr: Pick::Nth(0) });
    b.token(IDENT, TokenRole::Name)
        .token(NUM, TokenRole::Number)
        .token(STR, TokenRole::Str(Escapes::Standard));
    b.skip(SEMI).skip(PIPE).skip(AMP);
    b.type_name("int", TypeName::Prim(Prim::I64))
        .type_name("string", TypeName::Prim(Prim::Str))
        .type_name("float", TypeName::Prim(Prim::F64));
    b.binary_op(EQ, OpSpec::Assign);
    b
}

fn run(b: MappingBuilder<K>, w: W) -> (Lowered, Interner, String) {
    let mapping = b.build().expect("valid mapping");
    let (tree, src) = w.finish();
    let mut names = Interner::new();
    let out = lower_lang::lower(&tree, &src, &mapping, &mut names).expect("within budget");
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

fn tys(hir: &Hir) -> Vec<Ty> {
    let mut v = Vec::new();
    hir_lang::walk(hir, |n| {
        if let NodeRef::Ty(t) = n {
            v.push(*hir.ty(t));
        }
    });
    v
}

/// `let x: <members joined by sep> = 1;`
fn let_typed(w: &mut W, node: K, sep: (K, &str), members: &[&str]) {
    w.open(LET).t(IDENT, "let").t(IDENT, "x").t(COLON, ":");
    w.open(node);
    for (i, m) in members.iter().enumerate() {
        if i > 0 {
            w.t(sep.0, sep.1);
        }
        w.t(IDENT, m);
    }
    w.close();
    w.t(EQ, "=").t(NUM, "1").t(SEMI, ";").close();
}

#[test]
fn test_union_and_intersection_types_are_normalized_by_hir() {
    let mut w = W::new();
    w.open(FILE);
    // int|string|int: a repeat dropped, members sorted.
    let_typed(&mut w, UNION, (PIPE, "|"), &["string", "int", "string"]);
    // int|int: one member left, the union collapses.
    let_typed(&mut w, UNION, (PIPE, "|"), &["int", "int"]);
    // Countable & Traversable.
    let_typed(&mut w, INTER, (AMP, "&"), &["Countable", "Traversable"]);
    // No members at all: reported, an error type.
    let_typed(&mut w, UNION, (PIPE, "|"), &[]);
    w.close();
    let (out, names, _) = run(base(LangPolicy::default()), w);
    assert_eq!(
        out.problems.iter().map(|p| p.kind).collect::<Vec<_>>(),
        [ProblemKind::MissingField { field: "members" }]
    );
    let all = tys(&out.hir);
    let unions: Vec<_> = all
        .iter()
        .filter_map(|t| match t {
            Ty::Union(l) => Some(*l),
            _ => None,
        })
        .collect();
    assert_eq!(unions.len(), 1);
    let members: Vec<Ty> = out
        .hir
        .list(unions[0])
        .iter()
        .map(|m| *out.hir.ty(*m))
        .collect();
    assert_eq!(members, [Ty::Prim(Prim::I64), Ty::Prim(Prim::Str)]);
    assert!(all.iter().any(|t| matches!(t, Ty::Intersection(_))));
    assert!(all.iter().any(|t| matches!(t, Ty::Err)));
    let printed = hir_lang::print(&out.hir, &names);
    assert!(printed.contains("(union"), "{printed}");
}

#[test]
fn test_keyed_yield_and_yield_from() {
    let mut w = W::new();
    w.open(FILE);
    // yield 1 => 2;  yield 3 =>;  yield from 4;
    w.open(STMT)
        .open(YIELD_KV)
        .t(YIELD, "yield")
        .t(NUM, "1")
        .t(FAT, "=>")
        .t(NUM, "2")
        .close()
        .t(SEMI, ";")
        .close();
    w.open(STMT)
        .open(YIELD_KV)
        .t(YIELD, "yield")
        .t(NUM, "3")
        .t(FAT, "=>")
        .close()
        .t(SEMI, ";")
        .close();
    w.open(STMT)
        .open(YIELD_FROM)
        .t(YIELD, "yield")
        .t(FROM, "from")
        .t(NUM, "4")
        .close()
        .t(SEMI, ";")
        .close();
    w.close();
    let mut policy = LangPolicy::default();
    policy.yields = lower_lang::EffectMode::Inferred;
    let (out, _, _) = run(base(policy), w);
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let es = exprs(&out.hir);
    let keyed: Vec<_> = es
        .iter()
        .filter_map(|e| match e {
            Expr::Yield {
                key: Some(_),
                value: Some(v),
            } => Some(*out.hir.expr(*v)),
            _ => None,
        })
        .collect();
    assert_eq!(keyed.len(), 2);
    // A key without a value yields null under that key.
    assert!(keyed.iter().any(|v| matches!(v, Expr::Lit(Lit::Null))));
    assert!(es.iter().any(|e| matches!(e, Expr::YieldFrom(_))));
    // The module body is a generator (inferred).
    let hir_lang::ItemKind::Module { effects, .. } = out.hir.item(out.hir.root()).kind else {
        panic!("module")
    };
    assert!(effects.contains(hir_lang::Effects::YIELD));
}

#[test]
fn test_ref_assign_archetype_keeps_its_operands() {
    // a = &b;  (0.3 lowered the operands and dropped them: orphaned HIR)
    let mut w = W::new();
    w.open(FILE);
    w.open(STMT)
        .open(REF_ASSIGN)
        .t(IDENT, "a")
        .t(EQ, "=")
        .t(REF, "&")
        .t(IDENT, "b")
        .close()
        .t(SEMI, ";")
        .close();
    w.close();
    let (out, _, _) = run(base(LangPolicy::default()), w);
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    assert!(
        exprs(&out.hir)
            .iter()
            .any(|e| matches!(e, Expr::RefAssign { .. }))
    );
}

/// The compound-assignment mapping: `.=` an extern, `**=` an intrinsic,
/// `??=` the `coalesce` template, `*=` an OPS `pow`.
fn compound_mapping() -> MappingBuilder<K> {
    let mut b = base(LangPolicy::php());
    let concat = b.name("mox.concat");
    let pow = b.name("mox.pow");
    let co = b.template(templates::coalesce()).unwrap();
    b.binary_op(DOTEQ, OpSpec::CompoundExtern(concat))
        .binary_op(POWEQ, OpSpec::CompoundIntrinsic(pow))
        .binary_op(COALEQ, OpSpec::CompoundTemplate(co))
        .binary_op(STAREQ, OpSpec::Compound(OpKind::Pow));
    b
}

/// `a[k] <op> v;` through `node` (the binary or the assign archetype).
fn compound(w: &mut W, node: K, op: (K, &str), value: &str) {
    w.open(STMT).open(node);
    w.open(INDEX)
        .t(IDENT, "a")
        .t(LBRACK, "[")
        .t(IDENT, "k")
        .t(RBRACK, "]")
        .close();
    w.t(op.0, op.1).t(STR, value).close().t(SEMI, ";").close();
}

#[test]
fn test_non_ops_compound_assignment_names_its_place_once() {
    let mut w = W::new();
    w.open(FILE);
    compound(&mut w, ASSIGN, (DOTEQ, ".="), "\"!\"");
    compound(&mut w, ASSIGN_OP, (POWEQ, "**="), "\"2\"");
    compound(&mut w, ASSIGN, (COALEQ, "??="), "\"d\"");
    compound(&mut w, ASSIGN_OP, (STAREQ, "*="), "\"3\"");
    w.close();
    let (out, mut names, src) = run(compound_mapping(), w);
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let hir = &out.hir;
    let es = exprs(hir);
    // Three let_place forms (the OPS `pow` compound needs none), and `a[k]`
    // indexed exactly as often as the source writes it.
    assert_eq!(
        es.iter()
            .filter(|e| matches!(e, Expr::LetPlace { .. }))
            .count(),
        3
    );
    assert_eq!(
        es.iter()
            .filter(|e| matches!(e, Expr::Index { .. }))
            .count(),
        src.matches("a [ k ]").count()
    );
    // `*=` with `pow` is an OPS compound (valid since hir-lang 0.4).
    assert!(es.iter().any(|e| matches!(
        e,
        Expr::Assign { op: Some(op), .. } if op.kind == OpKind::Pow && op.policy.overflow == Some(Overflow::Promote)
    )));
    // The extern call and the intrinsic read the alias, then the alias is
    // written.
    let concat = names.intern("mox.concat");
    let pow = names.intern("mox.pow");
    assert!(es.iter().any(|e| matches!(
        e,
        Expr::Intrinsic { kind: Intrinsic::Named(s), .. } if *s == pow
    )));
    let externs = (0..hir.count(IdKind::Path))
        .filter(|&i| hir.path(hir_lang::PathId::from_index(i).unwrap()).res == Res::Extern(concat))
        .count();
    assert_eq!(externs, 1);
    // Each alias is a `Place` binder carrying a `compound` desugaring mark.
    let compound_sym = names.intern("compound");
    let mut aliases = 0;
    for i in 0..hir.count(IdKind::Binder) {
        let b = hir
            .binder(hir_lang::BinderId::from_index(i).unwrap())
            .unwrap();
        if b.kind == BinderKind::Place {
            aliases += 1;
            let x = hir.expansion(b.name.mark).unwrap();
            assert_eq!((x.kind, x.name), (ExpnKind::Desugar, compound_sym));
        }
    }
    assert_eq!(aliases, 3);
    let printed = hir_lang::print(hir, &names);
    assert!(printed.contains("(let-place p%"), "{printed}");
    // Resolution keeps the aliases hygienic.
    let (resolved, _) = common::resolve(out.hir.clone(), &names, resolve_lang::Policy::php());
    assert_eq!(resolved.validate(), Ok(()));
    assert!(common::hygienic(&resolved));
}

#[test]
fn test_a_compound_with_a_missing_operand_stays_valid() {
    // `a[k] .=` with no value: one problem, valid HIR, nothing orphaned.
    let mut w = W::new();
    w.open(FILE);
    w.open(STMT).open(ASSIGN_OP);
    w.open(INDEX)
        .t(IDENT, "a")
        .t(LBRACK, "[")
        .t(IDENT, "k")
        .t(RBRACK, "]")
        .close();
    w.t(DOTEQ, ".=").close().t(SEMI, ";").close();
    w.close();
    let (out, _, _) = run(compound_mapping(), w);
    assert_eq!(
        out.problems.iter().map(|p| p.kind).collect::<Vec<_>>(),
        [ProblemKind::MissingField { field: "rhs" }]
    );
}

#[test]
fn test_compound_specs_are_checked_against_their_position() {
    let mut b = MappingBuilder::<K>::new();
    let n = b.name("f");
    b.prefix_op(DOTEQ, OpSpec::CompoundExtern(n));
    assert_eq!(
        b.build().unwrap_err(),
        MappingError::BadOperator {
            fixity: Fixity::Prefix,
            index: 0
        }
    );
    // A compound template needs exactly two expression holes.
    let mut b = MappingBuilder::<K>::new();
    let one = b.template(templates::post_increment()).unwrap();
    b.binary_op(DOTEQ, OpSpec::CompoundTemplate(one));
    assert!(matches!(b.build(), Err(MappingError::BadOperator { .. })));
    let mut b = MappingBuilder::<K>::new();
    b.binary_op(STAREQ, OpSpec::Compound(OpKind::Pow))
        .binary_op(POWEQ, OpSpec::CompoundIntrinsic(n));
    assert!(b.build().is_ok());
}

#[test]
fn test_int_cast_in_a_template_never_promotes() {
    // Under PHP's `promote`, a template's `int_cast` takes `error`: HIR
    // refuses `promote` on a static result.
    let mut b = base(LangPolicy::php());
    let t = b.parse_template("cast", "int_cast<i32>(${value})").unwrap();
    b.template_node(TPL, t, &[Pick::Nth(0)]);
    let mut w = W::new();
    w.open(FILE).open(TPL).t(NUM, "7").close().close();
    let (out, _, _) = run(b, w);
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    assert!(exprs(&out.hir).iter().any(|e| matches!(
        e,
        Expr::Op { op, .. } if op.kind == OpKind::IntCast(Prim::I32) && op.policy.overflow == Some(Overflow::Error)
    )));
}

#[test]
fn test_template_yield_forms_and_let_place_instantiate() {
    // A keyed yield whose optional value is absent yields null; `yield from`
    // and `let_place` instantiate with fresh, marked binders.
    let mut policy = LangPolicy::default();
    policy.yields = lower_lang::EffectMode::Inferred;
    let mut b = base(policy);
    let t = Template::parse(
        "gen",
        "{ yield ${key} => ${value?}; let_place p = ${key} in p = add(p, 1); yield from ${key} }",
    )
    .unwrap()
    .desugar();
    let t = b.template(t).unwrap();
    b.template_node(TPL, t, &[Pick::Nth(0), Pick::Absent]);
    let mut w = W::new();
    w.open(FILE).open(TPL).t(IDENT, "x").close().close();
    let (out, _, _) = run(b, w);
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let es = exprs(&out.hir);
    assert!(es.iter().any(|e| matches!(
        e,
        Expr::Yield { key: Some(_), value: Some(v) } if matches!(out.hir.expr(*v), Expr::Lit(Lit::Null))
    )));
    assert!(es.iter().any(|e| matches!(e, Expr::YieldFrom(_))));
    assert!(es.iter().any(|e| matches!(e, Expr::LetPlace { .. })));
    // The builder form of the same keyed yield.
    let mut tb = TemplateBuilder::new("kv");
    let (k, v) = (tb.expr_hole("k"), tb.expr_hole("v"));
    let (k, v) = (tb.hole(k), tb.hole(v));
    let root = tb.yield_kv(k, v);
    assert!(tb.finish(root).is_ok());
}

#[test]
fn test_lenient_repair_reports_every_repeated_binding() {
    // `let (x, x, x) | (x, x, x) = 1;` — an or-pattern's alternatives share
    // one binder per name, so each alternative binds `x` three times.
    // hir-lang 0.4's lenient repair reports the duplicate at every repeat
    // of every alternative (0.3: the first only), and the HIR stays valid.
    const TUPLE_PAT: K = K(112);
    const OR_PAT: K = K(113);
    let mut b = base(LangPolicy::default());
    b.node(
        TUPLE_PAT,
        Arch::TuplePat {
            elems: Pick::AllKind(IDENT),
        },
    );
    b.node(
        OR_PAT,
        Arch::OrPat {
            alts: Pick::AllKind(TUPLE_PAT),
        },
    );
    let mut w = W::new();
    w.open(FILE).open(LET).t(IDENT, "let");
    w.open(OR_PAT);
    for i in 0..2 {
        if i > 0 {
            w.t(PIPE, "|");
        }
        w.open(TUPLE_PAT)
            .t(IDENT, "x")
            .t(SEMI, ",")
            .t(IDENT, "x")
            .t(SEMI, ",")
            .t(IDENT, "x")
            .close();
    }
    w.close();
    w.t(EQ, "=").t(NUM, "1").t(SEMI, ";").close().close();
    let (out, _, _) = run(b, w);
    let dups = out
        .problems
        .iter()
        .filter(|p| {
            matches!(
                p.kind,
                ProblemKind::Hir(hir_lang::HirError::DuplicateBinding { .. })
            )
        })
        .count();
    assert_eq!(dups, 4, "{:?}", out.problems);
}
