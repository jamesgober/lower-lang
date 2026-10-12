//! Lowering templates: small HIR-shaped ASTs with `${field}` holes.
//!
//! A template is what LexerSketch's `[semantics.templates]` section writes as
//! quasi-quoted HIR (`while = "loop { if ${cond} {} else { break }; ${body} }"`).
//! Templates are built programmatically with a [`TemplateBuilder`], or parsed
//! from that text with [`Template::parse`] (the `text` module, a pure front
//! end that produces the same [`Template`] values).
//!
//! Every instantiation records its own HIR expansion. Binders and labels a
//! template introduces are created fresh per instantiation, carry that
//! expansion's hygiene mark, and are referenced by binder id, so they can
//! neither capture nor be captured by user names. Free names the template
//! writes ([`global`](TemplateBuilder::global)) carry the mark too and are
//! resolved at the template's definition site; [`extern_`](TemplateBuilder::extern_)
//! names a host symbol directly.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::fmt;

use hir_lang::{CaptureMode, ExpnKind, Op, OpKind, Prim};

/// Marks an absent optional child in the node arena.
pub(crate) const NONE: u32 = u32::MAX;

/// The most nodes a template's tree may have once shared sub-trees are
/// counted at every use; larger templates are refused by
/// [`TemplateBuilder::finish`].
pub const MAX_TEMPLATE_SIZE: u32 = 1 << 20;

/// A template registered in a mapping, from
/// [`MappingBuilder::template`](crate::MappingBuilder::template).
///
/// # Examples
///
/// ```
/// use lower_lang::{templates, MappingBuilder};
///
/// let mut b = MappingBuilder::<u8>::new();
/// let first = b.template(templates::while_loop())?;
/// let second = b.template(templates::do_while())?;
/// assert_eq!((first.index(), second.index()), (0, 1));
/// # Ok::<(), lower_lang::MappingError>(())
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TemplateId(pub(crate) u32);

impl TemplateId {
    /// The registration position of the template in its mapping.
    ///
    /// # Examples
    ///
    /// ```
    /// use lower_lang::{templates, MappingBuilder};
    ///
    /// let mut b = MappingBuilder::<u8>::new();
    /// assert_eq!(b.template(templates::logical_or())?.index(), 0);
    /// # Ok::<(), lower_lang::MappingError>(())
    /// ```
    #[must_use]
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// What a hole is filled with, and where it may be used.
///
/// # Examples
///
/// ```
/// use lower_lang::{HoleKind, TemplateBuilder};
///
/// let mut t = TemplateBuilder::new("id");
/// let x = t.expr_hole("x");
/// let root = t.hole(x);
/// let template = t.finish(root)?;
/// assert_eq!(template.hole(0), Some(("x", HoleKind::Expr, false)));
/// # Ok::<(), lower_lang::TemplateError>(())
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HoleKind {
    /// One expression.
    Expr,
    /// One pattern (its names bind as locals).
    Pat,
    /// One type.
    Ty,
    /// Zero or more statements, spliced into a block.
    Stmts,
    /// Zero or more call arguments, spliced into an argument or element list.
    Args,
    /// The user's label for a loop or block the template creates.
    Label,
}

/// A literal a template writes (strings are written with
/// [`TemplateBuilder::str`]).
///
/// # Examples
///
/// ```
/// use lower_lang::TLit;
///
/// assert_ne!(TLit::Int(1), TLit::Bool(true));
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TLit {
    /// An integer.
    Int(i64),
    /// A float.
    Float(f64),
    /// `true` / `false`.
    Bool(bool),
    /// `null`.
    Null,
    /// A character.
    Char(char),
}

macro_rules! handle {
    ($(#[$m:meta])* $name:ident, $int:ty) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub struct $name(pub(crate) $int);
    };
}

handle!(
    /// An expression node of a template under construction.
    TExpr,
    u32
);
handle!(
    /// A pattern node of a template under construction.
    TPat,
    u32
);
handle!(
    /// A type node of a template under construction.
    TTy,
    u32
);
handle!(
    /// A statement node of a template under construction.
    TStmt,
    u32
);
handle!(
    /// A binder the template introduces (fresh and hygienic per instantiation).
    TBinder,
    u16
);
handle!(
    /// A loop or block label the template introduces.
    TLabel,
    u16
);
handle!(
    /// An expression hole.
    ExprHole,
    u16
);
handle!(
    /// A pattern hole.
    PatHole,
    u16
);
handle!(
    /// A type hole.
    TyHole,
    u16
);
handle!(
    /// A statement-list hole.
    StmtsHole,
    u16
);
handle!(
    /// An argument-list hole.
    ArgsHole,
    u16
);
handle!(
    /// A label hole.
    LabelHole,
    u16
);

/// The label of a loop or block a template creates, or a jump's target.
///
/// # Examples
///
/// ```
/// use lower_lang::{LabelRef, TemplateBuilder};
///
/// let mut t = TemplateBuilder::new("forever");
/// let user = t.label_hole("label");
/// let brk = t.break_(Some(LabelRef::Hole(user)), None);
/// let body = t.block(&[], Some(brk));
/// let root = t.loop_(Some(LabelRef::Hole(user)), body, None);
/// assert!(t.finish(root).is_ok());
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LabelRef {
    /// A label the template defines.
    Own(TLabel),
    /// The user's label, from a label hole.
    Hole(LabelHole),
}

/// An argument of [`TemplateBuilder::apply`]: what fills one hole of the
/// applied template, in its hole order.
///
/// # Examples
///
/// ```
/// use lower_lang::TArg;
///
/// assert_eq!(TArg::Absent, TArg::Absent);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TArg {
    /// An expression for an expression hole.
    Expr(TExpr),
    /// A pattern for a pattern hole.
    Pat(TPat),
    /// A type for a type hole.
    Ty(TTy),
    /// This template's statement-list hole, forwarded.
    Stmts(StmtsHole),
    /// This template's argument-list hole, forwarded.
    Args(ArgsHole),
    /// A label for a label hole.
    Label(LabelRef),
    /// Nothing, for an optional hole.
    Absent,
}

impl TArg {
    pub(crate) const fn kind(self) -> Option<HoleKind> {
        match self {
            Self::Expr(_) => Some(HoleKind::Expr),
            Self::Pat(_) => Some(HoleKind::Pat),
            Self::Ty(_) => Some(HoleKind::Ty),
            Self::Stmts(_) => Some(HoleKind::Stmts),
            Self::Args(_) => Some(HoleKind::Args),
            Self::Label(_) => Some(HoleKind::Label),
            Self::Absent => None,
        }
    }
}

/// A label slot in the node arena.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum LabelSlot {
    None,
    Own(u16),
    Hole(u16),
}

impl LabelSlot {
    fn of(label: Option<LabelRef>) -> Self {
        match label {
            None => Self::None,
            Some(LabelRef::Own(l)) => Self::Own(l.0),
            Some(LabelRef::Hole(h)) => Self::Hole(h.0),
        }
    }
}

/// A list in the template's pool: `(start, len)`.
pub(crate) type TList = (u32, u32);

/// One template node. Children are indexes into the node arena.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum TNode {
    // ---- expressions
    Hole(u16),
    Splice(u16),
    Lit(TLit),
    Str(u32),
    Var(u16),
    Global(u32),
    Extern(u32),
    Call {
        callee: u32,
        args: TList,
    },
    Method {
        recv: u32,
        name: u32,
        args: TList,
    },
    Field {
        base: u32,
        name: u32,
    },
    Index {
        base: u32,
        index: u32,
    },
    Op {
        op: Op,
        exact: bool,
        args: TList,
    },
    Assign {
        target: u32,
        value: u32,
    },
    Block {
        stmts: TList,
        tail: u32,
        label: LabelSlot,
    },
    If {
        cond: u32,
        then: u32,
        else_: u32,
    },
    Match {
        scrutinee: u32,
        arms: TList,
    },
    Loop {
        label: LabelSlot,
        body: u32,
        step: u32,
    },
    Break {
        label: LabelSlot,
        value: u32,
    },
    Continue {
        label: LabelSlot,
    },
    Return(u32),
    Throw(u32),
    Await(u32),
    Yield {
        key: u32,
        value: u32,
    },
    YieldFrom(u32),
    Spawn(u32),
    Tuple(TList),
    Array(TList),
    Closure {
        params: TList,
        body: u32,
        implicit: Option<CaptureMode>,
    },
    Apply {
        template: u32,
        args: TList,
    },
    LetPlace {
        binder: u16,
        place: u32,
        body: u32,
    },
    // ---- statements
    Let {
        pat: u32,
        ty: u32,
        init: u32,
    },
    ExprStmt(u32),
    Defer(u32),
    Stmts(u16),
    // ---- patterns
    PHole(u16),
    PWild,
    PBind(u16),
    PLit(TLit),
    PTuple(TList),
    PCtor {
        path: u32,
        elems: TList,
    },
    // ---- types
    THole(u16),
    TPrim(Prim),
    TNamed(u32),
    TInfer,
    TAny,
    TUnion(TList),
    TInter(TList),
}

/// A declared hole.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HoleDecl {
    pub(crate) name: Box<str>,
    pub(crate) kind: HoleKind,
    pub(crate) optional: bool,
    pub(crate) default: Option<TLit>,
}

/// A declared binder or label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BinderDecl {
    pub(crate) name: u32,
    pub(crate) mutable: bool,
}

/// A finished, checked lowering template.
///
/// Register it in a mapping with
/// [`MappingBuilder::template`](crate::MappingBuilder::template), then use it
/// for a node kind ([`template_node`](crate::MappingBuilder::template_node))
/// or an operator ([`OpSpec::Template`](crate::OpSpec::Template)).
///
/// # Examples
///
/// ```
/// use lower_lang::templates;
///
/// let t = templates::while_loop();
/// assert_eq!(t.name(), "while");
/// assert_eq!(t.hole_count(), 3);
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct Template {
    pub(crate) name: Box<str>,
    pub(crate) kind: ExpnKind,
    pub(crate) nodes: Vec<TNode>,
    pub(crate) lists: Vec<u32>,
    pub(crate) texts: Vec<Box<str>>,
    pub(crate) holes: Vec<HoleDecl>,
    pub(crate) binders: Vec<BinderDecl>,
    pub(crate) labels: Vec<BinderDecl>,
    pub(crate) args: Vec<TArg>,
    pub(crate) applies: Vec<u32>,
    pub(crate) root: u32,
    /// Set when registered: where this template's texts start in the
    /// mapping's string table.
    pub(crate) text_base: u32,
}

impl Template {
    /// The template's name (also the expansion's name in HIR origins).
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(lower_lang::templates::do_while().name(), "do-while");
    /// ```
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The expansion kind instantiations record (`Template` by default,
    /// `Desugar` for the built-in desugarings).
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::ExpnKind;
    ///
    /// assert_eq!(lower_lang::templates::while_loop().expansion_kind(), ExpnKind::Desugar);
    /// ```
    #[must_use]
    pub const fn expansion_kind(&self) -> ExpnKind {
        self.kind
    }

    /// The number of holes; a node that uses the template binds one pick per
    /// hole, in this order.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(lower_lang::templates::c_for().hole_count(), 5);
    /// ```
    #[must_use]
    pub fn hole_count(&self) -> usize {
        self.holes.len()
    }

    /// Hole `index`: its name, kind, and whether it may be absent.
    ///
    /// # Examples
    ///
    /// ```
    /// use lower_lang::HoleKind;
    ///
    /// let t = lower_lang::templates::while_loop();
    /// assert_eq!(t.hole(0), Some(("cond", HoleKind::Expr, false)));
    /// assert_eq!(t.hole(2), Some(("label", HoleKind::Label, true)));
    /// assert_eq!(t.hole(3), None);
    /// ```
    #[must_use]
    pub fn hole(&self, index: usize) -> Option<(&str, HoleKind, bool)> {
        self.holes
            .get(index)
            .map(|h| (&*h.name, h.kind, h.optional))
    }

    /// Parses a template written in HIR's textual template syntax (HIR spec
    /// §20, LSF2 §15.8): the lazy path for `[semantics.templates]` and
    /// inline `[semantics.lower]` texts.
    ///
    /// Holes are `${name}` (its kind — expression, pattern, type, or label —
    /// follows from where it is written), `${name?}` (optional),
    /// `${name?=<literal>}` (optional with a default), and `${...name}` (an
    /// argument-list splice in a call, tuple, or array, or a statement
    /// splice in a block). Holes are numbered in order of first appearance.
    /// Names bound with `let`, `let_place`, a pattern, or a closure parameter
    /// are template binders, fresh and hygienic per instantiation; labels
    /// (`'l loop { … }`) likewise. Free names are written `global:a::b`,
    /// host symbols `extern:mox.echo`. Operations are written by their OPS
    /// name: `add(a, b)` takes the language's policies, `add<overflow=wrap>(a,
    /// b)` exactly the ones written. `apply:<name>(…)` needs templates
    /// registered before this one: use
    /// [`MappingBuilder::parse_template`](crate::MappingBuilder::parse_template)
    /// for it. The full grammar is in `docs/API.md`.
    ///
    /// # Errors
    ///
    /// [`TemplateError::Syntax`] with the byte range and a
    /// [`SyntaxError`](crate::SyntaxError) for any text that is not a valid
    /// template (never a panic, at any length or depth); `TooLarge` when the
    /// result outgrows the template tables.
    ///
    /// # Examples
    ///
    /// ```
    /// use lower_lang::{HoleKind, Template};
    ///
    /// // PHP's `&&`, which returns a bool.
    /// let and = Template::parse("bool_and", "if ${lhs} { not(not(${rhs})) } else { false }")?;
    /// assert_eq!(and.hole(0), Some(("lhs", HoleKind::Expr, false)));
    /// assert_eq!(and.hole(1), Some(("rhs", HoleKind::Expr, false)));
    ///
    /// // `++$i`, with the place evaluated once.
    /// let inc = Template::parse("pre_increment", "let_place p = ${place} in p = add(p, 1)")?;
    /// assert_eq!(inc.hole_count(), 1);
    /// # Ok::<(), lower_lang::TemplateError>(())
    /// ```
    pub fn parse(name: &str, text: &str) -> Result<Self, TemplateError> {
        crate::text::parse(name, text, None)
    }

    /// Records `Desugar` expansions instead of `Template` ones (LSF2's
    /// `{ text = …, desugar = true }`), so tools can re-sugar the output.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::ExpnKind;
    /// use lower_lang::Template;
    ///
    /// let t = Template::parse("echo", "extern:mox.echo(${...values})")?.desugar();
    /// assert_eq!(t.expansion_kind(), ExpnKind::Desugar);
    /// # Ok::<(), lower_lang::TemplateError>(())
    /// ```
    #[must_use]
    pub fn desugar(mut self) -> Self {
        self.kind = ExpnKind::Desugar;
        self
    }

    /// The index of the hole named `name`, for binding node fields to holes
    /// by name (LSF2: a hole missing from `holes` takes the node's field of
    /// the same name).
    ///
    /// # Examples
    ///
    /// ```
    /// use lower_lang::Template;
    ///
    /// let t = Template::parse("new_object", "extern:mox.new(${class}, ${...args})")?;
    /// assert_eq!(t.hole_index("args"), Some(1));
    /// assert_eq!(t.hole_index("nope"), None);
    /// # Ok::<(), lower_lang::TemplateError>(())
    /// ```
    #[must_use]
    pub fn hole_index(&self, name: &str) -> Option<usize> {
        self.holes.iter().position(|h| &*h.name == name)
    }

    pub(crate) fn node(&self, index: u32) -> Option<&TNode> {
        self.nodes.get(index as usize)
    }

    pub(crate) fn list(&self, list: TList) -> &[u32] {
        let (start, len) = (list.0 as usize, list.1 as usize);
        self.lists
            .get(start..start.saturating_add(len))
            .unwrap_or(&[])
    }
}

/// Why a template was refused.
///
/// # Examples
///
/// ```
/// use lower_lang::{TemplateBuilder, TemplateError};
///
/// let mut t = TemplateBuilder::new("unused");
/// let _x = t.expr_hole("x");
/// let root = t.lit(lower_lang::TLit::Null);
/// assert_eq!(t.finish(root), Err(TemplateError::HoleUnused { hole: 0 }));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TemplateError {
    /// A declared hole is never used. Use it, or remove it (every hole must
    /// receive its user code somewhere, or that code would be lost).
    HoleUnused {
        /// The hole's index.
        hole: usize,
    },
    /// A binder is used but no pattern or closure parameter binds it.
    BinderUnbound {
        /// The binder's index.
        binder: usize,
    },
    /// A binder is bound by two patterns or parameters. Declare one binder
    /// per binding site.
    BinderBoundTwice {
        /// The binder's index.
        binder: usize,
    },
    /// A label is jumped to but no loop or block defines it.
    LabelUndefined {
        /// The label's index.
        label: usize,
    },
    /// A label is defined by two loops or blocks.
    LabelDefinedTwice {
        /// The label's index.
        label: usize,
    },
    /// An argument-list hole is spliced somewhere other than a call's
    /// arguments or a tuple's or array's elements.
    SpliceOutOfList {
        /// The hole's index.
        hole: usize,
    },
    /// A handle from another template builder (or a list longer than the
    /// arena) was used.
    ForeignHandle,
    /// The tree, counting shared sub-trees at every use, has more than
    /// [`MAX_TEMPLATE_SIZE`] nodes, or a table outgrew its index type.
    TooLarge,
    /// A template text ([`Template::parse`]) is not valid template syntax:
    /// what is wrong, and the byte range of the text it is about.
    Syntax {
        /// The offending bytes of the template text.
        at: hir_lang::Span,
        /// What is wrong there.
        error: crate::SyntaxError,
    },
}

impl fmt::Display for TemplateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HoleUnused { hole } => write!(f, "hole {hole} is never used"),
            Self::BinderUnbound { binder } => write!(f, "binder {binder} is used but never bound"),
            Self::BinderBoundTwice { binder } => write!(f, "binder {binder} is bound twice"),
            Self::LabelUndefined { label } => write!(f, "label {label} is used but never defined"),
            Self::LabelDefinedTwice { label } => write!(f, "label {label} is defined twice"),
            Self::SpliceOutOfList { hole } => {
                write!(
                    f,
                    "argument hole {hole} is spliced outside an argument or element list"
                )
            }
            Self::ForeignHandle => f.write_str("a handle from another template builder was used"),
            Self::TooLarge => f.write_str("the template is too large"),
            Self::Syntax { at, error } => write!(
                f,
                "template text, bytes {}..{}: {error}",
                at.start().to_usize(),
                at.end().to_usize()
            ),
        }
    }
}

impl core::error::Error for TemplateError {}

/// Builds a [`Template`] bottom-up, like hir-lang's `Builder` builds HIR.
///
/// Declare holes and binders, build nodes, then [`finish`](Self::finish) with
/// the root expression. Every method is infallible; problems (a foreign
/// handle, an unbound binder, an unused hole) are reported by `finish`.
///
/// # Examples
///
/// `unless cond { body }` as `if cond {} else { body }`:
///
/// ```
/// use lower_lang::TemplateBuilder;
///
/// let mut t = TemplateBuilder::new("unless");
/// let cond = t.expr_hole("cond");
/// let body = t.expr_hole("body");
/// let c = t.hole(cond);
/// let empty = t.block(&[], None);
/// let b = t.hole(body);
/// let root = t.if_(c, empty, Some(b));
/// let unless = t.finish(root)?;
/// assert_eq!(unless.hole_count(), 2);
/// # Ok::<(), lower_lang::TemplateError>(())
/// ```
#[derive(Clone, Debug)]
pub struct TemplateBuilder {
    pub(crate) t: Template,
    overflow: bool,
}

impl TemplateBuilder {
    /// An empty template named `name`, recording `Template` expansions.
    ///
    /// # Examples
    ///
    /// ```
    /// let t = lower_lang::TemplateBuilder::new("swap");
    /// let _ = t;
    /// ```
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            t: Template {
                name: name.into(),
                kind: ExpnKind::Template,
                nodes: Vec::new(),
                lists: Vec::new(),
                texts: Vec::new(),
                holes: Vec::new(),
                binders: Vec::new(),
                labels: Vec::new(),
                args: Vec::new(),
                applies: Vec::new(),
                root: NONE,
                text_base: 0,
            },
            overflow: false,
        }
    }

    /// Records `Desugar` expansions instead (for built-in sugar such as
    /// `while`, so tools can re-sugar it).
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::ExpnKind;
    /// use lower_lang::{TLit, TemplateBuilder};
    ///
    /// let mut t = TemplateBuilder::new("nothing").desugar();
    /// let root = t.lit(TLit::Null);
    /// assert_eq!(t.finish(root)?.expansion_kind(), ExpnKind::Desugar);
    /// # Ok::<(), lower_lang::TemplateError>(())
    /// ```
    #[must_use]
    pub fn desugar(mut self) -> Self {
        self.t.kind = ExpnKind::Desugar;
        self
    }

    // ---------------------------------------------------------------- tables

    fn push(&mut self, node: TNode) -> u32 {
        match u32::try_from(self.t.nodes.len()) {
            Ok(i) if i < NONE => {
                self.t.nodes.push(node);
                i
            }
            _ => {
                self.overflow = true;
                NONE
            }
        }
    }

    fn list(&mut self, items: impl IntoIterator<Item = u32>) -> TList {
        let start = self.t.lists.len();
        self.t.lists.extend(items);
        match (
            u32::try_from(start),
            u32::try_from(self.t.lists.len() - start),
        ) {
            (Ok(s), Ok(l)) => (s, l),
            _ => {
                self.overflow = true;
                (0, 0)
            }
        }
    }

    fn text(&mut self, text: &str) -> u32 {
        let i = self.t.texts.len();
        self.t.texts.push(text.into());
        u32::try_from(i).unwrap_or_else(|_| {
            self.overflow = true;
            0
        })
    }

    fn hole_decl(
        &mut self,
        name: &str,
        kind: HoleKind,
        optional: bool,
        default: Option<TLit>,
    ) -> u16 {
        let i = self.t.holes.len();
        self.t.holes.push(HoleDecl {
            name: name.into(),
            kind,
            optional,
            default,
        });
        u16::try_from(i).unwrap_or_else(|_| {
            self.overflow = true;
            0
        })
    }

    fn opt(e: Option<TExpr>) -> u32 {
        e.map_or(NONE, |e| e.0)
    }

    // ----------------------------------------------------------------- holes

    /// Declares a required expression hole.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut t = lower_lang::TemplateBuilder::new("t");
    /// let h = t.expr_hole("value");
    /// let root = t.hole(h);
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn expr_hole(&mut self, name: &str) -> ExprHole {
        ExprHole(self.hole_decl(name, HoleKind::Expr, false, None))
    }

    /// Declares an optional expression hole. When the field is absent the hole
    /// is `default` if given; otherwise it is left out where the slot is
    /// optional (an `else`, a loop step, a `return` value) and is an error
    /// where a value is required.
    ///
    /// # Examples
    ///
    /// ```
    /// use lower_lang::TLit;
    ///
    /// let mut t = lower_lang::TemplateBuilder::new("t");
    /// let cond = t.optional_expr_hole("cond", Some(TLit::Bool(true)));
    /// let root = t.hole(cond);
    /// assert_eq!(t.finish(root)?.hole(0).map(|h| h.2), Some(true));
    /// # Ok::<(), lower_lang::TemplateError>(())
    /// ```
    pub fn optional_expr_hole(&mut self, name: &str, default: Option<TLit>) -> ExprHole {
        ExprHole(self.hole_decl(name, HoleKind::Expr, true, default))
    }

    /// Declares a pattern hole; an absent optional pattern is `_`.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut t = lower_lang::TemplateBuilder::new("t");
    /// let p = t.pat_hole("pat", false);
    /// let pat = t.pat(p);
    /// let unit = t.tuple(&[]);
    /// let s = t.let_(pat, None, Some(unit));
    /// let root = t.block(&[s], None);
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn pat_hole(&mut self, name: &str, optional: bool) -> PatHole {
        PatHole(self.hole_decl(name, HoleKind::Pat, optional, None))
    }

    /// Declares a type hole; an absent optional type is left out where the
    /// slot is optional and is `_` (inferred) elsewhere.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut t = lower_lang::TemplateBuilder::new("t");
    /// let ty = t.ty_hole("ty", true);
    /// let x = t.binder("x");
    /// let pat = t.bind(x);
    /// let ann = t.ty(ty);
    /// let s = t.let_(pat, Some(ann), None);
    /// let root = t.block(&[s], None);
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn ty_hole(&mut self, name: &str, optional: bool) -> TyHole {
        TyHole(self.hole_decl(name, HoleKind::Ty, optional, None))
    }

    /// Declares a statement-list hole (zero or more statements).
    ///
    /// # Examples
    ///
    /// ```
    /// let mut t = lower_lang::TemplateBuilder::new("t");
    /// let body = t.stmts_hole("body");
    /// let s = t.stmts(body);
    /// let root = t.block(&[s], None);
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn stmts_hole(&mut self, name: &str) -> StmtsHole {
        StmtsHole(self.hole_decl(name, HoleKind::Stmts, true, None))
    }

    /// Declares an argument-list hole (zero or more arguments).
    ///
    /// # Examples
    ///
    /// ```
    /// let mut t = lower_lang::TemplateBuilder::new("echo");
    /// let args = t.args_hole("args");
    /// let callee = t.extern_("echo");
    /// let spliced = t.splice(args);
    /// let root = t.call(callee, &[spliced]);
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn args_hole(&mut self, name: &str) -> ArgsHole {
        ArgsHole(self.hole_decl(name, HoleKind::Args, true, None))
    }

    /// Declares a label hole: the user's label (optional) for a loop or block
    /// the template creates.
    ///
    /// # Examples
    ///
    /// ```
    /// use lower_lang::LabelRef;
    ///
    /// let mut t = lower_lang::TemplateBuilder::new("t");
    /// let l = t.label_hole("label");
    /// let body = t.block(&[], None);
    /// let root = t.loop_(Some(LabelRef::Hole(l)), body, None);
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn label_hole(&mut self, name: &str) -> LabelHole {
        LabelHole(self.hole_decl(name, HoleKind::Label, true, None))
    }

    // -------------------------------------------------------- binders, labels

    /// Declares an immutable local binder (hygienic: fresh per instantiation).
    ///
    /// # Examples
    ///
    /// ```
    /// let mut t = lower_lang::TemplateBuilder::new("t");
    /// let tmp = t.binder("tmp");
    /// let pat = t.bind(tmp);
    /// let one = t.lit(lower_lang::TLit::Int(1));
    /// let s = t.let_(pat, None, Some(one));
    /// let use_tmp = t.var(tmp);
    /// let root = t.block(&[s], Some(use_tmp));
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn binder(&mut self, name: &str) -> TBinder {
        self.binder_decl(name, false)
    }

    /// Declares a mutable local binder.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut t = lower_lang::TemplateBuilder::new("t");
    /// let n = t.mutable_binder("n");
    /// let pat = t.bind(n);
    /// let zero = t.lit(lower_lang::TLit::Int(0));
    /// let s = t.let_(pat, None, Some(zero));
    /// let root = t.block(&[s], None);
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn mutable_binder(&mut self, name: &str) -> TBinder {
        self.binder_decl(name, true)
    }

    fn binder_decl(&mut self, name: &str, mutable: bool) -> TBinder {
        let name = self.text(name);
        let i = self.t.binders.len();
        self.t.binders.push(BinderDecl { name, mutable });
        TBinder(u16::try_from(i).unwrap_or_else(|_| {
            self.overflow = true;
            0
        }))
    }

    /// Declares a label the template defines on one of its loops or blocks.
    ///
    /// # Examples
    ///
    /// ```
    /// use lower_lang::LabelRef;
    ///
    /// let mut t = lower_lang::TemplateBuilder::new("t");
    /// let l = t.label("outer");
    /// let brk = t.break_(Some(LabelRef::Own(l)), None);
    /// let root = t.loop_(Some(LabelRef::Own(l)), brk, None);
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn label(&mut self, name: &str) -> TLabel {
        let name = self.text(name);
        let i = self.t.labels.len();
        self.t.labels.push(BinderDecl {
            name,
            mutable: false,
        });
        TLabel(u16::try_from(i).unwrap_or_else(|_| {
            self.overflow = true;
            0
        }))
    }

    // ----------------------------------------------------------- expressions

    /// The value of an expression hole.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut t = lower_lang::TemplateBuilder::new("t");
    /// let h = t.expr_hole("e");
    /// let e = t.hole(h);
    /// assert!(t.finish(e).is_ok());
    /// ```
    pub fn hole(&mut self, hole: ExprHole) -> TExpr {
        TExpr(self.push(TNode::Hole(hole.0)))
    }

    /// The arguments of an argument-list hole, spliced in place; only valid
    /// directly in [`call`](Self::call), [`method`](Self::method),
    /// [`tuple`](Self::tuple), or [`array`](Self::array) lists.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut t = lower_lang::TemplateBuilder::new("t");
    /// let args = t.args_hole("args");
    /// let items = t.splice(args);
    /// let root = t.array(&[items]);
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn splice(&mut self, hole: ArgsHole) -> TExpr {
        TExpr(self.push(TNode::Splice(hole.0)))
    }

    /// A literal.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut t = lower_lang::TemplateBuilder::new("t");
    /// let root = t.lit(lower_lang::TLit::Float(0.5));
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn lit(&mut self, lit: TLit) -> TExpr {
        TExpr(self.push(TNode::Lit(lit)))
    }

    /// A string literal.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut t = lower_lang::TemplateBuilder::new("t");
    /// let root = t.str("done");
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn str(&mut self, text: &str) -> TExpr {
        let s = self.text(text);
        TExpr(self.push(TNode::Str(s)))
    }

    /// A reference to one of the template's binders, resolved by
    /// construction.
    ///
    /// # Examples
    ///
    /// See [`binder`](Self::binder).
    pub fn var(&mut self, binder: TBinder) -> TExpr {
        TExpr(self.push(TNode::Var(binder.0)))
    }

    /// A free name the template refers to (`iter`, `std::mem::swap`), split
    /// at `::`. It carries the expansion's hygiene mark, so a user variable of
    /// the same spelling cannot capture it; a resolver looks it up at the
    /// template's definition site.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut t = lower_lang::TemplateBuilder::new("t");
    /// let f = t.global("core::mem::take");
    /// let root = t.call(f, &[]);
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn global(&mut self, path: &str) -> TExpr {
        let s = self.text(path);
        TExpr(self.push(TNode::Global(s)))
    }

    /// A host symbol (`[stdlib]`), already resolved to `Res::Extern`.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut t = lower_lang::TemplateBuilder::new("t");
    /// let f = t.extern_("mox.iter");
    /// let root = t.call(f, &[]);
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn extern_(&mut self, name: &str) -> TExpr {
        let s = self.text(name);
        TExpr(self.push(TNode::Extern(s)))
    }

    /// `callee(args)`.
    ///
    /// # Examples
    ///
    /// See [`extern_`](Self::extern_).
    pub fn call(&mut self, callee: TExpr, args: &[TExpr]) -> TExpr {
        let args = self.list(args.iter().map(|a| a.0));
        TExpr(self.push(TNode::Call {
            callee: callee.0,
            args,
        }))
    }

    /// `recv.name(args)`.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut t = lower_lang::TemplateBuilder::new("t");
    /// let h = t.expr_hole("it");
    /// let it = t.hole(h);
    /// let root = t.method(it, "next", &[]);
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn method(&mut self, recv: TExpr, name: &str, args: &[TExpr]) -> TExpr {
        let name = self.text(name);
        let args = self.list(args.iter().map(|a| a.0));
        TExpr(self.push(TNode::Method {
            recv: recv.0,
            name,
            args,
        }))
    }

    /// `base.name`.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut t = lower_lang::TemplateBuilder::new("t");
    /// let h = t.expr_hole("e");
    /// let e = t.hole(h);
    /// let root = t.field(e, "len");
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn field(&mut self, base: TExpr, name: &str) -> TExpr {
        let name = self.text(name);
        TExpr(self.push(TNode::Field { base: base.0, name }))
    }

    /// `base[index]`.
    ///
    /// # Examples
    ///
    /// ```
    /// use lower_lang::TLit;
    ///
    /// let mut t = lower_lang::TemplateBuilder::new("t");
    /// let h = t.expr_hole("e");
    /// let e = t.hole(h);
    /// let zero = t.lit(TLit::Int(0));
    /// let root = t.index(e, zero);
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn index(&mut self, base: TExpr, index: TExpr) -> TExpr {
        TExpr(self.push(TNode::Index {
            base: base.0,
            index: index.0,
        }))
    }

    /// An intrinsic operation with the language's policies (from the
    /// mapping's [`LangPolicy`](crate::LangPolicy)).
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::OpKind;
    ///
    /// let mut t = lower_lang::TemplateBuilder::new("not");
    /// let h = t.expr_hole("e");
    /// let e = t.hole(h);
    /// let root = t.op(OpKind::Not, &[e]);
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn op(&mut self, kind: OpKind, args: &[TExpr]) -> TExpr {
        let args = self.list(args.iter().map(|a| a.0));
        TExpr(self.push(TNode::Op {
            op: Op::new(kind),
            exact: false,
            args,
        }))
    }

    /// An intrinsic operation with exactly this policy.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::{Op, OpKind, Overflow};
    ///
    /// let mut t = lower_lang::TemplateBuilder::new("inc");
    /// let h = t.expr_hole("e");
    /// let e = t.hole(h);
    /// let one = t.lit(lower_lang::TLit::Int(1));
    /// let root = t.op_with(Op::new(OpKind::Add).with_overflow(Overflow::Wrap), &[e, one]);
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn op_with(&mut self, op: Op, args: &[TExpr]) -> TExpr {
        let args = self.list(args.iter().map(|a| a.0));
        TExpr(self.push(TNode::Op {
            op,
            exact: true,
            args,
        }))
    }

    /// `target = value`.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut t = lower_lang::TemplateBuilder::new("t");
    /// let p = t.expr_hole("place");
    /// let v = t.expr_hole("value");
    /// let (p, v) = (t.hole(p), t.hole(v));
    /// let root = t.assign(p, v);
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn assign(&mut self, target: TExpr, value: TExpr) -> TExpr {
        TExpr(self.push(TNode::Assign {
            target: target.0,
            value: value.0,
        }))
    }

    /// `{ stmts; tail }`.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut t = lower_lang::TemplateBuilder::new("t");
    /// let root = t.block(&[], None);
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn block(&mut self, stmts: &[TStmt], tail: Option<TExpr>) -> TExpr {
        let stmts = self.list(stmts.iter().map(|s| s.0));
        TExpr(self.push(TNode::Block {
            stmts,
            tail: Self::opt(tail),
            label: LabelSlot::None,
        }))
    }

    /// `'label: { stmts; tail }`.
    ///
    /// # Examples
    ///
    /// ```
    /// use lower_lang::LabelRef;
    ///
    /// let mut t = lower_lang::TemplateBuilder::new("t");
    /// let l = t.label("done");
    /// let brk = t.break_(Some(LabelRef::Own(l)), None);
    /// let s = t.expr_stmt(brk);
    /// let root = t.labeled_block(LabelRef::Own(l), &[s], None);
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn labeled_block(
        &mut self,
        label: LabelRef,
        stmts: &[TStmt],
        tail: Option<TExpr>,
    ) -> TExpr {
        let stmts = self.list(stmts.iter().map(|s| s.0));
        TExpr(self.push(TNode::Block {
            stmts,
            tail: Self::opt(tail),
            label: LabelSlot::of(Some(label)),
        }))
    }

    /// `if cond { then } else { else_ }`.
    ///
    /// # Examples
    ///
    /// See [`TemplateBuilder`].
    pub fn if_(&mut self, cond: TExpr, then: TExpr, else_: Option<TExpr>) -> TExpr {
        TExpr(self.push(TNode::If {
            cond: cond.0,
            then: then.0,
            else_: Self::opt(else_),
        }))
    }

    /// `match scrutinee { pat if guard => body, ... }`.
    ///
    /// # Examples
    ///
    /// ```
    /// use lower_lang::TLit;
    ///
    /// let mut t = lower_lang::TemplateBuilder::new("t");
    /// let h = t.expr_hole("e");
    /// let e = t.hole(h);
    /// let null = t.lit_pat(TLit::Null);
    /// let zero = t.lit(TLit::Int(0));
    /// let wild = t.wild();
    /// let one = t.lit(TLit::Int(1));
    /// let root = t.match_(e, &[(null, None, zero), (wild, None, one)]);
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn match_(&mut self, scrutinee: TExpr, arms: &[(TPat, Option<TExpr>, TExpr)]) -> TExpr {
        let arms = self.list(arms.iter().flat_map(|(p, g, b)| [p.0, Self::opt(*g), b.0]));
        TExpr(self.push(TNode::Match {
            scrutinee: scrutinee.0,
            arms,
        }))
    }

    /// `'label: loop { body } [step]` — HIR's one loop.
    ///
    /// # Examples
    ///
    /// See [`label_hole`](Self::label_hole).
    pub fn loop_(&mut self, label: Option<LabelRef>, body: TExpr, step: Option<TExpr>) -> TExpr {
        TExpr(self.push(TNode::Loop {
            label: LabelSlot::of(label),
            body: body.0,
            step: Self::opt(step),
        }))
    }

    /// `break 'label value`.
    ///
    /// # Examples
    ///
    /// See [`label`](Self::label).
    pub fn break_(&mut self, label: Option<LabelRef>, value: Option<TExpr>) -> TExpr {
        TExpr(self.push(TNode::Break {
            label: LabelSlot::of(label),
            value: Self::opt(value),
        }))
    }

    /// `continue 'label`.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut t = lower_lang::TemplateBuilder::new("t");
    /// let c = t.continue_(None);
    /// let root = t.loop_(None, c, None);
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn continue_(&mut self, label: Option<LabelRef>) -> TExpr {
        TExpr(self.push(TNode::Continue {
            label: LabelSlot::of(label),
        }))
    }

    /// `return value`.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut t = lower_lang::TemplateBuilder::new("t");
    /// let root = t.return_(None);
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn return_(&mut self, value: Option<TExpr>) -> TExpr {
        TExpr(self.push(TNode::Return(Self::opt(value))))
    }

    /// `throw value`.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut t = lower_lang::TemplateBuilder::new("t");
    /// let h = t.expr_hole("e");
    /// let e = t.hole(h);
    /// let root = t.throw(e);
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn throw(&mut self, value: TExpr) -> TExpr {
        TExpr(self.push(TNode::Throw(value.0)))
    }

    /// `await value`.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut t = lower_lang::TemplateBuilder::new("t");
    /// let h = t.expr_hole("e");
    /// let e = t.hole(h);
    /// let root = t.await_(e);
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn await_(&mut self, value: TExpr) -> TExpr {
        TExpr(self.push(TNode::Await(value.0)))
    }

    /// `yield value`.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut t = lower_lang::TemplateBuilder::new("t");
    /// let root = t.yield_(None);
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn yield_(&mut self, value: Option<TExpr>) -> TExpr {
        TExpr(self.push(TNode::Yield {
            key: NONE,
            value: Self::opt(value),
        }))
    }

    /// `yield key => value`: a keyed yield (PHP and Mox generators), HIR
    /// `Yield { key, value }`.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut t = lower_lang::TemplateBuilder::new("pair");
    /// let (k, v) = (t.expr_hole("key"), t.expr_hole("value"));
    /// let (k, v) = (t.hole(k), t.hole(v));
    /// let root = t.yield_kv(k, v);
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn yield_kv(&mut self, key: TExpr, value: TExpr) -> TExpr {
        TExpr(self.push(TNode::Yield {
            key: key.0,
            value: value.0,
        }))
    }

    /// `yield from source`: delegate to an inner generator or iterable; the
    /// expression's value is the inner generator's return value (HIR
    /// `YieldFrom`).
    ///
    /// # Examples
    ///
    /// ```
    /// let mut t = lower_lang::TemplateBuilder::new("delegate");
    /// let h = t.expr_hole("from");
    /// let source = t.hole(h);
    /// let root = t.yield_from(source);
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn yield_from(&mut self, source: TExpr) -> TExpr {
        TExpr(self.push(TNode::YieldFrom(source.0)))
    }

    /// `spawn callable`.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut t = lower_lang::TemplateBuilder::new("t");
    /// let h = t.expr_hole("f");
    /// let f = t.hole(h);
    /// let root = t.spawn(f);
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn spawn(&mut self, callable: TExpr) -> TExpr {
        TExpr(self.push(TNode::Spawn(callable.0)))
    }

    /// `(elems)`; `()` is unit.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut t = lower_lang::TemplateBuilder::new("t");
    /// let root = t.tuple(&[]);
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn tuple(&mut self, elems: &[TExpr]) -> TExpr {
        let elems = self.list(elems.iter().map(|e| e.0));
        TExpr(self.push(TNode::Tuple(elems)))
    }

    /// `[elems]`.
    ///
    /// # Examples
    ///
    /// See [`splice`](Self::splice).
    pub fn array(&mut self, elems: &[TExpr]) -> TExpr {
        let elems = self.list(elems.iter().map(|e| e.0));
        TExpr(self.push(TNode::Array(elems)))
    }

    /// A closure with the given parameters (each binds one of the template's
    /// binders) and implicit-capture mode.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::CaptureMode;
    ///
    /// let mut t = lower_lang::TemplateBuilder::new("thunk");
    /// let h = t.expr_hole("e");
    /// let e = t.hole(h);
    /// let root = t.closure(&[], e, Some(CaptureMode::ByRef));
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn closure(
        &mut self,
        params: &[TBinder],
        body: TExpr,
        implicit: Option<CaptureMode>,
    ) -> TExpr {
        let params = self.list(params.iter().map(|p| u32::from(p.0)));
        TExpr(self.push(TNode::Closure {
            params,
            body: body.0,
            implicit,
        }))
    }

    /// Instantiates another template here, one argument per hole of the
    /// applied template. The template must be registered in the same mapping
    /// before this one, so templates can never apply themselves; checked by
    /// [`MappingBuilder::template`](crate::MappingBuilder::template). Each
    /// application is a nested expansion, counted against
    /// [`Budget::max_expansion_depth`](crate::Budget).
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::OpKind;
    /// use lower_lang::{templates, MappingBuilder, TArg, TemplateBuilder};
    ///
    /// let mut m = MappingBuilder::<u8>::new();
    /// let while_loop = m.template(templates::while_loop())?;
    /// // until c { b }  ==  while !c { b }
    /// let mut t = TemplateBuilder::new("until");
    /// let (c, b) = (t.expr_hole("cond"), t.expr_hole("body"));
    /// let c = t.hole(c);
    /// let not_c = t.op(OpKind::Not, &[c]);
    /// let b = t.hole(b);
    /// let root = t.apply(while_loop, &[TArg::Expr(not_c), TArg::Expr(b), TArg::Absent]);
    /// let until = t.finish(root).expect("valid");
    /// assert!(m.template(until).is_ok());
    /// # Ok::<(), lower_lang::MappingError>(())
    /// ```
    pub fn apply(&mut self, template: TemplateId, args: &[TArg]) -> TExpr {
        let start = self.t.args.len();
        self.t.args.extend_from_slice(args);
        let list = match (u32::try_from(start), u32::try_from(args.len())) {
            (Ok(s), Ok(l)) => (s, l),
            _ => {
                self.overflow = true;
                (0, 0)
            }
        };
        TExpr(self.push(TNode::Apply {
            template: template.0,
            args: list,
        }))
    }

    /// `let_place binder = place in body`: evaluates the operands of the place
    /// expression `place` once and makes `binder` name that place in `body`,
    /// for reads and writes (HIR `LetPlace`, binder kind `Place`). This is how
    /// `++`, `--`, and compound assignments with a non-OPS operator evaluate
    /// their place exactly once. The binder is bound here and must not be
    /// bound anywhere else; HIR forbids capturing it in a closure.
    ///
    /// # Examples
    ///
    /// `++place` with the place evaluated once:
    ///
    /// ```
    /// use hir_lang::OpKind;
    /// use lower_lang::{TLit, TemplateBuilder};
    ///
    /// let mut t = TemplateBuilder::new("pre++");
    /// let place = t.expr_hole("place");
    /// let p = t.binder("p");
    /// let target = t.hole(place);
    /// let read = t.var(p);
    /// let one = t.lit(TLit::Int(1));
    /// let sum = t.op(OpKind::Add, &[read, one]);
    /// let write = t.var(p);
    /// let body = t.assign(write, sum);
    /// let root = t.let_place(p, target, body);
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn let_place(&mut self, binder: TBinder, place: TExpr, body: TExpr) -> TExpr {
        TExpr(self.push(TNode::LetPlace {
            binder: binder.0,
            place: place.0,
            body: body.0,
        }))
    }

    // ------------------------------------------------------------ statements

    /// `let pat: ty = init;`
    ///
    /// # Examples
    ///
    /// See [`binder`](Self::binder).
    pub fn let_(&mut self, pat: TPat, ty: Option<TTy>, init: Option<TExpr>) -> TStmt {
        TStmt(self.push(TNode::Let {
            pat: pat.0,
            ty: ty.map_or(NONE, |t| t.0),
            init: Self::opt(init),
        }))
    }

    /// `expr;`
    ///
    /// # Examples
    ///
    /// See [`labeled_block`](Self::labeled_block).
    pub fn expr_stmt(&mut self, expr: TExpr) -> TStmt {
        TStmt(self.push(TNode::ExprStmt(expr.0)))
    }

    /// `defer expr;`
    ///
    /// # Examples
    ///
    /// ```
    /// let mut t = lower_lang::TemplateBuilder::new("t");
    /// let h = t.expr_hole("cleanup");
    /// let e = t.hole(h);
    /// let s = t.defer(e);
    /// let root = t.block(&[s], None);
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn defer(&mut self, expr: TExpr) -> TStmt {
        TStmt(self.push(TNode::Defer(expr.0)))
    }

    /// The statements of a statement-list hole, spliced into the block.
    ///
    /// # Examples
    ///
    /// See [`stmts_hole`](Self::stmts_hole).
    pub fn stmts(&mut self, hole: StmtsHole) -> TStmt {
        TStmt(self.push(TNode::Stmts(hole.0)))
    }

    // -------------------------------------------------------------- patterns

    /// The pattern of a pattern hole.
    ///
    /// # Examples
    ///
    /// See [`pat_hole`](Self::pat_hole).
    pub fn pat(&mut self, hole: PatHole) -> TPat {
        TPat(self.push(TNode::PHole(hole.0)))
    }

    /// `_`.
    ///
    /// # Examples
    ///
    /// See [`match_`](Self::match_).
    pub fn wild(&mut self) -> TPat {
        TPat(self.push(TNode::PWild))
    }

    /// Binds one of the template's binders (by value).
    ///
    /// # Examples
    ///
    /// See [`binder`](Self::binder).
    pub fn bind(&mut self, binder: TBinder) -> TPat {
        TPat(self.push(TNode::PBind(binder.0)))
    }

    /// Matches a literal.
    ///
    /// # Examples
    ///
    /// See [`match_`](Self::match_).
    pub fn lit_pat(&mut self, lit: TLit) -> TPat {
        TPat(self.push(TNode::PLit(lit)))
    }

    /// `(elems)`.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut t = lower_lang::TemplateBuilder::new("t");
    /// let k = t.pat_hole("key", true);
    /// let v = t.pat_hole("value", false);
    /// let (k, v) = (t.pat(k), t.pat(v));
    /// let pair = t.tuple_pat(&[k, v]);
    /// let unit = t.tuple(&[]);
    /// let s = t.let_(pair, None, Some(unit));
    /// let root = t.block(&[s], None);
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn tuple_pat(&mut self, elems: &[TPat]) -> TPat {
        let elems = self.list(elems.iter().map(|p| p.0));
        TPat(self.push(TNode::PTuple(elems)))
    }

    /// `Path(elems)`, the path written like [`global`](Self::global).
    ///
    /// # Examples
    ///
    /// ```
    /// let mut t = lower_lang::TemplateBuilder::new("t");
    /// let h = t.expr_hole("e");
    /// let e = t.hole(h);
    /// let v = t.binder("v");
    /// let bind = t.bind(v);
    /// let some = t.ctor_pat("Some", &[bind]);
    /// let use_v = t.var(v);
    /// let wild = t.wild();
    /// let zero = t.lit(lower_lang::TLit::Int(0));
    /// let root = t.match_(e, &[(some, None, use_v), (wild, None, zero)]);
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn ctor_pat(&mut self, path: &str, elems: &[TPat]) -> TPat {
        let path = self.text(path);
        let elems = self.list(elems.iter().map(|p| p.0));
        TPat(self.push(TNode::PCtor { path, elems }))
    }

    // ----------------------------------------------------------------- types

    /// The type of a type hole.
    ///
    /// # Examples
    ///
    /// See [`ty_hole`](Self::ty_hole).
    pub fn ty(&mut self, hole: TyHole) -> TTy {
        TTy(self.push(TNode::THole(hole.0)))
    }

    /// A primitive type.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::Prim;
    ///
    /// let mut t = lower_lang::TemplateBuilder::new("t");
    /// let x = t.binder("x");
    /// let p = t.bind(x);
    /// let int = t.prim(Prim::I64);
    /// let s = t.let_(p, Some(int), None);
    /// let root = t.block(&[s], None);
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn prim(&mut self, prim: Prim) -> TTy {
        TTy(self.push(TNode::TPrim(prim)))
    }

    /// A named type, written like [`global`](Self::global).
    ///
    /// # Examples
    ///
    /// ```
    /// let mut t = lower_lang::TemplateBuilder::new("t");
    /// let x = t.binder("x");
    /// let p = t.bind(x);
    /// let ty = t.named_ty("Iterator");
    /// let s = t.let_(p, Some(ty), None);
    /// let root = t.block(&[s], None);
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn named_ty(&mut self, path: &str) -> TTy {
        let path = self.text(path);
        TTy(self.push(TNode::TNamed(path)))
    }

    /// `_`: a type to infer.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut t = lower_lang::TemplateBuilder::new("t");
    /// let x = t.binder("x");
    /// let p = t.bind(x);
    /// let ty = t.infer_ty();
    /// let s = t.let_(p, Some(ty), None);
    /// let root = t.block(&[s], None);
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn infer_ty(&mut self) -> TTy {
        TTy(self.push(TNode::TInfer))
    }

    /// The gradual dynamic type.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut t = lower_lang::TemplateBuilder::new("t");
    /// let x = t.binder("x");
    /// let p = t.bind(x);
    /// let ty = t.any_ty();
    /// let s = t.let_(p, Some(ty), None);
    /// let root = t.block(&[s], None);
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn any_ty(&mut self) -> TTy {
        TTy(self.push(TNode::TAny))
    }

    /// A union type `A | B` (HIR `Ty::Union`). Members are kept in the order
    /// given; hir-lang's `finish` puts them in canonical form (flattened,
    /// sorted, repeats dropped, a single member collapsed).
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::Prim;
    ///
    /// let mut t = lower_lang::TemplateBuilder::new("t");
    /// let x = t.binder("x");
    /// let p = t.bind(x);
    /// let (int, string) = (t.prim(Prim::I64), t.prim(Prim::Str));
    /// let ty = t.union_ty(&[int, string]);
    /// let s = t.let_(p, Some(ty), None);
    /// let root = t.block(&[s], None);
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn union_ty(&mut self, members: &[TTy]) -> TTy {
        let members = self.list(members.iter().map(|m| m.0));
        TTy(self.push(TNode::TUnion(members)))
    }

    /// An intersection type `A & B` (HIR `Ty::Intersection`), normalized like
    /// [`union_ty`](Self::union_ty).
    ///
    /// # Examples
    ///
    /// ```
    /// let mut t = lower_lang::TemplateBuilder::new("t");
    /// let x = t.binder("x");
    /// let p = t.bind(x);
    /// let (a, b) = (t.named_ty("Countable"), t.named_ty("Traversable"));
    /// let ty = t.intersection_ty(&[a, b]);
    /// let s = t.let_(p, Some(ty), None);
    /// let root = t.block(&[s], None);
    /// assert!(t.finish(root).is_ok());
    /// ```
    pub fn intersection_ty(&mut self, members: &[TTy]) -> TTy {
        let members = self.list(members.iter().map(|m| m.0));
        TTy(self.push(TNode::TInter(members)))
    }

    // ---------------------------------------------------------------- finish

    /// Checks the template rooted at `root` and returns it.
    ///
    /// # Errors
    ///
    /// A [`TemplateError`]: an unused hole, an unbound or twice-bound binder,
    /// an undefined or twice-defined label, an argument splice outside a list,
    /// a handle from another builder, or a tree larger than
    /// [`MAX_TEMPLATE_SIZE`].
    ///
    /// # Examples
    ///
    /// ```
    /// use lower_lang::{TemplateBuilder, TemplateError};
    ///
    /// let mut t = TemplateBuilder::new("bad");
    /// let x = t.binder("x");
    /// let root = t.var(x); // never bound
    /// assert_eq!(t.finish(root), Err(TemplateError::BinderUnbound { binder: 0 }));
    /// ```
    pub fn finish(mut self, root: TExpr) -> Result<Template, TemplateError> {
        if self.overflow {
            return Err(TemplateError::TooLarge);
        }
        self.t.root = root.0;
        let applies = check(&self.t)?;
        self.t.applies = applies;
        Ok(self.t)
    }
}

/// Use and binding counts gathered by walking a template's tree.
struct Counts {
    hole_uses: Vec<u32>,
    binder_sites: Vec<u32>,
    binder_uses: Vec<u32>,
    label_defs: Vec<u32>,
    label_uses: Vec<u32>,
}

/// Walks the template's tree from its root (iteratively, counting shared
/// sub-trees at every use) and checks it. Returns the `Apply` nodes reached.
fn check(t: &Template) -> Result<Vec<u32>, TemplateError> {
    let mut c = Counts {
        hole_uses: alloc::vec![0; t.holes.len()],
        binder_sites: alloc::vec![0; t.binders.len()],
        binder_uses: alloc::vec![0; t.binders.len()],
        label_defs: alloc::vec![0; t.labels.len()],
        label_uses: alloc::vec![0; t.labels.len()],
    };
    let mut applies = Vec::new();
    // (node, whether it sits directly in an argument/element list)
    let mut stack: Vec<(u32, bool)> = alloc::vec![(t.root, false)];
    let mut size: u32 = 0;
    while let Some((n, in_list)) = stack.pop() {
        if n == NONE {
            continue;
        }
        size += 1;
        if size > MAX_TEMPLATE_SIZE {
            return Err(TemplateError::TooLarge);
        }
        let node = *t.node(n).ok_or(TemplateError::ForeignHandle)?;
        let kids = |list: TList,
                    in_list: bool,
                    stack: &mut Vec<(u32, bool)>|
         -> Result<(), TemplateError> {
            let (start, len) = (list.0 as usize, list.1 as usize);
            let slice = t
                .lists
                .get(start..start.saturating_add(len))
                .ok_or(TemplateError::ForeignHandle)?;
            stack.extend(slice.iter().map(|&k| (k, in_list)));
            Ok(())
        };
        match node {
            TNode::Hole(h) | TNode::PHole(h) | TNode::THole(h) | TNode::Stmts(h) => {
                bump(&mut c.hole_uses, usize::from(h))?;
            }
            TNode::Splice(h) => {
                if !in_list {
                    return Err(TemplateError::SpliceOutOfList {
                        hole: usize::from(h),
                    });
                }
                bump(&mut c.hole_uses, usize::from(h))?;
            }
            TNode::Var(b) => bump(&mut c.binder_uses, usize::from(b))?,
            TNode::PBind(b) => bump(&mut c.binder_sites, usize::from(b))?,
            TNode::Lit(_)
            | TNode::Str(_)
            | TNode::Global(_)
            | TNode::Extern(_)
            | TNode::PWild
            | TNode::PLit(_)
            | TNode::TPrim(_)
            | TNode::TNamed(_)
            | TNode::TInfer
            | TNode::TAny => {}
            TNode::Call { callee, args } => {
                stack.push((callee, false));
                kids(args, true, &mut stack)?;
            }
            TNode::Method { recv, args, .. } => {
                stack.push((recv, false));
                kids(args, true, &mut stack)?;
            }
            TNode::Field { base, .. } => stack.push((base, false)),
            TNode::Index { base, index } => stack.extend([(base, false), (index, false)]),
            TNode::Op { args, .. } => kids(args, false, &mut stack)?,
            TNode::Assign { target, value } => stack.extend([(target, false), (value, false)]),
            TNode::Block { stmts, tail, label } => {
                define_label(&mut c, label)?;
                kids(stmts, false, &mut stack)?;
                stack.push((tail, false));
            }
            TNode::If { cond, then, else_ } => {
                stack.extend([(cond, false), (then, false), (else_, false)]);
            }
            TNode::Match { scrutinee, arms } => {
                stack.push((scrutinee, false));
                kids(arms, false, &mut stack)?;
            }
            TNode::Loop { label, body, step } => {
                define_label(&mut c, label)?;
                stack.extend([(body, false), (step, false)]);
            }
            TNode::Break { label, value } => {
                use_label(&mut c, label)?;
                stack.push((value, false));
            }
            TNode::Continue { label } => use_label(&mut c, label)?,
            TNode::Return(v)
            | TNode::Throw(v)
            | TNode::Await(v)
            | TNode::YieldFrom(v)
            | TNode::Spawn(v)
            | TNode::ExprStmt(v)
            | TNode::Defer(v) => stack.push((v, false)),
            TNode::Yield { key, value } => stack.extend([(key, false), (value, false)]),
            TNode::LetPlace {
                binder,
                place,
                body,
            } => {
                bump(&mut c.binder_sites, usize::from(binder))?;
                stack.extend([(place, false), (body, false)]);
            }
            TNode::TUnion(l) | TNode::TInter(l) => kids(l, false, &mut stack)?,
            TNode::Tuple(l) | TNode::Array(l) => kids(l, true, &mut stack)?,
            TNode::Closure { params, body, .. } => {
                let (start, len) = (params.0 as usize, params.1 as usize);
                let slice = t
                    .lists
                    .get(start..start.saturating_add(len))
                    .ok_or(TemplateError::ForeignHandle)?;
                for &p in slice {
                    bump(&mut c.binder_sites, p as usize)?;
                }
                stack.push((body, false));
            }
            TNode::Apply { args, .. } => {
                applies.push(n);
                let (start, len) = (args.0 as usize, args.1 as usize);
                let slice = t
                    .args
                    .get(start..start.saturating_add(len))
                    .ok_or(TemplateError::ForeignHandle)?;
                for arg in slice {
                    match *arg {
                        TArg::Expr(e) => stack.push((e.0, false)),
                        TArg::Pat(p) => stack.push((p.0, false)),
                        TArg::Ty(ty) => stack.push((ty.0, false)),
                        TArg::Stmts(h) => bump(&mut c.hole_uses, usize::from(h.0))?,
                        TArg::Args(h) => bump(&mut c.hole_uses, usize::from(h.0))?,
                        TArg::Label(l) => use_label(&mut c, LabelSlot::of(Some(l)))?,
                        TArg::Absent => {}
                    }
                }
            }
            TNode::Let { pat, ty, init } => {
                stack.extend([(pat, false), (ty, false), (init, false)])
            }
            TNode::PTuple(l) => kids(l, false, &mut stack)?,
            TNode::PCtor { elems, .. } => kids(elems, false, &mut stack)?,
        }
    }
    for (i, &n) in c.hole_uses.iter().enumerate() {
        if n == 0 {
            return Err(TemplateError::HoleUnused { hole: i });
        }
    }
    for (i, (&sites, &uses)) in c.binder_sites.iter().zip(&c.binder_uses).enumerate() {
        if sites > 1 {
            return Err(TemplateError::BinderBoundTwice { binder: i });
        }
        if sites == 0 && uses > 0 {
            return Err(TemplateError::BinderUnbound { binder: i });
        }
    }
    for (i, (&defs, &uses)) in c.label_defs.iter().zip(&c.label_uses).enumerate() {
        if defs > 1 {
            return Err(TemplateError::LabelDefinedTwice { label: i });
        }
        if defs == 0 && uses > 0 {
            return Err(TemplateError::LabelUndefined { label: i });
        }
    }
    Ok(applies)
}

fn bump(counts: &mut [u32], i: usize) -> Result<(), TemplateError> {
    let slot = counts.get_mut(i).ok_or(TemplateError::ForeignHandle)?;
    *slot = slot.saturating_add(1);
    Ok(())
}

fn define_label(c: &mut Counts, label: LabelSlot) -> Result<(), TemplateError> {
    match label {
        LabelSlot::None => Ok(()),
        LabelSlot::Own(l) => bump(&mut c.label_defs, usize::from(l)),
        LabelSlot::Hole(h) => bump(&mut c.hole_uses, usize::from(h)),
    }
}

fn use_label(c: &mut Counts, label: LabelSlot) -> Result<(), TemplateError> {
    match label {
        LabelSlot::None => Ok(()),
        LabelSlot::Own(l) => bump(&mut c.label_uses, usize::from(l)),
        LabelSlot::Hole(h) => bump(&mut c.hole_uses, usize::from(h)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shared_subtrees_count_at_every_use() {
        let mut t = TemplateBuilder::new("blowup");
        let mut e = t.lit(TLit::Int(1));
        for _ in 0..40 {
            e = t.tuple(&[e, e]);
        }
        assert_eq!(t.finish(e), Err(TemplateError::TooLarge));
    }

    #[test]
    fn test_foreign_handles_are_refused() {
        let mut t = TemplateBuilder::new("foreign");
        let root = TExpr(99);
        let _ = t.lit(TLit::Null);
        assert_eq!(t.finish(root), Err(TemplateError::ForeignHandle));
    }

    #[test]
    fn test_label_rules() {
        let mut t = TemplateBuilder::new("labels");
        let l = t.label("l");
        let brk = t.break_(Some(LabelRef::Own(l)), None);
        assert_eq!(
            t.finish(brk),
            Err(TemplateError::LabelUndefined { label: 0 })
        );

        let mut t = TemplateBuilder::new("labels");
        let l = t.label("l");
        let body = t.block(&[], None);
        let a = t.loop_(Some(LabelRef::Own(l)), body, None);
        let b = t.loop_(Some(LabelRef::Own(l)), body, None);
        let root = t.tuple(&[a, b]);
        assert_eq!(
            t.finish(root),
            Err(TemplateError::LabelDefinedTwice { label: 0 })
        );
    }

    #[test]
    fn test_splice_outside_a_list_is_refused() {
        let mut t = TemplateBuilder::new("splice");
        let args = t.args_hole("args");
        let s = t.splice(args);
        let root = t.op(OpKind::Not, &[s]);
        assert_eq!(
            t.finish(root),
            Err(TemplateError::SpliceOutOfList { hole: 0 })
        );
    }

    #[test]
    fn test_binder_bound_twice() {
        let mut t = TemplateBuilder::new("twice");
        let x = t.binder("x");
        let (p1, p2) = (t.bind(x), t.bind(x));
        let pair = t.tuple_pat(&[p1, p2]);
        let unit = t.tuple(&[]);
        let s = t.let_(pair, None, Some(unit));
        let root = t.block(&[s], None);
        assert_eq!(
            t.finish(root),
            Err(TemplateError::BinderBoundTwice { binder: 0 })
        );
    }
}
