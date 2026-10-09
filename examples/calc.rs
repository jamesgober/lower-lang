//! The Tier-1 path end to end: forge a small language with lang-forge,
//! describe its semantics as a lower-lang mapping, lower a program (and a
//! half-typed one) into HIR, and print the result with origins.
//!
//! ```sh
//! cargo run --example calc
//! ```

#![allow(clippy::print_stdout, clippy::expect_used)]

use hir_lang::{OpKind, PrintOptions};
use intern_lang::Interner;
use lang_forge::Language;
use lower_lang::{Arch, MappingBuilder, OpSpec, Pick, TokenRole, templates};

const CALC: &str = r##"
[language]
name = "calc"

[rules]
program     = "_item*"
_item       = "fn_decl | let_stmt | while_stmt | return_stmt | expr_stmt"
fn_decl     = "'fn' IDENT '(' params? ')' block"
params      = "IDENT (',' IDENT)*"
block       = "'{' _item* '}'"
let_stmt    = "'let' IDENT '=' expr ';'"
while_stmt  = "'while' expr block"
return_stmt = "'return' expr? ';'"
expr_stmt   = "expr ';'"
group       = "'(' expr ')'"
args        = "expr (',' expr)*"

[rules.expr]
operand = "NUMBER | IDENT | group"
levels  = [
    { right   = ["="], node = "assign" },
    { left    = ["&&"] },
    { none    = ["<", ">"] },
    { left    = ["+", "-"] },
    { left    = ["*"] },
    { postfix = ["("], then = "args? ')'", node = "call" },
]
"##;

fn main() {
    let lang = Language::from_lsf(CALC).expect("calc forges");
    let k = |name: &str| lang.kind(name).expect("a calc kind");

    // The semantics, as data: which node kinds mean which archetypes, what
    // tokens are, and what operators lower to.
    let mut b = MappingBuilder::new();
    let while_loop = b.template(templates::while_loop()).expect("registers");
    let and = b.template(templates::logical_and()).expect("registers");
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
    // `while` is sugar: a template over HIR's one loop.
    b.template_node(
        k("while_stmt"),
        while_loop,
        &[Pick::Kind(k("expr")), Pick::Kind(k("block")), Pick::Absent],
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
    b.node(
        k("call"),
        Arch::Call {
            callee: Pick::Nth(0),
            args: Pick::Kind(k("args")),
        },
    );
    b.container(k("params"))
        .container(k("args"))
        .skip(k(","))
        .error(k("ERROR"));
    b.token(k("IDENT"), TokenRole::Name)
        .token(k("NUMBER"), TokenRole::Number);
    b.binary_op(k("+"), OpSpec::Op(OpKind::Add))
        .binary_op(k("-"), OpSpec::Op(OpKind::Sub))
        .binary_op(k("*"), OpSpec::Op(OpKind::Mul))
        .binary_op(k("<"), OpSpec::Op(OpKind::Lt))
        .binary_op(k(">"), OpSpec::Op(OpKind::Gt))
        .binary_op(k("="), OpSpec::Assign)
        .binary_op(k("&&"), OpSpec::Template(and));
    let mapping = b.build().expect("valid mapping");

    for src in [
        "fn pow(b, e) {\n  let r = 1;\n  while e > 0 && r < 1000 { r = r * b; e = e - 1; }\n  return r;\n}\nprint(pow(2, 8));\n",
        // Half-typed: a missing operand and an unfinished `while`.
        "let x = 1 + ;\nwhile { x = 2; }\nprint(x);\n",
    ] {
        let parse = lang.parse(src);
        let mut names = Interner::new();
        let out =
            lower_lang::lower(parse.tree(), src, &mapping, &mut names).expect("within budget");
        println!("--- {} problem(s)", out.problems.len());
        for p in &out.problems {
            println!("{p}");
        }
        println!(
            "{}\n",
            hir_lang::print_with(&out.hir, &names, PrintOptions::default().with_origins(true))
        );
    }
}
