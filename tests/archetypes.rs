//! The archetype catalogue, one construct family per test, on hand-built
//! trees: declarations, generics, parameters, patterns, types, paths, labels,
//! literals, templates, budgets, and every error path.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod common;
#[path = "support/writer.rs"]
mod writer;

use hir_lang::{
    ArgKind, BindMode, BinderKind, BorrowKind, CaptureMode, Effects, ExpnKind, Expr, Hir, IdKind,
    ItemKind, Lit, Member, NodeRef, Ns, OpKind, ParamKind, Pat, PathRoot, Prim, Shape, Stmt, Ty,
    UnitId,
};
use intern_lang::Interner;
use lower_lang::{
    Arch, Budget, Escapes, Labeler, LowerError, Lowered, Lowerer, MappingBuilder, Modifier, OpSpec,
    Pick, ProblemKind, RecordShape, TArg, TLit, TemplateBuilder, TokenRole, TypeName, templates,
};
use syntax_lang::Node;
use writer::{K, W};

// ------------------------------------------------------------------ tokens
const IDENT: K = K(1);
const NUM: K = K(2);
const STR: K = K(3);
const CHAR: K = K(4);
const BYTES: K = K(5);
const TRUE: K = K(6);
const NULL: K = K(8);
const WILD: K = K(9);
const DOTDOT: K = K(10);
const LIFETIME: K = K(11);
const LABEL: K = K(12);
const PLUS: K = K(13);
const MINUS: K = K(14);
const STAR: K = K(15);
const EQ: K = K(16);
const COMMA: K = K(17);
const SEMI: K = K(18);
const MUT: K = K(19);
const AMP: K = K(20);
const PUB: K = K(21);
const ASYNC: K = K(22);
const DOTDOTEQ: K = K(23);
const COLONCOLON: K = K(24);
const SUPER: K = K(25);
const SELFTY: K = K(26);
const SELFKW: K = K(27);
const ELLIPSIS: K = K(28);
const NAMED: K = K(29);
const POSONLY: K = K(30);
const I32: K = K(31);
const GLOB: K = K(32);
const KW: K = K(33);
const ARROW: K = K(36);
const COLON: K = K(37);
const UNION: K = K(39);
const QUESTION: K = K(40);
const BANG: K = K(41);
const GT: K = K(42);
const AS: K = K(43);

// ------------------------------------------------------------------- nodes
const FILE: K = K(100);
const FN: K = K(101);
const GENERICS: K = K(102);
const GP: K = K(103);
const PARAMS: K = K(104);
const PARAM: K = K(105);
const WHERE: K = K(106);
const WP: K = K(107);
const BLOCK: K = K(108);
const AWAIT: K = K(109);
const IMPL: K = K(110);
const RECORD: K = K(111);
const FIELD: K = K(112);
const SUM: K = K(113);
const VARIANT: K = K(114);
const ALIAS: K = K(115);
const CONST: K = K(116);
const GLOBAL: K = K(117);
const IMPORT: K = K(118);
const PATH: K = K(119);
const LET: K = K(120);
const MATCH: K = K(121);
const ARM: K = K(122);
const TUPLE: K = K(123);
const CALL: K = K(124);
const RECLIT: K = K(125);
const INIT: K = K(126);
const RANGE: K = K(127);
const REFPAT: K = K(128);
const SLICE: K = K(129);
const BINDAT: K = K(130);
const ORPAT: K = K(131);
const ARRAYTY: K = K(133);
const REFTY: K = K(134);
const FNTY: K = K(135);
const NULLTY: K = K(136);
const DYNTY: K = K(137);
const SEG: K = K(138);
const BINDING: K = K(139);
const LOOP: K = K(140);
const BREAK: K = K(141);
const CONTINUE: K = K(142);
const EXPR: K = K(143);
const BINARY: K = K(144);
const PREFIX: K = K(145);
const CLOSURE: K = K(146);
const SPAWN: K = K(147);
const DEFER: K = K(148);
const REPEAT: K = K(149);
const CAST: K = K(150);
const MEMBER: K = K(151);
const MCALL: K = K(152);
const ARG: K = K(153);
const MODULE: K = K(154);
const MULTI: K = K(155);
const UNSUP: K = K(156);
const ERROR: K = K(157);
const TPL: K = K(158);
const POSTFIX: K = K(159);
const PTRTY: K = K(160);
const SLICETY: K = K(161);
const TRAIT: K = K(162);
const ASSOC: K = K(163);
const TRY: K = K(164);
const ARGS: K = K(165);

/// Tokens, modifiers, roots, operators, and the root module.
fn base() -> MappingBuilder<K> {
    let mut b = MappingBuilder::new();
    b.node(
        FILE,
        Arch::Module {
            name: Pick::Absent,
            items: Pick::Nodes,
        },
    );
    b.token(IDENT, TokenRole::Name)
        .token(NUM, TokenRole::Number)
        .token(STR, TokenRole::Str(Escapes::Standard))
        .token(CHAR, TokenRole::Char(Escapes::Standard))
        .token(BYTES, TokenRole::Bytes(Escapes::Standard))
        .token(TRUE, TokenRole::True)
        .token(NULL, TokenRole::Null)
        .token(WILD, TokenRole::Wild)
        .token(DOTDOT, TokenRole::Rest)
        .token(LIFETIME, TokenRole::Lifetime { sigil: 1 })
        .token(LABEL, TokenRole::Label { sigil: 1 })
        .token(SELFKW, TokenRole::Name)
        .token(I32, TokenRole::Type(TypeName::Prim(Prim::I32)));
    b.modifier(MUT, Modifier::Mut)
        .modifier(AMP, Modifier::Ref)
        .modifier(PUB, Modifier::Public)
        .modifier(ASYNC, Modifier::Async)
        .modifier(DOTDOTEQ, Modifier::Inclusive)
        .modifier(ELLIPSIS, Modifier::Rest)
        .modifier(NAMED, Modifier::NamedOnly)
        .modifier(POSONLY, Modifier::PositionalOnly)
        .modifier(SELFKW, Modifier::Receiver)
        .modifier(GLOB, Modifier::Glob)
        .modifier(UNION, Modifier::Union);
    b.root(SUPER, PathRoot::Super(1))
        .root(SELFTY, PathRoot::SelfType);
    b.skip(COMMA).skip(SEMI);
    b.binary_op(PLUS, OpSpec::Op(OpKind::Add))
        .binary_op(MINUS, OpSpec::Op(OpKind::Sub))
        .binary_op(STAR, OpSpec::Op(OpKind::Mul))
        .binary_op(EQ, OpSpec::Assign)
        .prefix_op(MINUS, OpSpec::Op(OpKind::Neg))
        .prefix_op(AMP, OpSpec::Borrow(BorrowKind::Shared))
        .prefix_op(STAR, OpSpec::Deref);
    b.type_name("int", TypeName::Prim(Prim::I64));
    b.node(
        BLOCK,
        Arch::Block {
            stmts: Pick::Nodes,
            tail: Pick::Absent,
            label: Pick::Absent,
            tail_from_last: true,
        },
    );
    b.node(EXPR, Arch::pass(0));
    b.node(BINARY, Arch::binary());
    b.node(PREFIX, Arch::prefix());
    b.node(
        LET,
        Arch::Let {
            pat: Pick::Nth(1),
            ty: Pick::After(COLON),
            init: Pick::After(EQ),
            else_: Pick::Absent,
            mutable: false,
            hoist: false,
        },
    );
    b.node(
        PATH,
        Arch::Path {
            segments: Pick::All,
            global: Pick::First(COLONCOLON),
            qself: Pick::Absent,
            qtrait: Pick::Absent,
        },
    );
    b.skip(COLONCOLON);
    b.error(ERROR);
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

fn clean(b: MappingBuilder<K>, w: W) -> (Hir, Interner) {
    let (out, names, _) = run(b, w);
    assert!(out.problems.is_empty(), "{:#?}", out.problems);
    (out.hir, names)
}

fn items(hir: &Hir) -> Vec<hir_lang::ItemId> {
    let ItemKind::Module { items, .. } = hir.item(hir.root()).kind else {
        panic!("module")
    };
    hir.list(items).to_vec()
}

fn nodes(hir: &Hir) -> Vec<NodeRef> {
    let mut v = Vec::new();
    hir_lang::walk(hir, |n| v.push(n));
    v
}

fn exprs(hir: &Hir) -> Vec<Expr> {
    nodes(hir)
        .into_iter()
        .filter_map(|n| match n {
            NodeRef::Expr(e) => Some(*hir.expr(e)),
            _ => None,
        })
        .collect()
}

fn pats(hir: &Hir) -> Vec<Pat> {
    nodes(hir)
        .into_iter()
        .filter_map(|n| match n {
            NodeRef::Pat(p) => Some(*hir.pat(p)),
            _ => None,
        })
        .collect()
}

fn tys(hir: &Hir) -> Vec<Ty> {
    nodes(hir)
        .into_iter()
        .filter_map(|n| match n {
            NodeRef::Ty(t) => Some(*hir.ty(t)),
            _ => None,
        })
        .collect()
}

fn problem_kinds(out: &Lowered) -> Vec<ProblemKind> {
    out.problems.iter().map(|p| p.kind).collect()
}

// ------------------------------------------------------------------ tests

#[test]
fn test_methods_generics_where_params_and_async() {
    // impl Thing { pub async fn run<T: Show, 'a>(self, a: i32, /, b: int = 1, ...rest, *, tag) -> T
    //   where T: Show { await x; } }
    let mut b = base();
    b.node(
        IMPL,
        Arch::Impl {
            generics: Pick::Absent,
            interface: Pick::Absent,
            self_ty: Pick::Nth(1),
            items: Pick::Nodes,
        },
    );
    b.node(
        FN,
        Arch::Fn {
            name: Pick::Kind(IDENT),
            generics: Pick::Kind(GENERICS),
            params: Pick::Kind(PARAMS),
            ret: Pick::After(ARROW),
            throws: Pick::Absent,
            body: Pick::Kind(BLOCK),
            where_: Pick::Kind(WHERE),
        },
    );
    b.container(GENERICS).container(PARAMS).container(WHERE);
    b.node(
        GP,
        Arch::GenericParam {
            name: Pick::Nth(0),
            bounds: Pick::AllAfter(COLON),
            ty: Pick::Absent,
            default: Pick::Absent,
        },
    );
    b.node_when(
        PARAM,
        Pick::First(SELFKW),
        Arch::Param {
            pat: Pick::Nth(0),
            ty: Pick::Absent,
            default: Pick::Absent,
        },
    );
    b.node(
        PARAM,
        Arch::Param {
            pat: Pick::Kind(IDENT),
            ty: Pick::After(COLON),
            default: Pick::After(EQ),
        },
    );
    b.node(
        WP,
        Arch::WherePred {
            subject: Pick::Nth(0),
            bounds: Pick::AllAfter(COLON),
        },
    );
    b.node(
        AWAIT,
        Arch::Await {
            inner: Pick::Back(0),
        },
    );
    b.skip(KW);
    let mut w = W::new();
    w.open(FILE)
        .open(IMPL)
        .t(KW, "impl")
        .t(IDENT, "Thing")
        .t(KW, "{");
    w.open(FN)
        .t(PUB, "pub")
        .t(ASYNC, "async")
        .t(KW, "fn")
        .t(IDENT, "run");
    w.open(GENERICS).t(KW, "<");
    w.open(GP)
        .t(IDENT, "T")
        .t(COLON, ":")
        .t(IDENT, "Show")
        .close()
        .t(COMMA, ",");
    w.open(GP).t(LIFETIME, "'a").close().t(KW, ">").close();
    w.open(PARAMS).t(KW, "(");
    w.open(PARAM).t(SELFKW, "self").close().t(COMMA, ",");
    w.open(PARAM)
        .t(IDENT, "a")
        .t(COLON, ":")
        .t(I32, "i32")
        .close()
        .t(COMMA, ",");
    w.t(POSONLY, "/").t(COMMA, ",");
    w.open(PARAM)
        .t(IDENT, "b")
        .t(COLON, ":")
        .t(IDENT, "int")
        .t(EQ, "=")
        .t(NUM, "1")
        .close()
        .t(COMMA, ",");
    w.open(PARAM)
        .t(ELLIPSIS, "...")
        .t(IDENT, "rest")
        .close()
        .t(COMMA, ",");
    w.t(NAMED, "*").t(COMMA, ",");
    w.open(PARAM).t(IDENT, "tag").close().t(KW, ")").close();
    w.t(ARROW, "->").t(IDENT, "T");
    w.open(WHERE)
        .t(KW, "where")
        .open(WP)
        .t(IDENT, "T")
        .t(COLON, ":")
        .t(IDENT, "Show")
        .close()
        .close();
    w.open(BLOCK)
        .t(KW, "{")
        .open(AWAIT)
        .t(KW, "await")
        .t(IDENT, "x")
        .close()
        .t(KW, "}")
        .close();
    w.close().t(KW, "}").close().close();
    // Bare keyword tokens (`impl`, `fn`, `{`) have no role and are not picked.
    let (hir, mut names) = clean(b, w);
    let ItemKind::Impl(imp) = hir.item(items(&hir)[0]).kind else {
        panic!("impl")
    };
    let f = hir.list(imp.items)[0];
    let item = hir.item(f);
    assert_eq!(item.vis, hir_lang::Vis::Public);
    assert_eq!(item.name.map(|n| n.sym), Some(names.intern("run")));
    let ItemKind::Fn(def) = item.kind else {
        panic!("fn")
    };
    assert!(def.effects.contains(Effects::ASYNC));
    assert!(def.ret.is_some());
    let kinds: Vec<ParamKind> = hir
        .list(def.params)
        .iter()
        .map(|&p| hir.param(p).kind)
        .collect();
    assert_eq!(
        kinds,
        [
            ParamKind::Receiver,
            ParamKind::PositionalOnly,
            ParamKind::Normal,
            ParamKind::Rest,
            ParamKind::NamedOnly
        ]
    );
    let b_param = hir.param(hir.list(def.params)[2]);
    assert!(b_param.default.is_some());
    assert!(
        matches!(hir.ty(b_param.ty.unwrap()), Ty::Prim(Prim::I64)),
        "`int` is a type name"
    );
    let gens = hir.list(def.generics.params);
    assert_eq!(gens.len(), 2);
    assert_eq!(
        hir.binder(gens[0].binder).unwrap().kind,
        BinderKind::TypeParam
    );
    assert_eq!(hir.binder(gens[1].binder).unwrap().kind, BinderKind::Region);
    assert_eq!(gens[0].bounds.len(), 1);
    assert_eq!(def.generics.preds.len(), 1);
}

#[test]
fn test_records_unions_sums_aliases_consts_globals_imports() {
    let mut b = base();
    b.node(
        RECORD,
        Arch::Record {
            name: Pick::Nth(1),
            generics: Pick::Absent,
            fields: Pick::AllKind(FIELD),
            shape: RecordShape::Auto,
        },
    );
    b.node(
        FIELD,
        Arch::FieldDecl {
            name: Pick::Kind(IDENT),
            ty: Pick::After(COLON),
            default: Pick::Absent,
        },
    );
    b.node(
        SUM,
        Arch::Sum {
            name: Pick::Nth(1),
            generics: Pick::Absent,
            variants: Pick::Range(2, 0),
        },
    );
    b.node(
        VARIANT,
        Arch::VariantDecl {
            name: Pick::Nth(0),
            fields: Pick::AllKind(FIELD),
            discriminant: Pick::After(EQ),
            shape: RecordShape::Auto,
        },
    );
    b.node(
        ALIAS,
        Arch::Alias {
            name: Pick::Nth(1),
            generics: Pick::Absent,
            ty: Pick::After(EQ),
        },
    );
    b.node(
        CONST,
        Arch::Const {
            name: Pick::Nth(1),
            ty: Pick::After(COLON),
            value: Pick::After(EQ),
        },
    );
    b.node(
        GLOBAL,
        Arch::GlobalVar {
            name: Pick::Kind(IDENT),
            ty: Pick::Absent,
            init: Pick::After(EQ),
            mutable: false,
        },
    );
    b.node(
        IMPORT,
        Arch::Import {
            path: Pick::Kind(PATH),
            alias: Pick::After(AS),
        },
    );
    let mut w = W::new();
    w.open(FILE);
    // record Point { x: i32, y: i32 }
    w.open(RECORD).t(KW, "record").t(IDENT, "Point");
    w.open(FIELD)
        .t(IDENT, "x")
        .t(COLON, ":")
        .t(I32, "i32")
        .close();
    w.open(FIELD)
        .t(IDENT, "y")
        .t(COLON, ":")
        .t(I32, "i32")
        .close()
        .close();
    // union record Bits { (i32) }  — an unnamed field: tuple shape
    w.open(RECORD).t(KW, "record").t(IDENT, "Bits");
    w.open(FIELD).t(COLON, ":").t(I32, "i32").close().close();
    // sum Shape Circle(r: i32) Empty = 3 Unit
    w.open(SUM).t(KW, "sum").t(IDENT, "Shape");
    w.open(VARIANT)
        .t(IDENT, "Circle")
        .open(FIELD)
        .t(IDENT, "r")
        .t(COLON, ":")
        .t(I32, "i32")
        .close()
        .close();
    w.open(VARIANT)
        .t(IDENT, "Empty")
        .t(EQ, "=")
        .t(NUM, "3")
        .close();
    w.t(IDENT, "Unit").close();
    // alias Id = i32; const MAX: i32 = 10; mut global COUNT = 0;
    w.open(RECORD).t(UNION, "union").t(IDENT, "Word");
    w.open(FIELD)
        .t(IDENT, "lo")
        .t(COLON, ":")
        .t(I32, "i32")
        .close()
        .close();
    w.open(ALIAS)
        .t(KW, "alias")
        .t(IDENT, "Id")
        .t(EQ, "=")
        .t(I32, "i32")
        .close();
    w.open(CONST)
        .t(KW, "const")
        .t(IDENT, "MAX")
        .t(COLON, ":")
        .t(I32, "i32")
        .t(EQ, "=")
        .t(NUM, "10")
        .close();
    w.open(GLOBAL)
        .t(MUT, "mut")
        .t(KW, "global")
        .t(IDENT, "COUNT")
        .t(EQ, "=")
        .t(NUM, "0")
        .close();
    // use a::b as c; use a::*;
    w.open(IMPORT)
        .t(KW, "use")
        .open(PATH)
        .t(IDENT, "a")
        .t(COLONCOLON, "::")
        .t(IDENT, "b")
        .close();
    w.t(AS, "as").t(IDENT, "c").close();
    w.open(IMPORT)
        .open(PATH)
        .t(IDENT, "a")
        .close()
        .t(COLONCOLON, "::")
        .t(GLOB, "*")
        .close();
    w.close();
    let (hir, mut names) = clean(b, w);
    let it = items(&hir);
    assert_eq!(it.len(), 9);
    let kinds: Vec<&str> = it.iter().map(|&i| hir.item(i).kind.name()).collect();
    assert_eq!(
        kinds,
        [
            "record", "record", "sum", "record", "alias", "const", "global", "import", "import"
        ]
    );
    let ItemKind::Record(word) = hir.item(it[3]).kind else {
        panic!()
    };
    assert_eq!((word.shape, word.is_union), (Shape::Named, true));
    let it: Vec<_> = it
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != 3)
        .map(|(_, x)| *x)
        .collect();
    let ItemKind::Record(point) = hir.item(it[0]).kind else {
        panic!()
    };
    assert_eq!(
        (point.shape, point.is_union, point.fields.len()),
        (Shape::Named, false, 2)
    );
    let ItemKind::Record(bits) = hir.item(it[1]).kind else {
        panic!()
    };
    assert_eq!((bits.shape, bits.is_union), (Shape::Tuple, false));
    let ItemKind::Sum(shape) = hir.item(it[2]).kind else {
        panic!()
    };
    let vs: Vec<_> = hir
        .list(shape.variants)
        .iter()
        .map(|&v| *hir.variant(v).unwrap())
        .collect();
    assert_eq!(vs.len(), 3);
    assert_eq!(vs[0].shape, Shape::Named);
    assert_eq!(vs[1].shape, Shape::Unit);
    assert!(vs[1].discriminant.is_some());
    assert_eq!(vs[2].name.sym, names.intern("Unit"));
    let ItemKind::Global { mutable, init, .. } = hir.item(it[5]).kind else {
        panic!()
    };
    assert!(mutable && init.is_some());
    let ItemKind::Import { glob, path } = hir.item(it[6]).kind else {
        panic!()
    };
    assert!(!glob);
    assert_eq!(hir.item(it[6]).name.map(|n| n.sym), Some(names.intern("c")));
    assert_eq!(hir.path(path).ns, Ns::Import);
    assert_eq!(hir.path(path).segments.len(), 2);
    let ItemKind::Import { glob, .. } = hir.item(it[7]).kind else {
        panic!()
    };
    assert!(glob);
}

#[test]
fn test_every_pattern_form() {
    // match v { (a, .., b) => 1, Some(y) | Ok(y) => 2, P { x, y: 0, .. } => 3,
    //   1 ..= 5 => 4, & mut z => 5, [f, r @ .., l] => 6, -1 => 7, w @ _ => 8, C => 9, _ => 0 }
    let mut b = base();
    b.node(
        MATCH,
        Arch::Match {
            scrutinee: Pick::Nth(1),
            arms: Pick::AllKind(ARM),
        },
    );
    b.node(
        ARM,
        Arch::Arm {
            pat: Pick::Nth(0),
            guard: Pick::Absent,
            body: Pick::Back(0),
        },
    );
    b.node(TUPLE, Arch::Tuple { elems: Pick::All });
    b.node(
        CALL,
        Arch::Call {
            callee: Pick::Nth(0),
            args: Pick::Range(1, 0),
        },
    );
    b.node(
        RECLIT,
        Arch::RecordLit {
            path: Pick::Nth(0),
            fields: Pick::Range(1, 0),
            base: Pick::Absent,
        },
    );
    b.node(
        INIT,
        Arch::FieldInit {
            name: Pick::Nth(0),
            value: Pick::After(COLON),
        },
    );
    b.node(ORPAT, Arch::OrPat { alts: Pick::Nodes });
    b.node(
        RANGE,
        Arch::RangePat {
            lo: Pick::Nth(0),
            hi: Pick::Back(0),
        },
    );
    b.node(
        REFPAT,
        Arch::RefPat {
            inner: Pick::Back(0),
        },
    );
    b.node(SLICE, Arch::Array { elems: Pick::All });
    b.node(
        BINDAT,
        Arch::BindPat {
            name: Pick::Nth(0),
            sub: Pick::Back(0),
        },
    );
    let mut w = W::new();
    w.open(FILE).open(MATCH).t(KW, "match").t(IDENT, "v");
    let arm = |w: &mut W, pat: &dyn Fn(&mut W), n: &str| {
        w.open(ARM);
        pat(w);
        w.t(ARROW, "=>").t(NUM, n).close();
    };
    arm(
        &mut w,
        &|w| {
            w.open(TUPLE)
                .t(IDENT, "a")
                .t(COMMA, ",")
                .t(DOTDOT, "..")
                .t(COMMA, ",")
                .t(IDENT, "b")
                .close();
        },
        "1",
    );
    arm(
        &mut w,
        &|w| {
            w.open(ORPAT);
            w.open(CALL).t(IDENT, "Some").t(IDENT, "y").close();
            w.open(CALL).t(IDENT, "Ok").t(IDENT, "y").close();
            w.close();
        },
        "2",
    );
    arm(
        &mut w,
        &|w| {
            w.open(RECLIT).t(IDENT, "P").t(IDENT, "x").t(COMMA, ",");
            w.open(INIT)
                .t(IDENT, "y")
                .t(COLON, ":")
                .t(NUM, "0")
                .close()
                .t(COMMA, ",")
                .t(DOTDOT, "..")
                .close();
        },
        "3",
    );
    arm(
        &mut w,
        &|w| {
            w.open(RANGE)
                .t(NUM, "1")
                .t(DOTDOTEQ, "..=")
                .t(IDENT, "FIVE")
                .close();
        },
        "4",
    );
    arm(
        &mut w,
        &|w| {
            w.open(REFPAT)
                .t(AMP, "&")
                .t(MUT, "mut")
                .t(IDENT, "z")
                .close();
        },
        "5",
    );
    arm(
        &mut w,
        &|w| {
            w.open(SLICE).t(IDENT, "f").t(COMMA, ",");
            w.open(BINDAT)
                .t(IDENT, "r")
                .t(KW, "@")
                .t(DOTDOT, "..")
                .close()
                .t(COMMA, ",")
                .t(IDENT, "l")
                .close();
        },
        "6",
    );
    arm(
        &mut w,
        &|w| {
            w.open(PREFIX).t(MINUS, "-").t(NUM, "1").close();
        },
        "7",
    );
    arm(
        &mut w,
        &|w| {
            w.open(BINDAT).t(IDENT, "w").t(KW, "@").t(WILD, "_").close();
        },
        "8",
    );
    arm(
        &mut w,
        &|w| {
            w.t(IDENT, "C");
        },
        "9",
    );
    arm(
        &mut w,
        &|w| {
            w.t(WILD, "_");
        },
        "0",
    );
    w.close().close();
    let (hir, _) = clean(b, w);
    let ps = pats(&hir);
    assert!(
        ps.iter()
            .any(|p| matches!(p, Pat::Tuple { rest: Some(1), .. }))
    );
    assert!(ps.iter().any(|p| matches!(p, Pat::Or(_))));
    assert!(ps.iter().any(|p| matches!(p, Pat::Ctor { .. })));
    assert!(ps.iter().any(|p| matches!(
        p,
        Pat::Record {
            rest: true,
            path: Some(_),
            ..
        }
    )));
    assert!(ps.iter().any(|p| matches!(
        p,
        Pat::Range {
            inclusive: true,
            lo: Some(_),
            hi: Some(_)
        }
    )));
    assert!(
        ps.iter().any(|p| matches!(p, Pat::Path(_))),
        "a range bound name is a constant path"
    );
    assert!(
        ps.iter()
            .any(|p| matches!(p, Pat::Ref { mutable: true, .. }))
    );
    assert!(
        ps.iter()
            .any(|p| matches!(p, Pat::Slice { rest: Some(r), .. } if r.bind.is_some()))
    );
    assert!(
        ps.iter()
            .any(|p| matches!(p, Pat::Lit(Lit::Int(i)) if i.negative && i.value == 1))
    );
    assert!(
        ps.iter()
            .any(|p| matches!(p, Pat::Bind { sub: Some(_), .. }))
    );
    assert!(
        ps.iter().any(|p| matches!(p, Pat::Ident { .. })),
        "a bare arm identifier is left to the resolver"
    );
    assert!(ps.iter().any(|p| matches!(p, Pat::Wild)));
    // `Some(y) | Ok(y)`: both alternatives bind one binder.
    let or_binders: Vec<_> = ps
        .iter()
        .filter_map(|p| match p {
            Pat::Ident { binder, .. } | Pat::Bind { binder, .. } => Some(*binder),
            _ => None,
        })
        .collect();
    let y = or_binders
        .iter()
        .filter(|b| or_binders.iter().filter(|c| c == b).count() == 2)
        .count();
    assert_eq!(y, 2, "y is shared by both alternatives");
}

#[test]
fn test_every_type_form() {
    // let a: (i32, int) ; let b: [i32; 4]; let c: &'r mut T; let d: fn(i32) -> T;
    // let e: T?; let f: dyn Show; let g: *mut i32; let h: [T]; let i: Vec::<i32>;
    // let j: Iter<Item = u8>; let k: _;
    let mut b = base();
    b.node(TUPLE, Arch::Tuple { elems: Pick::All });
    b.node(
        ARRAYTY,
        Arch::ArrayTy {
            elem: Pick::Nth(0),
            len: Pick::Back(0),
        },
    );
    b.node(
        REFTY,
        Arch::RefTy {
            inner: Pick::Back(0),
            region: Pick::Kind(LIFETIME),
        },
    );
    b.node(
        FNTY,
        Arch::FnTy {
            params: Pick::Kind(TUPLE),
            ret: Pick::After(ARROW),
        },
    );
    b.container(TUPLE);
    b.node(
        NULLTY,
        Arch::NullableTy {
            inner: Pick::Nth(0),
        },
    );
    b.node(
        DYNTY,
        Arch::ObjectTy {
            bounds: Pick::AllKind(IDENT),
        },
    );
    b.node(
        PTRTY,
        Arch::PtrTy {
            inner: Pick::Back(0),
        },
    );
    b.node(SLICETY, Arch::SliceTy { elem: Pick::Nth(0) });
    b.node(
        SEG,
        Arch::Segment {
            name: Pick::Nth(0),
            args: Pick::Range(1, 0),
        },
    );
    b.node(
        BINDING,
        Arch::AssocBinding {
            name: Pick::Nth(0),
            ty: Pick::Back(0),
        },
    );
    let mut w = W::new();
    w.open(FILE);
    let decl = |w: &mut W, name: &str, ty: &dyn Fn(&mut W)| {
        w.open(LET).t(KW, "let").t(IDENT, name).t(COLON, ":");
        ty(w);
        w.t(SEMI, ";").close();
    };
    decl(&mut w, "a", &|w| {
        w.open(TUPLE)
            .t(I32, "i32")
            .t(COMMA, ",")
            .t(IDENT, "int")
            .close();
    });
    decl(&mut w, "b", &|w| {
        w.open(ARRAYTY)
            .t(I32, "i32")
            .t(SEMI, ";")
            .t(NUM, "4")
            .close();
    });
    decl(&mut w, "c", &|w| {
        w.open(REFTY)
            .t(AMP, "&")
            .t(LIFETIME, "'r")
            .t(MUT, "mut")
            .t(IDENT, "T")
            .close();
    });
    decl(&mut w, "d", &|w| {
        w.open(FNTY)
            .t(KW, "fn")
            .open(TUPLE)
            .t(I32, "i32")
            .close()
            .t(ARROW, "->")
            .t(IDENT, "T")
            .close();
    });
    decl(&mut w, "e", &|w| {
        w.open(NULLTY).t(IDENT, "T").t(QUESTION, "?").close();
    });
    decl(&mut w, "f", &|w| {
        w.open(DYNTY).t(KW, "dyn").t(IDENT, "Show").close();
    });
    decl(&mut w, "g", &|w| {
        w.open(PTRTY)
            .t(STAR, "*")
            .t(MUT, "mut")
            .t(I32, "i32")
            .close();
    });
    decl(&mut w, "h", &|w| {
        w.open(SLICETY).t(IDENT, "T").close();
    });
    decl(&mut w, "i", &|w| {
        w.open(PATH)
            .open(SEG)
            .t(IDENT, "Vec")
            .t(I32, "i32")
            .close()
            .t(COLONCOLON, "::")
            .t(IDENT, "Inner")
            .close();
    });
    decl(&mut w, "j", &|w| {
        w.open(SEG)
            .t(IDENT, "Iter")
            .open(BINDING)
            .t(IDENT, "Item")
            .t(EQ, "=")
            .t(I32, "i32")
            .close()
            .close();
    });
    decl(&mut w, "k", &|w| {
        w.t(WILD, "_");
    });
    w.close();
    let (hir, _) = clean(b, w);
    let ts = tys(&hir);
    assert!(ts.iter().any(|t| matches!(t, Ty::Tuple(l) if l.len() == 2)));
    assert!(ts.iter().any(|t| matches!(t, Ty::Prim(Prim::I64))));
    assert!(ts.iter().any(|t| matches!(t, Ty::Array { .. })));
    assert!(ts.iter().any(|t| matches!(
        t,
        Ty::Ref {
            mutable: true,
            region: Some(_),
            ..
        }
    )));
    assert!(ts.iter().any(|t| matches!(t, Ty::Fn { .. })));
    assert!(ts.iter().any(|t| matches!(t, Ty::Nullable(_))));
    assert!(
        ts.iter()
            .any(|t| matches!(t, Ty::Object(b) if b.len() == 1))
    );
    assert!(
        ts.iter()
            .any(|t| matches!(t, Ty::Ptr { mutable: true, .. }))
    );
    assert!(ts.iter().any(|t| matches!(t, Ty::Slice(_))));
    assert!(ts.iter().any(|t| matches!(t, Ty::Infer)));
    let paths: Vec<_> = ts
        .iter()
        .filter_map(|t| match t {
            Ty::Path(p) => Some(*hir.path(*p)),
            _ => None,
        })
        .collect();
    let vec_path = paths
        .iter()
        .find(|p| p.segments.len() == 2)
        .expect("Vec::<i32>::Inner");
    assert_eq!(hir.list(vec_path.segments)[0].args.len(), 1);
    let iter = paths
        .iter()
        .find(|p| hir.list(p.segments)[0].args.len() == 1 && p.segments.len() == 1);
    let iter = iter.expect("Iter<Item = i32>");
    assert!(matches!(
        hir.list(hir.list(iter.segments)[0].args)[0],
        hir_lang::GenericArg::Binding { .. }
    ));
}

#[test]
fn test_path_roots_and_qualified_paths() {
    // super::super::a; ::std::x; Self::new; <T as Tr>::Out;
    let mut b = base();
    b.node(K(170), Arch::ExprStmt { expr: Pick::Nth(0) });
    b.node(
        K(171),
        Arch::Path {
            segments: Pick::AllAfter(GT),
            global: Pick::Absent,
            qself: Pick::Nth(1),
            qtrait: Pick::Between(KW, GT),
        },
    );
    let mut w = W::new();
    w.open(FILE);
    let stmt = |w: &mut W, path: &dyn Fn(&mut W)| {
        w.open(K(170));
        path(w);
        w.t(SEMI, ";").close();
    };
    stmt(&mut w, &|w| {
        w.open(PATH)
            .t(SUPER, "super")
            .t(COLONCOLON, "::")
            .t(SUPER, "super")
            .t(COLONCOLON, "::")
            .t(IDENT, "a")
            .close();
    });
    stmt(&mut w, &|w| {
        w.open(PATH)
            .t(COLONCOLON, "::")
            .t(IDENT, "std")
            .t(COLONCOLON, "::")
            .t(IDENT, "x")
            .close();
    });
    stmt(&mut w, &|w| {
        w.open(PATH)
            .t(SELFTY, "Self")
            .t(COLONCOLON, "::")
            .t(IDENT, "new")
            .close();
    });
    stmt(&mut w, &|w| {
        w.open(K(171))
            .t(K(172), "<")
            .t(IDENT, "T")
            .t(KW, "as")
            .t(IDENT, "Tr")
            .t(GT, ">")
            .t(COLONCOLON, "::");
        w.t(IDENT, "Out").close();
    });
    w.close();
    let (hir, _) = clean(b, w);
    let paths: Vec<_> = (0..hir.count(IdKind::Path))
        .map(|i| *hir.path(hir_lang::PathId::from_index(i).unwrap()))
        .filter(|p| p.ns == Ns::Value)
        .collect();
    assert!(
        paths
            .iter()
            .any(|p| p.root == PathRoot::Super(2) && p.segments.len() == 1)
    );
    assert!(
        paths
            .iter()
            .any(|p| p.root == PathRoot::Global && p.segments.len() == 2)
    );
    assert!(
        paths
            .iter()
            .any(|p| p.root == PathRoot::SelfType && p.unresolved == 1)
    );
    let q = paths.iter().find(|p| p.qself.is_some()).expect("qualified");
    assert_eq!(q.qself.unwrap().trait_len, 1);
    assert_eq!((q.segments.len(), q.unresolved), (2, 1));
}

#[test]
fn test_labels_loops_and_unknown_labels() {
    // 'outer: loop { loop { break 'outer 1; continue 'outer; break; break 'nope; } }
    let mut b = base();
    b.node(
        LOOP,
        Arch::Loop {
            label: Pick::Kind(LABEL),
            body: Pick::Kind(BLOCK),
        },
    );
    b.node(
        BREAK,
        Arch::Break {
            label: Pick::Kind(LABEL),
            value: Pick::Kind(NUM),
        },
    );
    b.node(
        CONTINUE,
        Arch::Continue {
            label: Pick::Kind(LABEL),
        },
    );
    b.node(K(170), Arch::ExprStmt { expr: Pick::Nth(0) });
    let mut w = W::new();
    w.open(FILE)
        .open(K(170))
        .open(LOOP)
        .t(LABEL, "'outer")
        .t(KW, "loop")
        .open(BLOCK);
    w.open(K(170)).open(LOOP).t(KW, "loop").open(BLOCK);
    w.open(K(170))
        .open(BREAK)
        .t(KW, "break")
        .t(LABEL, "'outer")
        .t(NUM, "1")
        .close()
        .close();
    w.open(K(170))
        .open(CONTINUE)
        .t(KW, "continue")
        .t(LABEL, "'outer")
        .close()
        .close();
    w.open(K(170)).open(BREAK).t(KW, "break").close().close();
    w.open(K(170))
        .open(BREAK)
        .t(KW, "break")
        .t(LABEL, "'nope")
        .close()
        .close();
    w.close().close().close();
    w.close().close().close().close();
    let (out, _, _) = run(b, w);
    assert_eq!(problem_kinds(&out), [ProblemKind::UnknownLabel]);
    let es = exprs(&out.hir);
    let outer = es.iter().find_map(|e| match e {
        Expr::Loop { label: Some(l), .. } => Some(*l),
        _ => None,
    });
    let outer = outer.expect("labeled loop");
    assert!(
        es.iter()
            .any(|e| matches!(e, Expr::Break { label: Some(l), value: Some(_) } if *l == outer))
    );
    assert!(
        es.iter()
            .any(|e| matches!(e, Expr::Continue { label: Some(l) } if *l == outer))
    );
    assert!(es.iter().any(|e| matches!(
        e,
        Expr::Break {
            label: None,
            value: None
        }
    )));
}

#[test]
fn test_hoisted_var_and_splices() {
    // fn f() { x = 1; var x = 2; var y; var x = 3, z = 4; }
    let mut b = base();
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
    let var = K(172);
    b.node(
        var,
        Arch::Let {
            pat: Pick::Nth(1),
            ty: Pick::Absent,
            init: Pick::After(EQ),
            else_: Pick::Absent,
            mutable: true,
            hoist: true,
        },
    );
    b.node(MULTI, Arch::Splice { items: Pick::Nodes });
    b.node(K(170), Arch::ExprStmt { expr: Pick::Nth(0) });
    let mut w = W::new();
    w.open(FILE)
        .open(FN)
        .t(KW, "fn")
        .t(IDENT, "f")
        .open(BLOCK)
        .t(KW, "{");
    w.open(K(170))
        .open(BINARY)
        .t(IDENT, "x")
        .t(EQ, "=")
        .t(NUM, "1")
        .close()
        .t(SEMI, ";")
        .close();
    w.open(var)
        .t(KW, "var")
        .t(IDENT, "x")
        .t(EQ, "=")
        .t(NUM, "2")
        .close();
    w.open(var).t(KW, "var").t(IDENT, "y").close();
    w.open(MULTI)
        .open(var)
        .t(KW, "var")
        .t(IDENT, "x")
        .t(EQ, "=")
        .t(NUM, "3")
        .close()
        .t(COMMA, ",");
    w.open(var)
        .t(KW, "var")
        .t(IDENT, "z")
        .t(EQ, "=")
        .t(NUM, "4")
        .close()
        .close();
    w.t(KW, "}").close().close().close();
    let (hir, mut names) = clean(b, w);
    let ItemKind::Fn(def) = hir.item(items(&hir)[0]).kind else {
        panic!()
    };
    let Expr::Block(outer) = *hir.expr(def.body.unwrap()) else {
        panic!()
    };
    let lets: Vec<_> = hir
        .list(outer.stmts)
        .iter()
        .filter_map(|&s| match *hir.stmt(s) {
            Stmt::Let {
                pat, init: None, ..
            } => match *hir.pat(pat) {
                Pat::Bind { binder, .. } => Some(hir.binder(binder).unwrap().name.sym),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(
        lets,
        [names.intern("x"), names.intern("y"), names.intern("z")]
    );
    // The original body: `x = 1`, `x = 2`, (nothing for `var y`), `x = 3`, `z = 4`.
    let Expr::Block(inner) = *hir.expr(outer.tail.unwrap()) else {
        panic!()
    };
    assert_eq!(inner.stmts.len(), 4);
}

#[test]
fn test_literals_decode_and_report() {
    // 0x1F; 0b101; 0o17; 1_000; 99999999999999999999999; 2.5; 1e3; 7u8;
    // 'c'; b"\x00\x7f"; "a\tb"; 0xZZ; "\q"; 'ab'
    let mut b = base();
    b.suffix("u8", Prim::U8);
    b.node(K(170), Arch::ExprStmt { expr: Pick::Nth(0) });
    let mut w = W::new();
    w.open(FILE);
    for (k, text) in [
        (NUM, "0x1F"),
        (NUM, "0b101"),
        (NUM, "0o17"),
        (NUM, "1_000"),
        (NUM, "99999999999999999999999"),
        (NUM, "2.5"),
        (NUM, "1e3"),
        (NUM, "7u8"),
        (CHAR, "'c'"),
        (BYTES, "b\"\\x00\\x7f\""),
        (STR, "\"a\\tb\\u{1F600}\""),
        (NUM, "0xZZ"),
        (STR, "\"\\q\""),
        (CHAR, "'ab'"),
    ] {
        w.open(K(170)).t(k, text).t(SEMI, ";").close();
    }
    w.close();
    let (out, _, _) = run(b, w);
    assert_eq!(
        problem_kinds(&out),
        [
            ProblemKind::InvalidLiteral,
            ProblemKind::InvalidEscape,
            ProblemKind::InvalidLiteral
        ]
    );
    let hir = &out.hir;
    let lits: Vec<Lit> = exprs(hir)
        .into_iter()
        .filter_map(|e| match e {
            Expr::Lit(l) => Some(l),
            _ => None,
        })
        .collect();
    let ints: Vec<u64> = lits
        .iter()
        .filter_map(|l| match l {
            Lit::Int(i) => Some(i.value),
            _ => None,
        })
        .collect();
    assert_eq!(ints, [31, 5, 15, 1000, 7]);
    assert!(
        lits.iter()
            .any(|l| matches!(l, Lit::Int(i) if i.suffix == Some(Prim::U8)))
    );
    assert!(
        lits.iter()
            .any(|l| matches!(l, Lit::BigInt(t) if hir.str(*t) == "99999999999999999999999"))
    );
    let floats: Vec<f64> = lits
        .iter()
        .filter_map(|l| match l {
            Lit::Float(f) => Some(f.value()),
            _ => None,
        })
        .collect();
    assert_eq!(floats, [2.5, 1000.0]);
    assert!(lits.iter().any(|l| matches!(l, Lit::Char('c'))));
    assert!(
        lits.iter()
            .any(|l| matches!(l, Lit::Bytes(t) if hir.text(*t) == [0u8, 0x7f]))
    );
    assert!(
        lits.iter()
            .any(|l| matches!(l, Lit::Str(t) if hir.str(*t) == "a\tb\u{1F600}"))
    );
    assert!(
        lits.iter()
            .any(|l| matches!(l, Lit::Str(t) if hir.str(*t) == "\\q")),
        "an unknown escape is kept as written"
    );
}

#[test]
fn test_context_errors_are_problems_not_panics() {
    // A `let` used as an expression, an unmapped node, an unsupported node, a
    // syntax error node, an unknown operator, a missing required field, and a
    // name token where a type is expected all become error forms.
    let mut b = base();
    b.node(K(170), Arch::ExprStmt { expr: Pick::Nth(0) });
    b.node(UNSUP, Arch::Unsupported);
    b.node(TUPLE, Arch::Tuple { elems: Pick::All });
    b.node(
        K(173),
        Arch::If {
            cond: Pick::Kind(TRUE),
            then: Pick::Kind(BLOCK),
            else_: Pick::Absent,
        },
    );
    b.node(
        CAST,
        Arch::Cast {
            expr: Pick::Nth(0),
            ty: Pick::Back(0),
        },
    );
    let mut w = W::new();
    w.open(FILE);
    w.open(K(170))
        .open(LET)
        .t(KW, "let")
        .t(IDENT, "x")
        .close()
        .t(SEMI, ";")
        .close();
    w.open(K(170))
        .open(K(199))
        .t(IDENT, "mystery")
        .close()
        .t(SEMI, ";")
        .close();
    w.open(K(170))
        .open(UNSUP)
        .t(KW, "goto")
        .close()
        .t(SEMI, ";")
        .close();
    w.open(K(170))
        .open(ERROR)
        .t(KW, "@@")
        .close()
        .t(SEMI, ";")
        .close();
    w.open(K(170))
        .open(BINARY)
        .t(NUM, "1")
        .t(BANG, "!")
        .t(NUM, "2")
        .close()
        .t(SEMI, ";")
        .close();
    w.open(K(170))
        .open(K(173))
        .t(KW, "if")
        .open(BLOCK)
        .close()
        .close()
        .t(SEMI, ";")
        .close();
    w.open(K(170))
        .open(CAST)
        .t(NUM, "1")
        .t(KW, "as")
        .t(STR, "\"s\"")
        .close()
        .t(SEMI, ";")
        .close();
    w.close();
    let (out, _, _) = run(b, w);
    assert_eq!(
        problem_kinds(&out),
        [
            ProblemKind::NotAllowedHere {
                expected: lower_lang::Context::Expression
            },
            ProblemKind::Unmapped,
            ProblemKind::Unsupported,
            ProblemKind::SyntaxError,
            ProblemKind::UnknownOperator,
            ProblemKind::MissingField { field: "cond" },
            ProblemKind::NotAllowedHere {
                expected: lower_lang::Context::Type
            },
        ]
    );
    assert_eq!(common::error_exprs(&out.hir), 6);
}

#[test]
fn test_closures_calls_members_and_misc_expressions() {
    let mut b = base();
    b.node(K(170), Arch::ExprStmt { expr: Pick::Nth(0) });
    b.node(
        CLOSURE,
        Arch::Closure {
            params: Pick::Kind(PARAMS),
            ret: Pick::Absent,
            captures: Pick::Absent,
            body: Pick::Back(0),
            implicit: Some(CaptureMode::ByRef),
        },
    );
    b.container(PARAMS);
    b.node(
        SPAWN,
        Arch::Spawn {
            inner: Pick::Back(0),
        },
    );
    b.node(
        DEFER,
        Arch::Defer {
            body: Pick::Back(0),
        },
    );
    b.node(
        REPEAT,
        Arch::Repeat {
            elem: Pick::Nth(0),
            count: Pick::Back(0),
        },
    );
    b.node(
        CAST,
        Arch::Cast {
            expr: Pick::Nth(0),
            ty: Pick::Back(0),
        },
    );
    b.node(
        MEMBER,
        Arch::Member {
            base: Pick::Nth(0),
            member: Pick::Back(0),
            args: Pick::Absent,
            call: Pick::Absent,
        },
    );
    b.node(
        MCALL,
        Arch::MethodCall {
            receiver: Pick::Nth(0),
            method: Pick::Nth(2),
            generics: Pick::Kind(GENERICS),
            args: Pick::Kind(ARGS),
        },
    );
    b.container(GENERICS).container(ARGS);
    b.node(
        CALL,
        Arch::Call {
            callee: Pick::Nth(0),
            args: Pick::Kind(ARGS),
        },
    );
    b.node(
        ARG,
        Arch::Arg {
            name: Pick::Before(COLON),
            value: Pick::Back(0),
        },
    );
    b.modifier(K(174), Modifier::Spread);
    b.node(
        RECLIT,
        Arch::RecordLit {
            path: Pick::Nth(0),
            fields: Pick::AllKind(INIT),
            base: Pick::After(DOTDOT),
        },
    );
    b.node(
        INIT,
        Arch::FieldInit {
            name: Pick::Nth(0),
            value: Pick::After(COLON),
        },
    );
    b.node(
        K(175),
        Arch::Yield {
            value: Pick::Back(0),
        },
    );
    let mut w = W::new();
    w.open(FILE);
    // spawn |x| x + y;
    w.open(K(170))
        .open(SPAWN)
        .t(KW, "spawn")
        .open(CLOSURE)
        .open(PARAMS)
        .t(IDENT, "x")
        .close();
    w.open(BINARY)
        .t(IDENT, "x")
        .t(PLUS, "+")
        .t(IDENT, "y")
        .close()
        .close()
        .close()
        .t(SEMI, ";")
        .close();
    // defer &mut *p;
    w.open(DEFER)
        .t(KW, "defer")
        .open(PREFIX)
        .t(AMP, "&")
        .open(PREFIX)
        .t(STAR, "*")
        .t(IDENT, "p");
    w.close().close().close();
    // [0; 8] as i32; t.0; o.f::<i32>(n: 1, ...rest); P { x, y: 2, ..q }; f(1);
    w.open(K(170))
        .open(CAST)
        .open(REPEAT)
        .t(NUM, "0")
        .t(SEMI, ";")
        .t(NUM, "8")
        .close()
        .t(KW, "as")
        .t(I32, "i32");
    w.close().t(SEMI, ";").close();
    w.open(K(170))
        .open(MEMBER)
        .t(IDENT, "t")
        .t(KW, ".")
        .t(NUM, "0")
        .close()
        .t(SEMI, ";")
        .close();
    w.open(K(170))
        .open(MCALL)
        .t(IDENT, "o")
        .t(KW, ".")
        .t(IDENT, "f");
    w.open(GENERICS).t(I32, "i32").close().open(ARGS);
    w.open(ARG)
        .t(IDENT, "n")
        .t(COLON, ":")
        .t(NUM, "1")
        .close()
        .t(COMMA, ",");
    w.open(ARG)
        .t(K(174), "...")
        .t(IDENT, "rest")
        .close()
        .close()
        .close()
        .t(SEMI, ";")
        .close();
    w.open(K(170))
        .open(RECLIT)
        .t(IDENT, "P")
        .open(INIT)
        .t(IDENT, "x")
        .close();
    w.open(INIT)
        .t(IDENT, "y")
        .t(COLON, ":")
        .t(NUM, "2")
        .close()
        .t(DOTDOT, "..")
        .t(IDENT, "q")
        .close();
    w.t(SEMI, ";").close();
    w.open(K(170))
        .open(K(175))
        .t(KW, "yield")
        .t(NUM, "1")
        .close()
        .t(SEMI, ";")
        .close();
    w.close();
    let (out, _, _) = run(b, w);
    // `yield` outside a generator is hir-lang's to report (lenient), and is.
    assert_eq!(out.problems.len(), 1, "{:?}", out.problems);
    assert!(matches!(
        out.problems[0].kind,
        ProblemKind::Hir(hir_lang::HirError::Effect { .. })
    ));
    let hir = &out.hir;
    let es = exprs(hir);
    let c = es.iter().find_map(|e| match e {
        Expr::Closure(c) => Some(*c),
        _ => None,
    });
    let c = c.expect("closure");
    assert_eq!(c.implicit, Some(CaptureMode::ByRef));
    assert_eq!(c.params.len(), 1);
    assert!(es.iter().any(|e| matches!(e, Expr::Spawn(_))));
    assert!(es.iter().any(|e| matches!(
        e,
        Expr::Borrow {
            kind: BorrowKind::Shared,
            ..
        }
    )));
    assert!(es.iter().any(|e| matches!(e, Expr::Deref(_))));
    assert!(es.iter().any(|e| matches!(e, Expr::Repeat { .. })));
    assert!(es.iter().any(|e| matches!(e, Expr::Cast { .. })));
    assert!(es.iter().any(|e| matches!(
        e,
        Expr::Field {
            member: Member::Index(0),
            ..
        }
    )));
    let (gargs, args) = es
        .iter()
        .find_map(|e| match e {
            Expr::MethodCall {
                generic_args, args, ..
            } => Some((*generic_args, *args)),
            _ => None,
        })
        .expect("method call");
    assert_eq!(gargs.len(), 1);
    let kinds: Vec<_> = hir.list(args).iter().map(|a| a.kind).collect();
    assert!(matches!(kinds[0], ArgKind::Named(_)));
    assert_eq!(kinds[1], ArgKind::Spread);
    assert!(
        es.iter()
            .any(|e| matches!(e, Expr::Record { base: Some(_), fields, .. } if fields.len() == 2))
    );
    let stmts = nodes(hir)
        .into_iter()
        .filter(|n| matches!(n, NodeRef::Stmt(s) if matches!(hir.stmt(*s), Stmt::Defer(_))))
        .count();
    assert_eq!(stmts, 1);
}

#[test]
fn test_nested_modules_traits_try_and_units() {
    let mut b = base();
    b.node(
        MODULE,
        Arch::Module {
            name: Pick::Nth(1),
            items: Pick::Nodes,
        },
    );
    b.node(
        TRAIT,
        Arch::Interface {
            name: Pick::Nth(1),
            generics: Pick::Absent,
            supers: Pick::After(COLON),
            items: Pick::Nodes,
        },
    );
    b.node(
        ASSOC,
        Arch::AssocType {
            name: Pick::Nth(1),
            bounds: Pick::AllAfter(COLON),
            default: Pick::Absent,
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
        TRY,
        Arch::Try {
            body: Pick::Nth(1),
            catches: Pick::Absent,
            finally: Pick::Back(0),
        },
    );
    b.node(K(170), Arch::ExprStmt { expr: Pick::Nth(0) });
    let mut w = W::new();
    w.open(FILE).open(MODULE).t(KW, "mod").t(IDENT, "inner");
    w.open(TRAIT)
        .t(KW, "trait")
        .t(IDENT, "Iter")
        .t(COLON, ":")
        .t(IDENT, "Base");
    w.open(ASSOC)
        .t(KW, "type")
        .t(IDENT, "Item")
        .t(COLON, ":")
        .t(IDENT, "Show")
        .close();
    w.open(FN).t(KW, "fn").t(IDENT, "next").close().close();
    w.open(K(170))
        .open(TRY)
        .t(KW, "try")
        .open(BLOCK)
        .close()
        .t(KW, "finally")
        .open(BLOCK)
        .close()
        .close();
    w.t(SEMI, ";").close();
    w.close().close();
    let mapping = b.build().unwrap();
    let (tree, src) = w.finish();
    let mut names = Interner::new();
    let out = Lowerer::new(&mapping)
        .with_unit(UnitId::new(7))
        .lower(&tree, &src, &mut names)
        .unwrap();
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    assert_eq!(out.hir.unit(), UnitId::new(7));
    let hir = &out.hir;
    let inner = items(hir)[0];
    assert_eq!(
        hir.item(inner).name.map(|n| n.sym),
        Some(names.intern("inner"))
    );
    let ItemKind::Module {
        items: its, body, ..
    } = hir.item(inner).kind
    else {
        panic!()
    };
    assert!(
        body.is_some(),
        "the try is top-level code of the nested module"
    );
    let ItemKind::Interface(tr) = hir.item(hir.list(its)[0]).kind else {
        panic!()
    };
    assert_eq!(tr.supers.len(), 1);
    let members: Vec<&str> = hir
        .list(tr.items)
        .iter()
        .map(|&i| hir.item(i).kind.name())
        .collect();
    assert_eq!(members, ["assoc-type", "fn"]);
    assert!(exprs(hir).iter().any(|e| matches!(
        e,
        Expr::Try {
            finally: Some(_),
            ..
        }
    )));
}

#[test]
fn test_spans_outside_the_source_are_reported() {
    let mut b = base();
    b.node(K(170), Arch::ExprStmt { expr: Pick::Nth(0) });
    let mapping = b.build().unwrap();
    let tok = syntax_lang::Token::new(IDENT, syntax_lang::Span::new(50, 60));
    let stmt = Node::new(K(170), vec![syntax_lang::Element::Token(tok)]);
    let file = Node::new(FILE, vec![syntax_lang::Element::Node(stmt)]);
    let mut names = Interner::new();
    let out = lower_lang::lower(&file, "short", &mapping, &mut names).unwrap();
    assert_eq!(out.hir.validate(), Ok(()));
    assert!(problem_kinds(&out).contains(&ProblemKind::BadSpan));
}

#[test]
fn test_field_labels_from_a_labeler() {
    // `assign` nodes whose children carry host labels 1 (target) and 2 (value).
    struct ByPosition;
    impl Labeler<K> for ByPosition {
        fn label(&self, parent: &Node<K>, index: usize) -> Option<u16> {
            if *parent.kind() != K(176) {
                return None;
            }
            // Children: target, space, `=`, space, value.
            match index {
                0 => Some(1),
                4 => Some(2),
                _ => None,
            }
        }
    }
    let mut b = base();
    b.node(K(170), Arch::ExprStmt { expr: Pick::Nth(0) });
    b.node(
        K(176),
        Arch::Assign {
            target: Pick::Label(1),
            op: Pick::Absent,
            value: Pick::Label(2),
        },
    );
    let mapping = b.build().unwrap();
    let mut w = W::new();
    w.open(FILE)
        .open(K(170))
        .open(K(176))
        .t(IDENT, "a")
        .t(EQ, "=")
        .t(NUM, "5")
        .close()
        .close()
        .close();
    let (tree, src) = w.finish();
    let mut names = Interner::new();
    let out = Lowerer::new(&mapping)
        .with_labeler(&ByPosition)
        .lower(&tree, &src, &mut names)
        .unwrap();
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    assert!(
        exprs(&out.hir)
            .iter()
            .any(|e| matches!(e, Expr::Assign { op: None, .. }))
    );
    // Without the labeler the labels find nothing: both sides are missing.
    let out = lower_lang::lower(&tree, &src, &mapping, &mut names).unwrap();
    assert_eq!(out.problems.len(), 2);
}

#[test]
fn test_templates_compose_with_apply_and_the_depth_budget() {
    // until c { b } = while !c { b }; a chain of templates each applying the
    // previous one; a depth budget below the chain cuts it with a problem and
    // keeps the user code attached.
    let mut b = base();
    let w_id = b.template(templates::while_loop()).unwrap();
    let mut prev = w_id;
    for i in 0..4 {
        let mut t = TemplateBuilder::new(&format!("until{i}"));
        let (c, body) = (t.expr_hole("cond"), t.expr_hole("body"));
        let c = t.hole(c);
        let not_c = t.op(OpKind::Not, &[c]);
        let body = t.hole(body);
        let args: Vec<TArg> = if prev == w_id {
            vec![TArg::Expr(not_c), TArg::Expr(body), TArg::Absent]
        } else {
            vec![TArg::Expr(not_c), TArg::Expr(body)]
        };
        let root = t.apply(prev, &args);
        prev = b.template(t.finish(root).unwrap()).unwrap();
    }
    b.template_node(TPL, prev, &[Pick::Nth(1), Pick::Back(0)]);
    b.node(K(170), Arch::ExprStmt { expr: Pick::Nth(0) });
    let mapping = b.build().unwrap();
    let mut w = W::new();
    w.open(FILE)
        .open(K(170))
        .open(TPL)
        .t(KW, "until")
        .t(IDENT, "done");
    w.open(BLOCK)
        .open(K(170))
        .t(IDENT, "work")
        .close()
        .close()
        .close()
        .close()
        .close();
    let (tree, src) = w.finish();

    let mut names = Interner::new();
    let out = lower_lang::lower(&tree, &src, &mapping, &mut names).unwrap();
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    // Five expansions (until3 → until2 → until1 → until0 → while), each the
    // parent of the next; the innermost is the desugaring.
    let n = out.hir.count(IdKind::Expansion);
    assert_eq!(n, 5);
    let last = out
        .hir
        .expansion(hir_lang::ExpnId::from_u32(n as u32))
        .unwrap();
    assert_eq!(last.kind, ExpnKind::Desugar);
    assert!(!last.parent.is_root());
    assert!(
        exprs(&out.hir)
            .iter()
            .any(|e| matches!(e, Expr::Loop { .. }))
    );

    let mut budget = Budget::default();
    budget.max_expansion_depth = 3;
    let out = Lowerer::new(&mapping)
        .with_budget(budget)
        .lower(&tree, &src, &mut names)
        .unwrap();
    assert!(
        problem_kinds(&out)
            .iter()
            .any(|k| matches!(k, ProblemKind::ExpansionTooDeep { .. }))
    );
    assert_eq!(out.hir.validate(), Ok(()));
    // The user's `done` and `work` are still in the HIR (rescued, not lost).
    let printed = hir_lang::print(&out.hir, &names);
    assert!(
        printed.contains("(path done") && printed.contains("(path work"),
        "{printed}"
    );
}

#[test]
fn test_node_budget_stops_lowering() {
    let mut b = base();
    b.node(K(170), Arch::ExprStmt { expr: Pick::Nth(0) });
    let mapping = b.build().unwrap();
    let mut w = W::new();
    w.open(FILE);
    for _ in 0..50 {
        w.open(K(170))
            .open(BINARY)
            .t(NUM, "1")
            .t(PLUS, "+")
            .t(NUM, "2")
            .close()
            .t(SEMI, ";")
            .close();
    }
    w.close();
    let (tree, src) = w.finish();
    let mut budget = Budget::default();
    budget.max_nodes = 40;
    let mut names = Interner::new();
    let err = Lowerer::new(&mapping)
        .with_budget(budget)
        .lower(&tree, &src, &mut names)
        .unwrap_err();
    assert_eq!(err, LowerError::NodeBudget { limit: 40 });
}

#[test]
fn test_builtin_sugar_templates() {
    // do { x } while c; for (let i = 0; ; i = i + 1) { }; x ?? y; e?; x++ (place hole used twice).
    let mut b = base();
    let dw = b.template(templates::do_while()).unwrap();
    let cf = b.template(templates::c_for()).unwrap();
    let co = b.template(templates::coalesce()).unwrap();
    let tq = b.template(templates::try_operator()).unwrap();
    let pi = b.template(templates::post_increment()).unwrap();
    let ov = b.template(templates::or_value()).unwrap();
    let av = b.template(templates::and_value()).unwrap();
    b.template_node(K(180), dw, &[Pick::Nth(1), Pick::Back(0), Pick::Absent]);
    b.template_node(
        K(181),
        cf,
        &[
            Pick::Kind(LET),
            Pick::Absent,
            Pick::Kind(BINARY),
            Pick::Kind(BLOCK),
            Pick::Absent,
        ],
    );
    b.binary_op(QUESTION, OpSpec::Template(co));
    b.postfix_op(BANG, OpSpec::Template(tq));
    b.postfix_op(K(182), OpSpec::Template(pi));
    b.binary_op(K(183), OpSpec::Template(ov));
    b.binary_op(K(184), OpSpec::Template(av));
    b.node(
        POSTFIX,
        Arch::Postfix {
            operand: Pick::Nth(0),
            op: Pick::Nth(1),
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
    b.node(K(170), Arch::ExprStmt { expr: Pick::Nth(0) });
    let mut w = W::new();
    w.open(FILE).open(FN).t(KW, "fn").t(IDENT, "f").open(BLOCK);
    w.open(K(170))
        .open(K(180))
        .t(KW, "do")
        .open(BLOCK)
        .close()
        .t(KW, "while")
        .t(IDENT, "c")
        .close()
        .close();
    w.open(K(170))
        .open(K(181))
        .t(KW, "for")
        .open(LET)
        .t(KW, "let")
        .t(IDENT, "i")
        .t(EQ, "=")
        .t(NUM, "0")
        .close();
    w.open(BINARY)
        .t(IDENT, "i")
        .t(EQ, "=")
        .open(BINARY)
        .t(IDENT, "i")
        .t(PLUS, "+")
        .t(NUM, "1")
        .close()
        .close();
    w.open(BLOCK).close().close().close();
    w.open(K(170))
        .open(BINARY)
        .t(IDENT, "x")
        .t(QUESTION, "??")
        .t(IDENT, "y")
        .close()
        .close();
    w.open(K(170))
        .open(POSTFIX)
        .t(IDENT, "e")
        .t(BANG, "?")
        .close()
        .close();
    w.open(K(170))
        .open(POSTFIX)
        .t(IDENT, "n")
        .t(K(182), "++")
        .close()
        .close();
    w.open(K(170))
        .open(BINARY)
        .t(IDENT, "a")
        .t(K(183), "or")
        .t(IDENT, "b")
        .close()
        .close();
    w.open(K(170))
        .open(BINARY)
        .t(IDENT, "a")
        .t(K(184), "and")
        .t(IDENT, "b")
        .close()
        .close();
    w.close().close().close();
    let (hir, mut names) = clean(b, w);
    let es = exprs(&hir);
    assert!(
        es.iter()
            .any(|e| matches!(e, Expr::Loop { step: Some(_), .. })),
        "do-while and for have steps"
    );
    assert!(
        es.iter().any(|e| matches!(e, Expr::Lit(Lit::Bool(true)))),
        "the absent `for` condition is `true`"
    );
    assert!(
        es.iter().any(|e| matches!(e, Expr::Return(Some(_)))),
        "`?` returns the error"
    );
    // `n++` reads and writes `n`: the place hole was used twice (copied).
    let n = names.intern("n");
    let n_uses = (0..hir.count(IdKind::Path))
        .map(|i| *hir.path(hir_lang::PathId::from_index(i).unwrap()))
        .filter(|p| {
            hir.list(p.segments)
                .first()
                .is_some_and(|s| s.name.sym == n)
        })
        .count();
    assert_eq!(n_uses, 2);
    // Template temporaries are bound and used in-template only, never by mark-less names.
    for i in 0..hir.count(IdKind::Binder) {
        let binder = hir
            .binder(hir_lang::BinderId::from_index(i).unwrap())
            .unwrap();
        if !binder.name.mark.is_root() {
            assert!(hir.expansion(binder.name.mark).is_some());
            assert_eq!(binder.kind, BinderKind::Local);
        }
    }
    let _ = TLit::Null;
    let _ = BindMode::Value;
}

#[test]
fn test_a_child_picked_twice_is_reported_not_lowered_twice() {
    // `if BLOCK` with the condition missing: a positional `cond` pick lands on
    // the block that `then` also picks. The first field keeps it; `then` is
    // reported missing, and the conflict is reported.
    let mut b = base();
    b.node(K(170), Arch::ExprStmt { expr: Pick::Nth(0) });
    b.node(
        K(177),
        Arch::If {
            cond: Pick::Nth(1),
            then: Pick::Kind(BLOCK),
            else_: Pick::Absent,
        },
    );
    let mut w = W::new();
    w.open(FILE)
        .open(K(170))
        .open(K(177))
        .t(KW, "if")
        .open(BLOCK)
        .close()
        .close()
        .close()
        .close();
    let (out, _, _) = run(b, w);
    assert_eq!(
        problem_kinds(&out),
        // In source order: the `if` (missing `then`), then the block.
        [
            ProblemKind::MissingField { field: "then" },
            ProblemKind::AmbiguousChild
        ]
    );
    let blocks = exprs(&out.hir)
        .iter()
        .filter(|e| matches!(e, Expr::Block(_)))
        .count();
    assert_eq!(blocks, 2, "the module body and the block, lowered once");
}

#[test]
fn test_a_broken_class_member_is_one_problem() {
    // A class whose body has an unmapped member and a `let` (not a member).
    let mut b = base();
    b.node(
        K(178),
        Arch::Class {
            name: Pick::Nth(1),
            generics: Pick::Absent,
            bases: Pick::Absent,
            interfaces: Pick::Absent,
            members: Pick::Nodes,
            mixin: false,
        },
    );
    let mut w = W::new();
    w.open(FILE).open(K(178)).t(KW, "class").t(IDENT, "C");
    w.open(K(199)).t(IDENT, "junk").close();
    w.open(LET).t(KW, "let").t(IDENT, "x").close();
    w.close().close();
    let (out, _, _) = run(b, w);
    assert_eq!(
        problem_kinds(&out),
        [
            ProblemKind::Unmapped,
            ProblemKind::NotAllowedHere {
                expected: lower_lang::Context::Member
            }
        ]
    );
}
