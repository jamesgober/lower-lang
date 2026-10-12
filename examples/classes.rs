//! Mox classes, end to end: a class with a constructor setting `$this->x`,
//! a subclass calling `parent::__construct`, a closure over `$this`, and a
//! nested loop left with `break 2`. Lowered with the 0.4.1 policies
//! (`LangPolicy::implicit_receiver`, `BreakLevels::Numeric`), printed as HIR,
//! then resolved (resolve-lang 0.4), compiled (bcgen-lang 0.4), and run
//! (bvm-lang 2.0.0-alpha.4).
//!
//! The trees are built the way lang-forge 2.0 shapes Mox's rules; the
//! mapping is the one in `tests/support/mox_oop.rs`, which sets the keys
//! `sketches/mox.lsf` sets.
//!
//! ```sh
//! cargo run --example classes
//! ```

#![allow(clippy::print_stdout, clippy::expect_used, clippy::unwrap_used)]

#[path = "../tests/support/mox_oop.rs"]
mod oop;

use intern_lang::Interner;
use oop::{W, X};

fn main() {
    // class Point { public $x;
    //   public function __construct($x) { $this->x = $x; }
    //   public function adder() { return function ($n) { return $this->x + $n; }; } }
    // class Point3 extends Point {
    //   public function __construct($x) { parent::__construct($x + 1); } }
    // $cls = 'Point3'; $p = new $cls(41);
    // echo ($p->adder())(100);
    // $i = 0; while ($i < 5) { $i = $i + 1; loop { if ($i < 3) { continue 2; } break 2; } }
    // echo $i;
    let mut w = W::new();
    w.open(X::File);
    w.class("Point", None, |w| {
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
        w.function(X::Method, &[X::PublicKw], "adder", &[], |w| {
            w.ret(|w| {
                w.closure(false, &["n"], &[], |w| {
                    w.ret(|w| {
                        w.bin(
                            X::Plus,
                            |w| {
                                w.prop("this", "x");
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
    w.class("Point3", Some("Point"), |w| {
        w.function(X::Method, &[X::PublicKw], "__construct", &["x"], |w| {
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
                        w.bin(
                            X::Plus,
                            |w| {
                                w.var("x");
                            },
                            |w| {
                                w.num(1);
                            },
                        );
                    },
                );
            });
        });
    });
    w.stmt(|w| {
        w.assign(
            |w| {
                w.var("cls");
            },
            |w| {
                w.t(X::Str, "'Point3'");
            },
        );
    });
    w.stmt(|w| {
        w.assign(
            |w| {
                w.var("p");
            },
            |w| {
                w.open(X::New).t(X::NewKw, "new").var("cls");
                w.args(1, &[], |w, _| {
                    w.num(41);
                });
                w.close();
            },
        );
    });
    w.echo(|w| {
        w.call(
            |w| {
                w.mcall(
                    |w| {
                        w.var("p");
                    },
                    "adder",
                    0,
                    |_, _| {},
                );
            },
            1,
            &[],
            |w, _| {
                w.num(100);
            },
        );
    });
    w.stmt(|w| {
        w.assign(
            |w| {
                w.var("i");
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
                    w.var("i");
                },
                |w| {
                    w.num(5);
                },
            );
        },
        |w| {
            w.stmt(|w| {
                w.assign(
                    |w| {
                        w.var("i");
                    },
                    |w| {
                        w.bin(
                            X::Plus,
                            |w| {
                                w.var("i");
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
                                w.var("i");
                            },
                            |w| {
                                w.num(3);
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
    w.echo(|w| {
        w.var("i");
    });
    w.close();
    let (tree, src) = w.finish();

    // The Tier-1 call.
    let mapping = oop::mapping();
    let mut names = Interner::new();
    let out = lower_lang::lower(&tree, &src, &mapping, &mut names).expect("within budget");
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    println!("{}\n", hir_lang::print(&out.hir, &names));

    let (hir, diags) = oop::resolve(&out, &names);
    assert!(diags.is_empty(), "{diags:?}");
    match oop::execute(&hir, &names) {
        oop::Run::Printed(lines) => println!("prints: {lines:?}"),
        oop::Run::Failed(e, lines) => println!("failed: {e} after {lines:?}"),
    }
}
