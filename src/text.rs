//! Textual templates: the template subset of HIR's textual syntax (HIR spec
//! §20 "Template additions", LSF2 §15.7/§15.8), parsed straight into the
//! [`Template`] AST that [`TemplateBuilder`] builds.
//!
//! The text is parsed once, at forge time, into builder calls. Nothing is
//! ever substituted as text: holes are filled with already-lowered HIR when
//! the template is instantiated, so a template cannot produce text that
//! re-lexes differently (decision D26).
//!
//! The parser is recursive descent with an explicit nesting cap
//! ([`MAX_TEMPLATE_DEPTH`]) and a length cap ([`MAX_TEMPLATE_TEXT`]); every
//! lookup (holes, binders, labels, templates) is a sorted-map lookup, so
//! parsing is `O(n log n)` in the text and never quadratic. It never panics:
//! every refusal is a [`TemplateError::Syntax`] with the byte range of the
//! offending text.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use hir_lang::{CaptureMode, DivZero, FloatToInt, Op, OpKind, Overflow, Prim, Shift, Span};

use crate::template::{
    ArgsHole, ExprHole, HoleKind, LabelHole, LabelRef, PatHole, StmtsHole, TArg, TBinder, TExpr,
    TLabel, TLit, TPat, TStmt, TTy, Template, TemplateBuilder, TemplateError, TemplateId, TyHole,
};

/// The longest template text [`Template::parse`] accepts, in bytes (1 MiB).
/// Longer texts are refused with [`SyntaxError::TooLong`].
///
/// # Examples
///
/// ```
/// use lower_lang::{SyntaxError, Template, TemplateError, MAX_TEMPLATE_TEXT};
///
/// let long = " ".repeat(MAX_TEMPLATE_TEXT + 1);
/// assert!(matches!(
///     Template::parse("t", &long),
///     Err(TemplateError::Syntax { error: SyntaxError::TooLong, .. })
/// ));
/// ```
pub const MAX_TEMPLATE_TEXT: usize = 1 << 20;

/// The deepest nesting [`Template::parse`] accepts: expressions, blocks,
/// patterns, and types inside one another (Mox's largest template,
/// `yield_from`, nests 10 deep). Deeper text is refused with
/// [`SyntaxError::TooDeep`] instead of exhausting the stack: at the cap the
/// parser needs under 256 KiB of stack in an unoptimized build and under
/// 64 KiB optimized.
///
/// # Examples
///
/// ```
/// use lower_lang::{SyntaxError, Template, TemplateError, MAX_TEMPLATE_DEPTH};
///
/// let deep = "(".repeat(MAX_TEMPLATE_DEPTH as usize + 1);
/// assert!(matches!(
///     Template::parse("t", &deep),
///     Err(TemplateError::Syntax { error: SyntaxError::TooDeep, .. })
/// ));
/// ```
pub const MAX_TEMPLATE_DEPTH: u32 = 32;

/// What is wrong with a template text. Carried by
/// [`TemplateError::Syntax`] together with the byte range it is about.
///
/// # Examples
///
/// ```
/// use lower_lang::{SyntaxError, Template, TemplateError};
///
/// let err = Template::parse("t", "add(1)").unwrap_err();
/// let TemplateError::Syntax { at, error } = err else { unreachable!() };
/// assert_eq!(error, SyntaxError::Arity { expected: 2, found: 1 });
/// assert_eq!((at.start().to_usize(), at.end().to_usize()), (0, 3));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SyntaxError {
    /// The text is longer than [`MAX_TEMPLATE_TEXT`]. Split the template, or
    /// build it with [`TemplateBuilder`].
    TooLong,
    /// Nesting is deeper than [`MAX_TEMPLATE_DEPTH`]. Flatten the template
    /// (name intermediate values with `let`).
    TooDeep,
    /// A character that starts no token (templates are ASCII outside string
    /// literals).
    UnexpectedChar,
    /// A string, character literal, or hole that is not closed.
    Unterminated,
    /// An unknown escape sequence in a string or character literal.
    BadEscape,
    /// A malformed number, or one that does not fit `i64` / a finite `f64`.
    BadNumber,
    /// Something else was expected here; the payload says what.
    Expected(&'static str),
    /// A name that is neither a binder in scope nor an operation. Free names
    /// are written `global:name`, host symbols `extern:name`.
    UnknownName,
    /// A binder written with an id (`x%0`): template binders are written
    /// without one and are fresh per instantiation.
    BinderId,
    /// An operation name that OPS does not define, or a conversion without
    /// its target type (`int_cast<i32>(x)`).
    UnknownOp,
    /// A policy the operation does not consult, an unknown policy value, a
    /// repeated policy, or a target type on an operation that takes none.
    BadPolicy,
    /// An operation with the wrong number of operands.
    Arity {
        /// The operation's arity.
        expected: usize,
        /// The operands written.
        found: usize,
    },
    /// A hole used in positions of two different kinds (`${x}` as an
    /// expression and as a pattern).
    HoleKind {
        /// The kind of its first use.
        first: HoleKind,
        /// The kind of this use.
        here: HoleKind,
    },
    /// Two uses of a hole give it different `?` / `?=default` suffixes.
    HoleSuffix,
    /// A default on a hole that cannot have one (only expression holes take
    /// `?=<literal>`), or a default that is not a literal.
    HoleDefault,
    /// An argument splice `${...name}` outside a call's arguments, a tuple's
    /// or array's elements, or a block's statements.
    SpliceHere,
    /// One pattern (or one closure's parameters) binds the same name twice.
    BoundTwice,
    /// A label that no enclosing loop or block of the template defines.
    UnknownLabel,
    /// `apply:<name>` names no template registered before this one.
    UnknownTemplate,
    /// `apply:<name>(...)` arguments do not fit the applied template's holes
    /// (too many, a required one missing, or the wrong kind).
    ApplyArgs,
    /// A construct the template subset does not have; the payload names it.
    Unsupported(&'static str),
}

impl fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLong => write!(
                f,
                "the template text is longer than {MAX_TEMPLATE_TEXT} bytes"
            ),
            Self::TooDeep => write!(
                f,
                "the template nests deeper than {MAX_TEMPLATE_DEPTH} levels"
            ),
            Self::UnexpectedChar => f.write_str("unexpected character"),
            Self::Unterminated => f.write_str("unterminated literal or hole"),
            Self::BadEscape => f.write_str("unknown escape sequence"),
            Self::BadNumber => f.write_str("malformed or out-of-range number"),
            Self::Expected(what) => write!(f, "expected {what}"),
            Self::UnknownName => f.write_str(
                "unknown name (free names are written `global:name`, host symbols `extern:name`)",
            ),
            Self::BinderId => {
                f.write_str("template binders are written without an id (`x`, not `x%0`)")
            }
            Self::UnknownOp => f.write_str("unknown operation, or a conversion without a target"),
            Self::BadPolicy => f.write_str("a policy the operation does not take"),
            Self::Arity { expected, found } => {
                write!(f, "the operation takes {expected} operands, {found} given")
            }
            Self::HoleKind { first, here } => {
                write!(f, "the hole is used as {first:?} and as {here:?}")
            }
            Self::HoleSuffix => f.write_str("the hole's uses disagree on `?` or its default"),
            Self::HoleDefault => {
                f.write_str("only expression holes take a default, and it must be a literal")
            }
            Self::SpliceHere => f.write_str(
                "an argument splice must be a call argument, a tuple or array element, or a statement",
            ),
            Self::BoundTwice => f.write_str("the name is bound twice in one pattern"),
            Self::UnknownLabel => f.write_str("no enclosing loop or block has this label"),
            Self::UnknownTemplate => f.write_str("no template of this name is registered before"),
            Self::ApplyArgs => f.write_str("the arguments do not fit the applied template's holes"),
            Self::Unsupported(what) => write!(f, "{what} are not supported in templates"),
        }
    }
}

// ---------------------------------------------------------------- tokens

/// A hole's suffix: nothing, `?`, or `?=<literal>`.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Suffix {
    None,
    Optional,
    Default(TLit),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Free {
    Extern,
    Global,
    Apply,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum T {
    Ident,
    Int(i64),
    Float(f64),
    /// A string literal: its decoded text is `strs[i]`.
    Str(u32),
    Char(char),
    /// `'name`; the name is the token text after the quote.
    Label,
    /// `${name}`, `${name?}`, `${name?=lit}`, `${...name}`; the name is
    /// `text[name.0..name.1]`.
    Hole {
        splice: bool,
        name: (u32, u32),
        suffix: Suffix,
    },
    /// `extern:`, `global:`, `apply:`; the path is the token text after the
    /// colon.
    Free(Free),
    /// One punctuation byte: `{ } ( ) [ ] , ; = : < > | & . -`.
    P(u8),
    /// `=>`.
    Arrow,
    Eof,
}

#[derive(Clone, Copy, Debug)]
struct Tok {
    t: T,
    s: u32,
    e: u32,
}

type R<T> = Result<T, TemplateError>;

fn err<T>(s: u32, e: u32, error: SyntaxError) -> R<T> {
    Err(TemplateError::Syntax {
        at: Span::new(s, e.max(s)),
        error,
    })
}

const fn ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

const fn ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Splits the text into tokens and decodes its string literals.
struct Lexer<'a> {
    text: &'a str,
    bytes: &'a [u8],
    i: usize,
    toks: Vec<Tok>,
    strs: Vec<String>,
}

impl<'a> Lexer<'a> {
    fn run(text: &'a str) -> R<(Vec<Tok>, Vec<String>)> {
        let mut lx = Lexer {
            text,
            bytes: text.as_bytes(),
            i: 0,
            toks: Vec::new(),
            strs: Vec::new(),
        };
        while lx.i < lx.bytes.len() {
            lx.next()?;
        }
        let end = lx.pos(lx.bytes.len());
        lx.toks.push(Tok {
            t: T::Eof,
            s: end,
            e: end,
        });
        Ok((lx.toks, lx.strs))
    }

    /// Byte offsets fit `u32`: the text is at most [`MAX_TEMPLATE_TEXT`].
    fn pos(&self, i: usize) -> u32 {
        u32::try_from(i).unwrap_or(u32::MAX)
    }

    fn at(&self, i: usize) -> u8 {
        self.bytes.get(i).copied().unwrap_or(0)
    }

    fn push(&mut self, t: T, s: usize, e: usize) {
        let (s, e) = (self.pos(s), self.pos(e));
        self.toks.push(Tok { t, s, e });
    }

    fn next(&mut self) -> R<()> {
        let start = self.i;
        let b = self.at(start);
        match b {
            b' ' | b'\t' | b'\n' | b'\r' => {
                self.i += 1;
                Ok(())
            }
            b'$' if self.at(start + 1) == b'{' => self.hole(),
            b'"' => self.string(),
            b'\'' => self.quote(),
            b'0'..=b'9' => self.number(start, false),
            b'-' if self.at(start + 1).is_ascii_digit() => self.number(start, true),
            b'=' if self.at(start + 1) == b'>' => {
                self.i += 2;
                self.push(T::Arrow, start, start + 2);
                Ok(())
            }
            b'{' | b'}' | b'(' | b')' | b'[' | b']' | b',' | b';' | b'=' | b':' | b'<' | b'>'
            | b'|' | b'&' | b'.' | b'-' => {
                self.i += 1;
                self.push(T::P(b), start, start + 1);
                Ok(())
            }
            _ if ident_start(b) => self.word(),
            _ => {
                let len = self.text.get(start..).and_then(|r| r.chars().next());
                let end = start + len.map_or(1, char::len_utf8);
                err(self.pos(start), self.pos(end), SyntaxError::UnexpectedChar)
            }
        }
    }

    fn word(&mut self) -> R<()> {
        let start = self.i;
        while ident_char(self.at(self.i)) {
            self.i += 1;
        }
        let word = self.text.get(start..self.i).unwrap_or("");
        if self.at(self.i) == b'%' {
            return err(self.pos(start), self.pos(self.i + 1), SyntaxError::BinderId);
        }
        let free = match word {
            "extern" => Some(Free::Extern),
            "global" => Some(Free::Global),
            "apply" => Some(Free::Apply),
            _ => None,
        };
        if let (Some(kind), b':') = (free, self.at(self.i)) {
            // `extern:mox.iter`, `global:a::b`, `apply:name`.
            self.i += 1;
            if !ident_start(self.at(self.i)) {
                return err(
                    self.pos(start),
                    self.pos(self.i),
                    SyntaxError::Expected("a name after the prefix"),
                );
            }
            while ident_char(self.at(self.i)) || matches!(self.at(self.i), b'.' | b':') {
                self.i += 1;
            }
            self.push(T::Free(kind), start, self.i);
            return Ok(());
        }
        self.push(T::Ident, start, self.i);
        Ok(())
    }

    fn hole(&mut self) -> R<()> {
        let start = self.i;
        self.i += 2;
        let splice = self.bytes.get(self.i..self.i + 3) == Some(b"...");
        if splice {
            self.i += 3;
        }
        let name_start = self.i;
        while matches!(self.at(self.i), b'a'..=b'z' | b'_') {
            self.i += 1;
        }
        if self.i == name_start {
            return err(
                self.pos(start),
                self.pos(self.i),
                SyntaxError::Expected("a hole name (lowercase letters and `_`)"),
            );
        }
        let name = (self.pos(name_start), self.pos(self.i));
        let mut suffix = Suffix::None;
        if self.at(self.i) == b'?' {
            self.i += 1;
            suffix = Suffix::Optional;
            if self.at(self.i) == b'=' {
                self.i += 1;
                let lit_start = self.i;
                while self.i < self.bytes.len() && self.at(self.i) != b'}' {
                    self.i += 1;
                }
                let lit = self.text.get(lit_start..self.i).unwrap_or("");
                suffix = Suffix::Default(self.default_lit(lit, lit_start)?);
            }
        }
        if self.at(self.i) != b'}' {
            let at = if self.i >= self.bytes.len() {
                SyntaxError::Unterminated
            } else {
                SyntaxError::Expected("`}` closing the hole")
            };
            return err(self.pos(start), self.pos(self.i), at);
        }
        self.i += 1;
        self.push(
            T::Hole {
                splice,
                name,
                suffix,
            },
            start,
            self.i,
        );
        Ok(())
    }

    /// The literal of `${name?=<literal>}`, lexed by the template lexer.
    fn default_lit(&self, lit: &str, at: usize) -> R<TLit> {
        let (s, e) = (self.pos(at), self.pos(at + lit.len()));
        let mut inner = Lexer {
            text: lit,
            bytes: lit.as_bytes(),
            i: 0,
            toks: Vec::new(),
            strs: Vec::new(),
        };
        while inner.i < inner.bytes.len() {
            if inner.next().is_err() {
                return err(s, e, SyntaxError::HoleDefault);
            }
        }
        let word = |t: &Tok| lit.get(t.s as usize..t.e as usize).unwrap_or("");
        match inner.toks.as_slice() {
            [t] => match t.t {
                T::Int(v) => Ok(TLit::Int(v)),
                T::Float(v) => Ok(TLit::Float(v)),
                T::Char(c) => Ok(TLit::Char(c)),
                T::Ident if word(t) == "true" => Ok(TLit::Bool(true)),
                T::Ident if word(t) == "false" => Ok(TLit::Bool(false)),
                T::Ident if word(t) == "null" => Ok(TLit::Null),
                _ => err(s, e, SyntaxError::HoleDefault),
            },
            _ => err(s, e, SyntaxError::HoleDefault),
        }
    }

    fn number(&mut self, start: usize, negative: bool) -> R<()> {
        if negative {
            self.i += 1;
        }
        let digits = self.i;
        let radix = match (self.at(digits), self.at(digits + 1)) {
            (b'0', b'x' | b'X') => 16,
            (b'0', b'o' | b'O') => 8,
            (b'0', b'b' | b'B') => 2,
            _ => 10,
        };
        let bad = |lx: &Self| err(lx.pos(start), lx.pos(lx.i), SyntaxError::BadNumber);
        if radix != 10 {
            self.i += 2;
            let from = self.i;
            while self.at(self.i).is_ascii_alphanumeric() || self.at(self.i) == b'_' {
                self.i += 1;
            }
            let mut v: u64 = 0;
            let mut any = false;
            for &c in self.bytes.get(from..self.i).unwrap_or(&[]) {
                if c == b'_' {
                    continue;
                }
                let d = match char::from(c).to_digit(radix) {
                    Some(d) => u64::from(d),
                    None => return bad(self),
                };
                v = match v
                    .checked_mul(u64::from(radix))
                    .and_then(|v| v.checked_add(d))
                {
                    Some(v) => v,
                    None => return bad(self),
                };
                any = true;
            }
            if !any {
                return bad(self);
            }
            return self.int(v, negative, start);
        }
        while self.at(self.i).is_ascii_digit() || self.at(self.i) == b'_' {
            self.i += 1;
        }
        let mut float = false;
        if self.at(self.i) == b'.' && self.at(self.i + 1).is_ascii_digit() {
            float = true;
            self.i += 1;
            while self.at(self.i).is_ascii_digit() || self.at(self.i) == b'_' {
                self.i += 1;
            }
        }
        if matches!(self.at(self.i), b'e' | b'E') {
            let mut j = self.i + 1;
            if matches!(self.at(j), b'+' | b'-') {
                j += 1;
            }
            if self.at(j).is_ascii_digit() {
                float = true;
                self.i = j;
                while self.at(self.i).is_ascii_digit() {
                    self.i += 1;
                }
            }
        }
        if ident_char(self.at(self.i)) {
            // `12abc`: a suffix or a typo; neither is a template literal.
            while ident_char(self.at(self.i)) {
                self.i += 1;
            }
            return bad(self);
        }
        let raw = self.text.get(start..self.i).unwrap_or("");
        let mut clean = String::with_capacity(raw.len());
        clean.extend(raw.chars().filter(|&c| c != '_'));
        if float {
            match clean.parse::<f64>() {
                Ok(v) if v.is_finite() => {
                    self.push(T::Float(v), start, self.i);
                    Ok(())
                }
                _ => bad(self),
            }
        } else {
            let magnitude = clean.trim_start_matches('-');
            match magnitude.parse::<u64>() {
                Ok(v) => self.int(v, negative, start),
                Err(_) => bad(self),
            }
        }
    }

    fn int(&mut self, magnitude: u64, negative: bool, start: usize) -> R<()> {
        let v = if negative {
            if magnitude == 1 << 63 {
                i64::MIN
            } else {
                match i64::try_from(magnitude) {
                    Ok(v) => -v,
                    Err(_) => {
                        return err(self.pos(start), self.pos(self.i), SyntaxError::BadNumber);
                    }
                }
            }
        } else {
            match i64::try_from(magnitude) {
                Ok(v) => v,
                Err(_) => return err(self.pos(start), self.pos(self.i), SyntaxError::BadNumber),
            }
        };
        self.push(T::Int(v), start, self.i);
        Ok(())
    }

    /// One character after a backslash, at `self.i` (just past the `\`).
    fn escape(&mut self, start: usize) -> R<char> {
        let c = self.at(self.i);
        self.i += 1;
        let simple = match c {
            b'n' => Some('\n'),
            b'r' => Some('\r'),
            b't' => Some('\t'),
            b'0' => Some('\0'),
            b'\\' => Some('\\'),
            b'"' => Some('"'),
            b'\'' => Some('\''),
            _ => None,
        };
        if let Some(ch) = simple {
            return Ok(ch);
        }
        if c == b'u' && self.at(self.i) == b'{' {
            self.i += 1;
            let from = self.i;
            while self.at(self.i).is_ascii_hexdigit() && self.i - from < 6 {
                self.i += 1;
            }
            let hex = self.text.get(from..self.i).unwrap_or("");
            if self.at(self.i) == b'}' && !hex.is_empty() {
                self.i += 1;
                if let Some(ch) = u32::from_str_radix(hex, 16).ok().and_then(char::from_u32) {
                    return Ok(ch);
                }
            }
        }
        err(self.pos(start), self.pos(self.i), SyntaxError::BadEscape)
    }

    fn string(&mut self) -> R<()> {
        let start = self.i;
        self.i += 1;
        let mut out = String::new();
        loop {
            if self.i >= self.bytes.len() {
                return err(self.pos(start), self.pos(self.i), SyntaxError::Unterminated);
            }
            match self.at(self.i) {
                b'"' => {
                    self.i += 1;
                    break;
                }
                b'\\' => {
                    let at = self.i;
                    self.i += 1;
                    let ch = self.escape(at)?;
                    out.push(ch);
                }
                _ => {
                    let ch = self.text.get(self.i..).and_then(|r| r.chars().next());
                    match ch {
                        Some(ch) => {
                            out.push(ch);
                            self.i += ch.len_utf8();
                        }
                        None => {
                            return err(
                                self.pos(start),
                                self.pos(self.i),
                                SyntaxError::Unterminated,
                            );
                        }
                    }
                }
            }
        }
        let index = u32::try_from(self.strs.len()).unwrap_or(u32::MAX);
        self.strs.push(out);
        self.push(T::Str(index), start, self.i);
        Ok(())
    }

    /// `'a'` (a character) or `'name` (a label).
    fn quote(&mut self) -> R<()> {
        let start = self.i;
        self.i += 1;
        if self.at(self.i) == b'\\' {
            let at = self.i;
            self.i += 1;
            let ch = self.escape(at)?;
            return self.close_char(start, ch);
        }
        let Some(ch) = self.text.get(self.i..).and_then(|r| r.chars().next()) else {
            return err(self.pos(start), self.pos(self.i), SyntaxError::Unterminated);
        };
        if self.at(self.i + ch.len_utf8()) == b'\'' {
            self.i += ch.len_utf8();
            return self.close_char(start, ch);
        }
        if !ident_start(self.at(self.i)) {
            return err(
                self.pos(start),
                self.pos(self.i),
                SyntaxError::Expected("a label name or a character literal"),
            );
        }
        while ident_char(self.at(self.i)) {
            self.i += 1;
        }
        if self.at(self.i) == b'%' {
            return err(self.pos(start), self.pos(self.i + 1), SyntaxError::BinderId);
        }
        if self.at(self.i) == b'\'' {
            return err(
                self.pos(start),
                self.pos(self.i + 1),
                SyntaxError::Expected("one character between quotes"),
            );
        }
        self.push(T::Label, start, self.i);
        Ok(())
    }

    fn close_char(&mut self, start: usize, ch: char) -> R<()> {
        if self.at(self.i) != b'\'' {
            return err(self.pos(start), self.pos(self.i), SyntaxError::Unterminated);
        }
        self.i += 1;
        self.push(T::Char(ch), start, self.i);
        Ok(())
    }
}

// ---------------------------------------------------------------- tables

/// An operation name: a plain op, or a conversion that needs its target.
#[derive(Clone, Copy)]
enum Named {
    Plain(OpKind),
    Conv(fn(Prim) -> OpKind),
}

fn op_named(name: &str) -> Option<Named> {
    use Named::{Conv, Plain};
    Some(match name {
        "add" => Plain(OpKind::Add),
        "sub" => Plain(OpKind::Sub),
        "mul" => Plain(OpKind::Mul),
        "neg" => Plain(OpKind::Neg),
        "abs" => Plain(OpKind::Abs),
        "div" => Plain(OpKind::Div),
        "floor_div" => Plain(OpKind::FloorDiv),
        "rem" => Plain(OpKind::Rem),
        "floor_mod" => Plain(OpKind::FloorMod),
        "and" => Plain(OpKind::And),
        "or" => Plain(OpKind::Or),
        "xor" => Plain(OpKind::Xor),
        "not" => Plain(OpKind::Not),
        "bit_not" => Plain(OpKind::BitNot),
        "shl" => Plain(OpKind::Shl),
        "shr" => Plain(OpKind::Shr),
        "pow" => Plain(OpKind::Pow),
        "eq" => Plain(OpKind::Eq),
        "ne" => Plain(OpKind::Ne),
        "lt" => Plain(OpKind::Lt),
        "le" => Plain(OpKind::Le),
        "gt" => Plain(OpKind::Gt),
        "ge" => Plain(OpKind::Ge),
        "min" => Plain(OpKind::Min),
        "max" => Plain(OpKind::Max),
        "ieee_rem" => Plain(OpKind::IeeeRem),
        "sqrt" => Plain(OpKind::Sqrt),
        "fma" => Plain(OpKind::Fma),
        "floor" => Plain(OpKind::Floor),
        "ceil" => Plain(OpKind::Ceil),
        "trunc" => Plain(OpKind::Trunc),
        "round" => Plain(OpKind::Round),
        "round_even" => Plain(OpKind::RoundEven),
        "total_cmp" => Plain(OpKind::TotalCmp),
        "f32_to_f64" => Plain(OpKind::F32ToF64),
        "f64_to_f32" => Plain(OpKind::F64ToF32),
        "char_from_u32" => Plain(OpKind::CharFromU32),
        "int_cast" => Conv(OpKind::IntCast),
        "float_to_int" => Conv(OpKind::FloatToInt),
        "zext" => Conv(OpKind::Zext),
        "sext" => Conv(OpKind::Sext),
        "narrow" => Conv(OpKind::Narrow),
        "int_to_float" => Conv(OpKind::IntToFloat),
        "bitcast" => Conv(OpKind::Bitcast),
        "bool_to_int" => Conv(OpKind::BoolToInt),
        _ => return None,
    })
}

fn prim_named(name: &str) -> Option<Prim> {
    Some(match name {
        "bool" => Prim::Bool,
        "i8" => Prim::I8,
        "i16" => Prim::I16,
        "i32" => Prim::I32,
        "i64" => Prim::I64,
        "isize" => Prim::Isize,
        "u8" => Prim::U8,
        "u16" => Prim::U16,
        "u32" => Prim::U32,
        "u64" => Prim::U64,
        "usize" => Prim::Usize,
        "f32" => Prim::F32,
        "f64" => Prim::F64,
        "char" => Prim::Char,
        "str" => Prim::Str,
        _ => return None,
    })
}

/// Words that are never binder names.
fn reserved(word: &str) -> bool {
    matches!(
        word,
        "let"
            | "let_place"
            | "in"
            | "if"
            | "else"
            | "match"
            | "loop"
            | "break"
            | "continue"
            | "return"
            | "throw"
            | "await"
            | "yield"
            | "from"
            | "spawn"
            | "closure"
            | "defer"
            | "mut"
            | "true"
            | "false"
            | "null"
            | "_"
    )
}

/// The LSF2 names of lower-lang's built-in templates (LSF2 §15.7) and the
/// names they are registered under, for `apply:<name>`.
const BUILTIN_NAMES: [(&str, &str); 11] = [
    ("while", "while"),
    ("do_while", "do-while"),
    ("c_for", "for"),
    ("for_each", "foreach"),
    ("logical_and", "&&"),
    ("logical_or", "||"),
    ("and_value", "and"),
    ("or_value", "or"),
    ("coalesce", "??"),
    ("try", "?"),
    ("post_increment", "post++"),
];

/// The templates `apply:<name>` can name: those registered in a mapping
/// before the one being parsed, by name (the latest registration of a name
/// wins).
#[derive(Clone, Copy)]
pub(crate) struct Registry<'a> {
    pub(crate) templates: &'a [Template],
    pub(crate) by_name: &'a BTreeMap<alloc::boxed::Box<str>, u32>,
}

impl<'a> Registry<'a> {
    fn find(&self, name: &str) -> Option<(TemplateId, &'a Template)> {
        let builtin = BUILTIN_NAMES
            .iter()
            .find(|(lsf, _)| *lsf == name)
            .map(|(_, registered)| *registered);
        let index = self.by_name.get(name).copied().or_else(|| {
            let registered = builtin?;
            let i = self.by_name.get(registered).copied()?;
            // Only the built-in itself answers to its LSF2 name.
            let t = self.templates.get(i as usize)?;
            (t.kind == hir_lang::ExpnKind::Desugar).then_some(i)
        })?;
        self.templates
            .get(index as usize)
            .map(|t| (TemplateId(index), t))
    }
}

// ---------------------------------------------------------------- parser

#[derive(Clone, Copy)]
struct HoleInfo {
    index: u16,
    kind: HoleKind,
    /// Whether a use has written a `?` / `?=` suffix yet.
    suffixed: bool,
    suffix: Suffix,
}

struct Parser<'a> {
    text: &'a str,
    toks: Vec<Tok>,
    strs: Vec<String>,
    pos: usize,
    b: TemplateBuilder,
    holes: BTreeMap<&'a str, HoleInfo>,
    scope: BTreeMap<&'a str, Vec<TBinder>>,
    scope_log: Vec<&'a str>,
    labels: BTreeMap<&'a str, Vec<TLabel>>,
    label_log: Vec<&'a str>,
    /// Binders a pattern under construction introduces, not yet in scope.
    pending: Vec<(&'a str, TBinder, u32, u32)>,
    depth: u32,
    registry: Option<Registry<'a>>,
}

/// Parses `text` as template `name`, `apply:` resolving against `registry`.
pub(crate) fn parse(
    name: &str,
    text: &str,
    registry: Option<Registry<'_>>,
) -> Result<Template, TemplateError> {
    if text.len() > MAX_TEMPLATE_TEXT {
        let end = u32::try_from(MAX_TEMPLATE_TEXT).unwrap_or(u32::MAX);
        return err(0, end, SyntaxError::TooLong);
    }
    let (toks, strs) = Lexer::run(text)?;
    let mut p = Parser {
        text,
        toks,
        strs,
        pos: 0,
        b: TemplateBuilder::new(name),
        holes: BTreeMap::new(),
        scope: BTreeMap::new(),
        scope_log: Vec::new(),
        labels: BTreeMap::new(),
        label_log: Vec::new(),
        pending: Vec::new(),
        depth: 0,
        registry,
    };
    let root = p.expr()?;
    let end = p.peek();
    if end.t != T::Eof {
        return err(
            end.s,
            end.e,
            SyntaxError::Expected("the end of the template"),
        );
    }
    p.b.finish(root)
}

impl<'a> Parser<'a> {
    // ------------------------------------------------------------ tokens

    fn peek(&self) -> Tok {
        self.peek_at(0)
    }

    fn peek_at(&self, n: usize) -> Tok {
        let last = self.toks.last().copied().unwrap_or(Tok {
            t: T::Eof,
            s: 0,
            e: 0,
        });
        self.toks
            .get(self.pos.saturating_add(n))
            .copied()
            .unwrap_or(last)
    }

    fn bump(&mut self) -> Tok {
        let t = self.peek();
        if self.pos < self.toks.len() {
            self.pos += 1;
        }
        t
    }

    fn word(&self, t: Tok) -> &'a str {
        self.text.get(t.s as usize..t.e as usize).unwrap_or("")
    }

    fn is_p(&self, c: u8) -> bool {
        self.peek().t == T::P(c)
    }

    fn eat_p(&mut self, c: u8) -> bool {
        if self.is_p(c) {
            let _ = self.bump();
            true
        } else {
            false
        }
    }

    fn expect_p(&mut self, c: u8, what: &'static str) -> R<Tok> {
        let t = self.peek();
        if t.t == T::P(c) {
            Ok(self.bump())
        } else {
            err(t.s, t.e, SyntaxError::Expected(what))
        }
    }

    fn is_kw(&self, kw: &str) -> bool {
        let t = self.peek();
        t.t == T::Ident && self.word(t) == kw
    }

    fn eat_kw(&mut self, kw: &str) -> bool {
        if self.is_kw(kw) {
            let _ = self.bump();
            true
        } else {
            false
        }
    }

    fn expect_kw(&mut self, kw: &str, what: &'static str) -> R<()> {
        if self.eat_kw(kw) {
            Ok(())
        } else {
            let t = self.peek();
            err(t.s, t.e, SyntaxError::Expected(what))
        }
    }

    /// The token just consumed.
    fn prev(&self) -> Option<Tok> {
        self.pos
            .checked_sub(1)
            .and_then(|i| self.toks.get(i))
            .copied()
    }

    /// One more level of nesting, refused past [`MAX_TEMPLATE_DEPTH`].
    fn enter(&mut self) -> R<()> {
        self.depth += 1;
        if self.depth > MAX_TEMPLATE_DEPTH {
            let t = self.peek();
            return err(t.s, t.e, SyntaxError::TooDeep);
        }
        Ok(())
    }

    fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    /// Whether the next token can start an expression (for the optional
    /// operands of `return`, `break`, and `yield`).
    fn starts_expr(&self) -> bool {
        let t = self.peek();
        match t.t {
            T::Int(_)
            | T::Float(_)
            | T::Str(_)
            | T::Char(_)
            | T::Label
            | T::Free(_)
            | T::Hole { splice: false, .. } => true,
            T::P(c) => matches!(c, b'(' | b'[' | b'{'),
            T::Ident => !matches!(self.word(t), "else" | "in" | "from"),
            T::Hole { splice: true, .. } | T::Arrow | T::Eof => false,
        }
    }

    // ------------------------------------------------------------- scopes

    fn scope_mark(&self) -> (usize, usize) {
        (self.scope_log.len(), self.label_log.len())
    }

    fn scope_pop(&mut self, mark: (usize, usize)) {
        while self.scope_log.len() > mark.0 {
            if let Some(name) = self.scope_log.pop() {
                if let Some(stack) = self.scope.get_mut(name) {
                    let _ = stack.pop();
                }
            }
        }
        while self.label_log.len() > mark.1 {
            if let Some(name) = self.label_log.pop() {
                if let Some(stack) = self.labels.get_mut(name) {
                    let _ = stack.pop();
                }
            }
        }
    }

    fn bind_name(&mut self, name: &'a str, b: TBinder) {
        self.scope.entry(name).or_default().push(b);
        self.scope_log.push(name);
    }

    /// Moves the binders pending since `from` into scope, refusing a name
    /// bound twice among them.
    fn commit(&mut self, from: usize) -> R<()> {
        let mut fresh: Vec<(&'a str, TBinder, u32, u32)> = self.pending.split_off(from);
        let mut sorted: Vec<(&'a str, u32, u32)> =
            fresh.iter().map(|&(n, _, s, e)| (n, s, e)).collect();
        sorted.sort_by(|a, b| a.0.cmp(b.0).then(a.1.cmp(&b.1)));
        if let Some(w) = sorted.windows(2).find(|w| w[0].0 == w[1].0) {
            return err(w[1].1, w[1].2, SyntaxError::BoundTwice);
        }
        for (name, b, _, _) in fresh.drain(..) {
            self.bind_name(name, b);
        }
        Ok(())
    }

    fn lookup(&self, name: &str) -> Option<TBinder> {
        self.scope.get(name).and_then(|s| s.last().copied())
    }

    /// A binder name: an identifier that is not reserved.
    fn binder_name(&mut self) -> R<(&'a str, Tok)> {
        let t = self.peek();
        let word = self.word(t);
        if t.t != T::Ident || reserved(word) {
            return err(t.s, t.e, SyntaxError::Expected("a binder name"));
        }
        let _ = self.bump();
        Ok((word, t))
    }

    // -------------------------------------------------------------- holes

    /// The hole a hole token names, declared on first use with `kind`.
    fn hole(&mut self, t: Tok, kind: HoleKind) -> R<u16> {
        let T::Hole { name, suffix, .. } = t.t else {
            return err(t.s, t.e, SyntaxError::Expected("a hole"));
        };
        let name = self
            .text
            .get(name.0 as usize..name.1 as usize)
            .unwrap_or("");
        if matches!(suffix, Suffix::Default(_)) && kind != HoleKind::Expr {
            return err(t.s, t.e, SyntaxError::HoleDefault);
        }
        if let Some(info) = self.holes.get(name).copied() {
            if info.kind != kind {
                return err(
                    t.s,
                    t.e,
                    SyntaxError::HoleKind {
                        first: info.kind,
                        here: kind,
                    },
                );
            }
            if suffix != Suffix::None {
                if info.suffixed && info.suffix != suffix {
                    return err(t.s, t.e, SyntaxError::HoleSuffix);
                }
                self.set_suffix(name, info, suffix);
            }
            return Ok(info.index);
        }
        let optional = suffix != Suffix::None;
        let index = match kind {
            HoleKind::Expr => match suffix {
                Suffix::None => self.b.expr_hole(name).0,
                Suffix::Optional => self.b.optional_expr_hole(name, None).0,
                Suffix::Default(lit) => self.b.optional_expr_hole(name, Some(lit)).0,
            },
            HoleKind::Pat => self.b.pat_hole(name, optional).0,
            HoleKind::Ty => self.b.ty_hole(name, optional).0,
            HoleKind::Stmts => self.b.stmts_hole(name).0,
            HoleKind::Args => self.b.args_hole(name).0,
            HoleKind::Label => self.b.label_hole(name).0,
        };
        let _ = self.holes.insert(
            name,
            HoleInfo {
                index,
                kind,
                suffixed: optional,
                suffix,
            },
        );
        Ok(index)
    }

    /// A later use's suffix makes an expression, pattern, or type hole
    /// optional (holes of the other kinds always are).
    fn set_suffix(&mut self, name: &'a str, info: HoleInfo, suffix: Suffix) {
        if let Some(decl) = self.b.t.holes.get_mut(usize::from(info.index)) {
            if matches!(info.kind, HoleKind::Expr | HoleKind::Pat | HoleKind::Ty) {
                decl.optional = true;
            }
            if let Suffix::Default(lit) = suffix {
                decl.default = Some(lit);
            }
        }
        let _ = self.holes.insert(
            name,
            HoleInfo {
                suffixed: true,
                suffix,
                ..info
            },
        );
    }

    fn hole_kind(&self, t: Tok) -> Option<HoleKind> {
        let T::Hole { name, .. } = t.t else {
            return None;
        };
        let name = self
            .text
            .get(name.0 as usize..name.1 as usize)
            .unwrap_or("");
        self.holes.get(name).map(|h| h.kind)
    }

    // -------------------------------------------------------- expressions

    fn expr(&mut self) -> R<TExpr> {
        self.enter()?;
        let e = self.expr_inner();
        self.leave();
        e
    }

    fn expr_inner(&mut self) -> R<TExpr> {
        let t = self.peek();
        if t.t == T::Ident {
            match self.word(t) {
                "return" => {
                    let _ = self.bump();
                    let v = self.opt_value()?;
                    return Ok(self.b.return_(v));
                }
                "throw" => {
                    let _ = self.bump();
                    let v = self.expr()?;
                    return Ok(self.b.throw(v));
                }
                "await" => {
                    let _ = self.bump();
                    let v = self.expr()?;
                    return Ok(self.b.await_(v));
                }
                "spawn" => {
                    let _ = self.bump();
                    let v = self.expr()?;
                    return Ok(self.b.spawn(v));
                }
                "yield" => return self.yield_expr(),
                "break" => return self.break_expr(),
                "continue" => return self.continue_expr(),
                "let_place" => return self.let_place(),
                _ => {}
            }
        }
        let lhs = self.postfix()?;
        if self.eat_p(b'=') {
            let rhs = self.expr()?;
            return Ok(self.b.assign(lhs, rhs));
        }
        Ok(lhs)
    }

    fn opt_value(&mut self) -> R<Option<TExpr>> {
        if self.starts_expr() {
            Ok(Some(self.expr()?))
        } else {
            Ok(None)
        }
    }

    fn yield_expr(&mut self) -> R<TExpr> {
        let _ = self.bump();
        if self.eat_kw("from") {
            let source = self.expr()?;
            return Ok(self.b.yield_from(source));
        }
        if !self.starts_expr() {
            return Ok(self.b.yield_(None));
        }
        let first = self.expr()?;
        if self.peek().t == T::Arrow {
            let _ = self.bump();
            let value = self.expr()?;
            return Ok(self.b.yield_kv(first, value));
        }
        Ok(self.b.yield_(Some(first)))
    }

    fn break_expr(&mut self) -> R<TExpr> {
        let _ = self.bump();
        let t = self.peek();
        let label = match t.t {
            T::Label => Some(self.label_use()?),
            T::Hole { splice: false, .. } if self.hole_kind(t) == Some(HoleKind::Label) => {
                let _ = self.bump();
                Some(LabelRef::Hole(LabelHole(self.hole(t, HoleKind::Label)?)))
            }
            _ => None,
        };
        let value = self.opt_value()?;
        Ok(self.b.break_(label, value))
    }

    fn continue_expr(&mut self) -> R<TExpr> {
        let _ = self.bump();
        let t = self.peek();
        let label = match t.t {
            T::Label => Some(self.label_use()?),
            T::Hole { splice: false, .. } => {
                let _ = self.bump();
                Some(LabelRef::Hole(LabelHole(self.hole(t, HoleKind::Label)?)))
            }
            _ => None,
        };
        Ok(self.b.continue_(label))
    }

    /// `'name` used as a jump target.
    fn label_use(&mut self) -> R<LabelRef> {
        let t = self.bump();
        let name = self.word(t).get(1..).unwrap_or("");
        match self.labels.get(name).and_then(|s| s.last().copied()) {
            Some(l) => Ok(LabelRef::Own(l)),
            None => err(t.s, t.e, SyntaxError::UnknownLabel),
        }
    }

    /// `let_place p = place in body`.
    fn let_place(&mut self) -> R<TExpr> {
        let _ = self.bump();
        let (name, _) = self.binder_name()?;
        let _ = self.expect_p(b'=', "`=` after the place alias")?;
        let place = self.postfix()?;
        self.expect_kw("in", "`in` after the place")?;
        let alias = self.b.binder(name);
        let mark = self.scope_mark();
        self.bind_name(name, alias);
        let body = self.expr();
        self.scope_pop(mark);
        let body = body?;
        Ok(self.b.let_place(alias, place, body))
    }

    fn postfix(&mut self) -> R<TExpr> {
        let mut e = self.primary()?;
        loop {
            if self.is_p(b'(') {
                let _ = self.bump();
                let args = self.list(b')')?;
                e = self.b.call(e, &args);
            } else if self.is_p(b'.') {
                let _ = self.bump();
                let t = self.peek();
                if t.t != T::Ident {
                    return err(t.s, t.e, SyntaxError::Expected("a field or method name"));
                }
                let _ = self.bump();
                let name = self.word(t);
                if self.is_p(b'(') {
                    let _ = self.bump();
                    let args = self.list(b')')?;
                    e = self.b.method(e, name, &args);
                } else {
                    e = self.b.field(e, name);
                }
            } else if self.is_p(b'[') {
                let _ = self.bump();
                let i = self.expr()?;
                let _ = self.expect_p(b']', "`]` closing the index")?;
                e = self.b.index(e, i);
            } else {
                return Ok(e);
            }
        }
    }

    /// Comma-separated expressions up to `close` (consumed); `${...name}`
    /// splices an argument-list hole.
    fn list(&mut self, close: u8) -> R<Vec<TExpr>> {
        let mut out = Vec::new();
        loop {
            if self.eat_p(close) {
                return Ok(out);
            }
            let t = self.peek();
            if let T::Hole { splice: true, .. } = t.t {
                let _ = self.bump();
                let h = self.hole(t, HoleKind::Args)?;
                out.push(self.b.splice(ArgsHole(h)));
            } else {
                out.push(self.expr()?);
            }
            if !self.eat_p(b',') {
                let t = self.peek();
                if t.t != T::P(close) {
                    return err(t.s, t.e, SyntaxError::Expected("`,` or a closing bracket"));
                }
            }
        }
    }

    fn primary(&mut self) -> R<TExpr> {
        let t = self.peek();
        match t.t {
            T::Int(v) => {
                let _ = self.bump();
                Ok(self.b.lit(TLit::Int(v)))
            }
            T::Float(v) => {
                let _ = self.bump();
                Ok(self.b.lit(TLit::Float(v)))
            }
            T::Char(c) => {
                let _ = self.bump();
                Ok(self.b.lit(TLit::Char(c)))
            }
            T::Str(i) => {
                let _ = self.bump();
                let text = self.strs.get(i as usize).map_or("", String::as_str);
                let text: String = String::from(text);
                Ok(self.b.str(&text))
            }
            T::Hole { splice: true, .. } => err(t.s, t.e, SyntaxError::SpliceHere),
            T::Hole { .. } => {
                let _ = self.bump();
                if self.is_kw("loop") {
                    let h = self.hole(t, HoleKind::Label)?;
                    return self.loop_expr(Some(LabelRef::Hole(LabelHole(h))));
                }
                let h = self.hole(t, HoleKind::Expr)?;
                Ok(self.b.hole(ExprHole(h)))
            }
            T::Label => self.labeled(),
            T::Free(kind) => {
                let _ = self.bump();
                let path = self.word(t).split_once(':').map_or("", |(_, p)| p);
                match kind {
                    Free::Extern => Ok(self.b.extern_(path)),
                    Free::Global => {
                        check_path(path, t)?;
                        Ok(self.b.global(path))
                    }
                    Free::Apply => self.apply(path, t),
                }
            }
            T::P(b'(') => self.paren(),
            T::P(b'[') => {
                let _ = self.bump();
                let elems = self.list(b']')?;
                Ok(self.b.array(&elems))
            }
            T::P(b'{') => self.block(),
            T::Ident => self.word_expr(t),
            _ => err(t.s, t.e, SyntaxError::Expected("an expression")),
        }
    }

    fn word_expr(&mut self, t: Tok) -> R<TExpr> {
        let word = self.word(t);
        match word {
            "true" | "false" => {
                let _ = self.bump();
                Ok(self.b.lit(TLit::Bool(word == "true")))
            }
            "null" => {
                let _ = self.bump();
                Ok(self.b.lit(TLit::Null))
            }
            "if" => self.if_expr(),
            "match" => self.match_expr(),
            "loop" => self.loop_expr(None),
            "closure" => self.closure(),
            _ => {
                let next = self.peek_at(1).t;
                if matches!(next, T::P(b'(' | b'<')) {
                    if let Some(named) = op_named(word) {
                        return self.op(named, t);
                    }
                }
                if let Some(b) = self.lookup(word) {
                    let _ = self.bump();
                    return Ok(self.b.var(b));
                }
                if reserved(word) {
                    return err(t.s, t.e, SyntaxError::Expected("an expression"));
                }
                if next == T::P(b'(') {
                    return err(t.s, t.e, SyntaxError::UnknownOp);
                }
                err(t.s, t.e, SyntaxError::UnknownName)
            }
        }
    }

    /// `(e)`, `()`, `(a,)`, `(a, b)`.
    fn paren(&mut self) -> R<TExpr> {
        let _ = self.bump();
        if self.eat_p(b')') {
            return Ok(self.b.tuple(&[]));
        }
        if let T::Hole { splice: true, .. } = self.peek().t {
            let elems = self.list(b')')?;
            return Ok(self.b.tuple(&elems));
        }
        let first = self.expr()?;
        if self.eat_p(b')') {
            return Ok(first);
        }
        let _ = self.expect_p(b',', "`,` or `)`")?;
        let mut elems = alloc::vec![first];
        elems.extend(self.list(b')')?);
        Ok(self.b.tuple(&elems))
    }

    /// `'name loop { … }` or `'name { … }`.
    fn labeled(&mut self) -> R<TExpr> {
        let t = self.bump();
        let name = self.word(t).get(1..).unwrap_or("");
        let label = self.b.label(name);
        let mark = self.scope_mark();
        self.labels.entry(name).or_default().push(label);
        self.label_log.push(name);
        let out = if self.is_kw("loop") {
            self.loop_expr(Some(LabelRef::Own(label)))
        } else if self.is_p(b'{') {
            self.labeled_block(label)
        } else {
            let n = self.peek();
            err(
                n.s,
                n.e,
                SyntaxError::Expected("`loop` or a block after the label"),
            )
        };
        self.scope_pop(mark);
        out
    }

    fn labeled_block(&mut self, label: TLabel) -> R<TExpr> {
        self.enter()?;
        let parts = self.block_parts();
        self.leave();
        let (stmts, tail) = parts?;
        Ok(self.b.labeled_block(LabelRef::Own(label), &stmts, tail))
    }

    fn block(&mut self) -> R<TExpr> {
        self.enter()?;
        let parts = self.block_parts();
        self.leave();
        let (stmts, tail) = parts?;
        Ok(self.b.block(&stmts, tail))
    }

    /// Whether the next tokens start a block-like expression, which ends a
    /// statement without a `;`.
    fn starts_blocklike(&self) -> bool {
        let t = self.peek();
        match t.t {
            T::P(b'{') | T::Label => true,
            T::Ident => matches!(self.word(t), "if" | "match" | "loop"),
            T::Hole { splice: false, .. } => {
                let n = self.peek_at(1);
                n.t == T::Ident && self.word(n) == "loop"
            }
            _ => false,
        }
    }

    fn block_parts(&mut self) -> R<(Vec<TStmt>, Option<TExpr>)> {
        let _ = self.expect_p(b'{', "`{`")?;
        let mark = self.scope_mark();
        let out = self.stmts();
        self.scope_pop(mark);
        out
    }

    fn stmts(&mut self) -> R<(Vec<TStmt>, Option<TExpr>)> {
        let mut stmts = Vec::new();
        loop {
            if self.eat_p(b'}') {
                return Ok((stmts, None));
            }
            if self.eat_p(b';') {
                continue;
            }
            let t = self.peek();
            if t.t == T::Eof {
                return err(t.s, t.e, SyntaxError::Expected("`}` closing the block"));
            }
            if self.is_kw("let") {
                stmts.push(self.let_stmt()?);
                continue;
            }
            if self.eat_kw("defer") {
                let e = self.expr()?;
                stmts.push(self.b.defer(e));
                if !self.eat_p(b';') && !self.is_p(b'}') {
                    let n = self.peek();
                    return err(n.s, n.e, SyntaxError::Expected("`;` or `}`"));
                }
                continue;
            }
            if let T::Hole { splice: true, .. } = t.t {
                let _ = self.bump();
                let h = self.hole(t, HoleKind::Stmts)?;
                stmts.push(self.b.stmts(StmtsHole(h)));
                let _ = self.eat_p(b';');
                continue;
            }
            let blocky = self.starts_blocklike();
            // Every block-like form enters a nesting level of its own.
            let e = if blocky {
                self.blocklike()?
            } else {
                self.expr()?
            };
            if self.eat_p(b';') {
                stmts.push(self.b.expr_stmt(e));
            } else if self.eat_p(b'}') {
                return Ok((stmts, Some(e)));
            } else if blocky {
                stmts.push(self.b.expr_stmt(e));
            } else {
                let n = self.peek();
                return err(n.s, n.e, SyntaxError::Expected("`;` or `}`"));
            }
        }
    }

    /// A block-like expression in statement position: no postfix and no
    /// assignment follows it.
    fn blocklike(&mut self) -> R<TExpr> {
        let t = self.peek();
        match t.t {
            T::P(b'{') => self.block(),
            T::Label => self.labeled(),
            T::Hole { .. } => {
                let _ = self.bump();
                let h = self.hole(t, HoleKind::Label)?;
                self.loop_expr(Some(LabelRef::Hole(LabelHole(h))))
            }
            _ => match self.word(t) {
                "if" => self.if_expr(),
                "match" => self.match_expr(),
                _ => self.loop_expr(None),
            },
        }
    }

    /// `let pat (: ty)? (= init)?;` — the pattern's names are in scope after
    /// the statement.
    fn let_stmt(&mut self) -> R<TStmt> {
        let _ = self.bump();
        let from = self.pending.len();
        let pat = self.pat()?;
        let ty = if self.eat_p(b':') {
            Some(self.ty()?)
        } else {
            None
        };
        let init = if self.eat_p(b'=') {
            Some(self.expr()?)
        } else {
            None
        };
        let _ = self.expect_p(b';', "`;` after `let`")?;
        self.commit(from)?;
        Ok(self.b.let_(pat, ty, init))
    }

    fn if_expr(&mut self) -> R<TExpr> {
        self.enter()?;
        let out = self.if_inner();
        self.leave();
        out
    }

    fn if_inner(&mut self) -> R<TExpr> {
        let _ = self.bump();
        let cond = self.expr()?;
        if !self.is_p(b'{') {
            let t = self.peek();
            return err(
                t.s,
                t.e,
                SyntaxError::Expected("a block after the condition"),
            );
        }
        let then = self.block()?;
        let else_ = if self.eat_kw("else") {
            if self.is_kw("if") {
                Some(self.if_expr()?)
            } else if self.is_p(b'{') {
                Some(self.block()?)
            } else {
                let t = self.peek();
                return err(
                    t.s,
                    t.e,
                    SyntaxError::Expected("a block or `if` after `else`"),
                );
            }
        } else {
            None
        };
        Ok(self.b.if_(cond, then, else_))
    }

    fn match_expr(&mut self) -> R<TExpr> {
        let _ = self.bump();
        let scrutinee = self.expr()?;
        let _ = self.expect_p(b'{', "`{` opening the arms")?;
        let mut arms = Vec::new();
        loop {
            if self.eat_p(b'}') {
                break;
            }
            let from = self.pending.len();
            let pat = self.pat()?;
            let mark = self.scope_mark();
            if let Err(e) = self.commit(from) {
                self.scope_pop(mark);
                return Err(e);
            }
            let arm = self.arm_rest();
            self.scope_pop(mark);
            let (guard, body) = arm?;
            arms.push((pat, guard, body));
            if self.eat_p(b',') || self.is_p(b'}') {
                continue;
            }
            // A block-bodied arm needs no comma.
            if self.prev().is_some_and(|p| p.t == T::P(b'}')) {
                continue;
            }
            let t = self.peek();
            return err(t.s, t.e, SyntaxError::Expected("`,` or `}` after the arm"));
        }
        Ok(self.b.match_(scrutinee, &arms))
    }

    fn arm_rest(&mut self) -> R<(Option<TExpr>, TExpr)> {
        let guard = if self.eat_kw("if") {
            Some(self.expr()?)
        } else {
            None
        };
        let t = self.peek();
        if t.t != T::Arrow {
            return err(t.s, t.e, SyntaxError::Expected("`=>`"));
        }
        let _ = self.bump();
        let body = self.expr()?;
        Ok((guard, body))
    }

    /// `loop ([step: e])? { … }`, its label already parsed.
    fn loop_expr(&mut self, label: Option<LabelRef>) -> R<TExpr> {
        self.expect_kw("loop", "`loop`")?;
        let step = if self.eat_p(b'[') {
            self.expect_kw("step", "`step`")?;
            let _ = self.expect_p(b':', "`:` after `step`")?;
            let s = self.expr()?;
            let _ = self.expect_p(b']', "`]` closing the step")?;
            Some(s)
        } else {
            None
        };
        if !self.is_p(b'{') {
            let t = self.peek();
            return err(t.s, t.e, SyntaxError::Expected("the loop's block"));
        }
        let body = self.block()?;
        Ok(self.b.loop_(label, body, step))
    }

    /// `closure[mode](a, mut b) => body`; the mode is `none` (the default),
    /// `infer`, `by-ref`, `by-mut-ref`, or `by-value`.
    fn closure(&mut self) -> R<TExpr> {
        let _ = self.bump();
        let mut mode = None;
        if self.eat_p(b'[') {
            let first = self.peek();
            let mut word = String::new();
            loop {
                let t = self.peek();
                if t.t != T::Ident {
                    return err(t.s, t.e, SyntaxError::Expected("a capture mode"));
                }
                let _ = self.bump();
                word.push_str(self.word(t));
                if !self.eat_p(b'-') {
                    break;
                }
                word.push('-');
            }
            let end = self.prev().map_or(first.e, |p| p.e);
            mode = match word.as_str() {
                "none" => None,
                "infer" => Some(CaptureMode::Infer),
                "by-ref" => Some(CaptureMode::ByRef),
                "by-mut-ref" => Some(CaptureMode::ByMutRef),
                "by-value" => Some(CaptureMode::ByValue),
                _ => {
                    return err(
                        first.s,
                        end,
                        SyntaxError::Expected(
                            "`none`, `infer`, `by-ref`, `by-mut-ref`, or `by-value`",
                        ),
                    );
                }
            };
            let _ = self.expect_p(b']', "`]` closing the capture mode")?;
        }
        let _ = self.expect_p(b'(', "`(` opening the parameters")?;
        let from = self.pending.len();
        let mut params = Vec::new();
        loop {
            if self.eat_p(b')') {
                break;
            }
            let mutable = self.eat_kw("mut");
            let (name, t) = self.binder_name()?;
            let b = if mutable {
                self.b.mutable_binder(name)
            } else {
                self.b.binder(name)
            };
            self.pending.push((name, b, t.s, t.e));
            params.push(b);
            if !self.eat_p(b',') && !self.is_p(b')') {
                let n = self.peek();
                return err(n.s, n.e, SyntaxError::Expected("`,` or `)`"));
            }
        }
        let t = self.peek();
        if t.t != T::Arrow {
            return err(
                t.s,
                t.e,
                SyntaxError::Expected("`=>` before the closure body"),
            );
        }
        let _ = self.bump();
        let mark = self.scope_mark();
        if let Err(e) = self.commit(from) {
            self.scope_pop(mark);
            return Err(e);
        }
        let body = self.expr();
        self.scope_pop(mark);
        Ok(self.b.closure(&params, body?, mode))
    }

    /// `name(args)` or `name<policy, …>(args)`.
    fn op(&mut self, named: Named, name_tok: Tok) -> R<TExpr> {
        let _ = self.bump();
        let mut target: Option<Prim> = None;
        let mut policy = hir_lang::Policy::NONE;
        let mut written = false;
        let mut bad: Option<Tok> = None;
        if self.eat_p(b'<') {
            loop {
                let key = self.peek();
                if key.t != T::Ident {
                    return err(
                        key.s,
                        key.e,
                        SyntaxError::Expected("a policy or a target type"),
                    );
                }
                let _ = self.bump();
                if self.eat_p(b'=') {
                    let value = self.peek();
                    if value.t != T::Ident {
                        return err(value.s, value.e, SyntaxError::Expected("a policy value"));
                    }
                    let _ = self.bump();
                    let ok = set_policy(&mut policy, self.word(key), self.word(value));
                    if !ok && bad.is_none() {
                        bad = Some(Tok {
                            t: T::Ident,
                            s: key.s,
                            e: value.e,
                        });
                    }
                    written = true;
                } else {
                    match prim_named(self.word(key)) {
                        Some(p) if target.is_none() => target = Some(p),
                        _ => {
                            if bad.is_none() {
                                bad = Some(key);
                            }
                        }
                    }
                }
                if self.eat_p(b'>') {
                    break;
                }
                let _ = self.expect_p(b',', "`,` or `>`")?;
            }
        }
        let kind = match (named, target) {
            (Named::Plain(k), None) => k,
            (Named::Conv(make), Some(p)) => make(p),
            (Named::Plain(_), Some(_)) => {
                return err(name_tok.s, name_tok.e, SyntaxError::BadPolicy);
            }
            (Named::Conv(_), None) => {
                return err(name_tok.s, name_tok.e, SyntaxError::UnknownOp);
            }
        };
        if let Some(b) = bad {
            return err(b.s, b.e, SyntaxError::BadPolicy);
        }
        let _ = self.expect_p(b'(', "`(` opening the operands")?;
        let mut args = Vec::new();
        loop {
            if self.eat_p(b')') {
                break;
            }
            let t = self.peek();
            if let T::Hole { splice: true, .. } = t.t {
                return err(t.s, t.e, SyntaxError::SpliceHere);
            }
            args.push(self.expr()?);
            if !self.eat_p(b',') && !self.is_p(b')') {
                let n = self.peek();
                return err(n.s, n.e, SyntaxError::Expected("`,` or `)`"));
            }
        }
        if args.len() != kind.arity() {
            return err(
                name_tok.s,
                name_tok.e,
                SyntaxError::Arity {
                    expected: kind.arity(),
                    found: args.len(),
                },
            );
        }
        if !written {
            return Ok(self.b.op(kind, &args));
        }
        // Exact: the written fields, OPS defaults for the rest; a field the
        // op does not consult is refused.
        let defaults = kind.default_policy();
        let fits = (policy.overflow.is_none() || defaults.overflow.is_some())
            && (policy.div_zero.is_none() || defaults.div_zero.is_some())
            && (policy.shift.is_none() || defaults.shift.is_some())
            && (policy.float_to_int.is_none() || defaults.float_to_int.is_some());
        if !fits {
            return err(name_tok.s, name_tok.e, SyntaxError::BadPolicy);
        }
        let op = Op {
            kind,
            policy: hir_lang::Policy {
                overflow: policy.overflow.or(defaults.overflow),
                div_zero: policy.div_zero.or(defaults.div_zero),
                shift: policy.shift.or(defaults.shift),
                float_to_int: policy.float_to_int.or(defaults.float_to_int),
            },
        };
        Ok(self.b.op_with(op, &args))
    }

    /// `apply:name(args)`: one argument per hole of the applied template, of
    /// that hole's kind; trailing optional holes may be left out and `_`
    /// leaves an optional hole absent.
    fn apply(&mut self, name: &str, t: Tok) -> R<TExpr> {
        let Some((id, callee)) = self.registry.and_then(|r| r.find(name)) else {
            return err(t.s, t.e, SyntaxError::UnknownTemplate);
        };
        let _ = self.expect_p(b'(', "`(` after the template name")?;
        let mark = self.scope_mark();
        let out = self.apply_args(callee);
        self.scope_pop(mark);
        let args = out?;
        Ok(self.b.apply(id, &args))
    }

    fn apply_args(&mut self, callee: &Template) -> R<Vec<TArg>> {
        let mut args = Vec::with_capacity(callee.holes.len());
        let mut closed = false;
        for (i, hole) in callee.holes.iter().enumerate() {
            let t = self.peek();
            if closed || t.t == T::P(b')') {
                if !hole.optional {
                    return err(t.s, t.e, SyntaxError::ApplyArgs);
                }
                args.push(TArg::Absent);
                closed = true;
                continue;
            }
            let next = self.peek_at(1).t;
            let arg = if t.t == T::Ident
                && self.word(t) == "_"
                && matches!(next, T::P(b',' | b')'))
                && hole.kind != HoleKind::Pat
            {
                let _ = self.bump();
                if !hole.optional {
                    return err(t.s, t.e, SyntaxError::ApplyArgs);
                }
                TArg::Absent
            } else {
                self.apply_arg(hole.kind, t)?
            };
            args.push(arg);
            if i + 1 < callee.holes.len() && !self.eat_p(b',') && !self.is_p(b')') {
                let n = self.peek();
                return err(n.s, n.e, SyntaxError::Expected("`,` or `)`"));
            }
        }
        let _ = self.eat_p(b',');
        let t = self.peek();
        if t.t != T::P(b')') {
            return err(t.s, t.e, SyntaxError::ApplyArgs);
        }
        let _ = self.bump();
        Ok(args)
    }

    fn apply_arg(&mut self, kind: HoleKind, t: Tok) -> R<TArg> {
        match kind {
            HoleKind::Expr => {
                if let T::Hole { splice: true, .. } = t.t {
                    return err(t.s, t.e, SyntaxError::ApplyArgs);
                }
                Ok(TArg::Expr(self.expr()?))
            }
            HoleKind::Pat => {
                // Its names are in scope for the remaining arguments.
                let from = self.pending.len();
                let p = self.pat()?;
                self.commit(from)?;
                Ok(TArg::Pat(p))
            }
            HoleKind::Ty => Ok(TArg::Ty(self.ty()?)),
            HoleKind::Stmts | HoleKind::Args => match t.t {
                T::Hole { splice: true, .. } => {
                    let _ = self.bump();
                    let h = self.hole(t, kind)?;
                    Ok(if kind == HoleKind::Stmts {
                        TArg::Stmts(StmtsHole(h))
                    } else {
                        TArg::Args(ArgsHole(h))
                    })
                }
                _ => err(t.s, t.e, SyntaxError::ApplyArgs),
            },
            HoleKind::Label => match t.t {
                T::Label => Ok(TArg::Label(self.label_use()?)),
                T::Hole { splice: false, .. } => {
                    let _ = self.bump();
                    let h = self.hole(t, HoleKind::Label)?;
                    Ok(TArg::Label(LabelRef::Hole(LabelHole(h))))
                }
                _ => err(t.s, t.e, SyntaxError::ApplyArgs),
            },
        }
    }

    // ----------------------------------------------------------- patterns

    fn pat(&mut self) -> R<TPat> {
        self.enter()?;
        let p = self.pat_inner();
        self.leave();
        p
    }

    fn pat_inner(&mut self) -> R<TPat> {
        let t = self.peek();
        match t.t {
            T::Int(v) => {
                let _ = self.bump();
                Ok(self.b.lit_pat(TLit::Int(v)))
            }
            T::Float(v) => {
                let _ = self.bump();
                Ok(self.b.lit_pat(TLit::Float(v)))
            }
            T::Char(c) => {
                let _ = self.bump();
                Ok(self.b.lit_pat(TLit::Char(c)))
            }
            T::Str(_) => err(
                t.s,
                t.e,
                SyntaxError::Unsupported("string literal patterns"),
            ),
            T::Hole { splice: true, .. } => err(t.s, t.e, SyntaxError::SpliceHere),
            T::Hole { .. } => {
                let _ = self.bump();
                let h = self.hole(t, HoleKind::Pat)?;
                Ok(self.b.pat(PatHole(h)))
            }
            T::P(b'(') => {
                let _ = self.bump();
                let (elems, trailing) = self.pats(b')')?;
                if let ([only], false) = (elems.as_slice(), trailing) {
                    return Ok(*only);
                }
                Ok(self.b.tuple_pat(&elems))
            }
            T::Free(Free::Global) => {
                let _ = self.bump();
                let path = self.word(t).split_once(':').map_or("", |(_, p)| p);
                check_path(path, t)?;
                let elems = if self.eat_p(b'(') {
                    self.pats(b')')?.0
                } else {
                    Vec::new()
                };
                Ok(self.b.ctor_pat(path, &elems))
            }
            T::Ident => self.word_pat(t),
            _ => err(t.s, t.e, SyntaxError::Expected("a pattern")),
        }
    }

    fn word_pat(&mut self, t: Tok) -> R<TPat> {
        let word = self.word(t);
        match word {
            "_" => {
                let _ = self.bump();
                Ok(self.b.wild())
            }
            "true" | "false" => {
                let _ = self.bump();
                Ok(self.b.lit_pat(TLit::Bool(word == "true")))
            }
            "null" => {
                let _ = self.bump();
                Ok(self.b.lit_pat(TLit::Null))
            }
            "mut" => {
                let _ = self.bump();
                let (name, nt) = self.binder_name()?;
                let b = self.b.mutable_binder(name);
                self.pending.push((name, b, nt.s, nt.e));
                Ok(self.b.bind(b))
            }
            _ if reserved(word) => err(t.s, t.e, SyntaxError::Expected("a pattern")),
            _ => {
                let path_follows =
                    self.peek_at(1).t == T::P(b':') && self.peek_at(2).t == T::P(b':');
                if path_follows || self.peek_at(1).t == T::P(b'(') {
                    let path = self.path_text()?;
                    let elems = if self.eat_p(b'(') {
                        self.pats(b')')?.0
                    } else {
                        Vec::new()
                    };
                    return Ok(self.b.ctor_pat(&path, &elems));
                }
                let _ = self.bump();
                let b = self.b.binder(word);
                self.pending.push((word, b, t.s, t.e));
                Ok(self.b.bind(b))
            }
        }
    }

    /// Patterns up to `close` (consumed), and whether a trailing comma was
    /// written (`(p,)` is a one-tuple).
    fn pats(&mut self, close: u8) -> R<(Vec<TPat>, bool)> {
        let mut out = Vec::new();
        let mut trailing = false;
        loop {
            if self.eat_p(close) {
                return Ok((out, trailing));
            }
            out.push(self.pat()?);
            trailing = self.eat_p(b',');
            if !trailing && !self.is_p(close) {
                let n = self.peek();
                return err(n.s, n.e, SyntaxError::Expected("`,` or a closing bracket"));
            }
        }
    }

    /// `a::b::c` as written, from identifiers separated by `::`.
    fn path_text(&mut self) -> R<String> {
        let mut out = String::new();
        loop {
            let t = self.peek();
            if t.t != T::Ident {
                return err(t.s, t.e, SyntaxError::Expected("a path segment"));
            }
            let _ = self.bump();
            out.push_str(self.word(t));
            if self.is_p(b':') && self.peek_at(1).t == T::P(b':') {
                let _ = self.bump();
                let _ = self.bump();
                out.push_str("::");
            } else {
                return Ok(out);
            }
        }
    }

    // -------------------------------------------------------------- types

    fn ty(&mut self) -> R<TTy> {
        self.enter()?;
        let t = self.ty_inner();
        self.leave();
        t
    }

    fn ty_inner(&mut self) -> R<TTy> {
        let first = self.ty_atom()?;
        let sep = match self.peek().t {
            T::P(c @ (b'|' | b'&')) => c,
            _ => return Ok(first),
        };
        let mut members = alloc::vec![first];
        while self.eat_p(sep) {
            members.push(self.ty_atom()?);
        }
        let t = self.peek();
        if let T::P(b'|' | b'&') = t.t {
            return err(
                t.s,
                t.e,
                SyntaxError::Expected("parentheses around a union inside an intersection"),
            );
        }
        Ok(if sep == b'|' {
            self.b.union_ty(&members)
        } else {
            self.b.intersection_ty(&members)
        })
    }

    fn ty_atom(&mut self) -> R<TTy> {
        let t = self.peek();
        match t.t {
            T::Hole { splice: true, .. } => err(t.s, t.e, SyntaxError::SpliceHere),
            T::Hole { .. } => {
                let _ = self.bump();
                let h = self.hole(t, HoleKind::Ty)?;
                Ok(self.b.ty(TyHole(h)))
            }
            T::Free(Free::Global) => {
                let _ = self.bump();
                let path = self.word(t).split_once(':').map_or("", |(_, p)| p);
                check_path(path, t)?;
                Ok(self.b.named_ty(path))
            }
            T::P(b'(') => {
                let _ = self.bump();
                let inner = self.ty()?;
                let _ = self.expect_p(b')', "`)`")?;
                Ok(inner)
            }
            T::Ident => {
                let word = self.word(t);
                if word == "_" {
                    let _ = self.bump();
                    return Ok(self.b.infer_ty());
                }
                if word == "any" {
                    let _ = self.bump();
                    return Ok(self.b.any_ty());
                }
                if let Some(p) = prim_named(word) {
                    let _ = self.bump();
                    return Ok(self.b.prim(p));
                }
                if reserved(word) {
                    return err(t.s, t.e, SyntaxError::Expected("a type"));
                }
                let path = self.path_text()?;
                Ok(self.b.named_ty(&path))
            }
            _ => err(t.s, t.e, SyntaxError::Expected("a type")),
        }
    }
}

/// Sets one policy field from its written name and value; `false` for an
/// unknown name or value, or a field written twice.
fn set_policy(policy: &mut hir_lang::Policy, key: &str, value: &str) -> bool {
    match key {
        "overflow" if policy.overflow.is_none() => {
            policy.overflow = match value {
                "error" => Some(Overflow::Error),
                "wrap" => Some(Overflow::Wrap),
                "trap" => Some(Overflow::Trap),
                "promote" => Some(Overflow::Promote),
                _ => return false,
            };
        }
        "div_zero" if policy.div_zero.is_none() => {
            policy.div_zero = match value {
                "error" => Some(DivZero::Error),
                "trap" => Some(DivZero::Trap),
                _ => return false,
            };
        }
        "shift" if policy.shift.is_none() => {
            policy.shift = match value {
                "error" => Some(Shift::Error),
                "mask" => Some(Shift::Mask),
                "saturate" => Some(Shift::Saturate),
                _ => return false,
            };
        }
        "float_to_int" if policy.float_to_int.is_none() => {
            policy.float_to_int = match value {
                "error" => Some(FloatToInt::Error),
                "saturate" => Some(FloatToInt::Saturate),
                _ => return false,
            };
        }
        _ => return false,
    }
    true
}

/// A `global:` path: identifiers separated by `::`.
fn check_path(path: &str, t: Tok) -> R<()> {
    let ok = !path.is_empty()
        && path.split("::").all(|seg| {
            let mut bytes = seg.bytes();
            bytes.next().is_some_and(ident_start) && bytes.all(ident_char)
        });
    if ok {
        Ok(())
    } else {
        err(t.s, t.e, SyntaxError::Expected("a path like `a::b`"))
    }
}

#[cfg(test)]
mod tests;
