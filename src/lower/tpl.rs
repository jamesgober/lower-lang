//! Instantiating a template: build its HIR with an explicit stack, one
//! expansion per instantiation (and per nested `apply`), fresh marked binders,
//! and the holes' values substituted.
//!
//! The holes of the template a node uses were lowered as the frame's fields
//! before this runs. A hole used once takes its value; each further use takes
//! a copy (`Builder::copy_subtree`, fresh binders for everything bound inside),
//! so HIR stays a tree. An `apply` evaluates the applied template in a new
//! context whose holes forward to the arguments, evaluated lazily in the
//! applying context.

use alloc::vec::Vec;

use hir_lang::{
    Arg, Arm, BindMode, BinderId, BinderKind, Block, Capture, Closure, DefaultEval, Expansion,
    ExpnId, Expr, ExprId, FloatLit, Ident, IntLit, List, Lit, Member, Name, NodeRef, Ns, Origin,
    Param, Pat, PatId, Path, Res, Segment, Span, Stmt, StmtId, Ty, TyId,
};
use syntax_lang::TokenKind;

use super::{Frame, Run, Slot, Val};
use crate::problem::ProblemKind;
use crate::template::{
    HoleKind, LabelRef, LabelSlot, NONE, TArg, TLit, TNode, Template, TemplateId,
};

/// One instantiation context: a template, its expansion, its binder and
/// label tables, and where its holes come from.
#[derive(Clone, Copy)]
struct Ctx {
    tpl: u32,
    expn: ExpnId,
    benv: usize,
    lenv: usize,
    /// The applying context, or `NONE` for the instantiation the node made.
    parent: u32,
    /// The `apply`'s arguments in the parent template's argument pool.
    args: (u32, u32),
    depth: u32,
}

/// Where a hole's value comes from.
enum Source {
    /// The node's own hole `h` (lowered user code).
    Top(usize),
    /// A template node, evaluated in a context.
    Node(u32, u32),
    /// Nothing: an absent optional hole of template `tpl`.
    Absent(u32, usize),
}

#[derive(Clone, Copy)]
enum Work {
    Enter(u32, u32),
    Exit(u32, u32),
}

/// Buffers one instantiation uses, kept by the run and reused, so
/// instantiating a template allocates nothing once they have grown.
#[derive(Default)]
pub(crate) struct TplScratch {
    ctxs: Vec<Ctx>,
    benv: Vec<Option<BinderId>>,
    lenv: Vec<Option<BinderId>>,
    work: Vec<Work>,
    out: Vec<Slot>,
    kids: Vec<u32>,
    res: Vec<Slot>,
    uses: Vec<u32>,
}

/// Per-instantiation state for the node's own holes.
struct Holes {
    uses: Vec<u32>,
    /// Nodes the holes' user code took to build: the conservative charge for
    /// one copy of a hole.
    charge: u64,
    label: Option<BinderId>,
    missing_reported: bool,
}

impl<K> Run<'_, '_, K>
where
    K: Copy + Ord + TokenKind,
{
    pub(crate) fn finish_template(
        &mut self,
        f: &Frame<K>,
        id: TemplateId,
        label: Option<BinderId>,
        nodes_at: u64,
    ) -> Val {
        let m = self.m;
        let span = f.span;
        let Some(t) = m.templates.get(id.index()) else {
            let e = self.err_expr(span);
            return self.adapt_expr(e, false, f.want, span);
        };
        let mut sc = core::mem::take(&mut self.tpl_scratch);
        sc.uses.clear();
        sc.uses.resize(t.holes.len(), 0);
        let mut holes = Holes {
            uses: core::mem::take(&mut sc.uses),
            charge: self.nodes.saturating_sub(nodes_at),
            label,
            missing_reported: false,
        };
        let expn = self.b.expansion(Expansion {
            kind: t.kind,
            name: self.sym(m.template_names.get(id.index()).copied().unwrap_or(0)),
            call_site: span,
            parent: ExpnId::ROOT,
            def_site: ExpnId::ROOT,
        });
        sc.ctxs.clear();
        sc.ctxs.push(Ctx {
            tpl: id.0,
            expn,
            benv: 0,
            lenv: 0,
            parent: NONE,
            args: (0, 0),
            depth: 1,
        });
        sc.benv.clear();
        sc.benv.resize(t.binders.len(), None);
        sc.lenv.clear();
        sc.lenv.resize(t.labels.len(), None);
        sc.work.clear();
        sc.work.push(Work::Enter(t.root, 0));
        sc.out.clear();
        let TplScratch {
            ctxs,
            benv,
            lenv,
            work,
            out,
            kids,
            res,
            ..
        } = &mut sc;
        while let Some(w) = work.pop() {
            if self.nodes > self.budget.max_nodes {
                break;
            }
            match w {
                Work::Enter(n, c) => {
                    self.t_enter(f, n, c, ctxs, benv, lenv, work, out, kids, &mut holes);
                }
                Work::Exit(n, c) => {
                    self.t_exit(f, n, c, ctxs, benv, lenv, out, kids, res, &mut holes);
                }
            }
        }
        let popped = out.pop();
        sc.uses = core::mem::take(&mut holes.uses);
        self.tpl_scratch = sc;
        let result = match popped {
            Some(Slot {
                v: Val::Expr(e), ..
            }) => e,
            _ => self.err_expr(span),
        };
        let uses = core::mem::take(&mut self.tpl_scratch.uses);
        let result = self.rescue(f, &uses, result);
        self.tpl_scratch.uses = uses;
        self.at(span);
        self.adapt_expr(result, false, f.want, span)
    }

    /// Sets the builder's origin to the call site inside expansion `expn`.
    fn t_at(&mut self, f: &Frame<K>, expn: ExpnId) {
        self.b.set_origin(Origin::expanded(f.span, expn));
    }

    fn tpl_of(&self, ctx: &Ctx) -> Option<&Template> {
        self.m.templates.get(ctx.tpl as usize)
    }

    /// Follows hole `h` of context `c` back to where its value comes from.
    fn source(&self, mut h: usize, mut c: u32, ctxs: &[Ctx]) -> Source {
        loop {
            let Some(ctx) = ctxs.get(c as usize) else {
                return Source::Absent(0, h);
            };
            if ctx.parent == NONE {
                return Source::Top(h);
            }
            let parent = ctx.parent;
            let Some(pt) = ctxs.get(parent as usize).and_then(|p| self.tpl_of(p)) else {
                return Source::Absent(ctx.tpl, h);
            };
            let arg = pt.args.get(ctx.args.0 as usize + h).copied();
            match arg {
                Some(TArg::Expr(e)) => return Source::Node(e.0, parent),
                Some(TArg::Pat(p)) => return Source::Node(p.0, parent),
                Some(TArg::Ty(t)) => return Source::Node(t.0, parent),
                Some(TArg::Stmts(next)) => {
                    h = usize::from(next.0);
                    c = parent;
                }
                Some(TArg::Args(next)) => {
                    h = usize::from(next.0);
                    c = parent;
                }
                Some(TArg::Label(LabelRef::Hole(next))) => {
                    h = usize::from(next.0);
                    c = parent;
                }
                Some(TArg::Label(LabelRef::Own(_)) | TArg::Absent) | None => {
                    return Source::Absent(ctx.tpl, h);
                }
            }
        }
    }

    /// The binder a label slot names in context `c`.
    fn slot_label(
        &self,
        slot: LabelSlot,
        c: u32,
        ctxs: &[Ctx],
        lenv: &[Option<BinderId>],
        holes: &Holes,
    ) -> Option<BinderId> {
        match slot {
            LabelSlot::None => None,
            LabelSlot::Own(l) => {
                let ctx = ctxs.get(c as usize)?;
                lenv.get(ctx.lenv + usize::from(l)).copied().flatten()
            }
            LabelSlot::Hole(h) => {
                let mut h = usize::from(h);
                let mut c = c;
                loop {
                    let ctx = ctxs.get(c as usize)?;
                    if ctx.parent == NONE {
                        return holes.label;
                    }
                    let parent = ctx.parent;
                    let pt = ctxs.get(parent as usize).and_then(|p| self.tpl_of(p))?;
                    match pt.args.get(ctx.args.0 as usize + h).copied()? {
                        TArg::Label(LabelRef::Hole(next)) => {
                            h = usize::from(next.0);
                            c = parent;
                        }
                        TArg::Label(LabelRef::Own(l)) => {
                            let pctx = ctxs.get(parent as usize)?;
                            return lenv.get(pctx.lenv + usize::from(l.0)).copied().flatten();
                        }
                        _ => return None,
                    }
                }
            }
        }
    }

    /// The binder for template binder `b` in context `c`, created on first
    /// touch with the context's hygiene mark.
    fn t_binder(
        &mut self,
        f: &Frame<K>,
        b: usize,
        c: &Ctx,
        benv: &mut [Option<BinderId>],
        kind: BinderKind,
    ) -> Option<BinderId> {
        let slot = benv.get(c.benv + b).copied()?;
        if let Some(id) = slot {
            return Some(id);
        }
        let m = self.m;
        let t = m.templates.get(c.tpl as usize)?;
        let decl = *t.binders.get(b)?;
        let sym = self.sym(t.text_base + decl.name);
        self.t_at(f, c.expn);
        let id = self.binder(Name::marked(sym, c.expn), kind, decl.mutable);
        if let Some(s) = benv.get_mut(c.benv + b) {
            *s = Some(id);
        }
        Some(id)
    }

    /// The value of a node's own hole `h`, taken on first use and copied on
    /// later ones.
    fn top_hole(&mut self, f: &Frame<K>, h: usize, holes: &mut Holes, out: &mut Vec<Slot>) {
        let (s, e) = self.field_range(f, h);
        let first = holes.uses.get(h).copied().unwrap_or(0) == 0;
        if let Some(u) = holes.uses.get_mut(h) {
            *u = u.saturating_add(1);
        }
        let mut vals = self.bufs.pop().unwrap_or_default();
        vals.clear();
        for j in s..e {
            match self.vals.get(j).copied() {
                Some(Slot {
                    v: Val::Spill(ss, sl),
                    ..
                }) => {
                    let (ss, sl) = (ss as usize, sl as usize);
                    if let Some(run) = self.spill.get(ss..ss.saturating_add(sl)) {
                        vals.extend_from_slice(run);
                    }
                }
                Some(slot) => vals.push(slot),
                None => {}
            }
        }
        if !first {
            let charge = holes.charge.max(1);
            if self.nodes.saturating_add(charge) > self.budget.max_nodes {
                // Out of budget: stop here; the run reports `NodeBudget`.
                self.nodes = self.budget.max_nodes.saturating_add(1);
                out.push(Slot {
                    v: Val::Err,
                    span: f.span,
                    place: false,
                });
                return;
            }
            self.nodes += charge;
            for slot in &mut vals {
                slot.v = self.copy_val(slot.v);
            }
        }
        let kind = self
            .tpl_of_id(f)
            .and_then(|t| t.holes.get(h))
            .map(|d| d.kind);
        let v = match kind {
            Some(HoleKind::Stmts | HoleKind::Args) => self.spill_of(&vals),
            _ => match vals.first() {
                Some(slot) => slot.v,
                None => Val::Err,
            },
        };
        self.give(vals);
        out.push(Slot {
            v,
            span: f.span,
            place: false,
        });
    }

    fn tpl_of_id(&self, f: &Frame<K>) -> Option<&Template> {
        match f.build {
            super::Build::Template { id, .. } => self.m.templates.get(id.index()),
            _ => None,
        }
    }

    /// A copy of a hole value (fresh binders for everything bound inside).
    fn copy_val(&mut self, v: Val) -> Val {
        match v {
            Val::Expr(e) => match self.b.copy_subtree(NodeRef::Expr(e)) {
                NodeRef::Expr(c) => Val::Expr(c),
                _ => Val::Expr(e),
            },
            Val::Stmt(s) => match self.b.copy_subtree(NodeRef::Stmt(s)) {
                NodeRef::Stmt(c) => Val::Stmt(c),
                _ => Val::Stmt(s),
            },
            Val::Item(i) => match self.b.copy_subtree(NodeRef::Item(i)) {
                NodeRef::Item(c) => Val::Item(c),
                _ => Val::Item(i),
            },
            Val::Pat(p) => match self.b.copy_subtree(NodeRef::Pat(p)) {
                NodeRef::Pat(c) => Val::Pat(c),
                _ => Val::Pat(p),
            },
            Val::Ty(t) => match self.b.copy_subtree(NodeRef::Ty(t)) {
                NodeRef::Ty(c) => Val::Ty(c),
                _ => Val::Ty(t),
            },
            Val::Arg(a) => match self.b.copy_subtree(NodeRef::Expr(a.value)) {
                NodeRef::Expr(c) => Val::Arg(Arg { value: c, ..a }),
                _ => Val::Arg(a),
            },
            other => other,
        }
    }

    /// The value of an absent hole: its default literal, or `Val::Err`
    /// (left out where optional, an error where required). A required hole of
    /// the node itself is reported once.
    fn absent(&mut self, f: &Frame<K>, ctx: &Ctx, tpl: u32, h: usize, holes: &mut Holes) -> Val {
        let decl = self
            .m
            .templates
            .get(tpl as usize)
            .and_then(|t| t.holes.get(h))
            .cloned();
        match decl {
            Some(d) => {
                if let Some(lit) = d.default {
                    self.t_at(f, ctx.expn);
                    let lit = self.t_lit(lit);
                    return Val::Expr(self.ex(Expr::Lit(lit)));
                }
                if d.kind == HoleKind::Pat {
                    self.t_at(f, ctx.expn);
                    return Val::Pat(self.pt(Pat::Wild));
                }
                if !d.optional {
                    self.report_missing(f, holes);
                }
                Val::Err
            }
            None => Val::Err,
        }
    }

    fn report_missing(&mut self, f: &Frame<K>, holes: &mut Holes) {
        if !holes.missing_reported {
            holes.missing_reported = true;
            let template = match f.build {
                super::Build::Template { id, .. } => id,
                _ => TemplateId(0),
            };
            self.report(ProblemKind::MissingHole { template }, f.span);
        }
    }

    fn t_lit(&mut self, lit: TLit) -> Lit {
        match lit {
            TLit::Int(v) => Lit::Int(IntLit::signed(v)),
            TLit::Float(v) => Lit::Float(FloatLit::new(v)),
            TLit::Bool(b) => Lit::Bool(b),
            TLit::Null => Lit::Null,
            TLit::Char(c) => Lit::Char(c),
        }
    }

    /// A marked path for a free name the template writes (`a::b`).
    fn t_path(
        &mut self,
        f: &Frame<K>,
        ctx: &Ctx,
        text: u32,
        ns: Ns,
        external: bool,
    ) -> hir_lang::PathId {
        let m = self.m;
        let full = m.strings.get(text as usize).map_or("", |s| &**s);
        self.t_at(f, ctx.expn);
        let origin = Origin::expanded(f.span, ctx.expn);
        let mut segs = Vec::new();
        if external {
            let sym = self.sym(text);
            segs.push(Segment::new(Name::marked(sym, ctx.expn), origin));
        } else {
            for piece in full.split("::") {
                let sym = self.intern(piece);
                segs.push(Segment::new(Name::marked(sym, ctx.expn), origin));
            }
        }
        let list = self.list(&segs);
        let mut p = Path::new(list, ns);
        if external {
            p.res = Res::Extern(self.sym(text));
        }
        self.path(p)
    }

    #[allow(clippy::too_many_arguments)]
    fn t_enter(
        &mut self,
        f: &Frame<K>,
        n: u32,
        c: u32,
        ctxs: &mut Vec<Ctx>,
        benv: &mut Vec<Option<BinderId>>,
        lenv: &mut Vec<Option<BinderId>>,
        work: &mut Vec<Work>,
        out: &mut Vec<Slot>,
        kids: &mut Vec<u32>,
        holes: &mut Holes,
    ) {
        let Some(ctx) = ctxs.get(c as usize).copied() else {
            return;
        };
        let m = self.m;
        let Some(t) = m.templates.get(ctx.tpl as usize) else {
            return;
        };
        let Some(node) = t.node(n).copied() else {
            return;
        };
        let text_base = t.text_base;
        let push = |out: &mut Vec<Slot>, v: Val| {
            out.push(Slot {
                v,
                span: f.span,
                place: false,
            });
        };
        match node {
            TNode::Hole(h)
            | TNode::Splice(h)
            | TNode::Stmts(h)
            | TNode::PHole(h)
            | TNode::THole(h) => match self.source(usize::from(h), c, ctxs) {
                Source::Top(h) => {
                    if self.present(f, h) {
                        self.top_hole(f, h, holes, out);
                    } else {
                        let tpl = ctxs.first().map_or(0, |c| c.tpl);
                        let v = self.absent(f, &ctx, tpl, h, holes);
                        push(out, v);
                    }
                }
                Source::Node(node, pc) => work.push(Work::Enter(node, pc)),
                Source::Absent(tpl, h) => {
                    let v = self.absent(f, &ctx, tpl, h, holes);
                    push(out, v);
                }
            },
            TNode::Lit(l) => {
                self.t_at(f, ctx.expn);
                let lit = self.t_lit(l);
                let e = self.ex(Expr::Lit(lit));
                push(out, Val::Expr(e));
            }
            TNode::Str(s) => {
                let m = self.m;
                let text = m.strings.get((text_base + s) as usize).map_or("", |s| &**s);
                self.t_at(f, ctx.expn);
                let tr = self.b.text(text);
                let e = self.ex(Expr::Lit(Lit::Str(tr)));
                push(out, Val::Expr(e));
            }
            TNode::Var(b) => {
                let id = self.t_binder(f, usize::from(b), &ctx, benv, BinderKind::Local);
                self.t_at(f, ctx.expn);
                let e = match id {
                    Some(id) => self.b.use_binder(id),
                    None => self.ex(Expr::Err),
                };
                self.nodes += 2;
                push(out, Val::Expr(e));
            }
            TNode::Global(s) => {
                let p = self.t_path(f, &ctx, text_base + s, Ns::Value, false);
                let e = self.ex(Expr::Path(p));
                push(out, Val::Expr(e));
            }
            TNode::Extern(s) => {
                let p = self.t_path(f, &ctx, text_base + s, Ns::Value, true);
                let e = self.ex(Expr::Path(p));
                push(out, Val::Expr(e));
            }
            TNode::PWild => {
                self.t_at(f, ctx.expn);
                let p = self.pt(Pat::Wild);
                push(out, Val::Pat(p));
            }
            TNode::PBind(b) => {
                let id = self.t_binder(f, usize::from(b), &ctx, benv, BinderKind::Local);
                self.t_at(f, ctx.expn);
                let p = match id {
                    Some(binder) => self.pt(Pat::Bind {
                        binder,
                        mode: BindMode::Value,
                        sub: None,
                    }),
                    None => self.pt(Pat::Err),
                };
                push(out, Val::Pat(p));
            }
            TNode::PLit(l) => {
                self.t_at(f, ctx.expn);
                let lit = self.t_lit(l);
                let p = self.pt(Pat::Lit(lit));
                push(out, Val::Pat(p));
            }
            TNode::TPrim(p) => {
                self.t_at(f, ctx.expn);
                let t = self.ty(Ty::Prim(p));
                push(out, Val::Ty(t));
            }
            TNode::TNamed(s) => {
                let p = self.t_path(f, &ctx, text_base + s, Ns::Type, false);
                let t = self.ty(Ty::Path(p));
                push(out, Val::Ty(t));
            }
            TNode::TInfer => {
                self.t_at(f, ctx.expn);
                let t = self.ty(Ty::Infer);
                push(out, Val::Ty(t));
            }
            TNode::TAny => {
                self.t_at(f, ctx.expn);
                let t = self.ty(Ty::Any);
                push(out, Val::Ty(t));
            }
            TNode::Continue { label } => {
                let label = self.slot_label(label, c, ctxs, lenv, holes);
                self.t_at(f, ctx.expn);
                let e = self.ex(Expr::Continue { label });
                push(out, Val::Expr(e));
            }
            TNode::Apply { template, args } => {
                let max = self.budget.max_expansion_depth;
                let callee = self.m.templates.get(template as usize);
                match callee {
                    Some(callee) if ctx.depth < max => {
                        let expn = self.b.expansion(Expansion {
                            kind: callee.kind,
                            name: self.sym(
                                self.m
                                    .template_names
                                    .get(template as usize)
                                    .copied()
                                    .unwrap_or(0),
                            ),
                            call_site: f.span,
                            parent: ctx.expn,
                            def_site: ExpnId::ROOT,
                        });
                        let benv_base = benv.len();
                        benv.resize(benv_base + callee.binders.len(), None);
                        let lenv_base = lenv.len();
                        lenv.resize(lenv_base + callee.labels.len(), None);
                        let root = callee.root;
                        let Ok(idx) = u32::try_from(ctxs.len()) else {
                            push(out, Val::Err);
                            return;
                        };
                        ctxs.push(Ctx {
                            tpl: template,
                            expn,
                            benv: benv_base,
                            lenv: lenv_base,
                            parent: c,
                            args,
                            depth: ctx.depth + 1,
                        });
                        work.push(Work::Enter(root, idx));
                    }
                    _ => {
                        self.report(
                            ProblemKind::ExpansionTooDeep {
                                template: TemplateId(template),
                            },
                            f.span,
                        );
                        self.t_at(f, ctx.expn);
                        let e = self.ex(Expr::Err);
                        push(out, Val::Expr(e));
                    }
                }
            }
            _ => {
                // Composite nodes: bind what must exist before the children
                // (labels, closure parameters), then evaluate the children in
                // order and build on exit.
                match node {
                    TNode::Block {
                        label: LabelSlot::Own(l),
                        ..
                    }
                    | TNode::Loop {
                        label: LabelSlot::Own(l),
                        ..
                    } => {
                        if let Some(decl) = t.labels.get(usize::from(l)).copied() {
                            let sym = self.sym(text_base + decl.name);
                            self.t_at(f, ctx.expn);
                            let b =
                                self.binder(Name::marked(sym, ctx.expn), BinderKind::Label, false);
                            if let Some(s) = lenv.get_mut(ctx.lenv + usize::from(l)) {
                                *s = Some(b);
                            }
                        }
                    }
                    TNode::Closure { params, .. } => {
                        for &p in t.list(params) {
                            let _ = self.t_binder(f, p as usize, &ctx, benv, BinderKind::Param);
                        }
                    }
                    _ => {}
                }
                work.push(Work::Exit(n, c));
                fill_children(t, &node, kids);
                for &k in kids.iter().rev() {
                    work.push(Work::Enter(k, c));
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn t_exit(
        &mut self,
        f: &Frame<K>,
        n: u32,
        c: u32,
        ctxs: &[Ctx],
        benv: &mut [Option<BinderId>],
        lenv: &[Option<BinderId>],
        out: &mut Vec<Slot>,
        kids: &mut Vec<u32>,
        res: &mut Vec<Slot>,
        holes: &mut Holes,
    ) {
        let Some(ctx) = ctxs.get(c as usize).copied() else {
            return;
        };
        let m = self.m;
        let Some(t) = m.templates.get(ctx.tpl as usize) else {
            return;
        };
        let Some(node) = t.node(n).copied() else {
            return;
        };
        let text_base = t.text_base;
        fill_children(t, &node, kids);
        let start = out.len().saturating_sub(kids.len());
        res.clear();
        res.extend_from_slice(out.get(start..).unwrap_or(&[]));
        out.truncate(start);
        let mut it = res.iter();
        let mut next = || it.next().map_or(Val::Err, |s| s.v);
        self.t_at(f, ctx.expn);
        let v = match node {
            TNode::Call { args, .. } => {
                let callee = next();
                let callee = self.t_expr(f, callee, holes);
                let mut list = Vec::new();
                for _ in 0..args.1 {
                    let a = next();
                    self.t_args(a, &mut list);
                }
                self.t_at(f, ctx.expn);
                let args = self.list(&list);
                Val::Expr(self.ex(Expr::Call { callee, args }))
            }
            TNode::Method { name, args, .. } => {
                let recv = next();
                let receiver = self.t_expr(f, recv, holes);
                let mut list = Vec::new();
                for _ in 0..args.1 {
                    let a = next();
                    self.t_args(a, &mut list);
                }
                self.t_at(f, ctx.expn);
                let args = self.list(&list);
                let method = Ident::new(self.sym(text_base + name), f.span);
                Val::Expr(self.ex(Expr::MethodCall {
                    receiver,
                    method,
                    generic_args: List::EMPTY,
                    args,
                }))
            }
            TNode::Field { name, .. } => {
                let b = next();
                let base = self.t_expr(f, b, holes);
                self.t_at(f, ctx.expn);
                let member = Member::Named(self.sym(text_base + name));
                Val::Expr(self.ex(Expr::Field {
                    base,
                    member,
                    span: f.span,
                }))
            }
            TNode::Index { .. } => {
                let (b, i) = (next(), next());
                let base = self.t_expr(f, b, holes);
                let index = self.t_expr(f, i, holes);
                self.t_at(f, ctx.expn);
                Val::Expr(self.ex(Expr::Index { base, index }))
            }
            TNode::Op { op, exact, args } => {
                let mut ops = Vec::new();
                for _ in 0..args.1 {
                    let a = next();
                    ops.push(self.t_expr(f, a, holes));
                }
                let op = if exact { op } else { self.policy_op(op.kind) };
                self.t_at(f, ctx.expn);
                let args = self.list(&ops);
                Val::Expr(self.ex(Expr::Op { op, args }))
            }
            TNode::Assign { .. } => {
                let (a, b) = (next(), next());
                let target = self.t_expr(f, a, holes);
                let value = self.t_expr(f, b, holes);
                self.t_at(f, ctx.expn);
                Val::Expr(self.ex(Expr::Assign {
                    target,
                    op: None,
                    value,
                }))
            }
            TNode::Block { stmts, tail, label } => {
                let mut list: Vec<StmtId> = Vec::new();
                for _ in 0..stmts.1 {
                    let s = next();
                    self.t_stmts(f, ctx.expn, s, &mut list);
                }
                let tail = if tail == NONE {
                    None
                } else {
                    let v = next();
                    self.t_opt_expr(v)
                };
                let label = self.slot_label(label, c, ctxs, lenv, holes);
                self.t_at(f, ctx.expn);
                let stmts = self.list(&list);
                Val::Expr(self.ex(Expr::Block(Block {
                    stmts,
                    tail,
                    label,
                    is_unsafe: false,
                })))
            }
            TNode::If { else_, .. } => {
                let (a, b) = (next(), next());
                let cond = self.t_expr(f, a, holes);
                let then = self.t_expr(f, b, holes);
                let else_ = if else_ == NONE {
                    None
                } else {
                    let v = next();
                    self.t_opt_expr(v)
                };
                self.t_at(f, ctx.expn);
                Val::Expr(self.ex(Expr::If { cond, then, else_ }))
            }
            TNode::Match { arms, .. } => {
                let s = next();
                let scrutinee = self.t_expr(f, s, holes);
                let mut list = Vec::new();
                for arm in t.list(arms).chunks(3) {
                    let guard = arm.get(1).copied().unwrap_or(NONE);
                    let p = next();
                    let pat = self.t_pat(f, ctx.expn, p);
                    let guard = if guard == NONE {
                        None
                    } else {
                        let g = next();
                        self.t_opt_expr(g)
                    };
                    let b = next();
                    let body = self.t_expr(f, b, holes);
                    list.push(Arm { pat, guard, body });
                }
                self.t_at(f, ctx.expn);
                let arms = self.list(&list);
                Val::Expr(self.ex(Expr::Match { scrutinee, arms }))
            }
            TNode::Loop { label, step, .. } => {
                let b = next();
                let body = self.t_expr(f, b, holes);
                let step = if step == NONE {
                    None
                } else {
                    let s = next();
                    self.t_opt_expr(s)
                };
                let label = self.slot_label(label, c, ctxs, lenv, holes);
                self.t_at(f, ctx.expn);
                Val::Expr(self.ex(Expr::Loop { label, body, step }))
            }
            TNode::Break { label, value } => {
                let value = if value == NONE {
                    None
                } else {
                    let v = next();
                    self.t_opt_expr(v)
                };
                let label = self.slot_label(label, c, ctxs, lenv, holes);
                self.t_at(f, ctx.expn);
                Val::Expr(self.ex(Expr::Break { label, value }))
            }
            TNode::Return(v) | TNode::Yield(v) => {
                let value = if v == NONE {
                    None
                } else {
                    let x = next();
                    self.t_opt_expr(x)
                };
                if matches!(node, TNode::Yield(_)) {
                    self.infer(hir_lang::Effects::YIELD);
                }
                self.t_at(f, ctx.expn);
                let e = if matches!(node, TNode::Yield(_)) {
                    Expr::Yield(value)
                } else {
                    Expr::Return(value)
                };
                Val::Expr(self.ex(e))
            }
            TNode::Throw(_) | TNode::Await(_) | TNode::Spawn(_) => {
                let x = next();
                let inner = self.t_expr(f, x, holes);
                self.t_at(f, ctx.expn);
                let e = match node {
                    TNode::Throw(_) => {
                        self.infer(hir_lang::Effects::THROWS);
                        Expr::Throw(inner)
                    }
                    TNode::Await(_) => {
                        self.infer(hir_lang::Effects::ASYNC);
                        Expr::Await(inner)
                    }
                    _ => Expr::Spawn(inner),
                };
                Val::Expr(self.ex(e))
            }
            TNode::Tuple(l) | TNode::Array(l) => {
                let mut list = Vec::new();
                for _ in 0..l.1 {
                    let a = next();
                    let mut args = Vec::new();
                    self.t_args(a, &mut args);
                    list.extend(args.iter().map(|a| a.value));
                }
                self.t_at(f, ctx.expn);
                let elems = self.list(&list);
                Val::Expr(self.ex(if matches!(node, TNode::Tuple(_)) {
                    Expr::Tuple(elems)
                } else {
                    Expr::Array(elems)
                }))
            }
            TNode::Closure {
                params, implicit, ..
            } => {
                let b = next();
                let body = self.t_expr(f, b, holes);
                let mut ps = Vec::new();
                for &p in t.list(params) {
                    let binder = benv.get(ctx.benv + p as usize).copied().flatten();
                    if let Some(binder) = binder {
                        self.t_at(f, ctx.expn);
                        let pat = self.pt(Pat::Bind {
                            binder,
                            mode: BindMode::Value,
                            sub: None,
                        });
                        self.nodes += 1;
                        ps.push(self.b.param(Param::new(pat)));
                    }
                }
                self.t_at(f, ctx.expn);
                let params = self.list(&ps);
                let captures: List<Capture> = List::EMPTY;
                Val::Expr(self.ex(Expr::Closure(Closure {
                    params,
                    ret: None,
                    body,
                    effects: hir_lang::Effects::NONE,
                    implicit,
                    captures,
                    self_binder: None,
                    defaults: DefaultEval::PerCall,
                })))
            }
            TNode::Let { ty, init, .. } => {
                let p = next();
                let pat = self.t_pat(f, ctx.expn, p);
                let ty = if ty == NONE {
                    None
                } else {
                    let v = next();
                    self.t_opt_ty(v)
                };
                let init = if init == NONE {
                    None
                } else {
                    let v = next();
                    self.t_opt_expr(v)
                };
                self.t_at(f, ctx.expn);
                Val::Stmt(self.st(Stmt::Let {
                    pat,
                    ty,
                    init,
                    else_: None,
                }))
            }
            TNode::ExprStmt(_) | TNode::Defer(_) => {
                let x = next();
                let e = self.t_expr(f, x, holes);
                self.t_at(f, ctx.expn);
                Val::Stmt(self.st(if matches!(node, TNode::Defer(_)) {
                    Stmt::Defer(e)
                } else {
                    Stmt::Expr(e)
                }))
            }
            TNode::PTuple(l) => {
                let mut list = Vec::new();
                for _ in 0..l.1 {
                    let p = next();
                    list.push(self.t_pat(f, ctx.expn, p));
                }
                self.t_at(f, ctx.expn);
                let elems = self.list(&list);
                Val::Pat(self.pt(Pat::Tuple { elems, rest: None }))
            }
            TNode::PCtor { path, elems } => {
                let mut list = Vec::new();
                for _ in 0..elems.1 {
                    let p = next();
                    list.push(self.t_pat(f, ctx.expn, p));
                }
                let path = self.t_path(f, &ctx, text_base + path, Ns::Pattern, false);
                self.t_at(f, ctx.expn);
                let elems = self.list(&list);
                Val::Pat(self.pt(Pat::Ctor {
                    path,
                    elems,
                    rest: None,
                }))
            }
            _ => Val::Err,
        };
        out.push(Slot {
            v,
            span: f.span,
            place: false,
        });
    }

    /// A required expression slot.
    fn t_expr(&mut self, f: &Frame<K>, v: Val, holes: &mut Holes) -> ExprId {
        match v {
            Val::Expr(e) => e,
            Val::Arg(a) => a.value,
            _ => {
                self.report_missing(f, holes);
                self.ex(Expr::Err)
            }
        }
    }

    fn t_opt_expr(&mut self, v: Val) -> Option<ExprId> {
        match v {
            Val::Expr(e) => Some(e),
            Val::Arg(a) => Some(a.value),
            _ => None,
        }
    }

    fn t_opt_ty(&mut self, v: Val) -> Option<TyId> {
        match v {
            Val::Ty(t) => Some(t),
            _ => None,
        }
    }

    fn t_pat(&mut self, f: &Frame<K>, expn: ExpnId, v: Val) -> PatId {
        match v {
            Val::Pat(p) => p,
            Val::Rest(_) => {
                self.report(
                    ProblemKind::NotAllowedHere {
                        expected: crate::Context::Pattern,
                    },
                    f.span,
                );
                self.t_at(f, expn);
                self.pt(Pat::Err)
            }
            _ => {
                self.t_at(f, expn);
                self.pt(Pat::Wild)
            }
        }
    }

    /// Arguments from a list element: an expression, or a spliced hole.
    fn t_args(&mut self, v: Val, out: &mut Vec<Arg>) {
        match v {
            Val::Expr(e) => out.push(Arg::positional(e)),
            Val::Arg(a) => out.push(a),
            Val::Spill(s, l) => {
                let (s, l) = (s as usize, l as usize);
                for j in s..s.saturating_add(l) {
                    match self.spill.get(j).map(|x| x.v) {
                        Some(Val::Arg(a)) => out.push(a),
                        Some(Val::Expr(e)) => out.push(Arg::positional(e)),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    /// Statements from a block element: a statement, or a spliced hole.
    fn t_stmts(&mut self, f: &Frame<K>, expn: ExpnId, v: Val, out: &mut Vec<StmtId>) {
        let mut one = |this: &mut Self, v: Val, span: Span| match v {
            Val::Stmt(s) => out.push(s),
            Val::Expr(e) => {
                this.b.set_origin(Origin::expanded(span, ExpnId::ROOT));
                out.push(this.st(Stmt::Expr(e)));
            }
            Val::Item(i) => {
                this.b.set_origin(Origin::expanded(span, ExpnId::ROOT));
                out.push(this.st(Stmt::Item(i)));
            }
            _ => {}
        };
        match v {
            Val::Spill(s, l) => {
                let (s, l) = (s as usize, l as usize);
                for j in s..s.saturating_add(l) {
                    if let Some(slot) = self.spill.get(j).copied() {
                        one(self, slot.v, slot.span);
                    }
                }
            }
            other => one(self, other, f.span),
        }
        self.t_at(f, expn);
    }

    /// Attaches the values of holes no expansion used (only when an `apply`
    /// was cut short by the depth budget), so no lowered node is orphaned:
    /// `{ unused...; result }`.
    fn rescue(&mut self, f: &Frame<K>, uses: &[u32], result: ExprId) -> ExprId {
        if uses.iter().all(|&u| u > 0) {
            return result;
        }
        let mut stmts: Vec<StmtId> = Vec::new();
        for (h, &uses) in uses.iter().enumerate() {
            if uses > 0 {
                continue;
            }
            let (s, e) = self.field_range(f, h);
            let mut vals: Vec<Slot> = Vec::new();
            for j in s..e {
                match self.vals.get(j).copied() {
                    Some(Slot {
                        v: Val::Spill(ss, sl),
                        ..
                    }) => {
                        let (ss, sl) = (ss as usize, sl as usize);
                        if let Some(run) = self.spill.get(ss..ss.saturating_add(sl)) {
                            vals.extend_from_slice(run);
                        }
                    }
                    Some(slot) => vals.push(slot),
                    None => {}
                }
            }
            for slot in vals {
                self.at(slot.span);
                let st = match slot.v {
                    Val::Stmt(s) => Some(s),
                    Val::Expr(e) => Some(self.st(Stmt::Expr(e))),
                    Val::Item(i) => Some(self.st(Stmt::Item(i))),
                    Val::Arg(a) => Some(self.st(Stmt::Expr(a.value))),
                    Val::Pat(pat) => Some(self.st(Stmt::Let {
                        pat,
                        ty: None,
                        init: None,
                        else_: None,
                    })),
                    Val::Ty(ty) => {
                        let pat = self.pt(Pat::Wild);
                        Some(self.st(Stmt::Let {
                            pat,
                            ty: Some(ty),
                            init: None,
                            else_: None,
                        }))
                    }
                    _ => None,
                };
                if let Some(st) = st {
                    stmts.push(st);
                }
            }
        }
        if stmts.is_empty() {
            return result;
        }
        self.at(f.span);
        let list = self.list(&stmts);
        self.ex(Expr::Block(Block {
            stmts: list,
            tail: Some(result),
            label: None,
            is_unsafe: false,
        }))
    }
}

/// Fills `v` with a template node's children in evaluation order (absent
/// optional children left out).
fn fill_children(t: &Template, node: &TNode, v: &mut Vec<u32>) {
    let opt = |x: u32, v: &mut Vec<u32>| {
        if x != NONE {
            v.push(x);
        }
    };
    v.clear();
    match *node {
        TNode::Call { callee, args } => {
            v.push(callee);
            v.extend_from_slice(t.list(args));
        }
        TNode::Method { recv, args, .. } => {
            v.push(recv);
            v.extend_from_slice(t.list(args));
        }
        TNode::Field { base, .. } => v.push(base),
        TNode::Index { base, index } => v.extend([base, index]),
        TNode::Op { args, .. } => v.extend_from_slice(t.list(args)),
        TNode::Assign { target, value } => v.extend([target, value]),
        TNode::Block { stmts, tail, .. } => {
            v.extend_from_slice(t.list(stmts));
            opt(tail, v);
        }
        TNode::If { cond, then, else_ } => {
            v.extend([cond, then]);
            opt(else_, v);
        }
        TNode::Match { scrutinee, arms } => {
            v.push(scrutinee);
            for arm in t.list(arms).chunks(3) {
                for &x in arm {
                    opt(x, v);
                }
            }
        }
        TNode::Loop { body, step, .. } => {
            v.push(body);
            opt(step, v);
        }
        TNode::Break { value, .. } => opt(value, v),
        TNode::Return(x) | TNode::Yield(x) => opt(x, v),
        TNode::Throw(x)
        | TNode::Await(x)
        | TNode::Spawn(x)
        | TNode::ExprStmt(x)
        | TNode::Defer(x) => {
            v.push(x);
        }
        TNode::Tuple(l) | TNode::Array(l) | TNode::PTuple(l) => v.extend_from_slice(t.list(l)),
        TNode::PCtor { elems, .. } => v.extend_from_slice(t.list(elems)),
        TNode::Closure { body, .. } => v.push(body),
        TNode::Let { pat, ty, init } => {
            v.push(pat);
            opt(ty, v);
            opt(init, v);
        }
        _ => {}
    }
}
