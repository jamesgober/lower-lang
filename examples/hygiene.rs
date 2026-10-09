//! Hygiene: a Mox-style `foreach` lowered through the built-in `for_each`
//! template, in code that also has its own variable `$it`. The template's
//! iterator temporary is also called `it`, but it carries the expansion's
//! mark and is resolved by construction, so the user's `$it` and the
//! template's `it` can never be confused. The tree is built directly with
//! syntax-lang's `Builder`.
//!
//! ```sh
//! cargo run --example hygiene
//! ```

#![allow(clippy::print_stdout, clippy::expect_used)]

use hir_lang::{IdKind, Ns, PathId, Res};
use intern_lang::Interner;
use lower_lang::{Arch, MappingBuilder, Pick, TokenRole, templates};
use syntax_lang::{Builder, Span, Token, TokenKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum K {
    File,
    Assign,
    Foreach,
    Block,
    Echo,
    Var,
    Num,
    Eq,
    ForeachKw,
    As,
    FatArrow,
    LParen,
    RParen,
    EchoKw,
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
}

fn main() {
    // $it = 10;  foreach ($items as $k => $v) { echo $it; }
    let mut w = Writer {
        b: Builder::new(),
        src: String::new(),
    };
    w.open(K::File);
    w.open(K::Assign)
        .t(K::Var, "$it")
        .t(K::Eq, "=")
        .t(K::Num, "10")
        .close();
    w.open(K::Foreach)
        .t(K::ForeachKw, "foreach")
        .t(K::LParen, "(")
        .t(K::Var, "$items")
        .t(K::As, "as");
    w.t(K::Var, "$k")
        .t(K::FatArrow, "=>")
        .t(K::Var, "$v")
        .t(K::RParen, ")");
    w.open(K::Block)
        .open(K::Echo)
        .t(K::EchoKw, "echo")
        .t(K::Var, "$it")
        .close()
        .close();
    w.close().close();
    let tree = w.b.finish().expect("balanced");
    let src = w.src;

    let mut b = MappingBuilder::new();
    // PHP semantics: assigning `$it` declares it for the enclosing body.
    let mut policy = lower_lang::LangPolicy::default();
    policy.implicit_locals = true;
    b.policy(policy);
    let foreach = b
        .template(templates::for_each("mox.iter", "mox.next"))
        .expect("registers");
    b.node(
        K::File,
        Arch::Module {
            name: Pick::Absent,
            items: Pick::Nodes,
        },
    );
    b.node(K::Assign, Arch::binary());
    b.binary_op(K::Eq, lower_lang::OpSpec::Assign);
    // foreach ( SOURCE as KEY => VALUE ) BODY — holes: source, key, value, body, label.
    b.template_node(
        K::Foreach,
        foreach,
        &[
            Pick::KindNth(K::Var, 0),
            Pick::Before(K::FatArrow),
            Pick::Before(K::RParen),
            Pick::Kind(K::Block),
            Pick::Absent,
        ],
    );
    b.node(
        K::Block,
        Arch::Block {
            stmts: Pick::Nodes,
            tail: Pick::Absent,
            label: Pick::Absent,
            tail_from_last: false,
        },
    );
    b.node(
        K::Echo,
        Arch::Call {
            callee: Pick::Nth(0),
            args: Pick::AllAfter(K::EchoKw),
        },
    );
    b.token(K::Var, TokenRole::Var { sigil: 1 })
        .token(K::Num, TokenRole::Int)
        .token(K::EchoKw, TokenRole::Name);
    let mapping = b.build().expect("valid mapping");

    let mut names = Interner::new();
    let out = lower_lang::lower(&tree, &src, &mapping, &mut names).expect("within budget");
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let mut hir = out.hir;

    // A toy resolver: the innermost binder with the same name *and mark*.
    for i in 0..hir.count(IdKind::Path) {
        let path = PathId::from_index(i).expect("in range");
        let p = *hir.path(path);
        if !p.res.is_unresolved() || p.ns != Ns::Value {
            continue;
        }
        let Some(seg) = hir.list(p.segments).first().copied() else {
            continue;
        };
        if let Some(b) = hir.lookup_local(path, seg.name) {
            hir.resolve(path, Res::Local(b)).expect("in scope");
        }
    }
    let printed = hir_lang::print(&hir, &names);
    println!("{src}\n");
    println!("{printed}");
    // Both `$it` references resolve to the user's (hoisted) binder, the
    // template's `it'e1` to the template's own.
    assert_eq!(printed.matches("(use (path it → local it%3))").count(), 2);
    assert!(printed.contains("(use (path it'e1 → local it%2'e1))"));
    println!("\nthe user's $it resolves to the user's binder; the template's it'e1 to its own");
}
