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
use crate::mapping::{Escapes, IdentPatterns, Modifier, TokenRole, TypeName};
use crate::problem::ProblemKind;

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
                };
                match self.token_pat(role, span, site) {
                    Some(Val::Pat(p)) => Val::Param(Param::new(p), span),
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
            Want::Root | Want::Member | Want::Arm | Want::Where => self.wrong_place(want, span),
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
    /// alternative shares one binder per name.
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

    /// Decodes a number: radix prefixes, `_` separators, a registered type
    /// suffix; integers too large for 64 bits become big integers (decimal)
    /// or a problem (other radixes).
    fn number(&mut self, text: &str, span: Span, negative: bool, float: Option<bool>) -> Decoded {
        let (body, suffix) = self.split_suffix(text);
        self.scratch.clear();
        self.scratch.extend(body.chars().filter(|&c| c != '_'));
        let digits = core::mem::take(&mut self.scratch);
        let radix = radix_of(&digits);
        let is_float = match float {
            Some(f) => f,
            None => radix.is_none() && digits.bytes().any(|b| b == b'.' || b == b'e' || b == b'E'),
        } || suffix.is_some_and(Prim::is_float);
        let out = if is_float {
            match digits.parse::<f64>() {
                Ok(v) if radix.is_none() => {
                    let v = if negative { -v } else { v };
                    let mut lit = FloatLit::new(v);
                    if let Some(p) = suffix {
                        lit = lit.with_suffix(p);
                    }
                    Decoded::Lit(Lit::Float(lit))
                }
                _ => {
                    self.report(ProblemKind::InvalidLiteral, span);
                    Decoded::Bad
                }
            }
        } else {
            let (r, rest) = match radix {
                Some((r, skip)) => (r, digits.get(skip..).unwrap_or("")),
                None => (10, digits.as_str()),
            };
            match parse_u64(rest, r) {
                Some(value) if !(negative && value == 0) => {
                    let mut lit = IntLit::new(value);
                    lit.negative = negative;
                    if let Some(p) = suffix {
                        lit = lit.with_suffix(p);
                    }
                    Decoded::Lit(Lit::Int(lit))
                }
                Some(_) => Decoded::Lit(Lit::Int(IntLit::new(0))),
                None if r == 10 && !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()) => {
                    // Too large for 64 bits: a big integer, digits as written.
                    let mut s = alloc::string::String::new();
                    if negative {
                        s.push('-');
                    }
                    s.push_str(rest.trim_start_matches('0'));
                    let t = self.b.text(&s);
                    Decoded::Lit(Lit::BigInt(t))
                }
                None => {
                    self.report(ProblemKind::InvalidLiteral, span);
                    Decoded::Bad
                }
            }
        };
        self.scratch = digits;
        out
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
        self.text.clear();
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
    fn test_radix_prefixes() {
        assert_eq!(radix_of("0x1F"), Some((16, 2)));
        assert_eq!(radix_of("0b1"), Some((2, 2)));
        assert_eq!(radix_of("0o7"), Some((8, 2)));
        assert_eq!(radix_of("07"), None);
    }
}
