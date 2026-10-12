//! What lowering reports: problems in the code (values, never panics) and the
//! few failures that stop it.

use core::fmt;

use hir_lang::{HirError, Span};

use crate::template::TemplateId;

/// The context a construct was lowered in, for problem messages.
///
/// # Examples
///
/// ```
/// use lower_lang::Context;
///
/// assert_eq!(Context::Pattern.to_string(), "a pattern");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Context {
    /// An expression.
    Expression,
    /// A statement in a block or module body.
    Statement,
    /// A declaration (module or block item).
    Item,
    /// A member of a class, interface, or impl.
    Member,
    /// A pattern.
    Pattern,
    /// A type.
    Type,
    /// A path.
    Path,
    /// A name.
    Name,
    /// A parameter.
    Parameter,
    /// A call argument.
    Argument,
    /// A match arm or catch clause.
    Arm,
    /// A closure capture.
    Capture,
    /// A record, variant, or class field.
    Field,
    /// A sum variant.
    Variant,
    /// A generic parameter, argument, or where-predicate.
    Generic,
    /// A map entry or field initializer.
    Entry,
    /// A path segment.
    Segment,
    /// A label.
    Label,
}

impl fmt::Display for Context {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Expression => "an expression",
            Self::Statement => "a statement",
            Self::Item => "a declaration",
            Self::Member => "a member",
            Self::Pattern => "a pattern",
            Self::Type => "a type",
            Self::Path => "a path",
            Self::Name => "a name",
            Self::Parameter => "a parameter",
            Self::Argument => "an argument",
            Self::Arm => "a match arm",
            Self::Capture => "a capture",
            Self::Field => "a field",
            Self::Variant => "a variant",
            Self::Generic => "a generic parameter or argument",
            Self::Entry => "an entry",
            Self::Segment => "a path segment",
            Self::Label => "a label",
        })
    }
}

/// What is wrong with one construct.
///
/// Every kind except [`Hir`](ProblemKind::Hir) comes from lowering itself; a
/// `Hir` problem comes from hir-lang's lenient validator (a `break` outside a
/// loop, a duplicate named argument, an out-of-range literal, ...). Either
/// way the construct became an HIR error form and the rest of the unit was
/// lowered.
///
/// # Examples
///
/// ```
/// use lower_lang::ProblemKind;
///
/// assert_eq!(ProblemKind::UnknownLabel.to_string(), "no enclosing loop or block has this label");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProblemKind {
    /// A syntax error node from the parser (already reported by it; listed so
    /// a host can tell lowering skipped the region).
    SyntaxError,
    /// A node or token kind the mapping does not describe.
    Unmapped,
    /// A construct the mapping marks [`Unsupported`](crate::Arch::Unsupported).
    Unsupported,
    /// A construct in a context it cannot appear in (a `let` used as an
    /// expression, a call used as a type).
    NotAllowedHere {
        /// What was expected there.
        expected: Context,
    },
    /// A required part is missing (half-typed code: `if` with no condition,
    /// `fn` with no name).
    MissingField {
        /// The archetype field.
        field: &'static str,
    },
    /// One child was selected for two fields of a construct (positional
    /// picks alias in half-typed code, such as `while { ... }` with its
    /// condition missing). The first field keeps it; the later one is left
    /// absent.
    AmbiguousChild,
    /// An operator token with no entry in the operator table for its
    /// position.
    UnknownOperator,
    /// A `break` or `continue` whose label no enclosing loop or block has.
    UnknownLabel,
    /// A number or character literal that does not decode (bad digits, too
    /// large for its radix, an empty or multi-character char literal).
    InvalidLiteral,
    /// An escape sequence the literal's escape style does not define.
    InvalidEscape,
    /// A token's span does not lie inside the source text (a host bug).
    BadSpan,
    /// A required hole of a template is absent (the node lacks the part the
    /// template needs, such as a `while` without its condition).
    MissingHole {
        /// The template.
        template: TemplateId,
    },
    /// A template application nested deeper than
    /// [`Budget::max_expansion_depth`](crate::Budget).
    ExpansionTooDeep {
        /// The template that could not be applied.
        template: TemplateId,
    },
    /// A binding that must be function-scoped
    /// ([`LangPolicy::function_scoped_bindings`](crate::LangPolicy), or a
    /// hoisted `let`) but cannot be hoisted; it stays scoped to its construct.
    CannotHoist {
        /// Why.
        reason: HoistBlocker,
    },
    /// An attribute argument that is neither a literal nor a name (HIR
    /// attributes are data); it is left out of the attribute.
    AttributeArgument,
    /// An attribute on a construct that has no HIR node to carry it (a
    /// wrapper, a capture, a match arm); it is dropped.
    MisplacedAttribute,
    /// A problem hir-lang's lenient validator found and repaired.
    Hir(HirError),
    /// A numeric `break N` / `continue N`
    /// ([`BreakLevels::Numeric`](crate::BreakLevels::Numeric)) whose level
    /// is not a positive integer literal (`level` is `0`), or is larger than
    /// the number of loops around it in its function (`loops`). The jump is
    /// lowered as an error expression. PHP: "'break' operator accepts only
    /// positive integers", "Cannot 'break' 2 levels".
    ///
    /// ```
    /// use lower_lang::ProblemKind;
    ///
    /// let p = ProblemKind::BreakLevel { level: 3, loops: 2 };
    /// assert_eq!(p.to_string(), "cannot break or continue 3 levels: only 2 loops enclose it");
    /// ```
    BreakLevel {
        /// The level written (`0`: not a positive integer literal; levels
        /// beyond `u64` saturate).
        level: u64,
        /// The loops around the jump in its function.
        loops: u32,
    },
    /// The implicit receiver
    /// ([`LangPolicy::implicit_receiver`](crate::LangPolicy::implicit_receiver),
    /// PHP's `$this`) read inside a `static` closure, or a closure nested in
    /// one, where PHP has no `$this` ("Using $this when not in object
    /// context"). Lowered as written; the problem keeps the closure from
    /// silently capturing the method's receiver.
    NoReceiver,
}

/// Why a binding could not be hoisted to its function.
///
/// # Examples
///
/// ```
/// use lower_lang::{HoistBlocker, ProblemKind};
///
/// let p = ProblemKind::CannotHoist { reason: HoistBlocker::ByReference };
/// assert_eq!(
///     p.to_string(),
///     "this binding cannot be function-scoped: a by-reference binding would become a copy"
/// );
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum HoistBlocker {
    /// A by-reference binding (`&$v`, `ref x`): hoisting assigns a copy, so
    /// the function-scoped variable would not alias.
    ByReference,
    /// A template uses the pattern hole more than once, so one binding would
    /// be assigned from several places.
    ReusedHole,
    /// The template never binds the pattern hole in a `match` arm or a
    /// `let` (its expansion was cut short), so nothing assigns the binding.
    NoBindingSite,
}

impl fmt::Display for HoistBlocker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ByReference => "a by-reference binding would become a copy",
            Self::ReusedHole => "the template binds this pattern in more than one place",
            Self::NoBindingSite => "the template never binds this pattern",
        })
    }
}

impl fmt::Display for ProblemKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SyntaxError => f.write_str("syntax error"),
            Self::Unmapped => {
                f.write_str("the language's semantics do not describe this construct")
            }
            Self::Unsupported => f.write_str("this construct is not supported yet"),
            Self::NotAllowedHere { expected } => write!(f, "expected {expected} here"),
            Self::MissingField { field } => write!(f, "missing {field}"),
            Self::AmbiguousChild => {
                f.write_str("one part fills two roles here; a part is probably missing")
            }
            Self::UnknownOperator => f.write_str("unknown operator"),
            Self::UnknownLabel => f.write_str("no enclosing loop or block has this label"),
            Self::InvalidLiteral => f.write_str("invalid literal"),
            Self::InvalidEscape => f.write_str("invalid escape sequence"),
            Self::BadSpan => f.write_str("token span lies outside the source"),
            Self::MissingHole { template } => {
                write!(f, "missing a part template {} needs", template.index())
            }
            Self::ExpansionTooDeep { template } => write!(
                f,
                "template {} is nested too deeply to expand",
                template.index()
            ),
            Self::CannotHoist { reason } => {
                write!(f, "this binding cannot be function-scoped: {reason}")
            }
            Self::AttributeArgument => f.write_str(
                "an attribute argument must be a literal or a name; this one is left out",
            ),
            Self::MisplacedAttribute => {
                f.write_str("nothing here can carry an attribute; it is dropped")
            }
            Self::Hir(e) => write!(f, "{e}"),
            Self::BreakLevel { level: 0, .. } => {
                f.write_str("a break or continue level must be a positive integer literal")
            }
            Self::BreakLevel { level, loops: 0 } => write!(
                f,
                "cannot break or continue {level} levels: no loop encloses it in this function"
            ),
            Self::BreakLevel { level, loops: 1 } => write!(
                f,
                "cannot break or continue {level} levels: only 1 loop encloses it"
            ),
            Self::BreakLevel { level, loops } => write!(
                f,
                "cannot break or continue {level} levels: only {loops} loops enclose it"
            ),
            Self::NoReceiver => f.write_str(
                "the receiver is not available here: a static closure (and any closure inside one) has none",
            ),
        }
    }
}

/// One problem, at the source span it is about.
///
/// # Examples
///
/// ```
/// use lower_lang::{Problem, ProblemKind, Span};
///
/// let p = Problem { kind: ProblemKind::UnknownOperator, span: Span::new(4, 5) };
/// assert_eq!(p.to_string(), "4..5: unknown operator");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Problem {
    /// What is wrong.
    pub kind: ProblemKind,
    /// Where, in the syntax tree's source.
    pub span: Span,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}..{}: {}",
            self.span.start().to_u32(),
            self.span.end().to_u32(),
            self.kind
        )
    }
}

/// Why lowering stopped without a result. Problems in the code never stop it;
/// only exhausted resources do.
///
/// # Examples
///
/// ```
/// use lower_lang::LowerError;
///
/// let e = LowerError::NodeBudget { limit: 10 };
/// assert_eq!(e.to_string(), "lowering would build more than 10 HIR nodes");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LowerError {
    /// Lowering would build more HIR nodes than
    /// [`Budget::max_nodes`](crate::Budget). Raise the budget if the input is
    /// trusted and genuinely that large; otherwise reject it.
    NodeBudget {
        /// The budget that was exceeded.
        limit: u64,
    },
    /// The interner ran out of symbols. Use a fresh interner per session.
    SymbolSpaceExhausted,
    /// hir-lang refused the result without a repair (capacity exceeded). Split
    /// the input.
    Hir(HirError),
}

impl fmt::Display for LowerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NodeBudget { limit } => {
                write!(f, "lowering would build more than {limit} HIR nodes")
            }
            Self::SymbolSpaceExhausted => f.write_str("the interner ran out of symbols"),
            Self::Hir(e) => write!(f, "hir-lang refused the result: {e}"),
        }
    }
}

impl core::error::Error for LowerError {}
