//! The mapping: a language's semantics as data.
//!
//! A [`Mapping`] mirrors LexerSketch's LSF2 `[semantics]` sections:
//! node kinds to [`Arch`]etypes (`[semantics.bind]`), templates
//! (`[semantics.lower]`), and operators with their policies
//! (`[semantics.ops]`), plus the token roles, modifiers, and path roots that
//! the archetypes read. Build one with a [`MappingBuilder`]; it is immutable,
//! `Send` and `Sync` when `K` is, and lowers any number of trees.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::fmt;

use hir_lang::{DivZero, FloatToInt, Op, OpKind, Overflow, PathRoot, Prim, Shift, Vis};

use crate::arch::{Arch, TemplateUse};
use crate::pick::{KindSets, Pick};
use crate::template::{HoleKind, NONE, TArg, TNode, Template, TemplateError, TemplateId};
use crate::text::{self, Registry, SyntaxError};

/// What a token means when it is lowered on its own.
///
/// # Examples
///
/// ```
/// use lower_lang::{Escapes, TokenRole};
///
/// let var = TokenRole::Var { sigil: 1 };       // `$name`, name `name`
/// let string = TokenRole::Str(Escapes::Standard);
/// assert_ne!(var, string);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TokenRole {
    /// An identifier: a name, a variable reference, a binding, or a type name.
    Name,
    /// A sigiled variable such as `$x`; the first `sigil` bytes are not part
    /// of the name.
    Var {
        /// Bytes to strip.
        sigil: u8,
    },
    /// A loop label such as `'outer`.
    Label {
        /// Bytes to strip.
        sigil: u8,
    },
    /// A region (lifetime) such as `'a`, as a generic parameter or argument.
    Lifetime {
        /// Bytes to strip.
        sigil: u8,
    },
    /// An integer literal (`0x`, `0o`, `0b` prefixes and `_` separators
    /// accepted; a registered suffix sets its type).
    Int,
    /// A floating-point literal.
    Float,
    /// A number: a float if it has a fraction or an exponent, else an integer.
    Number,
    /// A string literal, quotes included in the token.
    Str(Escapes),
    /// A raw string literal, taken verbatim: optional ASCII prefix letters
    /// (`r`, `R`, `br`), optional `#` fences, then one quote character (`"`,
    /// `'`, or `` ` ``), or a tripled one (`"""`, Python), closed by the same
    /// quote(s) and the same number of `#`s. Rust `r#"a "b""#`, Python
    /// `r'\d+'`. Malformed fences are an
    /// [`InvalidLiteral`](crate::ProblemKind::InvalidLiteral).
    RawStr,
    /// A PHP heredoc (`Standard` escapes) or nowdoc (`None`) as one token:
    /// `<<<ID` (or `<<<"ID"`, `<<<'ID'`), a newline, the body lines, and the
    /// closing identifier on its own line. Per PHP 7.3+, the whitespace before
    /// the closing identifier is removed from every body line (a line
    /// indented less, or tabs mixed with spaces, is an
    /// [`InvalidLiteral`](crate::ProblemKind::InvalidLiteral)), and the newline
    /// before the closing identifier is not part of the string. A heredoc
    /// with interpolation is an [`Interp`](crate::Arch::Interp) node instead.
    Heredoc(Escapes),
    /// A literal piece of an [interpolated string](crate::Arch::Interp): the
    /// text between embedded expressions, without quotes; escapes decoded.
    StrPart(Escapes),
    /// A character literal, quotes included.
    Char(Escapes),
    /// A byte-string literal, quotes (and a leading `b`) included.
    Bytes(Escapes),
    /// `true`.
    True,
    /// `false`.
    False,
    /// `null` / `nil` / `None`-as-keyword.
    Null,
    /// `_`: a wildcard pattern, or an inferred type.
    Wild,
    /// `..` inside a pattern.
    Rest,
    /// A type keyword (`int`, `mixed`, `Self`, `!`).
    Type(TypeName),
}

/// How a string or character literal's escapes are decoded.
///
/// # Examples
///
/// ```
/// use lower_lang::Escapes;
///
/// assert_ne!(Escapes::None, Escapes::Standard);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Escapes {
    /// Raw: the text between the quotes, unchanged.
    None,
    /// Only `\\` and an escaped quote (PHP single-quoted strings).
    Quotes,
    /// `\n \r \t \0 \\ \' \" \$ \xHH \u{H..}`; any other escape is reported
    /// and kept as written.
    Standard,
}

/// A type keyword's meaning.
///
/// # Examples
///
/// ```
/// use hir_lang::Prim;
/// use lower_lang::TypeName;
///
/// assert_eq!(TypeName::Prim(Prim::I64), TypeName::Prim(Prim::I64));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TypeName {
    /// A primitive type.
    Prim(Prim),
    /// The gradual dynamic type (`mixed`, `any`).
    Any,
    /// The type of expressions that never complete (`never`, `!`).
    Never,
    /// `Self`.
    SelfTy,
    /// A type to infer (`_`, `auto`).
    Infer,
}

/// A keyword or symbol that sets a flag on the construct it appears in.
///
/// Modifiers are read from the **direct** significant children of the
/// archetypes that consult them (listed on each [`Arch`] variant), so a token
/// can be a modifier in one construct and something else elsewhere.
///
/// # Examples
///
/// ```
/// use lower_lang::Modifier;
///
/// assert_ne!(Modifier::Public, Modifier::Mut);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Modifier {
    /// `pub`, `public`.
    Public,
    /// `protected`.
    Protected,
    /// `private`.
    Private,
    /// `pub(crate)`, `internal`.
    Package,
    /// `mut`, `var`.
    Mut,
    /// `&` / `ref`: by reference (parameters, captures, bindings, arguments).
    Ref,
    /// `...` / `*` on a parameter: variadic.
    Rest,
    /// `**` on a parameter: collects named arguments.
    RestNamed,
    /// A `*` marker in a parameter list: the parameters after it are named-only.
    NamedOnly,
    /// A `/` marker in a parameter list: the parameters before it are
    /// positional-only.
    PositionalOnly,
    /// `self` as a parameter: the method receiver.
    Receiver,
    /// `async`.
    Async,
    /// `*` on a function (`function*`), `gen`: a generator.
    Generator,
    /// `throws`.
    Throws,
    /// `unsafe`.
    Unsafe,
    /// `static` on a class member.
    Static,
    /// `abstract`.
    Abstract,
    /// `final`.
    Final,
    /// `*` in an import: a glob.
    Glob,
    /// `..=`: an inclusive range pattern.
    Inclusive,
    /// `...` on an argument: spread positional arguments.
    Spread,
    /// `**` on an argument: spread named arguments.
    SpreadNamed,
    /// `union` on a record.
    Union,
}

impl Modifier {
    pub(crate) const fn bit(self) -> u32 {
        1 << (self as u32)
    }
}

/// How a function's effects are decided.
///
/// # Examples
///
/// ```
/// use lower_lang::EffectMode;
///
/// assert_eq!(EffectMode::default(), EffectMode::Declared);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum EffectMode {
    /// Only what modifiers declare (`async`, `throws`, generator markers).
    #[default]
    Declared,
    /// Declared, plus whatever the body does directly (a `yield` makes a
    /// generator, as in PHP and Python; a `throw` makes it throwing).
    Inferred,
    /// Every function, closure, and module body has the effect (any PHP
    /// function may throw).
    Always,
}

/// How a bare identifier in a `match` arm pattern is lowered.
///
/// # Examples
///
/// ```
/// use lower_lang::IdentPatterns;
///
/// assert_eq!(IdentPatterns::default(), IdentPatterns::Ident);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum IdentPatterns {
    /// `Pat::Ident`: the resolver decides between a constant or unit variant
    /// and a new binding (Rust, Kraken).
    #[default]
    Ident,
    /// Always a new binding.
    Bind,
}

/// Which call arguments are passed as places, so a by-reference parameter can
/// alias them.
///
/// # Examples
///
/// ```
/// use lower_lang::PlaceArgs;
///
/// assert_eq!(PlaceArgs::default(), PlaceArgs::Marked);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PlaceArgs {
    /// Only arguments with a `Ref` modifier (`f(&$x)` in old PHP, `ref x` in
    /// C#).
    #[default]
    Marked,
    /// Every argument that is a place (a variable, field, index, ...); the
    /// callee decides at run time whether to alias it (PHP and Mox, where
    /// by-reference is declared on the parameter).
    Places,
}

/// What the label of a `break` or `continue` is
/// ([`LangPolicy::break_levels`]; LSF2 `break_levels`).
///
/// # Examples
///
/// ```
/// use lower_lang::BreakLevels;
///
/// assert_eq!(BreakLevels::default(), BreakLevels::Labels);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum BreakLevels {
    /// A name: `break 'outer` targets the loop or block labelled `'outer`
    /// (Rust, Kraken, JavaScript).
    #[default]
    Labels,
    /// A count: `break 2` / `continue 2` target the second enclosing loop of
    /// the same function (PHP). The level is an integer literal from 1 up
    /// to the number of loops around the jump; `0`, anything that is not an
    /// integer literal, or a level beyond the enclosing loops is a
    /// [`BreakLevel`](crate::ProblemKind::BreakLevel) problem. Lowered to a
    /// labelled HIR `break`/`continue`: the targeted loop gets a hygienic
    /// label (or keeps its own). An [`Arch::Loop`](crate::Arch::Loop) and a
    /// template node whose template has a label hole (`while`, `do_while`,
    /// `c_for`, `for_each`) each count as one loop; closures and functions
    /// start a new count.
    Numeric,
}

/// Where `_` may separate the digits of a number.
///
/// # Examples
///
/// ```
/// use lower_lang::Separators;
///
/// assert_eq!(Separators::default(), Separators::Anywhere);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Separators {
    /// After the first digit, anywhere and repeated (`1_000`, `1__0`, `10_`;
    /// Rust, Kraken).
    #[default]
    Anywhere,
    /// Only between two digits of the number's radix: `1_000` and `0xFF_FF`,
    /// but not `1__0`, `10_`, `0x_FF`, or `1_.5` (PHP 7.4+, JavaScript; Python
    /// also accepts `0x_FF`, which this rejects).
    BetweenDigits,
    /// Never (C, PHP before 7.4).
    None,
}

/// What a leading `0` followed by more digits means (`0777`).
///
/// # Examples
///
/// ```
/// use lower_lang::LegacyOctal;
///
/// assert_eq!(LegacyOctal::default(), LegacyOctal::Decimal);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum LegacyOctal {
    /// Decimal: `0777` is 777 (Rust, Kraken).
    #[default]
    Decimal,
    /// Octal: `0777` is 511, and `08` is invalid (C, PHP, JavaScript sloppy
    /// mode).
    Octal,
    /// An error (Python 3: write `0o777`); `0`, `00`, and `0.5` stay valid.
    Error,
}

/// How number literals are spelled and decoded: the literal decoding table.
///
/// Raw strings, heredocs, nowdocs, and interpolation pieces are token roles
/// ([`TokenRole::RawStr`], [`TokenRole::Heredoc`], [`TokenRole::StrPart`]); an
/// integer too large for 64 bits follows [`LangPolicy::overflow`] (see
/// [`LangPolicy`]).
///
/// `#[non_exhaustive]`: start from [`Literals::default`] and assign fields.
///
/// # Examples
///
/// ```
/// use lower_lang::{LegacyOctal, Literals, Separators};
///
/// let mut php = Literals::default();
/// php.separators = Separators::BetweenDigits;
/// php.legacy_octal = LegacyOctal::Octal;
/// php.hex_floats = false;
/// assert!(Literals::default().hex_floats);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Literals {
    /// Digit separators.
    pub separators: Separators,
    /// A leading zero.
    pub legacy_octal: LegacyOctal,
    /// Whether hexadecimal floating-point literals (`0x1.8p3` = 12.0, `0x1p-2`
    /// = 0.25) are accepted (C, Java, Rust's `hexf`); they are decoded with
    /// correct rounding to the nearest `f64`, subnormals included.
    pub hex_floats: bool,
}

impl Default for Literals {
    fn default() -> Self {
        Self {
            separators: Separators::Anywhere,
            legacy_octal: LegacyOctal::Decimal,
            hex_floats: true,
        }
    }
}

/// Language-wide lowering policies.
///
/// `#[non_exhaustive]`: start from [`LangPolicy::default`] (or
/// [`LangPolicy::php`]) and assign fields.
///
/// An unsuffixed integer literal follows the overflow policy: with
/// [`Overflow::Promote`] (PHP, whose integers are 64-bit signed) a value
/// above `i64::MAX` is the `f64` nearest to it (`9223372036854775808` and
/// `0xFFFF_FFFF_FFFF_FFFF` are floats, as in PHP); otherwise a decimal value
/// beyond `u64` is an exact big integer, and a non-decimal one is converted
/// to its exact decimal digits (up to 1,024 digits; longer is an
/// [`InvalidLiteral`](crate::ProblemKind::InvalidLiteral)).
///
/// # Examples
///
/// ```
/// use hir_lang::Overflow;
/// use lower_lang::{EffectMode, LangPolicy, PlaceArgs};
///
/// // Mox: PHP-like integers, any function may throw, implicit locals.
/// let mut mox = LangPolicy::default();
/// mox.overflow = Overflow::Promote;
/// mox.throws = EffectMode::Always;
/// mox.yields = EffectMode::Inferred;
/// mox.implicit_locals = true;
/// mox.place_args = PlaceArgs::Places;
/// assert!(mox.implicit_locals);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct LangPolicy {
    /// The overflow policy of every op that consults one (OPS §2).
    pub overflow: Overflow,
    /// The division-by-zero policy.
    pub div_zero: DivZero,
    /// The shift-amount policy (`Shift::Saturate` is PHP's rule: an amount
    /// at or above the width gives `0`, or `-1` for `shr` of a negative).
    pub shift: Shift,
    /// The float-to-integer conversion policy.
    pub float_to_int: FloatToInt,
    /// When functions, closures, and module bodies throw.
    pub throws: EffectMode,
    /// When they are generators.
    pub yields: EffectMode,
    /// When they are async.
    pub asyncs: EffectMode,
    /// PHP-style implicit declaration: assigning to a plain variable declares
    /// it for the whole enclosing function (or module body) unless a
    /// parameter, `global`, or `static` already does.
    pub implicit_locals: bool,
    /// Bare identifiers in `match` arm patterns.
    pub ident_patterns: IdentPatterns,
    /// Which arguments are passed as places.
    pub place_args: PlaceArgs,
    /// PHP scoping: the variables a `for-each` binds (every template pattern
    /// hole) and the variable a `catch` binds belong to the enclosing
    /// function (or module body), so they keep their last value after the
    /// loop or handler. Each such pattern binds hygienic temporaries, which
    /// are assigned to the function-scoped names right after the pattern
    /// matches. A binding that cannot be hoisted this way (a by-reference
    /// binding, a hole the template uses twice) is reported
    /// ([`CannotHoist`](crate::ProblemKind::CannotHoist)) and stays scoped.
    pub function_scoped_bindings: bool,
    /// How number literals are spelled.
    pub literals: Literals,
    /// The implicit receiver (LSF2 `implicit_receiver`; PHP's `$this`): a
    /// name stored with [`MappingBuilder::name`]. When set, every method of
    /// a class, mixin, or interface that is not `static` (no
    /// [`Modifier::Static`]) and has no explicit receiver parameter gets a
    /// first parameter of kind `Receiver` binding this name, visible to the
    /// method's body (and to its parameter defaults) as an ordinary user
    /// name; template binders carry their expansion's mark and never
    /// capture it. A closure inside such a method captures it as PHP does:
    /// one that only sees explicit captures (`implicit: None`, PHP's
    /// `function () use (...)`) gets an extra by-value capture of the
    /// receiver; one with implicit captures reaches it through them. A
    /// `static` closure has no receiver, and neither does any closure nested
    /// in one: reading the name there is a
    /// [`NoReceiver`](crate::ProblemKind::NoReceiver) problem. Functions
    /// that are not methods (and `impl` members, which spell their
    /// receiver) are unchanged. A name [`build`](MappingBuilder::build)
    /// cannot find is [`MappingError::UnknownName`].
    pub implicit_receiver: Option<NameRef>,
    /// What a `break`/`continue` label is: a name, or (PHP) a loop count.
    pub break_levels: BreakLevels,
    /// The visibility of a member of a class, mixin, or interface (a
    /// method, a property, a static, a constant) that has no visibility
    /// modifier (LSF2 `default_visibility`). `Private` by default, as in
    /// 0.4.0; PHP and Mox members are `Public`. Declarations outside class
    /// bodies, `impl` members, and members with a visibility keyword are
    /// unchanged.
    pub default_member_visibility: Vis,
}

impl LangPolicy {
    /// PHP and Mox: integers promote to float on overflow (OPS §2), any
    /// function may throw, a `yield` makes a generator, assignment declares
    /// variables for the whole function, every place argument is passed as a
    /// place, `foreach` and `catch` variables are function-scoped, and
    /// numbers use PHP's spelling (`_` between digits, `0777` octal, no hex
    /// floats).
    ///
    /// The preset is the 0.4.0 one: it leaves
    /// [`implicit_receiver`](Self::implicit_receiver) unset (the name comes
    /// from a [`MappingBuilder`]), [`break_levels`](Self::break_levels) at
    /// `Labels`, and
    /// [`default_member_visibility`](Self::default_member_visibility) at
    /// `Private`. PHP and Mox set all three:
    ///
    /// ```
    /// use hir_lang::Vis;
    /// use lower_lang::{BreakLevels, LangPolicy, MappingBuilder};
    ///
    /// let mut b = MappingBuilder::<u8>::new();
    /// let mut mox = LangPolicy::php();
    /// mox.implicit_receiver = Some(b.name("this"));
    /// mox.break_levels = BreakLevels::Numeric;
    /// mox.default_member_visibility = Vis::Public;
    /// b.policy(mox);
    /// assert!(b.build().is_ok());
    /// ```
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::Overflow;
    /// use lower_lang::{LangPolicy, LegacyOctal, PlaceArgs};
    ///
    /// let php = LangPolicy::php();
    /// assert_eq!(php.overflow, Overflow::Promote);
    /// assert!(php.implicit_locals && php.function_scoped_bindings);
    /// assert_eq!(php.place_args, PlaceArgs::Places);
    /// assert_eq!(php.literals.legacy_octal, LegacyOctal::Octal);
    /// assert!(!LangPolicy::default().function_scoped_bindings);
    /// ```
    #[must_use]
    pub fn php() -> Self {
        Self {
            overflow: Overflow::Promote,
            throws: EffectMode::Always,
            yields: EffectMode::Inferred,
            implicit_locals: true,
            place_args: PlaceArgs::Places,
            function_scoped_bindings: true,
            literals: Literals {
                separators: Separators::BetweenDigits,
                legacy_octal: LegacyOctal::Octal,
                hex_floats: false,
            },
            ..Self::default()
        }
    }
}

impl Default for LangPolicy {
    fn default() -> Self {
        Self {
            overflow: Overflow::Error,
            div_zero: DivZero::Error,
            shift: Shift::Error,
            float_to_int: FloatToInt::Error,
            throws: EffectMode::Declared,
            yields: EffectMode::Declared,
            asyncs: EffectMode::Declared,
            implicit_locals: false,
            ident_patterns: IdentPatterns::Ident,
            place_args: PlaceArgs::Marked,
            function_scoped_bindings: false,
            literals: Literals::default(),
            implicit_receiver: None,
            break_levels: BreakLevels::Labels,
            default_member_visibility: Vis::Private,
        }
    }
}

/// A host or intrinsic name stored in a mapping, from
/// [`MappingBuilder::name`].
///
/// # Examples
///
/// ```
/// let mut b = lower_lang::MappingBuilder::<u8>::new();
/// let concat = b.name("mox.concat");
/// assert_ne!(concat, b.name("mox.echo"));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NameRef(pub(crate) u32);

/// How the pieces of an [interpolated string](crate::Arch::Interp) become one
/// string.
///
/// # Examples
///
/// ```
/// use lower_lang::{Join, MappingBuilder};
///
/// let mut b = MappingBuilder::<u8>::new();
/// let concat = b.name("mox.concat");
/// // "a{$x}b" → mox.concat(mox.concat("a", x), "b")
/// let fold = Join::Fold(concat);
/// assert_ne!(fold, Join::Call(concat));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Join {
    /// A host function of two arguments, folded left:
    /// `concat(concat(p0, p1), p2)` (PHP's `.`).
    Fold(NameRef),
    /// One call of a host function with every piece as an argument:
    /// `interp(p0, p1, p2)` (one LSB call, no intermediate strings).
    Call(NameRef),
    /// A template: with exactly two expression holes it is folded left like
    /// `Fold`; with exactly one argument-list hole it receives every piece at
    /// once. Any other hole shape is refused by
    /// [`build`](MappingBuilder::build)
    /// ([`MappingError::BadJoin`]).
    Template(TemplateId),
}

/// What an operator token lowers to.
///
/// # Examples
///
/// ```
/// use hir_lang::{Op, OpKind, Overflow};
/// use lower_lang::OpSpec;
///
/// let plus = OpSpec::Op(OpKind::Add);                       // language policy
/// let wrapping = OpSpec::OpWith(Op::new(OpKind::Add).with_overflow(Overflow::Wrap));
/// let plus_eq = OpSpec::Compound(OpKind::Add);              // `+=`
/// assert_ne!(plus, wrapping);
/// assert_ne!(plus, plus_eq);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum OpSpec {
    /// An intrinsic op with the language's policies ([`LangPolicy`]).
    Op(OpKind),
    /// An intrinsic op with exactly this policy (`[semantics.ops] "/" = { op =
    /// "div", zero = "trap" }`).
    OpWith(Op),
    /// Compound assignment (`+=`) with the language's policies.
    Compound(OpKind),
    /// Compound assignment with exactly this policy.
    CompoundWith(Op),
    /// Plain assignment (`=` as an operator).
    Assign,
    /// Reference assignment (`=&`).
    RefAssign,
    /// The operand itself (unary `+`).
    Identity,
    /// `&x` / `&mut x` as a prefix operator.
    Borrow(hir_lang::BorrowKind),
    /// `*x` as a prefix operator.
    Deref,
    /// `await x` (prefix) or `x.await` (postfix).
    Await,
    /// A template whose expression holes receive the operands, in order
    /// (`&&`, `||`, `??`, `x++`).
    Template(TemplateId),
    /// A call of a host function with the operands as arguments (Mox `.`
    /// string concatenation).
    Extern(NameRef),
    /// A named backend intrinsic applied to the operands.
    Intrinsic(NameRef),
    /// Compound assignment through a host function (`.=` as `mox.concat`):
    /// `let_place p = target in p = f(p, value)`, so the place's operands are
    /// evaluated once (HIR `LetPlace`).
    CompoundExtern(NameRef),
    /// Compound assignment through a named intrinsic (`**=` as `mox.pow`),
    /// lowered like [`CompoundExtern`](Self::CompoundExtern).
    CompoundIntrinsic(NameRef),
    /// Compound assignment through a template with exactly two expression
    /// holes, which receive the place alias and the value (`??=` through
    /// [`coalesce`](crate::templates::coalesce)): `let_place p = target in p =
    /// template(p, value)`.
    CompoundTemplate(TemplateId),
}

impl OpSpec {
    /// Whether this is a compound assignment through a non-OPS operator
    /// (lowered through `let_place`).
    pub(crate) const fn is_place_compound(self) -> bool {
        matches!(
            self,
            Self::CompoundExtern(_) | Self::CompoundIntrinsic(_) | Self::CompoundTemplate(_)
        )
    }
}

/// Ops a compound assignment may use (HIR §8.2; `pow` since hir-lang 0.4).
pub(crate) const fn compound_ok(kind: OpKind) -> bool {
    matches!(
        kind,
        OpKind::Add
            | OpKind::Sub
            | OpKind::Mul
            | OpKind::Div
            | OpKind::FloorDiv
            | OpKind::Rem
            | OpKind::FloorMod
            | OpKind::And
            | OpKind::Or
            | OpKind::Xor
            | OpKind::Shl
            | OpKind::Shr
            | OpKind::Pow
    )
}

/// Operator positions, for the tables and errors.
///
/// # Examples
///
/// ```
/// use lower_lang::Fixity;
///
/// assert_eq!(Fixity::Binary.arity(), 2);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Fixity {
    /// Before the operand.
    Prefix,
    /// Between two operands.
    Binary,
    /// After the operand.
    Postfix,
}

impl Fixity {
    /// The number of operands.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(lower_lang::Fixity::Prefix.arity(), 1);
    /// ```
    #[must_use]
    pub const fn arity(self) -> usize {
        match self {
            Self::Binary => 2,
            Self::Prefix | Self::Postfix => 1,
        }
    }
}

/// Why a mapping was refused.
///
/// # Examples
///
/// ```
/// use lower_lang::{Arch, MappingBuilder, MappingError, Pick};
///
/// let mut b = MappingBuilder::<u8>::new();
/// b.node(1, Arch::If { cond: Pick::Absent, then: Pick::Nth(1), else_: Pick::Absent });
/// assert_eq!(b.build().unwrap_err(), MappingError::MissingField { rule: 0, field: "cond" });
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum MappingError {
    /// Rule `rule` (in registration order) leaves a required field `Absent`.
    MissingField {
        /// The rule's registration index.
        rule: usize,
        /// The field.
        field: &'static str,
    },
    /// A kind has two entries in a table that allows one (`table` names it:
    /// "token role", "modifier", "root", "prefix operator", ...).
    DuplicateKind {
        /// The table.
        table: &'static str,
        /// The later entry's registration index in that table.
        index: usize,
    },
    /// A type name is registered twice.
    DuplicateTypeName {
        /// The later entry's registration index.
        index: usize,
    },
    /// A template id that this mapping did not issue (or, inside a template,
    /// one not registered before it).
    UnknownTemplate {
        /// The id's index.
        template: usize,
    },
    /// A node binds a template with the wrong number of picks.
    HoleCount {
        /// The rule's registration index.
        rule: usize,
        /// The template's hole count.
        expected: usize,
        /// The picks given.
        found: usize,
    },
    /// An `apply` inside template `template` passes arguments that do not
    /// match the applied template's holes (count, or kind, or `Absent` for a
    /// required hole).
    ApplyArgs {
        /// The registering template's index.
        template: usize,
    },
    /// An operator maps to a spec that cannot apply at its position: an op of
    /// the wrong arity, a compound op HIR does not allow, a policy that does
    /// not fit its op, assignment outside binary position, identity outside
    /// unary position, or a template whose holes are not one expression per
    /// operand.
    BadOperator {
        /// The operator's position.
        fixity: Fixity,
        /// Its registration index in that table.
        index: usize,
    },
    /// A table outgrew its index type.
    TooLarge,
    /// Rule `rule` joins an interpolated string with a template whose holes
    /// are neither two expressions nor one argument list.
    BadJoin {
        /// The rule's registration index.
        rule: usize,
    },
    /// A text-keyed operator entry has an empty text.
    EmptyOperatorText {
        /// The operator's position.
        fixity: Fixity,
        /// Its registration index in the text table.
        index: usize,
    },
    /// A per-archetype modifier entry
    /// ([`modifier_in`](MappingBuilder::modifier_in)) names no archetype
    /// ([`Arch::lsf_name`] lists the names).
    UnknownArchetype {
        /// Its registration index among the per-archetype modifiers.
        index: usize,
    },
    /// [`LangPolicy::implicit_receiver`] is a [`NameRef`] this builder did
    /// not issue.
    UnknownName {
        /// The name's index.
        index: usize,
    },
}

impl fmt::Display for MappingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingField { rule, field } => {
                write!(f, "rule {rule}: required field `{field}` has no pick")
            }
            Self::DuplicateKind { table, index } => {
                write!(f, "{table} {index}: the kind already has an entry")
            }
            Self::DuplicateTypeName { index } => write!(f, "type name {index} is registered twice"),
            Self::UnknownTemplate { template } => {
                write!(f, "template {template} is not registered")
            }
            Self::HoleCount {
                rule,
                expected,
                found,
            } => write!(
                f,
                "rule {rule}: the template has {expected} holes, {found} picks given"
            ),
            Self::ApplyArgs { template } => {
                write!(
                    f,
                    "template {template}: an `apply` does not match the applied template's holes"
                )
            }
            Self::BadOperator { fixity, index } => {
                write!(
                    f,
                    "{fixity:?} operator {index} maps to a spec that cannot apply there"
                )
            }
            Self::TooLarge => f.write_str("a mapping table outgrew its index type"),
            Self::BadJoin { rule } => write!(
                f,
                "rule {rule}: an interpolation template needs two expression holes or one argument-list hole"
            ),
            Self::EmptyOperatorText { fixity, index } => {
                write!(f, "{fixity:?} operator text {index} is empty")
            }
            Self::UnknownArchetype { index } => write!(
                f,
                "per-archetype modifier {index} names no archetype (expected an LSF2 `arch` name such as `param` or `arg`)"
            ),
            Self::UnknownName { index } => write!(
                f,
                "the implicit receiver is name {index}, which this mapping did not store"
            ),
        }
    }
}

impl core::error::Error for MappingError {}

/// One node rule: an archetype, applied when `when` finds something (or
/// always, when `when` is `Absent`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Rule<K> {
    pub(crate) when: Pick<K>,
    pub(crate) arch: Arch<K>,
}

/// A language's semantics as data: what each node kind means, what each token
/// is, which operators map to what, and the templates for sugar.
///
/// # Examples
///
/// ```
/// use lower_lang::{Arch, MappingBuilder, Pick, TokenRole};
///
/// const IDENT: u8 = 1;
/// const LET: u8 = 2;
/// let mut b = MappingBuilder::new();
/// b.token(IDENT, TokenRole::Name);
/// b.node(LET, Arch::Let {
///     pat: Pick::Kind(IDENT),
///     ty: Pick::Absent,
///     init: Pick::Back(0),
///     else_: Pick::Absent,
///     mutable: false,
///     hoist: false,
/// });
/// let mapping = b.build()?;
/// assert_eq!(mapping.role_for(&IDENT), Some(TokenRole::Name));
/// assert!(mapping.rule_for(&LET).is_some());
/// # Ok::<(), lower_lang::MappingError>(())
/// ```
#[derive(Clone, Debug)]
pub struct Mapping<K> {
    pub(crate) rules: Vec<Rule<K>>,
    pub(crate) by_kind: Vec<(K, u32)>,
    pub(crate) roles: Vec<(K, TokenRole)>,
    pub(crate) modifiers: Vec<(K, Modifier)>,
    /// Modifiers that depend on the archetype they appear in (`...` is a
    /// rest parameter in `param` and a spread in `arg`), sorted by kind;
    /// they win over `modifiers` inside their archetype.
    pub(crate) arch_modifiers: Vec<(K, &'static str, Modifier)>,
    pub(crate) roots: Vec<(K, PathRoot)>,
    pub(crate) containers: Vec<K>,
    pub(crate) skips: Vec<K>,
    pub(crate) errors: Vec<K>,
    pub(crate) type_names: Vec<(Box<str>, TypeName)>,
    pub(crate) suffixes: Vec<(Box<str>, Prim)>,
    pub(crate) prefix: Vec<(K, OpSpec)>,
    pub(crate) binary: Vec<(K, OpSpec)>,
    pub(crate) postfix: Vec<(K, OpSpec)>,
    /// Operator tables keyed by the token's text, consulted when the kind
    /// table has no entry (lexers with one generic operator kind).
    pub(crate) prefix_text: Vec<(Box<str>, OpSpec)>,
    pub(crate) binary_text: Vec<(Box<str>, OpSpec)>,
    pub(crate) postfix_text: Vec<(Box<str>, OpSpec)>,
    /// Kinds with an `Attr` rule, sorted: invisible to other picks.
    pub(crate) attr_kinds: Vec<K>,
    pub(crate) templates: Vec<Template>,
    pub(crate) template_names: Vec<u32>,
    pub(crate) hole_picks: Vec<Pick<K>>,
    pub(crate) strings: Vec<Box<str>>,
    pub(crate) policy: LangPolicy,
}

fn find<'a, K: Ord, V>(table: &'a [(K, V)], kind: &K) -> Option<&'a V> {
    table
        .binary_search_by(|(k, _)| k.cmp(kind))
        .ok()
        .and_then(|i| table.get(i))
        .map(|(_, v)| v)
}

impl<K: Copy + Ord> Mapping<K> {
    /// The archetype of the first rule registered for `kind`.
    ///
    /// # Examples
    ///
    /// ```
    /// use lower_lang::{Arch, MappingBuilder};
    ///
    /// let mut b = MappingBuilder::<u8>::new();
    /// b.node(3, Arch::binary());
    /// let m = b.build()?;
    /// assert_eq!(m.rule_for(&3), Some(&Arch::binary()));
    /// assert_eq!(m.rule_for(&4), None);
    /// # Ok::<(), lower_lang::MappingError>(())
    /// ```
    #[must_use]
    pub fn rule_for(&self, kind: &K) -> Option<&Arch<K>> {
        let (start, _) = self.rule_range(kind);
        self.by_kind
            .get(start)
            .filter(|(k, _)| k == kind)
            .and_then(|(_, r)| self.rules.get(*r as usize))
            .map(|r| &r.arch)
    }

    /// The role of token kind `kind`.
    ///
    /// # Examples
    ///
    /// See [`Mapping`].
    #[must_use]
    pub fn role_for(&self, kind: &K) -> Option<TokenRole> {
        find(&self.roles, kind).copied()
    }

    /// The modifier token kind `kind` stands for.
    ///
    /// # Examples
    ///
    /// ```
    /// use lower_lang::{MappingBuilder, Modifier};
    ///
    /// let mut b = MappingBuilder::<u8>::new();
    /// b.modifier(9, Modifier::Mut);
    /// assert_eq!(b.build()?.modifier_for(&9), Some(Modifier::Mut));
    /// # Ok::<(), lower_lang::MappingError>(())
    /// ```
    #[must_use]
    pub fn modifier_for(&self, kind: &K) -> Option<Modifier> {
        find(&self.modifiers, kind).copied()
    }

    /// The modifier token kind `kind` stands for inside archetype
    /// `archetype` (its [`Arch::lsf_name`]): the
    /// [per-archetype](MappingBuilder::modifier_in) entry, else the plain
    /// one ([`modifier_for`](Self::modifier_for)).
    ///
    /// # Examples
    ///
    /// ```
    /// use lower_lang::{MappingBuilder, Modifier};
    ///
    /// const DOTS: u8 = 7;
    /// let mut b = MappingBuilder::<u8>::new();
    /// b.modifier_in(DOTS, "param", Modifier::Rest)
    ///     .modifier_in(DOTS, "arg", Modifier::Spread);
    /// let m = b.build()?;
    /// assert_eq!(m.modifier_in(&DOTS, "param"), Some(Modifier::Rest));
    /// assert_eq!(m.modifier_in(&DOTS, "arg"), Some(Modifier::Spread));
    /// assert_eq!(m.modifier_in(&DOTS, "map_entry"), None);
    /// assert_eq!(m.modifier_for(&DOTS), None);
    /// # Ok::<(), lower_lang::MappingError>(())
    /// ```
    #[must_use]
    pub fn modifier_in(&self, kind: &K, archetype: &str) -> Option<Modifier> {
        self.arch_modifier(kind, archetype)
            .or_else(|| self.modifier_for(kind))
    }

    /// The per-archetype entry for `kind` in `archetype`, if any.
    fn arch_modifier(&self, kind: &K, archetype: &str) -> Option<Modifier> {
        if self.arch_modifiers.is_empty() {
            return None;
        }
        let start = self.arch_modifiers.partition_point(|(k, _, _)| k < kind);
        self.arch_modifiers
            .get(start..)
            .unwrap_or(&[])
            .iter()
            .take_while(|(k, _, _)| k == kind)
            .find(|(_, a, _)| *a == archetype)
            .map(|(_, _, m)| *m)
    }

    /// The registered template `id`.
    ///
    /// # Examples
    ///
    /// ```
    /// use lower_lang::{templates, MappingBuilder};
    ///
    /// let mut b = MappingBuilder::<u8>::new();
    /// let id = b.template(templates::coalesce())?;
    /// let m = b.build()?;
    /// assert_eq!(m.template(id).map(|t| t.name()), Some("??"));
    /// # Ok::<(), lower_lang::MappingError>(())
    /// ```
    #[must_use]
    pub fn template(&self, id: TemplateId) -> Option<&Template> {
        self.templates.get(id.index())
    }

    /// The language-wide policies.
    ///
    /// # Examples
    ///
    /// ```
    /// use lower_lang::{LangPolicy, MappingBuilder};
    ///
    /// let m = MappingBuilder::<u8>::new().build()?;
    /// assert_eq!(*m.policy(), LangPolicy::default());
    /// # Ok::<(), lower_lang::MappingError>(())
    /// ```
    #[must_use]
    pub const fn policy(&self) -> &LangPolicy {
        &self.policy
    }

    /// The rules registered for `kind`, as an index range into `by_kind`.
    pub(crate) fn rule_range(&self, kind: &K) -> (usize, usize) {
        let start = self.by_kind.partition_point(|(k, _)| k < kind);
        let end = start
            + self
                .by_kind
                .get(start..)
                .map_or(0, |rest| rest.partition_point(|(k, _)| k == kind));
        (start, end)
    }

    /// The operator spec of a token: by kind first, then by its text.
    pub(crate) fn op(&self, fixity: Fixity, kind: &K, text: &str) -> Option<OpSpec> {
        let (table, texts) = match fixity {
            Fixity::Prefix => (&self.prefix, &self.prefix_text),
            Fixity::Binary => (&self.binary, &self.binary_text),
            Fixity::Postfix => (&self.postfix, &self.postfix_text),
        };
        find(table, kind).copied().or_else(|| {
            if texts.is_empty() {
                return None;
            }
            texts
                .binary_search_by(|(t, _)| (**t).cmp(text))
                .ok()
                .and_then(|i| texts.get(i))
                .map(|(_, s)| *s)
        })
    }

    /// Whether the operator tables are keyed by text at all (so a lookup
    /// needs the token's text).
    pub(crate) fn has_op_text(&self) -> bool {
        !(self.prefix_text.is_empty()
            && self.binary_text.is_empty()
            && self.postfix_text.is_empty())
    }

    pub(crate) fn root_for(&self, kind: &K) -> Option<PathRoot> {
        find(&self.roots, kind).copied()
    }

    pub(crate) fn is_error(&self, kind: &K) -> bool {
        self.errors.binary_search(kind).is_ok()
    }

    pub(crate) fn type_name(&self, text: &str) -> Option<TypeName> {
        self.type_names
            .binary_search_by(|(t, _)| (**t).cmp(text))
            .ok()
            .and_then(|i| self.type_names.get(i))
            .map(|(_, n)| *n)
    }
}

impl<K: Ord> KindSets<K> for Mapping<K> {
    fn is_skip(&self, kind: &K) -> bool {
        self.skips.binary_search(kind).is_ok()
    }
    fn is_container(&self, kind: &K) -> bool {
        self.containers.binary_search(kind).is_ok()
    }
    fn is_attr(&self, kind: &K) -> bool {
        !self.attr_kinds.is_empty() && self.attr_kinds.binary_search(kind).is_ok()
    }
}

/// Builds a [`Mapping`]. Every method records; [`build`](Self::build) checks
/// everything and sorts the tables.
///
/// # Examples
///
/// A calculator's semantics: statements, `let`, numbers, names, and `+ - * /`.
///
/// ```
/// use hir_lang::OpKind;
/// use lower_lang::{Arch, MappingBuilder, OpSpec, Pick, TokenRole};
///
/// #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
/// enum K { Program, Let, Binary, Ident, Number, Plus, Star, Eq, Semi }
///
/// let mut b = MappingBuilder::new();
/// b.node(K::Program, Arch::Module { name: Pick::Absent, items: Pick::Nodes });
/// b.node(K::Let, Arch::Let {
///     pat: Pick::Kind(K::Ident), ty: Pick::Absent, init: Pick::After(K::Eq),
///     else_: Pick::Absent, mutable: false, hoist: false,
/// });
/// b.node(K::Binary, Arch::binary());
/// b.token(K::Ident, TokenRole::Name);
/// b.token(K::Number, TokenRole::Number);
/// b.binary_op(K::Plus, OpSpec::Op(OpKind::Add));
/// b.binary_op(K::Star, OpSpec::Op(OpKind::Mul));
/// b.skip(K::Semi);
/// let mapping = b.build()?;
/// # let _ = mapping;
/// # Ok::<(), lower_lang::MappingError>(())
/// ```
#[derive(Clone, Debug)]
pub struct MappingBuilder<K> {
    m: Mapping<K>,
    rule_picks_error: Option<MappingError>,
    /// Registered templates by name (the latest registration of a name), for
    /// `apply:<name>` in template texts.
    template_index: BTreeMap<Box<str>, u32>,
    /// Per-archetype modifiers as registered (archetype names checked by
    /// `build`).
    arch_modifiers: Vec<(K, Box<str>, Modifier)>,
}

impl<K: Copy + Ord> Default for MappingBuilder<K> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Copy + Ord> MappingBuilder<K> {
    /// An empty builder with the default [`LangPolicy`].
    ///
    /// # Examples
    ///
    /// ```
    /// let m = lower_lang::MappingBuilder::<u8>::new().build()?;
    /// assert!(m.rule_for(&0).is_none());
    /// # Ok::<(), lower_lang::MappingError>(())
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self {
            m: Mapping {
                rules: Vec::new(),
                by_kind: Vec::new(),
                roles: Vec::new(),
                modifiers: Vec::new(),
                arch_modifiers: Vec::new(),
                roots: Vec::new(),
                containers: Vec::new(),
                skips: Vec::new(),
                errors: Vec::new(),
                type_names: Vec::new(),
                suffixes: Vec::new(),
                prefix: Vec::new(),
                binary: Vec::new(),
                postfix: Vec::new(),
                prefix_text: Vec::new(),
                binary_text: Vec::new(),
                postfix_text: Vec::new(),
                attr_kinds: Vec::new(),
                templates: Vec::new(),
                template_names: Vec::new(),
                hole_picks: Vec::new(),
                strings: Vec::new(),
                policy: LangPolicy::default(),
            },
            rule_picks_error: None,
            template_index: BTreeMap::new(),
            arch_modifiers: Vec::new(),
        }
    }

    fn add_rule(&mut self, kind: K, when: Pick<K>, arch: Arch<K>) -> &mut Self {
        match u32::try_from(self.m.rules.len()) {
            Ok(i) if i < NONE => {
                self.m.rules.push(Rule { when, arch });
                self.m.by_kind.push((kind, i));
            }
            _ => self.rule_picks_error = Some(MappingError::TooLarge),
        }
        self
    }

    /// Nodes of `kind` mean `arch`.
    ///
    /// # Examples
    ///
    /// See [`MappingBuilder`].
    pub fn node(&mut self, kind: K, arch: Arch<K>) -> &mut Self {
        self.add_rule(kind, Pick::Absent, arch)
    }

    /// Nodes of `kind` mean `arch` when `when` finds a child; otherwise the
    /// next rule for `kind` (in registration order) is tried. For grammars
    /// that put alternatives in one rule (`stmt = "'let' ... | expr ';'"`).
    /// A node no rule applies to is reported as unmapped.
    ///
    /// # Examples
    ///
    /// ```
    /// use lower_lang::{Arch, MappingBuilder, Pick};
    ///
    /// const STMT: u8 = 1;
    /// const LET: u8 = 2;
    /// let mut b = MappingBuilder::new();
    /// b.node_when(STMT, Pick::First(LET), Arch::Let {
    ///     pat: Pick::Nth(1), ty: Pick::Absent, init: Pick::Nth(3),
    ///     else_: Pick::Absent, mutable: false, hoist: false,
    /// });
    /// b.node(STMT, Arch::ExprStmt { expr: Pick::Nth(0) });
    /// assert!(b.build().is_ok());
    /// ```
    pub fn node_when(&mut self, kind: K, when: Pick<K>, arch: Arch<K>) -> &mut Self {
        self.add_rule(kind, when, arch)
    }

    /// Tokens of `kind` mean `role`.
    ///
    /// # Examples
    ///
    /// See [`Mapping`].
    pub fn token(&mut self, kind: K, role: TokenRole) -> &mut Self {
        self.m.roles.push((kind, role));
        self
    }

    /// Tokens of `kind` set `modifier` on the construct they appear in.
    ///
    /// # Examples
    ///
    /// See [`Mapping::modifier_for`].
    pub fn modifier(&mut self, kind: K, modifier: Modifier) -> &mut Self {
        self.m.modifiers.push((kind, modifier));
        self
    }

    /// Tokens of `kind` set `modifier` on constructs of archetype
    /// `archetype` (its LSF2 name, [`Arch::lsf_name`]), overriding the
    /// plain [`modifier`](Self::modifier) entry there: LSF2's per-archetype
    /// value `"..." = { param = "rest", arg = "spread" }`, for a token whose
    /// meaning depends on the construct (PHP's `...` is a rest parameter in a
    /// parameter and a spread in an argument). Elsewhere the token sets the
    /// plain entry, or nothing.
    ///
    /// `Spread` on a [`MapEntry`](Arch::MapEntry) (`[...$a, 1]`) is reported
    /// as [`Unsupported`](crate::ProblemKind::Unsupported): HIR's map and
    /// array literals have no spread entry. Parameter-list markers
    /// (`NamedOnly`, `PositionalOnly` tokens standing alone) are read from
    /// the plain table only.
    ///
    /// # Errors
    ///
    /// [`build`](Self::build) refuses an unknown archetype name
    /// ([`MappingError::UnknownArchetype`]) and a kind given two entries for
    /// one archetype ([`MappingError::DuplicateKind`], table `"archetype
    /// modifier"`).
    ///
    /// # Examples
    ///
    /// See [`Mapping::modifier_in`].
    pub fn modifier_in(&mut self, kind: K, archetype: &str, modifier: Modifier) -> &mut Self {
        self.arch_modifiers.push((kind, archetype.into(), modifier));
        self
    }

    /// Leading tokens of `kind` in a [`Path`](Arch::Path) set its root
    /// (`self` → `SelfModule`, `super` → `Super(1)` each, `Self` → `SelfType`,
    /// PHP `parent`/`static` → `ParentType`/`StaticType`, `::` → `Global`).
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::PathRoot;
    ///
    /// let mut b = lower_lang::MappingBuilder::<u8>::new();
    /// b.root(5, PathRoot::Super(1));
    /// assert!(b.build().is_ok());
    /// ```
    pub fn root(&mut self, kind: K, root: PathRoot) -> &mut Self {
        self.m.roots.push((kind, root));
        self
    }

    /// Nodes of `kind` are containers: list picks splice their children in
    /// place (an `args` node inside a `call`, a `params` node inside a `fn`).
    ///
    /// # Examples
    ///
    /// ```
    /// let mut b = lower_lang::MappingBuilder::<u8>::new();
    /// b.container(7);
    /// assert!(b.build().is_ok());
    /// ```
    pub fn container(&mut self, kind: K) -> &mut Self {
        self.m.containers.push(kind);
        self
    }

    /// Tokens (or nodes) of `kind` are punctuation that list picks leave out
    /// (`,`, `;`, `(`).
    ///
    /// # Examples
    ///
    /// ```
    /// let mut b = lower_lang::MappingBuilder::<u8>::new();
    /// b.skip(8);
    /// assert!(b.build().is_ok());
    /// ```
    pub fn skip(&mut self, kind: K) -> &mut Self {
        self.m.skips.push(kind);
        self
    }

    /// Nodes of `kind` are syntax errors (lang-forge's `ERROR`): they lower to
    /// error forms with a [`SyntaxError`](crate::ProblemKind::SyntaxError)
    /// problem, whatever context they appear in.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut b = lower_lang::MappingBuilder::<u8>::new();
    /// b.error(0);
    /// assert!(b.build().is_ok());
    /// ```
    pub fn error(&mut self, kind: K) -> &mut Self {
        self.m.errors.push(kind);
        self
    }

    /// An identifier spelled `text` names this type (`int` → `i64`, `mixed`
    /// → any).
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::Prim;
    /// use lower_lang::TypeName;
    ///
    /// let mut b = lower_lang::MappingBuilder::<u8>::new();
    /// b.type_name("int", TypeName::Prim(Prim::I64));
    /// assert!(b.build().is_ok());
    /// ```
    pub fn type_name(&mut self, text: &str, name: TypeName) -> &mut Self {
        self.m.type_names.push((text.into(), name));
        self
    }

    /// A numeric literal ending in `suffix` has that type (`10u8`).
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::Prim;
    ///
    /// let mut b = lower_lang::MappingBuilder::<u8>::new();
    /// b.suffix("u8", Prim::U8);
    /// assert!(b.build().is_ok());
    /// ```
    pub fn suffix(&mut self, suffix: &str, prim: Prim) -> &mut Self {
        self.m.suffixes.push((suffix.into(), prim));
        self
    }

    /// A prefix operator.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::OpKind;
    /// use lower_lang::OpSpec;
    ///
    /// let mut b = lower_lang::MappingBuilder::<u8>::new();
    /// b.prefix_op(1, OpSpec::Op(OpKind::Neg));
    /// assert!(b.build().is_ok());
    /// ```
    pub fn prefix_op(&mut self, kind: K, spec: OpSpec) -> &mut Self {
        self.m.prefix.push((kind, spec));
        self
    }

    /// A binary operator.
    ///
    /// # Examples
    ///
    /// See [`MappingBuilder`].
    pub fn binary_op(&mut self, kind: K, spec: OpSpec) -> &mut Self {
        self.m.binary.push((kind, spec));
        self
    }

    /// A postfix operator.
    ///
    /// # Examples
    ///
    /// ```
    /// use lower_lang::{templates, OpSpec};
    ///
    /// let mut b = lower_lang::MappingBuilder::<u8>::new();
    /// let inc = b.template(templates::post_increment())?;
    /// b.postfix_op(1, OpSpec::Template(inc));
    /// assert!(b.build().is_ok());
    /// # Ok::<(), lower_lang::MappingError>(())
    /// ```
    pub fn postfix_op(&mut self, kind: K, spec: OpSpec) -> &mut Self {
        self.m.postfix.push((kind, spec));
        self
    }

    /// A prefix operator found by its token's **text** (`"-"`, `"!"`), for a
    /// lexer that gives every operator one generic kind. A kind entry for the
    /// token's kind ([`prefix_op`](Self::prefix_op)) wins over a text entry.
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::OpKind;
    /// use lower_lang::OpSpec;
    ///
    /// let mut b = lower_lang::MappingBuilder::<u8>::new();
    /// b.prefix_op_text("-", OpSpec::Op(OpKind::Neg));
    /// assert!(b.build().is_ok());
    /// ```
    pub fn prefix_op_text(&mut self, text: &str, spec: OpSpec) -> &mut Self {
        self.m.prefix_text.push((text.into(), spec));
        self
    }

    /// A binary operator found by its token's text (`"+"`, `"."`, `"+="`).
    ///
    /// # Examples
    ///
    /// ```
    /// use hir_lang::OpKind;
    /// use lower_lang::OpSpec;
    ///
    /// let mut b = lower_lang::MappingBuilder::<u8>::new();
    /// b.binary_op_text("+", OpSpec::Op(OpKind::Add))
    ///     .binary_op_text("+=", OpSpec::Compound(OpKind::Add));
    /// assert!(b.build().is_ok());
    /// ```
    pub fn binary_op_text(&mut self, text: &str, spec: OpSpec) -> &mut Self {
        self.m.binary_text.push((text.into(), spec));
        self
    }

    /// A postfix operator found by its token's text (`"++"`).
    ///
    /// # Examples
    ///
    /// ```
    /// use lower_lang::{templates, OpSpec};
    ///
    /// let mut b = lower_lang::MappingBuilder::<u8>::new();
    /// let inc = b.template(templates::post_increment())?;
    /// b.postfix_op_text("++", OpSpec::Template(inc));
    /// assert!(b.build().is_ok());
    /// # Ok::<(), lower_lang::MappingError>(())
    /// ```
    pub fn postfix_op_text(&mut self, text: &str, spec: OpSpec) -> &mut Self {
        self.m.postfix_text.push((text.into(), spec));
        self
    }

    /// Stores a host or intrinsic name for [`OpSpec::Extern`] and
    /// [`OpSpec::Intrinsic`]; interned per lowering.
    ///
    /// # Examples
    ///
    /// See [`NameRef`].
    pub fn name(&mut self, text: &str) -> NameRef {
        NameRef(self.string(text))
    }

    fn string(&mut self, text: &str) -> u32 {
        let i = self.m.strings.len();
        self.m.strings.push(text.into());
        u32::try_from(i).unwrap_or_else(|_| {
            self.rule_picks_error = Some(MappingError::TooLarge);
            0
        })
    }

    /// Sets the language-wide policies.
    ///
    /// # Examples
    ///
    /// ```
    /// use lower_lang::LangPolicy;
    ///
    /// let mut policy = LangPolicy::default();
    /// policy.implicit_locals = true;
    /// let mut b = lower_lang::MappingBuilder::<u8>::new();
    /// b.policy(policy);
    /// assert!(b.build()?.policy().implicit_locals);
    /// # Ok::<(), lower_lang::MappingError>(())
    /// ```
    pub fn policy(&mut self, policy: LangPolicy) -> &mut Self {
        self.m.policy = policy;
        self
    }

    /// Registers a template and returns its id. Templates it
    /// [`apply`](crate::TemplateBuilder::apply)s must already be registered,
    /// which keeps templates acyclic by construction.
    ///
    /// # Errors
    ///
    /// [`MappingError::UnknownTemplate`] if an `apply` names a template not
    /// registered before this one; [`MappingError::ApplyArgs`] if its
    /// arguments do not match that template's holes.
    ///
    /// # Examples
    ///
    /// ```
    /// use lower_lang::{templates, MappingBuilder};
    ///
    /// let mut b = MappingBuilder::<u8>::new();
    /// let id = b.template(templates::while_loop())?;
    /// assert_eq!(id.index(), 0);
    /// # Ok::<(), lower_lang::MappingError>(())
    /// ```
    pub fn template(&mut self, mut template: Template) -> Result<TemplateId, MappingError> {
        let index = self.m.templates.len();
        for &node in &template.applies {
            let Some(TNode::Apply {
                template: callee,
                args,
            }) = template.node(node).copied()
            else {
                continue;
            };
            let callee_t =
                self.m
                    .templates
                    .get(callee as usize)
                    .ok_or(MappingError::UnknownTemplate {
                        template: callee as usize,
                    })?;
            let (start, len) = (args.0 as usize, args.1 as usize);
            let given = template
                .args
                .get(start..start.saturating_add(len))
                .ok_or(MappingError::ApplyArgs { template: index })?;
            if given.len() != callee_t.holes.len() {
                return Err(MappingError::ApplyArgs { template: index });
            }
            for (arg, hole) in given.iter().zip(&callee_t.holes) {
                let fits = match arg.kind() {
                    Some(kind) => kind == hole.kind,
                    None => hole.optional,
                };
                let forwarded_ok = match *arg {
                    TArg::Stmts(h) => template
                        .holes
                        .get(usize::from(h.0))
                        .is_some_and(|d| d.kind == HoleKind::Stmts),
                    TArg::Args(h) => template
                        .holes
                        .get(usize::from(h.0))
                        .is_some_and(|d| d.kind == HoleKind::Args),
                    _ => true,
                };
                if !fits || !forwarded_ok {
                    return Err(MappingError::ApplyArgs { template: index });
                }
            }
        }
        let id = u32::try_from(index).map_err(|_| MappingError::TooLarge)?;
        let base = u32::try_from(self.m.strings.len()).map_err(|_| MappingError::TooLarge)?;
        template.text_base = base;
        self.m.strings.extend(template.texts.iter().cloned());
        let name = template.name.clone();
        let name_ref = self.string(&name);
        self.m.template_names.push(name_ref);
        self.m.templates.push(template);
        let _ = self.template_index.insert(name, id);
        Ok(TemplateId(id))
    }

    /// Parses a template text ([`Template::parse`]) and registers it under
    /// `name`: the path for LSF2's `[semantics.templates]`, in document
    /// order. `apply:<name>(…)` in the text names a template registered
    /// before this one — by its name, or, for the built-ins of
    /// [`templates`](crate::templates), by its LSF2 name (`while`,
    /// `do_while`, `c_for`, `for_each`, `logical_and`, `logical_or`,
    /// `and_value`, `or_value`, `coalesce`, `try`, `post_increment`). When a
    /// name is registered twice, the later registration answers.
    ///
    /// # Errors
    ///
    /// [`TemplateError::Syntax`] for text that is not a valid template,
    /// including an `apply:` of an unknown template
    /// ([`SyntaxError::UnknownTemplate`]) or with arguments that do not fit
    /// its holes ([`SyntaxError::ApplyArgs`]); `TooLarge` when a table
    /// outgrows its index type.
    ///
    /// # Examples
    ///
    /// ```
    /// use lower_lang::{templates, MappingBuilder};
    ///
    /// let mut b = MappingBuilder::<u8>::new();
    /// let _coalesce = b.template(templates::coalesce())?;
    /// // `??` under another name, through the built-in.
    /// let elvis = b.parse_template("elvis", "apply:coalesce(${lhs}, ${rhs})")?;
    /// assert_eq!(elvis.index(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn parse_template(&mut self, name: &str, text: &str) -> Result<TemplateId, TemplateError> {
        let registry = Registry {
            templates: &self.m.templates,
            by_name: &self.template_index,
        };
        let template = text::parse(name, text, Some(registry))?;
        self.template(template).map_err(|e| match e {
            MappingError::TooLarge => TemplateError::TooLarge,
            // The parser checked every `apply` against this registry.
            _ => TemplateError::Syntax {
                at: hir_lang::Span::new(0, u32::try_from(text.len()).unwrap_or(u32::MAX)),
                error: SyntaxError::ApplyArgs,
            },
        })
    }

    /// Nodes of `kind` lower through `template`, its holes filled from
    /// `holes` (one pick per hole, in the template's hole order).
    ///
    /// # Examples
    ///
    /// ```
    /// use lower_lang::{templates, MappingBuilder, Pick};
    ///
    /// const WHILE: u8 = 1;
    /// let mut b = MappingBuilder::new();
    /// let w = b.template(templates::while_loop())?;
    /// // `while COND BLOCK`: holes cond, body, label.
    /// b.template_node(WHILE, w, &[Pick::NthNode(0), Pick::NthNode(1), Pick::Absent]);
    /// assert!(b.build().is_ok());
    /// # Ok::<(), lower_lang::MappingError>(())
    /// ```
    pub fn template_node(&mut self, kind: K, template: TemplateId, holes: &[Pick<K>]) -> &mut Self {
        self.template_node_when(kind, Pick::Absent, template, holes)
    }

    /// Nodes of `kind` lower through `template` when `when` finds a child;
    /// otherwise the next rule for `kind` (in registration order) is tried,
    /// as with [`node_when`](Self::node_when). LSF2's conditional template
    /// alternative `{ when = …, template = …, holes = … }` (ISSUES P40): PHP's
    /// `include` / `include_once` / `require` / `require_once` are one node
    /// kind told apart by their keyword.
    ///
    /// # Examples
    ///
    /// ```
    /// use lower_lang::{MappingBuilder, Pick};
    ///
    /// const INCLUDE_EXPR: u8 = 1;
    /// const REQUIRE_KW: u8 = 2;
    /// let mut b = MappingBuilder::new();
    /// let require = b.parse_template("require", "extern:mox.require(${path})")?;
    /// let include = b.parse_template("include", "extern:mox.include(${path})")?;
    /// // `require EXPR` / `include EXPR`: the path is the last child.
    /// b.template_node_when(INCLUDE_EXPR, Pick::Kind(REQUIRE_KW), require, &[Pick::Back(0)]);
    /// b.template_node(INCLUDE_EXPR, include, &[Pick::Back(0)]);
    /// assert!(b.build().is_ok());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn template_node_when(
        &mut self,
        kind: K,
        when: Pick<K>,
        template: TemplateId,
        holes: &[Pick<K>],
    ) -> &mut Self {
        let first = self.m.hole_picks.len();
        self.m.hole_picks.extend_from_slice(holes);
        match (u32::try_from(first), u32::try_from(holes.len())) {
            (Ok(first), Ok(len)) => self.add_rule(
                kind,
                when,
                Arch::Template(TemplateUse {
                    template,
                    first,
                    len,
                }),
            ),
            _ => {
                self.rule_picks_error = Some(MappingError::TooLarge);
                self
            }
        }
    }

    /// Checks the mapping and returns it.
    ///
    /// # Errors
    ///
    /// A [`MappingError`]: a required field without a pick, a kind with two
    /// entries in a one-entry table (or a text with two entries in a text
    /// operator table), an unknown template or a wrong number of template
    /// picks, an operator spec that cannot apply where it is registered, an
    /// empty operator text, an interpolation join template of the wrong
    /// shape, a per-archetype modifier naming no archetype, or an implicit
    /// receiver name this builder did not store.
    ///
    /// # Examples
    ///
    /// See [`MappingBuilder`].
    pub fn build(mut self) -> Result<Mapping<K>, MappingError> {
        if let Some(e) = self.rule_picks_error {
            return Err(e);
        }
        if let Some(NameRef(i)) = self.m.policy.implicit_receiver {
            if i as usize >= self.m.strings.len() {
                return Err(MappingError::UnknownName { index: i as usize });
            }
        }
        let mut arch_mods = Vec::with_capacity(self.arch_modifiers.len());
        for (index, (kind, name, modifier)) in self.arch_modifiers.iter().enumerate() {
            let name =
                crate::arch::lsf_name_of(name).ok_or(MappingError::UnknownArchetype { index })?;
            arch_mods.push((*kind, name, *modifier, index));
        }
        arch_mods.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(b.1)).then(a.3.cmp(&b.3)));
        if let Some(w) = arch_mods
            .windows(2)
            .find(|w| w[0].0 == w[1].0 && w[0].1 == w[1].1)
        {
            return Err(MappingError::DuplicateKind {
                table: "archetype modifier",
                index: w[1].3,
            });
        }
        self.m.arch_modifiers = arch_mods
            .into_iter()
            .map(|(k, a, m, _)| (k, a, m))
            .collect();
        let m = &mut self.m;
        for (i, rule) in m.rules.iter().enumerate() {
            if let Some(field) = missing_field(&rule.arch) {
                return Err(MappingError::MissingField { rule: i, field });
            }
            if let Arch::Template(used) = rule.arch {
                let t = m.templates.get(used.template.index()).ok_or(
                    MappingError::UnknownTemplate {
                        template: used.template.index(),
                    },
                )?;
                if t.holes.len() != used.len as usize {
                    return Err(MappingError::HoleCount {
                        rule: i,
                        expected: t.holes.len(),
                        found: used.len as usize,
                    });
                }
            }
            if let Arch::Interp {
                join: Join::Template(id),
                ..
            } = rule.arch
            {
                let t = m
                    .templates
                    .get(id.index())
                    .ok_or(MappingError::UnknownTemplate {
                        template: id.index(),
                    })?;
                let fold = t.holes.len() == 2 && t.holes.iter().all(|h| h.kind == HoleKind::Expr);
                let all = t.holes.len() == 1 && t.holes.iter().all(|h| h.kind == HoleKind::Args);
                if !fold && !all {
                    return Err(MappingError::BadJoin { rule: i });
                }
            }
        }
        for (fixity, table) in [
            (Fixity::Prefix, &m.prefix_text),
            (Fixity::Binary, &m.binary_text),
            (Fixity::Postfix, &m.postfix_text),
        ] {
            for (index, (text, spec)) in table.iter().enumerate() {
                if text.is_empty() {
                    return Err(MappingError::EmptyOperatorText { fixity, index });
                }
                if !op_fits(*spec, fixity, &m.templates) {
                    return Err(MappingError::BadOperator { fixity, index });
                }
            }
        }
        for (fixity, table) in [
            (Fixity::Prefix, &m.prefix),
            (Fixity::Binary, &m.binary),
            (Fixity::Postfix, &m.postfix),
        ] {
            for (index, (_, spec)) in table.iter().enumerate() {
                if !op_fits(*spec, fixity, &m.templates) {
                    return Err(MappingError::BadOperator { fixity, index });
                }
            }
        }
        m.by_kind.sort_by_key(|e| e.0);
        sort_unique(&mut m.roles, "token role")?;
        sort_unique(&mut m.modifiers, "modifier")?;
        sort_unique(&mut m.roots, "root")?;
        sort_unique(&mut m.prefix, "prefix operator")?;
        sort_unique(&mut m.binary, "binary operator")?;
        sort_unique(&mut m.postfix, "postfix operator")?;
        sort_unique_text(&mut m.prefix_text, "prefix operator text")?;
        sort_unique_text(&mut m.binary_text, "binary operator text")?;
        sort_unique_text(&mut m.postfix_text, "postfix operator text")?;
        let mut attrs: Vec<K> = m
            .by_kind
            .iter()
            .filter(|(_, r)| {
                m.rules
                    .get(*r as usize)
                    .is_some_and(|r| matches!(r.arch, Arch::Attr { .. }))
            })
            .map(|(k, _)| *k)
            .collect();
        attrs.sort();
        attrs.dedup();
        m.attr_kinds = attrs;
        m.containers.sort();
        m.containers.dedup();
        m.skips.sort();
        m.skips.dedup();
        m.errors.sort();
        m.errors.dedup();
        let mut names: Vec<(usize, &str)> = m
            .type_names
            .iter()
            .enumerate()
            .map(|(i, (t, _))| (i, &**t))
            .collect();
        names.sort_by(|a, b| a.1.cmp(b.1).then(a.0.cmp(&b.0)));
        if let Some(w) = names.windows(2).find(|w| w[0].1 == w[1].1) {
            return Err(MappingError::DuplicateTypeName { index: w[1].0 });
        }
        m.type_names.sort_by(|a, b| a.0.cmp(&b.0));
        // Longest suffix first, so `i64` wins over a hypothetical `4`.
        m.suffixes
            .sort_by(|a, b| b.0.len().cmp(&a.0.len()).then(a.0.cmp(&b.0)));
        Ok(self.m)
    }
}

/// Sorts a kind-keyed table, refusing a kind with two entries (reported by
/// the later entry's registration index).
fn sort_unique<K: Ord + Copy, V>(
    table: &mut [(K, V)],
    name: &'static str,
) -> Result<(), MappingError> {
    let mut order: Vec<(K, usize)> = table
        .iter()
        .enumerate()
        .map(|(i, (k, _))| (*k, i))
        .collect();
    order.sort();
    if let Some(w) = order.windows(2).find(|w| w[0].0 == w[1].0) {
        return Err(MappingError::DuplicateKind {
            table: name,
            index: w[1].1,
        });
    }
    table.sort_by_key(|e| e.0);
    Ok(())
}

/// Sorts a text-keyed table, refusing a text with two entries.
fn sort_unique_text<V>(
    table: &mut [(Box<str>, V)],
    name: &'static str,
) -> Result<(), MappingError> {
    let mut order: Vec<(&str, usize)> = table
        .iter()
        .enumerate()
        .map(|(i, (t, _))| (&**t, i))
        .collect();
    order.sort_unstable();
    if let Some(w) = order.windows(2).find(|w| w[0].0 == w[1].0) {
        return Err(MappingError::DuplicateKind {
            table: name,
            index: w[1].1,
        });
    }
    table.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(())
}

/// Whether `spec` can apply at `fixity`.
fn op_fits(spec: OpSpec, fixity: Fixity, templates: &[Template]) -> bool {
    let arity = fixity.arity();
    match spec {
        OpSpec::Op(kind) => kind.arity() == arity,
        OpSpec::OpWith(op) => op.kind.arity() == arity && op.policy_matches(),
        OpSpec::Compound(kind) => fixity == Fixity::Binary && compound_ok(kind),
        OpSpec::CompoundWith(op) => {
            fixity == Fixity::Binary && compound_ok(op.kind) && op.policy_matches()
        }
        OpSpec::Assign | OpSpec::RefAssign => fixity == Fixity::Binary,
        OpSpec::Identity | OpSpec::Await => fixity != Fixity::Binary,
        OpSpec::Borrow(_) | OpSpec::Deref => fixity == Fixity::Prefix,
        OpSpec::Template(id) => templates.get(id.index()).is_some_and(|t| {
            t.holes.len() == arity && t.holes.iter().all(|h| h.kind == HoleKind::Expr)
        }),
        OpSpec::Extern(_) | OpSpec::Intrinsic(_) => true,
        OpSpec::CompoundExtern(_) | OpSpec::CompoundIntrinsic(_) => fixity == Fixity::Binary,
        OpSpec::CompoundTemplate(id) => {
            fixity == Fixity::Binary
                && templates.get(id.index()).is_some_and(|t| {
                    t.holes.len() == 2 && t.holes.iter().all(|h| h.kind == HoleKind::Expr)
                })
        }
    }
}

/// The first required field an archetype leaves `Absent`.
fn missing_field<K>(arch: &Arch<K>) -> Option<&'static str> {
    let need =
        |pairs: &[(&'static str, bool)]| pairs.iter().find(|(_, absent)| *absent).map(|(f, _)| *f);
    match arch {
        Arch::Module { .. }
        | Arch::Splice { .. }
        | Arch::Unsupported
        | Arch::GlobalDecl { .. }
        | Arch::MixinUse { .. }
        | Arch::UseTree { .. }
        | Arch::Interp { .. }
        | Arch::AttrArg { .. }
        | Arch::FieldDecl { .. }
        | Arch::Tuple { .. }
        | Arch::Array { .. }
        | Arch::Map { .. }
        | Arch::RecordLit { .. }
        | Arch::Block { .. }
        | Arch::Break { .. }
        | Arch::Continue { .. }
        | Arch::Return { .. }
        | Arch::Yield { .. }
        | Arch::Template(_)
        | Arch::WildPat
        | Arch::RestPat { .. }
        | Arch::TuplePat { .. }
        | Arch::RecordPat { .. }
        | Arch::OrPat { .. }
        | Arch::RangePat { .. }
        | Arch::SlicePat { .. }
        | Arch::TupleTy { .. }
        | Arch::FnTy { .. }
        | Arch::ObjectTy { .. }
        | Arch::ImplTy { .. }
        | Arch::UnionTy { .. }
        | Arch::IntersectionTy { .. } => None,
        Arch::Pass { inner } => need(&[("inner", inner.is_absent())]),
        Arch::Let { pat, .. } => need(&[("pat", pat.is_absent())]),
        Arch::ExprStmt { expr } => need(&[("expr", expr.is_absent())]),
        Arch::Defer { body } => need(&[("body", body.is_absent())]),
        Arch::Static { name, .. }
        | Arch::Fn { name, .. }
        | Arch::Record { name, .. }
        | Arch::Sum { name, .. }
        | Arch::Interface { name, .. }
        | Arch::Class { name, .. }
        | Arch::AssocType { name, .. }
        | Arch::Const { name, .. }
        | Arch::GlobalVar { name, .. }
        | Arch::VariantDecl { name, .. }
        | Arch::GenericParam { name, .. }
        | Arch::Capture { name }
        | Arch::Segment { name, .. }
        | Arch::BindPat { name, .. }
        | Arch::FieldPat { name, .. }
        | Arch::FieldInit { name, .. } => need(&[("name", name.is_absent())]),
        Arch::Import { path, .. } => need(&[("path", path.is_absent())]),
        Arch::MixinRule { method, .. } => need(&[("method", method.is_absent())]),
        Arch::Attr { name, .. } => need(&[("name", name.is_absent())]),
        Arch::Impl { self_ty, .. } => need(&[("self_ty", self_ty.is_absent())]),
        Arch::Alias { name, ty, .. } | Arch::AssocBinding { name, ty } => {
            need(&[("name", name.is_absent()), ("ty", ty.is_absent())])
        }
        Arch::WherePred { subject, .. } => need(&[("subject", subject.is_absent())]),
        Arch::Param { pat, .. } => need(&[("pat", pat.is_absent())]),
        Arch::Path {
            segments, qself, ..
        } => need(&[("segments", segments.is_absent() && qself.is_absent())]),
        Arch::Unary { op, operand } | Arch::Postfix { operand, op } => {
            need(&[("op", op.is_absent()), ("operand", operand.is_absent())])
        }
        Arch::Binary { lhs, op, rhs } => need(&[
            ("lhs", lhs.is_absent()),
            ("op", op.is_absent()),
            ("rhs", rhs.is_absent()),
        ]),
        Arch::Assign { target, value, .. } => {
            need(&[("target", target.is_absent()), ("value", value.is_absent())])
        }
        Arch::RefAssign { target, source } => need(&[
            ("target", target.is_absent()),
            ("source", source.is_absent()),
        ]),
        Arch::Call { callee, .. } => need(&[("callee", callee.is_absent())]),
        Arch::Arg { value, .. } | Arch::MapEntry { value, .. } => {
            need(&[("value", value.is_absent())])
        }
        Arch::Member { base, member, .. } => {
            need(&[("base", base.is_absent()), ("member", member.is_absent())])
        }
        Arch::MethodCall {
            receiver, method, ..
        } => need(&[
            ("receiver", receiver.is_absent()),
            ("method", method.is_absent()),
        ]),
        Arch::DynMember { base, name, .. } => {
            need(&[("base", base.is_absent()), ("name", name.is_absent())])
        }
        Arch::Index { base, index } => {
            need(&[("base", base.is_absent()), ("index", index.is_absent())])
        }
        Arch::VarVar { inner }
        | Arch::Deref { inner }
        | Arch::Await { inner }
        | Arch::YieldFrom { inner }
        | Arch::Spawn { inner }
        | Arch::RefPat { inner }
        | Arch::PtrTy { inner }
        | Arch::NullableTy { inner }
        | Arch::RefTy { inner, .. } => need(&[("inner", inner.is_absent())]),
        Arch::Append { base } => need(&[("base", base.is_absent())]),
        Arch::Repeat { elem, count } => {
            need(&[("elem", elem.is_absent()), ("count", count.is_absent())])
        }
        Arch::Cast { expr, ty } => need(&[("expr", expr.is_absent()), ("ty", ty.is_absent())]),
        Arch::Borrow { expr } => need(&[("expr", expr.is_absent())]),
        Arch::If { cond, then, .. } => {
            need(&[("cond", cond.is_absent()), ("then", then.is_absent())])
        }
        Arch::Match { scrutinee, .. } => need(&[("scrutinee", scrutinee.is_absent())]),
        Arch::Arm { pat, body, .. } => {
            need(&[("pat", pat.is_absent()), ("body", body.is_absent())])
        }
        Arch::Loop { body, .. }
        | Arch::Try { body, .. }
        | Arch::Catch { body, .. }
        | Arch::Closure { body, .. } => need(&[("body", body.is_absent())]),
        Arch::Throw { value } => need(&[("value", value.is_absent())]),
        Arch::CtorPat { path, .. } => need(&[("path", path.is_absent())]),
        Arch::TypeTestPat { ty, .. } => need(&[("ty", ty.is_absent())]),
        Arch::ArrayTy { elem, len } => {
            need(&[("elem", elem.is_absent()), ("len", len.is_absent())])
        }
        Arch::SliceTy { elem } => need(&[("elem", elem.is_absent())]),
    }
}

/// The modifier bit a token of `kind` sets inside archetype `arch` (its LSF2
/// name; `None` when the mapping has no per-archetype entries), for the scan
/// of a node's children.
pub(crate) fn modifier_bits<K: Copy + Ord>(map: &Mapping<K>, kind: &K, arch: Option<&str>) -> u32 {
    if let Some(m) = arch.and_then(|a| map.arch_modifier(kind, a)) {
        return m.bit();
    }
    find(&map.modifiers, kind).map_or(0, |m| m.bit())
}

#[cfg(test)]
// Builder methods return `&mut Self` for chaining; the tests discard it.
#[allow(unused_results)]
mod tests {
    use super::*;
    use crate::templates;

    #[test]
    fn test_duplicate_roles_are_refused() {
        let mut b = MappingBuilder::<u8>::new();
        b.token(1, TokenRole::Name).token(1, TokenRole::Int);
        assert_eq!(
            b.build().err(),
            Some(MappingError::DuplicateKind {
                table: "token role",
                index: 1
            })
        );
    }

    #[test]
    fn test_operator_specs_are_checked_against_their_position() {
        let mut b = MappingBuilder::<u8>::new();
        b.binary_op(1, OpSpec::Op(OpKind::Neg));
        assert!(matches!(b.build(), Err(MappingError::BadOperator { .. })));

        let mut b = MappingBuilder::<u8>::new();
        b.binary_op(1, OpSpec::Compound(OpKind::Eq));
        assert!(matches!(b.build(), Err(MappingError::BadOperator { .. })));

        let mut b = MappingBuilder::<u8>::new();
        b.prefix_op(1, OpSpec::Assign);
        assert!(matches!(b.build(), Err(MappingError::BadOperator { .. })));

        let mut b = MappingBuilder::<u8>::new();
        let w = b.template(templates::while_loop()).ok();
        if let Some(w) = w {
            b.binary_op(1, OpSpec::Template(w));
        }
        assert!(matches!(b.build(), Err(MappingError::BadOperator { .. })));
    }

    #[test]
    fn test_template_node_checks_hole_count() {
        let mut b = MappingBuilder::<u8>::new();
        let Ok(w) = b.template(templates::while_loop()) else {
            panic!("registers");
        };
        b.template_node(1, w, &[Pick::Nth(0)]);
        assert_eq!(
            b.build().err(),
            Some(MappingError::HoleCount {
                rule: 0,
                expected: 3,
                found: 1
            })
        );
    }

    #[test]
    fn test_conditional_rules_keep_registration_order() {
        let mut b = MappingBuilder::<u8>::new();
        b.node(9, Arch::WildPat);
        b.node_when(4, Pick::First(1), Arch::Unsupported);
        b.node(4, Arch::WildPat);
        let Ok(m) = b.build() else { panic!("valid") };
        let (s, e) = m.rule_range(&4);
        assert_eq!(e - s, 2);
        assert_eq!(m.rule_for(&4), Some(&Arch::Unsupported));
    }

    #[test]
    fn test_duplicate_type_names_are_refused() {
        let mut b = MappingBuilder::<u8>::new();
        b.type_name("int", TypeName::Any)
            .type_name("int", TypeName::Never);
        assert_eq!(
            b.build().err(),
            Some(MappingError::DuplicateTypeName { index: 1 })
        );
    }
}
