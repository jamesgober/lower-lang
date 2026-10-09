//! Mox-flavoured constructs, built with syntax-lang's `Builder` and lowered
//! through a dynamic-language mapping: `$variables`, `foreach` through its
//! hygienic template, by-reference parameters and place arguments, implicit
//! locals, `global`/`static`, `use` captures, `->` members (static and
//! dynamic), `$$var`, `$a[]`, classes with mixins and static properties,
//! try/catch/finally, generators, map literals, and string escapes.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;
#[path = "support/mox.rs"]
mod mox;

use hir_lang::{
    ArgKind, AttrValue, BindMode, BinderKind, CaptureMode, Effects, ExpnKind, Expr, Hir, IdKind,
    ItemKind, Lit, MixinAction, NodeRef, Ns, ParamKind, Pat, Res, Stmt,
};
use intern_lang::Interner;
use lower_lang::{Lowered, ProblemKind};
use mox::{M, W, mox_mapping};

fn lower(w: W) -> (Lowered, Interner, String) {
    let (tree, src) = w.finish();
    let mapping = mox_mapping();
    let mut names = Interner::new();
    let out = lower_lang::lower(&tree, &src, &mapping, &mut names).expect("within budget");
    assert_eq!(out.hir.validate(), Ok(()));
    assert!(common::origins_within(&out.hir, src.len()), "origins");
    (out, names, src)
}

/// All expressions in walk order.
fn exprs(hir: &Hir) -> Vec<Expr> {
    let mut v = Vec::new();
    hir_lang::walk(hir, |n| {
        if let NodeRef::Expr(e) = n {
            v.push(*hir.expr(e));
        }
    });
    v
}

/// `function NAME(PARAMS) { BODY }` — `params` writes the parameter nodes.
fn func(w: &mut W, name: &str, params: impl FnOnce(&mut W), body: impl FnOnce(&mut W)) {
    w.open(M::FnDecl)
        .t(M::Function, "function")
        .t(M::Ident, name)
        .t(M::LParen, "(");
    w.open(M::Params);
    params(w);
    w.close();
    w.t(M::RParen, ")");
    block(w, body);
    w.close();
}

fn block(w: &mut W, body: impl FnOnce(&mut W)) {
    w.open(M::Block).t(M::LBrace, "{");
    body(w);
    w.t(M::RBrace, "}").close();
}

/// `$name = <value>;`
fn assign(w: &mut W, name: &str, value: impl FnOnce(&mut W)) {
    w.open(M::ExprStmt).open(M::Assign).var(name).t(M::Eq, "=");
    value(w);
    w.close().t(M::Semi, ";").close();
}

#[test]
fn test_foreach_template_is_hygienic() {
    // $it = 10; foreach ($items as $k => $v) { echo $k, $v, $it; }
    let mut w = W::new();
    w.open(M::File);
    assign(&mut w, "it", |w| {
        w.t(M::Num, "10");
    });
    w.open(M::Foreach)
        .t(M::ForeachKw, "foreach")
        .t(M::LParen, "(")
        .var("items")
        .t(M::As, "as");
    w.var("k").t(M::FatArrow, "=>").var("v").t(M::RParen, ")");
    block(&mut w, |w| {
        w.open(M::Echo)
            .t(M::EchoKw, "echo")
            .var("k")
            .t(M::Comma, ",")
            .var("v");
        w.t(M::Comma, ",").var("it").t(M::Semi, ";").close();
    });
    w.close().close();
    let (out, mut names, _) = lower(w);
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let hir = out.hir;

    // Two template expansions: foreach, and echo nested in its body (a
    // separate instantiation in user code, so its parent is the root); and,
    // under PHP scoping, one `hoist` desugaring per function-scoped binding
    // (`$k`, `$v`), whose temporaries no user name can capture.
    let expns: Vec<_> = (1..=hir.count(IdKind::Expansion) as u32)
        .filter_map(|i| hir.expansion(hir_lang::ExpnId::from_u32(i)).copied())
        .collect();
    assert_eq!(expns.len(), 4);
    assert!(expns.iter().all(|e| e.parent.is_root()));
    let mut kinds: Vec<ExpnKind> = expns.iter().map(|e| e.kind).collect();
    kinds.sort_by_key(|k| *k as u8);
    assert_eq!(
        kinds,
        [
            ExpnKind::Template,
            ExpnKind::Desugar,
            ExpnKind::Desugar,
            ExpnKind::Desugar
        ]
    );
    let hoist = names.intern("hoist");
    assert_eq!(expns.iter().filter(|e| e.name == hoist).count(), 2);

    // The template's `it` carries the foreach expansion's mark; the user's
    // `$it` (implicitly declared) does not.
    let it = names.intern("it");
    let binders: Vec<_> = (0..hir.count(IdKind::Binder))
        .filter_map(|i| {
            hir.binder(hir_lang::BinderId::from_index(i).unwrap())
                .copied()
        })
        .filter(|b| b.name.sym == it)
        .collect();
    assert_eq!(binders.len(), 2);
    assert!(binders.iter().any(|b| b.name.mark.is_root()));
    assert!(binders.iter().any(|b| !b.name.mark.is_root()));

    // Resolve user names: the user's `$it` in the loop body resolves to the
    // user's binder, never to the template's.
    let mut hir = hir;
    let (resolved, _) = common::resolve_locals(&mut hir);
    assert!(resolved >= 4);
    for i in 0..hir.count(IdKind::Path) {
        let p = *hir.path(hir_lang::PathId::from_index(i).unwrap());
        if let Res::Local(b) = p.res {
            let seg_mark = hir.list(p.segments)[0].name.mark;
            assert_eq!(
                hir.binder(b).unwrap().name.mark,
                seg_mark,
                "a resolution crossed marks"
            );
        }
    }
    let printed = hir_lang::print(&hir, &names);
    assert!(printed.contains("→ extern mox.iter)"), "{printed}");
    assert!(printed.contains("(lit null)"), "{printed}");
}

#[test]
fn test_by_ref_params_place_args_and_implicit_locals() {
    // function inc(&$x, $by = 1) { $x = $x + $by; $tmp = $x; global $g; static $n = 0; }
    // inc($counter, 2); f($a . "s\n");
    let mut w = W::new();
    w.open(M::File);
    func(
        &mut w,
        "inc",
        |w| {
            w.open(M::Param)
                .t(M::Amp, "&")
                .var("x")
                .close()
                .t(M::Comma, ",");
            w.open(M::Param)
                .var("by")
                .t(M::Eq, "=")
                .t(M::Num, "1")
                .close();
        },
        |w| {
            assign(w, "x", |w| {
                w.open(M::Binary).var("x").t(M::Plus, "+").var("by").close();
            });
            assign(w, "tmp", |w| {
                w.var("x");
            });
            w.open(M::Global)
                .t(M::GlobalKw, "global")
                .var("g")
                .t(M::Semi, ";")
                .close();
            w.open(M::Static)
                .t(M::StaticKw, "static")
                .var("n")
                .t(M::Eq, "=")
                .t(M::Num, "0");
            w.t(M::Semi, ";").close();
            assign(w, "g", |w| {
                w.t(M::Num, "5");
            });
        },
    );
    w.open(M::ExprStmt)
        .open(M::Call)
        .t(M::Ident, "inc")
        .t(M::LParen, "(")
        .open(M::Args);
    w.var("counter")
        .t(M::Comma, ",")
        .t(M::Num, "2")
        .close()
        .t(M::RParen, ")")
        .close();
    w.t(M::Semi, ";").close();
    w.open(M::ExprStmt)
        .open(M::Call)
        .t(M::Ident, "f")
        .t(M::LParen, "(")
        .open(M::Args);
    w.open(M::Binary)
        .var("a")
        .t(M::Dot, ".")
        .t(M::Str, "\"s\\n\"")
        .close();
    w.close().t(M::RParen, ")").close().t(M::Semi, ";").close();
    w.close();
    let (out, names, _) = lower(w);
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let hir = &out.hir;

    let mut params = Vec::new();
    hir_lang::walk(hir, |n| {
        if let NodeRef::Param(p) = n {
            params.push(*hir.param(p));
        }
    });
    assert_eq!(params.len(), 2);
    assert!(params[0].by_ref && !params[1].by_ref);
    assert!(params[1].default.is_some());
    assert!(params.iter().all(|p| p.kind == ParamKind::Normal));

    // `$tmp` is hoisted (implicit local); `$x` (a parameter), `$g` (global),
    // and `$n` (static) are not. The function throws (Mox: any function may).
    let ItemKind::Module { items, .. } = hir.item(hir.root()).kind else {
        panic!()
    };
    let f = hir.list(items)[0];
    let ItemKind::Fn(def) = hir.item(f).kind else {
        panic!()
    };
    assert!(def.effects.contains(Effects::THROWS));
    let Expr::Block(outer) = *hir.expr(def.body.unwrap()) else {
        panic!()
    };
    let hoisted: Vec<_> = hir
        .list(outer.stmts)
        .iter()
        .map(|&s| match *hir.stmt(s) {
            Stmt::Let {
                pat, init: None, ..
            } => match *hir.pat(pat) {
                Pat::Bind { binder, .. } => names
                    .resolve(hir.binder(binder).unwrap().name.sym)
                    .unwrap()
                    .to_owned(),
                _ => String::new(),
            },
            _ => String::new(),
        })
        .collect();
    assert_eq!(hoisted, ["tmp"]);

    // Place arguments: `$counter` (a place) is passed as a place, `2` is not;
    // `.` became a call of the host's concat; the string escape decoded.
    let calls: Vec<_> = exprs(hir)
        .into_iter()
        .filter_map(|e| match e {
            Expr::Call { args, .. } => Some(hir.list(args).to_vec()),
            _ => None,
        })
        .collect();
    let inc_args = calls
        .iter()
        .find(|a| a.len() == 2 && a[0].place)
        .expect("inc call");
    assert!(!inc_args[1].place);
    assert!(inc_args.iter().all(|a| a.kind == ArgKind::Positional));
    let printed = hir_lang::print(hir, &names);
    assert!(printed.contains("extern mox.concat"), "{printed}");
    assert!(printed.contains("(lit \"s\\n\")"), "{printed}");
}

#[test]
fn test_closures_members_varvars_and_append() {
    // $f = function ($y) use (&$total) { $total = $total + $y; };
    // $o->m($x); $o->$name(); $o->prop = $$var; $arr[] = 1; $arr[0] = $o->prop;
    let mut w = W::new();
    w.open(M::File);
    assign(&mut w, "f", |w| {
        w.open(M::Closure)
            .t(M::Function, "function")
            .t(M::LParen, "(")
            .open(M::Params);
        w.open(M::Param).var("y").close().close().t(M::RParen, ")");
        w.t(M::UseKw, "use").t(M::LParen, "(").open(M::Uses);
        w.open(M::Capture)
            .t(M::Amp, "&")
            .var("total")
            .close()
            .close()
            .t(M::RParen, ")");
        block(w, |w| {
            assign(w, "total", |w| {
                w.open(M::Binary)
                    .var("total")
                    .t(M::Plus, "+")
                    .var("y")
                    .close();
            });
        });
        w.close();
    });
    w.open(M::ExprStmt)
        .open(M::Member)
        .var("o")
        .t(M::Arrow, "->")
        .t(M::Ident, "m")
        .t(M::LParen, "(");
    w.open(M::Args)
        .var("x")
        .close()
        .t(M::RParen, ")")
        .close()
        .t(M::Semi, ";")
        .close();
    w.open(M::ExprStmt)
        .open(M::DynMember)
        .var("o")
        .t(M::Arrow, "->")
        .var("name")
        .t(M::LParen, "(");
    w.open(M::Args)
        .close()
        .t(M::RParen, ")")
        .close()
        .t(M::Semi, ";")
        .close();
    w.open(M::ExprStmt)
        .open(M::Assign)
        .open(M::Member)
        .var("o")
        .t(M::Arrow, "->")
        .t(M::Ident, "prop")
        .close();
    w.t(M::Eq, "=")
        .open(M::VarVar)
        .t(M::Dollar, "$")
        .var("var")
        .close()
        .close()
        .t(M::Semi, ";")
        .close();
    w.open(M::ExprStmt)
        .open(M::Assign)
        .open(M::Append)
        .var("arr")
        .t(M::LBracket, "[")
        .t(M::RBracket, "]")
        .close();
    w.t(M::Eq, "=")
        .t(M::Num, "1")
        .close()
        .t(M::Semi, ";")
        .close();
    w.open(M::ExprStmt)
        .open(M::Assign)
        .open(M::Index)
        .var("arr")
        .t(M::LBracket, "[")
        .t(M::Num, "0");
    w.t(M::RBracket, "]")
        .close()
        .t(M::Eq, "=")
        .open(M::Member)
        .var("o")
        .t(M::Arrow, "->")
        .t(M::Ident, "prop");
    w.close().close().t(M::Semi, ";").close();
    w.close();
    let (out, _names, _) = lower(w);
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let hir = &out.hir;
    let es = exprs(hir);

    let closure = es.iter().find_map(|e| match e {
        Expr::Closure(c) => Some(*c),
        _ => None,
    });
    let c = closure.expect("a closure");
    assert_eq!(c.implicit, None);
    let caps = hir.list(c.captures);
    assert_eq!(caps.len(), 1);
    assert_eq!(caps[0].mode, CaptureMode::ByRef);
    assert_eq!(
        hir.binder(caps[0].binder).unwrap().kind,
        BinderKind::Capture
    );
    assert!(c.effects.contains(Effects::THROWS));

    assert!(es.iter().any(|e| matches!(e, Expr::MethodCall { .. })));
    assert!(es.iter().any(|e| matches!(e, Expr::DynMethodCall { .. })));
    assert!(es.iter().any(|e| matches!(e, Expr::VarVar(_))));
    assert!(es.iter().any(|e| matches!(e, Expr::Append(_))));
    assert!(es.iter().any(|e| matches!(e, Expr::Index { .. })));
    assert_eq!(
        es.iter()
            .filter(|e| matches!(e, Expr::Field { .. }))
            .count(),
        2
    );
}

#[test]
fn test_class_with_mixin_static_property_and_method() {
    // class Counter extends Base { use Loggable; public $count = 0;
    //   private static $instances = 0; function inc() { $n = 1; return $n; } }
    let mut w = W::new();
    w.open(M::File)
        .open(M::Class)
        .t(M::ClassKw, "class")
        .t(M::Ident, "Counter");
    w.t(M::ExtendsKw, "extends")
        .t(M::Ident, "Base")
        .t(M::LBrace, "{");
    w.open(M::MixinUse)
        .t(M::UseKw, "use")
        .t(M::Ident, "Loggable")
        .t(M::Semi, ";")
        .close();
    w.open(M::Prop)
        .t(M::Public, "public")
        .var("count")
        .t(M::Eq, "=")
        .t(M::Num, "0")
        .t(M::Semi, ";")
        .close();
    w.open(M::Prop)
        .t(M::Private, "private")
        .t(M::StaticKw, "static")
        .var("instances")
        .t(M::Eq, "=");
    w.t(M::Num, "0").t(M::Semi, ";").close();
    func(
        &mut w,
        "inc",
        |_| {},
        |w| {
            assign(w, "n", |w| {
                w.t(M::Num, "1");
            });
            w.open(M::Return)
                .t(M::ReturnKw, "return")
                .var("n")
                .t(M::Semi, ";")
                .close();
        },
    );
    w.t(M::RBrace, "}").close().close();
    let (out, mut names, _) = lower(w);
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let hir = &out.hir;
    let ItemKind::Module { items, .. } = hir.item(hir.root()).kind else {
        panic!()
    };
    let ItemKind::Class(class) = hir.item(hir.list(items)[0]).kind else {
        panic!("a class")
    };
    assert_eq!(hir.list(class.bases).len(), 1);
    assert_eq!(hir.list(class.fields).len(), 1);
    let kinds: Vec<&str> = hir
        .list(class.items)
        .iter()
        .map(|&i| hir.item(i).kind.name())
        .collect();
    assert_eq!(kinds, ["mixin-use", "global", "fn"]);
    let field = hir.field(hir.list(class.fields)[0]);
    assert_eq!(field.vis, hir_lang::Vis::Public);
    assert_eq!(field.name.map(|n| n.sym), Some(names.intern("count")));
}

#[test]
fn test_try_catch_finally_throw_and_generators() {
    // function gen() { yield 1; }
    // try { throw $e; } catch (Ex | Other $err) { echo $err; } finally { echo 1; }
    let mut w = W::new();
    w.open(M::File);
    func(
        &mut w,
        "gen",
        |_| {},
        |w| {
            w.open(M::ExprStmt)
                .open(M::Yield)
                .t(M::YieldKw, "yield")
                .t(M::Num, "1")
                .close()
                .t(M::Semi, ";")
                .close();
        },
    );
    w.open(M::ExprStmt).open(M::Try).t(M::TryKw, "try");
    block(&mut w, |w| {
        w.open(M::ExprStmt)
            .open(M::Throw)
            .t(M::ThrowKw, "throw")
            .var("e")
            .close()
            .t(M::Semi, ";")
            .close();
    });
    w.open(M::Catch)
        .t(M::CatchKw, "catch")
        .t(M::LParen, "(")
        .t(M::Ident, "Ex")
        .t(M::Pipe, "|");
    w.t(M::Ident, "Other").var("err").t(M::RParen, ")");
    block(&mut w, |w| {
        w.open(M::Echo)
            .t(M::EchoKw, "echo")
            .var("err")
            .t(M::Semi, ";")
            .close();
    });
    w.close();
    w.open(M::Finally).t(M::FinallyKw, "finally");
    block(&mut w, |w| {
        w.open(M::Echo)
            .t(M::EchoKw, "echo")
            .t(M::Num, "1")
            .t(M::Semi, ";")
            .close();
    });
    w.close().close().t(M::Semi, ";").close();
    w.close();
    let (out, _names, _) = lower(w);
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let hir = &out.hir;
    let ItemKind::Module { items, effects, .. } = hir.item(hir.root()).kind else {
        panic!()
    };
    assert!(effects.contains(Effects::THROWS));
    let ItemKind::Fn(def) = hir.item(hir.list(items)[0]).kind else {
        panic!()
    };
    assert!(
        def.effects.contains(Effects::YIELD),
        "a yield makes a generator"
    );

    let es = exprs(hir);
    let (catches, finally) = es
        .iter()
        .find_map(|e| match e {
            Expr::Try {
                catches, finally, ..
            } => Some((*catches, *finally)),
            _ => None,
        })
        .expect("a try");
    assert!(finally.is_some());
    let arm = hir.list(catches)[0];
    let Pat::Or(alts) = *hir.pat(arm.pat) else {
        panic!("two caught types")
    };
    let binders: Vec<_> = hir
        .list(alts)
        .iter()
        .map(|&a| match *hir.pat(a) {
            Pat::TypeTest { pat: Some(p), .. } => match *hir.pat(p) {
                Pat::Bind { binder, mode, .. } => {
                    assert_eq!(mode, BindMode::Value);
                    binder
                }
                _ => panic!("binds"),
            },
            _ => panic!("type test"),
        })
        .collect();
    assert_eq!(
        binders[0], binders[1],
        "or-alternatives bind the same binder"
    );
}

#[test]
fn test_map_literals_and_string_escapes() {
    // $m = [1, 'it\'s' => 2, "tab\there" => true, null];
    let mut w = W::new();
    w.open(M::File);
    assign(&mut w, "m", |w| {
        w.open(M::Array)
            .t(M::LBracket, "[")
            .t(M::Num, "1")
            .t(M::Comma, ",");
        w.open(M::Pair)
            .t(M::SqStr, "'it\\'s'")
            .t(M::FatArrow, "=>")
            .t(M::Num, "2")
            .close()
            .t(M::Comma, ",");
        w.open(M::Pair)
            .t(M::Str, "\"tab\\there\"")
            .t(M::FatArrow, "=>")
            .t(M::True, "true")
            .close();
        w.t(M::Comma, ",")
            .t(M::Null, "null")
            .t(M::RBracket, "]")
            .close();
    });
    w.close();
    let (out, _names, _) = lower(w);
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let hir = &out.hir;
    let entries = exprs(hir)
        .into_iter()
        .find_map(|e| match e {
            Expr::Map(m) => Some(hir.list(m).to_vec()),
            _ => None,
        })
        .expect("a map");
    assert_eq!(entries.len(), 4);
    assert!(entries[0].key.is_none() && entries[3].key.is_none());
    let key = |i: usize| match *hir.expr(entries[i].key.unwrap()) {
        Expr::Lit(Lit::Str(t)) => hir.str(t).to_owned(),
        _ => String::new(),
    };
    assert_eq!(key(1), "it's");
    assert_eq!(key(2), "tab\there");
}

#[test]
fn test_mox_namespaces_and_path_shapes() {
    // The value paths lowering builds are all in the value namespace and
    // unresolved (or extern), except template temporaries.
    let mut w = W::new();
    w.open(M::File);
    assign(&mut w, "a", |w| {
        w.open(M::Binary)
            .var("b")
            .t(M::Plus, "+")
            .t(M::Num, "1")
            .close();
    });
    w.close();
    let (out, _, _) = lower(w);
    for i in 0..out.hir.count(IdKind::Path) {
        let p = out.hir.path(hir_lang::PathId::from_index(i).unwrap());
        assert_eq!(p.ns, Ns::Value);
        assert!(p.res.is_unresolved());
    }
}

// ------------------------------------------------------------- 0.3: PHP

/// `"..."` with the given pieces: `Ok(text)` a `StrPart`, `Err(var)` a
/// `$var`, braced when `braced`.
fn dq(w: &mut W, pieces: &[Result<&str, &str>], braced: bool) {
    w.open(M::DqString).t(M::DQuote, "\"");
    for p in pieces {
        match p {
            Ok(text) => {
                w.t(M::StrPart, text);
            }
            Err(var) => {
                if braced {
                    w.t(M::CurlyOpen, "{");
                }
                w.var(var);
                if braced {
                    w.t(M::CurlyClose, "}");
                }
            }
        }
    }
    w.t(M::DQuote, "\"").close();
}

/// Lowers and prints.
fn printed(w: W) -> (Lowered, String) {
    let (out, names, _) = lower(w);
    let p = hir_lang::print(&out.hir, &names);
    (out, p)
}

/// The text of every string literal, in walk order.
fn strings(hir: &Hir) -> Vec<String> {
    exprs(hir)
        .into_iter()
        .filter_map(|e| match e {
            Expr::Lit(Lit::Str(t)) => Some(hir.str(t).to_owned()),
            _ => None,
        })
        .collect()
}

#[test]
fn test_interpolated_strings_join_with_concat() {
    // $s = "Hello {$name}!";  $t = "$name";  $u = "{$a['k']}";  $v = "plain\n";  $w = "";
    let mut w = W::new();
    w.open(M::File);
    assign(&mut w, "s", |w| {
        dq(w, &[Ok("Hello "), Err("name"), Ok("!")], true);
    });
    assign(&mut w, "t", |w| dq(w, &[Err("name")], false));
    assign(&mut w, "u", |w| {
        w.open(M::DqString)
            .t(M::DQuote, "\"")
            .t(M::CurlyOpen, "{")
            .open(M::Index)
            .var("a")
            .t(M::LBracket, "[")
            .t(M::SqStr, "'k'")
            .t(M::RBracket, "]")
            .close()
            .t(M::CurlyClose, "}")
            .t(M::DQuote, "\"")
            .close();
    });
    assign(&mut w, "v", |w| dq(w, &[Ok("pla"), Ok("in\\n")], false));
    assign(&mut w, "w", |w| dq(w, &[], false));
    w.close();
    let (out, p) = printed(w);
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    // "Hello {$name}!": concat(concat("Hello ", name), "!").
    let hello = [
        "(call",
        "          (use (path mox.concat → extern mox.concat))",
        "          (call",
        "            (use (path mox.concat → extern mox.concat))",
        "            (lit \"Hello \")",
        "            (use (path name)))",
        "          (lit \"!\"))",
    ]
    .join("\n");
    assert!(p.contains(&hello), "{p}");
    // "$name": a conversion, concat("", name).
    assert!(p.contains("(lit \"\")\n          (use (path name))"), "{p}");
    // "{$a['k']}": the embedded index expression.
    assert!(
        p.contains("(index\n            (use (path a))\n            (lit \"k\"))"),
        "{p}"
    );
    // Adjacent text pieces merge, escapes decoded; "" is the empty string.
    let texts = strings(&out.hir);
    assert!(texts.contains(&"plain\n".to_owned()), "{texts:?}");
    assert_eq!(
        texts.iter().filter(|t| t.is_empty()).count(),
        3,
        "{texts:?}"
    );
}

#[test]
fn test_heredoc_indentation_is_stripped_per_php_7_3() {
    // $h = <<<EOT
    //     Hi {$x}
    //       indented
    //     EOT;
    let mut w = W::new();
    w.open(M::File);
    assign(&mut w, "h", |w| {
        w.open(M::HeredocStr)
            .t(M::HeredocOpen, "<<<EOT\n")
            .t(M::StrPart, "    Hi ")
            .t(M::CurlyOpen, "{")
            .var("x")
            .t(M::CurlyClose, "}")
            .t(M::StrPart, "\n      indented\n")
            .t(M::HeredocClose, "    EOT")
            .close();
    });
    // A body line indented less than the closing marker is reported.
    assign(&mut w, "bad", |w| {
        w.open(M::HeredocStr)
            .t(M::HeredocOpen, "<<<EOT\n")
            .t(M::StrPart, "  short\n")
            .t(M::HeredocClose, "    EOT")
            .close();
    });
    // So is a line that starts with an expression (no indentation at all).
    assign(&mut w, "bad2", |w| {
        w.open(M::HeredocStr)
            .t(M::HeredocOpen, "<<<EOT\n")
            .t(M::StrPart, "    ok\n")
            .t(M::CurlyOpen, "{")
            .var("x")
            .t(M::CurlyClose, "}")
            .t(M::StrPart, "\n")
            .t(M::HeredocClose, "    EOT")
            .close();
    });
    // A nowdoc token: no escapes, indentation stripped (blank lines kept),
    // the last newline dropped.
    assign(&mut w, "n", |w| {
        w.t(M::Nowdoc, "<<<'EOT'\n  raw \\n {$x}\n\n   two\n  EOT");
    });
    w.close();
    let (out, p) = printed(w);
    let texts = strings(&out.hir);
    assert!(texts.contains(&"Hi ".to_owned()), "{texts:?}\n{p}");
    assert!(texts.contains(&"\n  indented".to_owned()), "{texts:?}");
    assert!(
        texts.contains(&"raw \\n {$x}\n\n two".to_owned()),
        "{texts:?}"
    );
    // The empty piece after `{$x}` (only the dropped final newline) is not
    // joined.
    assert!(!texts.contains(&String::new()), "{texts:?}\n{p}");
    assert_eq!(
        out.problems
            .iter()
            .filter(|p| p.kind == ProblemKind::InvalidLiteral)
            .count(),
        2,
        "{:?}",
        out.problems
    );
}

#[test]
fn test_foreach_and_catch_variables_are_function_scoped() {
    // function f($items) {
    //   foreach ($items as $k => $v) { }
    //   try { } catch (Ex $e) { }
    //   echo $k, $v, $e;
    // }
    let mut w = W::new();
    w.open(M::File);
    func(
        &mut w,
        "f",
        |w| {
            w.open(M::Param).var("items").close();
        },
        |w| {
            w.open(M::Foreach)
                .t(M::ForeachKw, "foreach")
                .t(M::LParen, "(")
                .var("items")
                .t(M::As, "as");
            w.var("k").t(M::FatArrow, "=>").var("v").t(M::RParen, ")");
            block(w, |_| {});
            w.close();
            w.open(M::ExprStmt).open(M::Try).t(M::TryKw, "try");
            block(w, |_| {});
            w.open(M::Catch)
                .t(M::CatchKw, "catch")
                .t(M::LParen, "(")
                .t(M::Ident, "Ex")
                .var("e")
                .t(M::RParen, ")");
            block(w, |_| {});
            w.close().close().t(M::Semi, ";").close();
            w.open(M::Echo)
                .t(M::EchoKw, "echo")
                .var("k")
                .t(M::Comma, ",")
                .var("v")
                .t(M::Comma, ",")
                .var("e")
                .t(M::Semi, ";")
                .close();
        },
    );
    w.close();
    let (out, names, _) = lower(w);
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let mut hir = out.hir;
    // The function body starts with the hoisted `let k; let v; let e;`.
    let ItemKind::Module { items, .. } = hir.item(hir.root()).kind else {
        panic!()
    };
    let ItemKind::Fn(def) = hir.item(hir.list(items)[0]).kind else {
        panic!()
    };
    let Expr::Block(body) = *hir.expr(def.body.unwrap()) else {
        panic!("wrapped in the hoisted declarations")
    };
    let hoisted: Vec<String> = hir
        .list(body.stmts)
        .iter()
        .filter_map(|s| match *hir.stmt(*s) {
            Stmt::Let { pat, .. } => match *hir.pat(pat) {
                Pat::Bind { binder, .. } => names
                    .resolve(hir.binder(binder).unwrap().name.sym)
                    .map(str::to_owned),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(hoisted, ["k", "v", "e"]);
    // After the loop and the handler, `$k`, `$v`, `$e` resolve to those
    // function-scoped binders (the pattern temporaries are marked, so no
    // name reaches them).
    let (resolved, open) = common::resolve_locals(&mut hir);
    assert_eq!(open, 0, "every local resolves");
    assert!(resolved >= 7, "{resolved}");
    let p = hir_lang::print(&hir, &names);
    assert!(p.contains("(path k → local k%"), "{p}");
    for i in 0..hir.count(IdKind::Path) {
        let path = *hir.path(hir_lang::PathId::from_index(i).unwrap());
        if let Res::Local(b) = path.res {
            assert_eq!(
                hir.binder(b).unwrap().name.mark,
                hir.list(path.segments)[0].name.mark,
                "hygiene"
            );
        }
    }
    // The echo after the loop reads the hoisted binders.
    let hoisted_binders: Vec<hir_lang::BinderId> = hir
        .list(body.stmts)
        .iter()
        .filter_map(|s| match *hir.stmt(*s) {
            Stmt::Let { pat, .. } => match *hir.pat(pat) {
                Pat::Bind { binder, .. } => Some(binder),
                _ => None,
            },
            _ => None,
        })
        .collect();
    let reads = (0..hir.count(IdKind::Path))
        .filter(|&i| {
            matches!(
                hir.path(hir_lang::PathId::from_index(i).unwrap()).res,
                Res::Local(b) if hoisted_binders.contains(&b)
            )
        })
        .count();
    // Three assignments (k, v, e) plus three reads in the echo.
    assert_eq!(reads, 6, "{p}");
}

#[test]
fn test_trait_insteadof_and_as_rules() {
    // class C { use A, B { A::hi insteadof B; B::hi as protected greet; hi as private; ho; } }
    let mut w = W::new();
    w.open(M::File)
        .open(M::Class)
        .t(M::ClassKw, "class")
        .t(M::Ident, "C")
        .t(M::LBrace, "{");
    w.open(M::MixinUse)
        .t(M::UseKw, "use")
        .t(M::Ident, "A")
        .t(M::Comma, ",")
        .t(M::Ident, "B")
        .t(M::LBrace, "{");
    w.open(M::TraitRule)
        .t(M::Ident, "A")
        .t(M::ColonColon, "::")
        .t(M::Ident, "hi")
        .t(M::InsteadofKw, "insteadof")
        .t(M::Ident, "B")
        .t(M::Semi, ";")
        .close();
    w.open(M::TraitRule)
        .t(M::Ident, "B")
        .t(M::ColonColon, "::")
        .t(M::Ident, "hi")
        .t(M::As, "as")
        .t(M::Protected, "protected")
        .t(M::Ident, "greet")
        .t(M::Semi, ";")
        .close();
    w.open(M::TraitRule)
        .t(M::Ident, "hi")
        .t(M::As, "as")
        .t(M::Private, "private")
        .t(M::Semi, ";")
        .close();
    w.open(M::TraitRule)
        .t(M::Ident, "ho")
        .t(M::Semi, ";")
        .close();
    w.t(M::RBrace, "}").close();
    w.t(M::RBrace, "}").close().close();
    let (out, mut names, _) = lower(w);
    let missing: Vec<_> = out
        .problems
        .iter()
        .filter(|p| p.kind == ProblemKind::MissingField { field: "action" })
        .collect();
    assert_eq!(missing.len(), 1, "`ho;` has no action: {:?}", out.problems);
    let hir = &out.hir;
    let use_item = (0..hir.count(IdKind::Item))
        .map(|i| hir.item(hir_lang::ItemId::from_index(i).unwrap()))
        .find_map(|it| match it.kind {
            ItemKind::MixinUse(def) => Some(def),
            _ => None,
        })
        .expect("a mixin use");
    assert_eq!(hir.list(use_item.mixins).len(), 2);
    let rules = hir.list(use_item.rules);
    assert_eq!(rules.len(), 3);
    let hi = names.intern("hi");
    assert!(rules.iter().all(|r| r.method.sym == hi));
    assert!(matches!(rules[0].action, MixinAction::Insteadof(l) if l.len() == 1));
    assert!(rules[0].from.is_some());
    let greet = names.intern("greet");
    assert!(matches!(
        rules[1].action,
        MixinAction::Alias { name: Some(n), vis: Some(hir_lang::Vis::Protected) } if n.sym == greet
    ));
    assert!(matches!(
        rules[2].action,
        MixinAction::Alias {
            name: None,
            vis: Some(hir_lang::Vis::Private)
        }
    ));
    assert!(rules[2].from.is_none());
}

/// An `NsName` path node of `\`-separated names.
fn ns_name(w: &mut W, parts: &[&str]) {
    w.open(M::NsName);
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            w.t(M::NsSep, "\\");
        }
        w.t(M::Ident, p);
    }
    w.close();
}

#[test]
fn test_group_use_flattens_to_one_import_per_leaf() {
    // use App\Models\{User, Post as P, Sub\{X, Y}};   use Lone;
    let mut w = W::new();
    w.open(M::File);
    w.open(M::UseStmt).t(M::UseKw, "use").open(M::UseDecl);
    ns_name(&mut w, &["App", "Models"]);
    w.t(M::NsSep, "\\").open(M::UseGroup).t(M::UseOpen, "{");
    w.open(M::UseDecl)
        .t(M::Ident, "User")
        .close()
        .t(M::Comma, ",");
    w.open(M::UseDecl)
        .t(M::Ident, "Post")
        .t(M::As, "as")
        .t(M::Ident, "P")
        .close()
        .t(M::Comma, ",");
    w.open(M::UseDecl)
        .t(M::Ident, "Sub")
        .t(M::NsSep, "\\")
        .open(M::UseGroup)
        .t(M::UseOpen, "{");
    w.open(M::UseDecl).t(M::Ident, "X").close().t(M::Comma, ",");
    w.open(M::UseDecl).t(M::Ident, "Y").close();
    w.t(M::UseClose, "}").close().close();
    w.t(M::UseClose, "}")
        .close()
        .close()
        .t(M::Semi, ";")
        .close();
    w.open(M::UseStmt).t(M::UseKw, "use").open(M::UseDecl);
    w.t(M::Ident, "Lone").close().t(M::Semi, ";").close();
    w.close();
    let (out, names, _) = lower(w);
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let hir = &out.hir;
    let ItemKind::Module { items, .. } = hir.item(hir.root()).kind else {
        panic!()
    };
    let imports: Vec<(String, Option<String>)> = hir
        .list(items)
        .iter()
        .filter_map(|&i| {
            let it = hir.item(i);
            let ItemKind::Import { path, glob } = it.kind else {
                return None;
            };
            assert!(!glob);
            let p = hir.path(path);
            assert_eq!(p.ns, Ns::Import);
            let segs: Vec<&str> = hir
                .list(p.segments)
                .iter()
                .map(|s| names.resolve(s.name.sym).unwrap())
                .collect();
            Some((
                segs.join("::"),
                it.name.map(|n| names.resolve(n.sym).unwrap().to_owned()),
            ))
        })
        .collect();
    assert_eq!(
        imports,
        [
            ("App::Models::User".to_owned(), None),
            ("App::Models::Post".to_owned(), Some("P".to_owned())),
            ("App::Models::Sub::X".to_owned(), None),
            ("App::Models::Sub::Y".to_owned(), None),
            ("Lone".to_owned(), None),
        ]
    );
}

#[test]
fn test_attributes_lower_to_hir_attributes() {
    // #[Route("/x", methods: GET), Pure] function f(#[Sensitive] $p) {}
    // #[Bad([1])] function g() {}
    let mut w = W::new();
    w.open(M::File);
    w.open(M::FnDecl);
    w.open(M::AttrGroup).t(M::HashBracket, "#[");
    w.open(M::Attr)
        .t(M::Ident, "Route")
        .t(M::LParen, "(")
        .t(M::Str, "\"/x\"")
        .t(M::Comma, ",")
        .open(M::AttrArg)
        .t(M::Ident, "methods")
        .t(M::Colon, ":")
        .t(M::Ident, "GET")
        .close()
        .t(M::RParen, ")")
        .close()
        .t(M::Comma, ",");
    w.open(M::Attr).t(M::Ident, "Pure").close();
    w.t(M::RBracket, "]").close();
    w.t(M::Function, "function")
        .t(M::Ident, "f")
        .t(M::LParen, "(");
    w.open(M::Params).open(M::Param);
    w.open(M::AttrGroup).t(M::HashBracket, "#[");
    w.open(M::Attr).t(M::Ident, "Sensitive").close();
    w.t(M::RBracket, "]").close();
    w.var("p").close().close();
    w.t(M::RParen, ")");
    block(&mut w, |_| {});
    w.close();
    w.open(M::FnDecl);
    w.open(M::AttrGroup).t(M::HashBracket, "#[");
    w.open(M::Attr)
        .t(M::Ident, "Bad")
        .t(M::LParen, "(")
        .open(M::Array)
        .t(M::LBracket, "[")
        .t(M::Num, "1")
        .t(M::RBracket, "]")
        .close()
        .t(M::RParen, ")")
        .close();
    w.t(M::RBracket, "]").close();
    w.t(M::Function, "function")
        .t(M::Ident, "g")
        .t(M::LParen, "(");
    w.open(M::Params).close();
    w.t(M::RParen, ")");
    block(&mut w, |_| {});
    w.close();
    w.close();
    let (out, mut names, _) = lower(w);
    assert_eq!(
        out.problems.iter().map(|p| p.kind).collect::<Vec<_>>(),
        [ProblemKind::AttributeArgument],
        "{:?}",
        out.problems
    );
    let hir = &out.hir;
    let ItemKind::Module { items, .. } = hir.item(hir.root()).kind else {
        panic!()
    };
    let f = hir.list(items)[0];
    let attrs = hir.attrs(NodeRef::Item(f));
    assert_eq!(attrs.len(), 2);
    assert_eq!(attrs[0].name.sym, names.intern("Route"));
    let args = hir.list(attrs[0].args);
    assert_eq!(args.len(), 2);
    assert!(matches!(args[0].value, Some(AttrValue::Lit(Lit::Str(t))) if hir.str(t) == "/x"));
    assert_eq!(args[1].key.map(|k| k.sym), Some(names.intern("methods")));
    let get = names.intern("GET");
    assert!(matches!(args[1].value, Some(AttrValue::Name(n)) if n.sym == get));
    assert_eq!(attrs[1].name.sym, names.intern("Pure"));
    // The parameter's attribute.
    let ItemKind::Fn(def) = hir.item(f).kind else {
        panic!()
    };
    let param = hir.list(def.params)[0];
    let pattrs = hir.attrs(NodeRef::Param(param));
    assert_eq!(pattrs.len(), 1);
    assert_eq!(pattrs[0].name.sym, names.intern("Sensitive"));
    // `Bad([1])` keeps its name and drops the array argument.
    let g = hir.list(items)[1];
    let gattrs = hir.attrs(NodeRef::Item(g));
    assert_eq!(gattrs.len(), 1);
    assert!(hir.list(gattrs[0].args).is_empty());
}

#[test]
fn test_php_number_literals() {
    // PHP spelling: 1_000, 0777 (octal), 9223372036854775808 (float),
    // 0xFFFFFFFFFFFFFFFF (float), PHP_INT_MAX (int); invalid: 1__0, 08,
    // 0x1.8p3 (PHP has no hex floats).
    let mut w = W::new();
    w.open(M::File);
    for (i, text) in [
        "1_000",
        "0777",
        "9223372036854775808",
        "0xFFFFFFFFFFFFFFFF",
        "9223372036854775807",
        "1__0",
        "08",
        "0x1.8p3",
    ]
    .iter()
    .enumerate()
    {
        assign(&mut w, &format!("n{i}"), |w| {
            w.t(M::Num, text);
        });
    }
    w.close();
    let (out, _, _) = lower(w);
    let lits: Vec<Lit> = exprs(&out.hir)
        .into_iter()
        .filter_map(|e| match e {
            Expr::Lit(l) => Some(l),
            _ => None,
        })
        .collect();
    let int = |l: &Lit| match l {
        Lit::Int(i) => Some(i.value),
        _ => None,
    };
    let float = |l: &Lit| match l {
        Lit::Float(f) => Some(f.value()),
        _ => None,
    };
    assert_eq!(int(&lits[0]), Some(1000));
    assert_eq!(int(&lits[1]), Some(511));
    assert_eq!(float(&lits[2]), Some(9_223_372_036_854_775_808.0));
    assert_eq!(float(&lits[3]), Some(18_446_744_073_709_551_616.0));
    assert_eq!(int(&lits[4]), Some(i64::MAX as u64));
    let bad = out
        .problems
        .iter()
        .filter(|p| p.kind == ProblemKind::InvalidLiteral)
        .count();
    assert_eq!(bad, 3, "{:?}", out.problems);
}
