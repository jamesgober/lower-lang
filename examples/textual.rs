//! Textual templates: Mox's sugar written as template text (HIR spec §20,
//! LSF2 `[semantics.templates]`), parsed once into templates, and used for
//! `&&`, `++`, `--`, `echo`, and `.=` while lowering a small Mox program.
//!
//! The `++`/`--` templates are the `let_place` forms the Mox sketch should
//! use (GAPS B2): the place is named once, so `++$a[f()]` calls `f` once.
//! `.=` needs no template at all: `OpSpec::CompoundIntrinsic` lowers it to
//! `let_place p = $a[$k] in p = mox.concat(p, "!")`.
//!
//! ```sh
//! cargo run --example textual
//! ```

#![allow(clippy::print_stdout, clippy::expect_used)]

use intern_lang::Interner;
use lower_lang::{Arch, LangPolicy, MappingBuilder, OpSpec, Pick, Template, TokenRole, templates};
use syntax_lang::{Builder, Span, Token, TokenKind};

/// The sketch's `[semantics.templates]` for the sugar below, with the
/// increment and decrement templates rewritten to evaluate their place once.
const TEMPLATES: [(&str, &str); 6] = [
    ("bool_and", "if ${lhs} { not(not(${rhs})) } else { false }"),
    ("pre_increment", "let_place p = ${place} in p = add(p, 1)"),
    ("pre_decrement", "let_place p = ${place} in p = sub(p, 1)"),
    (
        "post_increment",
        "let_place p = ${place} in { let old = p; p = add(old, 1); old }",
    ),
    (
        "post_decrement",
        "let_place p = ${place} in { let old = p; p = sub(old, 1); old }",
    ),
    ("echo", "extern:mox.echo(${...values})"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum K {
    File,
    Stmt,
    Assign,
    Binary,
    Prefix,
    Postfix,
    Index,
    Echo,
    Var,
    Str,
    Eq,
    DotEq,
    AndAnd,
    PlusPlus,
    MinusMinus,
    LBracket,
    RBracket,
    EchoKw,
    Comma,
    Semi,
    Space,
}

impl TokenKind for K {
    fn is_trivia(&self) -> bool {
        matches!(self, K::Space)
    }
}

/// Appends tokens to the source and the tree together, so spans are exact.
struct Writer {
    b: Builder<K>,
    src: String,
}

impl Writer {
    fn t(&mut self, k: K, text: &str) -> &mut Self {
        if !self.src.is_empty() {
            let at = self.src.len() as u32;
            self.src.push(' ');
            self.b.token(Token::new(K::Space, Span::new(at, at + 1)));
        }
        let at = self.src.len() as u32;
        self.src.push_str(text);
        self.b
            .token(Token::new(k, Span::new(at, self.src.len() as u32)));
        self
    }

    fn open(&mut self, k: K) -> &mut Self {
        self.b.start_node(k);
        self
    }

    fn close(&mut self) -> &mut Self {
        self.b.finish_node();
        self
    }

    /// `<body>;` as a statement.
    fn stmt(&mut self, body: impl FnOnce(&mut Self)) {
        self.open(K::Stmt);
        body(self);
        self.t(K::Semi, ";").close();
    }
}

fn main() {
    // One template on its own: the lazy path.
    let and = Template::parse("bool_and", TEMPLATES[0].1).expect("valid template text");
    println!("`bool_and` has {} holes", and.hole_count());

    // The mapping, with the template texts registered in sketch order.
    let mut b = MappingBuilder::new();
    b.policy(LangPolicy::php());
    let mut id = |name: &str| {
        let text = TEMPLATES
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, t)| *t)
            .expect("listed");
        b.parse_template(name, text).expect("valid template text")
    };
    let (bool_and, pre_inc, pre_dec, post_inc, post_dec, echo) = (
        id("bool_and"),
        id("pre_increment"),
        id("pre_decrement"),
        id("post_increment"),
        id("post_decrement"),
        id("echo"),
    );
    // The built-in `post++` would do as well: it also uses `let_place`.
    let _builtin = templates::post_increment();
    let concat = b.name("mox.concat");
    b.node(
        K::File,
        Arch::Module {
            name: Pick::Absent,
            items: Pick::Nodes,
        },
    );
    b.node(K::Stmt, Arch::ExprStmt { expr: Pick::Nth(0) });
    b.node(K::Assign, Arch::binary());
    b.node(K::Binary, Arch::binary());
    b.node(K::Prefix, Arch::prefix());
    b.node(
        K::Postfix,
        Arch::Postfix {
            operand: Pick::Nth(0),
            op: Pick::Nth(1),
        },
    );
    b.node(
        K::Index,
        Arch::Index {
            base: Pick::Nth(0),
            index: Pick::Nth(2),
        },
    );
    b.template_node(K::Echo, echo, &[Pick::AllAfter(K::EchoKw)]);
    b.token(K::Var, TokenRole::Var { sigil: 1 })
        .token(K::Str, TokenRole::Str(lower_lang::Escapes::Standard));
    b.skip(K::Comma).skip(K::Semi).skip(K::RBracket);
    b.binary_op(K::Eq, OpSpec::Assign)
        .binary_op(K::DotEq, OpSpec::CompoundIntrinsic(concat))
        .binary_op(K::AndAnd, OpSpec::Template(bool_and))
        .prefix_op(K::PlusPlus, OpSpec::Template(pre_inc))
        .prefix_op(K::MinusMinus, OpSpec::Template(pre_dec))
        .postfix_op(K::PlusPlus, OpSpec::Template(post_inc))
        .postfix_op(K::MinusMinus, OpSpec::Template(post_dec));
    let mapping = b.build().expect("valid mapping");

    // $i = $a && $b; ++$i; $i--; $a[$k] .= "!"; echo $i, $a;
    let mut w = Writer {
        b: Builder::new(),
        src: String::new(),
    };
    w.open(K::File);
    w.stmt(|w| {
        w.open(K::Assign).t(K::Var, "$i").t(K::Eq, "=");
        w.open(K::Binary)
            .t(K::Var, "$a")
            .t(K::AndAnd, "&&")
            .t(K::Var, "$b")
            .close()
            .close();
    });
    w.stmt(|w| {
        w.open(K::Prefix)
            .t(K::PlusPlus, "++")
            .t(K::Var, "$i")
            .close();
    });
    w.stmt(|w| {
        w.open(K::Postfix)
            .t(K::Var, "$i")
            .t(K::MinusMinus, "--")
            .close();
    });
    w.stmt(|w| {
        w.open(K::Assign).open(K::Index);
        w.t(K::Var, "$a")
            .t(K::LBracket, "[")
            .t(K::Var, "$k")
            .t(K::RBracket, "]")
            .close();
        w.t(K::DotEq, ".=").t(K::Str, "\"!\"").close();
    });
    w.open(K::Echo)
        .t(K::EchoKw, "echo")
        .t(K::Var, "$i")
        .t(K::Comma, ",")
        .t(K::Var, "$a")
        .t(K::Semi, ";")
        .close();
    w.close();
    let tree = w.b.finish().expect("balanced");

    let mut names = Interner::new();
    let out = lower_lang::lower(&tree, &w.src, &mapping, &mut names).expect("within budget");
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    println!("{}\n", w.src);
    println!("{}", hir_lang::print(&out.hir, &names));
}
