//! Lowering a token on its own: names, variables, literals, wildcards, type
//! keywords, and parameter-list markers.

use hir_lang::{
    BindMode, BinderKind, Bound, Expr, FieldDef, FieldInit, FieldPat, FloatLit, GenericArg,
    GenericParam, Ident, IntLit, List, Lit, Name, Ns, Origin, Param, ParamKind, Pat, PatId, Prim,
    Segment, Shape, Span, Ty, TyId, Variant,
};
use intern_lang::Symbol;
use syntax_lang::{Token, TokenKind};

use super::{PatSite, Run, Val, Want};
use crate::mapping::{
    Escapes, IdentPatterns, LegacyOctal, Modifier, Separators, TokenRole, TypeName,
};
use crate::problem::ProblemKind;

/// The most digits a non-decimal integer literal too large for 64 bits may
/// have to be converted to its exact decimal big integer (the conversion is
/// quadratic in the length).
const MAX_BIG_DIGITS: usize = 1024;

/// A decoded literal.
pub(crate) enum Decoded {
    Lit(Lit),
    Bad,
}

impl<'t, K> Run<'_, 't, K>
where
    K: Copy + Ord + TokenKind,
{
    pub(crate) fn lower_token(&mut self, tok: &'t Token<K>, want: Want) {
        let span = tok.span();
        let role = self.m.role_for(tok.kind());
        let modifier = self.m.modifier_for(tok.kind());
        let v = self.token_val(span, role, modifier, want);
        let place = matches!(role, Some(TokenRole::Name | TokenRole::Var { .. }));
        self.push_place(v, span, place);
    }

    fn token_val(
        &mut self,
        span: Span,
        role: Option<TokenRole>,
        modifier: Option<Modifier>,
        want: Want,
    ) -> Val {
        // Parameter-list markers are modifiers, not roles.
        if want == Want::Param {
            match modifier {
                Some(Modifier::NamedOnly) => return Val::Marker(ParamKind::NamedOnly),
                Some(Modifier::PositionalOnly) => return Val::Marker(ParamKind::PositionalOnly),
                _ => {}
            }
        }
        let Some(role) = role else {
            self.report(ProblemKind::Unmapped, span);
            return self.error_val(want, span);
        };
        match want {
            Want::Expr | Want::Stmt | Want::Arg | Want::Entry => {
                match self.token_expr(role, span) {
                    Some((e, place)) => self.adapt_expr(e, place, want, span),
                    None => self.wrong_place(want, span),
                }
            }
            Want::GArg => match role {
                TokenRole::Lifetime { .. } => {
                    let (sym, _) = self.token_name(span, role);
                    let p = self.name_path(sym, span, Ns::Region);
                    Val::GArg(GenericArg::Region(p))
                }
                TokenRole::Name | TokenRole::Type(_) | TokenRole::Wild => {
                    match self.token_ty(role, span) {
                        Some(t) => Val::GArg(GenericArg::Ty(t)),
                        None => self.wrong_place(want, span),
                    }
                }
                _ => match self.token_expr(role, span) {
                    Some((e, _)) => Val::GArg(GenericArg::Const(e)),
                    None => self.wrong_place(want, span),
                },
            },
            Want::Pat(site) => match self.token_pat(role, span, site) {
                Some(v) => v,
                None => self.wrong_place(want, span),
            },
            Want::Param => {
                let site = PatSite {
                    param: true,
                    mutable: false,
                    arm: false,
                    bound: false,
                    persist: false,
                };
                match self.token_pat(role, span, site) {
                    Some(Val::Pat(p)) => Val::Param(Param::new(p), span, (0, 0)),
                    _ => self.wrong_place(want, span),
                }
            }
            Want::Ty => match self.token_ty(role, span) {
                Some(t) => Val::Ty(t),
                None => self.wrong_place(want, span),
            },
            Want::Path(ns) => match role {
                TokenRole::Name | TokenRole::Var { .. } | TokenRole::Lifetime { .. } => {
                    let (sym, _) = self.token_name(span, role);
                    Val::Path(self.name_path(sym, span, ns))
                }
                _ => self.wrong_place(want, span),
            },
            Want::Segment => match role {
                TokenRole::Name | TokenRole::Var { .. } => {
                    let (sym, _) = self.token_name(span, role);
                    Val::Seg(Segment::new(Name::new(sym), Origin::new(span)))
                }
                _ => self.wrong_place(want, span),
            },
            Want::Capture => match role {
                TokenRole::Name | TokenRole::Var { .. } => {
                    let (sym, _) = self.token_name(span, role);
                    self.capture(sym, span, hir_lang::CaptureMode::ByValue)
                }
                _ => self.wrong_place(want, span),
            },
            Want::Field => match role {
                TokenRole::Name | TokenRole::Var { .. } => {
                    let (sym, _) = self.token_name(span, role);
                    self.at(span);
                    self.nodes += 1;
                    Val::Field(self.b.field(FieldDef::named(Ident::new(sym, span))), true)
                }
                TokenRole::Type(_) => match self.token_ty(role, span) {
                    Some(t) => {
                        self.at(span);
                        self.nodes += 1;
                        Val::Field(
                            self.b.field(FieldDef {
                                ty: Some(t),
                                ..FieldDef::default()
                            }),
                            false,
                        )
                    }
                    None => self.wrong_place(want, span),
                },
                _ => self.wrong_place(want, span),
            },
            Want::Variant => match role {
                TokenRole::Name => {
                    let (sym, _) = self.token_name(span, role);
                    self.at(span);
                    self.nodes += 1;
                    Val::Variant(self.b.variant(Variant {
                        name: Ident::new(sym, span),
                        shape: Shape::Unit,
                        fields: List::EMPTY,
                        discriminant: None,
                    }))
                }
                _ => self.wrong_place(want, span),
            },
            Want::Generic => match role {
                TokenRole::Name | TokenRole::Lifetime { .. } => {
                    let (sym, _) = self.token_name(span, role);
                    let kind = if matches!(role, TokenRole::Lifetime { .. }) {
                        BinderKind::Region
                    } else {
                        BinderKind::TypeParam
                    };
                    self.at(span);
                    let b = self.binder(Name::new(sym), kind, false);
                    Val::Generic(GenericParam::new(b))
                }
                _ => self.wrong_place(want, span),
            },
            Want::Init => match role {
                TokenRole::Name | TokenRole::Var { .. } => {
                    let (sym, _) = self.token_name(span, role);
                    let value = self.name_expr(sym, span);
                    Val::Init(FieldInit {
                        name: Ident::new(sym, span),
                        value,
                    })
                }
                _ => self.wrong_place(want, span),
            },
            Want::FieldPat(site) => match role {
                TokenRole::Name | TokenRole::Var { .. } => {
                    let (sym, _) = self.token_name(span, role);
                    let pat = self.bind_pat(sym, span, site, BindMode::Value);
                    Val::FieldPat(FieldPat {
                        name: Ident::new(sym, span),
                        pat,
                    })
                }
                TokenRole::Rest => Val::Rest(None),
                _ => self.wrong_place(want, span),
            },
            Want::Root | Want::Member | Want::Arm | Want::Where | Want::Rule => {
                self.wrong_place(want, span)
            }
        }
    }

    fn wrong_place(&mut self, want: Want, span: Span) -> Val {
        self.report(
            ProblemKind::NotAllowedHere {
                expected: want.context(),
            },
            span,
        );
        self.error_val(want, span)
    }

    /// A name-like token's name, sigil stripped.
    pub(crate) fn token_name(&mut self, span: Span, role: TokenRole) -> (Symbol, Span) {
        let strip = match role {
            TokenRole::Var { sigil }
            | TokenRole::Label { sigil }
            | TokenRole::Lifetime { sigil } => usize::from(sigil),
            _ => 0,
        };
        let text = self.text_of(span);
        let name = text.get(strip..).unwrap_or("");
        (self.intern(name), span)
    }

    /// A token as an expression, and whether it is a place.
    fn token_expr(&mut self, role: TokenRole, span: Span) -> Option<(hir_lang::ExprId, bool)> {
        match role {
            TokenRole::Name | TokenRole::Var { .. } => {
                let (sym, _) = self.token_name(span, role);
                Some((self.name_expr(sym, span), true))
            }
            TokenRole::Wild
            | TokenRole::Rest
            | TokenRole::Label { .. }
            | TokenRole::Lifetime { .. } => None,
            TokenRole::Type(_) => None,
            _ => {
                let lit = self.token_lit(role, span, false);
                self.at(span);
                Some((
                    match lit {
                        Decoded::Lit(l) => self.ex(Expr::Lit(l)),
                        Decoded::Bad => self.ex(Expr::Err),
                    },
                    false,
                ))
            }
        }
    }

    /// A token as a pattern.
    fn token_pat(&mut self, role: TokenRole, span: Span, site: PatSite) -> Option<Val> {
        match role {
            TokenRole::Name | TokenRole::Var { .. } => {
                let (sym, _) = self.token_name(span, role);
                if site.bound {
                    let p = self.name_path(sym, span, Ns::Pattern);
                    return Some(Val::Pat(self.pt(Pat::Path(p))));
                }
                Some(Val::Pat(self.ident_pat(sym, span, site)))
            }
            TokenRole::Wild => {
                self.at(span);
                Some(Val::Pat(self.pt(Pat::Wild)))
            }
            TokenRole::Rest => Some(Val::Rest(None)),
            TokenRole::Label { .. } | TokenRole::Lifetime { .. } | TokenRole::Type(_) => None,
            _ => {
                let lit = self.token_lit(role, span, false);
                self.at(span);
                Some(Val::Pat(match lit {
                    Decoded::Lit(l) => self.pt(Pat::Lit(l)),
                    Decoded::Bad => self.pt(Pat::Err),
                }))
            }
        }
    }

    /// A bare identifier in a pattern: a binding, or (in a `match` arm, by
    /// policy) an identifier pattern the resolver decides.
    pub(crate) fn ident_pat(&mut self, sym: Symbol, span: Span, site: PatSite) -> PatId {
        if site.arm && self.m.policy.ident_patterns == IdentPatterns::Ident {
            let binder = self.pat_binder(sym, span, site);
            let path = self.name_path(sym, span, Ns::Pattern);
            return self.pt(Pat::Ident { binder, path });
        }
        self.bind_pat(sym, span, site, BindMode::Value)
    }

    /// `Pat::Bind` of a fresh (or, inside an or-pattern, shared) binder.
    pub(crate) fn bind_pat(
        &mut self,
        sym: Symbol,
        span: Span,
        site: PatSite,
        mode: BindMode,
    ) -> PatId {
        let binder = self.pat_binder(sym, span, site);
        self.at(span);
        self.pt(Pat::Bind {
            binder,
            mode,
            sub: None,
        })
    }

    /// The binder a pattern binds `sym` to: inside an or-pattern every
    /// alternative shares one binder per name. At a function-scoped site it
    /// is a hygienic temporary, assigned to the hoisted name by the
    /// pattern's consumer.
    pub(crate) fn pat_binder(
        &mut self,
        sym: Symbol,
        span: Span,
        site: PatSite,
    ) -> hir_lang::BinderId {
        if self.or_depth > 0 {
            if let Some(&b) = self.or_map.get(&sym) {
                return b;
            }
        }
        if site.persist && !site.param {
            let b = self.persist_binder(sym, span);
            if self.or_depth > 0 {
                let _ = self.or_map.insert(sym, b);
            }
            return b;
        }
        let kind = if site.param {
            BinderKind::Param
        } else {
            BinderKind::Local
        };
        self.at(span);
        let b = self.binder(Name::new(sym), kind, site.mutable);
        if site.param {
            self.declare_name(sym);
        }
        if self.or_depth > 0 {
            let _ = self.or_map.insert(sym, b);
        }
        b
    }

    /// An explicit capture of `sym`.
    pub(crate) fn capture(&mut self, sym: Symbol, span: Span, mode: hir_lang::CaptureMode) -> Val {
        let outer = self.name_path(sym, span, Ns::Value);
        let binder = self.binder(Name::new(sym), BinderKind::Capture, false);
        self.declare_name(sym);
        Val::Cap(hir_lang::Capture {
            outer,
            binder,
            mode,
        })
    }

    /// A token as a type.
    fn token_ty(&mut self, role: TokenRole, span: Span) -> Option<TyId> {
        let ty = match role {
            TokenRole::Type(name) => type_of(name),
            TokenRole::Wild => Ty::Infer,
            TokenRole::Name => {
                let text = self.text_of(span);
                match self.m.type_name(text) {
                    Some(name) => type_of(name),
                    None => {
                        let (sym, _) = self.token_name(span, role);
                        let p = self.name_path(sym, span, Ns::Type);
                        Ty::Path(p)
                    }
                }
            }
            _ => return None,
        };
        self.at(span);
        Some(self.ty(ty))
    }

    // -------------------------------------------------------------- literals

    /// Decodes a literal token, reporting problems. `negative` applies a
    /// preceding minus sign (patterns).
    pub(crate) fn token_lit(&mut self, role: TokenRole, span: Span, negative: bool) -> Decoded {
        let text = self.text_of(span);
        match role {
            TokenRole::True => Decoded::Lit(Lit::Bool(true)),
            TokenRole::False => Decoded::Lit(Lit::Bool(false)),
            TokenRole::Null => Decoded::Lit(Lit::Null),
            TokenRole::Int => self.number(text, span, negative, Some(false)),
            TokenRole::Float => self.number(text, span, negative, Some(true)),
            TokenRole::Number => self.number(text, span, negative, None),
            TokenRole::Str(esc) => {
                self.unquote(text, span, esc, false);
                self.str_lit(span)
            }
            TokenRole::StrPart(esc) => {
                self.decode_escapes(text, span, esc, false);
                self.str_lit(span)
            }
            TokenRole::RawStr => match raw_body(text) {
                Some(body) => {
                    self.text.clear();
                    self.text.extend_from_slice(body.as_bytes());
                    self.str_lit(span)
                }
                None => {
                    self.report(ProblemKind::InvalidLiteral, span);
                    Decoded::Bad
                }
            },
            TokenRole::Heredoc(esc) => match self.heredoc_body(text, span) {
                Some(body) => {
                    self.decode_escapes(&body, span, esc, false);
                    self.str_lit(span)
                }
                None => {
                    self.report(ProblemKind::InvalidLiteral, span);
                    Decoded::Bad
                }
            },
            TokenRole::Bytes(esc) => {
                self.unquote(text, span, esc, true);
                let bytes = core::mem::take(&mut self.text);
                let t = self.b.bytes(&bytes);
                self.text = bytes;
                Decoded::Lit(Lit::Bytes(t))
            }
            TokenRole::Char(esc) => {
                self.unquote(text, span, esc, false);
                let decoded = core::str::from_utf8(&self.text).ok().and_then(|s| {
                    let mut chars = s.chars();
                    match (chars.next(), chars.next()) {
                        (Some(c), None) => Some(c),
                        _ => None,
                    }
                });
                match decoded {
                    Some(c) => Decoded::Lit(Lit::Char(c)),
                    None => {
                        self.report(ProblemKind::InvalidLiteral, span);
                        Decoded::Bad
                    }
                }
            }
            _ => Decoded::Bad,
        }
    }

    /// The decoded text in `self.text` as a string literal (invalid UTF-8
    /// is reported).
    fn str_lit(&mut self, span: Span) -> Decoded {
        match core::str::from_utf8(&self.text) {
            Ok(s) => {
                let s = alloc::string::String::from(s);
                Decoded::Lit(Lit::Str(self.b.text(&s)))
            }
            Err(_) => {
                self.report(ProblemKind::InvalidLiteral, span);
                Decoded::Bad
            }
        }
    }

    /// A heredoc or nowdoc token's body with PHP 7.3+ indentation removed:
    /// `None` when the token is not `<<<ID` + newline + body + newline +
    /// indented `ID`. A body line indented less than the closing marker, or
    /// an indentation mixing tabs and spaces, is reported (and the line
    /// keeps what it has).
    fn heredoc_body(&mut self, text: &str, span: Span) -> Option<alloc::string::String> {
        let rest = text.strip_prefix("<<<")?;
        let first_nl = rest.find('\n')?;
        let opener = rest
            .get(..first_nl)?
            .trim_end_matches('\r')
            .trim_matches([' ', '\t']);
        let id = match opener.as_bytes().first() {
            Some(b'"') => opener.strip_prefix('"')?.strip_suffix('"')?,
            Some(b'\'') => opener.strip_prefix('\'')?.strip_suffix('\'')?,
            _ => opener,
        };
        if id.is_empty() {
            return None;
        }
        let after = rest.get(first_nl + 1..)?;
        // The closing line: indentation, then the identifier.
        let (body, close) = match after.rfind('\n') {
            Some(i) => (after.get(..i)?, after.get(i + 1..)?),
            None => ("", after),
        };
        let indent_len = close.len() - close.trim_start_matches([' ', '\t']).len();
        let indent = close.get(..indent_len)?;
        if close.get(indent_len..)?.trim_end_matches('\r') != id {
            return None;
        }
        if indent.contains(' ') && indent.contains('\t') {
            self.report(ProblemKind::InvalidLiteral, span);
        }
        let body = body.strip_suffix('\r').unwrap_or(body);
        let no_body = after.rfind('\n').is_none();
        let mut out = alloc::string::String::with_capacity(body.len());
        let mut reported = false;
        if !no_body {
            for (i, line) in body.split('\n').enumerate() {
                if i > 0 {
                    out.push('\n');
                }
                let bare = line.strip_suffix('\r').unwrap_or(line);
                if bare.is_empty() {
                    out.push_str(line);
                    continue;
                }
                match line.strip_prefix(indent) {
                    Some(l) => out.push_str(l),
                    None => {
                        if !reported {
                            reported = true;
                            self.report(ProblemKind::InvalidLiteral, span);
                        }
                        out.push_str(line.trim_start_matches([' ', '\t']));
                    }
                }
            }
        }
        Some(out)
    }

    /// Decodes a number per the language's literal table: radix prefixes,
    /// `_` separators, a leading zero, hexadecimal floats, a registered type
    /// suffix. An unsuffixed integer above `i64::MAX` is a float under
    /// `Overflow::Promote` (PHP); otherwise one beyond `u64` is an exact big
    /// integer.
    fn number(&mut self, text: &str, span: Span, negative: bool, float: Option<bool>) -> Decoded {
        let table = self.m.policy.literals;
        let promote = self.m.policy.overflow == hir_lang::Overflow::Promote;
        let (body, suffix) = self.split_suffix(text);
        if !separators_ok(body, table.separators) {
            self.report(ProblemKind::InvalidLiteral, span);
            return Decoded::Bad;
        }
        self.scratch.clear();
        self.scratch.extend(body.chars().filter(|&c| c != '_'));
        let digits = core::mem::take(&mut self.scratch);
        let out = self.number_digits(&digits, span, negative, float, suffix, promote);
        self.scratch = digits;
        out
    }

    fn number_digits(
        &mut self,
        digits: &str,
        span: Span,
        negative: bool,
        float: Option<bool>,
        suffix: Option<Prim>,
        promote: bool,
    ) -> Decoded {
        let table = self.m.policy.literals;
        let radix = radix_of(digits);
        let float_lit = |v: f64, suffix: Option<Prim>| {
            let v = if negative { -v } else { v };
            let mut lit = FloatLit::new(v);
            if let Some(p) = suffix {
                lit = lit.with_suffix(p);
            }
            Decoded::Lit(Lit::Float(lit))
        };
        // A hexadecimal float: `0x1.8p3`.
        if let Some((16, skip)) = radix {
            let mant = digits.get(skip..).unwrap_or("");
            if float != Some(false) && mant.bytes().any(|b| matches!(b, b'.' | b'p' | b'P')) {
                if !table.hex_floats {
                    self.report(ProblemKind::InvalidLiteral, span);
                    return Decoded::Bad;
                }
                return match hex_float(mant) {
                    Some(v) => float_lit(v, suffix),
                    None => {
                        self.report(ProblemKind::InvalidLiteral, span);
                        Decoded::Bad
                    }
                };
            }
        }
        let is_float = match float {
            Some(f) => f,
            None => radix.is_none() && digits.bytes().any(|b| b == b'.' || b == b'e' || b == b'E'),
        } || suffix.is_some_and(Prim::is_float);
        if is_float {
            return match digits.parse::<f64>() {
                Ok(v) if radix.is_none() => float_lit(v, suffix),
                _ => {
                    self.report(ProblemKind::InvalidLiteral, span);
                    Decoded::Bad
                }
            };
        }
        let (mut r, mut rest) = match radix {
            Some((r, skip)) => (r, digits.get(skip..).unwrap_or("")),
            None => (10, digits),
        };
        // `0777`: a leading zero followed by more digits.
        if radix.is_none() && rest.len() > 1 && rest.starts_with('0') {
            match table.legacy_octal {
                LegacyOctal::Decimal => {}
                LegacyOctal::Octal => {
                    r = 8;
                    rest = rest.get(1..).unwrap_or("");
                }
                LegacyOctal::Error => {
                    if rest.bytes().any(|b| b != b'0') {
                        self.report(ProblemKind::InvalidLiteral, span);
                        return Decoded::Bad;
                    }
                }
            }
        }
        if rest.is_empty() || !rest.chars().all(|c| c.is_digit(r)) {
            self.report(ProblemKind::InvalidLiteral, span);
            return Decoded::Bad;
        }
        let int_lit = |value: u64, suffix: Option<Prim>| {
            if negative && value == 0 {
                return Decoded::Lit(Lit::Int(IntLit::new(0)));
            }
            let mut lit = IntLit::new(value);
            lit.negative = negative;
            if let Some(p) = suffix {
                lit = lit.with_suffix(p);
            }
            Decoded::Lit(Lit::Int(lit))
        };
        let promoted = promote && suffix.is_none();
        match parse_u64(rest, r) {
            Some(value) if promoted && value > i64::MAX as u64 => {
                // PHP: integers are 64-bit signed; above that, the nearest
                // float (`as` rounds to nearest, ties to even).
                #[allow(clippy::cast_precision_loss)]
                float_lit(value as f64, None)
            }
            Some(value) => int_lit(value, suffix),
            None if promoted => {
                let v = if r == 10 {
                    rest.parse::<f64>().ok()
                } else {
                    pow2_digits_to_f64(rest, r)
                };
                match v {
                    Some(v) => float_lit(v, None),
                    None => {
                        self.report(ProblemKind::InvalidLiteral, span);
                        Decoded::Bad
                    }
                }
            }
            None => {
                // Too large for 64 bits: an exact big integer.
                let mut s = alloc::string::String::new();
                if negative {
                    s.push('-');
                }
                if r == 10 {
                    s.push_str(rest.trim_start_matches('0'));
                } else {
                    match to_decimal(rest, r) {
                        Some(d) => s.push_str(&d),
                        None => {
                            self.report(ProblemKind::InvalidLiteral, span);
                            return Decoded::Bad;
                        }
                    }
                }
                let t = self.b.text(&s);
                Decoded::Lit(Lit::BigInt(t))
            }
        }
    }

    /// Splits a registered type suffix off a number's text.
    fn split_suffix<'s>(&self, text: &'s str) -> (&'s str, Option<Prim>) {
        // Hex digits include letters, so a suffix is only recognised after a
        // non-hex number or when it cannot be a run of hex digits.
        let hex = text.starts_with("0x") || text.starts_with("0X");
        for (suffix, prim) in &self.m.suffixes {
            if hex && suffix.bytes().all(|b| b.is_ascii_hexdigit()) {
                continue;
            }
            if let Some(body) = text.strip_suffix(&**suffix) {
                if !body.is_empty() {
                    return (body.trim_end_matches('_'), Some(*prim));
                }
            }
        }
        (text, None)
    }

    /// Strips quotes (and a leading `b` for bytes) and decodes escapes into
    /// `self.text`.
    fn unquote(&mut self, text: &str, span: Span, esc: Escapes, bytes: bool) {
        let text = if bytes {
            text.strip_prefix(['b', 'B']).unwrap_or(text)
        } else {
            text
        };
        let inner = match text.chars().next() {
            Some(q @ ('"' | '\'' | '`')) => {
                let after = text.get(q.len_utf8()..).unwrap_or("");
                after.strip_suffix(q).unwrap_or(after)
            }
            _ => text,
        };
        self.decode_escapes(inner, span, esc, bytes);
    }

    /// Decodes the escapes of `inner` (no quotes) into `self.text`.
    pub(crate) fn decode_escapes(&mut self, inner: &str, span: Span, esc: Escapes, bytes: bool) {
        self.text.clear();
        let mut chars = inner.char_indices().peekable();
        let mut buf = [0u8; 4];
        while let Some((_, c)) = chars.next() {
            if c != '\\' || esc == Escapes::None {
                self.text
                    .extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                continue;
            }
            let Some(&(_, next)) = chars.peek() else {
                self.text.push(b'\\');
                break;
            };
            let simple = match next {
                '\\' => Some('\\'),
                '\'' => Some('\''),
                '"' => Some('"'),
                '`' => Some('`'),
                _ => None,
            };
            if esc == Escapes::Quotes {
                match simple {
                    Some(s) => {
                        let _ = chars.next();
                        self.text
                            .extend_from_slice(s.encode_utf8(&mut buf).as_bytes());
                    }
                    None => self.text.push(b'\\'),
                }
                continue;
            }
            let _ = chars.next();
            let decoded = match next {
                'n' => Some('\n'),
                'r' => Some('\r'),
                't' => Some('\t'),
                '0' => Some('\0'),
                '$' => Some('$'),
                'x' => {
                    let hi = chars.next().and_then(|(_, c)| c.to_digit(16));
                    let lo = chars.next().and_then(|(_, c)| c.to_digit(16));
                    match (hi, lo) {
                        (Some(h), Some(l)) => {
                            let v = h * 16 + l;
                            if bytes {
                                self.text.push(u8::try_from(v).unwrap_or(0));
                                continue;
                            }
                            char::from_u32(v)
                        }
                        _ => None,
                    }
                }
                'u' => {
                    let mut value: Option<u32> = None;
                    if chars.peek().map(|&(_, c)| c) == Some('{') {
                        let _ = chars.next();
                        let mut v: u32 = 0;
                        let mut digits = 0;
                        let mut bad = false;
                        for (_, c) in chars.by_ref() {
                            if c == '}' {
                                break;
                            }
                            // Below 0x10_0000 one more digit cannot overflow;
                            // anything larger is not a scalar value anyway.
                            match c.to_digit(16) {
                                Some(d) if v < 0x10_0000 => {
                                    v = v * 16 + d;
                                    digits += 1;
                                }
                                _ => bad = true,
                            }
                        }
                        if !bad && digits > 0 {
                            value = Some(v);
                        }
                    }
                    value.and_then(char::from_u32)
                }
                _ => simple,
            };
            match decoded {
                Some(d) => self
                    .text
                    .extend_from_slice(d.encode_utf8(&mut buf).as_bytes()),
                None => {
                    self.report(ProblemKind::InvalidEscape, span);
                    self.text.push(b'\\');
                    self.text
                        .extend_from_slice(next.encode_utf8(&mut buf).as_bytes());
                }
            }
        }
    }

    /// A negative literal pattern (`-1` in a pattern).
    pub(crate) fn negative_lit_pat(&mut self, tok: &'t Token<K>) -> Option<PatId> {
        let role = self.m.role_for(tok.kind())?;
        if !matches!(role, TokenRole::Int | TokenRole::Float | TokenRole::Number) {
            return None;
        }
        let span = tok.span();
        let lit = self.token_lit(role, span, true);
        self.at(span);
        Some(match lit {
            Decoded::Lit(l) => self.pt(Pat::Lit(l)),
            Decoded::Bad => self.pt(Pat::Err),
        })
    }

    /// Converts generic arguments to bounds (a type or a region).
    pub(crate) fn bound_of(arg: GenericArg) -> Option<Bound> {
        match arg {
            GenericArg::Ty(t) => Some(Bound::Ty(t)),
            GenericArg::Region(p) => Some(Bound::Region(p)),
            _ => None,
        }
    }
}

/// The HIR type a type name means.
fn type_of(name: TypeName) -> Ty {
    match name {
        TypeName::Prim(p) => Ty::Prim(p),
        TypeName::Any => Ty::Any,
        TypeName::Never => Ty::Never,
        TypeName::SelfTy => Ty::SelfTy,
        TypeName::Infer => Ty::Infer,
    }
}

/// Whether a number's `_` separators follow the language's rule.
fn separators_ok(body: &str, rule: Separators) -> bool {
    if !body.contains('_') {
        return true;
    }
    match rule {
        Separators::Anywhere => true,
        Separators::None => false,
        Separators::BetweenDigits => {
            let b = body.as_bytes();
            let (hex, start) = match (b.first(), b.get(1)) {
                (Some(b'0'), Some(b'x' | b'X')) => (true, 2),
                (Some(b'0'), Some(b'o' | b'O' | b'b' | b'B')) => (false, 2),
                _ => (false, 0),
            };
            let digit = |c: u8| {
                if hex {
                    c.is_ascii_hexdigit()
                } else {
                    c.is_ascii_digit()
                }
            };
            b.iter().enumerate().all(|(i, &c)| {
                c != b'_'
                    || (i > start
                        && b.get(i - 1).is_some_and(|&p| digit(p))
                        && b.get(i + 1).is_some_and(|&n| digit(n)))
            })
        }
    }
}

/// The body of a raw string literal: prefix letters, `#` fences, and one or
/// three quote characters removed from both ends.
fn raw_body(text: &str) -> Option<&str> {
    let s = text.trim_start_matches(|c: char| c.is_ascii_alphabetic());
    let hashes = s.len() - s.trim_start_matches('#').len();
    let s = s.get(hashes..)?;
    let q = s.chars().next()?;
    if !matches!(q, '"' | '\'' | '`') {
        return None;
    }
    let fence: alloc::string::String = core::iter::repeat_n('#', hashes).collect();
    let triple: alloc::string::String = core::iter::repeat_n(q, 3).collect();
    let quotes = if s.len() >= 6 && s.starts_with(&*triple) {
        triple
    } else {
        alloc::string::String::from(q)
    };
    let body = s.strip_prefix(&*quotes)?;
    let body = body.strip_suffix(&*fence)?;
    body.strip_suffix(&*quotes)
}

/// The nearest `f64` to `mant · 2^exp2`, where `sticky` says bits below
/// `mant` were non-zero (round half to even; subnormals rounded once).
/// `None` when the value overflows `f64`.
fn round_to_f64(mant: u64, sticky: bool, exp2: i64) -> Option<f64> {
    if mant == 0 {
        return Some(0.0);
    }
    let n = i64::from(64 - mant.leading_zeros());
    let top = exp2 + n - 1;
    let mut lsb = (top - 52).max(-1074);
    let shift = lsb - exp2;
    let mut m: u64 = if shift <= 0 {
        let s = u32::try_from(-shift).ok()?;
        mant.checked_shl(s)?
    } else {
        let sh = u32::try_from(shift).unwrap_or(u32::MAX);
        let wide = u128::from(mant);
        if sh >= 128 {
            0
        } else {
            let q = wide >> sh;
            let rem = wide & ((1u128 << sh) - 1);
            let half = 1u128 << (sh - 1);
            let up = rem > half || (rem == half && (sticky || q & 1 == 1));
            u64::try_from(q).ok()? + u64::from(up)
        }
    };
    if m == 1 << 53 {
        m >>= 1;
        lsb += 1;
    }
    if m >= 1 << 52 {
        let e = lsb + 52;
        if e > 1023 {
            return None;
        }
        let field = u64::try_from(e + 1023).ok()?;
        Some(f64::from_bits((field << 52) | (m & ((1 << 52) - 1))))
    } else {
        Some(f64::from_bits(m))
    }
}

/// A hexadecimal float's value from its text after `0x`: hex digits with an
/// optional point, then `p`/`P` and a decimal exponent.
fn hex_float(text: &str) -> Option<f64> {
    let (mant, exp) = text.split_once(['p', 'P'])?;
    let exp: i64 = {
        let (neg, digits) = match exp.as_bytes().first() {
            Some(b'-') => (true, exp.get(1..)?),
            Some(b'+') => (false, exp.get(1..)?),
            _ => (false, exp),
        };
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        // Past this the result is 0 or overflow either way.
        let mut v: i64 = 0;
        for b in digits.bytes() {
            v = (v * 10 + i64::from(b - b'0')).min(1 << 20);
        }
        if neg { -v } else { v }
    };
    let (int, frac) = mant.split_once('.').unwrap_or((mant, ""));
    if int.is_empty() && frac.is_empty() {
        return None;
    }
    let mut acc: u64 = 0;
    let mut exp2: i64 = exp;
    let mut sticky = false;
    for c in int.chars() {
        let d = u64::from(c.to_digit(16)?);
        if acc < 1 << 60 {
            acc = acc * 16 + d;
        } else {
            exp2 += 4;
            sticky |= d != 0;
        }
    }
    for c in frac.chars() {
        let d = u64::from(c.to_digit(16)?);
        if acc < 1 << 60 {
            acc = acc * 16 + d;
            exp2 -= 4;
        } else {
            sticky |= d != 0;
        }
    }
    round_to_f64(acc, sticky, exp2)
}

/// The nearest `f64` to an integer written in a power-of-two radix (2, 8,
/// 16), streaming its digits (linear in the length).
fn pow2_digits_to_f64(digits: &str, radix: u32) -> Option<f64> {
    let bits: u32 = match radix {
        2 => 1,
        8 => 3,
        16 => 4,
        _ => return None,
    };
    let mut acc: u64 = 0;
    let mut exp2: i64 = 0;
    let mut sticky = false;
    for c in digits.chars() {
        let d = u64::from(c.to_digit(radix)?);
        if acc < 1 << 60 {
            acc = (acc << bits) | d;
        } else {
            exp2 += i64::from(bits);
            sticky |= d != 0;
        }
    }
    round_to_f64(acc, sticky, exp2)
}

/// The exact decimal digits of an integer written in `radix`, or `None`
/// past [`MAX_BIG_DIGITS`] (the conversion is quadratic).
fn to_decimal(digits: &str, radix: u32) -> Option<alloc::string::String> {
    let digits = digits.trim_start_matches('0');
    if digits.len() > MAX_BIG_DIGITS {
        return None;
    }
    // Little-endian base-10^9 limbs.
    let mut limbs: alloc::vec::Vec<u64> = alloc::vec![0];
    for c in digits.chars() {
        let mut carry = u64::from(c.to_digit(radix)?);
        for l in &mut limbs {
            let v = *l * u64::from(radix) + carry;
            *l = v % 1_000_000_000;
            carry = v / 1_000_000_000;
        }
        while carry > 0 {
            limbs.push(carry % 1_000_000_000);
            carry /= 1_000_000_000;
        }
    }
    let mut out = alloc::string::String::new();
    let mut iter = limbs.iter().rev();
    if let Some(top) = iter.next() {
        out.push_str(&alloc::format!("{top}"));
    }
    for l in iter {
        out.push_str(&alloc::format!("{l:09}"));
    }
    Some(out)
}

/// The radix prefix of a number: `(radix, prefix length)`.
fn radix_of(digits: &str) -> Option<(u32, usize)> {
    let b = digits.as_bytes();
    match (b.first(), b.get(1)) {
        (Some(b'0'), Some(b'x' | b'X')) => Some((16, 2)),
        (Some(b'0'), Some(b'o' | b'O')) => Some((8, 2)),
        (Some(b'0'), Some(b'b' | b'B')) => Some((2, 2)),
        _ => None,
    }
}

/// Parses digits in `radix` into a `u64`, or `None` on a bad digit, no
/// digits, or overflow.
fn parse_u64(digits: &str, radix: u32) -> Option<u64> {
    if digits.is_empty() {
        return None;
    }
    let mut v: u64 = 0;
    for c in digits.chars() {
        let d = c.to_digit(radix)?;
        v = v.checked_mul(u64::from(radix))?.checked_add(u64::from(d))?;
    }
    Some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_u64_radixes_and_overflow() {
        assert_eq!(parse_u64("ff", 16), Some(255));
        assert_eq!(parse_u64("777", 8), Some(511));
        assert_eq!(parse_u64("102", 2), None);
        assert_eq!(parse_u64("18446744073709551615", 10), Some(u64::MAX));
        assert_eq!(parse_u64("18446744073709551616", 10), None);
        assert_eq!(parse_u64("", 10), None);
    }

    #[test]
    fn test_round_to_f64_is_correctly_rounded() {
        assert_eq!(round_to_f64(3, false, -1), Some(1.5));
        assert_eq!(
            round_to_f64(1, false, 1023),
            Some(f64::from_bits(0x7FE0_0000_0000_0000))
        );
        assert_eq!(round_to_f64(1, false, 1024), None);
        // The smallest subnormal, and half of it (ties to even: zero).
        assert_eq!(round_to_f64(1, false, -1074), Some(f64::from_bits(1)));
        assert_eq!(round_to_f64(1, false, -1075), Some(0.0));
        assert_eq!(round_to_f64(1, true, -1075), Some(f64::from_bits(1)));
        // 2^53 + 1 rounds to even (2^53); + sticky rounds up.
        let big = (1u64 << 53) + 1;
        assert_eq!(round_to_f64(big, false, 0), Some(9_007_199_254_740_992.0));
        assert_eq!(round_to_f64(big, true, 0), Some(9_007_199_254_740_994.0));
        assert_eq!(
            round_to_f64(u64::MAX, false, 0),
            Some(18_446_744_073_709_551_616.0)
        );
    }

    #[test]
    fn test_hex_floats_and_power_of_two_digits() {
        assert_eq!(hex_float("1.8p3"), Some(12.0));
        assert_eq!(hex_float("1p-2"), Some(0.25));
        assert_eq!(hex_float(".8p1"), Some(1.0));
        assert_eq!(hex_float("1.fffffffffffffp1023"), Some(f64::MAX));
        assert_eq!(hex_float("1p1024"), None);
        assert_eq!(hex_float("1p-1074"), Some(f64::from_bits(1)));
        assert_eq!(hex_float("1.8"), None);
        assert_eq!(hex_float("p3"), None);
        assert_eq!(
            pow2_digits_to_f64("ffffffffffffffff", 16),
            Some(18_446_744_073_709_551_616.0)
        );
        assert_eq!(
            pow2_digits_to_f64("1".repeat(70).as_str(), 2),
            Some(2f64.powi(70))
        );
    }

    #[test]
    fn test_separators_and_decimal_conversion() {
        assert!(separators_ok("1_000", Separators::BetweenDigits));
        assert!(separators_ok("0xFF_FF", Separators::BetweenDigits));
        assert!(!separators_ok("1__0", Separators::BetweenDigits));
        assert!(!separators_ok("10_", Separators::BetweenDigits));
        assert!(!separators_ok("0x_FF", Separators::BetweenDigits));
        assert!(!separators_ok("1_.5", Separators::BetweenDigits));
        assert!(separators_ok("1__0", Separators::Anywhere));
        assert!(!separators_ok("1_0", Separators::None));
        assert_eq!(
            to_decimal("ffffffffffffffffff", 16).as_deref(),
            Some("4722366482869645213695")
        );
        assert_eq!(to_decimal("0", 8).as_deref(), Some("0"));
        assert!(to_decimal(&"7".repeat(MAX_BIG_DIGITS + 1), 8).is_none());
    }

    #[test]
    fn test_raw_string_bodies() {
        assert_eq!(raw_body(r##"r#"a "b""#"##), Some(r#"a "b""#));
        assert_eq!(raw_body(r"r'\d+'"), Some(r"\d+"));
        assert_eq!(raw_body(r#"r"""x"y""""#), Some(r#"x"y"#));
        assert_eq!(raw_body(r#"br"z""#), Some("z"));
        assert_eq!(raw_body(r##"r#"open""##), None);
        assert_eq!(raw_body("r"), None);
    }

    #[test]
    fn test_radix_prefixes() {
        assert_eq!(radix_of("0x1F"), Some((16, 2)));
        assert_eq!(radix_of("0b1"), Some((2, 2)));
        assert_eq!(radix_of("0o7"), Some((8, 2)));
        assert_eq!(radix_of("07"), None);
    }
}
