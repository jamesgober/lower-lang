//! A Mox-flavoured language (PHP-like: `$variables`, `foreach`, by-reference
//! parameters, implicit locals, `use` captures, `->` members, classes with
//! mixins, try/catch) whose trees are built directly with syntax-lang's
//! `Builder`, with a mapping that exercises the dynamic-language archetypes.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use hir_lang::{OpKind, Overflow};
use lower_lang::{
    Arch, EffectMode, Escapes, LangPolicy, Mapping, MappingBuilder, Modifier, OpSpec, Pick,
    PlaceArgs, TokenRole, templates,
};
use syntax_lang::{Builder, Node, Span, Token, TokenKind};

/// Mox's kinds: nodes first, then tokens.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum M {
    // nodes
    File,
    FnDecl,
    Params,
    Param,
    Block,
    ExprStmt,
    Assign,
    Binary,
    Call,
    Args,
    Member,
    DynMember,
    Index,
    Append,
    VarVar,
    Closure,
    Uses,
    Capture,
    Global,
    Static,
    Return,
    Foreach,
    Echo,
    Class,
    Prop,
    MixinUse,
    Try,
    Catch,
    Finally,
    Throw,
    Yield,
    Array,
    Pair,
    If,
    While,
    Paren,
    Error,
    // tokens
    Var,
    Ident,
    Num,
    Str,
    SqStr,
    True,
    Null,
    Dollar,
    Function,
    ForeachKw,
    As,
    Arrow,
    FatArrow,
    Amp,
    GlobalKw,
    StaticKw,
    ReturnKw,
    ClassKw,
    ExtendsKw,
    UseKw,
    TryKw,
    CatchKw,
    FinallyKw,
    ThrowKw,
    YieldKw,
    EchoKw,
    IfKw,
    WhileKw,
    Public,
    Private,
    Eq,
    Plus,
    Dot,
    Lt,
    AndAnd,
    Pipe,
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Semi,
    Space,
}

impl TokenKind for M {
    fn is_trivia(&self) -> bool {
        matches!(self, M::Space)
    }
}

/// Writes a tree and its source together: every token's text is appended to
/// the source (separated by a space token), so spans are exact.
pub struct W {
    b: Builder<M>,
    pub src: String,
}

impl W {
    pub fn new() -> Self {
        Self {
            b: Builder::new(),
            src: String::new(),
        }
    }

    pub fn open(&mut self, k: M) -> &mut Self {
        self.b.start_node(k);
        self
    }

    pub fn close(&mut self) -> &mut Self {
        self.b.finish_node();
        self
    }

    pub fn t(&mut self, k: M, text: &str) -> &mut Self {
        if !self.src.is_empty() {
            let at = self.src.len() as u32;
            self.src.push(' ');
            self.b.token(Token::new(M::Space, Span::new(at, at + 1)));
        }
        let start = self.src.len() as u32;
        self.src.push_str(text);
        self.b
            .token(Token::new(k, Span::new(start, self.src.len() as u32)));
        self
    }

    pub fn var(&mut self, name: &str) -> &mut Self {
        self.t(M::Var, &format!("${name}"))
    }

    pub fn finish(self) -> (Node<M>, String) {
        (self.b.finish().expect("balanced"), self.src)
    }
}

/// Mox's semantics.
pub fn mox_mapping() -> Mapping<M> {
    let mut b = MappingBuilder::new();
    let foreach = b
        .template(templates::for_each("mox.iter", "mox.next"))
        .expect("registers");
    let while_loop = b.template(templates::while_loop()).expect("registers");
    let and = b.template(templates::logical_and()).expect("registers");
    let echo = {
        let mut t = lower_lang::TemplateBuilder::new("echo");
        let args = t.args_hole("args");
        let f = t.extern_("mox.echo");
        let a = t.splice(args);
        let root = t.call(f, &[a]);
        b.template(t.finish(root).expect("valid"))
            .expect("registers")
    };
    let concat = b.name("mox.concat");

    let mut policy = LangPolicy::default();
    policy.overflow = Overflow::Promote;
    policy.throws = EffectMode::Always;
    policy.yields = EffectMode::Inferred;
    policy.implicit_locals = true;
    policy.place_args = PlaceArgs::Places;
    b.policy(policy);

    b.node(
        M::File,
        Arch::Module {
            name: Pick::Absent,
            items: Pick::Nodes,
        },
    );
    b.node(
        M::FnDecl,
        Arch::Fn {
            name: Pick::Kind(M::Ident),
            generics: Pick::Absent,
            params: Pick::Kind(M::Params),
            ret: Pick::Absent,
            throws: Pick::Absent,
            body: Pick::Kind(M::Block),
            where_: Pick::Absent,
        },
    );
    b.container(M::Params);
    b.node(
        M::Param,
        Arch::Param {
            pat: Pick::Kind(M::Var),
            ty: Pick::Absent,
            default: Pick::After(M::Eq),
        },
    );
    b.node(
        M::Block,
        Arch::Block {
            stmts: Pick::Nodes,
            tail: Pick::Absent,
            label: Pick::Absent,
            tail_from_last: false,
        },
    );
    b.node(M::ExprStmt, Arch::ExprStmt { expr: Pick::Nth(0) });
    b.node(M::Assign, Arch::binary());
    b.node(M::Binary, Arch::binary());
    b.node(M::Paren, Arch::pass(1));
    b.node(
        M::Call,
        Arch::Call {
            callee: Pick::Nth(0),
            args: Pick::Kind(M::Args),
        },
    );
    b.container(M::Args);
    b.node(
        M::Member,
        Arch::Member {
            base: Pick::Nth(0),
            member: Pick::Nth(2),
            args: Pick::Kind(M::Args),
            call: Pick::Kind(M::Args),
        },
    );
    b.node(
        M::DynMember,
        Arch::DynMember {
            base: Pick::Nth(0),
            name: Pick::Nth(2),
            args: Pick::Kind(M::Args),
            call: Pick::Kind(M::Args),
        },
    );
    b.node(
        M::Index,
        Arch::Index {
            base: Pick::Nth(0),
            index: Pick::Nth(2),
        },
    );
    b.node(M::Append, Arch::Append { base: Pick::Nth(0) });
    b.node(
        M::VarVar,
        Arch::VarVar {
            inner: Pick::Back(0),
        },
    );
    b.node(
        M::Closure,
        Arch::Closure {
            params: Pick::Kind(M::Params),
            ret: Pick::Absent,
            captures: Pick::Kind(M::Uses),
            body: Pick::Kind(M::Block),
            implicit: None,
        },
    );
    b.container(M::Uses);
    b.node(
        M::Capture,
        Arch::Capture {
            name: Pick::Kind(M::Var),
        },
    );
    b.node(
        M::Global,
        Arch::GlobalDecl {
            names: Pick::AllKind(M::Var),
        },
    );
    b.node(
        M::Static,
        Arch::Static {
            name: Pick::Kind(M::Var),
            ty: Pick::Absent,
            init: Pick::After(M::Eq),
        },
    );
    b.node(
        M::Return,
        Arch::Return {
            value: Pick::AllAfter(M::ReturnKw),
        },
    );
    b.node(
        M::Throw,
        Arch::Throw {
            value: Pick::AllAfter(M::ThrowKw),
        },
    );
    b.node(
        M::Yield,
        Arch::Yield {
            value: Pick::AllAfter(M::YieldKw),
        },
    );
    // foreach ( SOURCE as [KEY =>] VALUE ) BLOCK — holes: source, key, value, body, label.
    b.template_node(
        M::Foreach,
        foreach,
        &[
            Pick::KindNth(M::Var, 0),
            Pick::Before(M::FatArrow),
            Pick::Before(M::RParen),
            Pick::Kind(M::Block),
            Pick::Absent,
        ],
    );
    b.template_node(M::Echo, echo, &[Pick::AllAfter(M::EchoKw)]);
    b.template_node(
        M::While,
        while_loop,
        &[Pick::NthNode(0), Pick::Kind(M::Block), Pick::Absent],
    );
    b.node(
        M::If,
        Arch::If {
            cond: Pick::NthNode(0),
            then: Pick::NthNode(1),
            else_: Pick::NthNode(2),
        },
    );
    b.node(
        M::Class,
        Arch::Class {
            name: Pick::KindNth(M::Ident, 0),
            generics: Pick::Absent,
            bases: Pick::After(M::ExtendsKw),
            interfaces: Pick::Absent,
            members: Pick::Nodes,
            mixin: false,
        },
    );
    b.node(
        M::Prop,
        Arch::FieldDecl {
            name: Pick::Kind(M::Var),
            ty: Pick::Absent,
            default: Pick::After(M::Eq),
        },
    );
    b.node(
        M::MixinUse,
        Arch::MixinUse {
            mixins: Pick::AllKind(M::Ident),
        },
    );
    b.node(
        M::Try,
        Arch::Try {
            body: Pick::Kind(M::Block),
            catches: Pick::AllKind(M::Catch),
            finally: Pick::Kind(M::Finally),
        },
    );
    b.node(
        M::Catch,
        Arch::Catch {
            types: Pick::AllKind(M::Ident),
            name: Pick::Kind(M::Var),
            body: Pick::Kind(M::Block),
        },
    );
    b.node(
        M::Finally,
        Arch::Pass {
            inner: Pick::Kind(M::Block),
        },
    );
    b.node(
        M::Array,
        Arch::Map {
            entries: Pick::Range(1, 1),
        },
    );
    b.node(
        M::Pair,
        Arch::MapEntry {
            key: Pick::Nth(0),
            value: Pick::Nth(2),
        },
    );
    b.error(M::Error);
    b.skip(M::Comma);
    b.skip(M::Semi);

    b.token(M::Var, TokenRole::Var { sigil: 1 });
    b.token(M::Ident, TokenRole::Name);
    b.token(M::Num, TokenRole::Number);
    b.token(M::Str, TokenRole::Str(Escapes::Standard));
    b.token(M::SqStr, TokenRole::Str(Escapes::Quotes));
    b.token(M::True, TokenRole::True);
    b.token(M::Null, TokenRole::Null);

    b.modifier(M::Amp, Modifier::Ref);
    b.modifier(M::Public, Modifier::Public);
    b.modifier(M::Private, Modifier::Private);
    b.modifier(M::StaticKw, Modifier::Static);

    b.binary_op(M::Eq, OpSpec::Assign);
    b.binary_op(M::Plus, OpSpec::Op(OpKind::Add));
    b.binary_op(M::Lt, OpSpec::Op(OpKind::Lt));
    b.binary_op(M::Dot, OpSpec::Extern(concat));
    b.binary_op(M::AndAnd, OpSpec::Template(and));
    b.build().expect("mox mapping is valid")
}
