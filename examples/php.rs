//! PHP semantics: a Mox (PHP-like) program lowered under
//! `LangPolicy::php()`. It shows, in one function:
//!
//! - a group `use` flattened into one HIR import per leaf;
//! - an attribute (`#[Pure]`) attached to the function as an HIR attribute;
//! - `foreach` variables that stay visible after the loop (function-scoped,
//!   assigned from hygienic temporaries);
//! - an interpolated string and an indented heredoc, joined with `mox.concat`
//!   (the heredoc's closing-marker indentation is removed from its lines);
//! - PHP number spelling (`0777` is octal, `1_000` has a separator).
//!
//! The tree is built with the same writer the Mox tests use
//! (`tests/support/mox.rs`, which also documents the CST shapes the mapping
//! expects).
//!
//! ```sh
//! cargo run --example php
//! ```

#![allow(clippy::print_stdout, clippy::expect_used)]

#[path = "../tests/support/mox.rs"]
mod mox;

use hir_lang::{IdKind, ItemKind, NodeRef};
use intern_lang::Interner;
use mox::{M, W};

fn main() {
    // use App\Models\{User, Post as P};
    // #[Pure] function show($items) {
    //     foreach ($items as $k => $v) { echo "item {$k}: $v!"; }
    //     echo <<<EOT
    //         last {$k}
    //         EOT;
    //     echo $v, 0777, 1_000;
    // }
    let mut w = W::new();
    w.open(M::File);
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

    w.open(M::FnDecl);
    w.open(M::AttrGroup).t(M::HashBracket, "#[");
    w.open(M::Attr).t(M::Ident, "Pure").close();
    w.t(M::RBracket, "]").close();
    w.t(M::Function, "function")
        .t(M::Ident, "show")
        .t(M::LParen, "(");
    w.open(M::Params)
        .open(M::Param)
        .var("items")
        .close()
        .close();
    w.t(M::RParen, ")").open(M::Block).t(M::LBrace, "{");
    // foreach ($items as $k => $v) { echo "item {$k}: $v!"; }
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
    // echo <<<EOT ... EOT;
    w.open(M::Echo).t(M::EchoKw, "echo");
    w.open(M::HeredocStr)
        .t(M::HeredocOpen, "<<<EOT\n")
        .t(M::StrPart, "    last ")
        .t(M::CurlyOpen, "{")
        .var("k")
        .t(M::CurlyClose, "}")
        .t(M::StrPart, "\n")
        .t(M::HeredocClose, "    EOT")
        .close();
    w.t(M::Semi, ";").close();
    // echo $v, 0777, 1_000;   ($v is still in scope after the loop)
    w.open(M::Echo)
        .t(M::EchoKw, "echo")
        .var("v")
        .t(M::Comma, ",")
        .t(M::Num, "0777")
        .t(M::Comma, ",")
        .t(M::Num, "1_000")
        .t(M::Semi, ";")
        .close();
    w.t(M::RBrace, "}").close().close();
    w.close();
    let (tree, src) = w.finish();

    let mapping = mox::mox_mapping();
    let mut names = Interner::new();
    let out = lower_lang::lower(&tree, &src, &mapping, &mut names).expect("within budget");
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let hir = &out.hir;
    let printed = hir_lang::print(hir, &names);
    println!("{printed}\n");

    // One import per leaf of the group `use`.
    let ItemKind::Module { items, .. } = hir.item(hir.root()).kind else {
        unreachable!("the root is a module")
    };
    let imports = hir
        .list(items)
        .iter()
        .filter(|&&i| matches!(hir.item(i).kind, ItemKind::Import { .. }))
        .count();
    println!("imports from the group use: {imports}");
    assert_eq!(imports, 2);

    // The attribute is on the function.
    let show = hir
        .list(items)
        .iter()
        .copied()
        .find(|&i| matches!(hir.item(i).kind, ItemKind::Fn(_)))
        .expect("the function");
    let attrs = hir.attrs(NodeRef::Item(show));
    println!(
        "attributes on show(): {:?}",
        attrs
            .iter()
            .map(|a| names.resolve(a.name.sym).unwrap_or("?"))
            .collect::<Vec<_>>()
    );
    assert_eq!(attrs.len(), 1);

    // `foreach` binds hygienic temporaries, each assigned to the
    // function-scoped `$k` / `$v` (one `hoist` expansion per binding).
    let expansions = u32::try_from(hir.count(IdKind::Expansion)).expect("small");
    let hoists = (1..=expansions)
        .filter_map(|i| hir.expansion(hir_lang::ExpnId::from_u32(i)))
        .filter(|e| names.resolve(e.name) == Some("hoist"))
        .count();
    println!("function-scoped foreach bindings: {hoists}");
    assert_eq!(hoists, 2);

    // PHP literals: 0777 is octal.
    assert!(printed.contains("(lit 511)"));
    assert!(printed.contains("(lit 1000)"));
    // The heredoc's indentation is gone.
    assert!(printed.contains("\"last \""));
}
