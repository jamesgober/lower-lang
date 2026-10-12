//! Mox with classes, numeric break levels, `...`, and the `include` family:
//! the parts of `_lexersketch/sketches/mox.lsf` that lower-lang 0.4.1 adds,
//! with trees built directly (the shapes lang-forge 2.0 gives Mox's rules)
//! and a mapping that sets exactly the keys the sketch sets:
//!
//! - `[semantics.policy] implicit_receiver = "this"`, `break_levels = "numeric"`,
//!   `default_visibility = "public"` (`LangPolicy::implicit_receiver`,
//!   `LangPolicy::break_levels`, `LangPolicy::default_member_visibility`);
//! - `[semantics.modifiers] "..." = { param = "rest", arg = "spread" }`
//!   (`MappingBuilder::modifier_in`), plus `map_entry = "spread"`;
//! - `include_expr = [{ when = "'include_once'", template = … }, …]`
//!   (`MappingBuilder::template_node_when`);
//! - `[semantics.type_names] array = "any"` for the keyword type names.
//!
//! The harness resolves the lowered HIR with resolve-lang 0.4 (PHP policy),
//! compiles it with bcgen-lang 0.4 (Mox intrinsics), and runs it on bvm-lang
//! 2.0.0-alpha.4, collecting what `mox.echo` prints.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};

use hir_lang::{CaptureMode, OpKind, PathRoot};
use intern_lang::Interner;
use lower_lang::{
    Arch, BreakLevels, Escapes, LangPolicy, Lowered, Mapping, MappingBuilder, Modifier, OpSpec,
    Pick, TokenRole, TypeName, templates,
};
use syntax_lang::{Builder, Node, Span, Token, TokenKind};

/// The kinds: nodes first, then tokens.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum X {
    // nodes
    File,
    ClassDecl,
    TraitDecl,
    InterfaceDecl,
    ImplBlock,
    Method,
    FnDecl,
    ParamList,
    Param,
    Property,
    Block,
    ExprStmt,
    Echo,
    Assign,
    Binary,
    Member,
    StaticMember,
    Call,
    ArgList,
    Arg,
    New,
    Return,
    Closure,
    ArrowFn,
    Uses,
    Capture,
    While,
    Loop,
    If,
    Break,
    Continue,
    Include,
    ArrayLit,
    ArrayItem,
    Paren,
    SpecialType,
    TypedParam,
    TraitUse,
    Let,
    // tokens
    Var,
    Ident,
    Num,
    Str,
    ClassKw,
    TraitKw,
    InterfaceKw,
    ImplKw,
    ExtendsKw,
    FunctionKw,
    FnKw,
    StaticKw,
    PublicKw,
    PrivateKw,
    NewKw,
    ReturnKw,
    EchoKw,
    WhileKw,
    LoopKw,
    IfKw,
    BreakKw,
    ContinueKw,
    UseKw,
    LetKw,
    IncludeKw,
    IncludeOnceKw,
    RequireKw,
    RequireOnceKw,
    ParentKw,
    SelfKw,
    ArrayKw,
    NullKw,
    CallableKw,
    SelfParam,
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Semi,
    Eq,
    Plus,
    Lt,
    Arrow,
    ColonColon,
    Dots,
    FatArrow,
    Space,
}

impl TokenKind for X {
    fn is_trivia(&self) -> bool {
        matches!(self, X::Space)
    }
}

/// Writes a tree and its source together: every token's text is appended to
/// the source (separated by a space token), so spans are exact.
pub struct W {
    b: Builder<X>,
    pub src: String,
}

impl Default for W {
    fn default() -> Self {
        Self::new()
    }
}

impl W {
    pub fn new() -> Self {
        Self {
            b: Builder::new(),
            src: String::new(),
        }
    }

    pub fn open(&mut self, k: X) -> &mut Self {
        self.b.start_node(k);
        self
    }

    pub fn close(&mut self) -> &mut Self {
        self.b.finish_node();
        self
    }

    pub fn t(&mut self, k: X, text: &str) -> &mut Self {
        if !self.src.is_empty() {
            let at = self.src.len() as u32;
            self.src.push(' ');
            self.b.token(Token::new(X::Space, Span::new(at, at + 1)));
        }
        let start = self.src.len() as u32;
        self.src.push_str(text);
        self.b
            .token(Token::new(k, Span::new(start, self.src.len() as u32)));
        self
    }

    pub fn var(&mut self, name: &str) -> &mut Self {
        self.t(X::Var, &format!("${name}"))
    }

    pub fn num(&mut self, n: i64) -> &mut Self {
        self.t(X::Num, &n.to_string())
    }

    pub fn finish(self) -> (Node<X>, String) {
        (self.b.finish().expect("balanced"), self.src)
    }

    // ------------------------------------------------------------ helpers

    /// `( params ) ` from `$names`; a name starting with `...` is a rest
    /// parameter.
    pub fn params(&mut self, names: &[&str]) -> &mut Self {
        self.t(X::LParen, "(").open(X::ParamList);
        for (i, n) in names.iter().enumerate() {
            if i > 0 {
                self.t(X::Comma, ",");
            }
            self.open(X::Param);
            match n.strip_prefix("...") {
                Some(rest) => {
                    self.t(X::Dots, "...").var(rest);
                }
                None => {
                    self.var(n);
                }
            }
            self.close();
        }
        self.close().t(X::RParen, ")")
    }

    /// `( args )`: each argument written by `f(i)`; `spread[i]` prefixes it
    /// with `...`.
    pub fn args(
        &mut self,
        n: usize,
        spread: &[bool],
        mut f: impl FnMut(&mut Self, usize),
    ) -> &mut Self {
        self.t(X::LParen, "(").open(X::ArgList);
        for i in 0..n {
            if i > 0 {
                self.t(X::Comma, ",");
            }
            self.open(X::Arg);
            if spread.get(i).copied().unwrap_or(false) {
                self.t(X::Dots, "...");
            }
            f(self, i);
            self.close();
        }
        self.close().t(X::RParen, ")")
    }

    /// `{ stmts }`.
    pub fn block(&mut self, f: impl FnOnce(&mut Self)) -> &mut Self {
        self.open(X::Block).t(X::LBrace, "{");
        f(self);
        self.t(X::RBrace, "}").close()
    }

    /// `expr ;`.
    pub fn stmt(&mut self, f: impl FnOnce(&mut Self)) -> &mut Self {
        self.open(X::ExprStmt);
        f(self);
        self.t(X::Semi, ";").close()
    }

    /// `echo expr ;`.
    pub fn echo(&mut self, f: impl FnOnce(&mut Self)) -> &mut Self {
        self.open(X::Echo).t(X::EchoKw, "echo");
        f(self);
        self.t(X::Semi, ";").close()
    }

    /// `return expr ;`.
    pub fn ret(&mut self, f: impl FnOnce(&mut Self)) -> &mut Self {
        self.open(X::Return).t(X::ReturnKw, "return");
        f(self);
        self.t(X::Semi, ";").close()
    }

    /// `lhs = rhs`.
    pub fn assign(
        &mut self,
        lhs: impl FnOnce(&mut Self),
        rhs: impl FnOnce(&mut Self),
    ) -> &mut Self {
        self.open(X::Assign);
        lhs(self);
        self.t(X::Eq, "=");
        rhs(self);
        self.close()
    }

    /// `lhs op rhs` for `+` or `<`.
    pub fn bin(
        &mut self,
        op: X,
        lhs: impl FnOnce(&mut Self),
        rhs: impl FnOnce(&mut Self),
    ) -> &mut Self {
        let text = match op {
            X::Plus => "+",
            X::Lt => "<",
            _ => "?",
        };
        self.open(X::Binary);
        lhs(self);
        self.t(op, text);
        rhs(self);
        self.close()
    }

    /// `$var->name`.
    pub fn prop(&mut self, var: &str, name: &str) -> &mut Self {
        self.open(X::Member)
            .var(var)
            .t(X::Arrow, "->")
            .t(X::Ident, name)
            .close()
    }

    /// `base->name(args)` with the base written by `base`.
    pub fn mcall(
        &mut self,
        base: impl FnOnce(&mut Self),
        name: &str,
        n: usize,
        f: impl FnMut(&mut Self, usize),
    ) -> &mut Self {
        self.open(X::Member);
        base(self);
        self.t(X::Arrow, "->").t(X::Ident, name);
        self.args(n, &[], f);
        self.close()
    }

    /// `new Class(args)`.
    pub fn new_obj(&mut self, class: &str, n: usize, f: impl FnMut(&mut Self, usize)) -> &mut Self {
        self.open(X::New).t(X::NewKw, "new").t(X::Ident, class);
        self.args(n, &[], f);
        self.close()
    }

    /// `callee(args)` with an identifier or variable callee.
    pub fn call(
        &mut self,
        callee: impl FnOnce(&mut Self),
        n: usize,
        spread: &[bool],
        f: impl FnMut(&mut Self, usize),
    ) -> &mut Self {
        self.open(X::Call);
        callee(self);
        self.args(n, spread, f);
        self.close()
    }

    /// `mods function name(params) { body }` (a method or a function).
    pub fn function(
        &mut self,
        kind: X,
        mods: &[X],
        name: &str,
        params: &[&str],
        body: impl FnOnce(&mut Self),
    ) -> &mut Self {
        self.open(kind);
        for &m in mods {
            let text = match m {
                X::StaticKw => "static",
                X::PublicKw => "public",
                X::PrivateKw => "private",
                _ => "?",
            };
            self.t(m, text);
        }
        self.t(X::FunctionKw, "function").t(X::Ident, name);
        self.params(params);
        self.block(body);
        self.close()
    }

    /// `public $name;` (with a default `= n` when given).
    pub fn property(&mut self, name: &str, default: Option<i64>) -> &mut Self {
        self.open(X::Property).t(X::PublicKw, "public").var(name);
        if let Some(n) = default {
            self.t(X::Eq, "=").num(n);
        }
        self.t(X::Semi, ";").close()
    }

    /// `class Name (extends Base)? { members }`.
    pub fn class(
        &mut self,
        name: &str,
        base: Option<&str>,
        members: impl FnOnce(&mut Self),
    ) -> &mut Self {
        self.open(X::ClassDecl)
            .t(X::ClassKw, "class")
            .t(X::Ident, name);
        if let Some(b) = base {
            self.t(X::ExtendsKw, "extends").t(X::Ident, b);
        }
        self.t(X::LBrace, "{");
        members(self);
        self.t(X::RBrace, "}").close()
    }

    /// `static? function (params) use ($caps) { body }`.
    pub fn closure(
        &mut self,
        is_static: bool,
        params: &[&str],
        caps: &[&str],
        body: impl FnOnce(&mut Self),
    ) -> &mut Self {
        self.open(X::Closure);
        if is_static {
            self.t(X::StaticKw, "static");
        }
        self.t(X::FunctionKw, "function");
        self.params(params);
        if !caps.is_empty() {
            self.t(X::UseKw, "use").t(X::LParen, "(").open(X::Uses);
            for (i, c) in caps.iter().enumerate() {
                if i > 0 {
                    self.t(X::Comma, ",");
                }
                self.open(X::Capture).var(c).close();
            }
            self.close().t(X::RParen, ")");
        }
        self.block(body);
        self.close()
    }

    /// `static? fn (params) => expr`.
    pub fn arrow(
        &mut self,
        is_static: bool,
        params: &[&str],
        body: impl FnOnce(&mut Self),
    ) -> &mut Self {
        self.open(X::ArrowFn);
        if is_static {
            self.t(X::StaticKw, "static");
        }
        self.t(X::FnKw, "fn");
        self.params(params);
        self.t(X::FatArrow, "=>");
        body(self);
        self.close()
    }

    /// `while ( cond ) { body }`.
    pub fn while_(
        &mut self,
        cond: impl FnOnce(&mut Self),
        body: impl FnOnce(&mut Self),
    ) -> &mut Self {
        self.open(X::While).t(X::WhileKw, "while").t(X::LParen, "(");
        cond(self);
        self.t(X::RParen, ")");
        self.block(body);
        self.close()
    }

    /// `if ( cond ) { then }`.
    pub fn if_(&mut self, cond: impl FnOnce(&mut Self), then: impl FnOnce(&mut Self)) -> &mut Self {
        self.open(X::If).t(X::IfKw, "if").t(X::LParen, "(");
        cond(self);
        self.t(X::RParen, ")");
        self.block(then);
        self.close()
    }

    /// `break N;` / `continue N;` (`level` written as is; `None`: bare).
    pub fn jump(&mut self, kind: X, level: Option<&str>) -> &mut Self {
        let (node, text) = if kind == X::BreakKw {
            (X::Break, "break")
        } else {
            (X::Continue, "continue")
        };
        self.open(node).t(kind, text);
        if let Some(l) = level {
            self.t(X::Num, l);
        }
        self.t(X::Semi, ";").close()
    }
}

/// The Mox mapping for the 0.4.1 features. `receiver` and `numeric` switch
/// the two policies off, to compare with 0.4.0 behaviour; members default to
/// public.
pub fn mapping_with(receiver: bool, numeric: bool) -> Mapping<X> {
    mapping_full(receiver, numeric, true)
}

/// [`mapping_with`], and `public_members` off keeps 0.4.0's private default.
pub fn mapping_full(receiver: bool, numeric: bool, public_members: bool) -> Mapping<X> {
    let mut b = MappingBuilder::new();
    let while_loop = b.template(templates::while_loop()).expect("registers");
    let echo = b
        .parse_template("echo", "extern:mox.echo(${value})")
        .expect("parses");
    let new_object = b
        .parse_template("new_object", "extern:mox.new(${class}, ${...args})")
        .expect("parses");
    let include = b
        .parse_template("include", "extern:mox.include(${path})")
        .expect("parses");
    let include_once = b
        .parse_template("include_once", "extern:mox.include_once(${path})")
        .expect("parses");
    let require = b
        .parse_template("require", "extern:mox.require(${path})")
        .expect("parses");
    let require_once = b
        .parse_template("require_once", "extern:mox.require_once(${path})")
        .expect("parses");

    let mut policy = LangPolicy::php();
    if receiver {
        policy.implicit_receiver = Some(b.name("this"));
    }
    if numeric {
        policy.break_levels = BreakLevels::Numeric;
    }
    if public_members {
        policy.default_member_visibility = hir_lang::Vis::Public;
    }
    b.policy(policy);

    let a = Pick::Absent;
    b.node(
        X::File,
        Arch::Module {
            name: a,
            items: Pick::Nodes,
        },
    );
    b.node(
        X::ClassDecl,
        Arch::Class {
            name: Pick::After(X::ClassKw),
            generics: a,
            bases: Pick::After(X::ExtendsKw),
            interfaces: a,
            members: Pick::Nodes,
            mixin: false,
        },
    );
    b.node(
        X::TraitDecl,
        Arch::Class {
            name: Pick::After(X::TraitKw),
            generics: a,
            bases: a,
            interfaces: a,
            members: Pick::Nodes,
            mixin: true,
        },
    );
    b.node(
        X::InterfaceDecl,
        Arch::Interface {
            name: Pick::After(X::InterfaceKw),
            generics: a,
            supers: a,
            items: Pick::Nodes,
        },
    );
    b.node(
        X::ImplBlock,
        Arch::Impl {
            generics: a,
            interface: a,
            self_ty: Pick::After(X::ImplKw),
            items: Pick::Nodes,
        },
    );
    b.node(
        X::TraitUse,
        Arch::MixinUse {
            mixins: Pick::AllKind(X::Ident),
            rules: a,
        },
    );
    let function = Arch::Fn {
        name: Pick::After(X::FunctionKw),
        generics: a,
        params: Pick::Kind(X::ParamList),
        ret: a,
        throws: a,
        body: Pick::Kind(X::Block),
        where_: a,
    };
    b.node(X::Method, function);
    b.node(X::FnDecl, function);
    b.container(X::ParamList);
    b.node(
        X::Param,
        Arch::Param {
            pat: Pick::Kind(X::Var),
            ty: a,
            default: Pick::After(X::Eq),
        },
    );
    // `self` as an explicit receiver parameter (a language spelling it).
    b.node_when(
        X::TypedParam,
        Pick::Kind(X::SelfParam),
        Arch::Param {
            pat: Pick::Kind(X::SelfParam),
            ty: a,
            default: a,
        },
    );
    b.node(
        X::TypedParam,
        Arch::Param {
            pat: Pick::Kind(X::Var),
            ty: Pick::Kind(X::SpecialType),
            default: a,
        },
    );
    b.node(
        X::SpecialType,
        Arch::Path {
            segments: Pick::All,
            global: a,
            qself: a,
            qtrait: a,
        },
    );
    b.node(
        X::Property,
        Arch::FieldDecl {
            name: Pick::Kind(X::Var),
            ty: a,
            default: Pick::After(X::Eq),
        },
    );
    b.node(
        X::Block,
        Arch::Block {
            stmts: Pick::Nodes,
            tail: a,
            label: a,
            tail_from_last: false,
        },
    );
    b.node(X::ExprStmt, Arch::ExprStmt { expr: Pick::Nth(0) });
    b.template_node(X::Echo, echo, &[Pick::After(X::EchoKw)]);
    b.node(X::Assign, Arch::binary());
    b.node(X::Binary, Arch::binary());
    b.node(
        X::Member,
        Arch::Member {
            base: Pick::Nth(0),
            member: Pick::After(X::Arrow),
            args: Pick::Kind(X::ArgList),
            call: Pick::Kind(X::LParen),
        },
    );
    b.node(
        X::StaticMember,
        Arch::Path {
            segments: Pick::All,
            global: a,
            qself: a,
            qtrait: a,
        },
    );
    b.node(
        X::Call,
        Arch::Call {
            callee: Pick::Nth(0),
            args: Pick::Kind(X::ArgList),
        },
    );
    b.container(X::ArgList);
    b.node(
        X::Arg,
        Arch::Arg {
            name: a,
            value: Pick::Back(0),
        },
    );
    b.template_node(
        X::New,
        new_object,
        &[Pick::After(X::NewKw), Pick::Kind(X::ArgList)],
    );
    b.node(
        X::Return,
        Arch::Return {
            value: Pick::After(X::ReturnKw),
        },
    );
    b.node(
        X::Closure,
        Arch::Closure {
            params: Pick::Kind(X::ParamList),
            ret: a,
            captures: Pick::Kind(X::Uses),
            body: Pick::Kind(X::Block),
            implicit: None,
        },
    );
    b.node(
        X::ArrowFn,
        Arch::Closure {
            params: Pick::Kind(X::ParamList),
            ret: a,
            captures: a,
            body: Pick::After(X::FatArrow),
            implicit: Some(CaptureMode::ByValue),
        },
    );
    b.container(X::Uses);
    b.node(
        X::Capture,
        Arch::Capture {
            name: Pick::Kind(X::Var),
        },
    );
    b.template_node(
        X::While,
        while_loop,
        &[Pick::After(X::LParen), Pick::Kind(X::Block), a],
    );
    b.node(
        X::Loop,
        Arch::Loop {
            label: a,
            body: Pick::Kind(X::Block),
        },
    );
    b.node(
        X::If,
        Arch::If {
            cond: Pick::After(X::LParen),
            then: Pick::Kind(X::Block),
            else_: a,
        },
    );
    // `break_stmt = { arch = "break", label = "level" }`.
    b.node(
        X::Break,
        Arch::Break {
            label: Pick::AllAfter(X::BreakKw),
            value: a,
        },
    );
    b.node(
        X::Continue,
        Arch::Continue {
            label: Pick::AllAfter(X::ContinueKw),
        },
    );
    b.node(
        X::Let,
        Arch::Let {
            pat: Pick::Kind(X::Var),
            ty: a,
            init: Pick::After(X::Eq),
            else_: a,
            mutable: true,
            hoist: false,
        },
    );
    // `include_expr = [{ when = "'include_once'", template = "include_once" }, …]`.
    let path = [Pick::Back(0)];
    b.template_node_when(
        X::Include,
        Pick::Kind(X::IncludeOnceKw),
        include_once,
        &path,
    );
    b.template_node_when(X::Include, Pick::Kind(X::RequireKw), require, &path);
    b.template_node_when(
        X::Include,
        Pick::Kind(X::RequireOnceKw),
        require_once,
        &path,
    );
    b.template_node(X::Include, include, &path);
    b.node(
        X::ArrayLit,
        Arch::Map {
            entries: Pick::AllKind(X::ArrayItem),
        },
    );
    b.node(
        X::ArrayItem,
        Arch::MapEntry {
            key: Pick::Before(X::FatArrow),
            value: Pick::Back(0),
        },
    );
    b.node(
        X::Paren,
        Arch::Pass {
            inner: Pick::After(X::LParen),
        },
    );

    b.skip(X::Comma);
    b.skip(X::Semi);
    b.skip(X::ColonColon);
    b.token(X::Var, TokenRole::Var { sigil: 1 });
    b.token(X::Ident, TokenRole::Name);
    b.token(X::Num, TokenRole::Number);
    b.token(X::Str, TokenRole::Str(Escapes::Quotes));
    b.token(X::NullKw, TokenRole::Null);
    b.token(X::SelfParam, TokenRole::Name);
    b.modifier(X::PublicKw, Modifier::Public);
    b.modifier(X::PrivateKw, Modifier::Private);
    b.modifier(X::StaticKw, Modifier::Static);
    b.modifier(X::SelfParam, Modifier::Receiver);
    // `"..." = { param = "rest", arg = "spread" }`, and spread entries.
    b.modifier_in(X::Dots, "param", Modifier::Rest)
        .modifier_in(X::Dots, "arg", Modifier::Spread)
        .modifier_in(X::Dots, "map_entry", Modifier::Spread);
    b.root(X::ParentKw, PathRoot::ParentType);
    b.root(X::SelfKw, PathRoot::SelfType);
    b.binary_op(X::Eq, OpSpec::Assign);
    b.binary_op(X::Plus, OpSpec::Op(OpKind::Add));
    b.binary_op(X::Lt, OpSpec::Op(OpKind::Lt));
    // Keyword type names (`[semantics.type_names]`).
    b.type_name("array", TypeName::Any)
        .type_name("callable", TypeName::Any)
        .type_name("null", TypeName::Any);
    b.build().expect("the Mox 0.4.1 mapping is valid")
}

/// The mapping with every 0.4.1 policy on.
pub fn mapping() -> Mapping<X> {
    mapping_with(true, true)
}

/// Lowers a tree written by `f` under a file node.
pub fn lower(m: &Mapping<X>, f: impl FnOnce(&mut W)) -> (Lowered, Interner, String) {
    let mut w = W::new();
    w.open(X::File);
    f(&mut w);
    w.close();
    let (tree, src) = w.finish();
    let mut names = Interner::new();
    let out = lower_lang::lower(&tree, &src, m, &mut names).expect("within budget");
    assert_eq!(out.hir.validate(), Ok(()));
    (out, names, src)
}

/// Resolves (resolve-lang 0.4, PHP policy) and returns the HIR and the
/// diagnostics' messages.
pub fn resolve(out: &Lowered, names: &Interner) -> (hir_lang::Hir, Vec<String>) {
    let res = resolve_lang::Resolver::new(resolve_lang::Policy::php())
        .resolve(out.hir.clone(), names)
        .expect("within the resolver's budget");
    let msgs = res.diagnostics().iter().map(|d| d.message(names)).collect();
    let unit = out.hir.unit();
    let hir = res.hir(unit).cloned().expect("the unit");
    (hir, msgs)
}

/// How a run ended: the printed lines, or the uncaught error.
#[derive(Debug, PartialEq, Eq)]
pub enum Run {
    Printed(Vec<String>),
    Failed(String, Vec<String>),
}

/// Lowers, resolves (no diagnostics allowed), compiles with bcgen-lang 0.4,
/// and runs on bvm-lang; what `mox.echo` printed, one entry per call.
pub fn run(f: impl FnOnce(&mut W)) -> Run {
    let m = mapping();
    let (out, names, _) = lower(&m, f);
    assert!(
        out.problems.is_empty(),
        "lowering problems: {:?}",
        out.problems
    );
    let (hir, diags) = resolve(&out, &names);
    assert!(
        diags.is_empty(),
        "resolution diagnostics: {diags:?}\n{}",
        hir_lang::print(&hir, &names)
    );
    execute(&hir, &names)
}

/// Compiles and runs resolved HIR.
pub fn execute(hir: &hir_lang::Hir, names: &Interner) -> Run {
    let opts = bcgen_lang::Options::new()
        .with_body(bcgen_lang::BodyMode::Entry)
        .with_intrinsics(bcgen_lang::Intrinsics::mox());
    let compiled = bcgen_lang::Compiler::new(opts)
        .compile(hir, names)
        .unwrap_or_else(|e| panic!("compile error: {e}\n{}", hir_lang::print(hir, names)));
    let printed = Arc::new(Mutex::new(Vec::new()));
    let mut host = bvm_lang::Host::new();
    let out = printed.clone();
    let _ = host.register("mox", "echo", move |ctx, args| {
        let mut line = String::new();
        for &v in args {
            line.push_str(&show(ctx, v));
        }
        out.lock().expect("unpoisoned").push(line);
        Ok(bvm_lang::Value::Nil)
    });
    // The class operations' fallbacks for names the unit does not declare
    // (and the include family): the tests never reach them.
    for name in [
        "new",
        "instanceof",
        "clone",
        "cast_object",
        "include",
        "include_once",
        "require",
        "require_once",
    ] {
        let _ = host.register("mox", name, |_, _| {
            Err(bvm_lang::HostError::Throw(bvm_lang::Value::Nil))
        });
    }
    let program = bvm_lang::Program::load(compiled.module().clone(), &host)
        .unwrap_or_else(|e| panic!("load error: {e}"));
    let mut vm = bvm_lang::Vm::new(&program);
    let r = vm.run(compiled.entry(), &[]);
    let lines = printed.lock().expect("unpoisoned").clone();
    match r {
        Ok(_) => Run::Printed(lines),
        Err(e) => Run::Failed(e.to_string(), lines),
    }
}

fn show(ctx: &bvm_lang::HostCtx<'_>, v: bvm_lang::Value) -> String {
    match v {
        bvm_lang::Value::Nil => String::new(),
        bvm_lang::Value::Bool(true) => "1".into(),
        bvm_lang::Value::Bool(false) => String::new(),
        bvm_lang::Value::Int(i) => i.to_string(),
        bvm_lang::Value::Float(f) => format!("{f:?}"),
        other => match ctx.str_bytes(other) {
            Some(b) => String::from_utf8_lossy(b).into_owned(),
            None => "?".into(),
        },
    }
}

/// Every printed line, for `Run::Printed`.
pub fn lines(v: &[&str]) -> Run {
    Run::Printed(v.iter().map(|s| (*s).to_string()).collect())
}
