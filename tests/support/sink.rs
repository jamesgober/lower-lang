//! A "kitchen sink" mapping: most archetypes, every built-in template, a
//! template chain, operators, modifiers, and roles, over generic kinds, so
//! random trees reach every lowering path (right and wrong contexts alike).

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines
)]

use hir_lang::{BorrowKind, CaptureMode, OpKind, PathRoot, Prim};
use lower_lang::{
    Arch, Escapes, Join, LangPolicy, Mapping, MappingBuilder, Modifier, OpSpec, Pick, RecordShape,
    TArg, TemplateBuilder, TokenRole, TypeName, templates,
};

use super::writer::K;

pub const TOKEN_KINDS: std::ops::RangeInclusive<u16> = 1..=49;
pub const NODE_KINDS: std::ops::RangeInclusive<u16> = 100..=216;
pub const ERROR_KIND: K = K(197);
pub const ROOT: K = K(100);

/// Token texts that fit (and misfit) the roles.
pub const TEXTS: &[&str] = &[
    "x",
    "it",
    "t",
    "old",
    "$v",
    "$it",
    "1",
    "0x1F",
    "2.5",
    "1e3",
    "99999999999999999999",
    "0b2",
    "\"s\\n\"",
    "\"\\q\"",
    "'c'",
    "'ab'",
    "b\"\\xff\"",
    "true",
    "null",
    "_",
    "..",
    "'a",
    "'l",
    "+",
    "-",
    "*",
    "=",
    "&",
    "&&",
    "||",
    "??",
    "++",
    "(",
    ")",
    "::",
    "...",
    "mut",
    "pub",
    "-",
    "%",
    "r#\"raw \"x\"\"#",
    "r\"open",
    "<<<EOT\n  a\\n\n   b\n  EOT",
    "<<<'N'\nx $y\nN",
    "<<<EOT\nbad\n EOT",
    "Hello {",
    "1_000",
    "1__0",
    "0777",
    "08",
    "0x1.8p3",
    "0xFFFFFFFFFFFFFFFFFF",
    "9223372036854775808",
    ".=",
    "??=",
    "**=",
    // 0.4.1: the implicit receiver, and a break level.
    "$this",
    "2",
];

fn k(i: u16) -> K {
    K(i)
}

/// The sink mapping, with `policy` (the property tests try several). A
/// policy with numeric break levels is the 0.4.1 one: it also gets the
/// implicit receiver `this`, per-archetype modifiers, a closure that sees
/// only explicit captures (214), a numeric `break` (215), and a conditional
/// template (216); under the other policies 214..=216 stay unmapped.
pub fn sink(mut policy: LangPolicy) -> Mapping<K> {
    let mut b = MappingBuilder::new();
    let v041 = policy.break_levels == lower_lang::BreakLevels::Numeric;
    if v041 {
        policy.implicit_receiver = Some(b.name("this"));
    }
    b.policy(policy);
    // Tokens 1..=20 have roles; 21..=40 modifiers and operators; the rest
    // nothing.
    b.token(k(1), TokenRole::Name)
        .token(k(2), TokenRole::Var { sigil: 1 })
        .token(k(3), TokenRole::Number)
        .token(k(4), TokenRole::Int)
        .token(k(5), TokenRole::Float)
        .token(k(6), TokenRole::Str(Escapes::Standard))
        .token(k(7), TokenRole::Str(Escapes::Quotes))
        .token(k(8), TokenRole::Char(Escapes::Standard))
        .token(k(9), TokenRole::Bytes(Escapes::None))
        .token(k(10), TokenRole::True)
        .token(k(11), TokenRole::False)
        .token(k(12), TokenRole::Null)
        .token(k(13), TokenRole::Wild)
        .token(k(14), TokenRole::Rest)
        .token(k(15), TokenRole::Lifetime { sigil: 1 })
        .token(k(16), TokenRole::Label { sigil: 1 })
        .token(k(17), TokenRole::Type(TypeName::Prim(Prim::I32)))
        .token(k(18), TokenRole::Type(TypeName::Any))
        .token(k(19), TokenRole::Type(TypeName::SelfTy))
        .token(k(20), TokenRole::Name)
        .token(k(46), TokenRole::StrPart(Escapes::Standard))
        .token(k(47), TokenRole::RawStr)
        .token(k(48), TokenRole::Heredoc(Escapes::Standard))
        .token(k(49), TokenRole::Heredoc(Escapes::None));
    b.modifier(k(21), Modifier::Mut)
        .modifier(k(22), Modifier::Ref)
        .modifier(k(23), Modifier::Public)
        .modifier(k(24), Modifier::Async)
        .modifier(k(25), Modifier::Rest)
        .modifier(k(26), Modifier::NamedOnly)
        .modifier(k(27), Modifier::PositionalOnly)
        .modifier(k(28), Modifier::Receiver)
        .modifier(k(29), Modifier::Static)
        .modifier(k(30), Modifier::Inclusive)
        .modifier(k(31), Modifier::Spread)
        .modifier(k(32), Modifier::Glob)
        .modifier(k(20), Modifier::Generator);
    b.root(k(33), PathRoot::Super(1))
        .root(k(34), PathRoot::SelfType)
        .root(k(35), PathRoot::Global);
    b.skip(k(36)).skip(k(37));
    b.type_name("int", TypeName::Prim(Prim::I64))
        .type_name("mixed", TypeName::Any);
    b.suffix("u8", Prim::U8);

    let w = b.template(templates::while_loop()).unwrap();
    let fe = b.template(templates::for_each("iter", "next")).unwrap();
    let cf = b.template(templates::c_for()).unwrap();
    let dw = b.template(templates::do_while()).unwrap();
    let co = b.template(templates::coalesce()).unwrap();
    let and = b.template(templates::logical_and()).unwrap();
    let or = b.template(templates::or_value()).unwrap();
    let pi = b.template(templates::post_increment()).unwrap();
    let tq = b.template(templates::try_operator()).unwrap();
    // A two-step chain: until(c, b) = while(!c, b).
    let until = {
        let mut t = TemplateBuilder::new("until");
        let (c, body, l) = (
            t.expr_hole("cond"),
            t.expr_hole("body"),
            t.label_hole("label"),
        );
        let c = t.hole(c);
        let not_c = t.op(OpKind::Not, &[c]);
        let body = t.hole(body);
        let root = t.apply(
            w,
            &[
                TArg::Expr(not_c),
                TArg::Expr(body),
                TArg::Label(lower_lang::LabelRef::Hole(l)),
            ],
        );
        b.template(t.finish(root).unwrap()).unwrap()
    };
    let echo = b.name("echo");

    b.binary_op(k(38), OpSpec::Op(OpKind::Add))
        .binary_op(k(39), OpSpec::Op(OpKind::Mul))
        .binary_op(k(40), OpSpec::Assign)
        .binary_op(k(41), OpSpec::Template(and))
        .binary_op(k(42), OpSpec::Template(or))
        .binary_op(k(43), OpSpec::Template(co))
        .binary_op(k(44), OpSpec::Compound(OpKind::Add))
        .binary_op(k(45), OpSpec::Extern(echo))
        .prefix_op(k(38), OpSpec::Identity)
        .prefix_op(k(39), OpSpec::Deref)
        .prefix_op(k(22), OpSpec::Borrow(BorrowKind::Mut))
        .prefix_op(k(41), OpSpec::Op(OpKind::Not))
        .prefix_op(k(42), OpSpec::Op(OpKind::Neg))
        .postfix_op(k(43), OpSpec::Template(tq))
        .postfix_op(k(44), OpSpec::Template(pi))
        .postfix_op(k(45), OpSpec::Await)
        // Text-keyed: any token spelling `-` or `%` in operator position.
        .binary_op_text("-", OpSpec::Op(OpKind::Sub))
        .binary_op_text("%", OpSpec::Op(OpKind::Rem))
        .prefix_op_text("-", OpSpec::Op(OpKind::Neg));
    // 0.4: compound assignment through non-OPS operators (`let_place`), and
    // a textual template for prefix `++`.
    let pow = b.name("pow");
    let pre_inc = b
        .parse_template("pre_increment", "let_place p = ${place} in p = add(p, 1)")
        .unwrap();
    b.binary_op_text(".=", OpSpec::CompoundExtern(echo))
        .binary_op_text("??=", OpSpec::CompoundTemplate(co))
        .binary_op_text("**=", OpSpec::CompoundIntrinsic(pow))
        .prefix_op_text("++", OpSpec::Template(pre_inc));
    let textual = b
        .parse_template(
            "textual",
            "{ let t: i64 | str = ${cond}; let u: Countable & Traversable = t; \
             let_place p = ${place} in p = add<overflow=wrap>(p, u); \
             yield t => apply:until(${cond}, ${body}, _); yield from ${body} }",
        )
        .unwrap();
    let interp_all = {
        let mut t = TemplateBuilder::new("interp");
        let parts = t.args_hole("parts");
        let f = t.extern_("interp");
        let s = t.splice(parts);
        let root = t.call(f, &[s]);
        b.template(t.finish(root).unwrap()).unwrap()
    };

    let n = Pick::Nth;
    let all = Pick::All;
    let nodes = Pick::Nodes;
    let back = Pick::Back(0);
    let rest = Pick::Range(1, 0);
    let a = Pick::Absent;
    let rules: Vec<Arch<K>> = vec![
        Arch::Module {
            name: a,
            items: nodes,
        }, // 100
        Arch::Fn {
            name: Pick::Kind(k(1)),
            generics: Pick::Kind(k(103)),
            params: Pick::Kind(k(102)),
            ret: back,
            throws: a,
            body: Pick::Kind(k(108)),
            where_: Pick::Kind(k(106)),
        }, // 101
        Arch::Splice { items: all }, // 102
        Arch::Splice { items: all }, // 103
        Arch::Param {
            pat: n(0),
            ty: n(1),
            default: n(2),
        }, // 104
        Arch::GenericParam {
            name: n(0),
            bounds: rest,
            ty: a,
            default: a,
        }, // 105
        Arch::Splice { items: nodes }, // 106
        Arch::WherePred {
            subject: n(0),
            bounds: rest,
        }, // 107
        Arch::Block {
            stmts: nodes,
            tail: a,
            label: Pick::Kind(k(16)),
            tail_from_last: true,
        }, // 108
        Arch::Let {
            pat: n(0),
            ty: n(1),
            init: n(2),
            else_: a,
            mutable: false,
            hoist: false,
        }, // 109
        Arch::Let {
            pat: n(0),
            ty: a,
            init: n(1),
            else_: a,
            mutable: true,
            hoist: true,
        }, // 110
        Arch::ExprStmt { expr: n(0) }, // 111
        Arch::binary(),              // 112
        Arch::prefix(),              // 113
        Arch::Postfix {
            operand: n(0),
            op: n(1),
        }, // 114
        Arch::Call {
            callee: n(0),
            args: rest,
        }, // 115
        Arch::Member {
            base: n(0),
            member: n(1),
            args: Pick::Range(2, 0),
            call: n(2),
        }, // 116
        Arch::MethodCall {
            receiver: n(0),
            method: n(1),
            generics: a,
            args: Pick::Range(2, 0),
        }, // 117
        Arch::Index {
            base: n(0),
            index: n(1),
        }, // 118
        Arch::Tuple { elems: all },  // 119
        Arch::Array { elems: all },  // 120
        Arch::Map { entries: all },  // 121
        Arch::MapEntry {
            key: n(0),
            value: n(1),
        }, // 122
        Arch::RecordLit {
            path: n(0),
            fields: rest,
            base: a,
        }, // 123
        Arch::FieldInit {
            name: n(0),
            value: n(1),
        }, // 124
        Arch::If {
            cond: n(0),
            then: n(1),
            else_: n(2),
        }, // 125
        Arch::Match {
            scrutinee: n(0),
            arms: rest,
        }, // 126
        Arch::Arm {
            pat: n(0),
            guard: a,
            body: back,
        }, // 127
        Arch::Loop {
            label: Pick::Kind(k(16)),
            body: back,
        }, // 128
        Arch::Break {
            label: Pick::Kind(k(16)),
            value: Pick::Kind(k(3)),
        }, // 129
        Arch::Continue {
            label: Pick::Kind(k(16)),
        }, // 130
        Arch::Return { value: n(0) }, // 131
        Arch::Closure {
            params: Pick::Kind(k(102)),
            ret: a,
            captures: Pick::Kind(k(133)),
            body: back,
            implicit: Some(CaptureMode::ByValue),
        }, // 132
        Arch::Splice { items: all }, // 133
        Arch::Capture { name: n(0) }, // 134
        Arch::Try {
            body: n(0),
            catches: Pick::Range(1, 1),
            finally: back,
        }, // 135
        Arch::Catch {
            types: Pick::AllKind(k(1)),
            name: Pick::Kind(k(2)),
            body: back,
        }, // 136
        Arch::Record {
            name: n(0),
            generics: a,
            fields: rest,
            shape: RecordShape::Auto,
        }, // 137
        Arch::FieldDecl {
            name: n(0),
            ty: n(1),
            default: n(2),
        }, // 138
        Arch::Sum {
            name: n(0),
            generics: a,
            variants: rest,
        }, // 139
        Arch::VariantDecl {
            name: n(0),
            fields: rest,
            discriminant: a,
            shape: RecordShape::Auto,
        }, // 140
        Arch::Class {
            name: n(0),
            generics: a,
            bases: n(1),
            interfaces: a,
            members: nodes,
            mixin: false,
        }, // 141
        Arch::Interface {
            name: n(0),
            generics: a,
            supers: a,
            items: nodes,
        }, // 142
        Arch::Impl {
            generics: a,
            interface: a,
            self_ty: n(0),
            items: nodes,
        }, // 143
        Arch::Alias {
            name: n(0),
            generics: a,
            ty: n(1),
        }, // 144
        Arch::Const {
            name: n(0),
            ty: a,
            value: n(1),
        }, // 145
        Arch::GlobalVar {
            name: n(0),
            ty: a,
            init: n(1),
            mutable: true,
        }, // 146
        Arch::Import {
            path: n(0),
            alias: n(1),
        }, // 147
        Arch::Path {
            segments: all,
            global: Pick::First(k(35)),
            qself: a,
            qtrait: a,
        }, // 148
        Arch::Segment {
            name: n(0),
            args: rest,
        }, // 149
        Arch::TuplePat { elems: all }, // 150
        Arch::CtorPat {
            path: n(0),
            elems: rest,
        }, // 151
        Arch::OrPat { alts: all },   // 152
        Arch::RangePat { lo: n(0), hi: n(1) }, // 153
        Arch::RefPat { inner: back }, // 154
        Arch::SlicePat { elems: all }, // 155
        Arch::BindPat {
            name: n(0),
            sub: n(1),
        }, // 156
        Arch::TypeTestPat {
            ty: n(0),
            pat: n(1),
        }, // 157
        Arch::RestPat { bind: n(0) }, // 158
        Arch::TupleTy { elems: all }, // 159
        Arch::ArrayTy {
            elem: n(0),
            len: n(1),
        }, // 160
        Arch::SliceTy { elem: n(0) }, // 161
        Arch::RefTy {
            inner: back,
            region: Pick::Kind(k(15)),
        }, // 162
        Arch::PtrTy { inner: back }, // 163
        Arch::NullableTy { inner: n(0) }, // 164
        Arch::FnTy {
            params: Pick::Range(0, 1),
            ret: back,
        }, // 165
        Arch::ObjectTy { bounds: all }, // 166
        Arch::ImplTy { bounds: all }, // 167
        Arch::Splice { items: nodes }, // 168
        Arch::Unsupported,           // 169
        Arch::GlobalDecl { names: all }, // 170
        Arch::Static {
            name: n(0),
            ty: a,
            init: n(1),
        }, // 171
        Arch::Defer { body: n(0) },  // 172
        Arch::Await { inner: n(0) }, // 173
        Arch::Yield {
            key: Pick::Absent,
            value: n(0),
        }, // 174
        Arch::Spawn { inner: n(0) }, // 175
        Arch::Throw { value: n(0) }, // 176
        Arch::Cast {
            expr: n(0),
            ty: n(1),
        }, // 177
        Arch::Borrow { expr: back }, // 178
        Arch::Deref { inner: back }, // 179
        Arch::Repeat {
            elem: n(0),
            count: n(1),
        }, // 180
        Arch::VarVar { inner: back }, // 181
        Arch::Append { base: n(0) }, // 182
        Arch::DynMember {
            base: n(0),
            name: n(1),
            args: Pick::Range(2, 0),
            call: n(2),
        }, // 183
        Arch::Arg {
            name: Pick::Before(k(37)),
            value: back,
        }, // 184
        Arch::AssocType {
            name: n(0),
            bounds: rest,
            default: a,
        }, // 185
        Arch::MixinUse {
            mixins: Pick::AllKind(k(1)),
            rules: Pick::AllKind(k(205)),
        }, // 186
        Arch::AssocBinding {
            name: n(0),
            ty: n(1),
        }, // 187
        Arch::Pass { inner: n(0) },  // 188
        Arch::Assign {
            target: n(0),
            op: n(1),
            value: back,
        }, // 189
        Arch::RefAssign {
            target: n(0),
            source: back,
        }, // 190
    ];
    for (i, arch) in rules.into_iter().enumerate() {
        b.node(k(100 + i as u16), arch);
    }
    // 0.3 archetypes: interpolation (three joins), use trees, mixin rules,
    // attributes.
    b.node(
        k(201),
        Arch::Interp {
            parts: all,
            close: a,
            join: Join::Fold(echo),
        },
    );
    b.node(
        k(202),
        Arch::Interp {
            parts: Pick::Range(0, 1),
            close: back,
            join: Join::Call(echo),
        },
    );
    b.node(
        k(203),
        Arch::Interp {
            parts: all,
            close: a,
            join: Join::Template(interp_all),
        },
    );
    b.node(
        k(204),
        Arch::UseTree {
            path: n(0),
            alias: Pick::After(k(36)),
            children: Pick::Range(1, 0),
        },
    );
    b.node(
        k(205),
        Arch::MixinRule {
            from: Pick::Before(k(34)),
            method: Pick::Kind(k(1)),
            insteadof: Pick::AllAfter(k(37)),
            alias: Pick::After(k(36)),
        },
    );
    b.node(
        k(206),
        Arch::Attr {
            name: n(0),
            args: rest,
        },
    );
    b.node(
        k(207),
        Arch::AttrArg {
            key: n(0),
            value: n(1),
        },
    );
    b.node(
        k(208),
        Arch::Interp {
            parts: all,
            close: a,
            join: Join::Template(and),
        },
    );
    // 0.4 archetypes: union and intersection types, `yield from`, keyed
    // `yield`, a textual template.
    b.node(k(209), Arch::UnionTy { members: all });
    b.node(k(210), Arch::IntersectionTy { members: all });
    b.node(k(211), Arch::YieldFrom { inner: n(0) });
    b.node(
        k(212),
        Arch::Yield {
            key: n(0),
            value: n(1),
        },
    );
    b.template_node(k(213), textual, &[n(0), n(1), back]);
    b.template_node(k(191), w, &[n(0), back, Pick::Kind(k(16))]);
    b.template_node(k(192), fe, &[n(0), Pick::Before(k(37)), n(1), back, a]);
    b.template_node(k(193), cf, &[Pick::Kind(k(109)), n(1), n(2), back, a]);
    b.template_node(k(194), dw, &[n(0), n(1), a]);
    b.template_node(k(195), until, &[n(0), back, Pick::Kind(k(16))]);
    b.template_node(k(196), co, &[n(0), n(1)]);
    b.error(ERROR_KIND);
    b.node_when(k(198), Pick::First(k(1)), Arch::ExprStmt { expr: n(0) });
    b.node(k(198), Arch::Pass { inner: back });
    // 199 and 200: unmapped.
    if v041 {
        b.modifier_in(k(25), "arg", Modifier::Spread)
            .modifier_in(k(25), "map_entry", Modifier::Spread)
            .modifier_in(k(31), "param", Modifier::Rest);
        b.node(
            k(214),
            Arch::Closure {
                params: Pick::Kind(k(102)),
                ret: a,
                captures: Pick::Kind(k(133)),
                body: back,
                implicit: None,
            },
        );
        b.node(
            k(215),
            Arch::Break {
                label: Pick::Kind(k(3)),
                value: a,
            },
        );
        b.template_node_when(k(216), Pick::First(k(1)), co, &[n(0), n(1)]);
        b.template_node(k(216), w, &[n(0), back, a]);
    }
    b.build().expect("the sink mapping is valid")
}

/// The policies the properties run under.
pub fn policies() -> Vec<LangPolicy> {
    let mut dynamic = LangPolicy::default();
    dynamic.overflow = hir_lang::Overflow::Promote;
    dynamic.throws = lower_lang::EffectMode::Always;
    dynamic.yields = lower_lang::EffectMode::Inferred;
    dynamic.asyncs = lower_lang::EffectMode::Inferred;
    dynamic.implicit_locals = true;
    dynamic.place_args = lower_lang::PlaceArgs::Places;
    dynamic.ident_patterns = lower_lang::IdentPatterns::Bind;
    // PHP: function-scoped foreach/catch bindings, PHP number spelling.
    let php = LangPolicy::php();
    // 0.4.1: numeric break levels (and, in `sink`, the receiver).
    let mut mox = LangPolicy::php();
    mox.break_levels = lower_lang::BreakLevels::Numeric;
    mox.default_member_visibility = hir_lang::Vis::Public;
    vec![LangPolicy::default(), dynamic, php, mox]
}
