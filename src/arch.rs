//! The archetype catalogue: the semantic forms a construct can be declared as.
//!
//! Each variant of [`Arch`] is one archetype from LexerSketch's semantic layer
//! (ARCHITECTURE §5): binders, scopes, declarations, control, expressions,
//! patterns, and types. Its fields are [`Pick`]s that say where each part of
//! the construct sits among the node's children. Flags that a construct
//! spells with keywords (`mut`, `async`, `static`, `pub`, `...`, `&`) are not
//! fields: they are read from the node's direct children through the
//! mapping's [modifier table](crate::MappingBuilder::modifier).

use hir_lang::CaptureMode;

use crate::mapping::Join;
use crate::pick::Pick;
use crate::template::TemplateId;

/// How a record or variant's field layout is decided.
///
/// # Examples
///
/// ```
/// use lower_lang::RecordShape;
///
/// assert_eq!(RecordShape::default(), RecordShape::Auto);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RecordShape {
    /// Named when any field has a name or there are no fields; tuple-like when
    /// every field is unnamed.
    #[default]
    Auto,
    /// `{ x: T }`.
    Named,
    /// `(T, U)`.
    Tuple,
    /// No fields at all (`struct S;`, a bare enum variant).
    Unit,
}

/// A use of a registered template as a node's archetype, made by
/// [`MappingBuilder::template_node`](crate::MappingBuilder::template_node).
///
/// The template's holes are bound, in declaration order, to picks stored in
/// the mapping.
///
/// # Examples
///
/// ```
/// use lower_lang::{templates, Arch, MappingBuilder, Pick};
///
/// let mut b = MappingBuilder::<u8>::new();
/// let while_loop = b.template(templates::while_loop())?;
/// b.template_node(4, while_loop, &[Pick::NthNode(0), Pick::NthNode(1), Pick::Absent]);
/// let mapping = b.build()?;
/// assert!(matches!(mapping.rule_for(&4), Some(Arch::Template(_))));
/// # Ok::<(), lower_lang::MappingError>(())
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TemplateUse {
    pub(crate) template: TemplateId,
    pub(crate) first: u32,
    pub(crate) len: u32,
}

impl TemplateUse {
    /// The template this node instantiates.
    ///
    /// # Examples
    ///
    /// ```
    /// use lower_lang::{templates, Arch, MappingBuilder, Pick};
    ///
    /// let mut b = MappingBuilder::<u8>::new();
    /// let t = b.template(templates::logical_and())?;
    /// b.template_node(1, t, &[Pick::Nth(0), Pick::Nth(2)]);
    /// let m = b.build()?;
    /// let Some(Arch::Template(used)) = m.rule_for(&1) else { unreachable!() };
    /// assert_eq!(used.template(), t);
    /// # Ok::<(), lower_lang::MappingError>(())
    /// ```
    #[must_use]
    pub const fn template(&self) -> TemplateId {
        self.template
    }
}

/// A semantic archetype: what a syntax node *means*, and where its parts are.
///
/// A [`Mapping`](crate::Mapping) maps node kinds to archetypes. Lowering a node
/// lowers each field in the context the archetype gives it (an `If`'s `cond`
/// as an expression, a `Fn`'s `params` as parameters) and builds the HIR form.
/// Fields marked *required* that a node lacks become error nodes plus a
/// [`MissingField`](crate::ProblemKind::MissingField) problem; the others are
/// simply left out. List fields take every match of their pick.
///
/// The modifiers each archetype reads from its direct children are listed
/// with it (see [`Modifier`](crate::Modifier)).
///
/// # Examples
///
/// ```
/// use lower_lang::{Arch, Pick};
///
/// // `if COND BLOCK (else (if_expr | BLOCK))?` with no field labels:
/// let if_expr: Arch<u16> = Arch::If {
///     cond: Pick::NthNode(0),
///     then: Pick::NthNode(1),
///     else_: Pick::NthNode(2),
/// };
/// assert!(matches!(if_expr, Arch::If { .. }));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Arch<K> {
    // ------------------------------------------------------------ structure
    /// A module: the root of a file, or a nested module. Declarations among
    /// `items` become the module's items; everything else (statements and
    /// expressions) becomes its top-level body, in order (scripts, REPL
    /// entries, conditional definitions).
    Module {
        /// The module's name (nested modules).
        name: Pick<K>,
        /// List: declarations and top-level statements.
        items: Pick<K>,
    },
    /// A wrapper with one meaningful child (`expr`, a parenthesized group, a
    /// statement wrapper): lowered as that child, in the same context.
    Pass {
        /// Required: the child.
        inner: Pick<K>,
    },
    /// Several statements, items, or class members from one node
    /// (`let a = 1, b = 2;`), spliced into the enclosing list.
    Splice {
        /// List: the parts, lowered in the enclosing list's context.
        items: Pick<K>,
    },
    /// A construct the language has but does not lower yet: an error node
    /// plus an [`Unsupported`](crate::ProblemKind::Unsupported) problem.
    Unsupported,

    // ------------------------------------------------- statements and binders
    /// `let PAT: TY = INIT else ELSE;` — binds the pattern's names for the
    /// following statements. Modifiers: `Mut` makes the binders mutable.
    Let {
        /// Required: the pattern (an identifier binds it).
        pat: Pick<K>,
        /// The annotation.
        ty: Pick<K>,
        /// The initializer.
        init: Pick<K>,
        /// The diverging `else` of let-else.
        else_: Pick<K>,
        /// Whether the binders are mutable without a `Mut` modifier (`var`,
        /// JavaScript `let`) rather than only with one (`let` / `let mut`).
        mutable: bool,
        /// Function-scoped hoisting (JavaScript `var`): the name is declared
        /// once at the top of the enclosing function (or module body) and this
        /// statement becomes an assignment. Any other pattern (`var [a, b] =
        /// x`) binds hygienic temporaries that are assigned to the hoisted
        /// names right after it matches; a by-reference binding cannot be
        /// hoisted this way and is reported
        /// ([`CannotHoist`](crate::ProblemKind::CannotHoist)).
        hoist: bool,
    },
    /// `EXPR;` — an expression evaluated for its effect (never a block tail).
    ExprStmt {
        /// Required: the expression.
        expr: Pick<K>,
    },
    /// `defer BODY` — runs when the enclosing block exits.
    Defer {
        /// Required: the deferred expression.
        body: Pick<K>,
    },
    /// `static $x = INIT;` — a function-static variable (PHP, C).
    Static {
        /// Required: the variable's name.
        name: Pick<K>,
        /// The annotation.
        ty: Pick<K>,
        /// The initializer.
        init: Pick<K>,
    },
    /// `global $a, $b;` — locals aliasing globals, one statement per name.
    GlobalDecl {
        /// List: the names.
        names: Pick<K>,
    },
    /// `use a::b as c;` / `use a::*;` — an import item. Modifiers: `Glob`.
    Import {
        /// Required: the imported path (a path node or one name).
        path: Pick<K>,
        /// The alias (`as c`).
        alias: Pick<K>,
    },
    /// A use tree: `use a::{b, c::{d, e as f}, g::*};` (Rust), `use App\{Foo,
    /// Bar as Baz};` (PHP group use). Flattened into one HIR import per leaf,
    /// each path the concatenation of the prefixes above it. Modifiers:
    /// visibility (on the outermost tree, for every import it makes), `Glob`
    /// (a `*` leaf).
    ///
    /// The expected tree: a use-tree node holds an optional prefix `path` (a
    /// path node or one name), then either nested `children` (use-tree nodes,
    /// names, or paths; container nodes such as `{ ... }` splice) or, for a
    /// leaf, an optional `alias`. A child spelled `self` (a token in the
    /// [root table](crate::MappingBuilder::root) mapping to
    /// `PathRoot::SelfModule`) imports the prefix itself. Leading root tokens
    /// of the outermost prefix (`::`, `crate`, `super`) set the import's root.
    /// The walk is iterative; nesting depth is bounded only by the tree.
    UseTree {
        /// The prefix (or, for a leaf, the whole path).
        path: Pick<K>,
        /// A leaf's alias (`as f`).
        alias: Pick<K>,
        /// List: the nested trees of a group.
        children: Pick<K>,
    },

    // ------------------------------------------------------------ declarations
    /// A function. Modifiers: visibility, `Async`, `Generator`, `Throws`,
    /// `Unsafe`. In a class, interface, or impl it is a method.
    Fn {
        /// Required: the name.
        name: Pick<K>,
        /// List: generic parameters.
        generics: Pick<K>,
        /// List: parameters.
        params: Pick<K>,
        /// The declared return type.
        ret: Pick<K>,
        /// The declared thrown type.
        throws: Pick<K>,
        /// The body (absent for required, abstract, or foreign functions).
        body: Pick<K>,
        /// List: where-clause predicates.
        where_: Pick<K>,
    },
    /// A record (struct) or union. Modifiers: visibility, `Union`.
    Record {
        /// Required: the name.
        name: Pick<K>,
        /// List: generic parameters.
        generics: Pick<K>,
        /// List: the fields.
        fields: Pick<K>,
        /// How the field layout is decided.
        shape: RecordShape,
    },
    /// A sum (enum) with variants. Modifiers: visibility.
    Sum {
        /// Required: the name.
        name: Pick<K>,
        /// List: generic parameters.
        generics: Pick<K>,
        /// List: the variants.
        variants: Pick<K>,
    },
    /// An interface or trait. Modifiers: visibility.
    Interface {
        /// Required: the name.
        name: Pick<K>,
        /// List: generic parameters.
        generics: Pick<K>,
        /// List: super-interfaces (types).
        supers: Pick<K>,
        /// List: members (methods, constants, associated types).
        items: Pick<K>,
    },
    /// A class. Modifiers: visibility, `Abstract`, `Final`.
    Class {
        /// Required: the name.
        name: Pick<K>,
        /// List: generic parameters.
        generics: Pick<K>,
        /// List: base classes, in method-resolution order (types).
        bases: Pick<K>,
        /// List: implemented interfaces (types).
        interfaces: Pick<K>,
        /// List: members: fields, methods, constants, static fields, aliases,
        /// mixin uses.
        members: Pick<K>,
        /// A mixin (PHP trait): never instantiated, used by classes.
        mixin: bool,
    },
    /// An implementation block (`impl Tr for T { ... }`).
    Impl {
        /// List: generic parameters.
        generics: Pick<K>,
        /// The implemented interface (a type).
        interface: Pick<K>,
        /// Required: the implementing type.
        self_ty: Pick<K>,
        /// List: members.
        items: Pick<K>,
    },
    /// A type alias. Modifiers: visibility.
    Alias {
        /// Required: the name.
        name: Pick<K>,
        /// List: generic parameters.
        generics: Pick<K>,
        /// Required: the aliased type.
        ty: Pick<K>,
    },
    /// An associated type declared by an interface.
    AssocType {
        /// Required: the name.
        name: Pick<K>,
        /// List: bounds (types).
        bounds: Pick<K>,
        /// A default type.
        default: Pick<K>,
    },
    /// A compile-time constant. Modifiers: visibility.
    Const {
        /// Required: the name.
        name: Pick<K>,
        /// The annotation.
        ty: Pick<K>,
        /// The value (absent only in an interface).
        value: Pick<K>,
    },
    /// A global or static variable item. Modifiers: visibility, `Mut`.
    GlobalVar {
        /// Required: the name.
        name: Pick<K>,
        /// The annotation.
        ty: Pick<K>,
        /// The initializer.
        init: Pick<K>,
        /// Whether it is assignable without a `Mut` modifier.
        mutable: bool,
    },
    /// A mixin use inside a class (PHP `use T, U { rules }`).
    MixinUse {
        /// List: the mixins (types).
        mixins: Pick<K>,
        /// List: conflict-resolution rules ([`MixinRule`](Arch::MixinRule)
        /// nodes).
        rules: Pick<K>,
    },
    /// One rule of a mixin use: `T::hello insteadof U, V;` or
    /// `T::hello as protected greet;` / `hello as private;`. With
    /// `insteadof` types it is an exclusion; otherwise an alias, with a new
    /// name, a visibility modifier (`Public`, `Protected`, `Private`), or both.
    /// A rule with neither is reported ([`MissingField`](crate::ProblemKind::MissingField)
    /// `action`).
    MixinRule {
        /// The mixin the method comes from (`T` in `T::hello`), a type.
        from: Pick<K>,
        /// Required: the method's name.
        method: Pick<K>,
        /// List: the mixins it is taken instead of (types).
        insteadof: Pick<K>,
        /// The new name of an alias.
        alias: Pick<K>,
    },
    /// A field of a record, variant, or class. Modifiers: visibility; in a
    /// class, `Static` makes it a static (global) member instead.
    FieldDecl {
        /// The name (absent for a positional field).
        name: Pick<K>,
        /// The type.
        ty: Pick<K>,
        /// A default value.
        default: Pick<K>,
    },
    /// A variant of a sum.
    VariantDecl {
        /// Required: the name.
        name: Pick<K>,
        /// List: the fields.
        fields: Pick<K>,
        /// An explicit discriminant.
        discriminant: Pick<K>,
        /// How the field layout is decided.
        shape: RecordShape,
    },
    /// A generic parameter: a type parameter, a const parameter (with a
    /// type), or a region (a lifetime-role token).
    GenericParam {
        /// Required: the name.
        name: Pick<K>,
        /// List: bounds (types).
        bounds: Pick<K>,
        /// The type of a const parameter.
        ty: Pick<K>,
        /// A default (a type, or a constant for a const parameter).
        default: Pick<K>,
    },
    /// A where-clause predicate: `T: A + B`.
    WherePred {
        /// Required: the constrained type.
        subject: Pick<K>,
        /// List: the bounds (types).
        bounds: Pick<K>,
    },
    /// A parameter. Modifiers: `Ref` (by reference, PHP `&$x`), `Mut`,
    /// `Rest` (variadic), `RestNamed`, `NamedOnly`, `PositionalOnly`,
    /// `Receiver`.
    Param {
        /// Required: the pattern (usually one name).
        pat: Pick<K>,
        /// The annotation.
        ty: Pick<K>,
        /// The default value.
        default: Pick<K>,
    },
    /// An explicit closure capture (PHP `use ($x, &$y)`). Modifiers: `Ref`,
    /// `Mut`.
    Capture {
        /// Required: the captured name.
        name: Pick<K>,
    },

    // ------------------------------------------------------------- expressions
    /// A path: `a::b::c`, `::a`, `self::a`, `super::a`, `Self::a`,
    /// `<T as Tr>::a`. Leading tokens of `segments` whose kind is in the
    /// mapping's [root table](crate::MappingBuilder::root) set the root.
    Path {
        /// Required (unless qualified): list of segments (names or `Segment`
        /// nodes).
        segments: Pick<K>,
        /// A leading global marker (`::a`).
        global: Pick<K>,
        /// The qualified self type (`<T as ...>`).
        qself: Pick<K>,
        /// List: the trait segments of a qualified self (`<T as a::Tr>`).
        qtrait: Pick<K>,
    },
    /// A path segment with generic arguments (`Vec::<i32>`).
    Segment {
        /// Required: the name.
        name: Pick<K>,
        /// List: generic arguments.
        args: Pick<K>,
    },
    /// An associated-type binding in generic arguments: `Item = u8`.
    AssocBinding {
        /// Required: the associated type's name.
        name: Pick<K>,
        /// Required: its type.
        ty: Pick<K>,
    },
    /// A prefix operator; the operator token's kind is looked up in the
    /// prefix [operator table](crate::MappingBuilder::prefix_op).
    Unary {
        /// Required: the operator token.
        op: Pick<K>,
        /// Required: the operand.
        operand: Pick<K>,
    },
    /// A binary operator; the token's kind is looked up in the binary
    /// operator table. `&&`, `||`, `??`, assignment, and compound assignment
    /// are operators too.
    Binary {
        /// Required: the left operand.
        lhs: Pick<K>,
        /// Required: the operator token.
        op: Pick<K>,
        /// Required: the right operand.
        rhs: Pick<K>,
    },
    /// A postfix operator; looked up in the postfix operator table.
    Postfix {
        /// Required: the operand.
        operand: Pick<K>,
        /// Required: the operator token.
        op: Pick<K>,
    },
    /// `TARGET = VALUE` or `TARGET op= VALUE`; a compound operator token is
    /// looked up in the binary operator table.
    Assign {
        /// Required: the place.
        target: Pick<K>,
        /// The operator token (absent: plain assignment).
        op: Pick<K>,
        /// Required: the value.
        value: Pick<K>,
    },
    /// `$a = &$b`: reference assignment (PHP).
    RefAssign {
        /// Required: the place that becomes an alias.
        target: Pick<K>,
        /// Required: the aliased place.
        source: Pick<K>,
    },
    /// A call.
    Call {
        /// Required: the callee.
        callee: Pick<K>,
        /// List: arguments.
        args: Pick<K>,
    },
    /// A call argument: named (`name: value`), spread (`...xs`, modifier
    /// `Spread`; `**kw`, modifier `SpreadNamed`), or a place (`&$x`, modifier
    /// `Ref`). Plain expressions in an argument list need no archetype.
    Arg {
        /// The argument's name.
        name: Pick<K>,
        /// Required: the value.
        value: Pick<K>,
    },
    /// Member access, or a method call when `call` finds something:
    /// `a.b`, `a.0`, `a.b(x)`.
    Member {
        /// Required: the accessed value.
        base: Pick<K>,
        /// Required: the member (a name, or a number for tuple fields).
        member: Pick<K>,
        /// List: method-call arguments.
        args: Pick<K>,
        /// The call marker (such as `(`); `Absent` means never a call.
        call: Pick<K>,
    },
    /// A method call with explicit generic arguments: `x.f::<T>(a)`.
    MethodCall {
        /// Required: the receiver.
        receiver: Pick<K>,
        /// Required: the method name.
        method: Pick<K>,
        /// List: generic arguments.
        generics: Pick<K>,
        /// List: arguments.
        args: Pick<K>,
    },
    /// `$o->$name` / `$o->$name(args)`: a member chosen at run time (Mox).
    DynMember {
        /// Required: the accessed value.
        base: Pick<K>,
        /// Required: the member name, an expression.
        name: Pick<K>,
        /// List: method-call arguments.
        args: Pick<K>,
        /// The call marker; `Absent` means never a call.
        call: Pick<K>,
    },
    /// `base[index]`.
    Index {
        /// Required: the indexed value.
        base: Pick<K>,
        /// Required: the index.
        index: Pick<K>,
    },
    /// `$$name`: the variable named at run time (Mox).
    VarVar {
        /// Required: the expression naming the variable.
        inner: Pick<K>,
    },
    /// `$a[]`: append position (Mox); only a write target.
    Append {
        /// Required: the array.
        base: Pick<K>,
    },
    /// `(a, b)`.
    Tuple {
        /// List: elements.
        elems: Pick<K>,
    },
    /// `[a, b]`.
    Array {
        /// List: elements.
        elems: Pick<K>,
    },
    /// An ordered map literal: `[k => v, w]` (PHP arrays). Elements that are
    /// not `MapEntry` nodes take the next index.
    Map {
        /// List: entries.
        entries: Pick<K>,
    },
    /// `key => value` inside a map literal.
    MapEntry {
        /// The key (absent: next index).
        key: Pick<K>,
        /// Required: the value.
        value: Pick<K>,
    },
    /// `Path { x: 1, ..base }` or an anonymous record.
    RecordLit {
        /// The record type's path.
        path: Pick<K>,
        /// List: field initializers.
        fields: Pick<K>,
        /// The `..base` expression.
        base: Pick<K>,
    },
    /// `name: value` (or the shorthand `name`) in a record literal.
    FieldInit {
        /// Required: the field name.
        name: Pick<K>,
        /// The value (absent: shorthand, the variable of the same name).
        value: Pick<K>,
    },
    /// `[elem; count]`.
    Repeat {
        /// Required: the element.
        elem: Pick<K>,
        /// Required: the count.
        count: Pick<K>,
    },
    /// `expr as Type`.
    Cast {
        /// Required: the converted value.
        expr: Pick<K>,
        /// Required: the target type.
        ty: Pick<K>,
    },
    /// `&expr` / `&mut expr`. Modifiers: `Mut`.
    Borrow {
        /// Required: the borrowed place.
        expr: Pick<K>,
    },
    /// `*expr`.
    Deref {
        /// Required: the dereferenced value.
        inner: Pick<K>,
    },
    /// A block. Modifiers: `Unsafe`.
    Block {
        /// List: statements (and local items).
        stmts: Pick<K>,
        /// The tail expression (the block's value).
        tail: Pick<K>,
        /// A label (`'a: { ... }`).
        label: Pick<K>,
        /// Whether a final bare expression (not an expression statement)
        /// becomes the tail.
        tail_from_last: bool,
    },
    /// `if cond then else`.
    If {
        /// Required: the condition.
        cond: Pick<K>,
        /// Required: the value when true.
        then: Pick<K>,
        /// The value when false.
        else_: Pick<K>,
    },
    /// `match scrutinee { arms }`.
    Match {
        /// Required: the matched value.
        scrutinee: Pick<K>,
        /// List: arms.
        arms: Pick<K>,
    },
    /// A match arm: `pat if guard => body`.
    Arm {
        /// Required: the pattern.
        pat: Pick<K>,
        /// The guard.
        guard: Pick<K>,
        /// Required: the body.
        body: Pick<K>,
    },
    /// `loop body` — HIR's one loop. `while`, `do-while`, C `for`, and
    /// `for-each` are [templates](crate::templates).
    Loop {
        /// A label.
        label: Pick<K>,
        /// Required: the body.
        body: Pick<K>,
    },
    /// `break 'label value`.
    Break {
        /// The target label.
        label: Pick<K>,
        /// The value.
        value: Pick<K>,
    },
    /// `continue 'label`.
    Continue {
        /// The target label.
        label: Pick<K>,
    },
    /// `return value`.
    Return {
        /// The value.
        value: Pick<K>,
    },
    /// `throw value`.
    Throw {
        /// Required: the raised value.
        value: Pick<K>,
    },
    /// `try body catch... finally...`.
    Try {
        /// Required: the protected body.
        body: Pick<K>,
        /// List: catch clauses (`Catch` or `Arm` nodes).
        catches: Pick<K>,
        /// The `finally` body.
        finally: Pick<K>,
    },
    /// `catch (A | B $e) body`: a type test (or an alternative of several)
    /// binding `name`. Without types it catches everything.
    Catch {
        /// List: the caught types.
        types: Pick<K>,
        /// The bound name.
        name: Pick<K>,
        /// Required: the handler.
        body: Pick<K>,
    },
    /// `await e`.
    Await {
        /// Required: the awaited value.
        inner: Pick<K>,
    },
    /// `yield v`, or `yield k => v` (PHP and Mox generators). A key without
    /// a value yields `null` under that key.
    Yield {
        /// The explicit key.
        key: Pick<K>,
        /// The yielded value.
        value: Pick<K>,
    },
    /// `yield from g`: delegate to an inner generator or iterable; evaluates
    /// to its return value.
    YieldFrom {
        /// Required: the inner generator or iterable.
        inner: Pick<K>,
    },
    /// `spawn e` — starts a task running the callable `e`.
    Spawn {
        /// Required: the callable.
        inner: Pick<K>,
    },
    /// A closure. Modifiers: `Async`, `Generator`, `Throws`.
    Closure {
        /// List: parameters.
        params: Pick<K>,
        /// The return type.
        ret: Pick<K>,
        /// List: explicit captures (`Capture` nodes or names).
        captures: Pick<K>,
        /// Required: the body.
        body: Pick<K>,
        /// How outer variables are captured implicitly; `None` means only the
        /// explicit captures are visible (PHP `function () use (...)`).
        implicit: Option<CaptureMode>,
    },
    /// A node lowered through a template; see
    /// [`MappingBuilder::template_node`](crate::MappingBuilder::template_node).
    Template(TemplateUse),
    /// An interpolated string: `"Hello {$name}!"`, `"$name"`, `"{$a['k']}"`,
    /// and heredocs, as the lexer delivers them in segments.
    ///
    /// The expected tree: the string's node holds its pieces in order. Text
    /// pieces are tokens with role [`TokenRole::StrPart`](crate::TokenRole::StrPart)
    /// (no quotes; escapes decoded, adjacent pieces merged); every other
    /// selected element is an embedded expression (`$name` as a variable
    /// token, `{$a['k']}` as an index node; braces and quotes are skip kinds
    /// or left out by the `parts` pick). The parts are joined by `join`:
    /// one text piece alone is a string literal, no pieces the empty string,
    /// and a lone expression is joined with an empty string first, so the
    /// result is always a string conversion.
    ///
    /// With `close` (a heredoc), `close` picks the closing-marker token: the
    /// whitespace before its identifier is removed from the start of every
    /// line of the text pieces (PHP 7.3+), a line indented less (or starting
    /// with an embedded expression) is reported
    /// ([`InvalidLiteral`](crate::ProblemKind::InvalidLiteral)), and the
    /// newline before the marker is dropped.
    Interp {
        /// List: the pieces, in order.
        parts: Pick<K>,
        /// A heredoc's closing-marker token.
        close: Pick<K>,
        /// How the pieces become one string.
        join: Join,
    },

    // -------------------------------------------------------------- attributes
    /// An attribute (`#[Route("/home", methods: GET)]`, `@inline`), attached
    /// to the node it is a direct child of. Attribute nodes (and container
    /// nodes holding them, such as PHP's `#[A, B]` group) are invisible to
    /// every other pick of that node. HIR attributes are data, never code
    /// (HIR spec §11): an argument value must be a literal or a name; anything
    /// else is reported
    /// ([`AttributeArgument`](crate::ProblemKind::AttributeArgument)) and left
    /// out.
    Attr {
        /// Required: the attribute's name, kept as written (a qualified name
        /// such as `\App\Route` keeps its separators).
        name: Pick<K>,
        /// List: the arguments ([`AttrArg`](Arch::AttrArg) nodes, or values).
        args: Pick<K>,
    },
    /// `key: value` / `key = value` inside an attribute's arguments.
    AttrArg {
        /// The key.
        key: Pick<K>,
        /// The value (absent: a bare key).
        value: Pick<K>,
    },

    // ---------------------------------------------------------------- patterns
    /// `_` as a node.
    WildPat,
    /// `..` (or `rest @ ..`) inside a tuple, constructor, slice, or record
    /// pattern.
    RestPat {
        /// A pattern binding the rest (slices only).
        bind: Pick<K>,
    },
    /// `ref mut x @ sub`. Modifiers: `Ref`, `Mut`.
    BindPat {
        /// Required: the bound name.
        name: Pick<K>,
        /// A sub-pattern the value must also match.
        sub: Pick<K>,
    },
    /// `(a, b, ..)`.
    TuplePat {
        /// List: element patterns (a `RestPat` marks `..`).
        elems: Pick<K>,
    },
    /// `Some(x)`.
    CtorPat {
        /// Required: the constructor's path.
        path: Pick<K>,
        /// List: element patterns.
        elems: Pick<K>,
    },
    /// `Point { x, y: 0, .. }`.
    RecordPat {
        /// The record's path (absent: an anonymous record or object).
        path: Pick<K>,
        /// List: field patterns.
        fields: Pick<K>,
    },
    /// `name: pat` (or the shorthand `name`) in a record pattern.
    FieldPat {
        /// Required: the field name.
        name: Pick<K>,
        /// The pattern (absent: shorthand, binding the field's name).
        pat: Pick<K>,
    },
    /// `a | b`.
    OrPat {
        /// List: alternatives.
        alts: Pick<K>,
    },
    /// `lo..hi` / `lo..=hi`. Modifiers: `Inclusive`.
    RangePat {
        /// The lower bound.
        lo: Pick<K>,
        /// The upper bound.
        hi: Pick<K>,
    },
    /// `&p` / `&mut p`. Modifiers: `Mut`.
    RefPat {
        /// Required: the inner pattern.
        inner: Pick<K>,
    },
    /// `[a, b, rest @ .., z]`.
    SlicePat {
        /// List: element patterns (a `RestPat` marks `..`).
        elems: Pick<K>,
    },
    /// `e: Type` / `Type e` — a type test, as in `catch` clauses.
    TypeTestPat {
        /// Required: the tested type.
        ty: Pick<K>,
        /// The pattern the value must then match.
        pat: Pick<K>,
    },

    // ------------------------------------------------------------------- types
    /// `(A, B)`.
    TupleTy {
        /// List: element types.
        elems: Pick<K>,
    },
    /// `[T; N]`.
    ArrayTy {
        /// Required: the element type.
        elem: Pick<K>,
        /// Required: the length.
        len: Pick<K>,
    },
    /// `[T]`.
    SliceTy {
        /// Required: the element type.
        elem: Pick<K>,
    },
    /// `&'r mut T`. Modifiers: `Mut`.
    RefTy {
        /// Required: the referent type.
        inner: Pick<K>,
        /// An explicit region.
        region: Pick<K>,
    },
    /// `*mut T` / `*const T`. Modifiers: `Mut`.
    PtrTy {
        /// Required: the pointee type.
        inner: Pick<K>,
    },
    /// `T?`.
    NullableTy {
        /// Required: the inner type.
        inner: Pick<K>,
    },
    /// `fn(A, B) -> R`.
    FnTy {
        /// List: parameter types.
        params: Pick<K>,
        /// The return type (absent: unit).
        ret: Pick<K>,
    },
    /// `dyn A + B`.
    ObjectTy {
        /// List: bounds.
        bounds: Pick<K>,
    },
    /// `impl A + B`.
    ImplTy {
        /// List: bounds.
        bounds: Pick<K>,
    },
    /// `A | B | C` (PHP union types). Members are lowered in source order;
    /// hir-lang's `finish` puts them in canonical form (flattened, sorted,
    /// repeats dropped, `null` hoisted into `Nullable`, one member
    /// collapsed). No members at all is a
    /// [`MissingField`](crate::ProblemKind::MissingField) problem.
    UnionTy {
        /// List: the member types.
        members: Pick<K>,
    },
    /// `A & B` (PHP intersection types), normalized like
    /// [`UnionTy`](Self::UnionTy).
    IntersectionTy {
        /// List: the member types.
        members: Pick<K>,
    },
}

impl<K> Arch<K> {
    /// `Pass { inner: Nth(n) }`: the common wrapper.
    ///
    /// # Examples
    ///
    /// ```
    /// use lower_lang::{Arch, Pick};
    ///
    /// assert_eq!(Arch::<u8>::pass(1), Arch::Pass { inner: Pick::Nth(1) });
    /// ```
    #[must_use]
    pub const fn pass(n: u16) -> Self {
        Self::Pass {
            inner: Pick::Nth(n),
        }
    }

    /// The shape of lang-forge's operator nodes: `binary` is `lhs op rhs`.
    ///
    /// # Examples
    ///
    /// ```
    /// use lower_lang::{Arch, Pick};
    ///
    /// let Arch::Binary { op, .. } = Arch::<u8>::binary() else { unreachable!() };
    /// assert_eq!(op, Pick::Nth(1));
    /// ```
    #[must_use]
    pub const fn binary() -> Self {
        Self::Binary {
            lhs: Pick::Nth(0),
            op: Pick::Nth(1),
            rhs: Pick::Nth(2),
        }
    }

    /// The shape of lang-forge's `prefix` nodes: `op operand`.
    ///
    /// # Examples
    ///
    /// ```
    /// use lower_lang::{Arch, Pick};
    ///
    /// let Arch::Unary { operand, .. } = Arch::<u8>::prefix() else { unreachable!() };
    /// assert_eq!(operand, Pick::Nth(1));
    /// ```
    #[must_use]
    pub const fn prefix() -> Self {
        Self::Unary {
            op: Pick::Nth(0),
            operand: Pick::Nth(1),
        }
    }

    /// The archetype's name, as problems and docs spell it.
    ///
    /// # Examples
    ///
    /// ```
    /// use lower_lang::Arch;
    ///
    /// assert_eq!(Arch::<u8>::binary().name(), "binary");
    /// assert_eq!(Arch::<u8>::WildPat.name(), "wildcard pattern");
    /// ```
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Module { .. } => "module",
            Self::Pass { .. } => "pass",
            Self::Splice { .. } => "splice",
            Self::Unsupported => "unsupported",
            Self::Let { .. } => "let",
            Self::ExprStmt { .. } => "expression statement",
            Self::Defer { .. } => "defer",
            Self::Static { .. } => "static",
            Self::GlobalDecl { .. } => "global declaration",
            Self::Import { .. } => "import",
            Self::Fn { .. } => "function",
            Self::Record { .. } => "record",
            Self::Sum { .. } => "sum",
            Self::Interface { .. } => "interface",
            Self::Class { .. } => "class",
            Self::Impl { .. } => "impl",
            Self::Alias { .. } => "alias",
            Self::AssocType { .. } => "associated type",
            Self::Const { .. } => "constant",
            Self::GlobalVar { .. } => "global variable",
            Self::MixinUse { .. } => "mixin use",
            Self::MixinRule { .. } => "mixin rule",
            Self::UseTree { .. } => "use tree",
            Self::Interp { .. } => "interpolated string",
            Self::Attr { .. } => "attribute",
            Self::AttrArg { .. } => "attribute argument",
            Self::FieldDecl { .. } => "field",
            Self::VariantDecl { .. } => "variant",
            Self::GenericParam { .. } => "generic parameter",
            Self::WherePred { .. } => "where predicate",
            Self::Param { .. } => "parameter",
            Self::Capture { .. } => "capture",
            Self::Path { .. } => "path",
            Self::Segment { .. } => "path segment",
            Self::AssocBinding { .. } => "associated-type binding",
            Self::Unary { .. } => "unary",
            Self::Binary { .. } => "binary",
            Self::Postfix { .. } => "postfix",
            Self::Assign { .. } => "assignment",
            Self::RefAssign { .. } => "reference assignment",
            Self::Call { .. } => "call",
            Self::Arg { .. } => "argument",
            Self::Member { .. } => "member",
            Self::MethodCall { .. } => "method call",
            Self::DynMember { .. } => "dynamic member",
            Self::Index { .. } => "index",
            Self::VarVar { .. } => "variable variable",
            Self::Append { .. } => "append",
            Self::Tuple { .. } => "tuple",
            Self::Array { .. } => "array",
            Self::Map { .. } => "map",
            Self::MapEntry { .. } => "map entry",
            Self::RecordLit { .. } => "record literal",
            Self::FieldInit { .. } => "field initializer",
            Self::Repeat { .. } => "repeat",
            Self::Cast { .. } => "cast",
            Self::Borrow { .. } => "borrow",
            Self::Deref { .. } => "dereference",
            Self::Block { .. } => "block",
            Self::If { .. } => "if",
            Self::Match { .. } => "match",
            Self::Arm { .. } => "match arm",
            Self::Loop { .. } => "loop",
            Self::Break { .. } => "break",
            Self::Continue { .. } => "continue",
            Self::Return { .. } => "return",
            Self::Throw { .. } => "throw",
            Self::Try { .. } => "try",
            Self::Catch { .. } => "catch",
            Self::Await { .. } => "await",
            Self::Yield { .. } => "yield",
            Self::YieldFrom { .. } => "yield from",
            Self::Spawn { .. } => "spawn",
            Self::Closure { .. } => "closure",
            Self::Template(_) => "template",
            Self::WildPat => "wildcard pattern",
            Self::RestPat { .. } => "rest pattern",
            Self::BindPat { .. } => "binding pattern",
            Self::TuplePat { .. } => "tuple pattern",
            Self::CtorPat { .. } => "constructor pattern",
            Self::RecordPat { .. } => "record pattern",
            Self::FieldPat { .. } => "field pattern",
            Self::OrPat { .. } => "or-pattern",
            Self::RangePat { .. } => "range pattern",
            Self::RefPat { .. } => "reference pattern",
            Self::SlicePat { .. } => "slice pattern",
            Self::TypeTestPat { .. } => "type-test pattern",
            Self::TupleTy { .. } => "tuple type",
            Self::ArrayTy { .. } => "array type",
            Self::SliceTy { .. } => "slice type",
            Self::RefTy { .. } => "reference type",
            Self::PtrTy { .. } => "pointer type",
            Self::NullableTy { .. } => "nullable type",
            Self::FnTy { .. } => "function type",
            Self::ObjectTy { .. } => "object type",
            Self::ImplTy { .. } => "impl type",
            Self::UnionTy { .. } => "union type",
            Self::IntersectionTy { .. } => "intersection type",
        }
    }

    /// The archetype's LSF2 name: the value of `arch = "…"` in
    /// `[semantics.bind]` (LSF2 §15.4), and the key
    /// [`MappingBuilder::modifier_in`](crate::MappingBuilder::modifier_in)
    /// takes. Archetypes LSF2 has no `arch` value for use the snake case of
    /// the variant (`mixin_rule`, `use_tree`, `attr_arg`, `template`).
    ///
    /// # Examples
    ///
    /// ```
    /// use lower_lang::{Arch, Pick};
    ///
    /// let param: Arch<u8> = Arch::Param { pat: Pick::Nth(0), ty: Pick::Absent, default: Pick::Absent };
    /// assert_eq!(param.lsf_name(), "param");
    /// assert_eq!(Arch::<u8>::binary().lsf_name(), "binary");
    /// ```
    #[must_use]
    pub const fn lsf_name(&self) -> &'static str {
        match self {
            Self::Module { .. } => "module",
            Self::Pass { .. } => "pass",
            Self::Splice { .. } => "splice",
            Self::Unsupported => "unsupported",
            Self::Let { .. } => "let",
            Self::ExprStmt { .. } => "expr_stmt",
            Self::Defer { .. } => "defer",
            Self::Static { .. } => "static",
            Self::GlobalDecl { .. } => "global_decl",
            Self::Import { .. } => "import",
            Self::Fn { .. } => "fn",
            Self::Record { .. } => "record",
            Self::Sum { .. } => "sum",
            Self::Interface { .. } => "interface",
            Self::Class { .. } => "class",
            Self::Impl { .. } => "impl",
            Self::Alias { .. } => "alias",
            Self::AssocType { .. } => "assoc_type",
            Self::Const { .. } => "const",
            Self::GlobalVar { .. } => "global_var",
            Self::MixinUse { .. } => "mixin_use",
            Self::MixinRule { .. } => "mixin_rule",
            Self::UseTree { .. } => "use_tree",
            Self::Interp { .. } => "interpolate",
            Self::Attr { .. } => "attribute",
            Self::AttrArg { .. } => "attr_arg",
            Self::FieldDecl { .. } => "field_decl",
            Self::VariantDecl { .. } => "variant_decl",
            Self::GenericParam { .. } => "generic_param",
            Self::WherePred { .. } => "where_pred",
            Self::Param { .. } => "param",
            Self::Capture { .. } => "capture",
            Self::Path { .. } => "path",
            Self::Segment { .. } => "segment",
            Self::AssocBinding { .. } => "assoc_binding",
            Self::Unary { .. } => "unary",
            Self::Binary { .. } => "binary",
            Self::Postfix { .. } => "postfix",
            Self::Assign { .. } => "assign",
            Self::RefAssign { .. } => "ref_assign",
            Self::Call { .. } => "call",
            Self::Arg { .. } => "arg",
            Self::Member { .. } => "member",
            Self::MethodCall { .. } => "method_call",
            Self::DynMember { .. } => "dyn_member",
            Self::Index { .. } => "index",
            Self::VarVar { .. } => "var_var",
            Self::Append { .. } => "append",
            Self::Tuple { .. } => "tuple",
            Self::Array { .. } => "array",
            Self::Map { .. } => "map",
            Self::MapEntry { .. } => "map_entry",
            Self::RecordLit { .. } => "record_lit",
            Self::FieldInit { .. } => "field_init",
            Self::Repeat { .. } => "repeat",
            Self::Cast { .. } => "cast",
            Self::Borrow { .. } => "borrow",
            Self::Deref { .. } => "deref",
            Self::Block { .. } => "block",
            Self::If { .. } => "if",
            Self::Match { .. } => "match",
            Self::Arm { .. } => "arm",
            Self::Loop { .. } => "loop",
            Self::Break { .. } => "break",
            Self::Continue { .. } => "continue",
            Self::Return { .. } => "return",
            Self::Throw { .. } => "throw",
            Self::Try { .. } => "try",
            Self::Catch { .. } => "catch",
            Self::Await { .. } => "await",
            Self::Yield { .. } => "yield",
            Self::YieldFrom { .. } => "yield_from",
            Self::Spawn { .. } => "spawn",
            Self::Closure { .. } => "closure",
            Self::Template(_) => "template",
            Self::WildPat => "wild_pat",
            Self::RestPat { .. } => "rest_pat",
            Self::BindPat { .. } => "bind_pat",
            Self::TuplePat { .. } => "tuple_pat",
            Self::CtorPat { .. } => "ctor_pat",
            Self::RecordPat { .. } => "record_pat",
            Self::FieldPat { .. } => "field_pat",
            Self::OrPat { .. } => "or_pat",
            Self::RangePat { .. } => "range_pat",
            Self::RefPat { .. } => "ref_pat",
            Self::SlicePat { .. } => "slice_pat",
            Self::TypeTestPat { .. } => "type_test_pat",
            Self::TupleTy { .. } => "tuple_ty",
            Self::ArrayTy { .. } => "array_ty",
            Self::SliceTy { .. } => "slice_ty",
            Self::RefTy { .. } => "ref_ty",
            Self::PtrTy { .. } => "ptr_ty",
            Self::NullableTy { .. } => "nullable_ty",
            Self::FnTy { .. } => "fn_ty",
            Self::ObjectTy { .. } => "object_ty",
            Self::ImplTy { .. } => "impl_ty",
            Self::UnionTy { .. } => "union_ty",
            Self::IntersectionTy { .. } => "intersection_ty",
        }
    }
}

/// Every [`Arch::lsf_name`], for checking the archetype names a mapping is
/// given ([`modifier_in`](crate::MappingBuilder::modifier_in)). In variant
/// order; the unit test below checks it against every variant.
pub(crate) const LSF_NAMES: [&str; 98] = [
    "module",
    "pass",
    "splice",
    "unsupported",
    "let",
    "expr_stmt",
    "defer",
    "static",
    "global_decl",
    "import",
    "fn",
    "record",
    "sum",
    "interface",
    "class",
    "impl",
    "alias",
    "assoc_type",
    "const",
    "global_var",
    "mixin_use",
    "mixin_rule",
    "use_tree",
    "interpolate",
    "attribute",
    "attr_arg",
    "field_decl",
    "variant_decl",
    "generic_param",
    "where_pred",
    "param",
    "capture",
    "path",
    "segment",
    "assoc_binding",
    "unary",
    "binary",
    "postfix",
    "assign",
    "ref_assign",
    "call",
    "arg",
    "member",
    "method_call",
    "dyn_member",
    "index",
    "var_var",
    "append",
    "tuple",
    "array",
    "map",
    "map_entry",
    "record_lit",
    "field_init",
    "repeat",
    "cast",
    "borrow",
    "deref",
    "block",
    "if",
    "match",
    "arm",
    "loop",
    "break",
    "continue",
    "return",
    "throw",
    "try",
    "catch",
    "await",
    "yield",
    "yield_from",
    "spawn",
    "closure",
    "template",
    "wild_pat",
    "rest_pat",
    "bind_pat",
    "tuple_pat",
    "ctor_pat",
    "record_pat",
    "field_pat",
    "or_pat",
    "range_pat",
    "ref_pat",
    "slice_pat",
    "type_test_pat",
    "tuple_ty",
    "array_ty",
    "slice_ty",
    "ref_ty",
    "ptr_ty",
    "nullable_ty",
    "fn_ty",
    "object_ty",
    "impl_ty",
    "union_ty",
    "intersection_ty",
];

/// The canonical (`'static`) spelling of an LSF2 archetype name, or `None`
/// when no archetype has that name.
pub(crate) fn lsf_name_of(name: &str) -> Option<&'static str> {
    LSF_NAMES.iter().copied().find(|n| *n == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapping::Join;
    use crate::mapping::NameRef;
    use crate::template::TemplateId;

    /// One value of every variant, in declaration order.
    fn every_variant() -> alloc::vec::Vec<Arch<u8>> {
        let a = Pick::Absent;
        alloc::vec![
            Arch::Module { name: a, items: a },
            Arch::Pass { inner: a },
            Arch::Splice { items: a },
            Arch::Unsupported,
            Arch::Let {
                pat: a,
                ty: a,
                init: a,
                else_: a,
                mutable: false,
                hoist: false
            },
            Arch::ExprStmt { expr: a },
            Arch::Defer { body: a },
            Arch::Static {
                name: a,
                ty: a,
                init: a
            },
            Arch::GlobalDecl { names: a },
            Arch::Import { path: a, alias: a },
            Arch::Fn {
                name: a,
                generics: a,
                params: a,
                ret: a,
                throws: a,
                body: a,
                where_: a
            },
            Arch::Record {
                name: a,
                generics: a,
                fields: a,
                shape: RecordShape::Auto
            },
            Arch::Sum {
                name: a,
                generics: a,
                variants: a
            },
            Arch::Interface {
                name: a,
                generics: a,
                supers: a,
                items: a
            },
            Arch::Class {
                name: a,
                generics: a,
                bases: a,
                interfaces: a,
                members: a,
                mixin: false
            },
            Arch::Impl {
                generics: a,
                interface: a,
                self_ty: a,
                items: a
            },
            Arch::Alias {
                name: a,
                generics: a,
                ty: a
            },
            Arch::AssocType {
                name: a,
                bounds: a,
                default: a
            },
            Arch::Const {
                name: a,
                ty: a,
                value: a
            },
            Arch::GlobalVar {
                name: a,
                ty: a,
                init: a,
                mutable: false
            },
            Arch::MixinUse {
                mixins: a,
                rules: a
            },
            Arch::MixinRule {
                from: a,
                method: a,
                insteadof: a,
                alias: a
            },
            Arch::UseTree {
                path: a,
                alias: a,
                children: a
            },
            Arch::Interp {
                parts: a,
                close: a,
                join: Join::Call(NameRef(0))
            },
            Arch::Attr { name: a, args: a },
            Arch::AttrArg { key: a, value: a },
            Arch::FieldDecl {
                name: a,
                ty: a,
                default: a
            },
            Arch::VariantDecl {
                name: a,
                fields: a,
                discriminant: a,
                shape: RecordShape::Auto
            },
            Arch::GenericParam {
                name: a,
                bounds: a,
                ty: a,
                default: a
            },
            Arch::WherePred {
                subject: a,
                bounds: a
            },
            Arch::Param {
                pat: a,
                ty: a,
                default: a
            },
            Arch::Capture { name: a },
            Arch::Path {
                segments: a,
                global: a,
                qself: a,
                qtrait: a
            },
            Arch::Segment { name: a, args: a },
            Arch::AssocBinding { name: a, ty: a },
            Arch::Unary { op: a, operand: a },
            Arch::Binary {
                lhs: a,
                op: a,
                rhs: a
            },
            Arch::Postfix { operand: a, op: a },
            Arch::Assign {
                target: a,
                op: a,
                value: a
            },
            Arch::RefAssign {
                target: a,
                source: a
            },
            Arch::Call { callee: a, args: a },
            Arch::Arg { name: a, value: a },
            Arch::Member {
                base: a,
                member: a,
                args: a,
                call: a
            },
            Arch::MethodCall {
                receiver: a,
                method: a,
                generics: a,
                args: a
            },
            Arch::DynMember {
                base: a,
                name: a,
                args: a,
                call: a
            },
            Arch::Index { base: a, index: a },
            Arch::VarVar { inner: a },
            Arch::Append { base: a },
            Arch::Tuple { elems: a },
            Arch::Array { elems: a },
            Arch::Map { entries: a },
            Arch::MapEntry { key: a, value: a },
            Arch::RecordLit {
                path: a,
                fields: a,
                base: a
            },
            Arch::FieldInit { name: a, value: a },
            Arch::Repeat { elem: a, count: a },
            Arch::Cast { expr: a, ty: a },
            Arch::Borrow { expr: a },
            Arch::Deref { inner: a },
            Arch::Block {
                stmts: a,
                tail: a,
                label: a,
                tail_from_last: false
            },
            Arch::If {
                cond: a,
                then: a,
                else_: a
            },
            Arch::Match {
                scrutinee: a,
                arms: a
            },
            Arch::Arm {
                pat: a,
                guard: a,
                body: a
            },
            Arch::Loop { label: a, body: a },
            Arch::Break { label: a, value: a },
            Arch::Continue { label: a },
            Arch::Return { value: a },
            Arch::Throw { value: a },
            Arch::Try {
                body: a,
                catches: a,
                finally: a
            },
            Arch::Catch {
                types: a,
                name: a,
                body: a
            },
            Arch::Await { inner: a },
            Arch::Yield { key: a, value: a },
            Arch::YieldFrom { inner: a },
            Arch::Spawn { inner: a },
            Arch::Closure {
                params: a,
                ret: a,
                captures: a,
                body: a,
                implicit: None
            },
            Arch::Template(TemplateUse {
                template: TemplateId(0),
                first: 0,
                len: 0
            }),
            Arch::WildPat,
            Arch::RestPat { bind: a },
            Arch::BindPat { name: a, sub: a },
            Arch::TuplePat { elems: a },
            Arch::CtorPat { path: a, elems: a },
            Arch::RecordPat { path: a, fields: a },
            Arch::FieldPat { name: a, pat: a },
            Arch::OrPat { alts: a },
            Arch::RangePat { lo: a, hi: a },
            Arch::RefPat { inner: a },
            Arch::SlicePat { elems: a },
            Arch::TypeTestPat { ty: a, pat: a },
            Arch::TupleTy { elems: a },
            Arch::ArrayTy { elem: a, len: a },
            Arch::SliceTy { elem: a },
            Arch::RefTy {
                inner: a,
                region: a
            },
            Arch::PtrTy { inner: a },
            Arch::NullableTy { inner: a },
            Arch::FnTy { params: a, ret: a },
            Arch::ObjectTy { bounds: a },
            Arch::ImplTy { bounds: a },
            Arch::UnionTy { members: a },
            Arch::IntersectionTy { members: a },
        ]
    }

    #[test]
    fn test_lsf_names_cover_every_variant_once() {
        let all = every_variant();
        assert_eq!(all.len(), LSF_NAMES.len());
        for (arch, name) in all.iter().zip(LSF_NAMES) {
            assert_eq!(arch.lsf_name(), name);
            assert_eq!(lsf_name_of(name), Some(name));
        }
        let mut sorted = LSF_NAMES;
        sorted.sort_unstable();
        assert!(sorted.windows(2).all(|w| w[0] != w[1]), "names are unique");
        assert_eq!(lsf_name_of("parameter"), None);
        assert_eq!(lsf_name_of(""), None);
    }
}
