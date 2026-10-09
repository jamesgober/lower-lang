# lower-lang &mdash; API Reference

> Complete reference for every public item in `lower-lang`, with examples.
> **Status: pre-1.0 (0.3.0, PHP and Python semantics).** The surface is designed across the
> 0.x series and frozen at 1.0, after the LexerSketch app has lowered at least
> three target languages through it (decision D18). See
> [`../dev/ROADMAP.md`](../dev/ROADMAP.md).

<sub>Copyright &copy; 2026 <strong>James Gober</strong>.</sub>

## Table of contents

- [Overview](#overview)
- [Installation](#installation)
- [Quick start](#quick-start)
- [Concepts](#concepts)
  - [Contexts](#contexts)
  - [Fields: labels, kinds, positions](#fields-labels-kinds-positions)
  - [Modifiers, roles, roots](#modifiers-roles-roots)
  - [Templates and hygiene](#templates-and-hygiene)
  - [Origins](#origins)
  - [Effects and hoisting](#effects-and-hoisting)
  - [Function-scoped bindings (PHP)](#function-scoped-bindings-php)
  - [Interpolated strings and heredocs](#interpolated-strings-and-heredocs)
  - [Number literals](#number-literals)
  - [Operators by kind or by text](#operators-by-kind-or-by-text)
  - [Use trees, mixin rules, attributes](#use-trees-mixin-rules-attributes)
  - [Problems and half-typed code](#problems-and-half-typed-code)
- [`lower`](#lower)
- [`Lowerer`](#lowerer)
- [`Budget`](#budget)
- [`Lowered`](#lowered)
- [`Mapping`](#mapping)
- [`MappingBuilder`](#mappingbuilder)
- [`Arch`](#arch)
- [`RecordShape`](#recordshape)
- [`TemplateUse`](#templateuse)
- [`Pick`](#pick)
- [`Labeler`](#labeler)
- [`TokenRole`](#tokenrole), [`Escapes`](#escapes), [`TypeName`](#typename)
- [`Modifier`](#modifier)
- [`LangPolicy`](#langpolicy), [`EffectMode`](#effectmode), [`IdentPatterns`](#identpatterns), [`PlaceArgs`](#placeargs)
- [`Literals`](#literals), [`Separators`](#separators), [`LegacyOctal`](#legacyoctal)
- [`OpSpec`](#opspec), [`Fixity`](#fixity), [`NameRef`](#nameref), [`Join`](#join)
- [`Template`](#template)
- [`TemplateBuilder`](#templatebuilder)
- [Template handles](#template-handles)
- [`TemplateId`](#templateid), [`HoleKind`](#holekind), [`TLit`](#tlit), [`LabelRef`](#labelref), [`TArg`](#targ)
- [`templates`](#templates)
- [`Problem`](#problem), [`ProblemKind`](#problemkind), [`HoistBlocker`](#hoistblocker), [`Context`](#context)
- [Errors](#errors): [`LowerError`](#lowererror), [`MappingError`](#mappingerror), [`TemplateError`](#templateerror)
- [Re-exports](#re-exports)
- [Feature flags](#feature-flags)
- [Limits](#limits)
- [Stability](#stability)

## Overview

| Item | Kind | Purpose |
|---|---|---|
| [`lower`](#lower) | function | The Tier-1 entry point: tree + source + mapping + interner → HIR and problems. |
| [`Lowerer`](#lowerer) | struct | The configured path: budget, compilation unit, field labels. |
| [`Budget`](#budget) | struct | Node and template-depth limits. |
| [`Lowered`](#lowered) | struct | A valid `hir_lang::Hir` plus every problem, in source order. |
| [`Mapping`](#mapping), [`MappingBuilder`](#mappingbuilder) | structs | A language's semantics as data, and how to build one. |
| [`Arch`](#arch) | enum | The archetype catalogue. |
| [`Pick`](#pick), [`Labeler`](#labeler) | enum, trait | Where a field is among a node's children. |
| [`TokenRole`](#tokenrole), [`Modifier`](#modifier), [`TypeName`](#typename), [`Escapes`](#escapes) | enums | What tokens mean. |
| [`LangPolicy`](#langpolicy) and its enums | struct | Language-wide policies (overflow, effects, implicit locals, PHP scoping, ...); `LangPolicy::php()` is the PHP/Mox preset. |
| [`Literals`](#literals), [`Separators`](#separators), [`LegacyOctal`](#legacyoctal) | struct, enums | How number literals are spelled. |
| [`OpSpec`](#opspec) | enum | What an operator token lowers to. |
| [`Join`](#join) | enum | How the pieces of an interpolated string become one string. |
| [`Template`](#template), [`TemplateBuilder`](#templatebuilder), handles | types | Hygienic lowering templates. |
| [`templates`](#templates) | module | Ready-made templates for common sugar. |
| [`Problem`](#problem), [`ProblemKind`](#problemkind), [`HoistBlocker`](#hoistblocker), [`Context`](#context) | types | What is wrong with the code, and where. |
| [`LowerError`](#lowererror), [`MappingError`](#mappingerror), [`TemplateError`](#templateerror) | enums | Exhausted resources; refused mappings and templates. |

## Installation

```toml
[dependencies]
lower-lang = "0.3"
hir-lang = "0.3"
intern-lang = "1"
syntax-lang = "1"
```

Without the standard library:

```toml
[dependencies]
lower-lang = { version = "0.3", default-features = false }
```

## Quick start

```rust
use hir_lang::OpKind;
use intern_lang::Interner;
use lower_lang::{lower, Arch, MappingBuilder, OpSpec, Pick, TokenRole};
use syntax_lang::{Element, Node, Span, Token, TokenKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum K { File, Let, Mul, Name, Num, LetKw, Eq, Star }
impl TokenKind for K {}

let mut b = MappingBuilder::new();
b.node(K::File, Arch::Module { name: Pick::Absent, items: Pick::Nodes });
b.node(K::Let, Arch::Let {
    pat: Pick::Kind(K::Name), ty: Pick::Absent, init: Pick::After(K::Eq),
    else_: Pick::Absent, mutable: false, hoist: false,
});
b.node(K::Mul, Arch::binary());
b.token(K::Name, TokenRole::Name).token(K::Num, TokenRole::Int);
b.binary_op(K::Star, OpSpec::Op(OpKind::Mul));
let mapping = b.build()?;

// let x = 6*7   (spans are byte offsets into "let x = 6*7")
let tok = |k, s, e| Element::Token(Token::new(k, Span::new(s, e)));
let mul = Node::new(K::Mul, vec![tok(K::Num, 8, 9), tok(K::Star, 9, 10), tok(K::Num, 10, 11)]);
let decl = Node::new(K::Let, vec![tok(K::LetKw, 0, 3), tok(K::Name, 4, 5), tok(K::Eq, 6, 7), Element::Node(mul)]);
let file = Node::new(K::File, vec![Element::Node(decl)]);

let mut names = Interner::new();
let out = lower(&file, "let x = 6*7", &mapping, &mut names).expect("within budget");
assert!(out.problems.is_empty());
let printed = hir_lang::print(&out.hir, &names);
assert!(printed.contains("(bind x%0)"));
assert!(printed.contains("(op mul overflow=error"));
# Ok::<(), lower_lang::MappingError>(())
```

## Concepts

### Contexts

Lowering a node lowers each of its fields in the context the archetype gives
it: an `If`'s `cond` as an expression, a `Fn`'s `params` as parameters, a
`Let`'s `pat` as a pattern. The same node kind can therefore mean different
things in different places:

| Lowered as | A name token | A tuple node | A call node |
|---|---|---|---|
| expression | a path expression | `Expr::Tuple` | `Expr::Call` |
| pattern | a binding (or `Pat::Ident` in match arms) | `Pat::Tuple` | `Pat::Ctor` |
| type | a type path, or a type keyword | `Ty::Tuple` | an error |
| parameter | a parameter binding it | a destructuring parameter | an error |

An expression in a statement list becomes an expression statement (or the
block's tail, with `tail_from_last`); a declaration in a block becomes a local
item; in a module, declarations become items and everything else the module's
top-level body. A construct in a context it cannot appear in is an error node
plus a [`NotAllowedHere`](#problemkind) problem.

### Fields: labels, kinds, positions

A field's [`Pick`](#pick) finds it among the node's **significant** children
(nodes and non-trivia tokens). LSF2 trees carry field labels, read through a
[`Labeler`](#labeler) with `Pick::Label`; lang-forge 1.x trees have none, so
mappings use kinds (`Kind`, `KindNth`, `AllKind`), positions (`Nth`, `Back`,
`NthNode`, `Range`), and neighbours (`After`, `Before`, `Between`, `First`).
List picks splice *container* kinds (an `args` node inside a call) and leave
out *skip* kinds (`,`, `;`).

Prefer kind picks over positional ones: in half-typed code the parser may omit
a part, and "the first child node" then lands on the next part. When one child
is selected for two fields, the first field keeps it, the later field is
absent, and an [`AmbiguousChild`](#problemkind) problem is reported.

### Modifiers, roles, roots

Flags a construct spells with keywords (`mut`, `pub`, `async`, `static`, `&`,
`...`) are **modifiers**: tokens registered with
[`MappingBuilder::modifier`](#mappingbuilder), read from the direct children
of the archetypes that consult them (listed on each [`Arch`](#arch) variant).
A token lowered on its own is interpreted by its **role**
([`TokenRole`](#tokenrole)). Leading tokens of a `Path` whose kind is in the
**root** table set the path's root (`super`, `Self`, `::`, PHP
`parent`/`static`).

### Templates and hygiene

A [`Template`](#template) is a small HIR-shaped tree with holes. A node kind
(or an operator) that uses a template lowers its holes' fields as user code,
then instantiates the template: every instantiation records an HIR expansion
(`Template`, or `Desugar` for the built-ins), and every binder and label the
template introduces is created fresh, marked with that expansion, and
referenced by binder id. User code in holes keeps its own marks, so neither
can capture the other. Free names a template writes (`global`) carry the mark
and resolve at the template's definition site; host symbols (`extern_`) are
resolved to `Res::Extern` directly.

A hole used twice is copied (`Builder::copy_subtree`), since HIR is a tree. A
template may `apply` templates registered before it, so expansion always
terminates; each application is a nested expansion counted against
[`Budget::max_expansion_depth`](#budget).

### Origins

Every node is built with an origin: its own syntax span for user code, and
for template output the span of the node that invoked the template, with the
instantiation's expansion. Nested applications chain their expansions
(`parent`), so a diagnostic can show the surface construct.

### Effects and hoisting

Functions, closures, and module bodies record the effects their modifiers
declare (`Async`, `Generator`, `Throws`, `Unsafe`); [`LangPolicy`](#langpolicy)
can add effects the body uses (`Inferred`: a `yield` makes a generator, as in
PHP) or every frame (`Always`: any PHP function may throw). A hoisted `Let`
(JavaScript `var`) and, with `implicit_locals`, a plain variable assignment
(PHP) declare the name once at the top of the enclosing function or module
body (names a parameter, `global`, `static`, or capture already declares are
left alone). A hoisted `Let` with a destructuring pattern (`var [a, b] = x`)
binds hygienic temporaries and assigns them to the hoisted names right after
the pattern matches.

### Function-scoped bindings (PHP)

In PHP the variables a `foreach` binds and the variable a `catch` binds belong
to the enclosing function: they keep their last value after the loop or
handler. With [`LangPolicy::function_scoped_bindings`](#langpolicy) (on in
`LangPolicy::php()`), every pattern hole of a template (the `key` and `value`
of `for_each`) and every `Catch` name is lowered the same way as a hoisted
`Let`: the names are declared once at the top of the function (or module
body), the pattern itself binds hygienic temporaries (each marked with its own
`Desugar` expansion named `hoist`), and an assignment from each temporary to
its function-scoped name runs right after the pattern matches. Any pattern
shape works (`foreach ($m as [$a, [$b, $c]])`). What cannot be hoisted this
way stays scoped to its construct and is reported as
[`CannotHoist`](#problemkind) with a [`HoistBlocker`](#hoistblocker): a
by-reference binding (`foreach ($a as &$v)`: an assignment would copy, not
alias), a hole the template binds in more than one place, or a hole whose
binding site was cut away (a depth-budget cut).

### Interpolated strings and heredocs

A lexer delivers `"Hello {$name}!"` in segments. The string's node is an
[`Arch::Interp`](#arch): its text pieces are tokens with role
[`TokenRole::StrPart`](#tokenrole) (escapes decoded, adjacent pieces merged)
and every other selected child is an embedded expression (`$name` as a
variable token, `{$a['k']}` as an index node; braces and quotes are skip kinds
or left out by the `parts` pick). The pieces become one string through the
rule's [`Join`](#join): a left fold of a two-argument host function (PHP's
`.`), one call with every piece, or a template. One text piece alone is a
string literal; a lone expression is joined with the empty string, so the
result is always a string conversion.

A heredoc is the same node with a `close` pick naming its closing marker: the
marker's indentation is removed from every line of the text pieces (PHP
7.3+), a line indented less (or starting with an embedded expression) is an
[`InvalidLiteral`](#problemkind), and the
newline before the marker is dropped. A heredoc or nowdoc without
interpolation can also be one token ([`TokenRole::Heredoc`](#tokenrole)).

The expected CST shapes for Mox are documented in `tests/support/mox.rs`
(`DqString`, `HeredocStr`).

### Number literals

[`LangPolicy::literals`](#literals) is the decoding table: digit separators
(anywhere, only between digits as in PHP 7.4+, or never), what a leading `0`
means (decimal, octal as in PHP and C, or an error as in Python 3), and
whether hexadecimal floats (`0x1.8p3`) are accepted. Integers too large for
64 bits follow [`LangPolicy::overflow`](#langpolicy): with `Promote` (PHP) a
value above `i64::MAX` is the nearest `f64`; otherwise a decimal value is an
exact big integer and a binary, octal, or hex one is converted to its exact
decimal digits (up to 1,024). Raw strings ([`TokenRole::RawStr`](#tokenrole):
Rust `r#"…"#`, Python `r'…'`) are taken verbatim.

### Operators by kind or by text

Operator tables are keyed by token kind (`binary_op(kind, spec)`) or, for
lexers that give every operator one kind, by the token's text
(`binary_op_text("+=", spec)`). The kind entry wins when both match.

### Use trees, mixin rules, attributes

- [`Arch::UseTree`](#arch) flattens nested imports (`use a::{b, c::{d as e},
  self, *}`, PHP `use App\Models\{User, Post as P}`) into one HIR import per
  leaf, iteratively; `self` imports the prefix, an empty group imports nothing.
- [`Arch::MixinUse`](#arch) takes conflict-resolution `rules`; each
  [`Arch::MixinRule`](#arch) is `T::m insteadof U` (an exclusion) or
  `T::m as protected n` (an alias with a new name, a visibility, or both),
  lowered to hir-lang's `MixinRule`.
- [`Arch::Attr`](#arch) nodes (`#[Route("/x", methods: GET)]`) are attached to
  the HIR node of the construct they are a direct child of, as hir-lang
  attributes. Attribute nodes, and container nodes holding only attributes
  (PHP's `#[A, B]` group), are invisible to every other pick. Arguments must be
  literals or names (HIR attributes are data); anything else is an
  [`AttributeArgument`](#problemkind) problem, and an attribute on something
  with no HIR node to carry it (a wrapper, a capture, a match arm) is a
  [`MisplacedAttribute`](#problemkind) problem.

### Problems and half-typed code

Lowering never fails on code. Syntax-error nodes, unmapped kinds, constructs
marked `Unsupported`, missing required fields, wrong contexts, unknown
operators and labels, undecodable literals and escapes, and spans outside the
source each become an HIR error form plus a [`Problem`](#problem). hir-lang's
`finish_lenient` then repairs and reports what only it can see (a `break`
outside a loop, a duplicate named argument, a misplaced `yield`) as
[`ProblemKind::Hir`](#problemkind). The result is always a valid `Hir`.

## `lower`

```rust,ignore
pub fn lower<K>(tree: &Node<K>, source: &str, mapping: &Mapping<K>, names: &mut Interner)
    -> Result<Lowered, LowerError>
where K: Copy + Ord + TokenKind
```

Lowers `tree` (parsed from `source`) into a validated HIR unit with the
default [`Budget`](#budget), unit 0, and no field labels.

| Parameter | Meaning |
|---|---|
| `tree` | The root of a syntax-lang tree. If its kind is not a `Module` archetype, it is lowered as the only element of a synthetic root module. |
| `source` | The text the tree's spans index. |
| `mapping` | The language's semantics. |
| `names` | The session's interner; HIR names are interned in it. |

**Errors:** only for exhausted resources ([`LowerError`](#lowererror)).

```rust
use intern_lang::Interner;
use lower_lang::{lower, Arch, MappingBuilder, Pick};
use syntax_lang::{Node, TokenKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct K(u8);
impl TokenKind for K {}

let mut b = MappingBuilder::new();
b.node(K(0), Arch::Module { name: Pick::Absent, items: Pick::Nodes });
let mapping = b.build()?;
let mut names = Interner::new();
let out = lower(&Node::new(K(0), vec![]), "", &mapping, &mut names).expect("within budget");
assert!(out.problems.is_empty());
# Ok::<(), lower_lang::MappingError>(())
```

## `Lowerer`

```rust,ignore
pub struct Lowerer<'m, K> { /* private */ }
pub fn new(mapping: &'m Mapping<K>) -> Self
pub fn with_budget(self, budget: Budget) -> Self
pub fn with_unit(self, unit: UnitId) -> Self
pub fn with_labeler(self, labeler: &'m dyn Labeler<K>) -> Self
pub fn lower(&self, tree: &Node<K>, source: &str, names: &mut Interner) -> Result<Lowered, LowerError>
```

The configured path. `new` uses the default budget, unit 0, and no labels;
the `with_*` methods replace one setting each. `lower` behaves as
[`lower`](#lower).

```rust
use hir_lang::UnitId;
use intern_lang::Interner;
use lower_lang::{Arch, Budget, Lowerer, MappingBuilder, Pick};
use syntax_lang::{Node, TokenKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct K(u8);
impl TokenKind for K {}

let mut b = MappingBuilder::new();
b.node(K(0), Arch::Module { name: Pick::Absent, items: Pick::Nodes });
let mapping = b.build()?;
let mut budget = Budget::default();
budget.max_nodes = 1_000;
let out = Lowerer::new(&mapping)
    .with_budget(budget)
    .with_unit(UnitId::new(2))
    .lower(&Node::new(K(0), vec![]), "", &mut Interner::new())
    .expect("within budget");
assert_eq!(out.hir.unit(), UnitId::new(2));
# Ok::<(), lower_lang::MappingError>(())
```

## `Budget`

```rust,ignore
#[non_exhaustive]
pub struct Budget { pub max_nodes: u64, pub max_expansion_depth: u32 }
```

| Field | Default | Exceeded |
|---|---|---|
| `max_nodes` | `1 << 24` | lowering stops with [`LowerError::NodeBudget`](#lowererror). Counts HIR nodes, binders, and list entries built, plus a conservative charge for each copy of a repeated template hole. |
| `max_expansion_depth` | `64` | the template application is an error node and an [`ExpansionTooDeep`](#problemkind) problem; the user code of the instantiation stays attached. |

`#[non_exhaustive]`: start from `Budget::default()` and assign fields.

```rust
let mut b = lower_lang::Budget::default();
b.max_expansion_depth = 8;
assert_eq!(b.max_nodes, 1 << 24);
```

## `Lowered`

```rust,ignore
#[non_exhaustive]
pub struct Lowered { pub hir: Hir, pub problems: Vec<Problem> }
pub fn has_problems(&self) -> bool
```

The valid HIR and every problem, sorted by span start (then discovery).

```rust
use intern_lang::Interner;
use lower_lang::{lower, Arch, MappingBuilder, Pick};
use syntax_lang::{Element, Node, Span, Token, TokenKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct K(u8);
impl TokenKind for K {}

// A module whose only child is a token kind the mapping does not know.
let mut b = MappingBuilder::new();
b.node(K(0), Arch::Module { name: Pick::Absent, items: Pick::All });
let mapping = b.build()?;
let tree = Node::new(K(0), vec![Element::Token(Token::new(K(9), Span::new(0, 1)))]);
let out = lower(&tree, "?", &mapping, &mut Interner::new()).expect("within budget");
assert!(out.has_problems());
assert_eq!(out.hir.validate(), Ok(()));
# Ok::<(), lower_lang::MappingError>(())
```

## `Mapping`

```rust,ignore
pub struct Mapping<K> { /* private */ }   // Clone, Debug
pub fn rule_for(&self, kind: &K) -> Option<&Arch<K>>
pub fn role_for(&self, kind: &K) -> Option<TokenRole>
pub fn modifier_for(&self, kind: &K) -> Option<Modifier>
pub fn template(&self, id: TemplateId) -> Option<&Template>
pub const fn policy(&self) -> &LangPolicy
```

A language's semantics, immutable once built; lowers any number of trees.
`rule_for` returns the first rule registered for a node kind (conditional
rules are tried in registration order during lowering); the other accessors
read the token, modifier, template, and policy tables.

```rust
use lower_lang::{Arch, MappingBuilder, Modifier, TokenRole};

let mut b = MappingBuilder::<u8>::new();
b.node(1, Arch::binary()).token(2, TokenRole::Name).modifier(3, Modifier::Mut);
let m = b.build()?;
assert_eq!(m.rule_for(&1), Some(&Arch::binary()));
assert_eq!(m.role_for(&2), Some(TokenRole::Name));
assert_eq!(m.modifier_for(&3), Some(Modifier::Mut));
# Ok::<(), lower_lang::MappingError>(())
```

## `MappingBuilder`

```rust,ignore
pub struct MappingBuilder<K> { /* private */ }   // Default
```

Every method records and returns `&mut Self` (chainable); `build` checks.

| Method | Records |
|---|---|
| `new()` | an empty builder with the default [`LangPolicy`](#langpolicy) |
| `node(kind, arch)` | nodes of `kind` mean `arch` |
| `node_when(kind, when, arch)` | ... when the pick `when` finds a child; otherwise the next rule for `kind` is tried (grammars that put alternatives in one rule) |
| `token(kind, role)` | tokens of `kind` mean [`role`](#tokenrole) |
| `modifier(kind, modifier)` | tokens of `kind` set a [`Modifier`](#modifier) on the construct they appear in |
| `root(kind, root)` | leading tokens of `kind` in a path set its `hir_lang::PathRoot` (`Super(1)` per `super`) |
| `container(kind)` | list picks splice nodes of `kind` |
| `skip(kind)` | list picks leave out tokens (or nodes) of `kind` |
| `error(kind)` | nodes of `kind` are syntax errors |
| `type_name(text, name)` | an identifier spelled `text` is a [`TypeName`](#typename) |
| `suffix(text, prim)` | numbers ending in `text` have type `prim` |
| `prefix_op(kind, spec)`, `binary_op(kind, spec)`, `postfix_op(kind, spec)` | operator tokens and their [`OpSpec`](#opspec) |
| `prefix_op_text(text, spec)`, `binary_op_text(text, spec)`, `postfix_op_text(text, spec)` | operators found by the token's text (`"+="`), for lexers that give operators one kind; a kind entry wins over a text entry |
| `name(text) -> NameRef` | stores a host or intrinsic name for `OpSpec::Extern` / `Intrinsic` |
| `policy(policy)` | the [`LangPolicy`](#langpolicy) |
| `template(t) -> Result<TemplateId, MappingError>` | registers a [`Template`](#template); its `apply`s must name templates registered before it, with matching arguments |
| `template_node(kind, id, holes)` | nodes of `kind` lower through template `id`, one pick per hole in hole order |
| `build() -> Result<Mapping<K>, MappingError>` | checks and sorts everything |

**Errors (`build`):** a required field without a pick, a kind with two
entries in a one-entry table (roles, modifiers, roots, each operator
position, each operator text), a duplicate type name, an unknown template or
a wrong number of template picks, an operator spec that cannot apply at its
position, an empty operator text, or an interpolation [`Join`](#join)
template of the wrong shape. See [`MappingError`](#mappingerror).

```rust
use hir_lang::OpKind;
use lower_lang::{templates, Arch, MappingBuilder, OpSpec, Pick, TokenRole};

const STMT: u8 = 1;
const LET: u8 = 2;
const IDENT: u8 = 3;
const AND: u8 = 4;
let mut b = MappingBuilder::new();
let and = b.template(templates::logical_and())?;
b.node_when(STMT, Pick::First(LET), Arch::Let {
    pat: Pick::Kind(IDENT), ty: Pick::Absent, init: Pick::Back(0),
    else_: Pick::Absent, mutable: false, hoist: false,
});
b.node(STMT, Arch::ExprStmt { expr: Pick::Nth(0) });
b.token(IDENT, TokenRole::Name);
b.binary_op(AND, OpSpec::Template(and));
assert!(b.build().is_ok());

let mut bad = MappingBuilder::<u8>::new();
bad.binary_op(9, OpSpec::Op(OpKind::Neg)); // `neg` takes one operand
assert!(bad.build().is_err());
# Ok::<(), lower_lang::MappingError>(())
```

## `Arch`

```rust,ignore
#[non_exhaustive]
pub enum Arch<K> { /* struct-like variants whose fields are Pick<K>s */ }
pub const fn pass(n: u16) -> Self        // Pass { inner: Nth(n) }
pub const fn binary() -> Self             // Binary { lhs: Nth(0), op: Nth(1), rhs: Nth(2) }
pub const fn prefix() -> Self             // Unary { op: Nth(0), operand: Nth(1) }
pub const fn name(&self) -> &'static str
```

The archetype catalogue. Fields marked *req* are required: a node that lacks
one gets an error node there plus a `MissingField` problem (and a mapping that
leaves one `Absent` is refused). *List* fields take every match. The
"Modifiers" column lists the modifiers each archetype reads.

**Structure**

| Variant | Fields | Lowers to | Contexts | Modifiers |
|---|---|---|---|---|
| `Module` | `name`, `items` (list) | `ItemKind::Module` with items and a top-level body | root, statement, member | visibility |
| `Pass` | `inner` (req) | its child, in the same context | any | |
| `Splice` | `items` (list) | several elements spliced into the enclosing list | list contexts | |
| `Unsupported` | | an error node + `Unsupported` | any | |

**Statements and binders**

| Variant | Fields | Lowers to | Modifiers |
|---|---|---|---|
| `Let` | `pat` (req), `ty`, `init`, `else_`, `mutable: bool`, `hoist: bool` | `Stmt::Let`; hoisted: a function-level `let` plus an assignment (a destructuring pattern binds temporaries assigned to the hoisted names) | `Mut` |
| `ExprStmt` | `expr` (req) | `Stmt::Expr` (never a block tail) | |
| `Defer` | `body` (req) | `Stmt::Defer` | |
| `Static` | `name` (req), `ty`, `init` | `Stmt::Static` | |
| `GlobalDecl` | `names` (list) | one `Stmt::Global` per name | |
| `Import` | `path` (req), `alias` | `ItemKind::Import` | `Glob`, visibility |
| `UseTree` | `path`, `alias`, `children` (list) | one `ItemKind::Import` per leaf of a nested tree (`use a::{b, c::{d as e}, self, *}`); prefixes concatenate, `self` imports the prefix, an empty group imports nothing | `Glob` (a `*` leaf), visibility (outermost tree) |

**Declarations** (statement or member contexts)

| Variant | Fields | Lowers to | Modifiers |
|---|---|---|---|
| `Fn` | `name` (req), `generics`, `params` (lists), `ret`, `throws`, `body`, `where_` (list) | `ItemKind::Fn` | visibility, `Async`, `Generator`, `Throws`, `Unsafe` |
| `Record` | `name` (req), `generics`, `fields` (lists), `shape: RecordShape` | `ItemKind::Record` | visibility, `Union` |
| `Sum` | `name` (req), `generics`, `variants` | `ItemKind::Sum` | visibility |
| `Interface` | `name` (req), `generics`, `supers`, `items` | `ItemKind::Interface` | visibility |
| `Class` | `name` (req), `generics`, `bases`, `interfaces`, `members`, `mixin: bool` | `ItemKind::Class` (fields and member items) | visibility, `Abstract`, `Final` |
| `Impl` | `generics`, `interface`, `self_ty` (req), `items` | `ItemKind::Impl` | |
| `Alias` | `name` (req), `generics`, `ty` (req) | `ItemKind::Alias` | visibility |
| `AssocType` | `name` (req), `bounds`, `default` | `ItemKind::AssocType` (members only) | |
| `Const` | `name` (req), `ty`, `value` | `ItemKind::Const` | visibility |
| `GlobalVar` | `name` (req), `ty`, `init`, `mutable: bool` | `ItemKind::Global` | visibility, `Mut` |
| `MixinUse` | `mixins` (list), `rules` (list) | `ItemKind::MixinUse` with its rules (members only) | |
| `MixinRule` | `from`, `method` (req), `insteadof` (list), `alias` | a `MixinRule`: `T::m insteadof U` excludes; `T::m as n` / `m as protected` aliases (a rule with neither is a `MissingField` problem, field `action`) | `Public`, `Protected`, `Private` (the alias's visibility) |
| `FieldDecl` | `name`, `ty`, `default` | a `FieldDef` (or, for a `Static` class member, `ItemKind::Global`) | visibility, `Static` |
| `VariantDecl` | `name` (req), `fields`, `discriminant`, `shape` | a `Variant` | |
| `GenericParam` | `name` (req), `bounds`, `ty`, `default` | a `GenericParam` (type, const, or region parameter) | |
| `WherePred` | `subject` (req), `bounds` | a `WherePred` | |
| `Param` | `pat` (req), `ty`, `default` | a `Param` | `Ref`, `Mut`, `Rest`, `RestNamed`, `NamedOnly`, `PositionalOnly`, `Receiver` |
| `Capture` | `name` (req) | a `Capture` (fresh capture binder) | `Ref`, `Mut` |

In a parameter list, a token with the `NamedOnly` modifier (`*`) makes the
parameters after it named-only; one with `PositionalOnly` (`/`) makes the
ones before it positional-only.

**Expressions**

| Variant | Fields | Lowers to |
|---|---|---|
| `Path` | `segments` (req unless qualified), `global`, `qself`, `qtrait` | `Expr::Path` / `Ty::Path` / `Pat::Path` / a pattern binding, by context |
| `Segment` | `name` (req), `args` | a path segment with generic arguments |
| `AssocBinding` | `name`, `ty` (req) | `GenericArg::Binding` |
| `Unary`, `Binary`, `Postfix` | `op` (req) and operands (req) | per the operator's [`OpSpec`](#opspec) |
| `Assign` | `target`, `op`, `value` | `Expr::Assign` (compound with a binary `Compound` op) |
| `RefAssign` | `target`, `source` (req) | `Expr::RefAssign` |
| `Call` | `callee` (req), `args` (list) | `Expr::Call`; in a pattern, `Pat::Ctor` |
| `Arg` | `name`, `value` (req) | a named, spread (`Spread`/`SpreadNamed`), or place (`Ref`) argument |
| `Member` | `base`, `member` (req), `args`, `call` | `Expr::Field` (`a.b`, `a.0`), or `Expr::MethodCall` when `call` finds a child |
| `MethodCall` | `receiver`, `method` (req), `generics`, `args` | `Expr::MethodCall` |
| `DynMember` | `base`, `name` (req), `args`, `call` | `Expr::DynField` / `Expr::DynMethodCall` |
| `Index`, `VarVar`, `Append` | | `Expr::Index`, `Expr::VarVar`, `Expr::Append` |
| `Tuple`, `Array` | `elems` (list) | tuples and arrays; in patterns tuple and slice patterns; in types tuple and slice types |
| `Map`, `MapEntry` | `entries`; `key`, `value` (req) | `Expr::Map` (unkeyed elements take the next index) |
| `RecordLit`, `FieldInit` | `path`, `fields`, `base`; `name` (req), `value` | `Expr::Record` (shorthand `{ x }` reads `x`); in patterns `Pat::Record` |
| `Repeat` | `elem`, `count` (req) | `Expr::Repeat` |
| `Cast` | `expr`, `ty` (req) | `Expr::Cast` with the language's cast policy |
| `Borrow`, `Deref` | | `Expr::Borrow` (`Mut` modifier), `Expr::Deref` |
| `Block` | `stmts` (list), `tail`, `label`, `tail_from_last: bool` | `Expr::Block` (`Unsafe` modifier) |
| `If` | `cond`, `then` (req), `else_` | `Expr::If` |
| `Match`, `Arm` | `scrutinee` (req), `arms`; `pat`, `body` (req), `guard` | `Expr::Match` |
| `Loop` | `label`, `body` (req) | `Expr::Loop` |
| `Break`, `Continue` | `label`, `value` | jumps; a label no enclosing loop or block has is an `UnknownLabel` problem |
| `Return` | `value` | `Expr::Return` |
| `Throw`, `Try`, `Catch` | `value`; `body`, `catches`, `finally`; `types`, `name`, `body` | `Expr::Throw`, `Expr::Try`; a catch is an arm whose pattern is a type test (an or-pattern for several types) binding `name` |
| `Await`, `Yield`, `Spawn` | | effects |
| `Closure` | `params`, `ret`, `captures`, `body` (req), `implicit` | `Expr::Closure` (`Async`, `Generator`, `Throws` modifiers) |
| `Template` | (a [`TemplateUse`](#templateuse)) | the template's instantiation |
| `Interp` | `parts` (list), `close`, `join: Join` | an interpolated string or heredoc: text pieces (`StrPart` tokens) and embedded expressions joined by the [`Join`](#join); `close` picks a heredoc's closing marker (indentation stripping) |

**Attributes**

| Variant | Fields | Lowers to |
|---|---|---|
| `Attr` | `name` (req), `args` (list) | an HIR attribute (`Attr`) on the construct the node is a direct child of; the name is kept as written (`\App\Route`) |
| `AttrArg` | `key`, `value` | a named argument (`methods: GET`), or a bare key; values must be literals or names |

**Patterns**: `WildPat`, `RestPat { bind }`, `BindPat { name, sub }`
(`Ref`, `Mut`; `x @ ..` binds a slice rest), `TuplePat`, `CtorPat`,
`RecordPat`, `FieldPat`, `OrPat` (alternatives share one binder per name),
`RangePat` (`Inclusive`; bare names in bounds are constants), `RefPat`
(`Mut`), `SlicePat`, `TypeTestPat`.

**Types**: `TupleTy`, `ArrayTy`, `SliceTy`, `RefTy { inner, region }`
(`Mut`), `PtrTy` (`Mut`), `NullableTy`, `FnTy`, `ObjectTy`, `ImplTy`.

```rust
use lower_lang::{Arch, Pick};

assert_eq!(Arch::<u8>::pass(0), Arch::Pass { inner: Pick::Nth(0) });
assert_eq!(Arch::<u8>::binary().name(), "binary");
let closure: Arch<u8> = Arch::Closure {
    params: Pick::Kind(4), ret: Pick::Absent, captures: Pick::Absent,
    body: Pick::Back(0), implicit: Some(hir_lang::CaptureMode::ByRef),
};
assert_eq!(closure.name(), "closure");
```

## `RecordShape`

`Auto` (default: tuple-like when every field is unnamed, named otherwise; a
variant without fields is a unit variant), `Named`, `Tuple`, `Unit`.

```rust
assert_eq!(lower_lang::RecordShape::default(), lower_lang::RecordShape::Auto);
```

## `TemplateUse`

```rust,ignore
pub struct TemplateUse { /* private */ }   // Copy
pub const fn template(&self) -> TemplateId
```

The archetype a node gets from
[`MappingBuilder::template_node`](#mappingbuilder): which template, and which
picks fill its holes.

```rust
use lower_lang::{templates, Arch, MappingBuilder, Pick};

let mut b = MappingBuilder::<u8>::new();
let t = b.template(templates::coalesce())?;
b.template_node(1, t, &[Pick::Nth(0), Pick::Nth(2)]);
let m = b.build()?;
let Some(Arch::Template(used)) = m.rule_for(&1) else { unreachable!() };
assert_eq!(used.template(), t);
# Ok::<(), lower_lang::MappingError>(())
```

## `Pick`

```rust,ignore
#[non_exhaustive]
pub enum Pick<K> { Absent, Label(u16), Nth(u16), Back(u16), NthNode(u16), Kind(K),
                   KindNth(K, u16), First(K), After(K), Before(K), All, Nodes, AllKind(K),
                   Range(u16, u16), AllAfter(K), Between(K, K) }
pub const fn is_absent(&self) -> bool          // Default: Absent
```

| Pick | Selects |
|---|---|
| `Absent` | nothing: the construct has no such field |
| `Label(l)` | the child (list: every child) whose host label is `l` ([`Labeler`](#labeler)) |
| `Nth(n)`, `Back(n)`, `NthNode(n)` | the `n`-th significant child, the `n`-th from the end, or the `n`-th child node |
| `Kind(k)`, `KindNth(k, n)`, `First(k)` | the first (`n`-th) child of kind `k`; the first child if it is of kind `k` |
| `After(k)`, `Before(k)` | the child right after / before the first child of kind `k` |
| `All`, `Nodes`, `AllKind(k)` | list: every non-skip child; every child node; every child of kind `k` |
| `Range(from, back)` | list: positions `from .. len - back` |
| `AllAfter(k)`, `Between(a, b)` | list: everything after the first `k`; everything between the first `a` and the next `b` |

Used where one child is expected, a list pick takes its first match. List
picks splice container kinds and leave out skip kinds (except `Nodes`,
`AllKind`, and `Label`, which keep exactly what they name). Attribute nodes
(kinds with an [`Attr`](#arch) rule), and container nodes holding nothing but
attributes, are invisible to every pick: positions are counted without them.

```rust
use lower_lang::Pick;

let field: Pick<u8> = Pick::After(5);
assert!(!field.is_absent());
assert!(Pick::<u8>::default().is_absent());
```

## `Labeler`

```rust,ignore
pub trait Labeler<K> {
    fn label(&self, parent: &Node<K>, index: usize) -> Option<u16>;
}
```

Supplies field labels (LSF2 `name:expr`) for `Pick::Label`. `index` is the
child's position among **all** of `parent`'s children, trivia included.
Install one with [`Lowerer::with_labeler`](#lowerer).

```rust
use lower_lang::Labeler;
use syntax_lang::Node;

/// Labels the first child of every node `0` (`target`).
struct FirstIsTarget;
impl<K> Labeler<K> for FirstIsTarget {
    fn label(&self, _: &Node<K>, index: usize) -> Option<u16> {
        (index == 0).then_some(0)
    }
}
assert_eq!(FirstIsTarget.label(&Node::<u8>::new(1, vec![]), 0), Some(0));
```

## `TokenRole`

`#[non_exhaustive]`: `Name`, `Var { sigil }` (`$x`), `Label { sigil }`
(`'outer`), `Lifetime { sigil }` (`'a`), `Int`, `Float`, `Number` (a float if
it has a fraction or exponent), `Str(Escapes)`, `RawStr`, `Heredoc(Escapes)`,
`StrPart(Escapes)`, `Char(Escapes)`, `Bytes(Escapes)`, `True`, `False`,
`Null`, `Wild` (`_`: a wildcard pattern or an inferred type), `Rest` (`..` in
patterns), `Type(TypeName)` (a type keyword). `sigil` is the number of
leading bytes that are not part of the name.

| Role | Token text | Value |
|---|---|---|
| `Str(e)` | quotes included | the body, escapes decoded per `e` |
| `RawStr` | optional prefix letters (`r`, `R`, `br`), optional `#` fences, one quote or a tripled one (`"""`) | the body verbatim (Rust `r#"a "b""#`, Python `r'\d+'`); malformed fences are `InvalidLiteral` |
| `Heredoc(e)` | `<<<ID` (or `<<<"ID"`, `<<<'ID'`), a newline, the body, the closing `ID` on its own line | the body with the closing marker's indentation removed from every line (PHP 7.3+) and the final newline dropped; `Standard` for a heredoc, `None` for a nowdoc |
| `StrPart(e)` | a piece of an [`Interp`](#arch) string, no quotes | the piece, escapes decoded |

Numbers accept `0x`/`0o`/`0b` prefixes, digit separators, legacy octal, and
hex floats as the language's [`Literals`](#literals) table allows, and
registered suffixes; integers beyond 64 bits follow the overflow policy (see
[Number literals](#number-literals)).

```rust
use lower_lang::{Escapes, TokenRole};

assert_ne!(TokenRole::Var { sigil: 1 }, TokenRole::Name);
assert_eq!(TokenRole::Str(Escapes::Standard), TokenRole::Str(Escapes::Standard));
assert_ne!(TokenRole::Heredoc(Escapes::Standard), TokenRole::Heredoc(Escapes::None));
```

### `Escapes`

`None` (raw), `Quotes` (only `\\` and an escaped quote, PHP single-quoted),
`Standard` (`\n \r \t \0 \\ \' \" \$ \xHH \u{H..}`; any other escape is an
`InvalidEscape` problem and is kept as written).

```rust
assert_ne!(lower_lang::Escapes::None, lower_lang::Escapes::Quotes);
```

### `TypeName`

`Prim(Prim)`, `Any`, `Never`, `SelfTy`, `Infer`: what a type keyword (role
`Type`) or a registered type name means.

```rust
use hir_lang::Prim;
assert_eq!(lower_lang::TypeName::Prim(Prim::I32), lower_lang::TypeName::Prim(Prim::I32));
```

## `Modifier`

`#[non_exhaustive]`: `Public`, `Protected`, `Private`, `Package`, `Mut`,
`Ref`, `Rest`, `RestNamed`, `NamedOnly`, `PositionalOnly`, `Receiver`,
`Async`, `Generator`, `Throws`, `Unsafe`, `Static`, `Abstract`, `Final`,
`Glob`, `Inclusive`, `Spread`, `SpreadNamed`, `Union`. Read from the direct
significant children of the archetypes that consult them (and the tokens
directly inside container children). Visibility is the most public modifier
present, `Private` by default.

```rust
let mut b = lower_lang::MappingBuilder::<u8>::new();
b.modifier(1, lower_lang::Modifier::Async);
assert!(b.build().is_ok());
```

## `LangPolicy`

```rust,ignore
#[non_exhaustive]
pub struct LangPolicy {
    pub overflow: Overflow, pub div_zero: DivZero, pub shift: Shift, pub float_to_int: FloatToInt,
    pub throws: EffectMode, pub yields: EffectMode, pub asyncs: EffectMode,
    pub implicit_locals: bool, pub ident_patterns: IdentPatterns, pub place_args: PlaceArgs,
    pub function_scoped_bindings: bool, pub literals: Literals,
}
pub fn php() -> Self   // the PHP / Mox preset
```

| Field | Default | Meaning |
|---|---|---|
| `overflow`, `div_zero`, `shift`, `float_to_int` | `Error` each | the OPS policies of every op that consults them (`OpSpec::Op`, template `op`) |
| `throws`, `yields`, `asyncs` | `Declared` | when frames get `THROWS`, `YIELD`, `ASYNC` ([`EffectMode`](#effectmode)) |
| `implicit_locals` | `false` | PHP: assigning to a plain variable declares it for the enclosing body |
| `ident_patterns` | `Ident` | bare identifiers in match arms ([`IdentPatterns`](#identpatterns)) |
| `place_args` | `Marked` | which arguments are places ([`PlaceArgs`](#placeargs)) |
| `function_scoped_bindings` | `false` | PHP: `for-each` pattern holes and `catch` names belong to the enclosing function ([Function-scoped bindings](#function-scoped-bindings-php)) |
| `literals` | `Literals::default()` | how number literals are spelled ([`Literals`](#literals)) |

A cast carries the language's `overflow` and `float_to_int` policies, except
that `Promote` (valid only for dynamically typed results) becomes `Error`.

`LangPolicy::php()` is the PHP and Mox preset: `overflow = Promote`, `throws =
Always`, `yields = Inferred`, `implicit_locals`, `place_args = Places`,
`function_scoped_bindings`, and PHP's number spelling (`_` between digits,
`0777` octal, no hex floats); every other field is the default.

```rust
use hir_lang::Overflow;
use lower_lang::{LangPolicy, LegacyOctal};

let mut p = LangPolicy::default();
p.overflow = Overflow::Wrap;
assert_eq!(p.overflow, Overflow::Wrap);

let php = LangPolicy::php();
assert!(php.function_scoped_bindings && php.implicit_locals);
assert_eq!(php.literals.legacy_octal, LegacyOctal::Octal);
```

### `EffectMode`

`Declared` (default: modifiers only), `Inferred` (plus what the body does
directly), `Always` (every function, closure, and module body).

### `IdentPatterns`

`Ident` (default: `Pat::Ident`, the resolver decides between a constant and a
binding), `Bind` (always a binding). Outside match arms a bare identifier
always binds.

### `PlaceArgs`

`Marked` (default: only `Ref`-modified arguments), `Places` (every argument
that is a place: a variable, field, index, `$$var`, dereference).

```rust
use lower_lang::{EffectMode, IdentPatterns, PlaceArgs};
assert_eq!(EffectMode::default(), EffectMode::Declared);
assert_eq!(IdentPatterns::default(), IdentPatterns::Ident);
assert_eq!(PlaceArgs::default(), PlaceArgs::Marked);
```

## `Literals`

```rust,ignore
#[non_exhaustive]
pub struct Literals { pub separators: Separators, pub legacy_octal: LegacyOctal, pub hex_floats: bool }
```

The number-literal decoding table, in [`LangPolicy::literals`](#langpolicy).

| Field | Default | PHP (`LangPolicy::php()`) | Meaning |
|---|---|---|---|
| `separators` | `Anywhere` | `BetweenDigits` | where `_` may appear ([`Separators`](#separators)) |
| `legacy_octal` | `Decimal` | `Octal` | what `0777` means ([`LegacyOctal`](#legacyoctal)) |
| `hex_floats` | `true` | `false` | whether `0x1.8p3` (= 12.0) is accepted; decoded with correct rounding, subnormals included |

A literal the table rejects is an [`InvalidLiteral`](#problemkind) problem.

```rust
use lower_lang::{LegacyOctal, Literals, Separators};

let mut php = Literals::default();
php.separators = Separators::BetweenDigits;
php.legacy_octal = LegacyOctal::Octal;
php.hex_floats = false;
assert!(Literals::default().hex_floats);
```

### `Separators`

`Anywhere` (default: after the first digit, repeated and trailing allowed;
Rust), `BetweenDigits` (only between two digits of the radix: `1_000`,
`0xFF_FF`, not `1__0`, `10_`, `0x_FF`, `1_.5`; PHP 7.4+, JavaScript), `None`
(C, PHP before 7.4).

### `LegacyOctal`

`Decimal` (default: `0777` is 777; Rust), `Octal` (`0777` is 511, `08` is
invalid; C, PHP), `Error` (Python 3; `0`, `00`, and `0.5` stay valid).

```rust
use lower_lang::{LegacyOctal, Separators};
assert_eq!(Separators::default(), Separators::Anywhere);
assert_eq!(LegacyOctal::default(), LegacyOctal::Decimal);
```

## `OpSpec`

```rust,ignore
#[non_exhaustive]
pub enum OpSpec { Op(OpKind), OpWith(Op), Compound(OpKind), CompoundWith(Op), Assign, RefAssign,
                  Identity, Borrow(BorrowKind), Deref, Await, Template(TemplateId),
                  Extern(NameRef), Intrinsic(NameRef) }
```

| Spec | Lowers to | Positions |
|---|---|---|
| `Op(kind)` | `Expr::Op` with the language's policies in exactly the fields `kind` consults | arity must match |
| `OpWith(op)` | `Expr::Op` with exactly `op`'s policy (must fit the op) | arity must match |
| `Compound(kind)`, `CompoundWith(op)` | `Expr::Assign` with a compound op (`add sub mul div floor_div rem floor_mod and or xor shl shr`) | binary |
| `Assign`, `RefAssign` | `Expr::Assign`, `Expr::RefAssign` | binary |
| `Identity` | the operand | prefix, postfix |
| `Borrow(kind)`, `Deref` | `Expr::Borrow`, `Expr::Deref` | prefix |
| `Await` | `Expr::Await` | prefix, postfix |
| `Template(id)` | the template, its expression holes filled by the operands in order | the template must have one expression hole per operand |
| `Extern(name)` | a call of the host function `name` (`Res::Extern`) on the operands | any |
| `Intrinsic(name)` | `Expr::Intrinsic { kind: Named(name) }` on the operands | any |

```rust
use hir_lang::{Op, OpKind, Overflow};
use lower_lang::{MappingBuilder, OpSpec};

let mut b = MappingBuilder::<u8>::new();
let concat = b.name("mox.concat");
b.binary_op(1, OpSpec::OpWith(Op::new(OpKind::Add).with_overflow(Overflow::Wrap)))
    .binary_op(2, OpSpec::Extern(concat))
    .binary_op(3, OpSpec::Compound(OpKind::Mul));
assert!(b.build().is_ok());
```

### `Fixity`

`Prefix`, `Binary`, `Postfix`; `arity()` is 1, 2, 1. Names an operator
position in [`MappingError::BadOperator`](#mappingerror).

```rust
assert_eq!(lower_lang::Fixity::Postfix.arity(), 1);
```

### `NameRef`

A string stored in a mapping by [`MappingBuilder::name`](#mappingbuilder),
interned per lowering run.

```rust
let mut b = lower_lang::MappingBuilder::<u8>::new();
assert_ne!(b.name("a"), b.name("b"));
```

## `Join`

```rust,ignore
#[non_exhaustive]
pub enum Join { Fold(NameRef), Call(NameRef), Template(TemplateId) }
```

How the pieces of an [`Arch::Interp`](#arch) string become one string.

| Join | `"a{$x}b"` lowers to |
|---|---|
| `Fold(f)` | `f(f("a", x), "b")`: a host function of two arguments, folded left (PHP's `.`) |
| `Call(f)` | `f("a", x, "b")`: one host call with every piece |
| `Template(id)` | a template with two expression holes, folded left like `Fold`; or one with one argument-list hole, receiving every piece. Any other shape is refused by `build` ([`MappingError::BadJoin`](#mappingerror)) |

Every instantiation of a `Template` join records its expansion like any other
template use.

```rust
use lower_lang::{Arch, Join, MappingBuilder, Pick};

let mut b = MappingBuilder::<u8>::new();
let concat = b.name("mox.concat");
b.node(1, Arch::Interp { parts: Pick::All, close: Pick::Absent, join: Join::Fold(concat) });
assert!(b.build().is_ok());
```

## `Template`

```rust,ignore
pub struct Template { /* private */ }   // Clone, Debug, PartialEq
pub fn name(&self) -> &str
pub const fn expansion_kind(&self) -> ExpnKind
pub fn hole_count(&self) -> usize
pub fn hole(&self, index: usize) -> Option<(&str, HoleKind, bool)>   // name, kind, optional
```

A checked template, from [`TemplateBuilder::finish`](#templatebuilder).

```rust
use lower_lang::HoleKind;

let t = lower_lang::templates::for_each("iter", "next");
assert_eq!(t.name(), "foreach");
assert_eq!(t.hole(0), Some(("source", HoleKind::Expr, false)));
assert_eq!(t.hole(1), Some(("key", HoleKind::Pat, true)));
```

## `TemplateBuilder`

```rust,ignore
pub struct TemplateBuilder { /* private */ }
pub fn new(name: &str) -> Self
pub fn desugar(self) -> Self
pub fn finish(self, root: TExpr) -> Result<Template, TemplateError>
```

Builds a template bottom-up; every node method is infallible and returns a
typed handle, so expressions, patterns, types, and statements cannot be mixed
up. `desugar` records `Desugar` expansions instead of `Template` ones.

| Group | Methods |
|---|---|
| holes | `expr_hole(name)`, `optional_expr_hole(name, default: Option<TLit>)`, `pat_hole(name, optional)`, `ty_hole(name, optional)`, `stmts_hole(name)`, `args_hole(name)`, `label_hole(name)` |
| binders | `binder(name)`, `mutable_binder(name)`, `label(name)` |
| expressions | `hole`, `splice` (an argument hole, only in `call`/`method`/`tuple`/`array` lists), `lit`, `str`, `var`, `global(path)` (a marked free name, `a::b` splits), `extern_(name)` (a host symbol), `call`, `method`, `field`, `index`, `op` (language policy), `op_with` (exact policy), `assign`, `block`, `labeled_block`, `if_`, `match_`, `loop_`, `break_`, `continue_`, `return_`, `throw`, `await_`, `yield_`, `spawn`, `tuple`, `array`, `closure`, `apply` |
| statements | `let_`, `expr_stmt`, `defer`, `stmts` (a statement-list hole) |
| patterns | `pat`, `wild`, `bind`, `lit_pat`, `tuple_pat`, `ctor_pat(path, elems)` |
| types | `ty`, `prim`, `named_ty`, `infer_ty`, `any_ty` |

An absent optional hole is its default literal if it has one; otherwise it is
left out where its slot is optional (an `else`, a loop step, a `return` value,
a `let` type or initializer), is `_` as a pattern, is empty as a list, and is
an error node plus a [`MissingHole`](#problemkind) problem where a value is
required.

**Errors (`finish`):** [`TemplateError`](#templateerror).

```rust
use lower_lang::{LabelRef, TLit, TemplateBuilder};

// repeat N { body }  ==  { let n = N; 'l: loop { if n == 0 { break 'l } n = n - 1; body } }
let mut t = TemplateBuilder::new("repeat");
let count = t.expr_hole("count");
let body = t.expr_hole("body");
let n = t.mutable_binder("n");
let l = t.label("l");
let c = t.hole(count);
let bind_n = t.bind(n);
let decl = t.let_(bind_n, None, Some(c));
let (n1, zero) = (t.var(n), t.lit(TLit::Int(0)));
let done = t.op(hir_lang::OpKind::Eq, &[n1, zero]);
let brk = t.break_(Some(LabelRef::Own(l)), None);
let test = t.if_(done, brk, None);
let test = t.expr_stmt(test);
let (n2, n3, one) = (t.var(n), t.var(n), t.lit(TLit::Int(1)));
let dec = t.op(hir_lang::OpKind::Sub, &[n3, one]);
let step = t.assign(n2, dec);
let step = t.expr_stmt(step);
let b = t.hole(body);
let inner = t.block(&[test, step], Some(b));
let lp = t.loop_(Some(LabelRef::Own(l)), inner, None);
let root = t.block(&[decl], Some(lp));
let repeat = t.finish(root)?;
assert_eq!(repeat.hole_count(), 2);
# Ok::<(), lower_lang::TemplateError>(())
```

## Template handles

`TExpr`, `TPat`, `TTy`, `TStmt` (nodes); `TBinder`, `TLabel` (the template's
own binders and labels); `ExprHole`, `PatHole`, `TyHole`, `StmtsHole`,
`ArgsHole`, `LabelHole` (holes). All are `Copy` handles valid only for the
builder that made them; a foreign handle is refused by `finish`
(`ForeignHandle`).

## `TemplateId`

A registered template, from [`MappingBuilder::template`](#mappingbuilder);
`index()` is its registration position.

```rust
let mut b = lower_lang::MappingBuilder::<u8>::new();
let id = b.template(lower_lang::templates::do_while())?;
assert_eq!(id.index(), 0);
# Ok::<(), lower_lang::MappingError>(())
```

### `HoleKind`

`Expr`, `Pat` (names bind as locals), `Ty`, `Stmts` (spliced into a block),
`Args` (spliced into an argument or element list), `Label` (the user's label
for a loop or block the template creates).

### `TLit`

`Int(i64)`, `Float(f64)`, `Bool(bool)`, `Null`, `Char(char)`: literals a
template writes (strings: `TemplateBuilder::str`).

### `LabelRef`

`Own(TLabel)` or `Hole(LabelHole)`: a loop's or block's label, or a jump's
target.

### `TArg`

An `apply` argument per hole of the applied template: `Expr`, `Pat`, `Ty`
(nodes), `Stmts`, `Args` (this template's hole, forwarded),
`Label(LabelRef)`, or `Absent` (only for optional holes).

```rust
use hir_lang::OpKind;
use lower_lang::{templates, MappingBuilder, TArg, TemplateBuilder};

let mut m = MappingBuilder::<u8>::new();
let w = m.template(templates::while_loop())?;
let mut t = TemplateBuilder::new("until");
let (c, b) = (t.expr_hole("cond"), t.expr_hole("body"));
let c = t.hole(c);
let not_c = t.op(OpKind::Not, &[c]);
let b = t.hole(b);
let root = t.apply(w, &[TArg::Expr(not_c), TArg::Expr(b), TArg::Absent]);
let until = t.finish(root).expect("valid");
assert!(m.template(until).is_ok());
# Ok::<(), lower_lang::MappingError>(())
```

## `templates`

Ready-made templates (all record `Desugar` expansions). Hole order is the pick
order for [`template_node`](#mappingbuilder); `?` marks an optional hole.

| Function | Lowers | Holes |
|---|---|---|
| `while_loop()` | `while c { b }` → `'l: loop { if c {} else { break }; b }` | `cond`, `body`, `label`? |
| `do_while()` | `do { b } while c` → `'l: loop [step: if c {} else { break }] { b }` | `body`, `cond`, `label`? |
| `c_for()` | `for (i; c; s) b` → `{ i; 'l: loop [step: s] { if c {} else { break }; b } }` | `init` (statements), `cond`? (default `true`), `step`?, `body`, `label`? |
| `for_each(iter, next)` | `{ let it = iter(src); 'l: loop { match next(it) { null => break, (k, v) => b } } }` | `source`, `key`? (default `_`), `value`, `body`, `label`? |
| `logical_and()`, `logical_or()` | `if a { b } else { false }`, `if a { true } else { b }` | `lhs`, `rhs` |
| `and_value()`, `or_value()` | `{ let t = a; if t { b } else { t } }` and the `or` form (operand-valued) | `lhs`, `rhs` |
| `coalesce()` | `a ?? b` → `{ let t = a; if t == null { b } else { t } }` | `lhs`, `rhs` |
| `try_operator()` | `e?` → `match e { Ok(v) => v, Err(e) => return Err(e) }` | `expr` |
| `post_increment()` | `x++` → `{ let old = x; x = old + 1; old }` (`x` used twice: copied) | `place` |

```rust
assert_eq!(lower_lang::templates::post_increment().hole(0).map(|h| h.0), Some("place"));
```

## `Problem`

```rust,ignore
pub struct Problem { pub kind: ProblemKind, pub span: Span }   // Display: "start..end: message"
```

```rust
use lower_lang::{Problem, ProblemKind, Span};
let p = Problem { kind: ProblemKind::UnknownLabel, span: Span::new(3, 9) };
assert_eq!(p.to_string(), "3..9: no enclosing loop or block has this label");
```

## `ProblemKind`

`#[non_exhaustive]`. Every kind but `Hir` comes from lowering itself.

| Variant | Meaning |
|---|---|
| `SyntaxError` | a syntax-error node from the parser (already reported by it) |
| `Unmapped` | a node or token kind the mapping does not describe |
| `Unsupported` | a construct marked `Arch::Unsupported` |
| `NotAllowedHere { expected: Context }` | a construct in a context it cannot appear in |
| `MissingField { field }` | a required part is missing |
| `AmbiguousChild` | one child was selected for two fields; the later field is left absent |
| `UnknownOperator` | an operator token with no entry for its position |
| `UnknownLabel` | a jump to a label no enclosing loop or block has |
| `InvalidLiteral`, `InvalidEscape` | a literal or escape that does not decode |
| `BadSpan` | a token span outside the source (a host bug) |
| `MissingHole { template }` | a required template hole is absent |
| `ExpansionTooDeep { template }` | a template application past the depth budget |
| `CannotHoist { reason }` | a binding that should be function-scoped (a hoisted `let`, PHP `foreach`/`catch`) but cannot be; it stays scoped ([`HoistBlocker`](#hoistblocker)) |
| `AttributeArgument` | an attribute argument that is neither a literal nor a name; it is left out |
| `MisplacedAttribute` | an attribute on a construct with no HIR node to carry it; it is dropped |
| `Hir(HirError)` | a problem hir-lang's lenient validator repaired |

## `HoistBlocker`

`#[non_exhaustive]`: why a binding could not be function-scoped.
`ByReference` (an assignment from a temporary would copy, not alias),
`ReusedHole` (the template binds the pattern hole in more than one place),
`NoBindingSite` (the template's binding site was cut away, e.g. by the depth
budget).

```rust
use lower_lang::{HoistBlocker, ProblemKind};

let p = ProblemKind::CannotHoist { reason: HoistBlocker::ByReference };
assert_eq!(
    p.to_string(),
    "this binding cannot be function-scoped: a by-reference binding would become a copy"
);
```

## `Context`

`#[non_exhaustive]`: `Expression`, `Statement`, `Item`, `Member`, `Pattern`,
`Type`, `Path`, `Name`, `Parameter`, `Argument`, `Arm`, `Capture`, `Field`,
`Variant`, `Generic`, `Entry`, `Segment`, `Label`; `Display` spells it ("a
pattern").

```rust
assert_eq!(lower_lang::Context::Type.to_string(), "a type");
```

## Errors

### `LowerError`

`#[non_exhaustive]`; why lowering stopped without a result.

| Variant | Meaning | What to do |
|---|---|---|
| `NodeBudget { limit }` | more HIR than `Budget::max_nodes` | raise the budget for trusted input, or reject the input |
| `SymbolSpaceExhausted` | the interner ran out of symbols | use a fresh interner per session |
| `Hir(HirError)` | hir-lang refused the result without a repair (capacity) | split the input |

```rust
assert_eq!(
    lower_lang::LowerError::NodeBudget { limit: 5 }.to_string(),
    "lowering would build more than 5 HIR nodes"
);
```

### `MappingError`

`#[non_exhaustive]`: `MissingField { rule, field }`, `DuplicateKind { table,
index }`, `DuplicateTypeName { index }`, `UnknownTemplate { template }`,
`HoleCount { rule, expected, found }`, `ApplyArgs { template }`,
`BadOperator { fixity, index }`, `TooLarge`, `BadJoin { rule }` (an
interpolation [`Join`](#join) template with neither two expression holes nor
one argument-list hole), `EmptyOperatorText { fixity, index }`. Indexes are
registration positions.

```rust
use lower_lang::{Arch, MappingBuilder, MappingError, Pick};
let mut b = MappingBuilder::<u8>::new();
b.node(1, Arch::Loop { label: Pick::Absent, body: Pick::Absent });
assert_eq!(b.build().unwrap_err(), MappingError::MissingField { rule: 0, field: "body" });
```

### `TemplateError`

`#[non_exhaustive]`: `HoleUnused { hole }`, `BinderUnbound { binder }`,
`BinderBoundTwice { binder }`, `LabelUndefined { label }`,
`LabelDefinedTwice { label }`, `SpliceOutOfList { hole }`, `ForeignHandle`,
`TooLarge` (more than [`MAX_TEMPLATE_SIZE`](#limits) nodes with shared
sub-trees counted at every use).

```rust
use lower_lang::{TemplateBuilder, TemplateError};
let mut t = TemplateBuilder::new("t");
let _unused = t.expr_hole("x");
let root = t.lit(lower_lang::TLit::Null);
assert_eq!(t.finish(root), Err(TemplateError::HoleUnused { hole: 0 }));
```

## Re-exports

| Item | From | Why |
|---|---|---|
| `Span` | `hir-lang` (span-lang 0.4) | problem spans; the type syntax-lang's spans and hir-lang's origins share |

## Feature flags

| Feature | Default | Effect |
|---|---|---|
| `std` | yes | Forwards to `hir-lang/std`, `syntax-lang/std`, and `intern-lang/std`. Without it the crate is `no_std` and needs only `alloc`; the API is identical. |

## Limits

| Limit | Value | When exceeded |
|---|---|---|
| HIR nodes per lowering | `Budget::max_nodes` (default 2^24) | `LowerError::NodeBudget` |
| Template application depth | `Budget::max_expansion_depth` (default 64) | `ExpansionTooDeep` problem |
| Template size | `MAX_TEMPLATE_SIZE` = 2^20 nodes, shared sub-trees counted per use | `TemplateError::TooLarge` |
| Exact decimal digits of a non-decimal big integer | 1,024 | `InvalidLiteral` problem |
| Use-tree nesting, interpolation pieces | none (iterative) | — |
| Tree depth | none (iterative) | — |

## Stability

Pre-1.0. The 0.x series may break the API; each break is listed in the
CHANGELOG. `Arch`, `Pick`, `TokenRole`, `Modifier`, `OpSpec`, `ProblemKind`,
`Context`, `HoistBlocker`, `Join`, the error enums, `Budget`, `Lowered`,
`LangPolicy`, and `Literals` are
`#[non_exhaustive]`, so new archetypes, picks, roles, and policies are
additive. The exact wording of problem and error messages is not part of the
contract; match on variants.
