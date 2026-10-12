//! Ready-made lowering templates for the sugar most languages share.
//!
//! HIR has one loop and no short-circuit operators, so `while`, `do-while`,
//! C `for`, `for-each`, `&&`, `||`, `??`, and `?` are lowered through these
//! templates (HIR spec §8.5, §9). Each is an ordinary [`Template`]; register
//! the ones a language uses with
//! [`MappingBuilder::template`](crate::MappingBuilder::template). Their hole
//! order is documented on each function: a node that uses the template binds
//! one pick per hole, in that order.

use hir_lang::OpKind;

use crate::template::{LabelRef, TLit, Template, TemplateBuilder};

/// Finishes a built-in template. The built-ins are fixed and every one is
/// checked by the tests, so `finish` does not fail; the fallback (a template
/// with no holes that evaluates to unit) keeps this path panic-free anyway.
fn done(t: TemplateBuilder, root: crate::template::TExpr, name: &str) -> Template {
    t.finish(root).unwrap_or_else(|_| Template {
        name: name.into(),
        kind: hir_lang::ExpnKind::Template,
        nodes: alloc::vec![crate::template::TNode::Tuple((0, 0))],
        lists: alloc::vec::Vec::new(),
        texts: alloc::vec::Vec::new(),
        holes: alloc::vec::Vec::new(),
        binders: alloc::vec::Vec::new(),
        labels: alloc::vec::Vec::new(),
        args: alloc::vec::Vec::new(),
        applies: alloc::vec::Vec::new(),
        root: 0,
        text_base: 0,
    })
}

/// `while cond { body }` → `'label: loop { if cond {} else { break }; body }`.
///
/// Holes: `cond` (expression), `body` (expression), `label` (optional label).
///
/// # Examples
///
/// ```
/// use lower_lang::HoleKind;
///
/// let t = lower_lang::templates::while_loop();
/// assert_eq!(t.hole(1), Some(("body", HoleKind::Expr, false)));
/// ```
#[must_use]
pub fn while_loop() -> Template {
    let mut t = TemplateBuilder::new("while").desugar();
    let cond = t.expr_hole("cond");
    let body = t.expr_hole("body");
    let label = t.label_hole("label");
    let c = t.hole(cond);
    let then = t.block(&[], None);
    let brk = t.break_(None, None);
    let test = t.if_(c, then, Some(brk));
    let test = t.expr_stmt(test);
    let b = t.hole(body);
    let inner = t.block(&[test], Some(b));
    let root = t.loop_(Some(LabelRef::Hole(label)), inner, None);
    done(t, root, "while")
}

/// `do { body } while cond` → `'label: loop [step: if cond {} else { break }] { body }`.
///
/// Holes: `body` (expression), `cond` (expression), `label` (optional label).
///
/// # Examples
///
/// ```
/// let t = lower_lang::templates::do_while();
/// assert_eq!(t.hole(1).map(|h| h.0), Some("cond"));
/// ```
#[must_use]
pub fn do_while() -> Template {
    let mut t = TemplateBuilder::new("do-while").desugar();
    let body = t.expr_hole("body");
    let cond = t.expr_hole("cond");
    let label = t.label_hole("label");
    let b = t.hole(body);
    let c = t.hole(cond);
    let then = t.block(&[], None);
    let brk = t.break_(None, None);
    let step = t.if_(c, then, Some(brk));
    let root = t.loop_(Some(LabelRef::Hole(label)), b, Some(step));
    done(t, root, "do-while")
}

/// C `for (init; cond; step) body` →
/// `{ init; 'label: loop [step: step] { if cond {} else { break }; body } }`;
/// `continue` runs `step`, as in C.
///
/// Holes: `init` (statements), `cond` (optional expression, default `true`),
/// `step` (optional expression), `body` (expression), `label` (optional label).
///
/// # Examples
///
/// ```
/// use lower_lang::HoleKind;
///
/// let t = lower_lang::templates::c_for();
/// assert_eq!(t.hole(0), Some(("init", HoleKind::Stmts, true)));
/// assert_eq!(t.hole(1), Some(("cond", HoleKind::Expr, true)));
/// ```
#[must_use]
pub fn c_for() -> Template {
    let mut t = TemplateBuilder::new("for").desugar();
    let init = t.stmts_hole("init");
    let cond = t.optional_expr_hole("cond", Some(TLit::Bool(true)));
    let step = t.optional_expr_hole("step", None);
    let body = t.expr_hole("body");
    let label = t.label_hole("label");
    let init = t.stmts(init);
    let c = t.hole(cond);
    let then = t.block(&[], None);
    let brk = t.break_(None, None);
    let test = t.if_(c, then, Some(brk));
    let test = t.expr_stmt(test);
    let b = t.hole(body);
    let inner = t.block(&[test], Some(b));
    let s = t.hole(step);
    let lp = t.loop_(Some(LabelRef::Hole(label)), inner, Some(s));
    let root = t.block(&[init], Some(lp));
    done(t, root, "for")
}

/// `for-each` over an iteration protocol of two host functions:
/// `{ let it = iter(source); 'label: loop { match next(it) { null => break, (key, value) => body } } }`.
///
/// `next` returns `null` when the iteration is over and a `(key, value)`
/// tuple otherwise (Mox's `foreach ($xs as $k => $v)`). Without a `key`
/// pattern the key is ignored (`_`). `iter` and `next` are host symbols
/// (`Res::Extern`), and `it` is the template's own hygienic binder, so a user
/// variable named `it` is untouched.
///
/// Holes: `source` (expression), `key` (optional pattern), `value`
/// (pattern), `body` (expression), `label` (optional label).
///
/// # Examples
///
/// ```
/// let t = lower_lang::templates::for_each("mox.iter", "mox.next");
/// assert_eq!(t.name(), "foreach");
/// assert_eq!(t.hole_count(), 5);
/// ```
#[must_use]
pub fn for_each(iter: &str, next: &str) -> Template {
    let mut t = TemplateBuilder::new("foreach").desugar();
    let source = t.expr_hole("source");
    let key = t.pat_hole("key", true);
    let value = t.pat_hole("value", false);
    let body = t.expr_hole("body");
    let label = t.label_hole("label");
    let it = t.binder("it");
    let iter_fn = t.extern_(iter);
    let src = t.hole(source);
    let start = t.call(iter_fn, &[src]);
    let it_pat = t.bind(it);
    let decl = t.let_(it_pat, None, Some(start));
    let next_fn = t.extern_(next);
    let it_use = t.var(it);
    let step = t.call(next_fn, &[it_use]);
    let null = t.lit_pat(TLit::Null);
    let brk = t.break_(None, None);
    let k = t.pat(key);
    let v = t.pat(value);
    let pair = t.tuple_pat(&[k, v]);
    let b = t.hole(body);
    let m = t.match_(step, &[(null, None, brk), (pair, None, b)]);
    let lp = t.loop_(Some(LabelRef::Hole(label)), m, None);
    let root = t.block(&[decl], Some(lp));
    done(t, root, "foreach")
}

/// `lhs && rhs` → `if lhs { rhs } else { false }` (a boolean result).
///
/// Holes: `lhs`, `rhs` (expressions). Bind it to the `&&` token with
/// [`OpSpec::Template`](crate::OpSpec::Template).
///
/// # Examples
///
/// ```
/// assert_eq!(lower_lang::templates::logical_and().hole_count(), 2);
/// ```
#[must_use]
pub fn logical_and() -> Template {
    let mut t = TemplateBuilder::new("&&").desugar();
    let lhs = t.expr_hole("lhs");
    let rhs = t.expr_hole("rhs");
    let l = t.hole(lhs);
    let r = t.hole(rhs);
    let no = t.lit(TLit::Bool(false));
    let root = t.if_(l, r, Some(no));
    done(t, root, "&&")
}

/// `lhs || rhs` → `if lhs { true } else { rhs }` (a boolean result).
///
/// Holes: `lhs`, `rhs` (expressions).
///
/// # Examples
///
/// ```
/// assert_eq!(lower_lang::templates::logical_or().name(), "||");
/// ```
#[must_use]
pub fn logical_or() -> Template {
    let mut t = TemplateBuilder::new("||").desugar();
    let lhs = t.expr_hole("lhs");
    let rhs = t.expr_hole("rhs");
    let l = t.hole(lhs);
    let yes = t.lit(TLit::Bool(true));
    let r = t.hole(rhs);
    let root = t.if_(l, yes, Some(r));
    done(t, root, "||")
}

/// `lhs || rhs` returning an operand (JavaScript, Python `or`):
/// `{ let t = lhs; if t { t } else { rhs } }`.
///
/// Holes: `lhs`, `rhs` (expressions).
///
/// # Examples
///
/// ```
/// assert_eq!(lower_lang::templates::or_value().hole_count(), 2);
/// ```
#[must_use]
pub fn or_value() -> Template {
    value_choice("or", false)
}

/// `lhs && rhs` returning an operand (JavaScript, Python `and`):
/// `{ let t = lhs; if t { rhs } else { t } }`.
///
/// Holes: `lhs`, `rhs` (expressions).
///
/// # Examples
///
/// ```
/// assert_eq!(lower_lang::templates::and_value().name(), "and");
/// ```
#[must_use]
pub fn and_value() -> Template {
    value_choice("and", true)
}

fn value_choice(name: &str, and: bool) -> Template {
    let mut t = TemplateBuilder::new(name).desugar();
    let lhs = t.expr_hole("lhs");
    let rhs = t.expr_hole("rhs");
    let tmp = t.binder("t");
    let l = t.hole(lhs);
    let p = t.bind(tmp);
    let decl = t.let_(p, None, Some(l));
    let cond = t.var(tmp);
    let keep = t.var(tmp);
    let r = t.hole(rhs);
    let test = if and {
        t.if_(cond, r, Some(keep))
    } else {
        t.if_(cond, keep, Some(r))
    };
    let root = t.block(&[decl], Some(test));
    done(t, root, name)
}

/// `lhs ?? rhs` → `{ let t = lhs; if t == null { rhs } else { t } }`.
///
/// Holes: `lhs`, `rhs` (expressions).
///
/// # Examples
///
/// ```
/// assert_eq!(lower_lang::templates::coalesce().name(), "??");
/// ```
#[must_use]
pub fn coalesce() -> Template {
    let mut t = TemplateBuilder::new("??").desugar();
    let lhs = t.expr_hole("lhs");
    let rhs = t.expr_hole("rhs");
    let tmp = t.binder("t");
    let l = t.hole(lhs);
    let p = t.bind(tmp);
    let decl = t.let_(p, None, Some(l));
    let probe = t.var(tmp);
    let null = t.lit(TLit::Null);
    let is_null = t.op(OpKind::Eq, &[probe, null]);
    let r = t.hole(rhs);
    let keep = t.var(tmp);
    let test = t.if_(is_null, r, Some(keep));
    let root = t.block(&[decl], Some(test));
    done(t, root, "??")
}

/// Rust's `expr?` → `match expr { Ok(v) => v, Err(e) => return Err(e) }`,
/// with `Ok` and `Err` resolved at the template's definition site.
///
/// Holes: `expr` (expression).
///
/// # Examples
///
/// ```
/// assert_eq!(lower_lang::templates::try_operator().hole_count(), 1);
/// ```
#[must_use]
pub fn try_operator() -> Template {
    let mut t = TemplateBuilder::new("?").desugar();
    let expr = t.expr_hole("expr");
    let v = t.binder("v");
    let e = t.binder("e");
    let scrutinee = t.hole(expr);
    let bind_v = t.bind(v);
    let ok = t.ctor_pat("Ok", &[bind_v]);
    let use_v = t.var(v);
    let bind_e = t.bind(e);
    let err = t.ctor_pat("Err", &[bind_e]);
    let err_fn = t.global("Err");
    let use_e = t.var(e);
    let rewrap = t.call(err_fn, &[use_e]);
    let ret = t.return_(Some(rewrap));
    let root = t.match_(scrutinee, &[(ok, None, use_v), (err, None, ret)]);
    done(t, root, "?")
}

/// `x++` as an expression (C, PHP):
/// `let_place p = x in { let old = p; p = add(old, 1); old }`.
///
/// The place's operands are evaluated once (HIR `LetPlace`), so `$a[f()]++`
/// calls `f` once. Holes: `place` (expression).
///
/// # Examples
///
/// ```
/// assert_eq!(lower_lang::templates::post_increment().name(), "post++");
/// ```
#[must_use]
pub fn post_increment() -> Template {
    let mut t = TemplateBuilder::new("post++").desugar();
    let place = t.expr_hole("place");
    let alias = t.binder("p");
    let old = t.binder("old");
    let target = t.hole(place);
    let read = t.var(alias);
    let p = t.bind(old);
    let decl = t.let_(p, None, Some(read));
    let cur = t.var(old);
    let one = t.lit(TLit::Int(1));
    let sum = t.op(OpKind::Add, &[cur, one]);
    let write_to = t.var(alias);
    let write = t.assign(write_to, sum);
    let write = t.expr_stmt(write);
    let result = t.var(old);
    let body = t.block(&[decl, write], Some(result));
    let root = t.let_place(alias, target, body);
    done(t, root, "post++")
}

#[cfg(test)]
pub(crate) fn all() -> alloc::vec::Vec<Template> {
    alloc::vec![
        while_loop(),
        do_while(),
        c_for(),
        for_each("iter", "next"),
        logical_and(),
        logical_or(),
        or_value(),
        and_value(),
        coalesce(),
        try_operator(),
        post_increment(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_every_builtin_template_is_valid() {
        // `done` falls back silently; check each one really finished.
        let names = [
            "while", "do-while", "for", "foreach", "&&", "||", "or", "and", "??", "?", "post++",
        ];
        for (t, name) in all().iter().zip(names) {
            assert_eq!(t.name(), name);
            assert!(t.hole_count() > 0, "{name} fell back");
            assert_ne!(t.nodes.len(), 1, "{name} fell back");
        }
    }
}
