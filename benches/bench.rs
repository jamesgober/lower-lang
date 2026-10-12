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
//! - `oop/*`: Mox classes under the 0.4.1 policies: methods with the
//!   implicit receiver, closures capturing it, nested loops with `break 2`
//!   and `continue 2`, rest parameters and spread arguments through one
//!   per-archetype `...`, and the `include` family through conditional
//!   templates; `oop_off/*` lowers the same trees with the receiver and
//!   numeric levels switched off, so the difference is their cost.
//! - `template_text/*`: parsing template texts (`Template::parse`): the Mox
//!   sketch's templates registered into a mapping, and one text just under
//!   the 1 MiB cap (12,878 statements, about 155,000 template nodes).
//!
//! Sizes are counted in syntax-tree *elements* (nodes plus tokens, trivia
//! included) and printed when each benchmark starts.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stdout)]

#[path = "../tests/common/mod.rs"]
mod common;
#[path = "../tests/support/mox.rs"]
mod mox;
#[path = "../tests/support/mox_oop.rs"]
mod oop;

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

/// The template texts of the Mox sketch (`[semantics.templates]`).
const MOX_TEMPLATES: [(&str, &str); 19] = [
    ("bool_and", "if ${lhs} { not(not(${rhs})) } else { false }"),
    ("bool_or", "if ${lhs} { true } else { not(not(${rhs})) }"),
    ("pre_increment", "let_place p = ${place} in p = add(p, 1)"),
    ("pre_decrement", "let_place p = ${place} in p = sub(p, 1)"),
    (
        "post_decrement",
        "let_place p = ${place} in { let old = p; p = sub(old, 1); old }",
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
        "{ let g = extern:mox.iter(${from});
  loop {
    match extern:mox.next(g) {
      null => break extern:mox.gen_return(g),
      (k, v) => { yield k => v; }
    }
  } }",
    ),
];

/// One template text just under `MAX_TEMPLATE_TEXT`: a chain of `let`s,
/// each reading the previous binder, with ops, holes, and a `match`.
fn large_template() -> String {
    let mut s = String::from("{ let v0 = ${x}; ");
    let mut i = 1;
    while s.len() < lower_lang::MAX_TEMPLATE_TEXT - 200 {
        s.push_str(&format!(
            "let v{i} = match add<overflow=wrap>(v{}, {i}) {{ 0 => ${{y}}, n => mul(n, 2) }}; ",
            i - 1
        ));
        i += 1;
    }
    s.push_str(&format!("v{} }}", i - 1));
    s
}

fn template_text(c: &mut Criterion) {
    let mut g = c.benchmark_group("template_text");
    let bytes: usize = MOX_TEMPLATES.iter().map(|(_, t)| t.len()).sum();
    println!(
        "template_text/mox: {} templates, {bytes} bytes",
        MOX_TEMPLATES.len()
    );
    g.throughput(Throughput::Bytes(bytes as u64));
    g.bench_function("mox", |b| {
        b.iter(|| {
            let mut m = lower_lang::MappingBuilder::<u8>::new();
            for (name, text) in MOX_TEMPLATES {
                let _ = m.parse_template(name, text).expect("parses");
            }
            m
        });
    });
    let large = large_template();
    let template = lower_lang::Template::parse("large", &large).expect("parses");
    println!(
        "template_text/large: {} bytes, {} holes",
        large.len(),
        template.hole_count()
    );
    g.sample_size(10);
    g.throughput(Throughput::Bytes(large.len() as u64));
    g.bench_function("large", |b| {
        b.iter(|| lower_lang::Template::parse("large", &large).expect("parses"));
    });
    g.finish();
}

/// `n` classes like this (plus an `include_once` and a spread call each):
///
/// ```php
/// class Ci {
///     public $v;
///     public function __construct($v) { $this->v = $v; }
///     public function run(...$xs) {
///         $t = 0;
///         while ($t < 3) { $t = $t + 1; loop { if ($t < 2) { continue 2; } break 2; } }
///         return function ($n) { return $this->v + $n; };
///     }
/// }
/// include_once 'ci.mox';
/// f(...$xs);
/// ```
fn oop_program(n: usize) -> (Node<oop::X>, String) {
    use oop::{W, X};
    let mut w = W::new();
    w.open(X::File);
    for i in 0..n {
        w.class(&format!("C{i}"), None, |w| {
            w.property("v", None);
            w.function(X::Method, &[X::PublicKw], "__construct", &["v"], |w| {
                w.stmt(|w| {
                    w.assign(
                        |w| {
                            w.prop("this", "v");
                        },
                        |w| {
                            w.var("v");
                        },
                    );
                });
            });
            w.function(X::Method, &[X::PublicKw], "run", &["...xs"], |w| {
                w.stmt(|w| {
                    w.assign(
                        |w| {
                            w.var("t");
                        },
                        |w| {
                            w.num(0);
                        },
                    );
                });
                w.while_(
                    |w| {
                        w.bin(
                            X::Lt,
                            |w| {
                                w.var("t");
                            },
                            |w| {
                                w.num(3);
                            },
                        );
                    },
                    |w| {
                        w.stmt(|w| {
                            w.assign(
                                |w| {
                                    w.var("t");
                                },
                                |w| {
                                    w.bin(
                                        X::Plus,
                                        |w| {
                                            w.var("t");
                                        },
                                        |w| {
                                            w.num(1);
                                        },
                                    );
                                },
                            );
                        });
                        w.open(X::Loop).t(X::LoopKw, "loop");
                        w.block(|w| {
                            w.if_(
                                |w| {
                                    w.bin(
                                        X::Lt,
                                        |w| {
                                            w.var("t");
                                        },
                                        |w| {
                                            w.num(2);
                                        },
                                    );
                                },
                                |w| {
                                    w.jump(X::ContinueKw, Some("2"));
                                },
                            );
                            w.jump(X::BreakKw, Some("2"));
                        });
                        w.close();
                    },
                );
                w.ret(|w| {
                    w.closure(false, &["n"], &[], |w| {
                        w.ret(|w| {
                            w.bin(
                                X::Plus,
                                |w| {
                                    w.prop("this", "v");
                                },
                                |w| {
                                    w.var("n");
                                },
                            );
                        });
                    });
                });
            });
        });
        w.stmt(|w| {
            w.open(X::Include)
                .t(X::IncludeOnceKw, "include_once")
                .t(X::Str, "'ci.mox'")
                .close();
        });
        w.stmt(|w| {
            w.call(
                |w| {
                    w.t(X::Ident, "f");
                },
                1,
                &[true],
                |w, _| {
                    w.var("xs");
                },
            );
        });
    }
    w.close();
    w.finish()
}

fn oop_bench(c: &mut Criterion) {
    for (group, mapping) in [
        ("oop", oop::mapping()),
        ("oop_off", oop::mapping_with(false, false)),
    ] {
        let mut g = c.benchmark_group(group);
        g.sample_size(10);
        for (label, n) in [("100k", 520), ("1m", 5_200)] {
            let (tree, src) = oop_program(n);
            let elems = elements(&tree);
            println!(
                "{group}/{label}: {elems} syntax-tree elements, {} bytes",
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
}

criterion_group!(
    benches,
    calc,
    templates,
    deep,
    php,
    template_text,
    oop_bench
);
criterion_main!(benches);
