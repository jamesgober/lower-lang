//! Finishing a frame: build the HIR form from the fields' values, then adapt
//! it to the context the node was lowered in.

use alloc::vec::Vec;

use hir_lang::{
    Arg, ArgKind, Arm, BindMode, BinderKind, Block, BorrowKind, Bound, Closure, DefaultEval, Expr,
    ExprId, FieldDef, FieldInit, FieldPat, FnDef, GenericArg, GenericParam, Generics, Ident,
    ImplDef, InterfaceDef, Item, ItemKind, List, MapEntry, Member, MixinUseDef, Name, Ns, Op,
    OpKind, Origin, Overflow, Param, ParamKind, Pat, PatId, Path, PathId, PathRoot, Policy, QSelf,
    RecordDef, Res, Segment, Shape, SliceRest, Span, Stmt, StmtId, SumDef, Ty, TyId, Variant, Vis,
    WherePred,
};
use intern_lang::Symbol;
use syntax_lang::TokenKind;

use super::{Build, Frame, Piece, Run, Slot, Val, Want};
use crate::arch::{Arch, RecordShape};
use crate::mapping::{Join, Modifier, OpSpec};
use crate::problem::ProblemKind;
use crate::template::{HoleKind, TemplateId};

const fn has(mods: u32, m: Modifier) -> bool {
    mods & m.bit() != 0
}

/// Visibility from modifiers: the most public one wins.
const fn vis_of(mods: u32) -> Vis {
    if has(mods, Modifier::Public) {
        Vis::Public
    } else if has(mods, Modifier::Protected) {
        Vis::Protected
    } else if has(mods, Modifier::Package) {
        Vis::Package
    } else {
        Vis::Private
    }
}

impl<K> Run<'_, '_, K>
where
    K: Copy + Ord + TokenKind,
{
    pub(crate) fn finish(&mut self) {
        let Some(f) = self.frames.pop() else {
            return;
        };
        self.at(f.span);
        let (mut v, place) = match f.build {
            Build::Arch => self.finish_arch(&f),
            Build::Op(spec) => self.finish_op(&f, spec),
            Build::Template {
                id,
                label,
                nodes_at,
            } => (self.finish_template(&f, id, label, nodes_at), false),
            Build::RootWrap => (self.finish_module(&f, None), false),
            Build::HoistedLet { name, name_span } => {
                (self.finish_hoisted(&f, name, name_span), false)
            }
        };
        if f.attrs.1 > 0 {
            self.attach_attrs(&mut v, f.attrs, f.span);
        }
        if f.or {
            self.or_depth = self.or_depth.saturating_sub(1);
        }
        self.pop_labels(f.label_mark);
        self.vals.truncate(f.vals_base);
        self.counts.truncate(f.counts_base);
        let v = self.settle(v, f.spill_base);
        self.push_place(v, f.span, place);
    }

    /// Moves a spilled result down to the frame's spill base, dropping the
    /// spill entries of its children.
    fn settle(&mut self, v: Val, base: usize) -> Val {
        match v {
            Val::Spill(s, l) => {
                let (s, l) = (s as usize, l as usize);
                if s >= base && s.saturating_add(l) <= self.spill.len() {
                    self.spill.copy_within(s..s + l, base);
                    self.spill.truncate(base + l);
                    Val::Spill(
                        u32::try_from(base).unwrap_or(0),
                        u32::try_from(l).unwrap_or(0),
                    )
                } else {
                    self.spill.truncate(base);
                    Val::Spill(0, 0)
                }
            }
            other => {
                self.spill.truncate(base);
                other
            }
        }
    }

    /// Appends `slots` to the spill stack and returns the spill value.
    pub(crate) fn spill_of(&mut self, slots: &[Slot]) -> Val {
        let start = self.spill.len();
        self.spill.extend_from_slice(slots);
        Val::Spill(
            u32::try_from(start).unwrap_or(0),
            u32::try_from(slots.len()).unwrap_or(0),
        )
    }

    // ------------------------------------------------------------ accessors

    pub(crate) fn opt_e(&self, f: &Frame<K>, i: usize) -> Option<ExprId> {
        match self.one(f, i) {
            Val::Expr(e) => Some(e),
            _ => None,
        }
    }

    /// A required expression: present, or an error expression (with a
    /// problem only when the field was missing, not when it failed).
    pub(crate) fn req_e(&mut self, f: &Frame<K>, i: usize, field: &'static str) -> ExprId {
        match self.one(f, i) {
            Val::Expr(e) => e,
            _ if self.present(f, i) => self.err_expr(f.span),
            _ => self.missing_expr(field, f.span),
        }
    }

    fn opt_p(&self, f: &Frame<K>, i: usize) -> Option<PatId> {
        match self.one(f, i) {
            Val::Pat(p) => Some(p),
            _ => None,
        }
    }

    fn req_p(&mut self, f: &Frame<K>, i: usize, field: &'static str) -> PatId {
        match self.one(f, i) {
            Val::Pat(p) => p,
            _ => {
                if !self.present(f, i) {
                    self.report(ProblemKind::MissingField { field }, f.span);
                }
                self.at(f.span);
                self.pt(Pat::Err)
            }
        }
    }

    fn opt_t(&self, f: &Frame<K>, i: usize) -> Option<TyId> {
        match self.one(f, i) {
            Val::Ty(t) => Some(t),
            _ => None,
        }
    }

    fn req_t(&mut self, f: &Frame<K>, i: usize, field: &'static str) -> TyId {
        match self.one(f, i) {
            Val::Ty(t) => t,
            _ => {
                if !self.present(f, i) {
                    self.report(ProblemKind::MissingField { field }, f.span);
                }
                self.at(f.span);
                self.ty(Ty::Err)
            }
        }
    }

    fn opt_path(&self, f: &Frame<K>, i: usize) -> Option<PathId> {
        match self.one(f, i) {
            Val::Path(p) => Some(p),
            _ => None,
        }
    }

    fn req_path(&mut self, f: &Frame<K>, i: usize, field: &'static str) -> PathId {
        match self.one(f, i) {
            Val::Path(p) => p,
            _ => {
                if !self.present(f, i) {
                    self.report(ProblemKind::MissingField { field }, f.span);
                }
                self.at(f.span);
                let mut p = Path::new(List::EMPTY, Ns::Value);
                p.res = Res::Err;
                self.path(p)
            }
        }
    }

    /// Generic arguments of field `i` as bounds.
    fn bounds(&mut self, f: &Frame<K>, i: usize) -> List<Bound> {
        let vals = self.many(f, i);
        let out: Vec<Bound> = vals
            .iter()
            .filter_map(|s| match s.v {
                Val::GArg(g) => Self::bound_of(g),
                _ => None,
            })
            .collect();
        self.give(vals);
        self.list(&out)
    }

    fn generics(&mut self, f: &Frame<K>, params: usize, preds: Option<usize>) -> Generics {
        let vals = self.many(f, params);
        let ps: Vec<GenericParam> = vals
            .iter()
            .filter_map(|s| match s.v {
                Val::Generic(g) => Some(g),
                _ => None,
            })
            .collect();
        self.give(vals);
        let params = self.list(&ps);
        let preds = match preds {
            Some(i) => {
                let vals = self.many(f, i);
                let ws: Vec<WherePred> = vals
                    .iter()
                    .filter_map(|s| match s.v {
                        Val::Where(w) => Some(w),
                        _ => None,
                    })
                    .collect();
                self.give(vals);
                self.list(&ws)
            }
            None => List::EMPTY,
        };
        Generics { params, preds }
    }

    fn exprs(&mut self, f: &Frame<K>, i: usize) -> List<ExprId> {
        let vals = self.many(f, i);
        let mut out = core::mem::take(&mut self.lists.exprs);
        out.clear();
        out.extend(vals.iter().filter_map(|s| match s.v {
            Val::Expr(x) => Some(x),
            _ => None,
        }));
        self.give(vals);
        let list = self.list(&out);
        self.lists.exprs = out;
        list
    }

    fn tys(&mut self, f: &Frame<K>, i: usize) -> List<TyId> {
        let vals = self.many(f, i);
        let mut out = core::mem::take(&mut self.lists.tys);
        out.clear();
        out.extend(vals.iter().filter_map(|s| match s.v {
            Val::Ty(x) => Some(x),
            _ => None,
        }));
        self.give(vals);
        let list = self.list(&out);
        self.lists.tys = out;
        list
    }

    fn args(&mut self, f: &Frame<K>, i: usize) -> List<Arg> {
        let vals = self.many(f, i);
        let mut out = core::mem::take(&mut self.lists.args);
        out.clear();
        out.extend(vals.iter().filter_map(|s| match s.v {
            Val::Arg(x) => Some(x),
            _ => None,
        }));
        self.give(vals);
        let list = self.list(&out);
        self.lists.args = out;
        list
    }

    fn arms(&mut self, f: &Frame<K>, i: usize) -> List<Arm> {
        let vals = self.many(f, i);
        let mut out = core::mem::take(&mut self.lists.arms);
        out.clear();
        out.extend(vals.iter().filter_map(|s| match s.v {
            Val::Arm(x) => Some(x),
            _ => None,
        }));
        self.give(vals);
        let list = self.list(&out);
        self.lists.arms = out;
        list
    }

    fn gargs(&mut self, f: &Frame<K>, i: usize) -> List<GenericArg> {
        let vals = self.many(f, i);
        let mut out = core::mem::take(&mut self.lists.gargs);
        out.clear();
        out.extend(vals.iter().filter_map(|s| match s.v {
            Val::GArg(x) => Some(x),
            _ => None,
        }));
        self.give(vals);
        let list = self.list(&out);
        self.lists.gargs = out;
        list
    }

    /// Field definitions of field `i`, and whether every one is named.
    fn field_defs(&mut self, f: &Frame<K>, i: usize) -> (List<hir_lang::FieldId>, bool, bool) {
        let vals = self.many(f, i);
        let mut any_named = false;
        let mut all_named = true;
        let mut out = Vec::new();
        for s in vals.iter() {
            if let Val::Field(id, named) = s.v {
                out.push(id);
                any_named |= named;
                all_named &= named;
            }
        }
        self.give(vals);
        (self.list(&out), any_named, all_named)
    }

    /// Patterns of field `i` with `..` markers: the patterns, the position of
    /// the first `..`, and its binding.
    fn pats_with_rest(
        &mut self,
        f: &Frame<K>,
        i: usize,
    ) -> (Vec<PatId>, Option<u32>, Option<PatId>) {
        let vals = self.many(f, i);
        let mut pats = Vec::new();
        let mut rest = None;
        let mut bind = None;
        for s in vals.iter() {
            match s.v {
                Val::Pat(p) => pats.push(p),
                Val::Rest(b) => {
                    if rest.is_none() {
                        rest = u32::try_from(pats.len()).ok();
                        bind = b;
                    } else {
                        self.report(
                            ProblemKind::NotAllowedHere {
                                expected: crate::Context::Pattern,
                            },
                            s.span,
                        );
                    }
                }
                _ => {}
            }
        }
        self.give(vals);
        (pats, rest, bind)
    }

    /// Statements from a block's or module body's values: expressions become
    /// expression statements, items become item statements.
    fn stmt_of(&mut self, slot: Slot) -> Option<StmtId> {
        match slot.v {
            Val::Stmt(s) => Some(s),
            Val::Expr(e) => {
                self.at(slot.span);
                Some(self.st(Stmt::Expr(e)))
            }
            Val::Item(i) => {
                self.at(slot.span);
                Some(self.st(Stmt::Item(i)))
            }
            _ => None,
        }
    }

    /// Creates parameter nodes, applying `*` (named-only after) and `/`
    /// (positional-only before) markers.
    fn params(&mut self, f: &Frame<K>, i: usize) -> List<hir_lang::ParamId> {
        let vals = self.many(f, i);
        let positional_end = vals
            .iter()
            .rposition(|s| matches!(s.v, Val::Marker(ParamKind::PositionalOnly)));
        let mut named = false;
        let mut out = core::mem::take(&mut self.lists.params);
        out.clear();
        for (j, s) in vals.iter().enumerate() {
            match s.v {
                Val::Marker(ParamKind::NamedOnly) => named = true,
                Val::Param(mut p, span, attrs) => {
                    if p.kind == ParamKind::Normal {
                        if named {
                            p.kind = ParamKind::NamedOnly;
                        } else if positional_end.is_some_and(|end| j < end) {
                            p.kind = ParamKind::PositionalOnly;
                        }
                    }
                    self.at(span);
                    self.nodes += 1;
                    let id = self.b.param(p);
                    if attrs.1 > 0 {
                        let (a, l) = (attrs.0 as usize, attrs.1 as usize);
                        let list: Vec<hir_lang::Attr> = self
                            .attr_pool
                            .get(a..a.saturating_add(l))
                            .unwrap_or(&[])
                            .to_vec();
                        self.b.attach(hir_lang::NodeRef::Param(id), &list);
                    }
                    out.push(id);
                }
                _ => {}
            }
        }
        self.give(vals);
        self.at(f.span);
        let list = self.list(&out);
        self.lists.params = out;
        list
    }

    fn named_item(&mut self, f: &Frame<K>, kind: ItemKind) -> Val {
        let (sym, span) = f.name.unwrap_or((self.empty, f.span));
        self.at(f.span);
        let item = Item::new(Some(Name::new(sym)), kind)
            .with_vis(vis_of(f.mods))
            .with_name_span(span);
        Val::Item(self.it(item))
    }

    /// The op with the language's policies in exactly the fields it consults.
    pub(crate) fn policy_op(&self, kind: OpKind) -> Op {
        let d = kind.default_policy();
        let p = self.m.policy;
        let mut policy = Policy::NONE;
        if d.overflow.is_some() {
            policy.overflow = Some(p.overflow);
        }
        if d.div_zero.is_some() {
            policy.div_zero = Some(p.div_zero);
        }
        if d.shift.is_some() {
            policy.shift = Some(p.shift);
        }
        if d.float_to_int.is_some() {
            policy.float_to_int = Some(p.float_to_int);
        }
        Op { kind, policy }
    }

    /// A pattern in its context.
    pub(crate) fn adapt_pat(&mut self, p: PatId, want: Want, span: Span) -> Val {
        match want {
            Want::Pat(_) => Val::Pat(p),
            Want::Param => Val::Param(Param::new(p), span, (0, 0)),
            _ => {
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

    /// A type in its context.
    fn adapt_ty(&mut self, t: TyId, want: Want, span: Span) -> Val {
        match want {
            Want::Ty => Val::Ty(t),
            Want::GArg => Val::GArg(GenericArg::Ty(t)),
            _ => {
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

    /// An expression in its context (`place` marks place forms).
    fn done_expr(&mut self, f: &Frame<K>, e: Expr, place: bool) -> (Val, bool) {
        self.at(f.span);
        let id = self.ex(e);
        (self.adapt_expr(id, place, f.want, f.span), place)
    }

    // ----------------------------------------------------------- archetypes

    #[allow(clippy::too_many_lines)]
    fn finish_arch(&mut self, f: &Frame<K>) -> (Val, bool) {
        let want = f.want;
        let span = f.span;
        let v = match f.arch {
            Arch::Module { .. } => self.finish_module(f, f.name),
            Arch::Splice { .. } => {
                let vals = self.many(f, 0);
                let v = self.spill_of(&vals);
                self.give(vals);
                v
            }
            Arch::Let { .. } => {
                let pat = self.req_p(f, 0, "pat");
                let ty = self.opt_t(f, 1);
                let init = self.opt_e(f, 2);
                let else_ = self.opt_e(f, 3);
                self.at(span);
                let stmt = self.st(Stmt::Let {
                    pat,
                    ty,
                    init,
                    else_,
                });
                if f.aux.1 == 1 {
                    // A hoisted `let` of a destructuring pattern: the pattern
                    // bound temporaries; assign them to the hoisted names.
                    let mark = (f.aux.0 as usize).min(self.persist.len());
                    let binds = self.persist.split_off(mark);
                    let mut stmts = alloc::vec![stmt];
                    self.persist_assigns(&binds, &mut stmts);
                    let slots: Vec<Slot> = stmts
                        .into_iter()
                        .map(|s| Slot {
                            v: Val::Stmt(s),
                            span,
                            place: false,
                        })
                        .collect();
                    self.spill_of(&slots)
                } else {
                    Val::Stmt(stmt)
                }
            }
            Arch::ExprStmt { .. } => {
                let e = self.req_e(f, 0, "expr");
                self.at(span);
                Val::Stmt(self.st(Stmt::Expr(e)))
            }
            Arch::Defer { .. } => {
                let e = self.req_e(f, 0, "body");
                self.at(span);
                Val::Stmt(self.st(Stmt::Defer(e)))
            }
            Arch::Static { .. } => {
                let ty = self.opt_t(f, 0);
                let init = self.opt_e(f, 1);
                let (sym, nspan) = f.name.unwrap_or((self.empty, span));
                self.at(nspan);
                let binder = self.binder(Name::new(sym), BinderKind::Local, true);
                self.at(span);
                Val::Stmt(self.st(Stmt::Static { binder, ty, init }))
            }
            Arch::Import { .. } => {
                let path = self.req_path(f, 0, "path");
                let glob = has(f.mods, Modifier::Glob);
                self.at(span);
                let mut item = Item::new(
                    f.name.map(|(s, _)| Name::new(s)),
                    ItemKind::Import { path, glob },
                )
                .with_vis(vis_of(f.mods));
                if let Some((_, nspan)) = f.name {
                    item = item.with_name_span(nspan);
                }
                Val::Item(self.it(item))
            }
            Arch::Fn { .. } => {
                let (effects, hoisted) = self.pop_fx(span);
                let generics = self.generics(f, 0, Some(1));
                let params = self.params(f, 2);
                let ret = self.opt_t(f, 3);
                let throws = self.opt_t(f, 4);
                let body = self
                    .opt_e(f, 5)
                    .map(|b| self.wrap_hoisted(&hoisted, b, span));
                let def = FnDef {
                    generics,
                    params,
                    ret,
                    effects,
                    throws,
                    abi: None,
                    body,
                    defaults: DefaultEval::PerCall,
                };
                self.named_item(f, ItemKind::Fn(def))
            }
            Arch::Record { shape, .. } => {
                let generics = self.generics(f, 0, None);
                let (fields, any, all) = self.field_defs(f, 1);
                let shape = decide_shape(shape, fields.len(), any, all);
                let is_union = has(f.mods, Modifier::Union);
                self.named_item(
                    f,
                    ItemKind::Record(RecordDef {
                        generics,
                        shape,
                        fields,
                        is_union,
                    }),
                )
            }
            Arch::Sum { .. } => {
                let generics = self.generics(f, 0, None);
                let vals = self.many(f, 1);
                let vs: Vec<hir_lang::VariantId> = vals
                    .iter()
                    .filter_map(|s| match s.v {
                        Val::Variant(v) => Some(v),
                        _ => None,
                    })
                    .collect();
                self.give(vals);
                let variants = self.list(&vs);
                self.named_item(f, ItemKind::Sum(SumDef { generics, variants }))
            }
            Arch::Interface { .. } => {
                let generics = self.generics(f, 0, None);
                let supers = self.tys(f, 1);
                let items = self.member_items(f, 2).0;
                self.named_item(
                    f,
                    ItemKind::Interface(InterfaceDef {
                        generics,
                        supers,
                        items,
                    }),
                )
            }
            Arch::Class { mixin, .. } => {
                let generics = self.generics(f, 0, None);
                let bases = self.tys(f, 1);
                let interfaces = self.tys(f, 2);
                let (items, fields) = self.member_items(f, 3);
                self.named_item(
                    f,
                    ItemKind::Class(hir_lang::ClassDef {
                        generics,
                        bases,
                        interfaces,
                        fields,
                        items,
                        is_abstract: has(f.mods, Modifier::Abstract),
                        is_final: has(f.mods, Modifier::Final),
                        mixin,
                    }),
                )
            }
            Arch::Impl { .. } => {
                let generics = self.generics(f, 0, None);
                let interface = self.opt_t(f, 1);
                let self_ty = self.req_t(f, 2, "self_ty");
                let items = self.member_items(f, 3).0;
                self.at(span);
                let def = ImplDef {
                    generics,
                    interface,
                    self_ty,
                    items,
                };
                Val::Item(self.it(Item::new(None, ItemKind::Impl(def))))
            }
            Arch::Alias { .. } => {
                let generics = self.generics(f, 0, None);
                let ty = self.req_t(f, 1, "ty");
                self.named_item(f, ItemKind::Alias { generics, ty })
            }
            Arch::AssocType { .. } => {
                let bounds = self.bounds(f, 0);
                let default = self.opt_t(f, 1);
                self.named_item(f, ItemKind::AssocType { bounds, default })
            }
            Arch::Const { .. } => {
                let ty = self.opt_t(f, 0);
                let value = self.opt_e(f, 1);
                self.named_item(f, ItemKind::Const { ty, value })
            }
            Arch::GlobalVar { mutable, .. } => {
                let ty = self.opt_t(f, 0);
                let init = self.opt_e(f, 1);
                let mutable = mutable || has(f.mods, Modifier::Mut);
                self.named_item(f, ItemKind::Global { ty, mutable, init })
            }
            Arch::MixinUse { .. } => {
                let mixins = self.tys(f, 0);
                let vals = self.many(f, 1);
                let rules: Vec<hir_lang::MixinRule> = vals
                    .iter()
                    .filter_map(|s| match s.v {
                        Val::Rule(r) => Some(r),
                        _ => None,
                    })
                    .collect();
                self.give(vals);
                let rules = self.list(&rules);
                self.at(span);
                let def = MixinUseDef { mixins, rules };
                Val::Item(self.it(Item::new(None, ItemKind::MixinUse(def))))
            }
            Arch::MixinRule { .. } => {
                let from = self.opt_t(f, 0);
                let others = self.tys(f, 1);
                let (msym, mspan) = f.name.unwrap_or((self.empty, span));
                let method = Ident::new(msym, mspan);
                let vis_given = has(f.mods, Modifier::Public)
                    || has(f.mods, Modifier::Protected)
                    || has(f.mods, Modifier::Private)
                    || has(f.mods, Modifier::Package);
                let action = if !others.is_empty() {
                    Some(hir_lang::MixinAction::Insteadof(others))
                } else if f.alias.is_some() || vis_given {
                    Some(hir_lang::MixinAction::Alias {
                        name: f.alias.map(|(s, sp)| Ident::new(s, sp)),
                        vis: vis_given.then(|| vis_of(f.mods)),
                    })
                } else {
                    None
                };
                match action {
                    Some(action) => Val::Rule(hir_lang::MixinRule {
                        method,
                        from,
                        action,
                    }),
                    None => {
                        self.report(ProblemKind::MissingField { field: "action" }, span);
                        Val::Err
                    }
                }
            }
            Arch::Interp { join, .. } => return self.finish_interp(f, join),
            Arch::FieldDecl { .. } => {
                let ty = self.opt_t(f, 0);
                let default = self.opt_e(f, 1);
                if want == Want::Member && has(f.mods, Modifier::Static) {
                    if f.name.is_none() {
                        self.report(ProblemKind::MissingField { field: "name" }, span);
                    }
                    self.named_item(
                        f,
                        ItemKind::Global {
                            ty,
                            mutable: true,
                            init: default,
                        },
                    )
                } else {
                    self.at(span);
                    self.nodes += 1;
                    let id = self.b.field(FieldDef {
                        name: f.name.map(|(s, sp)| Ident::new(s, sp)),
                        vis: vis_of(f.mods),
                        ty,
                        default,
                    });
                    Val::Field(id, f.name.is_some())
                }
            }
            Arch::VariantDecl { shape, .. } => {
                let (fields, any, all) = self.field_defs(f, 0);
                let discriminant = self.opt_e(f, 1);
                let shape = match shape {
                    RecordShape::Auto if fields.is_empty() => Shape::Unit,
                    s => decide_shape(s, fields.len(), any, all),
                };
                let (sym, nspan) = f.name.unwrap_or((self.empty, span));
                self.at(span);
                self.nodes += 1;
                Val::Variant(self.b.variant(Variant {
                    name: Ident::new(sym, nspan),
                    shape,
                    fields,
                    discriminant,
                }))
            }
            Arch::GenericParam { .. } => {
                let bounds = self.bounds(f, 0);
                let ty = self.opt_t(f, 1);
                let default = match self.one(f, 2) {
                    Val::GArg(g) => Some(g),
                    _ => None,
                };
                let kind = if f.flag {
                    BinderKind::Region
                } else if ty.is_some() {
                    BinderKind::ConstParam
                } else {
                    BinderKind::TypeParam
                };
                let (sym, nspan) = f.name.unwrap_or((self.empty, span));
                self.at(nspan);
                let binder = self.binder(Name::new(sym), kind, false);
                Val::Generic(GenericParam {
                    binder,
                    bounds,
                    ty,
                    default,
                })
            }
            Arch::WherePred { .. } => {
                let subject = match self.one(f, 0) {
                    Val::GArg(g) => Self::bound_of(g),
                    _ => None,
                };
                let subject = match subject {
                    Some(s) => s,
                    None => {
                        if !self.present(f, 0) {
                            self.report(ProblemKind::MissingField { field: "subject" }, span);
                        }
                        self.at(span);
                        Bound::Ty(self.ty(Ty::Err))
                    }
                };
                let bounds = self.bounds(f, 1);
                Val::Where(WherePred { subject, bounds })
            }
            Arch::Param { .. } => {
                let pat = self.req_p(f, 0, "pat");
                let ty = self.opt_t(f, 1);
                let default = self.opt_e(f, 2);
                let kind = if has(f.mods, Modifier::Receiver) {
                    ParamKind::Receiver
                } else if has(f.mods, Modifier::RestNamed) {
                    ParamKind::RestNamed
                } else if has(f.mods, Modifier::Rest) {
                    ParamKind::Rest
                } else if has(f.mods, Modifier::NamedOnly) {
                    ParamKind::NamedOnly
                } else if has(f.mods, Modifier::PositionalOnly) {
                    ParamKind::PositionalOnly
                } else {
                    ParamKind::Normal
                };
                Val::Param(
                    Param {
                        pat,
                        ty,
                        default,
                        kind,
                        by_ref: has(f.mods, Modifier::Ref),
                    },
                    span,
                    (0, 0),
                )
            }
            Arch::Path { .. } => return self.finish_path(f),
            Arch::Segment { .. } => {
                let args = self.gargs(f, 0);
                let (sym, nspan) = f.name.unwrap_or((self.empty, span));
                let seg = Segment {
                    name: Name::new(sym),
                    args,
                    origin: Origin::new(nspan),
                };
                if want == Want::Segment {
                    Val::Seg(seg)
                } else {
                    return self.path_val(f, PathRoot::Relative, None, &[], &[seg]);
                }
            }
            Arch::AssocBinding { .. } => {
                let ty = self.req_t(f, 0, "ty");
                let (sym, nspan) = f.name.unwrap_or((self.empty, span));
                Val::GArg(GenericArg::Binding {
                    name: Ident::new(sym, nspan),
                    ty,
                })
            }
            Arch::Call { .. } => {
                if let Want::Pat(_) | Want::Param = want {
                    let path = self.req_path(f, 0, "callee");
                    let (pats, rest, _) = self.pats_with_rest(f, 1);
                    let elems = self.list(&pats);
                    self.at(span);
                    let p = self.pt(Pat::Ctor { path, elems, rest });
                    self.adapt_pat(p, want, span)
                } else {
                    let callee = self.req_e(f, 0, "callee");
                    let args = self.args(f, 1);
                    return self.done_expr(f, Expr::Call { callee, args }, false);
                }
            }
            Arch::Arg { .. } => {
                let value = self.req_e(f, 0, "value");
                let place_hint = self.slot(f, 0).is_some_and(|s| s.place);
                let kind = if has(f.mods, Modifier::SpreadNamed) {
                    ArgKind::SpreadNamed
                } else if has(f.mods, Modifier::Spread) {
                    ArgKind::Spread
                } else if let Some((sym, nspan)) = f.name {
                    ArgKind::Named(Ident::new(sym, nspan))
                } else {
                    ArgKind::Positional
                };
                let spread = matches!(kind, ArgKind::Spread | ArgKind::SpreadNamed);
                let place = !spread
                    && (has(f.mods, Modifier::Ref)
                        || (place_hint && self.m.policy.place_args == crate::PlaceArgs::Places));
                Val::Arg(Arg { kind, value, place })
            }
            Arch::Member { .. } => {
                let base = self.req_e(f, 0, "base");
                let (sym, nspan) = f.name.unwrap_or((self.empty, span));
                if f.flag {
                    let args = self.args(f, 1);
                    return self.done_expr(
                        f,
                        Expr::MethodCall {
                            receiver: base,
                            method: Ident::new(sym, nspan),
                            generic_args: List::EMPTY,
                            args,
                        },
                        false,
                    );
                }
                let member = match f.index {
                    Some(i) => Member::Index(i),
                    None => Member::Named(sym),
                };
                return self.done_expr(
                    f,
                    Expr::Field {
                        base,
                        member,
                        span: nspan,
                    },
                    true,
                );
            }
            Arch::MethodCall { .. } => {
                let receiver = self.req_e(f, 0, "receiver");
                let generic_args = self.gargs(f, 1);
                let args = self.args(f, 2);
                let (sym, nspan) = f.name.unwrap_or((self.empty, span));
                return self.done_expr(
                    f,
                    Expr::MethodCall {
                        receiver,
                        method: Ident::new(sym, nspan),
                        generic_args,
                        args,
                    },
                    false,
                );
            }
            Arch::DynMember { .. } => {
                let base = self.req_e(f, 0, "base");
                let name = self.req_e(f, 1, "name");
                if f.flag {
                    let args = self.args(f, 2);
                    return self.done_expr(
                        f,
                        Expr::DynMethodCall {
                            receiver: base,
                            name,
                            args,
                        },
                        false,
                    );
                }
                return self.done_expr(f, Expr::DynField { base, name }, true);
            }
            Arch::Index { .. } => {
                let base = self.req_e(f, 0, "base");
                let index = self.req_e(f, 1, "index");
                return self.done_expr(f, Expr::Index { base, index }, true);
            }
            Arch::VarVar { .. } => {
                let e = self.req_e(f, 0, "inner");
                return self.done_expr(f, Expr::VarVar(e), true);
            }
            Arch::Append { .. } => {
                let e = self.req_e(f, 0, "base");
                return self.done_expr(f, Expr::Append(e), true);
            }
            Arch::Tuple { .. } => match want {
                Want::Pat(_) | Want::Param => {
                    let (pats, rest, _) = self.pats_with_rest(f, 0);
                    let elems = self.list(&pats);
                    self.at(span);
                    let p = self.pt(Pat::Tuple { elems, rest });
                    self.adapt_pat(p, want, span)
                }
                Want::Ty | Want::GArg => {
                    let elems = self.tys(f, 0);
                    self.at(span);
                    let t = self.ty(Ty::Tuple(elems));
                    self.adapt_ty(t, want, span)
                }
                _ => {
                    let elems = self.exprs(f, 0);
                    return self.done_expr(f, Expr::Tuple(elems), false);
                }
            },
            Arch::Array { .. } => match want {
                Want::Pat(_) | Want::Param => {
                    let p = self.slice_pat(f, 0);
                    self.adapt_pat(p, want, span)
                }
                Want::Ty | Want::GArg => {
                    let vals = self.many(f, 0);
                    let tys: Vec<TyId> = vals
                        .iter()
                        .filter_map(|s| match s.v {
                            Val::Ty(t) => Some(t),
                            _ => None,
                        })
                        .collect();
                    self.give(vals);
                    let t = match tys.as_slice() {
                        [t] => Ty::Slice(*t),
                        _ => {
                            self.report(
                                ProblemKind::NotAllowedHere {
                                    expected: crate::Context::Type,
                                },
                                span,
                            );
                            Ty::Err
                        }
                    };
                    self.at(span);
                    let t = self.ty(t);
                    self.adapt_ty(t, want, span)
                }
                _ => {
                    let elems = self.exprs(f, 0);
                    return self.done_expr(f, Expr::Array(elems), false);
                }
            },
            Arch::Map { .. } => {
                let vals = self.many(f, 0);
                let out: Vec<MapEntry> = vals
                    .iter()
                    .filter_map(|s| match s.v {
                        Val::Entry(e) => Some(e),
                        _ => None,
                    })
                    .collect();
                self.give(vals);
                let entries = self.list(&out);
                return self.done_expr(f, Expr::Map(entries), false);
            }
            Arch::MapEntry { .. } => {
                let key = self.opt_e(f, 0);
                let value = self.req_e(f, 1, "value");
                Val::Entry(MapEntry { key, value })
            }
            Arch::RecordLit { .. } => {
                let path = self.opt_path(f, 0);
                if let Want::Pat(_) | Want::Param = want {
                    let (fields, rest) = self.field_pats(f, 1);
                    self.at(span);
                    let p = self.pt(Pat::Record { path, fields, rest });
                    self.adapt_pat(p, want, span)
                } else {
                    let vals = self.many(f, 1);
                    let out: Vec<FieldInit> = vals
                        .iter()
                        .filter_map(|s| match s.v {
                            Val::Init(i) => Some(i),
                            _ => None,
                        })
                        .collect();
                    self.give(vals);
                    let fields = self.list(&out);
                    let base = self.opt_e(f, 2);
                    return self.done_expr(f, Expr::Record { path, fields, base }, false);
                }
            }
            Arch::FieldInit { .. } => {
                let (sym, nspan) = f.name.unwrap_or((self.empty, span));
                match want {
                    Want::FieldPat(site) => {
                        let pat = match self.opt_p(f, 0) {
                            Some(p) => p,
                            None => self.bind_pat(sym, nspan, site, BindMode::Value),
                        };
                        Val::FieldPat(FieldPat {
                            name: Ident::new(sym, nspan),
                            pat,
                        })
                    }
                    _ => {
                        let value = match self.opt_e(f, 0) {
                            Some(e) => e,
                            None => self.name_expr(sym, nspan),
                        };
                        Val::Init(FieldInit {
                            name: Ident::new(sym, nspan),
                            value,
                        })
                    }
                }
            }
            Arch::Repeat { .. } => {
                let elem = self.req_e(f, 0, "elem");
                let count = self.req_e(f, 1, "count");
                return self.done_expr(f, Expr::Repeat { elem, count }, false);
            }
            Arch::Cast { .. } => {
                let expr = self.req_e(f, 0, "expr");
                let ty = self.req_t(f, 1, "ty");
                let p = self.m.policy;
                // `promote` needs a dynamically typed result (HIR §12); a
                // cast's target is static, so promoting languages trap.
                let overflow = match p.overflow {
                    Overflow::Promote => Overflow::Error,
                    o => o,
                };
                let policy = Policy::CAST
                    .with_overflow(overflow)
                    .with_float_to_int(p.float_to_int);
                return self.done_expr(f, Expr::Cast { expr, ty, policy }, false);
            }
            Arch::Borrow { .. } => {
                let expr = self.req_e(f, 0, "expr");
                let kind = if has(f.mods, Modifier::Mut) {
                    BorrowKind::Mut
                } else {
                    BorrowKind::Shared
                };
                return self.done_expr(f, Expr::Borrow { kind, expr }, false);
            }
            Arch::Deref { .. } => {
                let e = self.req_e(f, 0, "inner");
                return self.done_expr(f, Expr::Deref(e), true);
            }
            Arch::Block { tail_from_last, .. } => {
                let block = self.block_of(f, 0, Some(1), tail_from_last);
                return self.done_expr(f, Expr::Block(block), false);
            }
            Arch::If { .. } => {
                let cond = self.req_e(f, 0, "cond");
                let then = self.req_e(f, 1, "then");
                let else_ = self.opt_e(f, 2);
                return self.done_expr(f, Expr::If { cond, then, else_ }, false);
            }
            Arch::Match { .. } => {
                let scrutinee = self.req_e(f, 0, "scrutinee");
                let arms = self.arms(f, 1);
                return self.done_expr(f, Expr::Match { scrutinee, arms }, false);
            }
            Arch::Arm { .. } => {
                let pat = self.req_p(f, 0, "pat");
                let guard = self.opt_e(f, 1);
                let body = self.req_e(f, 2, "body");
                Val::Arm(Arm { pat, guard, body })
            }
            Arch::Loop { .. } => {
                let body = self.req_e(f, 0, "body");
                return self.done_expr(
                    f,
                    Expr::Loop {
                        label: f.binder,
                        body,
                        step: None,
                    },
                    false,
                );
            }
            Arch::Break { .. } => {
                let value = self.opt_e(f, 0);
                return self.done_expr(
                    f,
                    Expr::Break {
                        label: f.binder,
                        value,
                    },
                    false,
                );
            }
            Arch::Continue { .. } => {
                return self.done_expr(f, Expr::Continue { label: f.binder }, false);
            }
            Arch::Return { .. } => {
                let value = self.opt_e(f, 0);
                return self.done_expr(f, Expr::Return(value), false);
            }
            Arch::Throw { .. } => {
                let value = self.req_e(f, 0, "value");
                return self.done_expr(f, Expr::Throw(value), false);
            }
            Arch::Try { .. } => {
                let body = self.req_e(f, 0, "body");
                let catches = self.arms(f, 1);
                let finally = self.opt_e(f, 2);
                return self.done_expr(
                    f,
                    Expr::Try {
                        body,
                        catches,
                        finally,
                    },
                    false,
                );
            }
            Arch::Catch { .. } => self.finish_catch(f),
            Arch::Await { .. } => {
                let e = self.req_e(f, 0, "inner");
                return self.done_expr(f, Expr::Await(e), false);
            }
            Arch::Yield { .. } => {
                let value = self.opt_e(f, 0);
                return self.done_expr(f, Expr::Yield(value), false);
            }
            Arch::Spawn { .. } => {
                let e = self.req_e(f, 0, "inner");
                return self.done_expr(f, Expr::Spawn(e), false);
            }
            Arch::Closure { implicit, .. } => {
                let (effects, hoisted) = self.pop_fx(span);
                let vals = self.many(f, 0);
                let caps: Vec<hir_lang::Capture> = vals
                    .iter()
                    .filter_map(|s| match s.v {
                        Val::Cap(c) => Some(c),
                        _ => None,
                    })
                    .collect();
                self.give(vals);
                let captures = self.list(&caps);
                let params = self.params(f, 1);
                let ret = self.opt_t(f, 2);
                let body = self.req_e(f, 3, "body");
                let body = self.wrap_hoisted(&hoisted, body, span);
                return self.done_expr(
                    f,
                    Expr::Closure(Closure {
                        params,
                        ret,
                        body,
                        effects,
                        implicit,
                        captures,
                        self_binder: None,
                        defaults: DefaultEval::PerCall,
                    }),
                    false,
                );
            }
            Arch::RestPat { .. } => Val::Rest(self.opt_p(f, 0)),
            Arch::BindPat { .. } => {
                // `rest @ ..` binds the rest of a slice pattern.
                if let (Val::Rest(None), Some(binder), Want::Pat(_)) =
                    (self.one(f, 0), f.binder, want)
                {
                    self.at(span);
                    let p = self.pt(Pat::Bind {
                        binder,
                        mode: BindMode::Value,
                        sub: None,
                    });
                    return (Val::Rest(Some(p)), false);
                }
                let sub = self.opt_p(f, 0);
                let mode = match (has(f.mods, Modifier::Ref), has(f.mods, Modifier::Mut)) {
                    (true, true) => BindMode::RefMut,
                    (true, false) => BindMode::Ref,
                    _ => BindMode::Value,
                };
                let p = match f.binder {
                    Some(binder) => {
                        self.at(span);
                        self.pt(Pat::Bind { binder, mode, sub })
                    }
                    None => {
                        self.at(span);
                        self.pt(Pat::Err)
                    }
                };
                self.adapt_pat(p, want, span)
            }
            Arch::TuplePat { .. } => {
                let (pats, rest, _) = self.pats_with_rest(f, 0);
                let elems = self.list(&pats);
                self.at(span);
                let p = self.pt(Pat::Tuple { elems, rest });
                self.adapt_pat(p, want, span)
            }
            Arch::SlicePat { .. } => {
                let p = self.slice_pat(f, 0);
                self.adapt_pat(p, want, span)
            }
            Arch::CtorPat { .. } => {
                let path = self.req_path(f, 0, "path");
                let (pats, rest, _) = self.pats_with_rest(f, 1);
                let elems = self.list(&pats);
                self.at(span);
                let p = self.pt(Pat::Ctor { path, elems, rest });
                self.adapt_pat(p, want, span)
            }
            Arch::RecordPat { .. } => {
                let path = self.opt_path(f, 0);
                let (fields, rest) = self.field_pats(f, 1);
                self.at(span);
                let p = self.pt(Pat::Record { path, fields, rest });
                self.adapt_pat(p, want, span)
            }
            Arch::FieldPat { .. } => {
                let (sym, nspan) = f.name.unwrap_or((self.empty, span));
                let pat = match (self.opt_p(f, 0), want) {
                    (Some(p), _) => p,
                    (None, Want::FieldPat(site)) => {
                        self.bind_pat(sym, nspan, site, BindMode::Value)
                    }
                    (None, _) => {
                        self.at(span);
                        self.pt(Pat::Err)
                    }
                };
                Val::FieldPat(FieldPat {
                    name: Ident::new(sym, nspan),
                    pat,
                })
            }
            Arch::OrPat { .. } => {
                let vals = self.many(f, 0);
                let pats: Vec<PatId> = vals
                    .iter()
                    .filter_map(|s| match s.v {
                        Val::Pat(p) => Some(p),
                        _ => None,
                    })
                    .collect();
                self.give(vals);
                self.at(span);
                let p = if pats.is_empty() {
                    self.report(ProblemKind::MissingField { field: "alts" }, span);
                    self.pt(Pat::Err)
                } else {
                    let alts = self.list(&pats);
                    self.pt(Pat::Or(alts))
                };
                self.adapt_pat(p, want, span)
            }
            Arch::RangePat { .. } => {
                let lo = self.opt_p(f, 0);
                let hi = self.opt_p(f, 1);
                self.at(span);
                let p = self.pt(Pat::Range {
                    lo,
                    hi,
                    inclusive: has(f.mods, Modifier::Inclusive),
                });
                self.adapt_pat(p, want, span)
            }
            Arch::RefPat { .. } => {
                let inner = self.req_p(f, 0, "inner");
                self.at(span);
                let p = self.pt(Pat::Ref {
                    mutable: has(f.mods, Modifier::Mut),
                    inner,
                });
                self.adapt_pat(p, want, span)
            }
            Arch::TypeTestPat { .. } => {
                let ty = self.req_t(f, 0, "ty");
                let pat = self.opt_p(f, 1);
                self.at(span);
                let p = self.pt(Pat::TypeTest { ty, pat });
                self.adapt_pat(p, want, span)
            }
            Arch::TupleTy { .. } => {
                let elems = self.tys(f, 0);
                self.at(span);
                let t = self.ty(Ty::Tuple(elems));
                self.adapt_ty(t, want, span)
            }
            Arch::ArrayTy { .. } => {
                let elem = self.req_t(f, 0, "elem");
                let len = self.req_e(f, 1, "len");
                self.at(span);
                let t = self.ty(Ty::Array { elem, len });
                self.adapt_ty(t, want, span)
            }
            Arch::SliceTy { .. } => {
                let elem = self.req_t(f, 0, "elem");
                self.at(span);
                let t = self.ty(Ty::Slice(elem));
                self.adapt_ty(t, want, span)
            }
            Arch::RefTy { .. } => {
                let inner = self.req_t(f, 0, "inner");
                let region = self.opt_path(f, 1);
                self.at(span);
                let t = self.ty(Ty::Ref {
                    mutable: has(f.mods, Modifier::Mut),
                    region,
                    inner,
                });
                self.adapt_ty(t, want, span)
            }
            Arch::PtrTy { .. } => {
                let inner = self.req_t(f, 0, "inner");
                self.at(span);
                let t = self.ty(Ty::Ptr {
                    mutable: has(f.mods, Modifier::Mut),
                    inner,
                });
                self.adapt_ty(t, want, span)
            }
            Arch::NullableTy { .. } => {
                let inner = self.req_t(f, 0, "inner");
                self.at(span);
                let t = self.ty(Ty::Nullable(inner));
                self.adapt_ty(t, want, span)
            }
            Arch::FnTy { .. } => {
                let params = self.tys(f, 0);
                let ret = match self.opt_t(f, 1) {
                    Some(t) => t,
                    None => {
                        self.at(span);
                        self.ty(Ty::Tuple(List::EMPTY))
                    }
                };
                self.at(span);
                let t = self.ty(Ty::Fn {
                    params,
                    ret,
                    effects: hir_lang::Effects::NONE,
                    throws: None,
                    abi: None,
                });
                self.adapt_ty(t, want, span)
            }
            Arch::ObjectTy { .. } => {
                let bounds = self.bounds(f, 0);
                self.at(span);
                let t = self.ty(Ty::Object(bounds));
                self.adapt_ty(t, want, span)
            }
            Arch::ImplTy { .. } => {
                let bounds = self.bounds(f, 0);
                self.at(span);
                let t = self.ty(Ty::Impl(bounds));
                self.adapt_ty(t, want, span)
            }
            // Handled at entry; never opens a frame.
            Arch::Pass { .. }
            | Arch::Unsupported
            | Arch::GlobalDecl { .. }
            | Arch::Capture { .. }
            | Arch::WildPat
            | Arch::Template(_)
            | Arch::Unary { .. }
            | Arch::Binary { .. }
            | Arch::Postfix { .. }
            | Arch::Assign { .. }
            | Arch::RefAssign { .. }
            | Arch::UseTree { .. }
            | Arch::Attr { .. }
            | Arch::AttrArg { .. } => self.error_val(want, span),
        };
        (v, false)
    }

    /// An interpolated string: its pieces joined per `join`.
    fn finish_interp(&mut self, f: &Frame<K>, join: Join) -> (Val, bool) {
        let span = f.span;
        let (base, len) = (f.aux.0 as usize, f.aux.1 as usize);
        let pieces: Vec<Piece> = self
            .pieces
            .get(base..base.saturating_add(len))
            .unwrap_or(&[])
            .to_vec();
        self.pieces.truncate(base);
        let (s, _) = self.field_range(f, 0);
        let mut parts: Vec<ExprId> = Vec::with_capacity(pieces.len());
        let mut k = 0usize;
        let mut exprs = 0usize;
        for piece in pieces {
            match piece {
                Piece::Text(t, sp) => {
                    self.at(sp);
                    parts.push(self.ex(Expr::Lit(hir_lang::Lit::Str(t))));
                }
                Piece::Expr => {
                    let e = match self.vals.get(s + k).map(|x| x.v) {
                        Some(Val::Expr(e)) => e,
                        _ => self.err_expr(span),
                    };
                    k += 1;
                    exprs += 1;
                    parts.push(e);
                }
            }
        }
        let result = match parts.as_slice() {
            [] => self.empty_str(span),
            [one] if exprs == 0 => *one,
            _ => {
                // A lone expression is joined with "" first: a conversion.
                if parts.len() == 1 {
                    let empty = self.empty_str(span);
                    parts.insert(0, empty);
                }
                self.join_parts(join, &parts, span)
            }
        };
        let v = self.adapt_expr(result, false, f.want, span);
        (v, false)
    }

    fn empty_str(&mut self, span: Span) -> ExprId {
        self.at(span);
        let t = self.b.text("");
        self.ex(Expr::Lit(hir_lang::Lit::Str(t)))
    }

    /// A call of host function `name` (`Res::Extern`) with `args`.
    fn extern_call(
        &mut self,
        name: crate::mapping::NameRef,
        args: &[ExprId],
        span: Span,
    ) -> ExprId {
        let sym = self.sym(name.0);
        self.at(span);
        let seg = Segment::new(Name::new(sym), Origin::new(span));
        let segs = self.list(&[seg]);
        let mut p = Path::new(segs, Ns::Value);
        p.res = Res::Extern(sym);
        let path = self.path(p);
        let callee = self.ex(Expr::Path(path));
        let args: Vec<Arg> = args.iter().map(|&e| Arg::positional(e)).collect();
        let args = self.list(&args);
        self.ex(Expr::Call { callee, args })
    }

    /// Joins at least two parts.
    fn join_parts(&mut self, join: Join, parts: &[ExprId], span: Span) -> ExprId {
        match join {
            Join::Call(name) => self.extern_call(name, parts, span),
            Join::Fold(name) => {
                let mut acc = parts
                    .first()
                    .copied()
                    .unwrap_or_else(|| self.err_expr(span));
                for &p in parts.get(1..).unwrap_or(&[]) {
                    acc = self.extern_call(name, &[acc, p], span);
                }
                acc
            }
            Join::Template(id) => {
                let args_hole = self.m.templates.get(id.index()).is_some_and(|t| {
                    t.holes.len() == 1 && t.holes.iter().all(|h| h.kind == HoleKind::Args)
                });
                if args_hole {
                    let vals: Vec<Val> = parts
                        .iter()
                        .map(|&e| Val::Arg(Arg::positional(e)))
                        .collect();
                    self.instantiate(id, span, &[&vals])
                } else {
                    let mut acc = parts
                        .first()
                        .copied()
                        .unwrap_or_else(|| self.err_expr(span));
                    for &p in parts.get(1..).unwrap_or(&[]) {
                        acc = self.instantiate(id, span, &[&[Val::Expr(acc)], &[Val::Expr(p)]]);
                    }
                    acc
                }
            }
        }
    }

    /// Instantiates template `id` with already-lowered hole values (one
    /// slice per hole), through a synthetic frame.
    pub(crate) fn instantiate(&mut self, id: TemplateId, span: Span, holes: &[&[Val]]) -> ExprId {
        let vals_base = self.vals.len();
        let counts_base = self.counts.len();
        for h in holes {
            for &v in *h {
                self.vals.push(Slot {
                    v,
                    span,
                    place: false,
                });
            }
            self.counts.push(h.len());
        }
        let mut fr = self.frame(Arch::Unsupported, Want::Expr, span);
        fr.build = Build::Template {
            id,
            label: None,
            nodes_at: self.nodes,
        };
        fr.counts_base = counts_base;
        fr.nfields = holes.len();
        fr.vals_base = vals_base;
        fr.spill_base = self.spill.len();
        let nodes = self.nodes;
        let v = self.finish_template(&fr, id, None, nodes);
        self.vals.truncate(vals_base);
        self.counts.truncate(counts_base);
        match v {
            Val::Expr(e) => e,
            _ => self.err_expr(span),
        }
    }

    /// A slice pattern from a list with an optional `..`.
    fn slice_pat(&mut self, f: &Frame<K>, i: usize) -> PatId {
        let (pats, rest, bind) = self.pats_with_rest(f, i);
        let span = f.span;
        let (prefix, rest) = match rest {
            Some(at) => {
                let at = at as usize;
                let (pre, suf) = pats.split_at(at.min(pats.len()));
                let prefix = self.list(pre);
                let suffix = self.list(suf);
                (prefix, Some(SliceRest { bind, suffix }))
            }
            None => (self.list(&pats), None),
        };
        self.at(span);
        self.pt(Pat::Slice { prefix, rest })
    }

    /// Record-pattern fields and whether a `..` ends them.
    fn field_pats(&mut self, f: &Frame<K>, i: usize) -> (List<FieldPat>, bool) {
        let vals = self.many(f, i);
        let mut rest = false;
        let out: Vec<FieldPat> = vals
            .iter()
            .filter_map(|s| match s.v {
                Val::FieldPat(fp) => Some(fp),
                Val::Rest(_) => {
                    rest = true;
                    None
                }
                _ => None,
            })
            .collect();
        self.give(vals);
        (self.list(&out), rest)
    }

    /// Members: items, and fields (classes only).
    fn member_items(
        &mut self,
        f: &Frame<K>,
        i: usize,
    ) -> (List<hir_lang::ItemId>, List<hir_lang::FieldId>) {
        let vals = self.many(f, i);
        let mut items = Vec::new();
        let mut fields = Vec::new();
        for s in vals.iter() {
            match s.v {
                Val::Item(it) => items.push(it),
                Val::Field(fd, _) => fields.push(fd),
                _ => {}
            }
        }
        self.give(vals);
        (self.list(&items), self.list(&fields))
    }

    /// A block from statement values (field `stmts`) and an optional explicit
    /// tail (field `tail`).
    fn block_of(
        &mut self,
        f: &Frame<K>,
        stmts: usize,
        tail: Option<usize>,
        tail_from_last: bool,
    ) -> Block {
        let vals = self.many(f, stmts);
        let explicit = tail.and_then(|t| self.opt_e(f, t));
        let mut last_tail = None;
        let n = vals.len();
        let mut out = core::mem::take(&mut self.lists.stmts);
        out.clear();
        for (j, s) in vals.iter().enumerate() {
            if j + 1 == n && explicit.is_none() && tail_from_last {
                if let Val::Expr(e) = s.v {
                    last_tail = Some(e);
                    continue;
                }
            }
            if let Some(st) = self.stmt_of(*s) {
                out.push(st);
            }
        }
        self.give(vals);
        let stmts = self.list(&out);
        self.lists.stmts = out;
        Block {
            stmts,
            tail: explicit.or(last_tail),
            label: f.binder,
            is_unsafe: has(f.mods, Modifier::Unsafe),
        }
    }

    /// A module (or the synthetic root module): declarations become items,
    /// everything else the top-level body, after any hoisted declarations.
    fn finish_module(&mut self, f: &Frame<K>, name: Option<(Symbol, Span)>) -> Val {
        let (effects, hoisted) = self.pop_fx(f.span);
        let vals = self.many(f, 0);
        let mut items = Vec::new();
        let mut stmts = hoisted;
        for s in vals.iter() {
            match s.v {
                Val::Item(i) => items.push(i),
                _ => {
                    if let Some(st) = self.stmt_of(*s) {
                        stmts.push(st);
                    }
                }
            }
        }
        self.give(vals);
        self.at(f.span);
        let items = self.list(&items);
        let body = if stmts.is_empty() {
            None
        } else {
            let stmts = self.list(&stmts);
            Some(self.ex(Expr::Block(Block {
                stmts,
                tail: None,
                label: None,
                is_unsafe: false,
            })))
        };
        let mut item = Item::new(
            name.map(|(s, _)| Name::new(s)),
            ItemKind::Module {
                items,
                body,
                effects,
            },
        )
        .with_vis(vis_of(f.mods));
        if let Some((_, nspan)) = name {
            item = item.with_name_span(nspan);
        }
        Val::Item(self.it(item))
    }

    /// `var x = init` after hoisting: `x = init`, or nothing.
    fn finish_hoisted(&mut self, f: &Frame<K>, name: Symbol, name_span: Span) -> Val {
        match self.opt_e(f, 0) {
            Some(value) => {
                let target = self.name_expr(name, name_span);
                self.at(f.span);
                let assign = self.ex(Expr::Assign {
                    target,
                    op: None,
                    value,
                });
                Val::Stmt(self.st(Stmt::Expr(assign)))
            }
            None => self.spill_of(&[]),
        }
    }

    /// `catch (A | B $e) body`.
    fn finish_catch(&mut self, f: &Frame<K>) -> Val {
        let span = f.span;
        let vals = self.many(f, 0);
        let types: Vec<TyId> = vals
            .iter()
            .filter_map(|s| match s.v {
                Val::Ty(t) => Some(t),
                _ => None,
            })
            .collect();
        self.give(vals);
        let body = self.req_e(f, 1, "body");
        // PHP scoping: the caught exception's variable belongs to the
        // function; the arm binds a temporary and assigns it on entry.
        let persist = self.m.policy.function_scoped_bindings;
        let mut binds: Vec<super::Persist> = Vec::new();
        let binder = f.name.map(|(sym, nspan)| {
            if persist {
                let b = self.persist_binder(sym, nspan);
                if let Some(p) = self.persist.pop() {
                    binds.push(p);
                }
                b
            } else {
                self.at(nspan);
                self.binder(Name::new(sym), BinderKind::Local, false)
            }
        });
        let body = self.with_assigns(&binds, body, span);
        self.at(span);
        let bind = |this: &mut Self| {
            binder.map(|b| {
                this.pt(Pat::Bind {
                    binder: b,
                    mode: BindMode::Value,
                    sub: None,
                })
            })
        };
        let pat = if types.is_empty() {
            match bind(self) {
                Some(p) => p,
                None => self.pt(Pat::Wild),
            }
        } else {
            let mut alts = Vec::with_capacity(types.len());
            for ty in types {
                let pat = bind(self);
                alts.push(self.pt(Pat::TypeTest { ty, pat }));
            }
            if alts.len() == 1 {
                alts.first().copied().unwrap_or_else(|| self.pt(Pat::Err))
            } else {
                let list = self.list(&alts);
                self.pt(Pat::Or(list))
            }
        };
        Val::Arm(Arm {
            pat,
            guard: None,
            body,
        })
    }

    /// A path node: segments from fields 1 (trait) and 2 (the rest).
    fn finish_path(&mut self, f: &Frame<K>) -> (Val, bool) {
        let qself = self.opt_t(f, 0);
        let trait_vals = self.many(f, 1);
        let trait_segs: Vec<Segment> = trait_vals
            .iter()
            .filter_map(|s| match s.v {
                Val::Seg(seg) => Some(seg),
                _ => None,
            })
            .collect();
        self.give(trait_vals);
        let vals = self.many(f, 2);
        let segs: Vec<Segment> = vals
            .iter()
            .filter_map(|s| match s.v {
                Val::Seg(seg) => Some(seg),
                _ => None,
            })
            .collect();
        self.give(vals);
        self.path_val(f, f.path_root, qself, &trait_segs, &segs)
    }

    /// Builds a path in the namespace of `f`'s context and adapts it.
    fn path_val(
        &mut self,
        f: &Frame<K>,
        root: PathRoot,
        qself: Option<TyId>,
        trait_segs: &[Segment],
        segs: &[Segment],
    ) -> (Val, bool) {
        let want = f.want;
        let span = f.span;
        let total = trait_segs.len() + segs.len();
        if total == 0 {
            self.report(ProblemKind::MissingField { field: "segments" }, span);
            let v = self.error_val(want, span);
            return (v, false);
        }
        // A bare name in a pattern binds (or is an identifier pattern); in a
        // type it may be a type keyword.
        let simple = qself.is_none()
            && root == PathRoot::Relative
            && trait_segs.is_empty()
            && segs.len() == 1;
        if simple {
            if let Some(seg) = segs.first() {
                if seg.args.is_empty() {
                    let sym = seg.name.sym;
                    let nspan = seg.origin.span;
                    match want {
                        Want::Pat(site) if !site.bound => {
                            let p = self.ident_pat(sym, nspan, site);
                            return (Val::Pat(p), false);
                        }
                        Want::Param => {
                            let site = super::enter::site_of(want).unwrap_or(super::PatSite::LOCAL);
                            let p = self.ident_pat(sym, nspan, site);
                            return (Val::Param(Param::new(p), span, (0, 0)), false);
                        }
                        Want::Ty | Want::GArg => {
                            let m = self.m;
                            if let Some(name) = self.names.resolve(sym).and_then(|t| m.type_name(t))
                            {
                                let ty = match name {
                                    crate::TypeName::Prim(p) => Ty::Prim(p),
                                    crate::TypeName::Any => Ty::Any,
                                    crate::TypeName::Never => Ty::Never,
                                    crate::TypeName::SelfTy => Ty::SelfTy,
                                    crate::TypeName::Infer => Ty::Infer,
                                };
                                self.at(span);
                                let t = self.ty(ty);
                                return (self.adapt_ty(t, want, span), false);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        let ns = match want {
            Want::Ty | Want::GArg => Ns::Type,
            Want::Pat(_) | Want::Param => Ns::Pattern,
            Want::Path(ns) => ns,
            _ => Ns::Value,
        };
        let mut all: Vec<Segment> = Vec::with_capacity(total);
        all.extend_from_slice(trait_segs);
        all.extend_from_slice(segs);
        let segments = self.list(&all);
        let trait_len = u32::try_from(trait_segs.len()).unwrap_or(u32::MAX);
        let count = u32::try_from(total).unwrap_or(u32::MAX);
        let unresolved = if qself.is_some() {
            count.saturating_sub(trait_len)
        } else if root.is_type_root() {
            count
        } else {
            0
        };
        self.at(span);
        let path = self.path(Path {
            segments,
            ns,
            root,
            qself: qself.map(|ty| QSelf { ty, trait_len }),
            res: Res::Unresolved,
            unresolved,
        });
        match want {
            Want::Path(_) => (Val::Path(path), false),
            Want::Ty | Want::GArg => {
                let t = self.ty(Ty::Path(path));
                (self.adapt_ty(t, want, span), false)
            }
            Want::Pat(_) | Want::Param => {
                let p = self.pt(Pat::Path(path));
                (self.adapt_pat(p, want, span), false)
            }
            Want::Segment => {
                // A path where one segment was expected: keep its last segment.
                match segs.last() {
                    Some(seg) => (Val::Seg(*seg), false),
                    None => (Val::Err, false),
                }
            }
            _ => {
                let e = self.ex(Expr::Path(path));
                (self.adapt_expr(e, true, want, span), true)
            }
        }
    }

    // ------------------------------------------------------------ operators

    fn finish_op(&mut self, f: &Frame<K>, spec: OpSpec) -> (Val, bool) {
        let binary = f.nfields == 2;
        let a = self.req_e(f, 0, if binary { "lhs" } else { "operand" });
        let b = if binary {
            Some(self.req_e(f, 1, "rhs"))
        } else {
            None
        };
        let pair = [a, b.unwrap_or(a)];
        let operands = if binary { &pair[..] } else { &pair[..1] };
        let e = match spec {
            OpSpec::Op(kind) => {
                let op = self.policy_op(kind);
                let args = self.list(operands);
                Expr::Op { op, args }
            }
            OpSpec::OpWith(op) => {
                let args = self.list(operands);
                Expr::Op { op, args }
            }
            OpSpec::Compound(kind) => Expr::Assign {
                target: a,
                op: Some(self.policy_op(kind)),
                value: b.unwrap_or(a),
            },
            OpSpec::CompoundWith(op) => Expr::Assign {
                target: a,
                op: Some(op),
                value: b.unwrap_or(a),
            },
            OpSpec::Assign => match b {
                Some(value) => Expr::Assign {
                    target: a,
                    op: None,
                    value,
                },
                None => Expr::Err,
            },
            OpSpec::RefAssign => match b {
                Some(source) => Expr::RefAssign { target: a, source },
                None => Expr::Err,
            },
            OpSpec::Extern(name) => {
                let sym = self.sym(name.0);
                self.at(f.span);
                let seg = Segment::new(Name::new(sym), Origin::new(f.span));
                let segs = self.list(&[seg]);
                let mut p = Path::new(segs, Ns::Value);
                p.res = Res::Extern(sym);
                let path = self.path(p);
                let callee = self.ex(Expr::Path(path));
                let args: Vec<Arg> = operands.iter().map(|&e| Arg::positional(e)).collect();
                let args = self.list(&args);
                Expr::Call { callee, args }
            }
            OpSpec::Intrinsic(name) => {
                let sym = self.sym(name.0);
                let args = self.list(operands);
                Expr::Intrinsic {
                    kind: hir_lang::Intrinsic::Named(sym),
                    generic_args: List::EMPTY,
                    args,
                }
            }
            OpSpec::Borrow(kind) => Expr::Borrow { kind, expr: a },
            OpSpec::Deref => Expr::Deref(a),
            OpSpec::Await => Expr::Await(a),
            // Lowered at entry.
            OpSpec::Identity | OpSpec::Template(_) => Expr::Err,
        };
        let place = matches!(spec, OpSpec::Deref);
        self.done_expr(f, e, place)
    }
}

/// The shape a record or variant gets.
fn decide_shape(shape: RecordShape, len: usize, any_named: bool, all_named: bool) -> Shape {
    match shape {
        RecordShape::Named => Shape::Named,
        RecordShape::Tuple => Shape::Tuple,
        RecordShape::Unit => Shape::Unit,
        RecordShape::Auto => {
            if len > 0 && !any_named && !all_named {
                Shape::Tuple
            } else {
                Shape::Named
            }
        }
    }
}
