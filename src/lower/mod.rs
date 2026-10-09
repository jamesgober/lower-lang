//! The lowering engine: an explicit-stack machine over the syntax tree.
//!
//! Lowering is recursive in nature (an expression contains expressions) and
//! hir-lang's builder is bottom-up (children before parents), so the engine
//! runs a work stack of tasks: *lower this element in this context* and
//! *finish the innermost open frame*. Entering a node resolves its fields'
//! picks, opens a frame, and schedules one task per selected child; every
//! task leaves exactly one value on the value stack, so when the frame
//! finishes its fields' values sit in order above the frame's base. Nothing
//! recurses on the call stack, so any tree depth is safe.

mod enter;
mod finish;
mod token;
mod tpl;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use hir_lang::{
    Arg, ArgKind, BinderId, BinderKind, Builder, Capture, Effects, Expr, ExprId, FieldInit,
    FieldPat, GenericArg, GenericParam, Hir, HirError, Item, ItemId, ItemKind, List, MapEntry,
    Name, NodeRef, Ns, Origin, Param, ParamKind, Pat, PatId, Path, PathId, Res, Segment, Site,
    Span, Stmt, StmtId, Ty, TyId, UnitId, VariantId, WherePred,
};
use intern_lang::{Interner, Symbol};
use syntax_lang::{Element, Node, TokenKind};

use crate::arch::Arch;
use crate::mapping::{EffectMode, Mapping, OpSpec, TokenRole};
use crate::pick::{self, Kid, Labeler, Pick};
use crate::problem::{Context, LowerError, Problem, ProblemKind};
use crate::template::TemplateId;

/// Limits on what one lowering may do. Untrusted trees and templates cannot
/// exceed them; exceeding one is a reported problem or error, never a hang.
///
/// `#[non_exhaustive]`: start from [`Budget::default`] and assign fields.
///
/// # Examples
///
/// ```
/// use lower_lang::Budget;
///
/// let mut budget = Budget::default();
/// budget.max_nodes = 1_000;
/// assert_eq!(budget.max_expansion_depth, 64);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Budget {
    /// The most HIR nodes (and list entries) one lowering may build, copies
    /// for repeated template holes included; past it lowering stops with
    /// [`LowerError::NodeBudget`]. Default `1 << 24` (16,777,216).
    pub max_nodes: u64,
    /// The deepest chain of template applications
    /// ([`TemplateBuilder::apply`](crate::TemplateBuilder::apply)); a deeper
    /// one becomes an error node and an
    /// [`ExpansionTooDeep`](ProblemKind::ExpansionTooDeep) problem.
    /// Default 64.
    pub max_expansion_depth: u32,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            max_nodes: 1 << 24,
            max_expansion_depth: 64,
        }
    }
}

/// The result of lowering: a valid HIR unit and every problem found.
///
/// The HIR is valid even when there are problems: each broken construct is an
/// HIR error form, and the rest of the unit is lowered normally.
///
/// # Examples
///
/// See [`lower`](crate::lower).
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Lowered {
    /// The validated HIR.
    pub hir: Hir,
    /// Every problem, in source order (span start, then discovery).
    pub problems: Vec<Problem>,
}

impl Lowered {
    /// Whether any problem was found.
    ///
    /// # Examples
    ///
    /// ```
    /// use lower_lang::{lower, Arch, MappingBuilder, Pick};
    /// use syntax_lang::Node;
    ///
    /// # #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
    /// # struct K(u8);
    /// # impl syntax_lang::TokenKind for K {}
    /// let mut b = MappingBuilder::new();
    /// b.node(K(0), Arch::Module { name: Pick::Absent, items: Pick::Nodes });
    /// let mapping = b.build()?;
    /// let tree = Node::new(K(0), vec![]);
    /// let mut names = intern_lang::Interner::new();
    /// let out = lower(&tree, "", &mapping, &mut names).expect("within budget");
    /// assert!(!out.has_problems());
    /// # Ok::<(), lower_lang::MappingError>(())
    /// ```
    #[must_use]
    pub fn has_problems(&self) -> bool {
        !self.problems.is_empty()
    }
}

/// Lowers syntax trees with one mapping: the configured path, for a budget,
/// a compilation unit other than 0, or host-provided field labels.
///
/// # Examples
///
/// ```
/// use hir_lang::UnitId;
/// use lower_lang::{Arch, Budget, Lowerer, MappingBuilder, Pick};
/// use syntax_lang::Node;
///
/// # #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
/// # struct K(u8);
/// # impl syntax_lang::TokenKind for K {}
/// let mut b = MappingBuilder::new();
/// b.node(K(0), Arch::Module { name: Pick::Absent, items: Pick::Nodes });
/// let mapping = b.build()?;
/// let tree = Node::new(K(0), vec![]);
/// let mut names = intern_lang::Interner::new();
/// let out = Lowerer::new(&mapping)
///     .with_unit(UnitId::new(3))
///     .with_budget(Budget::default())
///     .lower(&tree, "", &mut names)
///     .expect("within budget");
/// assert_eq!(out.hir.unit(), UnitId::new(3));
/// # Ok::<(), lower_lang::MappingError>(())
/// ```
pub struct Lowerer<'m, K> {
    mapping: &'m Mapping<K>,
    budget: Budget,
    unit: UnitId,
    labeler: Option<&'m dyn Labeler<K>>,
}

impl<'m, K> Lowerer<'m, K>
where
    K: Copy + Ord + TokenKind,
{
    /// A lowerer for `mapping`, with the default budget, unit 0, and no field
    /// labels.
    ///
    /// # Examples
    ///
    /// See [`Lowerer`].
    #[must_use]
    pub fn new(mapping: &'m Mapping<K>) -> Self {
        Self {
            mapping,
            budget: Budget::default(),
            unit: UnitId::default(),
            labeler: None,
        }
    }

    /// Sets the budget.
    ///
    /// # Examples
    ///
    /// See [`Lowerer`].
    #[must_use]
    pub fn with_budget(mut self, budget: Budget) -> Self {
        self.budget = budget;
        self
    }

    /// Sets the compilation unit the HIR is built for.
    ///
    /// # Examples
    ///
    /// See [`Lowerer`].
    #[must_use]
    pub fn with_unit(mut self, unit: UnitId) -> Self {
        self.unit = unit;
        self
    }

    /// Uses `labeler` for [`Pick::Label`](crate::Pick::Label) fields.
    ///
    /// # Examples
    ///
    /// ```
    /// use lower_lang::{Labeler, Lowerer, MappingBuilder};
    /// use syntax_lang::Node;
    ///
    /// struct NoFields;
    /// # #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
    /// # struct K(u8);
    /// # impl syntax_lang::TokenKind for K {}
    /// impl Labeler<K> for NoFields {
    ///     fn label(&self, _: &Node<K>, _: usize) -> Option<u16> { None }
    /// }
    /// let mapping = MappingBuilder::<K>::new().build()?;
    /// let _lowerer = Lowerer::new(&mapping).with_labeler(&NoFields);
    /// # Ok::<(), lower_lang::MappingError>(())
    /// ```
    #[must_use]
    pub fn with_labeler(mut self, labeler: &'m dyn Labeler<K>) -> Self {
        self.labeler = Some(labeler);
        self
    }

    /// Lowers `tree` (parsed from `source`) into a validated HIR unit, with
    /// names interned in `names`.
    ///
    /// # Errors
    ///
    /// Only for exhausted resources: [`LowerError::NodeBudget`],
    /// [`LowerError::SymbolSpaceExhausted`], or [`LowerError::Hir`] when
    /// hir-lang's arenas overflow. Problems in the code are never errors:
    /// they are in [`Lowered::problems`].
    ///
    /// # Examples
    ///
    /// See [`Lowerer`].
    pub fn lower(
        &self,
        tree: &Node<K>,
        source: &str,
        names: &mut Interner,
    ) -> Result<Lowered, LowerError> {
        Run::new(self, source, names)?.run(tree)
    }
}

// ------------------------------------------------------------------ machine

/// Where a pattern's binders are bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PatSite {
    /// A parameter (binders of kind `Param`) rather than a local.
    pub(crate) param: bool,
    /// The binders are mutable.
    pub(crate) mutable: bool,
    /// A `match` arm, where bare identifiers follow the language's
    /// [`IdentPatterns`](crate::IdentPatterns) policy.
    pub(crate) arm: bool,
    /// A range bound: bare identifiers are constants, not bindings.
    pub(crate) bound: bool,
}

impl PatSite {
    pub(crate) const LOCAL: Self = Self {
        param: false,
        mutable: false,
        arm: false,
        bound: false,
    };
    pub(crate) const ARM: Self = Self {
        param: false,
        mutable: false,
        arm: true,
        bound: false,
    };
}

/// The context an element is lowered in, and so the kind of value it leaves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Want {
    /// The root module.
    Root,
    /// An expression.
    Expr,
    /// An element of a block or module body: a statement, an expression, or
    /// an item.
    Stmt,
    /// A member of a class, interface, or impl: an item or a field.
    Member,
    /// A pattern.
    Pat(PatSite),
    /// A type.
    Ty,
    /// A path in a namespace.
    Path(Ns),
    /// A path segment.
    Segment,
    /// A parameter.
    Param,
    /// A call argument.
    Arg,
    /// A match arm or catch clause.
    Arm,
    /// An explicit capture.
    Capture,
    /// A field definition.
    Field,
    /// A sum variant.
    Variant,
    /// A generic parameter.
    Generic,
    /// A generic argument.
    GArg,
    /// A where-clause predicate.
    Where,
    /// A map-literal entry.
    Entry,
    /// A record-literal field initializer.
    Init,
    /// A record-pattern field.
    FieldPat(PatSite),
}

impl Want {
    pub(crate) const fn context(self) -> Context {
        match self {
            Self::Root | Self::Stmt => Context::Statement,
            Self::Expr => Context::Expression,
            Self::Member => Context::Member,
            Self::Pat(_) | Self::FieldPat(_) => Context::Pattern,
            Self::Ty => Context::Type,
            Self::Path(_) => Context::Path,
            Self::Segment => Context::Segment,
            Self::Param => Context::Parameter,
            Self::Arg => Context::Argument,
            Self::Arm => Context::Arm,
            Self::Capture => Context::Capture,
            Self::Field => Context::Field,
            Self::Variant => Context::Variant,
            Self::Generic | Self::GArg | Self::Where => Context::Generic,
            Self::Entry | Self::Init => Context::Entry,
        }
    }

    /// Whether an expression's value adapts to this context.
    pub(crate) const fn takes_expr(self) -> bool {
        matches!(
            self,
            Self::Expr | Self::Stmt | Self::Arg | Self::Entry | Self::GArg
        )
    }
}

/// The value one lowered element leaves on the value stack.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Val {
    /// Nothing: an absent element, or a list element that failed and was
    /// dropped (its problem already reported).
    Err,
    Expr(ExprId),
    Stmt(StmtId),
    Item(ItemId),
    /// Several values from one element (a `Splice`, `global $a, $b`): a run of
    /// the spill stack. Spill entries are never themselves spills.
    Spill(u32, u32),
    Pat(PatId),
    /// A `..` marker in a pattern list, with its optional binding.
    Rest(Option<PatId>),
    Ty(TyId),
    Path(PathId),
    Seg(Segment),
    /// A parameter, created as a node by the enclosing function so that list
    /// markers can still adjust its kind.
    Param(Param, Span),
    /// A `*` or `/` marker in a parameter list.
    Marker(ParamKind),
    Arg(Arg),
    Arm(hir_lang::Arm),
    Cap(Capture),
    /// A field definition, and whether it has a name.
    Field(hir_lang::FieldId, bool),
    Variant(VariantId),
    Generic(GenericParam),
    GArg(GenericArg),
    Where(WherePred),
    Entry(MapEntry),
    Init(FieldInit),
    FieldPat(FieldPat),
}

/// A value on the value stack, with the span of the element it came from and
/// whether it is a place (for place arguments).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Slot {
    pub(crate) v: Val,
    pub(crate) span: Span,
    pub(crate) place: bool,
}

/// A unit of work.
#[derive(Clone, Copy)]
pub(crate) enum Task<'t, K> {
    /// Lower this element in this context.
    Elem(&'t Element<K>, Want),
    /// Lower this node in this context (the root).
    Node(&'t Node<K>, Want),
    /// Finish the innermost open frame.
    Finish,
}

/// What a frame builds when it finishes.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Build {
    /// The frame's archetype, in its context.
    Arch,
    /// An operator node lowered through an operator spec.
    Op(OpSpec),
    /// A template instantiation.
    Template {
        id: TemplateId,
        label: Option<BinderId>,
        nodes_at: u64,
    },
    /// A synthetic module around a root that is not a module.
    RootWrap,
    /// `var x = init` hoisted: an assignment to the hoisted name.
    HoistedLet { name: Symbol, name_span: Span },
}

/// An open construct whose fields are being lowered.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Frame<K> {
    pub(crate) arch: Arch<K>,
    pub(crate) build: Build,
    pub(crate) want: Want,
    pub(crate) span: Span,
    pub(crate) vals_base: usize,
    pub(crate) counts_base: usize,
    pub(crate) nfields: usize,
    pub(crate) spill_base: usize,
    pub(crate) label_mark: usize,
    pub(crate) mods: u32,
    pub(crate) fx: bool,
    pub(crate) or: bool,
    /// Names read synchronously at entry (declaration names, aliases,
    /// members, labels).
    pub(crate) name: Option<(Symbol, Span)>,
    /// A binder created or found at entry (a label, a pattern's binder).
    pub(crate) binder: Option<BinderId>,
    /// A tuple-field index (`a.0`).
    pub(crate) index: Option<u32>,
    /// An archetype-specific flag: a member access is a call, a generic
    /// parameter is a region.
    pub(crate) flag: bool,
    /// A path's root, read from its leading tokens.
    pub(crate) path_root: hir_lang::PathRoot,
}

/// A function, closure, or module body under construction: its effects and
/// its hoisted declarations.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Fx {
    pub(crate) declared: Effects,
    pub(crate) inferred: Effects,
    pub(crate) hoist_base: usize,
    pub(crate) decl_base: usize,
}

/// One lowering run.
pub(crate) struct Run<'a, 't, K> {
    pub(crate) m: &'a Mapping<K>,
    pub(crate) src: &'t str,
    pub(crate) labeler: Option<&'a dyn Labeler<K>>,
    pub(crate) names: &'a mut Interner,
    pub(crate) b: Builder,
    pub(crate) nodes: u64,
    pub(crate) budget: Budget,
    pub(crate) syms: Vec<Symbol>,
    pub(crate) empty: Symbol,
    pub(crate) sym_failed: bool,
    pub(crate) tasks: Vec<Task<'t, K>>,
    pub(crate) vals: Vec<Slot>,
    pub(crate) frames: Vec<Frame<K>>,
    pub(crate) counts: Vec<usize>,
    pub(crate) spill: Vec<Slot>,
    pub(crate) kids: Vec<Kid<'t, K>>,
    pub(crate) kids2: Vec<Kid<'t, K>>,
    pub(crate) sel: Vec<&'t Element<K>>,
    pub(crate) labels: BTreeMap<Symbol, Vec<BinderId>>,
    pub(crate) label_log: Vec<Symbol>,
    pub(crate) fx: Vec<Fx>,
    pub(crate) hoist: Vec<(Symbol, Span)>,
    pub(crate) declared: Vec<Symbol>,
    pub(crate) or_depth: u32,
    pub(crate) or_map: BTreeMap<Symbol, BinderId>,
    pub(crate) problems: Vec<Problem>,
    pub(crate) text: Vec<u8>,
    pub(crate) scratch: String,
    pub(crate) bufs: Vec<Vec<Slot>>,
    pub(crate) tpl_scratch: tpl::TplScratch,
    pub(crate) lists: Lists,
    pub(crate) tfields: Vec<(Pick<K>, Want, bool)>,
}

/// Typed scratch lists for building HIR lists without allocating per node.
#[derive(Default)]
pub(crate) struct Lists {
    pub(crate) exprs: Vec<ExprId>,
    pub(crate) tys: Vec<TyId>,
    pub(crate) args: Vec<Arg>,
    pub(crate) arms: Vec<hir_lang::Arm>,
    pub(crate) stmts: Vec<StmtId>,
    pub(crate) params: Vec<hir_lang::ParamId>,
    pub(crate) gargs: Vec<GenericArg>,
}

impl<'a, 't, K> Run<'a, 't, K>
where
    K: Copy + Ord + TokenKind,
{
    fn new(
        cfg: &Lowerer<'a, K>,
        src: &'t str,
        names: &'a mut Interner,
    ) -> Result<Self, LowerError> {
        let empty = names
            .try_intern("")
            .map_err(|_| LowerError::SymbolSpaceExhausted)?;
        let mut syms = Vec::with_capacity(cfg.mapping.strings.len());
        for s in &cfg.mapping.strings {
            syms.push(
                names
                    .try_intern(s)
                    .map_err(|_| LowerError::SymbolSpaceExhausted)?,
            );
        }
        Ok(Self {
            m: cfg.mapping,
            src,
            labeler: cfg.labeler,
            names,
            b: Builder::for_unit(cfg.unit),
            nodes: 0,
            budget: cfg.budget,
            syms,
            empty,
            sym_failed: false,
            tasks: Vec::new(),
            vals: Vec::new(),
            frames: Vec::new(),
            counts: Vec::new(),
            spill: Vec::new(),
            kids: Vec::new(),
            kids2: Vec::new(),
            sel: Vec::new(),
            labels: BTreeMap::new(),
            label_log: Vec::new(),
            fx: Vec::new(),
            hoist: Vec::new(),
            declared: Vec::new(),
            or_depth: 0,
            or_map: BTreeMap::new(),
            problems: Vec::new(),
            text: Vec::new(),
            scratch: String::new(),
            bufs: Vec::new(),
            tpl_scratch: tpl::TplScratch::default(),
            lists: Lists::default(),
            tfields: Vec::new(),
        })
    }

    fn run(mut self, tree: &'t Node<K>) -> Result<Lowered, LowerError> {
        self.tasks.push(Task::Node(tree, Want::Root));
        while let Some(task) = self.tasks.pop() {
            match task {
                Task::Elem(Element::Node(n), want) => self.lower_node(n, want),
                Task::Elem(Element::Token(t), want) => self.lower_token(t, want),
                Task::Node(n, want) => self.lower_node(n, want),
                Task::Finish => self.finish(),
            }
            if self.nodes > self.budget.max_nodes {
                return Err(LowerError::NodeBudget {
                    limit: self.budget.max_nodes,
                });
            }
            if self.sym_failed {
                return Err(LowerError::SymbolSpaceExhausted);
            }
        }
        let root = match self.vals.pop().map(|s| s.v) {
            Some(Val::Item(root)) => root,
            _ => {
                self.b.set_origin(Origin::new(tree.span()));
                self.b.module(None, &[])
            }
        };
        let (hir, errors) = self.b.finish_lenient(root).map_err(LowerError::Hir)?;
        let mut problems = self.problems;
        problems.reserve(errors.len());
        for e in errors {
            let span = hir_span(&hir, &e);
            problems.push(Problem {
                kind: ProblemKind::Hir(e),
                span,
            });
        }
        problems.sort_by_key(|p| p.span.start());
        Ok(Lowered { hir, problems })
    }

    // ------------------------------------------------------------- dispatch

    fn lower_node(&mut self, node: &'t Node<K>, want: Want) {
        let span = node.span();
        if self.m.is_error(node.kind()) {
            self.report(ProblemKind::SyntaxError, span);
            let v = self.error_val(want, span);
            self.push(v, span);
            return;
        }
        pick::collect_kids(node, self.labeler, &mut self.kids);
        let (start, end) = self.m.rule_range(node.kind());
        let mut chosen = None;
        for &(_, rule) in self.m.by_kind.get(start..end).unwrap_or(&[]) {
            let Some(rule) = self.m.rules.get(rule as usize) else {
                continue;
            };
            if rule.when.is_absent() || pick::single(rule.when, &self.kids, self.m).is_some() {
                chosen = Some(rule.arch);
                break;
            }
        }
        match chosen {
            Some(arch) => self.enter(node, arch, want),
            None => {
                self.report(ProblemKind::Unmapped, span);
                let v = self.error_val(want, span);
                self.push(v, span);
            }
        }
    }

    /// Leaves a value for the element at `span`.
    pub(crate) fn push(&mut self, v: Val, span: Span) {
        self.vals.push(Slot {
            v,
            span,
            place: false,
        });
    }

    /// Leaves a value that may be a place.
    pub(crate) fn push_place(&mut self, v: Val, span: Span, place: bool) {
        self.vals.push(Slot { v, span, place });
    }

    // -------------------------------------------------------------- frames

    /// Opens a frame for `node` and schedules its fields: each `(pick, want,
    /// many)` is resolved against the node's children (already in
    /// `self.kids`), and one task per selected child is pushed.
    pub(crate) fn plan(&mut self, mut frame: Frame<K>, fields: &[(Pick<K>, Want, bool)]) {
        let counts_base = self.counts.len();
        self.sel.clear();
        for &(p, _, many) in fields {
            let before = self.sel.len();
            if many {
                pick::list(p, &self.kids, self.m, &mut self.sel);
            } else if let Some(e) = pick::single(p, &self.kids, self.m) {
                self.sel.push(e);
            }
            self.counts.push(self.sel.len() - before);
        }
        self.drop_shared_children(fields, counts_base);
        frame.counts_base = counts_base;
        frame.nfields = fields.len();
        frame.vals_base = self.vals.len();
        frame.spill_base = self.spill.len();
        self.frames.push(frame);
        self.tasks.push(Task::Finish);
        let mut end = self.sel.len();
        for (i, &(_, want, _)) in fields.iter().enumerate().rev() {
            let n = self.counts.get(counts_base + i).copied().unwrap_or(0);
            let start = end.saturating_sub(n);
            if let Some(slice) = self.sel.get(start..end) {
                for &e in slice.iter().rev() {
                    self.tasks.push(Task::Elem(e, want));
                }
            }
            end = start;
        }
        self.sel.clear();
    }

    /// A child selected by an earlier field is not lowered again for a later
    /// single-valued field: that field is left absent and the conflict is
    /// reported. Positional picks alias this way in half-typed code (a
    /// missing condition makes "the first node" the body).
    fn drop_shared_children(&mut self, fields: &[(Pick<K>, Want, bool)], counts_base: usize) {
        let mut offset = 0;
        for (i, &(_, _, many)) in fields.iter().enumerate() {
            let c = self.counts.get(counts_base + i).copied().unwrap_or(0);
            if !many && c == 1 {
                if let Some(&e) = self.sel.get(offset) {
                    let earlier = self.sel.get(..offset).unwrap_or(&[]);
                    if earlier.iter().any(|&x| core::ptr::eq(x, e)) {
                        let _ = self.sel.remove(offset);
                        if let Some(slot) = self.counts.get_mut(counts_base + i) {
                            *slot = 0;
                        }
                        self.report(ProblemKind::AmbiguousChild, e.span());
                        continue;
                    }
                }
            }
            offset += c;
        }
    }

    /// A frame for `node` with nothing scheduled yet.
    pub(crate) fn frame(&self, arch: Arch<K>, want: Want, span: Span) -> Frame<K> {
        Frame {
            arch,
            build: Build::Arch,
            want,
            span,
            vals_base: 0,
            counts_base: 0,
            nfields: 0,
            spill_base: 0,
            label_mark: self.label_log.len(),
            mods: 0,
            fx: false,
            or: false,
            name: None,
            binder: None,
            index: None,
            flag: false,
            path_root: hir_lang::PathRoot::Relative,
        }
    }

    /// The value range of field `i` of `f`.
    pub(crate) fn field_range(&self, f: &Frame<K>, i: usize) -> (usize, usize) {
        let counts = self
            .counts
            .get(f.counts_base..f.counts_base + f.nfields)
            .unwrap_or(&[]);
        let start = f.vals_base + counts.iter().take(i).sum::<usize>();
        let len = counts.get(i).copied().unwrap_or(0);
        (start, start + len)
    }

    /// The first value of field `i` (or `Val::Err` when absent).
    pub(crate) fn one(&self, f: &Frame<K>, i: usize) -> Val {
        self.slot(f, i).map_or(Val::Err, |s| s.v)
    }

    /// The first slot of field `i`.
    pub(crate) fn slot(&self, f: &Frame<K>, i: usize) -> Option<Slot> {
        let (s, e) = self.field_range(f, i);
        if s < e {
            self.vals.get(s).copied()
        } else {
            None
        }
    }

    /// Whether field `i` selected an element at all (even one that failed).
    pub(crate) fn present(&self, f: &Frame<K>, i: usize) -> bool {
        let (s, e) = self.field_range(f, i);
        s < e
    }

    /// The values of field `i`, spills flattened, into a pooled buffer.
    pub(crate) fn many(&mut self, f: &Frame<K>, i: usize) -> Vec<Slot> {
        let mut out = self.bufs.pop().unwrap_or_default();
        out.clear();
        let (s, e) = self.field_range(f, i);
        for j in s..e {
            match self.vals.get(j).copied() {
                Some(Slot {
                    v: Val::Spill(ss, sl),
                    ..
                }) => {
                    let (ss, sl) = (ss as usize, sl as usize);
                    if let Some(run) = self.spill.get(ss..ss.saturating_add(sl)) {
                        out.extend_from_slice(run);
                    }
                }
                Some(Slot { v: Val::Err, .. }) | None => {}
                Some(slot) => out.push(slot),
            }
        }
        out
    }

    /// Returns a buffer from [`many`](Self::many) to the pool.
    pub(crate) fn give(&mut self, mut buf: Vec<Slot>) {
        buf.clear();
        self.bufs.push(buf);
    }

    // ------------------------------------------------------------- problems

    pub(crate) fn report(&mut self, kind: ProblemKind, span: Span) {
        self.problems.push(Problem { kind, span });
    }

    /// Reports a missing required field and returns an error expression.
    pub(crate) fn missing_expr(&mut self, field: &'static str, span: Span) -> ExprId {
        self.report(ProblemKind::MissingField { field }, span);
        self.err_expr(span)
    }

    // ------------------------------------------------------- node creation

    pub(crate) fn at(&mut self, span: Span) {
        self.b.set_origin(Origin::new(span));
    }

    pub(crate) fn ex(&mut self, e: Expr) -> ExprId {
        self.nodes += 1;
        self.b.expr(e)
    }

    pub(crate) fn st(&mut self, s: Stmt) -> StmtId {
        self.nodes += 1;
        self.b.stmt(s)
    }

    pub(crate) fn pt(&mut self, p: Pat) -> PatId {
        self.nodes += 1;
        self.b.pat(p)
    }

    pub(crate) fn ty(&mut self, t: Ty) -> TyId {
        self.nodes += 1;
        self.b.ty(t)
    }

    pub(crate) fn it(&mut self, i: Item) -> ItemId {
        self.nodes += 1;
        self.b.item(i)
    }

    pub(crate) fn path(&mut self, p: Path) -> PathId {
        self.nodes += 1;
        self.b.path(p)
    }

    pub(crate) fn binder(&mut self, name: Name, kind: BinderKind, mutable: bool) -> BinderId {
        self.nodes += 1;
        self.b
            .binder(hir_lang::Binder::new(name, kind).with_mutable(mutable))
    }

    pub(crate) fn list<T: hir_lang::Pooled>(&mut self, items: &[T]) -> List<T> {
        self.nodes += items.len() as u64;
        self.b.list(items)
    }

    pub(crate) fn err_expr(&mut self, span: Span) -> ExprId {
        self.at(span);
        self.ex(Expr::Err)
    }

    /// A one-segment unresolved path to a source name.
    pub(crate) fn name_path(&mut self, sym: Symbol, span: Span, ns: Ns) -> PathId {
        self.at(span);
        let seg = Segment::new(Name::new(sym), Origin::new(span));
        let segs = self.list(&[seg]);
        self.path(Path::new(segs, ns))
    }

    pub(crate) fn name_expr(&mut self, sym: Symbol, span: Span) -> ExprId {
        let p = self.name_path(sym, span, Ns::Value);
        self.ex(Expr::Path(p))
    }

    /// The error value for a context: an error node where the context keeps
    /// positions (expressions, statements, patterns, parameters, arguments),
    /// nothing where a list can simply lose the element.
    pub(crate) fn error_val(&mut self, want: Want, span: Span) -> Val {
        self.at(span);
        match want {
            Want::Root => {
                // The root must stay a module for hir-lang to accept the unit.
                self.nodes += 1;
                Val::Item(self.b.module(None, &[]))
            }
            Want::Member => Val::Item(self.it(Item::new(None, ItemKind::Err))),
            Want::Expr => Val::Expr(self.ex(Expr::Err)),
            Want::Stmt => Val::Stmt(self.st(Stmt::Err)),
            Want::Pat(_) => Val::Pat(self.pt(Pat::Err)),
            Want::Ty => Val::Ty(self.ty(Ty::Err)),
            Want::Path(_) => {
                let mut p = Path::new(List::EMPTY, Ns::Value);
                p.res = Res::Err;
                Val::Path(self.path(p))
            }
            Want::Param => {
                let pat = self.pt(Pat::Err);
                Val::Param(Param::new(pat), span)
            }
            Want::Arg => {
                let e = self.ex(Expr::Err);
                Val::Arg(Arg::positional(e))
            }
            Want::Entry => {
                let e = self.ex(Expr::Err);
                Val::Entry(MapEntry {
                    key: None,
                    value: e,
                })
            }
            Want::GArg => {
                let t = self.ty(Ty::Err);
                Val::GArg(GenericArg::Ty(t))
            }
            Want::Segment
            | Want::Arm
            | Want::Capture
            | Want::Field
            | Want::Variant
            | Want::Generic
            | Want::Where
            | Want::Init
            | Want::FieldPat(_) => Val::Err,
        }
    }

    /// Reports that a construct cannot appear in `want` and leaves its error
    /// value.
    pub(crate) fn not_here(&mut self, want: Want, span: Span) {
        self.report(
            ProblemKind::NotAllowedHere {
                expected: want.context(),
            },
            span,
        );
        let v = self.error_val(want, span);
        self.push(v, span);
    }

    /// Converts an expression to the value a context takes.
    pub(crate) fn adapt_expr(&mut self, e: ExprId, place: bool, want: Want, span: Span) -> Val {
        match want {
            Want::Expr | Want::Stmt => Val::Expr(e),
            Want::Arg => Val::Arg(Arg {
                kind: ArgKind::Positional,
                value: e,
                place: place && self.m.policy.place_args == crate::PlaceArgs::Places,
            }),
            Want::Entry => Val::Entry(MapEntry {
                key: None,
                value: e,
            }),
            Want::GArg => Val::GArg(GenericArg::Const(e)),
            _ => {
                // Unreachable by construction (`enter` checks the context
                // first); keep the expression attached by wrapping it in an
                // error-free statement where possible.
                self.report(
                    ProblemKind::NotAllowedHere {
                        expected: want.context(),
                    },
                    span,
                );
                self.error_val(want, span)
            }
        }
    }

    // ---------------------------------------------------------------- text

    /// The source text of `span`, or `""` (and a problem) when the span lies
    /// outside the source.
    pub(crate) fn text_of(&mut self, span: Span) -> &'t str {
        let (s, e) = (span.start().to_usize(), span.end().to_usize());
        match self.src.get(s..e) {
            Some(t) => t,
            None => {
                self.report(ProblemKind::BadSpan, span);
                ""
            }
        }
    }

    pub(crate) fn intern(&mut self, s: &str) -> Symbol {
        match self.names.try_intern(s) {
            Ok(sym) => sym,
            Err(_) => {
                self.sym_failed = true;
                self.empty
            }
        }
    }

    /// The mapping string `i` (texts of templates, extern names) as a symbol.
    pub(crate) fn sym(&self, i: u32) -> Symbol {
        self.syms.get(i as usize).copied().unwrap_or(self.empty)
    }

    /// The name an element spells: a name-like token (sigil stripped), or a
    /// `Pass` wrapper around one. Other tokens count by their text.
    pub(crate) fn name_of(&mut self, elem: &'t Element<K>) -> Option<(Symbol, Span)> {
        let mut cur = elem;
        loop {
            match cur {
                Element::Token(t) => {
                    let span = t.span();
                    let strip = match self.m.role_for(t.kind()) {
                        Some(TokenRole::Name) | None => 0,
                        Some(
                            TokenRole::Var { sigil }
                            | TokenRole::Label { sigil }
                            | TokenRole::Lifetime { sigil },
                        ) => usize::from(sigil),
                        Some(_) => return None,
                    };
                    let text = self.text_of(span);
                    let name = text.get(strip..).unwrap_or("");
                    return Some((self.intern(name), span));
                }
                Element::Node(n) => {
                    pick::collect_kids(n, self.labeler, &mut self.kids2);
                    let arch = self.m.rule_for(n.kind()).copied();
                    let inner = match arch {
                        Some(Arch::Pass { inner } | Arch::Segment { name: inner, .. }) => {
                            pick::single(inner, &self.kids2, self.m)
                        }
                        Some(Arch::Path { segments, .. }) => {
                            let mut out = Vec::new();
                            pick::list(segments, &self.kids2, self.m, &mut out);
                            if out.len() == 1 {
                                out.first().copied()
                            } else {
                                None
                            }
                        }
                        _ => None,
                    };
                    cur = inner?;
                }
            }
        }
    }

    /// The modifiers among the node's direct significant children (and the
    /// tokens directly inside container children).
    pub(crate) fn mods(&self) -> u32 {
        if self.m.modifiers.is_empty() {
            return 0;
        }
        let mut bits = 0;
        for kid in &self.kids {
            match kid.elem {
                Element::Token(t) => bits |= crate::mapping::modifier_bits(self.m, t.kind()),
                Element::Node(n) => {
                    if crate::pick::KindSets::is_container(self.m, n.kind()) {
                        for c in n.child_tokens() {
                            bits |= crate::mapping::modifier_bits(self.m, c.kind());
                        }
                    }
                }
            }
        }
        bits
    }

    // --------------------------------------------------------------- scopes

    pub(crate) fn push_label(&mut self, sym: Symbol, b: BinderId) {
        self.labels.entry(sym).or_default().push(b);
        self.label_log.push(sym);
    }

    pub(crate) fn pop_labels(&mut self, mark: usize) {
        while self.label_log.len() > mark {
            if let Some(sym) = self.label_log.pop() {
                if let Some(stack) = self.labels.get_mut(&sym) {
                    let _ = stack.pop();
                }
            }
        }
    }

    pub(crate) fn find_label(&self, sym: Symbol) -> Option<BinderId> {
        self.labels.get(&sym).and_then(|s| s.last().copied())
    }

    pub(crate) fn push_fx(&mut self, declared: Effects) {
        self.fx.push(Fx {
            declared,
            inferred: Effects::NONE,
            hoist_base: self.hoist.len(),
            decl_base: self.declared.len(),
        });
    }

    /// Marks the innermost function-like frame as doing `effect`.
    pub(crate) fn infer(&mut self, effect: Effects) {
        if let Some(top) = self.fx.last_mut() {
            top.inferred = top.inferred.union(effect);
        }
    }

    pub(crate) fn hoist_name(&mut self, sym: Symbol, span: Span) {
        if !self.fx.is_empty() {
            self.hoist.push((sym, span));
        }
    }

    pub(crate) fn declare_name(&mut self, sym: Symbol) {
        if !self.fx.is_empty() {
            self.declared.push(sym);
        }
    }

    /// Closes the innermost function-like frame: its effects, and the `let`
    /// statements for its hoisted names (deduplicated, minus the names a
    /// parameter, `global`, or `static` declares), in first-use order.
    pub(crate) fn pop_fx(&mut self, span: Span) -> (Effects, Vec<StmtId>) {
        let Some(fx) = self.fx.pop() else {
            return (Effects::NONE, Vec::new());
        };
        let policy = self.m.policy;
        let mut effects = fx.declared;
        for (mode, flag) in [
            (policy.throws, Effects::THROWS),
            (policy.yields, Effects::YIELD),
            (policy.asyncs, Effects::ASYNC),
        ] {
            let add = match mode {
                EffectMode::Declared => false,
                EffectMode::Inferred => fx.inferred.contains(flag),
                EffectMode::Always => true,
            };
            if add {
                effects = effects.union(flag);
            }
        }
        let mut stmts = Vec::new();
        if self.hoist.len() > fx.hoist_base {
            let mut names: Vec<(Symbol, Span)> = self.hoist.split_off(fx.hoist_base);
            let mut declared: Vec<Symbol> = self.declared.split_off(fx.decl_base);
            declared.sort();
            names.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.start().cmp(&b.1.start())));
            names.dedup_by(|later, first| later.0 == first.0);
            names.retain(|(s, _)| declared.binary_search(s).is_err());
            names.sort_by_key(|(_, sp)| sp.start());
            for (sym, sp) in names {
                self.at(sp);
                let b = self.binder(Name::new(sym), BinderKind::Local, true);
                let pat = self.pt(Pat::Bind {
                    binder: b,
                    mode: hir_lang::BindMode::Value,
                    sub: None,
                });
                stmts.push(self.st(Stmt::Let {
                    pat,
                    ty: None,
                    init: None,
                    else_: None,
                }));
            }
            self.at(span);
        } else {
            self.hoist.truncate(fx.hoist_base);
            self.declared.truncate(fx.decl_base);
        }
        (effects, stmts)
    }

    /// `body`, or `{ hoisted lets; body }` when there are hoisted names.
    pub(crate) fn wrap_hoisted(&mut self, hoisted: &[StmtId], body: ExprId, span: Span) -> ExprId {
        if hoisted.is_empty() {
            return body;
        }
        self.at(span);
        let stmts = self.list(hoisted);
        self.ex(Expr::Block(hir_lang::Block {
            stmts,
            tail: Some(body),
            label: None,
            is_unsafe: false,
        }))
    }
}

/// The span a HIR validation problem is about: its node's origin, or its
/// binder's or expansion's.
fn hir_span(hir: &Hir, e: &HirError) -> Span {
    match e.node() {
        Some(n) => hir.origin(n).span,
        None => match e {
            HirError::BinderBoundTwice { binder, .. }
            | HirError::BinderKind { binder, .. }
            | HirError::DuplicateBinding { binder, .. } => hir.binder_origin(*binder).span,
            HirError::Dangling {
                site: Site::Binder(b),
                ..
            } => hir.binder_origin(*b).span,
            HirError::Dangling {
                site: Site::Expansion(x),
                ..
            }
            | HirError::ExpansionOrder { expn: x } => {
                hir.expansion(*x).map_or(Span::empty(0), |x| x.call_site)
            }
            _ => hir.origin(NodeRef::Item(hir.root())).span,
        },
    }
}
