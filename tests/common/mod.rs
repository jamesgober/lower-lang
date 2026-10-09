//! Shared fixtures: the calc language (forged by lang-forge from an `.lsf`
//! schematic, lowered through a lower-lang mapping) and HIR checks.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use hir_lang::{BinderId, Expr, Hir, IdKind, Ns, PathId, Res};
use intern_lang::Interner;
use lang_forge::{Kind, Language};
use lower_lang::{Arch, Lowered, Mapping, MappingBuilder, OpSpec, Pick, TokenRole, templates};

/// A small C-flavoured language: functions, `let`, assignment, `while`
/// (through the `while` template), `if`/`else`, `return`, calls, and the
/// usual operators, `&&`/`||` through their templates.
pub const CALC_LSF: &str = r##"
[language]
name = "calc"

[lexer]
line_comments = ["#"]

[rules]
program     = "_item*"
_item       = "fn_decl | let_stmt | while_stmt | if_stmt | return_stmt | expr_stmt"
fn_decl     = "'fn' IDENT '(' params? ')' block"
params      = "IDENT (',' IDENT)*"
block       = "'{' _item* '}'"
let_stmt    = "'let' IDENT '=' expr ';'"
while_stmt  = "'while' expr block"
if_stmt     = "'if' expr block ('else' (if_stmt | block))?"
return_stmt = "'return' expr? ';'"
expr_stmt   = "expr ';'"
group       = "'(' expr ')'"
args        = "expr (',' expr)*"

[rules.expr]
operand = "NUMBER | IDENT | group"
levels  = [
    { right   = ["="], node = "assign" },
    { left    = ["||"] },
    { left    = ["&&"] },
    { none    = ["==", "!=", "<", "<=", ">", ">="] },
    { left    = ["+", "-"] },
    { left    = ["*", "/", "%"] },
    { prefix  = ["-", "!"] },
    { postfix = ["("], then = "args? ')'", node = "call" },
]
"##;

pub fn calc_language() -> Language {
    Language::from_lsf(CALC_LSF).expect("calc.lsf forges")
}

/// calc's semantics, as a lower-lang mapping over lang-forge's kinds.
pub fn calc_mapping(lang: &Language) -> Mapping<Kind> {
    let k = |name: &str| {
        lang.kind(name)
            .unwrap_or_else(|| panic!("calc has `{name}`"))
    };
    let mut b = MappingBuilder::new();
    let while_loop = b.template(templates::while_loop()).expect("registers");
    let and = b.template(templates::logical_and()).expect("registers");
    let or = b.template(templates::logical_or()).expect("registers");

    b.node(
        k("program"),
        Arch::Module {
            name: Pick::Absent,
            items: Pick::Nodes,
        },
    );
    b.node(
        k("fn_decl"),
        Arch::Fn {
            name: Pick::Kind(k("IDENT")),
            generics: Pick::Absent,
            params: Pick::Kind(k("params")),
            ret: Pick::Absent,
            throws: Pick::Absent,
            body: Pick::Kind(k("block")),
            where_: Pick::Absent,
        },
    );
    b.container(k("params"));
    b.node(
        k("block"),
        Arch::Block {
            stmts: Pick::Nodes,
            tail: Pick::Absent,
            label: Pick::Absent,
            tail_from_last: false,
        },
    );
    b.node(
        k("let_stmt"),
        Arch::Let {
            pat: Pick::Kind(k("IDENT")),
            ty: Pick::Absent,
            init: Pick::After(k("=")),
            else_: Pick::Absent,
            mutable: true,
            hoist: false,
        },
    );
    b.template_node(
        k("while_stmt"),
        while_loop,
        &[Pick::Kind(k("expr")), Pick::Kind(k("block")), Pick::Absent],
    );
    b.node(
        k("if_stmt"),
        Arch::If {
            cond: Pick::Kind(k("expr")),
            then: Pick::Kind(k("block")),
            else_: Pick::After(k("else")),
        },
    );
    b.node(
        k("return_stmt"),
        Arch::Return {
            value: Pick::Kind(k("expr")),
        },
    );
    b.node(
        k("expr_stmt"),
        Arch::ExprStmt {
            expr: Pick::Kind(k("expr")),
        },
    );
    b.node(k("expr"), Arch::pass(0));
    b.node(
        k("group"),
        Arch::Pass {
            inner: Pick::Kind(k("expr")),
        },
    );
    b.node(k("binary"), Arch::binary());
    b.node(k("assign"), Arch::binary());
    b.node(k("prefix"), Arch::prefix());
    b.node(
        k("call"),
        Arch::Call {
            callee: Pick::Nth(0),
            args: Pick::Kind(k("args")),
        },
    );
    b.container(k("args"));
    b.skip(k(","));
    b.error(k("ERROR"));
    b.token(k("IDENT"), TokenRole::Name);
    b.token(k("NUMBER"), TokenRole::Number);
    for (op, kind) in [
        ("+", hir_lang::OpKind::Add),
        ("-", hir_lang::OpKind::Sub),
        ("*", hir_lang::OpKind::Mul),
        ("/", hir_lang::OpKind::Div),
        ("%", hir_lang::OpKind::Rem),
        ("==", hir_lang::OpKind::Eq),
        ("!=", hir_lang::OpKind::Ne),
        ("<", hir_lang::OpKind::Lt),
        ("<=", hir_lang::OpKind::Le),
        (">", hir_lang::OpKind::Gt),
        (">=", hir_lang::OpKind::Ge),
    ] {
        b.binary_op(k(op), OpSpec::Op(kind));
    }
    b.binary_op(k("="), OpSpec::Assign);
    b.binary_op(k("&&"), OpSpec::Template(and));
    b.binary_op(k("||"), OpSpec::Template(or));
    b.prefix_op(k("-"), OpSpec::Op(hir_lang::OpKind::Neg));
    b.prefix_op(k("!"), OpSpec::Op(hir_lang::OpKind::Not));
    b.build().expect("calc mapping is valid")
}

/// Parses and lowers calc source.
pub fn lower_calc(
    lang: &Language,
    mapping: &Mapping<Kind>,
    src: &str,
    names: &mut Interner,
) -> Lowered {
    let parse = lang.parse(src);
    lower_lang::lower(parse.tree(), src, mapping, names).expect("within budget")
}

/// Every node's and binder's origin lies within `0..=len`, and every
/// expansion's call site too.
pub fn origins_within(hir: &Hir, len: usize) -> bool {
    let mut ok = true;
    hir_lang::walk(hir, |n| {
        let o = hir.origin(n);
        ok &= o.span.end().to_usize() <= len;
        ok &= o.expn.is_root() || hir.expansion(o.expn).is_some();
    });
    for i in 0..hir.count(IdKind::Binder) {
        let b = BinderId::from_index(i).expect("in range");
        ok &= hir.binder_origin(b).span.end().to_usize() <= len;
    }
    ok
}

/// A toy resolver: every unresolved one-segment value path whose name
/// `lookup_local` finds is resolved to that binder. Returns how many paths
/// resolved and how many stayed unresolved.
pub fn resolve_locals(hir: &mut Hir) -> (usize, usize) {
    let mut resolved = 0;
    let mut open = 0;
    for i in 0..hir.count(IdKind::Path) {
        let path = PathId::from_index(i).expect("in range");
        let p = *hir.path(path);
        if !p.res.is_unresolved() || p.ns != Ns::Value || p.segments.len() != 1 {
            continue;
        }
        let Some(seg) = hir.list(p.segments).first().copied() else {
            continue;
        };
        match hir.lookup_local(path, seg.name) {
            Some(b) if hir.resolve(path, Res::Local(b)).is_ok() => resolved += 1,
            _ => open += 1,
        }
    }
    (resolved, open)
}

/// The expression nodes of `hir` that are `Expr::Err`.
pub fn error_exprs(hir: &Hir) -> usize {
    let mut n = 0;
    hir_lang::walk(hir, |node| {
        if let hir_lang::NodeRef::Expr(e) = node {
            if matches!(hir.expr(e), Expr::Err) {
                n += 1;
            }
        }
    });
    n
}
