//! Property tests for every invariant in `dev/DIRECTIVES.md` §4:
//!
//! - lowering never panics on any tree (random kinds and shapes, error nodes,
//!   unmapped kinds, spans outside the source) and always yields valid HIR;
//! - every produced node and binder has an origin within the source, through
//!   template expansions;
//! - template hygiene: template temporaries never resolve to user binders,
//!   and user names never resolve to template binders;
//! - lowering is deterministic;
//! - and, against a reference: lowering preserves the meaning of calc
//!   programs (a direct evaluator of the generated program and an evaluator
//!   of the lowered HIR print the same values).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod common;
#[path = "support/sink.rs"]
mod sink;
#[path = "support/writer.rs"]
mod writer;

use std::collections::BTreeMap;

use hir_lang::{
    BinderId, Expr, ExprId, Hir, IdKind, Lit, NodeRef, OpKind, Pat, PrintOptions, Res, Stmt,
};
use intern_lang::Interner;
use lower_lang::{Budget, LangPolicy, Lowerer, Mapping};
use proptest::prelude::*;
use syntax_lang::{Element, Node, Span, Token};
use writer::{K, W};

// ------------------------------------------------------------ random trees

#[derive(Clone, Debug)]
enum T {
    Tok(u16, usize),
    Node(u16, Vec<T>),
}

fn tree_strategy() -> impl Strategy<Value = T> {
    let leaf = (sink::TOKEN_KINDS, 0..sink::TEXTS.len()).prop_map(|(k, t)| T::Tok(k, t));
    leaf.prop_recursive(10, 160, 6, |inner| {
        (sink::NODE_KINDS, prop::collection::vec(inner, 0..6)).prop_map(|(k, c)| T::Node(k, c))
    })
}

/// Writes `t` under a root module node, with exact spans.
fn write(t: &T) -> (Node<K>, String) {
    let mut w = W::new();
    w.open(sink::ROOT);
    // Iterative, so deep random trees cannot overflow the test's stack.
    enum Step<'a> {
        Visit(&'a T),
        Close,
    }
    let mut stack = vec![Step::Visit(t)];
    while let Some(s) = stack.pop() {
        match s {
            Step::Visit(T::Tok(k, i)) => {
                w.t(K(*k), sink::TEXTS[*i]);
            }
            Step::Visit(T::Node(k, kids)) => {
                w.open(K(*k));
                stack.push(Step::Close);
                for kid in kids.iter().rev() {
                    stack.push(Step::Visit(kid));
                }
            }
            Step::Close => {
                w.close();
            }
        }
    }
    w.close();
    w.finish()
}

/// Builds `t` with `Node::new` and arbitrary token spans (possibly outside
/// the source, reversed, or off character boundaries).
fn raw(t: &T, spans: &[(u32, u32)], next: &mut usize) -> Node<K> {
    match t {
        T::Tok(k, _) => {
            let (a, b) = spans[*next % spans.len()];
            *next += 1;
            Node::new(
                sink::ROOT,
                vec![Element::Token(Token::new(K(*k), Span::new(a, b)))],
            )
        }
        T::Node(k, kids) => {
            let children = kids
                .iter()
                .map(|c| match c {
                    T::Tok(kk, _) => {
                        let (a, b) = spans[*next % spans.len()];
                        *next += 1;
                        Element::Token(Token::new(K(*kk), Span::new(a, b)))
                    }
                    T::Node(..) => Element::Node(raw(c, spans, next)),
                })
                .collect();
            Node::new(K(*k), children)
        }
    }
}

fn lower_with(
    mapping: &Mapping<K>,
    tree: &Node<K>,
    src: &str,
    names: &mut Interner,
) -> lower_lang::Lowered {
    Lowerer::new(mapping)
        .lower(tree, src, names)
        .expect("small trees stay within the default budget")
}

/// Every resolved local reference names a binder with the reference's own
/// hygiene mark; and every template binder is referenced only from inside
/// its own expansion.
fn hygienic(hir: &Hir) -> bool {
    for i in 0..hir.count(IdKind::Path) {
        let path = hir.path(hir_lang::PathId::from_index(i).unwrap());
        let Res::Local(b) = path.res else { continue };
        let Some(seg) = hir.list(path.segments).first() else {
            return false;
        };
        let Some(binder) = hir.binder(b) else {
            return false;
        };
        if binder.name.mark != seg.name.mark {
            return false;
        }
    }
    true
}

/// A structural fingerprint: the printed form with origins, the problems, and
/// every binder.
fn fingerprint(out: &lower_lang::Lowered, names: &Interner) -> (String, String, String) {
    let printed = hir_lang::print_with(&out.hir, names, PrintOptions::default().with_origins(true));
    let problems = format!("{:?}", out.problems);
    let binders = format!(
        "{:?}",
        (0..out.hir.count(IdKind::Binder))
            .map(|i| out.hir.binder(BinderId::from_index(i).unwrap()).copied())
            .collect::<Vec<_>>()
    );
    (printed, problems, binders)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    /// Any tree lowers without panicking to valid HIR whose nodes all have
    /// origins inside the source, with every problem inside the source;
    /// templates stay hygienic (also after a resolver runs); and lowering is
    /// deterministic.
    #[test]
    fn prop_random_trees_lower_soundly(t in tree_strategy(), policy in 0usize..2) {
        let mapping = sink::sink(sink::policies()[policy]);
        let (tree, src) = write(&t);
        let mut names = Interner::new();
        let out = lower_with(&mapping, &tree, &src, &mut names);
        prop_assert_eq!(out.hir.validate(), Ok(()));
        prop_assert!(common::origins_within(&out.hir, src.len()));
        prop_assert!(out.problems.iter().all(|p| p.span.end().to_usize() <= src.len()));
        prop_assert!(out.problems.windows(2).all(|w| w[0].span.start() <= w[1].span.start()));
        prop_assert!(hygienic(&out.hir));
        let mut resolved = out.hir.clone();
        let _ = common::resolve_locals(&mut resolved);
        prop_assert!(hygienic(&resolved));
        prop_assert_eq!(resolved.validate(), Ok(()));

        // Deterministic: a second run (fresh interner) gives the same HIR.
        let mut names2 = Interner::new();
        let again = lower_with(&mapping, &tree, &src, &mut names2);
        prop_assert_eq!(fingerprint(&out, &names), fingerprint(&again, &names2));
    }

    /// Trees whose spans lie anywhere (outside the source, reversed, off
    /// character boundaries) never panic and still yield valid HIR.
    #[test]
    fn prop_arbitrary_spans_never_panic(
        t in tree_strategy(),
        spans in prop::collection::vec((0u32..40, 0u32..40), 1..16),
        src in "[a-zé€0-9 \"'\\\\]{0,24}",
    ) {
        let mapping = sink::sink(LangPolicy::default());
        let mut next = 0;
        let tree = raw(&t, &spans, &mut next);
        let mut names = Interner::new();
        let out = lower_with(&mapping, &tree, &src, &mut names);
        prop_assert_eq!(out.hir.validate(), Ok(()));
    }

    /// A tiny node budget is an error, never a panic or a hang; a budget large
    /// enough for the tree changes nothing.
    #[test]
    fn prop_budgets_are_errors(t in tree_strategy(), max in 0u64..40) {
        let mapping = sink::sink(LangPolicy::default());
        let (tree, src) = write(&t);
        let mut budget = Budget::default();
        budget.max_nodes = max;
        let mut names = Interner::new();
        match Lowerer::new(&mapping).with_budget(budget).lower(&tree, &src, &mut names) {
            Ok(out) => prop_assert_eq!(out.hir.validate(), Ok(())),
            Err(e) => prop_assert_eq!(e, lower_lang::LowerError::NodeBudget { limit: max }),
        }
    }
}

// ------------------------------------------------- reference: calc programs

#[derive(Clone, Debug)]
enum IntE {
    Num(i64),
    Var(usize),
    Bin(char, Box<IntE>, Box<IntE>),
    Neg(Box<IntE>),
}

#[derive(Clone, Debug)]
enum BoolE {
    Cmp(&'static str, IntE, IntE),
    And(Box<BoolE>, Box<BoolE>),
    Or(Box<BoolE>, Box<BoolE>),
    Not(Box<BoolE>),
}

#[derive(Clone, Debug)]
enum S {
    Assign(usize, IntE),
    Print(IntE),
    If(BoolE, Vec<S>, Vec<S>),
    Repeat(u8, Vec<S>),
}

const VARS: usize = 3;

fn int_e() -> impl Strategy<Value = IntE> {
    let leaf = prop_oneof![
        (0i64..20).prop_map(IntE::Num),
        (0..VARS).prop_map(IntE::Var)
    ];
    leaf.prop_recursive(4, 24, 2, |inner| {
        prop_oneof![
            (
                prop_oneof![Just('+'), Just('-'), Just('*')],
                inner.clone(),
                inner.clone()
            )
                .prop_map(|(o, a, b)| IntE::Bin(o, Box::new(a), Box::new(b))),
            inner.prop_map(|a| IntE::Neg(Box::new(a))),
        ]
    })
}

fn bool_e() -> impl Strategy<Value = BoolE> {
    let cmp = (
        prop_oneof![
            Just("<"),
            Just("<="),
            Just("=="),
            Just("!="),
            Just(">"),
            Just(">=")
        ],
        int_e(),
        int_e(),
    )
        .prop_map(|(o, a, b)| BoolE::Cmp(o, a, b));
    cmp.prop_recursive(3, 12, 2, |inner| {
        prop_oneof![
            (inner.clone(), inner.clone()).prop_map(|(a, b)| BoolE::And(Box::new(a), Box::new(b))),
            (inner.clone(), inner.clone()).prop_map(|(a, b)| BoolE::Or(Box::new(a), Box::new(b))),
            inner.prop_map(|a| BoolE::Not(Box::new(a))),
        ]
    })
}

fn stmts() -> impl Strategy<Value = Vec<S>> {
    let simple = prop_oneof![
        (0..VARS, int_e()).prop_map(|(v, e)| S::Assign(v, e)),
        int_e().prop_map(S::Print),
    ];
    let s = simple.prop_recursive(3, 16, 3, |inner| {
        prop_oneof![
            (
                bool_e(),
                prop::collection::vec(inner.clone(), 0..3),
                prop::collection::vec(inner.clone(), 0..3)
            )
                .prop_map(|(c, a, b)| S::If(c, a, b)),
            (0u8..4, prop::collection::vec(inner, 0..3)).prop_map(|(n, b)| S::Repeat(n, b)),
        ]
    });
    prop::collection::vec(s, 1..6)
}

/// Renders the program in calc's syntax. `Repeat` becomes a counted `while`
/// over a fresh counter variable.
fn render(prog: &[S]) -> String {
    fn int(e: &IntE, out: &mut String) {
        match e {
            IntE::Num(n) => out.push_str(&n.to_string()),
            IntE::Var(v) => out.push_str(&format!("v{v}")),
            IntE::Bin(o, a, b) => {
                out.push('(');
                int(a, out);
                out.push_str(&format!(" {o} "));
                int(b, out);
                out.push(')');
            }
            IntE::Neg(a) => {
                out.push_str("(-");
                int(a, out);
                out.push(')');
            }
        }
    }
    fn boolean(e: &BoolE, out: &mut String) {
        match e {
            BoolE::Cmp(o, a, b) => {
                out.push('(');
                int(a, out);
                out.push_str(&format!(" {o} "));
                int(b, out);
                out.push(')');
            }
            BoolE::And(a, b) | BoolE::Or(a, b) => {
                out.push('(');
                boolean(a, out);
                out.push_str(if matches!(e, BoolE::And(..)) {
                    " && "
                } else {
                    " || "
                });
                boolean(b, out);
                out.push(')');
            }
            BoolE::Not(a) => {
                out.push_str("(!");
                boolean(a, out);
                out.push(')');
            }
        }
    }
    fn stmt(s: &S, counter: &mut usize, out: &mut String) {
        match s {
            S::Assign(v, e) => {
                out.push_str(&format!("v{v} = "));
                int(e, out);
                out.push_str(";\n");
            }
            S::Print(e) => {
                out.push_str("print(");
                int(e, out);
                out.push_str(");\n");
            }
            S::If(c, a, b) => {
                out.push_str("if ");
                boolean(c, out);
                out.push_str(" {\n");
                for s in a {
                    stmt(s, counter, out);
                }
                out.push_str("} else {\n");
                for s in b {
                    stmt(s, counter, out);
                }
                out.push_str("}\n");
            }
            S::Repeat(n, body) => {
                let c = *counter;
                *counter += 1;
                out.push_str(&format!("let c{c} = 0;\nwhile c{c} < {n} {{\n"));
                for s in body {
                    stmt(s, counter, out);
                }
                out.push_str(&format!("c{c} = c{c} + 1;\n}}\n"));
            }
        }
    }
    let mut out = String::new();
    for v in 0..VARS {
        out.push_str(&format!("let v{v} = {v};\n"));
    }
    let mut counter = 0;
    for s in prog {
        stmt(s, &mut counter, &mut out);
    }
    out
}

/// The reference: evaluates the generated program directly. `None` on
/// arithmetic overflow (the HIR ops carry `overflow = error`).
fn reference(prog: &[S]) -> Option<Vec<i64>> {
    fn int(e: &IntE, vars: &[i64]) -> Option<i64> {
        Some(match e {
            IntE::Num(n) => *n,
            IntE::Var(v) => vars[*v],
            IntE::Bin(o, a, b) => {
                let (a, b) = (int(a, vars)?, int(b, vars)?);
                match o {
                    '+' => a.checked_add(b)?,
                    '-' => a.checked_sub(b)?,
                    _ => a.checked_mul(b)?,
                }
            }
            IntE::Neg(a) => int(a, vars)?.checked_neg()?,
        })
    }
    fn boolean(e: &BoolE, vars: &[i64]) -> Option<bool> {
        Some(match e {
            BoolE::Cmp(o, a, b) => {
                let (a, b) = (int(a, vars)?, int(b, vars)?);
                match *o {
                    "<" => a < b,
                    "<=" => a <= b,
                    "==" => a == b,
                    "!=" => a != b,
                    ">" => a > b,
                    _ => a >= b,
                }
            }
            BoolE::And(a, b) => boolean(a, vars)? && boolean(b, vars)?,
            BoolE::Or(a, b) => boolean(a, vars)? || boolean(b, vars)?,
            BoolE::Not(a) => !boolean(a, vars)?,
        })
    }
    fn run(stmts: &[S], vars: &mut Vec<i64>, out: &mut Vec<i64>) -> Option<()> {
        for s in stmts {
            match s {
                S::Assign(v, e) => vars[*v] = int(e, vars)?,
                S::Print(e) => out.push(int(e, vars)?),
                S::If(c, a, b) => {
                    if boolean(c, vars)? {
                        run(a, vars, out)?;
                    } else {
                        run(b, vars, out)?;
                    }
                }
                S::Repeat(n, body) => {
                    for _ in 0..*n {
                        run(body, vars, out)?;
                    }
                }
            }
        }
        Some(())
    }
    let mut vars: Vec<i64> = (0..VARS as i64).collect();
    let mut out = Vec::new();
    run(prog, &mut vars, &mut out)?;
    Some(out)
}

/// A minimal evaluator of the HIR calc lowers to (after `resolve_locals`).
struct Eval<'h> {
    hir: &'h Hir,
    print: intern_lang::Symbol,
    env: BTreeMap<BinderId, Val>,
    out: Vec<i64>,
    fuel: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Val {
    Int(i64),
    Bool(bool),
    Unit,
}

enum Flow {
    Break,
    Fail,
}

impl Eval<'_> {
    fn expr(&mut self, e: ExprId) -> Result<Val, Flow> {
        self.fuel = self.fuel.checked_sub(1).ok_or(Flow::Fail)?;
        let hir = self.hir;
        match *hir.expr(e) {
            Expr::Lit(Lit::Int(i)) => {
                let v = i64::try_from(i.value).map_err(|_| Flow::Fail)?;
                Ok(Val::Int(if i.negative { -v } else { v }))
            }
            Expr::Lit(Lit::Bool(b)) => Ok(Val::Bool(b)),
            Expr::Path(p) => match hir.path(p).res {
                Res::Local(b) => self.env.get(&b).copied().ok_or(Flow::Fail),
                _ => Err(Flow::Fail),
            },
            Expr::Op { op, args } => {
                let args = hir.list(args).to_vec();
                let vals: Vec<Val> = args
                    .iter()
                    .map(|&a| self.expr(a))
                    .collect::<Result<_, _>>()?;
                let int = |v: Val| match v {
                    Val::Int(i) => Ok(i),
                    _ => Err(Flow::Fail),
                };
                Ok(match (op.kind, vals.as_slice()) {
                    (OpKind::Add, [a, b]) => {
                        Val::Int(int(*a)?.checked_add(int(*b)?).ok_or(Flow::Fail)?)
                    }
                    (OpKind::Sub, [a, b]) => {
                        Val::Int(int(*a)?.checked_sub(int(*b)?).ok_or(Flow::Fail)?)
                    }
                    (OpKind::Mul, [a, b]) => {
                        Val::Int(int(*a)?.checked_mul(int(*b)?).ok_or(Flow::Fail)?)
                    }
                    (OpKind::Neg, [a]) => Val::Int(int(*a)?.checked_neg().ok_or(Flow::Fail)?),
                    (OpKind::Lt, [a, b]) => Val::Bool(int(*a)? < int(*b)?),
                    (OpKind::Le, [a, b]) => Val::Bool(int(*a)? <= int(*b)?),
                    (OpKind::Eq, [a, b]) => Val::Bool(int(*a)? == int(*b)?),
                    (OpKind::Ne, [a, b]) => Val::Bool(int(*a)? != int(*b)?),
                    (OpKind::Gt, [a, b]) => Val::Bool(int(*a)? > int(*b)?),
                    (OpKind::Ge, [a, b]) => Val::Bool(int(*a)? >= int(*b)?),
                    (OpKind::Not, [Val::Bool(a)]) => Val::Bool(!a),
                    _ => return Err(Flow::Fail),
                })
            }
            Expr::Assign {
                target,
                op: None,
                value,
            } => {
                let v = self.expr(value)?;
                let Expr::Path(p) = *hir.expr(target) else {
                    return Err(Flow::Fail);
                };
                let Res::Local(b) = hir.path(p).res else {
                    return Err(Flow::Fail);
                };
                let _ = self.env.insert(b, v);
                Ok(Val::Unit)
            }
            Expr::Block(block) => {
                for &s in hir.list(block.stmts) {
                    self.stmt(s)?;
                }
                match block.tail {
                    Some(t) => self.expr(t),
                    None => Ok(Val::Unit),
                }
            }
            Expr::If { cond, then, else_ } => match self.expr(cond)? {
                Val::Bool(true) => self.expr(then),
                Val::Bool(false) => match else_ {
                    Some(e) => self.expr(e),
                    None => Ok(Val::Unit),
                },
                _ => Err(Flow::Fail),
            },
            Expr::Loop {
                body,
                step: None,
                label: None,
            } => loop {
                match self.expr(body) {
                    Ok(_) => {}
                    Err(Flow::Break) => return Ok(Val::Unit),
                    Err(Flow::Fail) => return Err(Flow::Fail),
                }
            },
            Expr::Break {
                label: None,
                value: None,
            } => Err(Flow::Break),
            Expr::Call { callee, args } => {
                let Expr::Path(p) = *hir.expr(callee) else {
                    return Err(Flow::Fail);
                };
                let seg = hir
                    .list(hir.path(p).segments)
                    .first()
                    .copied()
                    .ok_or(Flow::Fail)?;
                if seg.name.sym != self.print {
                    return Err(Flow::Fail);
                }
                let args = hir.list(args).to_vec();
                for a in args {
                    match self.expr(a.value)? {
                        Val::Int(i) => self.out.push(i),
                        _ => return Err(Flow::Fail),
                    }
                }
                Ok(Val::Unit)
            }
            _ => Err(Flow::Fail),
        }
    }

    fn stmt(&mut self, s: hir_lang::StmtId) -> Result<(), Flow> {
        let hir = self.hir;
        match *hir.stmt(s) {
            Stmt::Expr(e) => self.expr(e).map(|_| ()),
            Stmt::Let {
                pat,
                init: Some(init),
                ..
            } => {
                let v = self.expr(init)?;
                let Pat::Bind { binder, .. } = *hir.pat(pat) else {
                    return Err(Flow::Fail);
                };
                let _ = self.env.insert(binder, v);
                Ok(())
            }
            _ => Err(Flow::Fail),
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// Lowering preserves meaning: the reference evaluation of a random calc
    /// program equals the evaluation of the HIR lowered from its text
    /// (parsed by lang-forge, `while` and `&&`/`||` through templates).
    #[test]
    fn prop_lowering_preserves_calc_semantics(prog in stmts()) {
        let expected = reference(&prog);
        prop_assume!(expected.is_some());
        let src = render(&prog);
        let lang = common::calc_language();
        let mapping = common::calc_mapping(&lang);
        let mut names = Interner::new();
        let out = common::lower_calc(&lang, &mapping, &src, &mut names);
        prop_assert!(out.problems.is_empty(), "{:?}\n{}", out.problems, src);
        let mut hir = out.hir;
        let (_, open) = common::resolve_locals(&mut hir);
        // Only `print` stays unresolved.
        let prints = src.matches("print(").count();
        prop_assert_eq!(open, prints);
        let ItemKind::Module { body: Some(body), .. } = hir.item(hir.root()).kind else {
            return Err(TestCaseError::fail("no module body"));
        };
        let mut eval = Eval { hir: &hir, print: names.intern("print"), env: BTreeMap::new(), out: Vec::new(), fuel: 1_000_000 };
        let ran = eval.expr(body);
        prop_assert!(ran.is_ok(), "the HIR did not evaluate:\n{}", src);
        prop_assert_eq!(Some(eval.out), expected, "\n{}", src);
    }

    /// Random token soup in calc's vocabulary, parsed by lang-forge (deep,
    /// half-typed trees full of error nodes and missing parts), lowers to
    /// valid HIR with in-source origins and problems, deterministically.
    #[test]
    fn prop_calc_token_soup_lowers_soundly(words in prop::collection::vec(0usize..CALC_WORDS.len(), 0..120)) {
        let src = words.iter().map(|&i| CALC_WORDS[i]).collect::<Vec<_>>().join(" ");
        let lang = common::calc_language();
        let mapping = common::calc_mapping(&lang);
        let mut names = Interner::new();
        let out = common::lower_calc(&lang, &mapping, &src, &mut names);
        prop_assert_eq!(out.hir.validate(), Ok(()));
        prop_assert!(common::origins_within(&out.hir, src.len()));
        prop_assert!(out.problems.iter().all(|p| p.span.end().to_usize() <= src.len()));
        prop_assert!(hygienic(&out.hir));
        let mut names2 = Interner::new();
        let again = common::lower_calc(&lang, &mapping, &src, &mut names2);
        prop_assert_eq!(fingerprint(&out, &names), fingerprint(&again, &names2));
    }
}

const CALC_WORDS: &[&str] = &[
    "fn", "let", "while", "if", "else", "return", "(", ")", "{", "}", ";", ",", "=", "+", "-", "*",
    "/", "%", "==", "!=", "<", ">=", "&&", "||", "!", "x", "y", "f", "acc", "0", "1", "42", "x",
    "=", "1", ";", "{", "}",
];

use hir_lang::ItemKind;

#[test]
fn test_deep_trees_lower_without_recursion() {
    // 100,000 nested prefix negations and 100,000 nested blocks.
    let lang = common::calc_language();
    let mapping = common::calc_mapping(&lang);
    let depth = 100_000;
    let src = format!("{}1{};", "-(".repeat(depth / 2), ")".repeat(depth / 2));
    // lang-forge caps its parse depth, so build the deep tree directly.
    let _ = &src;
    let mut b = syntax_lang::Builder::new();
    let k = |n: &str| lang.kind(n).unwrap();
    let mut text = String::new();
    b.start_node(k("program"));
    b.start_node(k("expr_stmt"));
    b.start_node(k("expr"));
    for _ in 0..depth {
        b.start_node(k("prefix"));
        let at = text.len() as u32;
        text.push('-');
        b.token(Token::new(k("-"), Span::new(at, at + 1)));
    }
    let at = text.len() as u32;
    text.push('1');
    b.token(Token::new(k("NUMBER"), Span::new(at, at + 1)));
    for _ in 0..depth {
        b.finish_node();
    }
    b.finish_node();
    let at = text.len() as u32;
    text.push(';');
    b.token(Token::new(k(";"), Span::new(at, at + 1)));
    b.finish_node();
    b.finish_node();
    let tree = b.finish().unwrap();
    let mut names = Interner::new();
    let out = lower_lang::lower(&tree, &text, &mapping, &mut names).unwrap();
    assert!(
        out.problems.is_empty(),
        "{:?}",
        &out.problems[..out.problems.len().min(3)]
    );
    let mut negs = 0;
    hir_lang::walk(&out.hir, |n| {
        if let NodeRef::Expr(e) = n {
            if matches!(out.hir.expr(e), Expr::Op { op, .. } if op.kind == OpKind::Neg) {
                negs += 1;
            }
        }
    });
    assert_eq!(negs, depth);
}
