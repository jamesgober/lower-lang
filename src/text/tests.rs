//! Unit tests for the template text parser: the Mox templates, every error
//! with its byte range, and a differential property against templates built
//! directly with [`TemplateBuilder`] (the reference).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

extern crate std;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::fmt::Write as _;

use hir_lang::{CaptureMode, Op, OpKind, Overflow, Prim};
use proptest::prelude::*;

use super::*;
use crate::template::{LabelSlot, NONE, TNode};
use crate::{MappingBuilder, templates};

// ------------------------------------------------------------- canonical form

/// A canonical rendering of a template's tree from its root: binders and
/// labels by first appearance (with their declared names), holes by their
/// declarations, so two templates with the same meaning render the same
/// whatever order their arenas were filled in.
pub(crate) fn canon(t: &Template) -> String {
    let mut c = Canon {
        t,
        out: String::new(),
        binders: BTreeMap::new(),
        labels: BTreeMap::new(),
    };
    for h in &t.holes {
        let _ = write!(
            c.out,
            "hole {} {:?} opt={} default={:?}; ",
            h.name,
            h.kind,
            h.optional,
            h.default.map(lit_key)
        );
    }
    let _ = write!(c.out, "kind={:?}; ", t.kind);
    c.node(t.root);
    c.out
}

fn lit_key(l: TLit) -> String {
    match l {
        TLit::Float(f) => format!("f{}", f.to_bits()),
        other => format!("{other:?}"),
    }
}

struct Canon<'t> {
    t: &'t Template,
    out: String,
    binders: BTreeMap<u16, usize>,
    labels: BTreeMap<u16, usize>,
}

impl Canon<'_> {
    fn text(&self, i: u32) -> String {
        self.t
            .texts
            .get(i as usize)
            .map_or_else(|| "?".to_string(), |s| s.to_string())
    }

    fn binder(&mut self, b: u16) {
        let n = self.binders.len();
        let id = *self.binders.entry(b).or_insert(n);
        let decl = self.t.binders[usize::from(b)];
        let name = self.text(decl.name);
        let _ = write!(self.out, "b{id}:{name}:{} ", decl.mutable);
    }

    fn label(&mut self, l: LabelSlot) {
        match l {
            LabelSlot::None => self.out.push_str("nolabel "),
            LabelSlot::Hole(h) => {
                let _ = write!(self.out, "lhole{h} ");
            }
            LabelSlot::Own(l) => {
                let n = self.labels.len();
                let id = *self.labels.entry(l).or_insert(n);
                let name = self.text(self.t.labels[usize::from(l)].name);
                let _ = write!(self.out, "l{id}:{name} ");
            }
        }
    }

    fn list(&mut self, l: (u32, u32)) {
        self.out.push('[');
        let items: Vec<u32> = self.t.list(l).to_vec();
        for k in items {
            self.node(k);
        }
        self.out.push(']');
    }

    #[allow(clippy::too_many_lines)]
    fn node(&mut self, n: u32) {
        if n == NONE {
            self.out.push_str("_ ");
            return;
        }
        let node = *self.t.node(n).expect("in arena");
        self.out.push('(');
        match node {
            TNode::Hole(h) => {
                let _ = write!(self.out, "hole {h}");
            }
            TNode::Splice(h) => {
                let _ = write!(self.out, "splice {h}");
            }
            TNode::Lit(l) => {
                let _ = write!(self.out, "lit {}", lit_key(l));
            }
            TNode::Str(s) => {
                let _ = write!(self.out, "str {:?}", self.text(s));
            }
            TNode::Var(b) => {
                self.out.push_str("var ");
                self.binder(b);
            }
            TNode::Global(s) => {
                let _ = write!(self.out, "global {}", self.text(s));
            }
            TNode::Extern(s) => {
                let _ = write!(self.out, "extern {}", self.text(s));
            }
            TNode::Call { callee, args } => {
                self.out.push_str("call ");
                self.node(callee);
                self.list(args);
            }
            TNode::Method { recv, name, args } => {
                let _ = write!(self.out, "method {} ", self.text(name));
                self.node(recv);
                self.list(args);
            }
            TNode::Field { base, name } => {
                let _ = write!(self.out, "field {} ", self.text(name));
                self.node(base);
            }
            TNode::Index { base, index } => {
                self.out.push_str("index ");
                self.node(base);
                self.node(index);
            }
            TNode::Op { op, exact, args } => {
                let _ = write!(self.out, "op {op:?} exact={exact} ");
                self.list(args);
            }
            TNode::Assign { target, value } => {
                self.out.push_str("assign ");
                self.node(target);
                self.node(value);
            }
            TNode::Block { stmts, tail, label } => {
                self.out.push_str("block ");
                self.label(label);
                self.list(stmts);
                self.node(tail);
            }
            TNode::If { cond, then, else_ } => {
                self.out.push_str("if ");
                self.node(cond);
                self.node(then);
                self.node(else_);
            }
            TNode::Match { scrutinee, arms } => {
                self.out.push_str("match ");
                self.node(scrutinee);
                self.list(arms);
            }
            TNode::Loop { label, body, step } => {
                self.out.push_str("loop ");
                self.label(label);
                self.node(step);
                self.node(body);
            }
            TNode::Break { label, value } => {
                self.out.push_str("break ");
                self.label(label);
                self.node(value);
            }
            TNode::Continue { label } => {
                self.out.push_str("continue ");
                self.label(label);
            }
            TNode::Return(v) => {
                self.out.push_str("return ");
                self.node(v);
            }
            TNode::Throw(v) => {
                self.out.push_str("throw ");
                self.node(v);
            }
            TNode::Await(v) => {
                self.out.push_str("await ");
                self.node(v);
            }
            TNode::Yield { key, value } => {
                self.out.push_str("yield ");
                self.node(key);
                self.node(value);
            }
            TNode::YieldFrom(v) => {
                self.out.push_str("yield-from ");
                self.node(v);
            }
            TNode::Spawn(v) => {
                self.out.push_str("spawn ");
                self.node(v);
            }
            TNode::Tuple(l) => {
                self.out.push_str("tuple ");
                self.list(l);
            }
            TNode::Array(l) => {
                self.out.push_str("array ");
                self.list(l);
            }
            TNode::Closure {
                params,
                body,
                implicit,
            } => {
                let _ = write!(self.out, "closure {implicit:?} ");
                let ps: Vec<u32> = self.t.list(params).to_vec();
                for p in ps {
                    self.binder(u16::try_from(p).unwrap());
                }
                self.node(body);
            }
            TNode::Apply { template, args } => {
                let _ = write!(self.out, "apply {template} ");
                let (s, l) = (args.0 as usize, args.1 as usize);
                let list: Vec<TArg> = self.t.args[s..s + l].to_vec();
                for a in list {
                    match a {
                        TArg::Expr(e) => self.node(e.0),
                        TArg::Pat(p) => self.node(p.0),
                        TArg::Ty(t) => self.node(t.0),
                        other => {
                            let _ = write!(self.out, "{other:?} ");
                        }
                    }
                }
            }
            TNode::LetPlace {
                binder,
                place,
                body,
            } => {
                self.out.push_str("let-place ");
                self.binder(binder);
                self.node(place);
                self.node(body);
            }
            TNode::Let { pat, ty, init } => {
                self.out.push_str("let ");
                self.node(pat);
                self.node(ty);
                self.node(init);
            }
            TNode::ExprStmt(e) => {
                self.out.push_str("expr ");
                self.node(e);
            }
            TNode::Defer(e) => {
                self.out.push_str("defer ");
                self.node(e);
            }
            TNode::Stmts(h) => {
                let _ = write!(self.out, "stmts {h}");
            }
            TNode::PHole(h) => {
                let _ = write!(self.out, "phole {h}");
            }
            TNode::PWild => self.out.push_str("wild"),
            TNode::PBind(b) => {
                self.out.push_str("bind ");
                self.binder(b);
            }
            TNode::PLit(l) => {
                let _ = write!(self.out, "plit {}", lit_key(l));
            }
            TNode::PTuple(l) => {
                self.out.push_str("ptuple ");
                self.list(l);
            }
            TNode::PCtor { path, elems } => {
                let _ = write!(self.out, "pctor {} ", self.text(path));
                self.list(elems);
            }
            TNode::THole(h) => {
                let _ = write!(self.out, "thole {h}");
            }
            TNode::TPrim(p) => {
                let _ = write!(self.out, "prim {p:?}");
            }
            TNode::TNamed(s) => {
                let _ = write!(self.out, "named {}", self.text(s));
            }
            TNode::TInfer => self.out.push_str("infer"),
            TNode::TAny => self.out.push_str("any"),
            TNode::TUnion(l) => {
                self.out.push_str("union ");
                self.list(l);
            }
            TNode::TInter(l) => {
                self.out.push_str("inter ");
                self.list(l);
            }
        }
        self.out.push(')');
    }
}

fn ok(name: &str, text: &str) -> Template {
    match Template::parse(name, text) {
        Ok(t) => t,
        Err(e) => panic!("{name}: {e}\n{text}"),
    }
}

fn syntax(text: &str) -> (SyntaxError, usize, usize) {
    match Template::parse("t", text) {
        Err(TemplateError::Syntax { at, error }) => {
            (error, at.start().to_usize(), at.end().to_usize())
        }
        other => panic!("expected a syntax error for {text:?}, got {other:?}"),
    }
}

// ------------------------------------------------------------ Mox templates

/// `[semantics.templates]` of `_lexersketch/sketches/mox.lsf` (2026-10-11),
/// verbatim, then the `let_place` rewrites.
const MOX_TEMPLATES: [(&str, &str); 22] = [
    ("bool_and", "if ${lhs} { not(not(${rhs})) } else { false }"),
    ("bool_or", "if ${lhs} { true } else { not(not(${rhs})) }"),
    ("pre_increment", "{ ${place} = add(${place}, 1); ${place} }"),
    ("pre_decrement", "{ ${place} = sub(${place}, 1); ${place} }"),
    (
        "post_decrement",
        "{ let old = ${place}; ${place} = sub(old, 1); old }",
    ),
    ("new_object", "extern:mox.new(${class}, ${...args})"),
    ("echo", "extern:mox.echo(${...values})"),
    ("echo_text", "extern:mox.echo(${...text})"),
    ("isset", "extern:mox.isset(${...args})"),
    ("empty", "extern:mox.empty(${arg})"),
    ("unset", "extern:mox.unset(${...places})"),
    ("exit", "extern:mox.exit(${...value})"),
    ("print", "extern:mox.print(${value})"),
    ("include", "extern:mox.include(${path})"),
    ("include_once", "extern:mox.include_once(${path})"),
    ("require", "extern:mox.require(${path})"),
    ("require_once", "extern:mox.require_once(${path})"),
    ("spawn_task", "spawn(closure[by-value]() => ${operand})"),
    (
        "yield_from",
        "\n{ let g = extern:mox.iter(${from});\n  loop {\n    match extern:mox.next(g) {\n      null => break extern:mox.gen_return(g),\n      (k, v) => { yield (k, v); }\n    }\n  } }\n",
    ),
    // The `let_place` rewrites (GAPS B2): the place is evaluated once.
    (
        "pre_increment_once",
        "let_place p = ${place} in p = add(p, 1)",
    ),
    (
        "post_decrement_once",
        "let_place p = ${place} in { let old = p; p = sub(old, 1); old }",
    ),
    ("yield_from_native", "yield from ${from}"),
];

#[test]
fn test_every_mox_template_parses() {
    for (name, text) in MOX_TEMPLATES {
        let t = ok(name, text);
        assert_eq!(t.name(), name);
        assert!(t.hole_count() >= 1, "{name}");
    }
}

#[test]
fn test_mox_templates_match_their_builder_form() {
    // bool_and, built by hand.
    let mut b = TemplateBuilder::new("bool_and");
    let lhs = b.expr_hole("lhs");
    let rhs = b.expr_hole("rhs");
    let l = b.hole(lhs);
    let r = b.hole(rhs);
    let n1 = b.op(OpKind::Not, &[r]);
    let n2 = b.op(OpKind::Not, &[n1]);
    let then = b.block(&[], Some(n2));
    let no = b.lit(TLit::Bool(false));
    let els = b.block(&[], Some(no));
    let root = b.if_(l, then, Some(els));
    let built = b.finish(root).unwrap();
    assert_eq!(canon(&ok("bool_and", MOX_TEMPLATES[0].1)), canon(&built));

    // yield_from, built by hand.
    let mut b = TemplateBuilder::new("yield_from");
    let from = b.expr_hole("from");
    let g = b.binder("g");
    let iter = b.extern_("mox.iter");
    let f = b.hole(from);
    let start = b.call(iter, &[f]);
    let gp = b.bind(g);
    let decl = b.let_(gp, None, Some(start));
    let next = b.extern_("mox.next");
    let gu = b.var(g);
    let step = b.call(next, &[gu]);
    let null = b.lit_pat(TLit::Null);
    let ret = b.extern_("mox.gen_return");
    let gu2 = b.var(g);
    let rv = b.call(ret, &[gu2]);
    let brk = b.break_(None, Some(rv));
    let k = b.binder("k");
    let v = b.binder("v");
    let (kp, vp) = (b.bind(k), b.bind(v));
    let pair = b.tuple_pat(&[kp, vp]);
    let (ku, vu) = (b.var(k), b.var(v));
    let tup = b.tuple(&[ku, vu]);
    let y = b.yield_(Some(tup));
    let ys = b.expr_stmt(y);
    let arm = b.block(&[ys], None);
    let m = b.match_(step, &[(null, None, brk), (pair, None, arm)]);
    let body = b.block(&[], Some(m));
    let lp = b.loop_(None, body, None);
    let root = b.block(&[decl], Some(lp));
    let built = b.finish(root).unwrap();
    assert_eq!(canon(&ok("yield_from", MOX_TEMPLATES[18].1)), canon(&built));

    // spawn_task.
    let mut b = TemplateBuilder::new("spawn_task");
    let op = b.expr_hole("operand");
    let o = b.hole(op);
    let c = b.closure(&[], o, Some(CaptureMode::ByValue));
    let root = b.spawn(c);
    let built = b.finish(root).unwrap();
    assert_eq!(canon(&ok("spawn_task", MOX_TEMPLATES[17].1)), canon(&built));

    // pre_increment_once.
    let mut b = TemplateBuilder::new("pre_increment_once");
    let place = b.expr_hole("place");
    let p = b.binder("p");
    let target = b.hole(place);
    let read = b.var(p);
    let one = b.lit(TLit::Int(1));
    let sum = b.op(OpKind::Add, &[read, one]);
    let write = b.var(p);
    let body = b.assign(write, sum);
    let root = b.let_place(p, target, body);
    let built = b.finish(root).unwrap();
    assert_eq!(
        canon(&ok("pre_increment_once", MOX_TEMPLATES[19].1)),
        canon(&built)
    );
}

#[test]
fn test_hole_kinds() {
    let t = ok(
        "kinds",
        "{ let ${pat}: ${ty} = ${init}; ${...body}; ${label} loop { break ${label} ${value} }; extern:f(${...args}) }",
    );
    let kinds: Vec<(String, HoleKind)> = (0..t.hole_count())
        .map(|i| {
            let (n, k, _) = t.hole(i).unwrap();
            (n.to_string(), k)
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("pat".to_string(), HoleKind::Pat),
            ("ty".to_string(), HoleKind::Ty),
            ("init".to_string(), HoleKind::Expr),
            ("body".to_string(), HoleKind::Stmts),
            ("label".to_string(), HoleKind::Label),
            ("value".to_string(), HoleKind::Expr),
            ("args".to_string(), HoleKind::Args),
        ]
    );
}

#[test]
fn test_hole_suffixes() {
    let t = ok(
        "s",
        "(${a?}, ${b?=7}, ${c}, ${c?=-1.5}, ${d?=null}, ${e?='x'})",
    );
    assert_eq!(t.hole(0), Some(("a", HoleKind::Expr, true)));
    assert_eq!(t.holes[1].default, Some(TLit::Int(7)));
    // A later use may add the suffix the first one left out.
    assert_eq!(t.hole(2), Some(("c", HoleKind::Expr, true)));
    assert_eq!(t.holes[2].default, Some(TLit::Float(-1.5)));
    assert_eq!(t.holes[3].default, Some(TLit::Null));
    assert_eq!(t.holes[4].default, Some(TLit::Char('x')));
    // But two different suffixes disagree.
    assert_eq!(syntax("(${a?=1}, ${a?=2})").0, SyntaxError::HoleSuffix);
    assert_eq!(syntax("(${a?}, ${a?=2})").0, SyntaxError::HoleSuffix);
    // Defaults are literals, on expression holes only.
    assert_eq!(syntax("${a?=x}"), (SyntaxError::HoleDefault, 5, 6));
    assert_eq!(syntax("{ let ${p?=1} = 1; }").0, SyntaxError::HoleDefault);
    assert_eq!(syntax("${a?=\"s\"}").0, SyntaxError::HoleDefault);
    assert_eq!(syntax("${a?=1 2}").0, SyntaxError::HoleDefault);
}

#[test]
fn test_ops_policies_and_conversions() {
    let t = ok(
        "o",
        "add<overflow=wrap>(int_cast<i32>(${x}), shl<shift=saturate>(1, 2))",
    );
    let TNode::Op { op, exact, .. } = t.nodes[t.root as usize] else {
        panic!("op")
    };
    assert!(exact);
    assert_eq!(op, Op::new(OpKind::Add).with_overflow(Overflow::Wrap));
    let ops: Vec<(OpKind, bool)> = t
        .nodes
        .iter()
        .filter_map(|n| match n {
            TNode::Op { op, exact, .. } => Some((op.kind, *exact)),
            _ => None,
        })
        .collect();
    assert!(ops.contains(&(OpKind::IntCast(Prim::I32), false)));
    assert!(ops.contains(&(OpKind::Shl, true)));
    let _ = ok("o", "pow(2, 3)");
    let _ = ok("o", "bit_not(${x})");
    let _ = ok("o", "div<div_zero=trap, overflow=promote>(1, 2)");
    let _ = ok("o", "float_to_int<i64, float_to_int=saturate>(1.5)");
    assert_eq!(
        syntax("add(1)"),
        (
            SyntaxError::Arity {
                expected: 2,
                found: 1
            },
            0,
            3
        )
    );
    assert_eq!(
        syntax("eq<overflow=wrap>(1, 2)"),
        (SyntaxError::BadPolicy, 0, 2)
    );
    assert_eq!(
        syntax("add<overflow=sideways>(1, 2)"),
        (SyntaxError::BadPolicy, 4, 21)
    );
    assert_eq!(
        syntax("add<overflow=wrap, overflow=trap>(1, 2)").0,
        SyntaxError::BadPolicy
    );
    assert_eq!(syntax("add<i32>(1, 2)"), (SyntaxError::BadPolicy, 0, 3));
    assert_eq!(syntax("int_cast(1)"), (SyntaxError::UnknownOp, 0, 8));
    assert_eq!(syntax("frobnicate(1)"), (SyntaxError::UnknownOp, 0, 10));
    assert_eq!(syntax("not(${...xs})"), (SyntaxError::SpliceHere, 4, 12));
}

#[test]
fn test_binders_scope_and_hygiene_rules() {
    // A `let` name is visible after its statement, not in its initializer.
    assert_eq!(
        syntax("{ let x = x; x }"),
        (SyntaxError::UnknownName, 10, 11)
    );
    // Block scope ends at the block.
    assert_eq!(
        syntax("{ { let x = 1; }; x }"),
        (SyntaxError::UnknownName, 18, 19)
    );
    // Shadowing makes a second binder.
    let t = ok("s", "{ let x = 1; let x = add(x, 1); x }");
    assert_eq!(t.binders.len(), 2);
    // One pattern may not bind a name twice.
    assert_eq!(
        syntax("{ let (a, a) = ${v}; a }"),
        (SyntaxError::BoundTwice, 10, 11)
    );
    assert_eq!(syntax("closure(a, a) => a").0, SyntaxError::BoundTwice);
    // Binder ids are not written in templates.
    assert_eq!(syntax("{ let x%0 = 1; }"), (SyntaxError::BinderId, 6, 8));
    // Free names are explicit.
    assert_eq!(syntax("iter(${x})").0, SyntaxError::UnknownOp);
    assert_eq!(syntax("count"), (SyntaxError::UnknownName, 0, 5));
    // Match arms bind for their guard and body only.
    let _ = ok("m", "match ${x} { (a, b) if a => b, _ => null }");
    assert_eq!(
        syntax("match ${x} { a => a, _ => a }").0,
        SyntaxError::UnknownName
    );
    // let_place's alias is visible in its body only.
    assert_eq!(
        syntax("(let_place p = ${x} in p, p)").0,
        SyntaxError::UnknownName
    );
    // Op names are reserved before `(`; a binder of that name is still usable.
    let t = ok("o", "{ let add = 1; add }");
    assert_eq!(t.binders.len(), 1);
}

#[test]
fn test_labels() {
    let _ = ok("l", "'outer loop { loop { break 'outer } }");
    let _ = ok("l", "'done { break 'done 1 }");
    let _ = ok("l", "${label} loop [step: ${step}] { continue ${label} }");
    assert_eq!(
        syntax("loop { break 'nowhere }"),
        (SyntaxError::UnknownLabel, 13, 21)
    );
    assert_eq!(
        syntax("{ 'l loop {}; break 'l }").0,
        SyntaxError::UnknownLabel
    );
    assert_eq!(
        syntax("'l 5").0,
        SyntaxError::Expected("`loop` or a block after the label")
    );
}

#[test]
fn test_lexical_errors_carry_their_range() {
    assert_eq!(syntax("\"abc"), (SyntaxError::Unterminated, 0, 4));
    assert_eq!(syntax("\"a\\qb\""), (SyntaxError::BadEscape, 2, 4));
    assert_eq!(syntax("1 # 2"), (SyntaxError::UnexpectedChar, 2, 3));
    assert_eq!(syntax("é"), (SyntaxError::UnexpectedChar, 0, 2));
    assert_eq!(
        syntax("99999999999999999999"),
        (SyntaxError::BadNumber, 0, 20)
    );
    assert_eq!(syntax("12ab"), (SyntaxError::BadNumber, 0, 4));
    assert_eq!(syntax("0x"), (SyntaxError::BadNumber, 0, 2));
    assert_eq!(
        syntax("${Bad}").0,
        SyntaxError::Expected("a hole name (lowercase letters and `_`)")
    );
    assert_eq!(syntax("${x").0, SyntaxError::Unterminated);
    assert_eq!(
        syntax("'ab'").0,
        SyntaxError::Expected("one character between quotes")
    );
    assert_eq!(
        syntax("extern:"),
        (SyntaxError::Expected("a name after the prefix"), 0, 7)
    );
    assert_eq!(
        syntax("global:a::").0,
        SyntaxError::Expected("a path like `a::b`")
    );
    assert_eq!(syntax(""), (SyntaxError::Expected("an expression"), 0, 0));
    assert_eq!(
        syntax("1 2"),
        (SyntaxError::Expected("the end of the template"), 2, 3)
    );
    let _ = ok(
        "n",
        "(-9223372036854775808, 0x7fff_ffff, 0b101, 0o17, 1_000, 2.5e-3, '\\n', 'x', \"\\u{1F600}\")",
    );
    assert_eq!(syntax("9223372036854775808").0, SyntaxError::BadNumber);
}

#[test]
fn test_splices_only_in_lists_and_blocks() {
    let _ = ok("s", "[${...xs}, 1]");
    let _ = ok("s", "(${...xs})");
    let _ = ok("s", "{ ${...body} }");
    assert_eq!(syntax("${...xs}"), (SyntaxError::SpliceHere, 0, 8));
    assert_eq!(syntax("{ let ${...p} = 1; }").0, SyntaxError::SpliceHere);
    // A hole is one kind everywhere.
    assert_eq!(
        syntax("(${x}, [${...x}])").0,
        SyntaxError::HoleKind {
            first: HoleKind::Expr,
            here: HoleKind::Args
        }
    );
}

#[test]
fn test_types_and_patterns() {
    let _ = ok("t", "{ let x: i64 | str | global:app::Id = ${v}; x }");
    let _ = ok("t", "{ let x: Countable & Traversable = ${v}; x }");
    let _ = ok("t", "{ let x: (i64 | Null) & any = ${v}; x }");
    let _ = ok(
        "t",
        "{ let (a, (b,), _, mut c, 1, -2, 'z', true, null, global:Some(d), Ok(e)): _ = ${v}; c }",
    );
    assert_eq!(
        syntax("{ let x: a | b & c = 1; }").0,
        SyntaxError::Expected("parentheses around a union inside an intersection")
    );
    assert_eq!(
        syntax("match 1 { \"s\" => 1 }").0,
        SyntaxError::Unsupported("string literal patterns")
    );
}

#[test]
fn test_apply_resolves_against_the_registry() {
    let mut m = MappingBuilder::<u8>::new();
    let _w = m.template(templates::while_loop()).unwrap();
    let _c = m.template(templates::coalesce()).unwrap();
    let first = m.parse_template("first", "add(${a}, 1)").unwrap();
    // By user name, by a built-in's LSF2 name, trailing optional holes left out.
    let id = m
        .parse_template(
            "both",
            "{ apply:first(${x}); apply:while(${c}, ${b}); apply:coalesce(${x}, 0); apply:while(true, {}, _) }",
        )
        .unwrap();
    assert!(id.index() > first.index());
    assert_eq!(
        m.parse_template("bad", "apply:nothing(1)").unwrap_err(),
        TemplateError::Syntax {
            at: Span::new(0, 13),
            error: SyntaxError::UnknownTemplate
        }
    );
    assert!(matches!(
        m.parse_template("bad", "apply:first(1, 2)"),
        Err(TemplateError::Syntax {
            error: SyntaxError::ApplyArgs,
            ..
        })
    ));
    assert!(matches!(
        m.parse_template("bad", "apply:first()"),
        Err(TemplateError::Syntax {
            error: SyntaxError::ApplyArgs,
            ..
        })
    ));
    // A template cannot apply itself: it is not registered yet.
    assert!(matches!(
        m.parse_template("me", "apply:me(1)"),
        Err(TemplateError::Syntax {
            error: SyntaxError::UnknownTemplate,
            ..
        })
    ));
    // Standalone parsing has no registry.
    assert_eq!(syntax("apply:first(1)").0, SyntaxError::UnknownTemplate);
    // Forwarded statement and label holes.
    let _cfor = m.template(templates::c_for()).unwrap();
    let _ = m
        .parse_template(
            "my_for",
            "apply:c_for(${...init}, ${cond?}, ${step?}, ${body}, ${label})",
        )
        .unwrap();
    assert!(m.build().is_ok());
}

#[test]
fn test_misc_forms() {
    for text in [
        "{ defer extern:close(${h}); return; }",
        "{ throw ${e} }",
        "await ${e}",
        "yield ${k} => ${v}",
        "yield",
        "${a}.len",
        "${a}.push(1, ${...rest})",
        "${a}[0][1]",
        "(${f})(1)",
        "closure[infer](x, mut y) => add(x, y)",
        "closure() => ${e}",
        "if ${a} { 1 } else if ${b} { 2 } else { 3 }",
        "match ${x} { 0 => { 1 } _ => 2 }",
        "{ if ${c} { 1 } else { 2 } ${d} }",
        "${a} = ${b} = 3",
        "global:core::mem::take(${x})",
        "()",
        "(1,)",
    ] {
        let _ = ok("m", text);
    }
    assert_eq!(
        syntax("closure[sideways]() => 1").0,
        SyntaxError::Expected("`none`, `infer`, `by-ref`, `by-mut-ref`, or `by-value`")
    );
    assert_eq!(syntax("{ 1 2 }").0, SyntaxError::Expected("`;` or `}`"));
    assert_eq!(
        syntax("match 1 { 1 => 2 3 => 4 }").0,
        SyntaxError::Expected("`,` or `}` after the arm")
    );
}

#[test]
fn test_limits() {
    let deep = "(".repeat(MAX_TEMPLATE_DEPTH as usize + 5);
    assert_eq!(syntax(&deep).0, SyntaxError::TooDeep);
    let ok_depth = format!(
        "{}1{}",
        "(".repeat(MAX_TEMPLATE_DEPTH as usize - 1),
        ")".repeat(MAX_TEMPLATE_DEPTH as usize - 1)
    );
    let _ = ok("d", &ok_depth);
    let long = " ".repeat(MAX_TEMPLATE_TEXT + 1);
    assert_eq!(syntax(&long).0, SyntaxError::TooLong);
}

#[test]
fn test_parser_stack_stays_small_at_the_depth_cap() {
    // Every nesting form past the cap, on a small stack: the cap, not the
    // machine, ends the recursion.
    let n = MAX_TEMPLATE_DEPTH as usize * 2;
    let forms = vec![
        "(".repeat(n),
        "{".repeat(n),
        "not(".repeat(n),
        "[".repeat(n),
        "if 1 { ".repeat(n),
        "return ".repeat(n),
        "match 1 { _ => (".repeat(n),
        format!("match 1 {{ {}", "(".repeat(n)),
        format!("{{ let x: {}", "(".repeat(n)),
        "${a}[".repeat(n),
        "if 1 {} else ".repeat(n),
        "closure() => ".repeat(n),
        "let_place p = (".repeat(n),
        "'l { ".repeat(n),
        "yield yield ".repeat(n),
        "extern:f(".repeat(n),
    ];
    // 512 KiB: half of Windows' main-thread stack, and twice what an
    // unoptimized build needs at the cap.
    let handle = std::thread::Builder::new()
        .stack_size(512 * 1024)
        .spawn(move || {
            for f in &forms {
                assert_eq!(
                    Template::parse("deep", f).err().map(|e| match e {
                        TemplateError::Syntax { error, .. } => error,
                        _ => SyntaxError::TooLong,
                    }),
                    Some(SyntaxError::TooDeep),
                    "{f}"
                );
            }
        })
        .unwrap();
    handle.join().unwrap();
}

// ----------------------------------------------------- differential property

/// A generated template, rendered as text and built with the builder.
#[derive(Clone, Debug)]
enum G {
    Int(i64),
    Float(f64),
    Bool(bool),
    Null,
    Str(String),
    Char(char),
    Hole(u8),
    OptHole,
    DefaultHole,
    Var(u8),
    Global(u8),
    Continue(Option<u8>),
    Extern(u8, Vec<A>),
    Call(Box<G>, Vec<A>),
    Method(Box<G>, u8, Vec<A>),
    Field(Box<G>, u8),
    Index(Box<G>, Box<G>),
    Op1(u8, Box<G>),
    Op2(u8, Box<G>, Box<G>),
    Exact(Box<G>, Box<G>),
    Assign(Box<G>, Box<G>),
    Tuple(Vec<A>),
    Array(Vec<A>),
    Block(Vec<S>, Option<Box<G>>),
    If(Box<G>, Box<G>, Option<Box<G>>),
    Match(Box<G>, Vec<(P, Option<G>, G)>),
    Loop(bool, Box<G>, Option<Box<G>>),
    Break(Option<u8>, Option<Box<G>>),
    Return(Option<Box<G>>),
    Throw(Box<G>),
    Await(Box<G>),
    Spawn(Box<G>),
    Yield(Option<Box<G>>),
    YieldKv(Box<G>, Box<G>),
    YieldFrom(Box<G>),
    LetPlace(Box<G>, Box<G>),
    Closure(u8, Option<u8>, Box<G>),
}

/// A list element: an expression or an argument splice.
#[derive(Clone, Debug)]
enum A {
    E(G),
    Splice(u8),
}

#[derive(Clone, Debug)]
enum S {
    Let(P, Option<Ty>, Option<G>),
    Expr(G),
    Splice(u8),
    Defer(G),
}

#[derive(Clone, Debug)]
enum P {
    Wild,
    Int(i64),
    Null,
    Bool(bool),
    Bind(bool),
    Hole(u8),
    Tuple(Vec<P>),
    Ctor(u8, Vec<P>),
}

#[derive(Clone, Debug)]
enum Ty {
    Prim(u8),
    Any,
    Infer,
    Hole,
    Named(u8),
    Union(Vec<Ty>),
}

const EXPR_HOLES: [&str; 4] = ["x", "y", "z", "w"];
const ARGS_HOLES: [&str; 2] = ["args", "rest"];
const STMTS_HOLES: [&str; 2] = ["body", "init"];
const PAT_HOLES: [&str; 2] = ["pa", "pb"];
const UNARY: [OpKind; 4] = [OpKind::Not, OpKind::BitNot, OpKind::Neg, OpKind::Abs];
const BINARY: [OpKind; 5] = [
    OpKind::Add,
    OpKind::Pow,
    OpKind::Eq,
    OpKind::Shl,
    OpKind::Min,
];
const PRIMS: [Prim; 4] = [Prim::I64, Prim::Str, Prim::Bool, Prim::F64];

fn arb_ty() -> impl Strategy<Value = Ty> {
    let leaf = prop_oneof![
        (0u8..4).prop_map(Ty::Prim),
        Just(Ty::Any),
        Just(Ty::Infer),
        Just(Ty::Hole),
        (0u8..3).prop_map(Ty::Named),
    ];
    leaf.prop_recursive(2, 8, 3, |inner| {
        prop::collection::vec(inner, 2..4).prop_map(Ty::Union)
    })
}

fn arb_pat() -> impl Strategy<Value = P> {
    let leaf = prop_oneof![
        Just(P::Wild),
        (-5i64..5).prop_map(P::Int),
        Just(P::Null),
        any::<bool>().prop_map(P::Bool),
        any::<bool>().prop_map(P::Bind),
        (0u8..2).prop_map(P::Hole),
    ];
    leaf.prop_recursive(3, 12, 3, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..3).prop_map(P::Tuple),
            ((0u8..2), prop::collection::vec(inner, 0..3)).prop_map(|(c, ps)| P::Ctor(c, ps)),
        ]
    })
}

fn arb_g() -> impl Strategy<Value = G> {
    let leaf = prop_oneof![
        any::<i64>().prop_map(G::Int),
        (-1000.0f64..1000.0).prop_map(G::Float),
        any::<bool>().prop_map(G::Bool),
        Just(G::Null),
        "[a-z \\\\\"\\n\\t\u{e9}\u{1F600}]{0,6}".prop_map(G::Str),
        prop::char::any().prop_map(G::Char),
        (0u8..4).prop_map(G::Hole),
        Just(G::OptHole),
        Just(G::DefaultHole),
        any::<u8>().prop_map(G::Var),
        (0u8..3).prop_map(G::Global),
        any::<Option<u8>>().prop_map(G::Continue),
    ];
    leaf.prop_recursive(5, 48, 4, |inner| {
        let b = || inner.clone().prop_map(Box::new);
        let arg = prop_oneof![
            4 => inner.clone().prop_map(A::E),
            1 => (0u8..2).prop_map(A::Splice),
        ];
        let args = prop::collection::vec(arg, 0..3);
        let stmt = prop_oneof![
            (
                arb_pat(),
                prop::option::of(arb_ty()),
                prop::option::of(inner.clone())
            )
                .prop_map(|(p, t, e)| S::Let(p, t, e)),
            inner.clone().prop_map(S::Expr),
            (0u8..2).prop_map(S::Splice),
            inner.clone().prop_map(S::Defer),
        ];
        let arms = prop::collection::vec(
            (arb_pat(), prop::option::of(inner.clone()), inner.clone()),
            0..3,
        );
        prop_oneof![
            ((0u8..3), args.clone()).prop_map(|(n, a)| G::Extern(n, a)),
            (b(), args.clone()).prop_map(|(c, a)| G::Call(c, a)),
            (b(), (0u8..3), args.clone()).prop_map(|(r, n, a)| G::Method(r, n, a)),
            (b(), (0u8..3)).prop_map(|(e, n)| G::Field(e, n)),
            (b(), b()).prop_map(|(e, i)| G::Index(e, i)),
            ((0u8..4), b()).prop_map(|(o, e)| G::Op1(o, e)),
            ((0u8..5), b(), b()).prop_map(|(o, a, c)| G::Op2(o, a, c)),
            (b(), b()).prop_map(|(a, c)| G::Exact(a, c)),
            (b(), b()).prop_map(|(t, v)| G::Assign(t, v)),
            args.clone().prop_map(G::Tuple),
            args.prop_map(G::Array),
            (prop::collection::vec(stmt, 0..4), prop::option::of(b()))
                .prop_map(|(s, t)| G::Block(s, t)),
            (b(), b(), prop::option::of(b())).prop_map(|(c, t, e)| G::If(c, t, e)),
            (b(), arms).prop_map(|(s, arms)| G::Match(s, arms)),
            (any::<bool>(), b(), prop::option::of(b()))
                .prop_map(|(l, body, s)| G::Loop(l, body, s)),
            (any::<Option<u8>>(), prop::option::of(b())).prop_map(|(l, v)| G::Break(l, v)),
            prop::option::of(b()).prop_map(G::Return),
            b().prop_map(G::Throw),
            b().prop_map(G::Await),
            b().prop_map(G::Spawn),
            prop::option::of(b()).prop_map(G::Yield),
            (b(), b()).prop_map(|(k, v)| G::YieldKv(k, v)),
            b().prop_map(G::YieldFrom),
            (b(), b()).prop_map(|(p, body)| G::LetPlace(p, body)),
            ((0u8..3), any::<Option<u8>>(), b()).prop_map(|(n, m, body)| G::Closure(n, m, body)),
        ]
    })
}

/// Renders a generated template as text and builds the same template with
/// the builder, side by side, tracking scopes like the parser does.
struct Twin {
    text: String,
    b: TemplateBuilder,
    holes: BTreeMap<String, u16>,
    scope: Vec<(String, TBinder)>,
    labels: Vec<(String, TLabel)>,
    fresh: u32,
}

impl Twin {
    fn new() -> Self {
        Self {
            text: String::new(),
            b: TemplateBuilder::new("gen"),
            holes: BTreeMap::new(),
            scope: Vec::new(),
            labels: Vec::new(),
            fresh: 0,
        }
    }

    fn name(&mut self, prefix: &str) -> String {
        self.fresh += 1;
        format!("{prefix}{}", self.fresh)
    }

    fn hole(&mut self, name: &str, kind: HoleKind, suffix: &str) -> u16 {
        let dots = if matches!(kind, HoleKind::Args | HoleKind::Stmts) {
            "..."
        } else {
            ""
        };
        let _ = write!(self.text, "${{{dots}{name}{suffix}}}");
        if let Some(&h) = self.holes.get(name) {
            return h;
        }
        let h = match (kind, suffix) {
            (HoleKind::Expr, "") => self.b.expr_hole(name).0,
            (HoleKind::Expr, "?") => self.b.optional_expr_hole(name, None).0,
            (HoleKind::Expr, _) => self.b.optional_expr_hole(name, Some(TLit::Int(7))).0,
            (HoleKind::Pat, _) => self.b.pat_hole(name, false).0,
            (HoleKind::Ty, _) => self.b.ty_hole(name, false).0,
            (HoleKind::Stmts, _) => self.b.stmts_hole(name).0,
            (HoleKind::Args, _) => self.b.args_hole(name).0,
            (HoleKind::Label, _) => self.b.label_hole(name).0,
        };
        let _ = self.holes.insert(name.to_string(), h);
        h
    }

    fn paren(&mut self, g: &G) -> TExpr {
        self.text.push('(');
        let e = self.expr(g);
        self.text.push(')');
        e
    }

    fn list(&mut self, items: &[A], close: char) -> Vec<TExpr> {
        let mut out = Vec::new();
        for (i, a) in items.iter().enumerate() {
            if i > 0 {
                self.text.push_str(", ");
            }
            match a {
                A::E(g) => out.push(self.expr(g)),
                A::Splice(h) => {
                    let h = self.hole(ARGS_HOLES[usize::from(*h)], HoleKind::Args, "");
                    out.push(self.b.splice(ArgsHole(h)));
                }
            }
        }
        self.text.push(close);
        out
    }

    fn lit(&mut self, l: TLit) -> TExpr {
        match l {
            TLit::Int(v) => {
                let _ = write!(self.text, "{v}");
            }
            TLit::Float(v) => {
                let _ = write!(self.text, "{v:?}");
            }
            TLit::Bool(v) => {
                let _ = write!(self.text, "{v}");
            }
            TLit::Null => self.text.push_str("null"),
            TLit::Char(c) => {
                let _ = write!(self.text, "'{}'", escape(c));
            }
        }
        self.b.lit(l)
    }

    fn block(&mut self, stmts: &[S], tail: Option<&G>) -> TExpr {
        self.text.push_str("{ ");
        let scope = self.scope.len();
        let mut out = Vec::new();
        for (i, s) in stmts.iter().enumerate() {
            let last = i + 1 == stmts.len() && tail.is_none();
            out.push(self.stmt(s, last));
            self.text.push(' ');
        }
        let tail = tail.map(|g| {
            let e = self.expr(g);
            self.text.push(' ');
            e
        });
        self.text.push('}');
        self.scope.truncate(scope);
        self.b.block(&out, tail)
    }

    fn stmt(&mut self, s: &S, last: bool) -> TStmt {
        match s {
            S::Let(p, ty, init) => {
                self.text.push_str("let ");
                let mut binds = Vec::new();
                let pat = self.pat(p, &mut binds);
                let ty = ty.as_ref().map(|t| {
                    self.text.push_str(": ");
                    self.ty(t)
                });
                let init = init.as_ref().map(|g| {
                    self.text.push_str(" = ");
                    self.expr(g)
                });
                self.text.push(';');
                self.scope.extend(binds);
                self.b.let_(pat, ty, init)
            }
            S::Expr(g) if blocky(g) => {
                let e = self.expr(g);
                if last {
                    self.text.push(';');
                }
                self.b.expr_stmt(e)
            }
            S::Expr(g) => {
                let e = self.expr(g);
                self.text.push(';');
                self.b.expr_stmt(e)
            }
            S::Splice(h) => {
                let h = self.hole(STMTS_HOLES[usize::from(*h)], HoleKind::Stmts, "");
                self.text.push(';');
                self.b.stmts(StmtsHole(h))
            }
            S::Defer(g) => {
                self.text.push_str("defer ");
                let e = self.expr(g);
                self.text.push(';');
                self.b.defer(e)
            }
        }
    }

    fn pats(&mut self, ps: &[P], binds: &mut Vec<(String, TBinder)>) -> Vec<TPat> {
        let mut out = Vec::new();
        for (i, q) in ps.iter().enumerate() {
            if i > 0 {
                self.text.push_str(", ");
            }
            out.push(self.pat(q, binds));
        }
        out
    }

    fn pat(&mut self, p: &P, binds: &mut Vec<(String, TBinder)>) -> TPat {
        match p {
            P::Wild => {
                self.text.push('_');
                self.b.wild()
            }
            P::Int(v) => {
                let _ = write!(self.text, "{v}");
                self.b.lit_pat(TLit::Int(*v))
            }
            P::Null => {
                self.text.push_str("null");
                self.b.lit_pat(TLit::Null)
            }
            P::Bool(v) => {
                let _ = write!(self.text, "{v}");
                self.b.lit_pat(TLit::Bool(*v))
            }
            P::Bind(mutable) => {
                let name = self.name("v");
                let b = if *mutable {
                    self.text.push_str("mut ");
                    self.b.mutable_binder(&name)
                } else {
                    self.b.binder(&name)
                };
                self.text.push_str(&name);
                binds.push((name, b));
                self.b.bind(b)
            }
            P::Hole(h) => {
                let h = self.hole(PAT_HOLES[usize::from(*h)], HoleKind::Pat, "");
                self.b.pat(PatHole(h))
            }
            P::Tuple(ps) => {
                self.text.push('(');
                let out = self.pats(ps, binds);
                if ps.len() == 1 {
                    self.text.push(',');
                }
                self.text.push(')');
                self.b.tuple_pat(&out)
            }
            P::Ctor(c, ps) => {
                let path = ["Some", "app::Pair"][usize::from(*c)];
                let _ = write!(self.text, "global:{path}(");
                let out = self.pats(ps, binds);
                self.text.push(')');
                self.b.ctor_pat(path, &out)
            }
        }
    }

    fn ty(&mut self, t: &Ty) -> TTy {
        match t {
            Ty::Prim(p) => {
                let p = PRIMS[usize::from(*p)];
                self.text.push_str(p.name());
                self.b.prim(p)
            }
            Ty::Any => {
                self.text.push_str("any");
                self.b.any_ty()
            }
            Ty::Infer => {
                self.text.push('_');
                self.b.infer_ty()
            }
            Ty::Hole => {
                let h = self.hole("ty", HoleKind::Ty, "");
                self.b.ty(TyHole(h))
            }
            Ty::Named(n) => {
                let path = ["Countable", "app::Id", "core::Iter"][usize::from(*n)];
                let _ = write!(self.text, "global:{path}");
                self.b.named_ty(path)
            }
            Ty::Union(ms) => {
                self.text.push('(');
                let mut out = Vec::new();
                for (i, m) in ms.iter().enumerate() {
                    if i > 0 {
                        self.text.push_str(" | ");
                    }
                    out.push(self.ty(m));
                }
                self.text.push(')');
                self.b.union_ty(&out)
            }
        }
    }

    fn opt_value(&mut self, v: Option<&G>) -> Option<TExpr> {
        v.map(|g| {
            self.text.push(' ');
            self.paren(g)
        })
    }

    fn label_ref(&mut self, l: Option<u8>) -> Option<LabelRef> {
        let l = l?;
        if self.labels.is_empty() {
            return None;
        }
        let (name, label) = self.labels[usize::from(l) % self.labels.len()].clone();
        let _ = write!(self.text, " '{name}");
        Some(LabelRef::Own(label))
    }

    fn expr(&mut self, g: &G) -> TExpr {
        match g {
            G::Int(v) => self.lit(TLit::Int(*v)),
            G::Float(v) => self.lit(TLit::Float(*v)),
            G::Bool(v) => self.lit(TLit::Bool(*v)),
            G::Null => self.lit(TLit::Null),
            G::Char(c) => self.lit(TLit::Char(*c)),
            G::Str(s) => {
                self.text.push('"');
                for c in s.chars() {
                    self.text.push_str(&escape(c));
                }
                self.text.push('"');
                self.b.str(s)
            }
            G::Hole(h) => {
                let h = self.hole(EXPR_HOLES[usize::from(*h)], HoleKind::Expr, "");
                self.b.hole(ExprHole(h))
            }
            G::OptHole => {
                let h = self.hole("maybe", HoleKind::Expr, "?");
                self.b.hole(ExprHole(h))
            }
            G::DefaultHole => {
                let h = self.hole("seven", HoleKind::Expr, "?=7");
                self.b.hole(ExprHole(h))
            }
            G::Var(k) => {
                if self.scope.is_empty() {
                    return self.lit(TLit::Int(i64::from(*k)));
                }
                let (name, b) = self.scope[usize::from(*k) % self.scope.len()].clone();
                self.text.push_str(&name);
                self.b.var(b)
            }
            G::Global(n) => {
                let path = ["iter", "core::mem::take", "app::x::y"][usize::from(*n)];
                let _ = write!(self.text, "global:{path}");
                self.b.global(path)
            }
            G::Continue(l) => {
                self.text.push_str("continue");
                let l = self.label_ref(*l);
                self.b.continue_(l)
            }
            _ => self.compound(g),
        }
    }

    #[allow(clippy::too_many_lines)]
    fn compound(&mut self, g: &G) -> TExpr {
        match g {
            G::Extern(n, args) => {
                let name = ["mox.echo", "host.f", "io::print"][usize::from(*n)];
                let _ = write!(self.text, "extern:{name}(");
                let f = self.b.extern_(name);
                let args = self.list(args, ')');
                self.b.call(f, &args)
            }
            G::Call(callee, args) => {
                let c = self.paren(callee);
                self.text.push('(');
                let args = self.list(args, ')');
                self.b.call(c, &args)
            }
            G::Method(recv, n, args) => {
                let r = self.paren(recv);
                let name = ["len", "push", "get"][usize::from(*n)];
                let _ = write!(self.text, ".{name}(");
                let args = self.list(args, ')');
                self.b.method(r, name, &args)
            }
            G::Field(base, n) => {
                let e = self.paren(base);
                let name = ["a", "len", "x_1"][usize::from(*n)];
                let _ = write!(self.text, ".{name}");
                self.b.field(e, name)
            }
            G::Index(base, i) => {
                let e = self.paren(base);
                self.text.push('[');
                let i = self.expr(i);
                self.text.push(']');
                self.b.index(e, i)
            }
            G::Op1(o, a) => {
                let kind = UNARY[usize::from(*o)];
                let _ = write!(self.text, "{}(", kind.name());
                let a = self.expr(a);
                self.text.push(')');
                self.b.op(kind, &[a])
            }
            G::Op2(o, a, c) => {
                let kind = BINARY[usize::from(*o)];
                let _ = write!(self.text, "{}(", kind.name());
                let a = self.expr(a);
                self.text.push_str(", ");
                let c = self.expr(c);
                self.text.push(')');
                self.b.op(kind, &[a, c])
            }
            G::Exact(a, c) => {
                self.text.push_str("sub<overflow=wrap>(");
                let a = self.expr(a);
                self.text.push_str(", ");
                let c = self.expr(c);
                self.text.push(')');
                self.b
                    .op_with(Op::new(OpKind::Sub).with_overflow(Overflow::Wrap), &[a, c])
            }
            G::Assign(t, v) => {
                let t = self.paren(t);
                self.text.push_str(" = ");
                let v = self.expr(v);
                self.b.assign(t, v)
            }
            G::Tuple(items) => {
                self.text.push('(');
                if let [A::E(only)] = items.as_slice() {
                    let e = self.expr(only);
                    self.text.push_str(",)");
                    return self.b.tuple(&[e]);
                }
                let es = self.list(items, ')');
                self.b.tuple(&es)
            }
            G::Array(items) => {
                self.text.push('[');
                let es = self.list(items, ']');
                self.b.array(&es)
            }
            G::Block(stmts, tail) => self.block(stmts, tail.as_deref()),
            G::If(c, t, e) => {
                self.text.push_str("if ");
                let c = self.paren(c);
                self.text.push(' ');
                let t = self.block(&[], Some(t));
                let e = e.as_ref().map(|e| {
                    self.text.push_str(" else ");
                    self.block(&[], Some(e))
                });
                self.b.if_(c, t, e)
            }
            G::Match(s, arms) => {
                self.text.push_str("match ");
                let s = self.paren(s);
                self.text.push_str(" { ");
                let mut out = Vec::new();
                for (p, guard, body) in arms {
                    let scope = self.scope.len();
                    let mut binds = Vec::new();
                    let pat = self.pat(p, &mut binds);
                    self.scope.extend(binds);
                    let guard = guard.as_ref().map(|g| {
                        self.text.push_str(" if ");
                        self.paren(g)
                    });
                    self.text.push_str(" => ");
                    let body = self.expr(body);
                    self.text.push_str(", ");
                    self.scope.truncate(scope);
                    out.push((pat, guard, body));
                }
                self.text.push('}');
                self.b.match_(s, &out)
            }
            G::Loop(labeled, body, step) => {
                let label = if *labeled {
                    let name = self.name("l");
                    let _ = write!(self.text, "'{name} ");
                    let l = self.b.label(&name);
                    self.labels.push((name, l));
                    Some(l)
                } else {
                    None
                };
                self.text.push_str("loop ");
                let step = step.as_ref().map(|s| {
                    self.text.push_str("[step: ");
                    let s = self.expr(s);
                    self.text.push_str("] ");
                    s
                });
                let body = self.block(&[], Some(body));
                if label.is_some() {
                    let _ = self.labels.pop();
                }
                self.b.loop_(label.map(LabelRef::Own), body, step)
            }
            G::Break(l, v) => {
                self.text.push_str("break");
                let l = self.label_ref(*l);
                let v = self.opt_value(v.as_deref());
                self.b.break_(l, v)
            }
            G::Return(v) => {
                self.text.push_str("return");
                let v = self.opt_value(v.as_deref());
                self.b.return_(v)
            }
            G::Throw(v) => {
                self.text.push_str("throw ");
                let v = self.paren(v);
                self.b.throw(v)
            }
            G::Await(v) => {
                self.text.push_str("await ");
                let v = self.paren(v);
                self.b.await_(v)
            }
            G::Spawn(v) => {
                self.text.push_str("spawn ");
                let v = self.paren(v);
                self.b.spawn(v)
            }
            G::Yield(v) => {
                self.text.push_str("yield");
                let v = self.opt_value(v.as_deref());
                self.b.yield_(v)
            }
            G::YieldKv(k, v) => {
                self.text.push_str("yield ");
                let k = self.paren(k);
                self.text.push_str(" => ");
                let v = self.paren(v);
                self.b.yield_kv(k, v)
            }
            G::YieldFrom(v) => {
                self.text.push_str("yield from ");
                let v = self.paren(v);
                self.b.yield_from(v)
            }
            G::LetPlace(place, body) => {
                let name = self.name("p");
                let _ = write!(self.text, "let_place {name} = ");
                let place = self.paren(place);
                self.text.push_str(" in ");
                let alias = self.b.binder(&name);
                self.scope.push((name, alias));
                let body = self.paren(body);
                let _ = self.scope.pop();
                self.b.let_place(alias, place, body)
            }
            G::Closure(n, mode, body) => {
                let (word, implicit) = match mode.map(|m| m % 5) {
                    None => ("", None),
                    Some(0) => ("[none]", None),
                    Some(1) => ("[infer]", Some(CaptureMode::Infer)),
                    Some(2) => ("[by-ref]", Some(CaptureMode::ByRef)),
                    Some(3) => ("[by-mut-ref]", Some(CaptureMode::ByMutRef)),
                    Some(_) => ("[by-value]", Some(CaptureMode::ByValue)),
                };
                let _ = write!(self.text, "closure{word}(");
                let scope = self.scope.len();
                let mut params = Vec::new();
                for i in 0..*n {
                    if i > 0 {
                        self.text.push_str(", ");
                    }
                    let name = self.name("c");
                    self.text.push_str(&name);
                    let b = self.b.binder(&name);
                    params.push(b);
                    self.scope.push((name, b));
                }
                self.text.push_str(") => ");
                let body = self.paren(body);
                self.scope.truncate(scope);
                self.b.closure(&params, body, implicit)
            }
            leaf => self.expr(leaf),
        }
    }
}

fn blocky(g: &G) -> bool {
    matches!(g, G::Block(..) | G::If(..) | G::Match(..) | G::Loop(..))
}

fn escape(c: char) -> String {
    match c {
        '\\' => "\\\\".to_string(),
        '"' => "\\\"".to_string(),
        '\'' => "\\'".to_string(),
        '\n' => "\\n".to_string(),
        '\r' => "\\r".to_string(),
        '\t' => "\\t".to_string(),
        '\0' => "\\0".to_string(),
        c => c.to_string(),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// Parsing the rendered text gives exactly the template the builder
    /// builds from the same generated tree (the reference).
    #[test]
    fn prop_parse_equals_builder(g in arb_g()) {
        let mut twin = Twin::new();
        let root = twin.expr(&g);
        let text = twin.text.clone();
        let built = twin.b.finish(root);
        let parsed = Template::parse("gen", &text);
        match (built, parsed) {
            (Ok(b), Ok(p)) => prop_assert_eq!(canon(&p), canon(&b), "text: {}", text),
            (b, p) => prop_assert!(false, "built {:?}, parsed {:?}\ntext: {}", b.err(), p.err(), text),
        }
    }

    /// Any text: never a panic.
    #[test]
    fn prop_arbitrary_text_never_panics(text in "\\PC{0,200}") {
        let _ = Template::parse("hostile", &text);
    }

    /// Token soup from the template vocabulary: never a panic, and whatever
    /// parses has at most one hole per token.
    #[test]
    fn prop_token_soup_never_panics(words in prop::collection::vec(prop::sample::select(vec![
        "{", "}", "(", ")", "[", "]", ",", ";", "=", "=>", ":", "<", ">", "|", "&", ".", "-",
        "let", "let_place", "in", "if", "else", "match", "loop", "break", "continue", "return",
        "yield", "from", "await", "spawn", "throw", "defer", "closure", "mut", "_", "x", "y",
        "add", "not", "int_cast", "i32", "overflow", "wrap", "1", "-2", "3.5", "'a'", "'l",
        "\"s\"", "null", "true", "${a}", "${b?}", "${c?=1}", "${...d}", "extern:f", "global:g::h",
        "apply:t", "step", "[step:", "by-value", "%",
    ]), 0..40)) {
        let text = words.join(" ");
        if let Ok(t) = Template::parse("soup", &text) {
            prop_assert!(t.hole_count() <= words.len());
        }
    }
}
