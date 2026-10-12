//! # lower_lang
//!
//! Lowers a lossless syntax tree into HIR. A language describes what its
//! constructs *mean* as data — a [`Mapping`] from node kinds to semantic
//! [archetypes](Arch) (binders, scopes, declarations, control, expressions,
//! patterns, types), operator tables with their policies, and hygienic
//! [templates](Template) for sugar — and lower-lang builds the HIR, records
//! where every node came from, and validates the result.
//!
//! ## The lazy path
//!
//! Build a mapping once, then [`lower`] any tree of that language:
//!
//! ```
//! use hir_lang::OpKind;
//! use intern_lang::Interner;
//! use lower_lang::{lower, Arch, MappingBuilder, OpSpec, Pick, TokenRole};
//! use syntax_lang::{Element, Node, Span, Token, TokenKind};
//!
//! #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
//! enum K { File, Add, Num, Plus, Space }
//! impl TokenKind for K {
//!     fn is_trivia(&self) -> bool { matches!(self, K::Space) }
//! }
//!
//! let mut b = MappingBuilder::new();
//! b.node(K::File, Arch::Module { name: Pick::Absent, items: Pick::Nodes });
//! b.node(K::Add, Arch::binary());
//! b.token(K::Num, TokenRole::Int);
//! b.binary_op(K::Plus, OpSpec::Op(OpKind::Add));
//! let mapping = b.build()?;
//!
//! // `1 + 2`
//! let tok = |k, s, e| Element::Token(Token::new(k, Span::new(s, e)));
//! let add = Node::new(K::Add, vec![
//!     tok(K::Num, 0, 1), tok(K::Space, 1, 2), tok(K::Plus, 2, 3),
//!     tok(K::Space, 3, 4), tok(K::Num, 4, 5),
//! ]);
//! let file = Node::new(K::File, vec![Element::Node(add)]);
//!
//! let mut names = Interner::new();
//! let out = lower(&file, "1 + 2", &mapping, &mut names).expect("within budget");
//! assert!(out.problems.is_empty());
//! assert!(hir_lang::print(&out.hir, &names).contains("(op add overflow=error"));
//! # Ok::<(), lower_lang::MappingError>(())
//! ```
//!
//! ## Templates as text
//!
//! Sugar is lowered through hygienic [templates](Template), written in HIR's
//! template syntax and parsed once (never substituted):
//!
//! ```
//! use lower_lang::{MappingBuilder, OpSpec};
//!
//! let mut b = MappingBuilder::<u8>::new();
//! // PHP's `&&` (a bool), and `++$x` with the place evaluated once.
//! let and = b.parse_template("bool_and", "if ${lhs} { not(not(${rhs})) } else { false }")?;
//! let inc = b.parse_template("pre_increment", "let_place p = ${place} in p = add(p, 1)")?;
//! b.binary_op(1, OpSpec::Template(and));
//! b.prefix_op(2, OpSpec::Template(inc));
//! assert!(b.build().is_ok());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! ## What lowering guarantees
//!
//! - **It never panics and always returns valid HIR** for any tree, including
//!   trees full of syntax errors, unmapped kinds, missing parts, and spans
//!   outside the source: each broken construct becomes an HIR error form and a
//!   [`Problem`] with its source span, and hir-lang's lenient validation
//!   repairs and reports everything else. Only an exhausted [`Budget`] (or
//!   exhausted interner or arena space) stops it.
//! - **Every HIR node has an origin inside the source**: its own syntax span,
//!   or, for template output, the span of the node that invoked the template,
//!   with the template's expansion recorded.
//! - **Templates are hygienic**: names a template introduces carry its
//!   expansion's mark and are resolved by construction, so they never capture
//!   or shadow user names.
//! - **It is deterministic and iterative**: the same input lowers to the same
//!   HIR, at any tree depth.
//!
//! ## `no_std`
//!
//! The crate needs only `alloc`. The default `std` feature is additive.

#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(unused_must_use)]
#![deny(unused_results)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::todo)]
#![deny(clippy::unimplemented)]
#![deny(clippy::print_stdout)]
#![deny(clippy::print_stderr)]
#![deny(clippy::dbg_macro)]
#![deny(clippy::unreachable)]

extern crate alloc;

mod arch;
mod lower;
mod mapping;
mod pick;
mod problem;
mod template;
pub mod templates;
mod text;

pub use arch::{Arch, RecordShape, TemplateUse};
pub use hir_lang::Span;
pub use lower::{Budget, Lowered, Lowerer};
pub use mapping::{
    EffectMode, Escapes, Fixity, IdentPatterns, Join, LangPolicy, LegacyOctal, Literals, Mapping,
    MappingBuilder, MappingError, Modifier, NameRef, OpSpec, PlaceArgs, Separators, TokenRole,
    TypeName,
};
pub use pick::{Labeler, Pick};
pub use problem::{Context, HoistBlocker, LowerError, Problem, ProblemKind};
pub use template::{
    ArgsHole, ExprHole, HoleKind, LabelHole, LabelRef, MAX_TEMPLATE_SIZE, PatHole, StmtsHole, TArg,
    TBinder, TExpr, TLabel, TLit, TPat, TStmt, TTy, Template, TemplateBuilder, TemplateError,
    TemplateId, TyHole,
};
pub use text::{MAX_TEMPLATE_DEPTH, MAX_TEMPLATE_TEXT, SyntaxError};

use intern_lang::Interner;
use syntax_lang::{Node, TokenKind};

/// Lowers `tree`, parsed from `source`, into a validated HIR unit with the
/// default [`Budget`], names interned in `names`.
///
/// The Tier-1 entry point; use [`Lowerer`] for a budget, a unit id, or field
/// labels.
///
/// # Errors
///
/// Only for exhausted resources: [`LowerError::NodeBudget`],
/// [`LowerError::SymbolSpaceExhausted`], or [`LowerError::Hir`] when hir-lang's
/// arenas overflow. Problems in the code are never errors; they are in
/// [`Lowered::problems`], and the HIR is still valid.
///
/// # Examples
///
/// See the [crate documentation](crate).
pub fn lower<K>(
    tree: &Node<K>,
    source: &str,
    mapping: &Mapping<K>,
    names: &mut Interner,
) -> Result<Lowered, LowerError>
where
    K: Copy + Ord + TokenKind,
{
    Lowerer::new(mapping).lower(tree, source, names)
}

/// Compiles and runs the `rust` code blocks in `README.md` and `docs/API.md` as
/// part of `cargo test`, so the published examples cannot drift from the API.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
#[doc = include_str!("../docs/API.md")]
pub struct MarkdownDocTests;
