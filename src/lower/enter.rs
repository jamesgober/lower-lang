//! Entering a node: check its context, read its synchronous parts (names,
//! labels, modifiers, operators), open a frame, and schedule its fields.

use alloc::vec::Vec;

use hir_lang::{BinderKind, CaptureMode, Effects, Name, Ns, Pat, PathRoot, Span};
use intern_lang::Symbol;
use syntax_lang::{Element, Node, TokenKind};

use super::{Build, Frame, PatSite, Run, Task, Val, Want};
use crate::arch::Arch;
use crate::mapping::{Fixity, Modifier, OpSpec, TokenRole};
use crate::pick::{self, Pick};
use crate::problem::ProblemKind;
use crate::template::{HoleKind, TemplateId};

/// Whether a context takes a pattern, and with which binder site.
pub(crate) const fn site_of(want: Want) -> Option<PatSite> {
    match want {
        Want::Pat(site) => Some(site),
        Want::Param => Some(PatSite {
            param: true,
            mutable: false,
            arm: false,
            bound: false,
        }),
        _ => None,
    }
}

const fn has(mods: u32, m: Modifier) -> bool {
    mods & m.bit() != 0
}

impl<'t, K> Run<'_, 't, K>
where
    K: Copy + Ord + TokenKind,
{
    /// Resolves a single pick against the current node's children.
    fn pick1(&self, p: Pick<K>) -> Option<&'t Element<K>> {
        pick::single(p, &self.kids, self.m)
    }

    /// The name a pick finds, if any.
    fn opt_name(&mut self, p: Pick<K>) -> Option<(Symbol, Span)> {
        let e = self.pick1(p)?;
        self.name_of(e)
    }

    /// The name a pick finds; a missing one is reported and replaced by the
    /// empty name, so the construct (and everything inside it) still lowers.
    fn req_name(&mut self, p: Pick<K>, span: Span) -> (Symbol, Span) {
        match self.opt_name(p) {
            Some(n) => n,
            None => {
                self.report(ProblemKind::MissingField { field: "name" }, span);
                (self.empty, span)
            }
        }
    }

    /// A plain variable or identifier element (through `Pass` wrappers), for
    /// hoisting and implicit declarations.
    fn simple_var(&mut self, e: Option<&'t Element<K>>) -> Option<(Symbol, Span)> {
        let e = e?;
        if let Element::Token(t) = e {
            if !matches!(
                self.m.role_for(t.kind()),
                Some(TokenRole::Name | TokenRole::Var { .. })
            ) {
                return None;
            }
        }
        match e {
            Element::Token(_) => self.name_of(e),
            Element::Node(n) => match self.m.rule_for(n.kind()) {
                Some(Arch::Pass { .. }) => self.name_of(e),
                _ => None,
            },
        }
    }

    /// A label read from a pick: a fresh `Label` binder, pushed on the label
    /// scope (popped when the frame finishes).
    fn def_label(&mut self, p: Pick<K>) -> Option<hir_lang::BinderId> {
        let (sym, span) = self.opt_name(p)?;
        self.at(span);
        let b = self.binder(Name::new(sym), BinderKind::Label, false);
        self.push_label(sym, b);
        Some(b)
    }

    /// Opens a frame around `node` whose single field is `node` itself in
    /// another context (conversions).
    fn wrap(&mut self, mut frame: Frame<K>, node: &'t Node<K>, inner: Want) {
        frame.counts_base = self.counts.len();
        frame.nfields = 1;
        frame.vals_base = self.vals.len();
        frame.spill_base = self.spill.len();
        self.counts.push(1);
        self.frames.push(frame);
        self.tasks.push(Task::Finish);
        self.tasks.push(Task::Node(node, inner));
    }

    pub(crate) fn enter(&mut self, node: &'t Node<K>, arch: Arch<K>, want: Want) {
        let span = node.span();
        if want == Want::Root && !matches!(arch, Arch::Module { .. }) {
            // The root is not a module: wrap it in one, lowering it as the
            // module's only body element (or item).
            let mut f = self.frame(arch, want, span);
            f.build = Build::RootWrap;
            self.push_fx(Effects::NONE);
            f.fx = true;
            return self.wrap(f, node, Want::Stmt);
        }
        let mut f = self.frame(arch, want, span);
        f.mods = self.mods();
        let site = site_of(want);
        let expr_ok = want.takes_expr();
        let item_ok = matches!(want, Want::Root | Want::Stmt | Want::Member);
        let stmt_ok = want == Want::Stmt;
        let ty_ok = matches!(want, Want::Ty | Want::GArg);
        let pmut = |site: PatSite, mods: u32| PatSite {
            mutable: site.mutable || has(mods, Modifier::Mut),
            ..site
        };
        match arch {
            // ------------------------------------------------------ structure
            Arch::Pass { inner } => match self.pick1(inner) {
                Some(e) => self.tasks.push(Task::Elem(e, want)),
                None => {
                    self.report(ProblemKind::MissingField { field: "inner" }, span);
                    let v = self.error_val(want, span);
                    self.push(v, span);
                }
            },
            Arch::Unsupported => {
                self.report(ProblemKind::Unsupported, span);
                let v = self.error_val(want, span);
                self.push(v, span);
            }
            Arch::Splice { items } => {
                let list_ctx = matches!(
                    want,
                    Want::Stmt
                        | Want::Member
                        | Want::Param
                        | Want::Arg
                        | Want::Arm
                        | Want::Capture
                        | Want::Field
                        | Want::Variant
                        | Want::Generic
                        | Want::GArg
                        | Want::Where
                        | Want::Entry
                        | Want::Init
                        | Want::Segment
                );
                if list_ctx {
                    self.plan(f, &[(items, want, true)]);
                } else {
                    self.not_here(want, span);
                }
            }
            Arch::Module { name, items } => {
                if !item_ok {
                    return self.not_here(want, span);
                }
                f.name = self.opt_name(name);
                self.push_fx(Effects::NONE);
                f.fx = true;
                self.plan(f, &[(items, Want::Stmt, true)]);
            }

            // ------------------------------------------- statements, binders
            Arch::Let {
                pat,
                ty,
                init,
                else_,
                mutable,
                hoist,
            } => {
                if !stmt_ok {
                    return self.not_here(want, span);
                }
                if hoist {
                    let target = self.pick1(pat);
                    if let Some((sym, name_span)) = self.simple_var(target) {
                        self.hoist_name(sym, name_span);
                        f.build = Build::HoistedLet {
                            name: sym,
                            name_span,
                        };
                        return self.plan(f, &[(init, Want::Expr, false)]);
                    }
                }
                let site = PatSite {
                    param: false,
                    mutable: mutable || has(f.mods, Modifier::Mut),
                    arm: false,
                    bound: false,
                };
                self.plan(
                    f,
                    &[
                        (pat, Want::Pat(site), false),
                        (ty, Want::Ty, false),
                        (init, Want::Expr, false),
                        (else_, Want::Expr, false),
                    ],
                );
            }
            Arch::ExprStmt { expr } => {
                if !stmt_ok {
                    return self.not_here(want, span);
                }
                self.plan(f, &[(expr, Want::Expr, false)]);
            }
            Arch::Defer { body } => {
                if !stmt_ok {
                    return self.not_here(want, span);
                }
                self.plan(f, &[(body, Want::Expr, false)]);
            }
            Arch::Static { name, ty, init } => {
                if !stmt_ok {
                    return self.not_here(want, span);
                }
                let n = self.req_name(name, span);
                self.declare_name(n.0);
                f.name = Some(n);
                self.plan(f, &[(ty, Want::Ty, false), (init, Want::Expr, false)]);
            }
            Arch::GlobalDecl { names } => {
                if !stmt_ok {
                    return self.not_here(want, span);
                }
                self.global_decl(names, span);
            }
            Arch::Import { path, alias } => {
                if !item_ok {
                    return self.not_here(want, span);
                }
                f.name = self.opt_name(alias);
                self.plan(f, &[(path, Want::Path(Ns::Import), false)]);
            }

            // --------------------------------------------------- declarations
            Arch::Fn {
                name,
                generics,
                params,
                ret,
                throws,
                body,
                where_,
            } => {
                if !item_ok {
                    return self.not_here(want, span);
                }
                f.name = Some(self.req_name(name, span));
                let declared = declared_effects(f.mods);
                self.push_fx(declared);
                f.fx = true;
                self.plan(
                    f,
                    &[
                        (generics, Want::Generic, true),
                        (where_, Want::Where, true),
                        (params, Want::Param, true),
                        (ret, Want::Ty, false),
                        (throws, Want::Ty, false),
                        (body, Want::Expr, false),
                    ],
                );
            }
            Arch::Record {
                name,
                generics,
                fields,
                ..
            } => {
                if !item_ok {
                    return self.not_here(want, span);
                }
                f.name = Some(self.req_name(name, span));
                self.plan(
                    f,
                    &[(generics, Want::Generic, true), (fields, Want::Field, true)],
                );
            }
            Arch::Sum {
                name,
                generics,
                variants,
            } => {
                if !item_ok {
                    return self.not_here(want, span);
                }
                f.name = Some(self.req_name(name, span));
                self.plan(
                    f,
                    &[
                        (generics, Want::Generic, true),
                        (variants, Want::Variant, true),
                    ],
                );
            }
            Arch::Interface {
                name,
                generics,
                supers,
                items,
            } => {
                if !item_ok {
                    return self.not_here(want, span);
                }
                f.name = Some(self.req_name(name, span));
                self.plan(
                    f,
                    &[
                        (generics, Want::Generic, true),
                        (supers, Want::Ty, true),
                        (items, Want::Member, true),
                    ],
                );
            }
            Arch::Class {
                name,
                generics,
                bases,
                interfaces,
                members,
                ..
            } => {
                if !item_ok {
                    return self.not_here(want, span);
                }
                f.name = Some(self.req_name(name, span));
                self.plan(
                    f,
                    &[
                        (generics, Want::Generic, true),
                        (bases, Want::Ty, true),
                        (interfaces, Want::Ty, true),
                        (members, Want::Member, true),
                    ],
                );
            }
            Arch::Impl {
                generics,
                interface,
                self_ty,
                items,
            } => {
                if !item_ok {
                    return self.not_here(want, span);
                }
                self.plan(
                    f,
                    &[
                        (generics, Want::Generic, true),
                        (interface, Want::Ty, false),
                        (self_ty, Want::Ty, false),
                        (items, Want::Member, true),
                    ],
                );
            }
            Arch::Alias { name, generics, ty } => {
                if !item_ok {
                    return self.not_here(want, span);
                }
                f.name = Some(self.req_name(name, span));
                self.plan(f, &[(generics, Want::Generic, true), (ty, Want::Ty, false)]);
            }
            Arch::AssocType {
                name,
                bounds,
                default,
            } => {
                if want != Want::Member {
                    return self.not_here(want, span);
                }
                f.name = Some(self.req_name(name, span));
                self.plan(f, &[(bounds, Want::GArg, true), (default, Want::Ty, false)]);
            }
            Arch::Const { name, ty, value } => {
                if !item_ok {
                    return self.not_here(want, span);
                }
                f.name = Some(self.req_name(name, span));
                self.plan(f, &[(ty, Want::Ty, false), (value, Want::Expr, false)]);
            }
            Arch::GlobalVar { name, ty, init, .. } => {
                if !item_ok {
                    return self.not_here(want, span);
                }
                f.name = Some(self.req_name(name, span));
                self.plan(f, &[(ty, Want::Ty, false), (init, Want::Expr, false)]);
            }
            Arch::MixinUse { mixins } => {
                if want != Want::Member {
                    return self.not_here(want, span);
                }
                self.plan(f, &[(mixins, Want::Ty, true)]);
            }
            Arch::FieldDecl { name, ty, default } => {
                if !matches!(want, Want::Field | Want::Member) {
                    return self.not_here(want, span);
                }
                f.name = self.opt_name(name);
                self.plan(f, &[(ty, Want::Ty, false), (default, Want::Expr, false)]);
            }
            Arch::VariantDecl {
                name,
                fields,
                discriminant,
                ..
            } => {
                if want != Want::Variant {
                    return self.not_here(want, span);
                }
                f.name = Some(self.req_name(name, span));
                self.plan(
                    f,
                    &[
                        (fields, Want::Field, true),
                        (discriminant, Want::Expr, false),
                    ],
                );
            }
            Arch::GenericParam {
                name,
                bounds,
                ty,
                default,
            } => {
                if want != Want::Generic {
                    return self.not_here(want, span);
                }
                let name_elem = self.pick1(name);
                let lifetime = matches!(
                    name_elem,
                    Some(Element::Token(t)) if matches!(self.m.role_for(t.kind()), Some(TokenRole::Lifetime { .. }))
                );
                f.name = Some(self.req_name(name, span));
                f.flag = lifetime;
                self.plan(
                    f,
                    &[
                        (bounds, Want::GArg, true),
                        (ty, Want::Ty, false),
                        (default, Want::GArg, false),
                    ],
                );
            }
            Arch::WherePred { subject, bounds } => {
                if want != Want::Where {
                    return self.not_here(want, span);
                }
                self.plan(
                    f,
                    &[(subject, Want::GArg, false), (bounds, Want::GArg, true)],
                );
            }
            Arch::Param { pat, ty, default } => {
                if want != Want::Param {
                    return self.not_here(want, span);
                }
                let site = PatSite {
                    param: true,
                    mutable: has(f.mods, Modifier::Mut),
                    arm: false,
                    bound: false,
                };
                self.plan(
                    f,
                    &[
                        (pat, Want::Pat(site), false),
                        (ty, Want::Ty, false),
                        (default, Want::Expr, false),
                    ],
                );
            }
            Arch::Capture { name } => {
                if want != Want::Capture {
                    return self.not_here(want, span);
                }
                let (sym, nspan) = self.req_name(name, span);
                let mode = match (has(f.mods, Modifier::Ref), has(f.mods, Modifier::Mut)) {
                    (true, true) => CaptureMode::ByMutRef,
                    (true, false) => CaptureMode::ByRef,
                    _ => CaptureMode::ByValue,
                };
                let v = self.capture(sym, nspan, mode);
                self.push(v, span);
            }

            // ----------------------------------------------------- expressions
            Arch::Path {
                segments,
                global,
                qself,
                qtrait,
            } => {
                let ok = expr_ok
                    || ty_ok
                    || site.is_some()
                    || matches!(want, Want::Path(_) | Want::Segment);
                if !ok {
                    return self.not_here(want, span);
                }
                self.enter_path(f, segments, global, qself, qtrait);
            }
            Arch::Segment { name, args } => {
                let ok = expr_ok
                    || ty_ok
                    || site.is_some()
                    || matches!(want, Want::Path(_) | Want::Segment);
                if !ok {
                    return self.not_here(want, span);
                }
                f.name = Some(self.req_name(name, span));
                self.plan(f, &[(args, Want::GArg, true)]);
            }
            Arch::AssocBinding { name, ty } => {
                if want != Want::GArg {
                    return self.not_here(want, span);
                }
                f.name = Some(self.req_name(name, span));
                self.plan(f, &[(ty, Want::Ty, false)]);
            }
            Arch::Unary { op, operand } => self.enter_op(node, f, Fixity::Prefix, op, &[operand]),
            Arch::Postfix { operand, op } => {
                self.enter_op(node, f, Fixity::Postfix, op, &[operand])
            }
            Arch::Binary { lhs, op, rhs } => {
                self.enter_op(node, f, Fixity::Binary, op, &[lhs, rhs])
            }
            Arch::Assign { target, op, value } => {
                if !expr_ok {
                    return self.not_here(want, span);
                }
                if !op.is_absent() {
                    match self.pick1(op) {
                        Some(Element::Token(t)) => match self.m.op(Fixity::Binary, t.kind()) {
                            Some(
                                spec @ (OpSpec::Compound(_)
                                | OpSpec::CompoundWith(_)
                                | OpSpec::Assign),
                            ) => {
                                f.build = Build::Op(spec);
                            }
                            _ => {
                                self.report(ProblemKind::UnknownOperator, t.span());
                                let e = self.err_expr(span);
                                let v = self.adapt_expr(e, false, want, span);
                                self.push(v, span);
                                return;
                            }
                        },
                        _ => f.build = Build::Op(OpSpec::Assign),
                    }
                } else {
                    f.build = Build::Op(OpSpec::Assign);
                }
                self.implicit_local(target);
                self.plan(
                    f,
                    &[(target, Want::Expr, false), (value, Want::Expr, false)],
                );
            }
            Arch::RefAssign { target, source } => {
                if !expr_ok {
                    return self.not_here(want, span);
                }
                self.implicit_local(target);
                self.plan(
                    f,
                    &[(target, Want::Expr, false), (source, Want::Expr, false)],
                );
            }
            Arch::Call { callee, args } => {
                if let Some(site) = site {
                    let fields = [
                        (callee, Want::Path(Ns::Pattern), false),
                        (args, Want::Pat(site), true),
                    ];
                    return self.plan(f, &fields);
                }
                if !expr_ok {
                    return self.not_here(want, span);
                }
                self.plan(f, &[(callee, Want::Expr, false), (args, Want::Arg, true)]);
            }
            Arch::Arg { name, value } => {
                if want != Want::Arg {
                    return self.not_here(want, span);
                }
                f.name = self.opt_name(name);
                self.plan(f, &[(value, Want::Expr, false)]);
            }
            Arch::Member {
                base,
                member,
                args,
                call,
            } => {
                if !expr_ok {
                    return self.not_here(want, span);
                }
                self.member_name(&mut f, member, span);
                let is_call = !call.is_absent() && self.pick1(call).is_some();
                if is_call {
                    f.flag = true;
                    self.plan(f, &[(base, Want::Expr, false), (args, Want::Arg, true)]);
                } else {
                    self.plan(f, &[(base, Want::Expr, false)]);
                }
            }
            Arch::MethodCall {
                receiver,
                method,
                generics,
                args,
            } => {
                if !expr_ok {
                    return self.not_here(want, span);
                }
                f.name = Some(self.req_name(method, span));
                self.plan(
                    f,
                    &[
                        (receiver, Want::Expr, false),
                        (generics, Want::GArg, true),
                        (args, Want::Arg, true),
                    ],
                );
            }
            Arch::DynMember {
                base,
                name,
                args,
                call,
            } => {
                if !expr_ok {
                    return self.not_here(want, span);
                }
                f.flag = !call.is_absent() && self.pick1(call).is_some();
                self.plan(
                    f,
                    &[
                        (base, Want::Expr, false),
                        (name, Want::Expr, false),
                        (args, Want::Arg, true),
                    ],
                );
            }
            Arch::Index { base, index } => {
                if !expr_ok {
                    return self.not_here(want, span);
                }
                self.plan(f, &[(base, Want::Expr, false), (index, Want::Expr, false)]);
            }
            Arch::VarVar { inner } => {
                if !expr_ok {
                    return self.not_here(want, span);
                }
                self.plan(f, &[(inner, Want::Expr, false)]);
            }
            Arch::Append { base } => {
                if !expr_ok {
                    return self.not_here(want, span);
                }
                self.plan(f, &[(base, Want::Expr, false)]);
            }
            Arch::Tuple { elems } => {
                if let Some(site) = site {
                    return self.plan(f, &[(elems, Want::Pat(site), true)]);
                }
                if ty_ok {
                    return self.plan(f, &[(elems, Want::Ty, true)]);
                }
                if !expr_ok {
                    return self.not_here(want, span);
                }
                self.plan(f, &[(elems, Want::Expr, true)]);
            }
            Arch::Array { elems } => {
                if let Some(site) = site {
                    return self.plan(f, &[(elems, Want::Pat(site), true)]);
                }
                if ty_ok {
                    return self.plan(f, &[(elems, Want::Ty, true)]);
                }
                if !expr_ok {
                    return self.not_here(want, span);
                }
                self.plan(f, &[(elems, Want::Expr, true)]);
            }
            Arch::Map { entries } => {
                if !expr_ok {
                    return self.not_here(want, span);
                }
                self.plan(f, &[(entries, Want::Entry, true)]);
            }
            Arch::MapEntry { key, value } => {
                if want != Want::Entry {
                    return self.not_here(want, span);
                }
                self.plan(f, &[(key, Want::Expr, false), (value, Want::Expr, false)]);
            }
            Arch::RecordLit { path, fields, base } => {
                if let Some(site) = site {
                    return self.plan(
                        f,
                        &[
                            (path, Want::Path(Ns::Type), false),
                            (fields, Want::FieldPat(site), true),
                        ],
                    );
                }
                if !expr_ok {
                    return self.not_here(want, span);
                }
                self.plan(
                    f,
                    &[
                        (path, Want::Path(Ns::Type), false),
                        (fields, Want::Init, true),
                        (base, Want::Expr, false),
                    ],
                );
            }
            Arch::FieldInit { name, value } => match want {
                Want::Init => {
                    f.name = Some(self.req_name(name, span));
                    self.plan(f, &[(value, Want::Expr, false)]);
                }
                Want::FieldPat(site) => {
                    f.name = Some(self.req_name(name, span));
                    self.plan(f, &[(value, Want::Pat(site), false)]);
                }
                _ => self.not_here(want, span),
            },
            Arch::Repeat { elem, count } => {
                if !expr_ok {
                    return self.not_here(want, span);
                }
                self.plan(f, &[(elem, Want::Expr, false), (count, Want::Expr, false)]);
            }
            Arch::Cast { expr, ty } => {
                if !expr_ok {
                    return self.not_here(want, span);
                }
                self.plan(f, &[(expr, Want::Expr, false), (ty, Want::Ty, false)]);
            }
            Arch::Borrow { expr } => {
                if !expr_ok {
                    return self.not_here(want, span);
                }
                self.plan(f, &[(expr, Want::Expr, false)]);
            }
            Arch::Deref { inner } => {
                if !expr_ok {
                    return self.not_here(want, span);
                }
                self.plan(f, &[(inner, Want::Expr, false)]);
            }
            Arch::Block {
                stmts, tail, label, ..
            } => {
                if !expr_ok {
                    return self.not_here(want, span);
                }
                f.binder = self.def_label(label);
                self.plan(f, &[(stmts, Want::Stmt, true), (tail, Want::Expr, false)]);
            }
            Arch::If { cond, then, else_ } => {
                if !expr_ok {
                    return self.not_here(want, span);
                }
                self.plan(
                    f,
                    &[
                        (cond, Want::Expr, false),
                        (then, Want::Expr, false),
                        (else_, Want::Expr, false),
                    ],
                );
            }
            Arch::Match { scrutinee, arms } => {
                if !expr_ok {
                    return self.not_here(want, span);
                }
                self.plan(
                    f,
                    &[(scrutinee, Want::Expr, false), (arms, Want::Arm, true)],
                );
            }
            Arch::Arm { pat, guard, body } => {
                if want != Want::Arm {
                    return self.not_here(want, span);
                }
                self.plan(
                    f,
                    &[
                        (pat, Want::Pat(PatSite::ARM), false),
                        (guard, Want::Expr, false),
                        (body, Want::Expr, false),
                    ],
                );
            }
            Arch::Loop { label, body } => {
                if !expr_ok {
                    return self.not_here(want, span);
                }
                f.binder = self.def_label(label);
                self.plan(f, &[(body, Want::Expr, false)]);
            }
            Arch::Break { label, value } => {
                if !expr_ok {
                    return self.not_here(want, span);
                }
                if !self.jump_label(&mut f, label, want) {
                    return;
                }
                self.plan(f, &[(value, Want::Expr, false)]);
            }
            Arch::Continue { label } => {
                if !expr_ok {
                    return self.not_here(want, span);
                }
                if !self.jump_label(&mut f, label, want) {
                    return;
                }
                self.plan(f, &[]);
            }
            Arch::Return { value } => {
                if !expr_ok {
                    return self.not_here(want, span);
                }
                self.plan(f, &[(value, Want::Expr, false)]);
            }
            Arch::Throw { value } => {
                if !expr_ok {
                    return self.not_here(want, span);
                }
                self.infer(Effects::THROWS);
                self.plan(f, &[(value, Want::Expr, false)]);
            }
            Arch::Try {
                body,
                catches,
                finally,
            } => {
                if !expr_ok {
                    return self.not_here(want, span);
                }
                self.plan(
                    f,
                    &[
                        (body, Want::Expr, false),
                        (catches, Want::Arm, true),
                        (finally, Want::Expr, false),
                    ],
                );
            }
            Arch::Catch { types, name, body } => {
                if want != Want::Arm {
                    return self.not_here(want, span);
                }
                f.name = self.opt_name(name);
                self.plan(f, &[(types, Want::Ty, true), (body, Want::Expr, false)]);
            }
            Arch::Await { inner } => {
                if !expr_ok {
                    return self.not_here(want, span);
                }
                self.infer(Effects::ASYNC);
                self.plan(f, &[(inner, Want::Expr, false)]);
            }
            Arch::Yield { value } => {
                if !expr_ok {
                    return self.not_here(want, span);
                }
                self.infer(Effects::YIELD);
                self.plan(f, &[(value, Want::Expr, false)]);
            }
            Arch::Spawn { inner } => {
                if !expr_ok {
                    return self.not_here(want, span);
                }
                self.plan(f, &[(inner, Want::Expr, false)]);
            }
            Arch::Closure {
                params,
                ret,
                captures,
                body,
                ..
            } => {
                if !expr_ok {
                    return self.not_here(want, span);
                }
                self.push_fx(declared_effects(f.mods));
                f.fx = true;
                self.plan(
                    f,
                    &[
                        (captures, Want::Capture, true),
                        (params, Want::Param, true),
                        (ret, Want::Ty, false),
                        (body, Want::Expr, false),
                    ],
                );
            }
            Arch::Template(used) => {
                let first = used.first as usize;
                let m = self.m;
                let picks = m
                    .hole_picks
                    .get(first..first.saturating_add(used.len as usize))
                    .unwrap_or(&[]);
                self.enter_template(f, used.template, picks);
            }

            // -------------------------------------------------------- patterns
            Arch::WildPat => match site {
                Some(_) => {
                    self.at(span);
                    let p = self.pt(Pat::Wild);
                    let v = self.adapt_pat(p, want, span);
                    self.push(v, span);
                }
                None => self.not_here(want, span),
            },
            Arch::RestPat { bind } => match want {
                Want::Pat(site) | Want::FieldPat(site) => {
                    self.plan(f, &[(bind, Want::Pat(site), false)])
                }
                _ => self.not_here(want, span),
            },
            Arch::BindPat { name, sub } => {
                let Some(site) = site else {
                    return self.not_here(want, span);
                };
                let site = pmut(site, f.mods);
                let (sym, nspan) = self.req_name(name, span);
                f.binder = Some(self.pat_binder(sym, nspan, site));
                self.plan(f, &[(sub, Want::Pat(site), false)]);
            }
            Arch::TuplePat { elems } | Arch::SlicePat { elems } => {
                let Some(site) = site else {
                    return self.not_here(want, span);
                };
                self.plan(f, &[(elems, Want::Pat(site), true)]);
            }
            Arch::CtorPat { path, elems } => {
                let Some(site) = site else {
                    return self.not_here(want, span);
                };
                self.plan(
                    f,
                    &[
                        (path, Want::Path(Ns::Pattern), false),
                        (elems, Want::Pat(site), true),
                    ],
                );
            }
            Arch::RecordPat { path, fields } => {
                let Some(site) = site else {
                    return self.not_here(want, span);
                };
                self.plan(
                    f,
                    &[
                        (path, Want::Path(Ns::Type), false),
                        (fields, Want::FieldPat(site), true),
                    ],
                );
            }
            Arch::FieldPat { name, pat } => {
                let Want::FieldPat(site) = want else {
                    return self.not_here(want, span);
                };
                f.name = Some(self.req_name(name, span));
                self.plan(f, &[(pat, Want::Pat(site), false)]);
            }
            Arch::OrPat { alts } => {
                let Some(site) = site else {
                    return self.not_here(want, span);
                };
                if self.or_depth == 0 {
                    self.or_map.clear();
                }
                self.or_depth += 1;
                f.or = true;
                self.plan(f, &[(alts, Want::Pat(site), true)]);
            }
            Arch::RangePat { lo, hi } => {
                let Some(site) = site else {
                    return self.not_here(want, span);
                };
                let bound = PatSite {
                    bound: true,
                    ..site
                };
                self.plan(
                    f,
                    &[(lo, Want::Pat(bound), false), (hi, Want::Pat(bound), false)],
                );
            }
            Arch::RefPat { inner } => {
                let Some(site) = site else {
                    return self.not_here(want, span);
                };
                self.plan(f, &[(inner, Want::Pat(site), false)]);
            }
            Arch::TypeTestPat { ty, pat } => {
                let Some(site) = site else {
                    return self.not_here(want, span);
                };
                self.plan(f, &[(ty, Want::Ty, false), (pat, Want::Pat(site), false)]);
            }

            // ----------------------------------------------------------- types
            Arch::TupleTy { elems } => {
                if !ty_ok {
                    return self.not_here(want, span);
                }
                self.plan(f, &[(elems, Want::Ty, true)]);
            }
            Arch::ArrayTy { elem, len } => {
                if !ty_ok {
                    return self.not_here(want, span);
                }
                self.plan(f, &[(elem, Want::Ty, false), (len, Want::Expr, false)]);
            }
            Arch::SliceTy { elem } => {
                if !ty_ok {
                    return self.not_here(want, span);
                }
                self.plan(f, &[(elem, Want::Ty, false)]);
            }
            Arch::RefTy { inner, region } => {
                if !ty_ok {
                    return self.not_here(want, span);
                }
                self.plan(
                    f,
                    &[
                        (inner, Want::Ty, false),
                        (region, Want::Path(Ns::Region), false),
                    ],
                );
            }
            Arch::PtrTy { inner } | Arch::NullableTy { inner } => {
                if !ty_ok {
                    return self.not_here(want, span);
                }
                self.plan(f, &[(inner, Want::Ty, false)]);
            }
            Arch::FnTy { params, ret } => {
                if !ty_ok {
                    return self.not_here(want, span);
                }
                self.plan(f, &[(params, Want::Ty, true), (ret, Want::Ty, false)]);
            }
            Arch::ObjectTy { bounds } | Arch::ImplTy { bounds } => {
                if !ty_ok {
                    return self.not_here(want, span);
                }
                self.plan(f, &[(bounds, Want::GArg, true)]);
            }
        }
    }

    /// `global $a, $b;`: one `Stmt::Global` per name, spilled.
    fn global_decl(&mut self, names: Pick<K>, span: Span) {
        let mut elems = Vec::new();
        pick::list(names, &self.kids, self.m, &mut elems);
        let start = self.spill.len();
        for e in elems {
            let Some((sym, nspan)) = self.name_of(e) else {
                self.report(ProblemKind::MissingField { field: "name" }, e.span());
                continue;
            };
            self.declare_name(sym);
            let path = self.name_path(sym, nspan, Ns::Value);
            let binder = self.binder(Name::new(sym), BinderKind::Local, true);
            let s = self.st(hir_lang::Stmt::Global { binder, path });
            self.spill.push(super::Slot {
                v: Val::Stmt(s),
                span: nspan,
                place: false,
            });
        }
        let len = self.spill.len() - start;
        let v = Val::Spill(
            u32::try_from(start).unwrap_or(0),
            u32::try_from(len).unwrap_or(0),
        );
        self.push(v, span);
    }

    /// Records an implicitly declared local (PHP) for a plain variable target.
    fn implicit_local(&mut self, target: Pick<K>) {
        if !self.m.policy.implicit_locals {
            return;
        }
        let e = self.pick1(target);
        if let Some((sym, span)) = self.simple_var(e) {
            self.hoist_name(sym, span);
        }
    }

    /// Reads a member name (a name, or a tuple index) into the frame.
    fn member_name(&mut self, f: &mut Frame<K>, member: Pick<K>, span: Span) {
        let Some(e) = self.pick1(member) else {
            self.report(ProblemKind::MissingField { field: "member" }, span);
            f.name = Some((self.empty, span));
            return;
        };
        if let Element::Token(t) = e {
            if matches!(
                self.m.role_for(t.kind()),
                Some(TokenRole::Int | TokenRole::Number)
            ) {
                let text = self.text_of(t.span());
                if let Ok(i) = text.parse::<u32>() {
                    f.index = Some(i);
                    f.name = Some((self.empty, t.span()));
                    return;
                }
                self.report(ProblemKind::InvalidLiteral, t.span());
            }
        }
        f.name = Some(match self.name_of(e) {
            Some(n) => n,
            None => {
                self.report(ProblemKind::MissingField { field: "member" }, span);
                (self.empty, span)
            }
        });
    }

    /// Resolves a jump's label into the frame. Returns false (after leaving an
    /// error value) when the label names no enclosing loop or block.
    fn jump_label(&mut self, f: &mut Frame<K>, label: Pick<K>, want: Want) -> bool {
        if label.is_absent() {
            return true;
        }
        let Some((sym, span)) = self.opt_name(label) else {
            return true;
        };
        match self.find_label(sym) {
            Some(b) => {
                f.binder = Some(b);
                true
            }
            None => {
                self.report(ProblemKind::UnknownLabel, span);
                let e = self.err_expr(f.span);
                let v = self.adapt_expr(e, false, want, f.span);
                self.push(v, span);
                false
            }
        }
    }

    /// A path: leading root tokens become the root, the rest are segments.
    fn enter_path(
        &mut self,
        mut f: Frame<K>,
        segments: Pick<K>,
        global: Pick<K>,
        qself: Pick<K>,
        qtrait: Pick<K>,
    ) {
        let mut segs = Vec::new();
        pick::list(segments, &self.kids, self.m, &mut segs);
        let mut root = if !global.is_absent() && self.pick1(global).is_some() {
            PathRoot::Global
        } else {
            PathRoot::Relative
        };
        let mut skip = 0;
        for e in &segs {
            let Element::Token(t) = e else { break };
            let Some(r) = self.m.root_for(t.kind()) else {
                break;
            };
            root = match (root, r) {
                (PathRoot::Super(n), PathRoot::Super(m)) => PathRoot::Super(n.saturating_add(m)),
                (_, r) => r,
            };
            skip += 1;
        }
        let rest: Vec<&'t Element<K>> = segs.get(skip..).unwrap_or(&[]).to_vec();
        let q = self.pick1(qself);
        let mut trait_segs = Vec::new();
        pick::list(qtrait, &self.kids, self.m, &mut trait_segs);
        f.build = Build::Arch;
        f.path_root = root;
        // Schedule: qself (0/1), trait segments, segments.
        let counts_base = self.counts.len();
        self.counts.push(usize::from(q.is_some()));
        self.counts.push(trait_segs.len());
        self.counts.push(rest.len());
        f.counts_base = counts_base;
        f.nfields = 3;
        f.vals_base = self.vals.len();
        f.spill_base = self.spill.len();
        self.frames.push(f);
        self.tasks.push(Task::Finish);
        for &e in rest.iter().rev() {
            self.tasks.push(Task::Elem(e, Want::Segment));
        }
        for &e in trait_segs.iter().rev() {
            self.tasks.push(Task::Elem(e, Want::Segment));
        }
        if let Some(e) = q {
            self.tasks.push(Task::Elem(e, Want::Ty));
        }
    }

    /// An operator node: looks the operator up and schedules accordingly.
    fn enter_op(
        &mut self,
        node: &'t Node<K>,
        mut f: Frame<K>,
        fixity: Fixity,
        op: Pick<K>,
        operands: &[Pick<K>],
    ) {
        let want = f.want;
        let span = f.span;
        let tok: Option<&'t syntax_lang::Token<K>> = match self.pick1(op) {
            Some(Element::Token(t)) => Some(t),
            _ => None,
        };
        let spec = tok.and_then(|t| self.m.op(fixity, t.kind()));
        // `-1` in a pattern.
        if let Some(site) = site_of(want) {
            let _ = site;
            let neg = matches!(spec, Some(OpSpec::Op(hir_lang::OpKind::Neg)))
                || matches!(spec, Some(OpSpec::OpWith(o)) if o.kind == hir_lang::OpKind::Neg);
            if neg && fixity == Fixity::Prefix {
                if let Some(Element::Token(t)) = operands.first().and_then(|&p| self.pick1(p)) {
                    if let Some(p) = self.negative_lit_pat(t) {
                        let v = self.adapt_pat(p, want, span);
                        self.push(v, span);
                        return;
                    }
                }
            }
            return self.not_here(want, span);
        }
        if !want.takes_expr() {
            return self.not_here(want, span);
        }
        let Some(spec) = spec else {
            let at = tok.map_or(span, syntax_lang::Token::span);
            self.report(ProblemKind::UnknownOperator, at);
            let e = self.err_expr(span);
            let v = self.adapt_expr(e, false, want, span);
            self.push(v, span);
            return;
        };
        match spec {
            OpSpec::Identity => match operands.first().and_then(|&p| self.pick1(p)) {
                Some(e) => self.tasks.push(Task::Elem(e, want)),
                None => {
                    let e = self.missing_expr("operand", span);
                    let v = self.adapt_expr(e, false, want, span);
                    self.push(v, span);
                }
            },
            OpSpec::Template(id) => self.enter_template(f, id, operands),
            OpSpec::Assign | OpSpec::Compound(_) | OpSpec::CompoundWith(_) | OpSpec::RefAssign => {
                if let Some(&target) = operands.first() {
                    self.implicit_local(target);
                }
                f.build = Build::Op(spec);
                self.plan_operands(f, operands);
            }
            _ => {
                if matches!(spec, OpSpec::Await) {
                    self.infer(Effects::ASYNC);
                }
                f.build = Build::Op(spec);
                self.plan_operands(f, operands);
            }
        }
        let _ = node;
    }

    /// Plans an operator's operands (at most two) as expressions.
    fn plan_operands(&mut self, f: Frame<K>, operands: &[Pick<K>]) {
        let mut fields = [(Pick::Absent, Want::Expr, false); 2];
        for (slot, &p) in fields.iter_mut().zip(operands) {
            slot.0 = p;
        }
        let n = operands.len().min(2);
        self.plan(f, fields.get(..n).unwrap_or(&[]));
    }

    /// A template instantiation: the user's label (if any) is bound and in
    /// scope while the holes are lowered; each hole is a field.
    pub(crate) fn enter_template(&mut self, mut f: Frame<K>, id: TemplateId, picks: &[Pick<K>]) {
        let want = f.want;
        let span = f.span;
        if !want.takes_expr() {
            return self.not_here(want, span);
        }
        let Some(t) = self.m.templates.get(id.index()) else {
            return self.not_here(want, span);
        };
        let mut fields = core::mem::take(&mut self.tfields);
        fields.clear();
        let mut label = None;
        for (i, hole) in t.holes.iter().enumerate() {
            let p = picks.get(i).copied().unwrap_or(Pick::Absent);
            match hole.kind {
                HoleKind::Expr => fields.push((p, Want::Expr, false)),
                HoleKind::Pat => fields.push((p, Want::Pat(PatSite::LOCAL), false)),
                HoleKind::Ty => fields.push((p, Want::Ty, false)),
                HoleKind::Stmts => fields.push((p, Want::Stmt, true)),
                HoleKind::Args => fields.push((p, Want::Arg, true)),
                HoleKind::Label => {
                    if label.is_none() {
                        label = self.def_label(p);
                    }
                    fields.push((Pick::Absent, Want::Expr, false));
                }
            }
        }
        f.build = Build::Template {
            id,
            label,
            nodes_at: self.nodes,
        };
        self.plan(f, &fields);
        self.tfields = fields;
    }
}

/// The effects a construct's modifiers declare.
fn declared_effects(mods: u32) -> Effects {
    let mut e = Effects::NONE;
    for (m, flag) in [
        (Modifier::Async, Effects::ASYNC),
        (Modifier::Generator, Effects::YIELD),
        (Modifier::Throws, Effects::THROWS),
        (Modifier::Unsafe, Effects::UNSAFE),
    ] {
        if has(mods, m) {
            e = e.union(flag);
        }
    }
    e
}
