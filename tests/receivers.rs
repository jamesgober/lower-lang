//! The implicit receiver (`LangPolicy::implicit_receiver`, LSF2
//! `implicit_receiver = "this"`): methods of classes, mixins, and
//! interfaces bind `$this` as their HIR receiver parameter, closures capture
//! it as PHP does, and the result runs end to end through resolve-lang 0.4,
//! bcgen-lang 0.4, and bvm-lang 2.0.0-alpha.4.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

#[path = "support/mox_oop.rs"]
mod oop;

use hir_lang::{BinderKind, CaptureMode, Expr, Hir, ItemKind, NodeRef, ParamKind, Pat, Res};
use intern_lang::Interner;
use lower_lang::{LangPolicy, MappingBuilder, MappingError, ProblemKind};
use oop::{W, X, lines};

// ------------------------------------------------------------ program parts

/// `class A { public $x; function __construct($x) { $this->x = $x; }
/// function get() { return $this->x; } }`
fn class_a(w: &mut W) {
    w.class("A", None, |w| {
        w.property("x", None);
        w.function(X::Method, &[X::PublicKw], "__construct", &["x"], |w| {
            w.stmt(|w| {
                w.assign(
                    |w| {
                        w.prop("this", "x");
                    },
                    |w| {
                        w.var("x");
                    },
                );
            });
        });
        w.function(X::Method, &[X::PublicKw], "get", &[], |w| {
            w.ret(|w| {
                w.prop("this", "x");
            });
        });
    });
}

/// `class B extends A { public $y; function __construct($x, $y) {
/// parent::__construct($x); $this->y = $y; } function sum() { return
/// $this->get() + $this->y; } }`
fn class_b(w: &mut W) {
    w.class("B", Some("A"), |w| {
        w.property("y", None);
        w.function(X::Method, &[X::PublicKw], "__construct", &["x", "y"], |w| {
            w.stmt(|w| {
                w.call(
                    |w| {
                        w.open(X::StaticMember)
                            .t(X::ParentKw, "parent")
                            .t(X::ColonColon, "::")
                            .t(X::Ident, "__construct")
                            .close();
                    },
                    1,
                    &[],
                    |w, _| {
                        w.var("x");
                    },
                );
            });
            w.stmt(|w| {
                w.assign(
                    |w| {
                        w.prop("this", "y");
                    },
                    |w| {
                        w.var("y");
                    },
                );
            });
        });
        w.function(X::Method, &[X::PublicKw], "sum", &[], |w| {
            w.ret(|w| {
                w.bin(
                    X::Plus,
                    |w| {
                        w.mcall(
                            |w| {
                                w.var("this");
                            },
                            "get",
                            0,
                            |_, _| {},
                        );
                    },
                    |w| {
                        w.prop("this", "y");
                    },
                );
            });
        });
    });
}

/// `$name = new Class(a, b);`
fn new_into(w: &mut W, name: &str, class: &str, args: &[i64]) {
    // `$name = new Class(args);` through the sketch's `new_object` template
    // (`extern:mox.new(${class}, ${...args})`): resolve-lang 0.4.1 resolves
    // the class name in value position under `Policy::php()`.
    w.stmt(|w| {
        w.assign(
            |w| {
                w.var(name);
            },
            |w| {
                w.new_obj(class, args.len(), |w, i| {
                    w.num(args[i]);
                });
            },
        );
    });
}

/// The parameters of every function item named `name`, as (kind, binder
/// name) pairs.
fn params_of(hir: &Hir, names: &Interner, name: &str) -> Vec<Vec<(ParamKind, String)>> {
    let mut out = Vec::new();
    hir_lang::walk(hir, |n| {
        let NodeRef::Item(i) = n else { return };
        let item = hir.item(i);
        let ItemKind::Fn(def) = item.kind else { return };
        if item.name.and_then(|nm| names.resolve(nm.sym)) != Some(name) {
            return;
        }
        let ps = hir
            .list(def.params)
            .iter()
            .map(|&p| {
                let p = hir.param(p);
                let bname = match hir.pat(p.pat) {
                    Pat::Bind { binder, .. } => hir
                        .binder(*binder)
                        .and_then(|b| names.resolve(b.name.sym))
                        .unwrap_or("?")
                        .to_string(),
                    _ => "_".into(),
                };
                (p.kind, bname)
            })
            .collect();
        out.push(ps);
    });
    out
}

fn recv(name: &str) -> (ParamKind, String) {
    (ParamKind::Receiver, name.to_string())
}

fn normal(name: &str) -> (ParamKind, String) {
    (ParamKind::Normal, name.to_string())
}

// --------------------------------------------------------------- end to end

#[test]
fn test_constructor_method_and_parent_construct_run_end_to_end() {
    let r = oop::run(|w| {
        class_a(w);
        class_b(w);
        new_into(w, "b", "B", &[2, 3]);
        w.echo(|w| {
            w.mcall(
                |w| {
                    w.var("b");
                },
                "sum",
                0,
                |_, _| {},
            );
        });
        w.echo(|w| {
            w.prop("b", "x");
        });
        new_into(w, "a", "A", &[40]);
        w.echo(|w| {
            w.mcall(
                |w| {
                    w.var("a");
                },
                "get",
                0,
                |_, _| {},
            );
        });
    });
    assert_eq!(r, lines(&["5", "2", "40"]));
}

#[test]
fn test_closures_capture_this_end_to_end() {
    // class C { public $v = 5;
    //   function adder() { return function ($n) { return $this->v + $n; }; }
    //   function arrow() { return fn ($n) => $this->v + $n; }
    //   function nested() { return function () { return function () { return $this->v; }; }; } }
    // $c = new C(); echo ($c->adder())(10); echo ($c->arrow())(20); echo (($c->nested())())();
    let r = oop::run(|w| {
        w.class("C", None, |w| {
            w.property("v", Some(5));
            w.function(X::Method, &[X::PublicKw], "adder", &[], |w| {
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
            w.function(X::Method, &[X::PublicKw], "arrow", &[], |w| {
                w.ret(|w| {
                    w.arrow(false, &["n"], |w| {
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
            w.function(X::Method, &[X::PublicKw], "nested", &[], |w| {
                w.ret(|w| {
                    w.closure(false, &[], &[], |w| {
                        w.ret(|w| {
                            w.closure(false, &[], &[], |w| {
                                w.ret(|w| {
                                    w.prop("this", "v");
                                });
                            });
                        });
                    });
                });
            });
        });
        new_into(w, "c", "C", &[]);
        for (method, arg) in [("adder", Some(10)), ("arrow", Some(20))] {
            w.echo(|w| {
                w.call(
                    |w| {
                        w.mcall(
                            |w| {
                                w.var("c");
                            },
                            method,
                            0,
                            |_, _| {},
                        );
                    },
                    1,
                    &[],
                    |w, _| {
                        w.num(arg.unwrap_or(0));
                    },
                );
            });
        }
        w.echo(|w| {
            w.call(
                |w| {
                    w.call(
                        |w| {
                            w.mcall(
                                |w| {
                                    w.var("c");
                                },
                                "nested",
                                0,
                                |_, _| {},
                            );
                        },
                        0,
                        &[],
                        |_, _| {},
                    );
                },
                0,
                &[],
                |_, _| {},
            );
        });
    });
    assert_eq!(r, lines(&["15", "25", "5"]));
}

#[test]
fn test_static_methods_have_no_receiver_and_run() {
    // class K { static function twice($n) { return $n + $n; }
    //           function viaStatic() { return self::twice(4); } }
    // echo K::twice(3); $k = new K(); echo $k->viaStatic();
    let static_call = |w: &mut W, root: (X, &str), arg: i64| {
        w.call(
            |w| {
                w.open(X::StaticMember)
                    .t(root.0, root.1)
                    .t(X::ColonColon, "::")
                    .t(X::Ident, "twice")
                    .close();
            },
            1,
            &[],
            |w, _| {
                w.num(arg);
            },
        );
    };
    let r = oop::run(|w| {
        w.class("K", None, |w| {
            w.function(
                X::Method,
                &[X::PublicKw, X::StaticKw],
                "twice",
                &["n"],
                |w| {
                    w.ret(|w| {
                        w.bin(
                            X::Plus,
                            |w| {
                                w.var("n");
                            },
                            |w| {
                                w.var("n");
                            },
                        );
                    });
                },
            );
            w.function(X::Method, &[X::PublicKw], "viaStatic", &[], |w| {
                w.ret(|w| static_call(w, (X::SelfKw, "self"), 4));
            });
        });
        w.echo(|w| static_call(w, (X::Ident, "K"), 3));
        new_into(w, "k", "K", &[]);
        w.echo(|w| {
            w.mcall(
                |w| {
                    w.var("k");
                },
                "viaStatic",
                0,
                |_, _| {},
            );
        });
    });
    assert_eq!(r, lines(&["6", "8"]));
}

#[test]
fn test_mixin_methods_bind_the_using_object() {
    // trait Greets { function hi() { return $this->n; } }
    // class P { use Greets; public $n = 7; }  echo (new P())->hi();
    let r = oop::run(|w| {
        w.open(X::TraitDecl)
            .t(X::TraitKw, "trait")
            .t(X::Ident, "Greets")
            .t(X::LBrace, "{");
        w.function(X::Method, &[X::PublicKw], "hi", &[], |w| {
            w.ret(|w| {
                w.prop("this", "n");
            });
        });
        w.t(X::RBrace, "}").close();
        w.class("P", None, |w| {
            w.open(X::TraitUse)
                .t(X::UseKw, "use")
                .t(X::Ident, "Greets")
                .t(X::Semi, ";")
                .close();
            w.property("n", Some(7));
        });
        new_into(w, "p", "P", &[]);
        w.echo(|w| {
            w.mcall(
                |w| {
                    w.var("p");
                },
                "hi",
                0,
                |_, _| {},
            );
        });
    });
    assert_eq!(r, lines(&["7"]));
}

// ------------------------------------------------------------------ shapes

#[test]
fn test_methods_get_a_receiver_first_and_statics_none() {
    let m = oop::mapping();
    let (out, names, _) = oop::lower(&m, |w| {
        class_a(w);
        w.class("S", None, |w| {
            w.function(X::Method, &[X::StaticKw], "make", &["v"], |_| {});
            w.function(X::Method, &[X::PublicKw], "show", &["v"], |_| {});
        });
        w.function(X::FnDecl, &[], "free", &["v"], |_| {});
    });
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let hir = &out.hir;
    assert_eq!(
        params_of(hir, &names, "__construct"),
        [vec![recv("this"), normal("x")]]
    );
    assert_eq!(params_of(hir, &names, "get"), [vec![recv("this")]]);
    assert_eq!(params_of(hir, &names, "make"), [vec![normal("v")]]);
    assert_eq!(
        params_of(hir, &names, "show"),
        [vec![recv("this"), normal("v")]]
    );
    assert_eq!(params_of(hir, &names, "free"), [vec![normal("v")]]);
    // The receiver's binder is a parameter binder with an unmarked name,
    // and its origin is the method's name.
    let mut seen = 0;
    hir_lang::walk(hir, |n| {
        if let NodeRef::Param(p) = n {
            let p = hir.param(p);
            if p.kind == ParamKind::Receiver {
                let Pat::Bind { binder, .. } = hir.pat(p.pat) else {
                    panic!("binds")
                };
                let b = hir.binder(*binder).expect("binder");
                assert_eq!(b.kind, BinderKind::Param);
                assert!(b.name.mark.is_root());
                assert!(!hir.binder_origin(*binder).span.is_empty());
                seen += 1;
            }
        }
    });
    assert_eq!(seen, 3);
}

#[test]
fn test_interfaces_and_mixins_get_receivers_impls_do_not() {
    let m = oop::mapping();
    let (out, names, _) = oop::lower(&m, |w| {
        w.open(X::InterfaceDecl)
            .t(X::InterfaceKw, "interface")
            .t(X::Ident, "Shape")
            .t(X::LBrace, "{");
        w.open(X::Method)
            .t(X::FunctionKw, "function")
            .t(X::Ident, "area");
        w.params(&[]);
        w.t(X::Semi, ";").close();
        w.open(X::Method)
            .t(X::StaticKw, "static")
            .t(X::FunctionKw, "function")
            .t(X::Ident, "unit");
        w.params(&[]);
        w.t(X::Semi, ";").close();
        w.t(X::RBrace, "}").close();
        w.open(X::TraitDecl)
            .t(X::TraitKw, "trait")
            .t(X::Ident, "T")
            .t(X::LBrace, "{");
        w.function(X::Method, &[X::PublicKw], "mixed", &[], |_| {});
        w.t(X::RBrace, "}").close();
        w.open(X::ImplBlock)
            .t(X::ImplKw, "impl")
            .t(X::Ident, "Shape")
            .t(X::LBrace, "{");
        w.function(X::Method, &[X::PublicKw], "implemented", &["v"], |_| {});
        w.t(X::RBrace, "}").close();
    });
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let hir = &out.hir;
    assert_eq!(params_of(hir, &names, "area"), [vec![recv("this")]]);
    assert_eq!(
        params_of(hir, &names, "unit"),
        [Vec::<(ParamKind, String)>::new()]
    );
    assert_eq!(params_of(hir, &names, "mixed"), [vec![recv("this")]]);
    assert_eq!(params_of(hir, &names, "implemented"), [vec![normal("v")]]);
}

#[test]
fn test_an_explicit_receiver_wins() {
    let m = oop::mapping();
    let (out, names, _) = oop::lower(&m, |w| {
        w.class("E", None, |w| {
            w.open(X::Method)
                .t(X::FunctionKw, "function")
                .t(X::Ident, "own");
            w.t(X::LParen, "(").open(X::ParamList);
            w.open(X::TypedParam).t(X::SelfParam, "me").close();
            w.close().t(X::RParen, ")");
            w.block(|_| {});
            w.close();
        });
    });
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    assert_eq!(params_of(&out.hir, &names, "own"), [vec![recv("me")]]);
}

#[test]
fn test_without_the_policy_methods_are_unchanged() {
    let m = oop::mapping_with(false, true);
    let (out, names, _) = oop::lower(&m, class_a);
    assert_eq!(
        params_of(&out.hir, &names, "__construct"),
        [vec![normal("x")]]
    );
    assert_eq!(
        params_of(&out.hir, &names, "get"),
        [Vec::<(ParamKind, String)>::new()]
    );
}

/// Captures of each closure, in source order: (binder name, mode).
fn closure_captures(hir: &Hir, names: &Interner) -> Vec<Vec<(String, CaptureMode)>> {
    let mut out = Vec::new();
    hir_lang::walk(hir, |n| {
        if let NodeRef::Expr(e) = n {
            if let Expr::Closure(c) = hir.expr(e) {
                let caps = hir
                    .list(c.captures)
                    .iter()
                    .map(|cap| {
                        let name = hir
                            .binder(cap.binder)
                            .and_then(|b| names.resolve(b.name.sym))
                            .unwrap_or("?")
                            .to_string();
                        (name, cap.mode)
                    })
                    .collect();
                out.push((hir.origin(n).span.start(), caps));
            }
        }
    });
    out.sort_by_key(|(s, _)| *s);
    out.into_iter().map(|(_, c)| c).collect()
}

#[test]
fn test_closure_capture_rules() {
    let m = oop::mapping();
    let body_this = |w: &mut W| {
        w.ret(|w| {
            w.prop("this", "v");
        });
    };
    let (out, names, _) = oop::lower(&m, |w| {
        w.class("C", None, |w| {
            w.function(X::Method, &[X::PublicKw], "m", &["x"], |w| {
                // function () { … $this … }: an implicit by-value capture.
                w.stmt(|w| {
                    w.closure(false, &[], &[], body_this);
                });
                // function () use ($x) { … }: the receiver first, then $x.
                w.stmt(|w| {
                    w.closure(false, &[], &["x"], body_this);
                });
                // function () use ($this) { … }: spelled, not doubled.
                w.stmt(|w| {
                    w.closure(false, &[], &["this"], body_this);
                });
                // fn () => $this->v: reached through implicit captures.
                w.stmt(|w| {
                    w.arrow(false, &[], |w| {
                        w.prop("this", "v");
                    });
                });
                // static function () { }: none.
                w.stmt(|w| {
                    w.closure(true, &[], &[], |_| {});
                });
            });
            w.function(X::Method, &[X::StaticKw], "s", &[], |w| {
                // A closure in a static method: no receiver to capture.
                w.stmt(|w| {
                    w.closure(false, &[], &[], |_| {});
                });
            });
        });
        // A closure outside any class.
        w.stmt(|w| {
            w.closure(false, &[], &[], |_| {});
        });
    });
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let by = CaptureMode::ByValue;
    let this = || ("this".to_string(), by);
    assert_eq!(
        closure_captures(&out.hir, &names),
        vec![
            vec![this()],
            vec![this(), ("x".to_string(), by)],
            vec![this()],
            vec![],
            vec![],
            vec![],
            vec![],
        ]
    );
    // Resolution: every `$this` reaches a receiver or a capture of it.
    let (hir, diags) = oop::resolve(&out, &names);
    assert!(diags.is_empty(), "{diags:?}");
    let this_sym = names.get("this").expect("interned");
    hir_lang::walk(&hir, |n| {
        if let NodeRef::Expr(e) = n {
            if let Expr::Path(p) = hir.expr(e) {
                let path = hir.path(*p);
                if hir.list(path.segments).first().map(|s| s.name.sym) == Some(this_sym) {
                    let Res::Local(b) = path.res else {
                        panic!("`$this` resolved: {:?}", path.res)
                    };
                    let kind = hir.binder(b).expect("binder").kind;
                    assert!(
                        matches!(kind, BinderKind::Param | BinderKind::Capture),
                        "{kind:?}"
                    );
                }
            }
        }
    });
}

#[test]
fn test_this_in_a_static_closure_is_a_problem() {
    let m = oop::mapping();
    let (out, _, src) = oop::lower(&m, |w| {
        w.class("C", None, |w| {
            w.function(X::Method, &[X::PublicKw], "m", &[], |w| {
                // static fn () => $this->v
                w.stmt(|w| {
                    w.arrow(true, &[], |w| {
                        w.prop("this", "v");
                    });
                });
                // static function () { return fn () => $this; }
                w.stmt(|w| {
                    w.closure(true, &[], &[], |w| {
                        w.ret(|w| {
                            w.arrow(false, &[], |w| {
                                w.var("this");
                            });
                        });
                    });
                });
            });
        });
    });
    let found: Vec<_> = out
        .problems
        .iter()
        .map(|p| {
            (
                p.kind,
                &src[p.span.start().to_usize()..p.span.end().to_usize()],
            )
        })
        .collect();
    assert_eq!(
        found,
        vec![
            (ProblemKind::NoReceiver, "$this"),
            (ProblemKind::NoReceiver, "$this"),
        ]
    );
    assert_eq!(
        ProblemKind::NoReceiver.to_string(),
        "the receiver is not available here: a static closure (and any closure inside one) has none"
    );
}

#[test]
fn test_this_is_declared_so_it_never_becomes_an_implicit_local() {
    // `$this = 1;` inside a method must not hoist a local `this` that would
    // shadow the receiver (PHP rejects the assignment; HIR keeps it a write
    // to the receiver's binder for the resolver to judge).
    let m = oop::mapping();
    let (out, names, _) = oop::lower(&m, |w| {
        w.class("C", None, |w| {
            w.function(X::Method, &[X::PublicKw], "m", &[], |w| {
                w.stmt(|w| {
                    w.assign(
                        |w| {
                            w.var("this");
                        },
                        |w| {
                            w.num(1);
                        },
                    );
                });
            });
        });
    });
    let this_sym = names.get("this").expect("interned");
    let mut locals = 0;
    for i in 0..out.hir.count(hir_lang::IdKind::Binder) {
        let b = out
            .hir
            .binder(hir_lang::BinderId::from_index(i).unwrap())
            .unwrap();
        if b.name.sym == this_sym && b.kind == BinderKind::Local {
            locals += 1;
        }
    }
    assert_eq!(locals, 0);
}

#[test]
fn test_template_binders_never_capture_the_receiver() {
    // A template that binds its own `this` around a user body: the user's
    // `$this` still reaches the receiver (hygiene), and the template's
    // `this` stays its own.
    let mut b = MappingBuilder::<X>::new();
    let shadow = b
        .parse_template("shadow", "{ let this = 1; this; ${body} }")
        .expect("parses");
    let mut policy = LangPolicy::php();
    policy.implicit_receiver = Some(b.name("this"));
    b.policy(policy);
    let a = lower_lang::Pick::Absent;
    b.node(
        X::File,
        lower_lang::Arch::Module {
            name: a,
            items: lower_lang::Pick::Nodes,
        },
    );
    b.node(
        X::ClassDecl,
        lower_lang::Arch::Class {
            name: lower_lang::Pick::After(X::ClassKw),
            generics: a,
            bases: a,
            interfaces: a,
            members: lower_lang::Pick::Nodes,
            mixin: false,
        },
    );
    b.node(
        X::Method,
        lower_lang::Arch::Fn {
            name: lower_lang::Pick::After(X::FunctionKw),
            generics: a,
            params: lower_lang::Pick::Kind(X::ParamList),
            ret: a,
            throws: a,
            body: lower_lang::Pick::Kind(X::Block),
            where_: a,
        },
    );
    b.container(X::ParamList);
    b.template_node(X::Block, shadow, &[lower_lang::Pick::After(X::LBrace)]);
    b.token(X::Var, lower_lang::TokenRole::Var { sigil: 1 });
    b.token(X::Ident, lower_lang::TokenRole::Name);
    let m = b.build().expect("valid");
    let (out, names, _) = oop::lower(&m, |w| {
        w.class("C", None, |w| {
            w.open(X::Method)
                .t(X::FunctionKw, "function")
                .t(X::Ident, "m");
            w.params(&[]);
            w.open(X::Block)
                .t(X::LBrace, "{")
                .var("this")
                .t(X::RBrace, "}")
                .close();
            w.close();
        });
    });
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let (hir, diags) = oop::resolve(&out, &names);
    assert!(diags.is_empty(), "{diags:?}");
    // Two paths named `this`: the template's (marked) resolves to its own
    // local, the user's (unmarked) to the receiver parameter.
    let this_sym = names.get("this").expect("interned");
    let mut kinds = Vec::new();
    hir_lang::walk(&hir, |n| {
        if let NodeRef::Expr(e) = n {
            if let Expr::Path(p) = hir.expr(e) {
                let path = hir.path(*p);
                let Some(seg) = hir.list(path.segments).first() else {
                    return;
                };
                if seg.name.sym == this_sym {
                    let Res::Local(bd) = path.res else {
                        panic!("resolved")
                    };
                    let binder = hir.binder(bd).expect("binder");
                    assert_eq!(binder.name.mark, seg.name.mark, "hygiene");
                    kinds.push((seg.name.mark.is_root(), binder.kind));
                }
            }
        }
    });
    kinds.sort_by_key(|k| k.0);
    assert_eq!(
        kinds,
        vec![(false, BinderKind::Local), (true, BinderKind::Param)]
    );
}

#[test]
fn test_receiver_name_must_come_from_the_builder() {
    let mut other = MappingBuilder::<u8>::new();
    for i in 0..5 {
        let _ = other.name(&format!("n{i}"));
    }
    let foreign = other.name("this");
    let mut b = MappingBuilder::<u8>::new();
    let mut policy = LangPolicy::php();
    policy.implicit_receiver = Some(foreign);
    b.policy(policy);
    assert_eq!(
        b.build().err(),
        Some(MappingError::UnknownName { index: 5 })
    );
}

#[test]
fn test_receivers_are_deterministic_and_origins_stay_in_source() {
    let m = oop::mapping();
    let build = |w: &mut W| {
        class_a(w);
        class_b(w);
    };
    let (one, n1, src) = oop::lower(&m, build);
    let (two, n2, _) = oop::lower(&m, build);
    assert_eq!(
        hir_lang::print(&one.hir, &n1),
        hir_lang::print(&two.hir, &n2)
    );
    hir_lang::walk(&one.hir, |n| {
        assert!(one.hir.origin(n).span.end().to_usize() <= src.len());
    });
    for i in 0..one.hir.count(hir_lang::IdKind::Binder) {
        let b = hir_lang::BinderId::from_index(i).unwrap();
        assert!(one.hir.binder_origin(b).span.end().to_usize() <= src.len());
    }
}

#[test]
fn test_new_by_class_name_and_by_run_time_name_run_end_to_end() {
    // `echo (new B(1))->get();` (the class as the template's value operand,
    // resolved by resolve-lang 0.4.1) and `$cls = 'B'; echo (new $cls(7))->get();`
    // (bcgen-lang's run-time class lookup).
    let r = oop::run(|w| {
        class_a(w);
        w.class("B", Some("A"), |_| {});
        w.echo(|w| {
            w.mcall(
                |w| {
                    w.new_obj("B", 1, |w, _| {
                        w.num(1);
                    });
                },
                "get",
                0,
                |_, _| {},
            );
        });
        w.stmt(|w| {
            w.assign(
                |w| {
                    w.var("cls");
                },
                |w| {
                    w.t(X::Str, "'B'");
                },
            );
        });
        w.echo(|w| {
            w.mcall(
                |w| {
                    w.open(X::New).t(X::NewKw, "new").var("cls");
                    w.args(1, &[], |w, _| {
                        w.num(7);
                    });
                    w.close();
                },
                "get",
                0,
                |_, _| {},
            );
        });
    });
    assert_eq!(r, lines(&["1", "7"]));
}

/// `class A { $x; function __construct($x) { $this->x = $x; } function get() { … } }`
/// and `class B extends A { function __construct() { parent::__construct(1); } }`
/// with no visibility keyword anywhere (the property is spelled `var $x;`
/// in PHP; here it is a property node with no modifier).
fn unmarked_classes(w: &mut W) {
    w.class("A", None, |w| {
        w.open(X::Property).var("x").t(X::Semi, ";").close();
        w.function(X::Method, &[], "__construct", &["x"], |w| {
            w.stmt(|w| {
                w.assign(
                    |w| {
                        w.prop("this", "x");
                    },
                    |w| {
                        w.var("x");
                    },
                );
            });
        });
        w.function(X::Method, &[], "get", &[], |w| {
            w.ret(|w| {
                w.prop("this", "x");
            });
        });
    });
    w.class("B", Some("A"), |w| {
        w.function(X::Method, &[], "__construct", &[], |w| {
            w.stmt(|w| {
                w.call(
                    |w| {
                        w.open(X::StaticMember)
                            .t(X::ParentKw, "parent")
                            .t(X::ColonColon, "::")
                            .t(X::Ident, "__construct")
                            .close();
                    },
                    1,
                    &[],
                    |w, _| {
                        w.num(1);
                    },
                );
            });
        });
    });
}

#[test]
fn test_members_without_a_visibility_keyword_default_to_public_end_to_end() {
    // `$b = new B(); echo $b->get(); echo $b->x;`
    let r = oop::run(|w| {
        unmarked_classes(w);
        new_into(w, "b", "B", &[]);
        w.echo(|w| {
            w.mcall(
                |w| {
                    w.var("b");
                },
                "get",
                0,
                |_, _| {},
            );
        });
        w.echo(|w| {
            w.prop("b", "x");
        });
    });
    assert_eq!(r, lines(&["1", "1"]));
}

/// The visibility of every item and field named `name`.
fn vis_of_named(hir: &Hir, names: &Interner, name: &str) -> Vec<hir_lang::Vis> {
    let mut out = Vec::new();
    hir_lang::walk(hir, |n| match n {
        NodeRef::Item(i) => {
            let item = hir.item(i);
            if item.name.and_then(|nm| names.resolve(nm.sym)) == Some(name) {
                out.push(item.vis);
            }
        }
        NodeRef::Field(f) => {
            let fd = hir.field(f);
            if fd.name.and_then(|nm| names.resolve(nm.sym)) == Some(name) {
                out.push(fd.vis);
            }
        }
        _ => {}
    });
    out
}

#[test]
fn test_default_member_visibility_applies_only_to_unmarked_members() {
    use hir_lang::Vis;
    let program = |w: &mut W| {
        unmarked_classes(w);
        w.class("C", None, |w| {
            w.function(X::Method, &[X::PrivateKw], "hidden", &[], |_| {});
            w.function(X::Method, &[X::StaticKw], "make", &[], |_| {});
        });
        w.open(X::InterfaceDecl)
            .t(X::InterfaceKw, "interface")
            .t(X::Ident, "I")
            .t(X::LBrace, "{");
        w.open(X::Method)
            .t(X::FunctionKw, "function")
            .t(X::Ident, "area");
        w.params(&[]);
        w.t(X::Semi, ";").close();
        w.t(X::RBrace, "}").close();
        w.open(X::ImplBlock)
            .t(X::ImplKw, "impl")
            .t(X::Ident, "I")
            .t(X::LBrace, "{");
        w.function(X::Method, &[], "implemented", &[], |_| {});
        w.t(X::RBrace, "}").close();
        w.function(X::FnDecl, &[], "free", &[], |_| {});
    };
    let public = oop::mapping();
    let (out, names, _) = oop::lower(&public, program);
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let hir = &out.hir;
    assert_eq!(
        vis_of_named(hir, &names, "__construct"),
        [Vis::Public, Vis::Public]
    );
    assert_eq!(vis_of_named(hir, &names, "get"), [Vis::Public]);
    assert_eq!(vis_of_named(hir, &names, "x"), [Vis::Public]);
    assert_eq!(vis_of_named(hir, &names, "make"), [Vis::Public]);
    assert_eq!(vis_of_named(hir, &names, "area"), [Vis::Public]);
    // A keyword wins; `impl` members and free functions keep the 0.4.0 rule.
    assert_eq!(vis_of_named(hir, &names, "hidden"), [Vis::Private]);
    assert_eq!(vis_of_named(hir, &names, "implemented"), [Vis::Private]);
    assert_eq!(vis_of_named(hir, &names, "free"), [Vis::Private]);
    // The default policy keeps 0.4.0's private members, and resolve-lang
    // then refuses `parent::__construct`.
    let private = oop::mapping_full(true, true, false);
    let (out, names, _) = oop::lower(&private, program);
    assert_eq!(vis_of_named(&out.hir, &names, "get"), [Vis::Private]);
    let (_, diags) = oop::resolve(&out, &names);
    assert_eq!(diags, vec!["`__construct` is private".to_string()]);
    assert_eq!(
        lower_lang::LangPolicy::default().default_member_visibility,
        Vis::Private
    );
    assert_eq!(
        lower_lang::LangPolicy::php().default_member_visibility,
        Vis::Private
    );
}

// ------------------------------------------- reference: who sees `$this`

use proptest::prelude::*;

/// A random nest of methods, closures, and functions reading `$this`.
#[derive(Clone, Debug)]
enum S {
    /// `$this;`
    Read,
    /// A closure: static or not, arrow (implicit captures) or `function`
    /// (explicit only), with `use ($this)` or not.
    Closure {
        is_static: bool,
        arrow: bool,
        spells_this: bool,
        body: Vec<S>,
    },
    /// A named function declared inside: never a method.
    Function(Vec<S>),
}

fn body() -> impl Strategy<Value = Vec<S>> {
    let leaf = Just(S::Read);
    let s = leaf.prop_recursive(5, 48, 3, |inner| {
        prop_oneof![
            3 => (any::<bool>(), any::<bool>(), any::<bool>(), prop::collection::vec(inner.clone(), 0..3))
                .prop_map(|(is_static, arrow, spells_this, body)| S::Closure { is_static, arrow, spells_this, body }),
            1 => prop::collection::vec(inner, 0..3).prop_map(S::Function),
        ]
    });
    prop::collection::vec(s, 0..3)
}

/// Methods (static or not) of one class, plus top-level code.
fn program() -> impl Strategy<Value = (Vec<(bool, Vec<S>)>, Vec<S>)> {
    (prop::collection::vec((any::<bool>(), body()), 0..3), body())
}

/// The receiver state the reference tracks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum St {
    None,
    Visible,
    Hidden,
}

/// The reference: (receiver parameters, per closure in source order
/// whether it carries a capture named `this`, `NoReceiver` problems).
fn reference(methods: &[(bool, Vec<S>)], top: &[S]) -> (usize, Vec<bool>, usize) {
    let mut receivers = 0;
    let mut caps = Vec::new();
    let mut problems = 0;
    fn walk(list: &[S], st: St, caps: &mut Vec<bool>, problems: &mut usize) {
        let mut work: Vec<(&S, St)> = list.iter().rev().map(|s| (s, st)).collect();
        while let Some((s, st)) = work.pop() {
            match s {
                S::Read => *problems += usize::from(st == St::Hidden),
                S::Function(b) => {
                    for x in b.iter().rev() {
                        work.push((x, St::None));
                    }
                }
                S::Closure {
                    is_static,
                    arrow,
                    spells_this,
                    body,
                } => {
                    let inner = match (st, *is_static) {
                        (St::None, _) => St::None,
                        (St::Visible, false) => St::Visible,
                        _ => St::Hidden,
                    };
                    // `use ($this)` names it explicitly; an explicit-only
                    // closure in a visible context gets it implicitly.
                    let explicit = !*arrow && *spells_this;
                    caps.push(explicit || (inner == St::Visible && !*arrow));
                    for x in body.iter().rev() {
                        work.push((x, inner));
                    }
                }
            }
        }
    }
    for (is_static, b) in methods {
        if !*is_static {
            receivers += 1;
        }
        let st = if *is_static { St::None } else { St::Visible };
        walk(b, st, &mut caps, &mut problems);
    }
    walk(top, St::None, &mut caps, &mut problems);
    (receivers, caps, problems)
}

fn write_body(w: &mut W, list: &[S]) {
    for s in list {
        match s {
            S::Read => {
                w.stmt(|w| {
                    w.var("this");
                });
            }
            S::Function(b) => {
                w.function(X::FnDecl, &[], "inner", &[], |w| write_body(w, b));
            }
            S::Closure {
                is_static,
                arrow,
                spells_this,
                body,
            } => {
                w.stmt(|w| {
                    if *arrow {
                        // `fn () => { … }` is not PHP, but the archetype
                        // takes any body: a block keeps the shapes simple.
                        w.open(X::ArrowFn);
                        if *is_static {
                            w.t(X::StaticKw, "static");
                        }
                        w.t(X::FnKw, "fn");
                        w.params(&[]);
                        w.t(X::FatArrow, "=>");
                        w.block(|w| write_body(w, body));
                        w.close();
                    } else {
                        let caps: &[&str] = if *spells_this { &["this"] } else { &[] };
                        w.closure(*is_static, &[], caps, |w| write_body(w, body));
                    }
                });
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// Receivers, closure captures of `$this`, and `NoReceiver` problems
    /// match the reference; the HIR is valid, hygienic, deterministic, and
    /// every resolved `$this` names a receiver or a capture.
    #[test]
    fn prop_receivers_match_the_reference((methods, top) in program()) {
        let m = oop::mapping();
        let build = |w: &mut W| {
            w.class("C", None, |w| {
                for (i, (is_static, b)) in methods.iter().enumerate() {
                    let mods: &[X] = if *is_static { &[X::PublicKw, X::StaticKw] } else { &[X::PublicKw] };
                    w.function(X::Method, mods, &format!("m{i}"), &[], |w| write_body(w, b));
                }
            });
            write_body(w, &top);
        };
        let (out, names, _) = oop::lower(&m, build);
        let (receivers, caps, problems) = reference(&methods, &top);
        let mut got_receivers = 0;
        hir_lang::walk(&out.hir, |n| {
            if let NodeRef::Param(p) = n {
                got_receivers += usize::from(out.hir.param(p).kind == ParamKind::Receiver);
            }
        });
        prop_assert_eq!(got_receivers, receivers);
        let got_caps: Vec<bool> = closure_captures(&out.hir, &names)
            .iter()
            .map(|c| c.iter().any(|(n, _)| n == "this"))
            .collect();
        prop_assert_eq!(got_caps, caps);
        let got_problems = out.problems.iter().filter(|p| p.kind == ProblemKind::NoReceiver).count();
        prop_assert_eq!(got_problems, problems);
        // No capture is ever doubled.
        prop_assert!(closure_captures(&out.hir, &names)
            .iter()
            .all(|c| c.iter().filter(|(n, _)| n == "this").count() <= 1));
        let (hir, _) = oop::resolve(&out, &names);
        prop_assert_eq!(hir.validate(), Ok(()));
        let this = names.get("this");
        let mut ok = true;
        hir_lang::walk(&hir, |n| {
            if let NodeRef::Expr(e) = n {
                if let Expr::Path(p) = hir.expr(e) {
                    let path = hir.path(*p);
                    if hir.list(path.segments).first().map(|s| s.name.sym) == this {
                        if let Res::Local(b) = path.res {
                            let kind = hir.binder(b).map(|b| b.kind);
                            ok &= matches!(kind, Some(BinderKind::Param | BinderKind::Capture));
                        }
                    }
                }
            }
        });
        prop_assert!(ok);
        let (again, names2, _) = oop::lower(&m, build);
        prop_assert_eq!(hir_lang::print(&out.hir, &names), hir_lang::print(&again.hir, &names2));
    }
}
