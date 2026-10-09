//! Lowering throughput at realistic scale.
//!
//! - `calc/*`: programs in the calc worked example (forged by lang-forge),
//!   parsed once outside the measurement; the measurement is `lower` alone
//!   (lowering, `finish_lenient` validation, and problem collection).
//! - `templates/*`: a calc program that is mostly `while` loops and `&&`, so
//!   much of the HIR comes from template instantiation.
//! - `deep/*`: one 100,000-deep chain of prefix operators.
//! - `php/*`: Mox (PHP) code under `LangPolicy::php()`: functions with
//!   `foreach` and `catch` whose variables are function-scoped, interpolated
//!   strings, heredocs, and group `use` declarations.
//!
//! Sizes are counted in syntax-tree *elements* (nodes plus tokens, trivia
//! included) and printed when each benchmark starts.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stdout)]

#[path = "../tests/common/mod.rs"]
mod common;
#[path = "../tests/support/mox.rs"]
mod mox;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use intern_lang::Interner;
use lang_forge::Language;
use syntax_lang::{Node, Span, Token};

/// One function and some top-level code, repeated `n` times.
fn calc_program(n: usize) -> String {
    let mut s = String::new();
    for i in 0..n {
        s.push_str(&format!(
            "fn f{i}(a, b) {{\n    let acc = a * 2 + b;\n    if acc > 10 {{ acc = acc - 1; }} else {{ acc = acc + 1; }}\n    return g(acc, b, (a + b) * (a - b));\n}}\nlet r{i} = f{i}({i}, 3) % 7;\n"
        ));
    }
    s
}

/// Template-heavy code: loops and short-circuit conditions.
fn loop_program(n: usize) -> String {
    let mut s = String::new();
    for i in 0..n {
        s.push_str(&format!(
            "let i{i} = 0;\nwhile i{i} < 10 && i{i} != 5 || i{i} == 7 {{ i{i} = i{i} + 1; }}\n"
        ));
    }
    s
}

fn elements<K>(tree: &Node<K>) -> u64 {
    (tree.descendants().count() + tree.tokens().count()) as u64
}

fn bench_lowering(c: &mut Criterion, group: &str, lang: &Language, programs: &[(&str, String)]) {
    let mapping = common::calc_mapping(lang);
    let mut g = c.benchmark_group(group);
    g.sample_size(10);
    for (label, src) in programs {
        let parse = lang.parse(src);
        assert!(!parse.has_errors(), "benchmark input parses cleanly");
        let tree = parse.tree();
        let n = elements(tree);
        println!(
            "{group}/{label}: {n} syntax-tree elements, {} bytes",
            src.len()
        );
        g.throughput(Throughput::Elements(n));
        g.bench_with_input(BenchmarkId::from_parameter(label), tree, |b, tree| {
            b.iter(|| {
                let mut names = Interner::new();
                let out =
                    lower_lang::lower(tree, src, &mapping, &mut names).expect("within budget");
                assert!(out.problems.is_empty());
                out.hir
            });
        });
    }
    g.finish();
}

fn calc(c: &mut Criterion) {
    let lang = common::calc_language();
    let programs = [("100k", calc_program(1_100)), ("1m", calc_program(11_000))];
    bench_lowering(c, "calc", &lang, &programs);
}

fn templates(c: &mut Criterion) {
    let lang = common::calc_language();
    let programs = [("100k", loop_program(1_500)), ("1m", loop_program(15_000))];
    bench_lowering(c, "templates", &lang, &programs);
}

fn deep(c: &mut Criterion) {
    let lang = common::calc_language();
    let mapping = common::calc_mapping(&lang);
    let k = |n: &str| lang.kind(n).expect("calc kind");
    let depth = 100_000;
    let mut b = syntax_lang::Builder::new();
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
    let tree = b.finish().expect("balanced");
    let n = elements(&tree);
    println!("deep/100k_prefix_chain: {n} syntax-tree elements");
    let mut g = c.benchmark_group("deep");
    g.sample_size(10);
    g.throughput(Throughput::Elements(n));
    g.bench_function("100k_prefix_chain", |bch| {
        bch.iter(|| {
            let mut names = Interner::new();
            lower_lang::lower(&tree, &text, &mapping, &mut names)
                .expect("within budget")
                .hir
        });
    });
    g.finish();
}

/// `n` Mox functions, each:
///
/// ```text
/// use App\Models\{User, Post as P};
/// function fN($items) {
///     foreach ($items as $k => $v) { echo "item {$k}: $v!"; }
///     try { echo $k; } catch (Ex $e) { echo <<<EOT
///         at {$e}
///         EOT; }
///     echo $v, $e;
/// }
/// ```
fn php_program(n: usize) -> (Node<mox::M>, String) {
    use mox::{M, W};
    let mut w = W::new();
    w.open(M::File);
    for i in 0..n {
        w.open(M::UseStmt).t(M::UseKw, "use").open(M::UseDecl);
        w.open(M::NsName)
            .t(M::Ident, "App")
            .t(M::NsSep, "\\")
            .t(M::Ident, "Models")
            .close();
        w.t(M::NsSep, "\\").open(M::UseGroup).t(M::UseOpen, "{");
        w.open(M::UseDecl)
            .t(M::Ident, "User")
            .close()
            .t(M::Comma, ",");
        w.open(M::UseDecl)
            .t(M::Ident, "Post")
            .t(M::As, "as")
            .t(M::Ident, "P")
            .close();
        w.t(M::UseClose, "}")
            .close()
            .close()
            .t(M::Semi, ";")
            .close();
        w.open(M::FnDecl)
            .t(M::Function, "function")
            .t(M::Ident, &format!("f{i}"))
            .t(M::LParen, "(");
        w.open(M::Params)
            .open(M::Param)
            .var("items")
            .close()
            .close();
        w.t(M::RParen, ")").open(M::Block).t(M::LBrace, "{");
        w.open(M::Foreach)
            .t(M::ForeachKw, "foreach")
            .t(M::LParen, "(")
            .var("items")
            .t(M::As, "as")
            .var("k")
            .t(M::FatArrow, "=>")
            .var("v")
            .t(M::RParen, ")");
        w.open(M::Block).t(M::LBrace, "{");
        w.open(M::Echo).t(M::EchoKw, "echo");
        w.open(M::DqString)
            .t(M::DQuote, "\"")
            .t(M::StrPart, "item ")
            .t(M::CurlyOpen, "{")
            .var("k")
            .t(M::CurlyClose, "}")
            .t(M::StrPart, ": ")
            .var("v")
            .t(M::StrPart, "!")
            .t(M::DQuote, "\"")
            .close();
        w.t(M::Semi, ";").close();
        w.t(M::RBrace, "}").close().close();
        w.open(M::ExprStmt).open(M::Try).t(M::TryKw, "try");
        w.open(M::Block)
            .t(M::LBrace, "{")
            .open(M::Echo)
            .t(M::EchoKw, "echo")
            .var("k")
            .t(M::Semi, ";")
            .close()
            .t(M::RBrace, "}")
            .close();
        w.open(M::Catch)
            .t(M::CatchKw, "catch")
            .t(M::LParen, "(")
            .t(M::Ident, "Ex")
            .var("e")
            .t(M::RParen, ")");
        w.open(M::Block)
            .t(M::LBrace, "{")
            .open(M::Echo)
            .t(M::EchoKw, "echo");
        w.open(M::HeredocStr)
            .t(M::HeredocOpen, "<<<EOT\n")
            .t(M::StrPart, "    at ")
            .t(M::CurlyOpen, "{")
            .var("e")
            .t(M::CurlyClose, "}")
            .t(M::StrPart, "\n")
            .t(M::HeredocClose, "    EOT")
            .close();
        w.t(M::Semi, ";").close().t(M::RBrace, "}").close();
        w.close().close().t(M::Semi, ";").close();
        w.open(M::Echo)
            .t(M::EchoKw, "echo")
            .var("v")
            .t(M::Comma, ",")
            .var("e")
            .t(M::Semi, ";")
            .close();
        w.t(M::RBrace, "}").close().close();
    }
    w.close();
    w.finish()
}

fn php(c: &mut Criterion) {
    let mapping = mox::mox_mapping();
    let mut g = c.benchmark_group("php");
    g.sample_size(10);
    for (label, n) in [("100k", 1_050), ("1m", 10_500)] {
        let (tree, src) = php_program(n);
        let elems = elements(&tree);
        println!(
            "php/{label}: {elems} syntax-tree elements, {} bytes",
            src.len()
        );
        g.throughput(Throughput::Elements(elems));
        g.bench_with_input(BenchmarkId::from_parameter(label), &tree, |b, tree| {
            b.iter(|| {
                let mut names = Interner::new();
                let out =
                    lower_lang::lower(tree, &src, &mapping, &mut names).expect("within budget");
                assert!(out.problems.is_empty(), "{:?}", &out.problems[..1]);
                out.hir
            });
        });
    }
    g.finish();
}

criterion_group!(benches, calc, templates, deep, php);
criterion_main!(benches);
