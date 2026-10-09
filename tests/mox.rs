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
    ArgKind, BindMode, BinderKind, CaptureMode, Effects, ExpnKind, Expr, Hir, IdKind, ItemKind,
    Lit, NodeRef, Ns, ParamKind, Pat, Res, Stmt,
};
use intern_lang::Interner;
use lower_lang::Lowered;
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
    // separate instantiation in user code, so its parent is the root).
    let expns: Vec<_> = (1..=hir.count(IdKind::Expansion) as u32)
        .filter_map(|i| hir.expansion(hir_lang::ExpnId::from_u32(i)).copied())
        .collect();
    assert_eq!(expns.len(), 2);
    assert!(expns.iter().all(|e| e.parent.is_root()));
    let mut kinds: Vec<ExpnKind> = expns.iter().map(|e| e.kind).collect();
    kinds.sort_by_key(|k| *k as u8);
    assert_eq!(kinds, [ExpnKind::Template, ExpnKind::Desugar]);

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
