//! Tree access: how an archetype's fields are found among a node's children.
//!
//! A syntax tree from lang-forge 1.x has no field labels, so a field is found
//! by position or by kind ([`Pick::Nth`], [`Pick::Kind`], [`Pick::After`], ...).
//! Trees from LSF2 sketches will carry `name:expr` labels; a host that has them
//! supplies a [`Labeler`] and addresses fields with [`Pick::Label`]. Either way
//! the mapping is data: a `Pick` is a small `Copy` value, and the common path
//! needs no closures.

use alloc::vec::Vec;

use syntax_lang::{Element, Node, TokenKind};

/// Where a field of an archetype is found among a node's children.
///
/// Positions count the node's **significant** children: every child node and
/// every token that is not trivia (whitespace and comments never count). List
/// picks ([`All`](Pick::All), [`Nodes`](Pick::Nodes), [`AllKind`](Pick::AllKind),
/// [`Range`](Pick::Range), [`AllAfter`](Pick::AllAfter),
/// [`Between`](Pick::Between), and [`Label`](Pick::Label)) select every match,
/// splice the children of *container* kinds in place, and (except `AllKind`,
/// `Nodes`, and `Label`) leave out *skip* kinds such as `,` and `;` (both
/// declared in the [`Mapping`](crate::Mapping)). Used where one child is
/// expected, a list pick takes its first match.
///
/// A pick that finds nothing makes the field absent: optional fields are then
/// left out, and required ones become an error node plus a
/// [`MissingField`](crate::ProblemKind::MissingField) problem.
///
/// # Examples
///
/// ```
/// use lower_lang::Pick;
///
/// // `let NAME = VALUE ;` in a tree without labels: the name is the first
/// // identifier, the value is whatever follows `=`.
/// let ident = 7u16; // the language's IDENT kind
/// let eq = 9u16;    // its `=` kind
/// let name = Pick::Kind(ident);
/// let value = Pick::After(eq);
/// assert_ne!(name, value);
/// assert_eq!(Pick::<u16>::default(), Pick::Absent);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Pick<K> {
    /// The construct never has this field.
    Absent,
    /// The child (or, as a list, every child) whose host-provided field label
    /// is this id. Needs a [`Labeler`]; without one it finds nothing.
    Label(u16),
    /// The `n`-th significant child (0-based).
    Nth(u16),
    /// The `n`-th significant child from the end (`Back(0)` is the last).
    Back(u16),
    /// The `n`-th child node (tokens not counted).
    NthNode(u16),
    /// The first significant child of this kind.
    Kind(K),
    /// The `n`-th significant child of this kind.
    KindNth(K, u16),
    /// The first significant child, if it is of this kind (a leading `::`, a
    /// leading keyword).
    First(K),
    /// The first significant child after the first child of this kind (the
    /// value after `=`, the return type after `->`).
    After(K),
    /// The significant child just before the first child of this kind (the
    /// key before `=>`, the last item before `)`).
    Before(K),
    /// List: every significant child that is not a skip kind.
    All,
    /// List: every child node.
    Nodes,
    /// List: every significant child of this kind.
    AllKind(K),
    /// List: the significant children from position `from` up to `back`
    /// positions before the end (`Range(1, 1)` is everything between a node's
    /// first and last child, such as the items between `(` and `)`).
    Range(u16, u16),
    /// List: every significant child after the first child of this kind.
    AllAfter(K),
    /// List: the significant children after the first child of the first kind
    /// and before the next child of the second kind (or the end).
    Between(K, K),
}

// Written out: `#[derive(Default)]` would require `K: Default`, which kind
// types such as lang-forge's `Kind` do not implement.
#[allow(clippy::derivable_impls)]
impl<K> Default for Pick<K> {
    fn default() -> Self {
        Self::Absent
    }
}

impl<K> Pick<K> {
    /// Whether this pick names nothing (`Absent`).
    ///
    /// # Examples
    ///
    /// ```
    /// use lower_lang::Pick;
    ///
    /// assert!(Pick::<u8>::Absent.is_absent());
    /// assert!(!Pick::<u8>::Nth(0).is_absent());
    /// ```
    #[must_use]
    pub const fn is_absent(&self) -> bool {
        matches!(self, Self::Absent)
    }
}

/// Supplies field labels for the children of a node: the integration point for
/// trees whose grammar names its fields (LSF2 `name:expr`).
///
/// `index` is the child's position among **all** of `parent`'s children,
/// trivia included, as [`Node::children`] yields them. Return the label id the
/// mapping's [`Pick::Label`] values use, or `None` for an unlabelled child.
///
/// # Examples
///
/// ```
/// use lower_lang::Labeler;
/// use syntax_lang::Node;
///
/// // A toy host whose grammar labels the first child of every node `0`.
/// struct FirstIsZero;
/// impl<K> Labeler<K> for FirstIsZero {
///     fn label(&self, _parent: &Node<K>, index: usize) -> Option<u16> {
///         (index == 0).then_some(0)
///     }
/// }
/// let n: Node<u8> = Node::new(1, vec![]);
/// assert_eq!(FirstIsZero.label(&n, 0), Some(0));
/// assert_eq!(FirstIsZero.label(&n, 1), None);
/// ```
pub trait Labeler<K> {
    /// The label of `parent`'s child at `index`, if it has one.
    fn label(&self, parent: &Node<K>, index: usize) -> Option<u16>;
}

/// One significant child with its label.
#[derive(Clone, Copy)]
pub(crate) struct Kid<'t, K> {
    pub(crate) elem: &'t Element<K>,
    pub(crate) label: Option<u16>,
}

/// The kind sets a pick consults while selecting.
pub(crate) trait KindSets<K> {
    /// Whether list picks leave this kind out.
    fn is_skip(&self, kind: &K) -> bool;
    /// Whether list picks splice this node kind's children in place.
    fn is_container(&self, kind: &K) -> bool;
    /// Whether nodes of this kind are attributes (invisible to picks).
    fn is_attr(&self, _kind: &K) -> bool {
        false
    }
}

/// Collects `node`'s significant children (and their labels) into `out`.
/// Attribute nodes, and container nodes whose significant non-skip children
/// are all attributes (an attribute group such as `#[A, B]`), are not
/// children for picks: with `attrs`, the attributes are collected there
/// instead (a group's attributes spliced in order). A container that mixes
/// attributes with other children is an ordinary container.
pub(crate) fn collect_kids<'t, K: Copy + TokenKind>(
    node: &'t Node<K>,
    labeler: Option<&dyn Labeler<K>>,
    sets: &dyn KindSets<K>,
    out: &mut Vec<Kid<'t, K>>,
    mut attrs: Option<&mut Vec<&'t Element<K>>>,
) {
    out.clear();
    if let Some(a) = attrs.as_deref_mut() {
        a.clear();
    }
    for (index, elem) in node.children().enumerate() {
        match elem {
            Element::Token(t) => {
                if t.kind().is_trivia() {
                    continue;
                }
            }
            Element::Node(n) => {
                if sets.is_attr(n.kind()) {
                    if let Some(a) = attrs.as_deref_mut() {
                        a.push(elem);
                    }
                    continue;
                }
                if sets.is_container(n.kind()) && is_attr_group(n, sets) {
                    if let Some(a) = attrs.as_deref_mut() {
                        a.extend(
                            n.children().filter(
                                |c| matches!(c, Element::Node(x) if sets.is_attr(x.kind())),
                            ),
                        );
                    }
                    continue;
                }
            }
        }
        let label = match labeler {
            Some(l) => l.label(node, index),
            None => None,
        };
        out.push(Kid { elem, label });
    }
}

/// Whether a container node holds attributes and nothing else significant
/// (skip kinds such as `#[`, `,`, `]` aside).
fn is_attr_group<K: Copy + TokenKind>(n: &Node<K>, sets: &dyn KindSets<K>) -> bool {
    let mut any = false;
    for c in n.children() {
        match c {
            Element::Token(t) => {
                if !t.kind().is_trivia() && !sets.is_skip(t.kind()) {
                    return false;
                }
            }
            Element::Node(x) => {
                if sets.is_attr(x.kind()) {
                    any = true;
                } else if !sets.is_skip(x.kind()) {
                    return false;
                }
            }
        }
    }
    any
}

/// The element's kind.
fn kind_of<K>(e: &Element<K>) -> &K {
    e.kind()
}

/// Resolves a pick that names one child.
pub(crate) fn single<'t, K: Copy + Eq + TokenKind>(
    pick: Pick<K>,
    kids: &[Kid<'t, K>],
    sets: &dyn KindSets<K>,
) -> Option<&'t Element<K>> {
    let at = |i: usize| kids.get(i).map(|k| k.elem);
    match pick {
        Pick::Absent => None,
        Pick::Label(l) => kids.iter().find(|k| k.label == Some(l)).map(|k| k.elem),
        Pick::Nth(n) => at(usize::from(n)),
        Pick::Back(n) => kids.len().checked_sub(usize::from(n) + 1).and_then(at),
        Pick::NthNode(n) => kids
            .iter()
            .filter(|k| k.elem.is_node())
            .nth(usize::from(n))
            .map(|k| k.elem),
        Pick::Kind(kind) => kids
            .iter()
            .find(|k| *kind_of(k.elem) == kind)
            .map(|k| k.elem),
        Pick::KindNth(kind, n) => kids
            .iter()
            .filter(|k| *kind_of(k.elem) == kind)
            .nth(usize::from(n))
            .map(|k| k.elem),
        Pick::First(kind) => kids
            .first()
            .filter(|k| *kind_of(k.elem) == kind)
            .map(|k| k.elem),
        Pick::After(kind) => {
            let i = kids.iter().position(|k| *kind_of(k.elem) == kind)?;
            at(i + 1)
        }
        Pick::Before(kind) => {
            let i = kids.iter().position(|k| *kind_of(k.elem) == kind)?;
            at(i.checked_sub(1)?)
        }
        Pick::All
        | Pick::Nodes
        | Pick::AllKind(_)
        | Pick::Range(..)
        | Pick::AllAfter(_)
        | Pick::Between(..) => {
            let (lo, hi) = list_bounds(pick, kids);
            kids.get(lo..hi)?
                .iter()
                .find(|k| list_keeps(pick, k, sets))
                .map(|k| k.elem)
        }
    }
}

/// The index range a list pick scans.
fn list_bounds<K: Copy + Eq>(pick: Pick<K>, kids: &[Kid<'_, K>]) -> (usize, usize) {
    let len = kids.len();
    match pick {
        Pick::Range(from, back) => {
            let lo = usize::from(from).min(len);
            let hi = len.saturating_sub(usize::from(back)).max(lo);
            (lo, hi)
        }
        Pick::AllAfter(kind) => match kids.iter().position(|k| *kind_of(k.elem) == kind) {
            Some(i) => (i + 1, len),
            None => (len, len),
        },
        Pick::Between(open, close) => match kids.iter().position(|k| *kind_of(k.elem) == open) {
            Some(i) => {
                let end = kids
                    .get(i + 1..)
                    .and_then(|rest| rest.iter().position(|k| *kind_of(k.elem) == close))
                    .map_or(len, |j| i + 1 + j);
                (i + 1, end)
            }
            None => (len, len),
        },
        _ => (0, len),
    }
}

/// Whether a list pick keeps this child (before container splicing).
fn list_keeps<K: Copy + Eq>(pick: Pick<K>, kid: &Kid<'_, K>, sets: &dyn KindSets<K>) -> bool {
    let kind = kind_of(kid.elem);
    match pick {
        Pick::Nodes => kid.elem.is_node(),
        Pick::AllKind(k) => *kind == k,
        Pick::Label(l) => kid.label == Some(l),
        _ => !sets.is_skip(kind),
    }
}

/// Resolves a pick as a list, appending the selected elements to `out` in
/// source order. Container nodes are replaced by their significant, non-skip
/// children, iteratively (nested containers are spliced too).
pub(crate) fn list<'t, K: Copy + Eq + TokenKind>(
    pick: Pick<K>,
    kids: &[Kid<'t, K>],
    sets: &dyn KindSets<K>,
    out: &mut Vec<&'t Element<K>>,
) {
    match pick {
        Pick::Absent => {}
        Pick::Nth(_)
        | Pick::Back(_)
        | Pick::NthNode(_)
        | Pick::Kind(_)
        | Pick::KindNth(..)
        | Pick::First(_)
        | Pick::After(_)
        | Pick::Before(_) => {
            if let Some(e) = single(pick, kids, sets) {
                splice_into(e, sets, out);
            }
        }
        Pick::Label(_)
        | Pick::All
        | Pick::Nodes
        | Pick::AllKind(_)
        | Pick::Range(..)
        | Pick::AllAfter(_)
        | Pick::Between(..) => {
            let (lo, hi) = list_bounds(pick, kids);
            if let Some(slice) = kids.get(lo..hi) {
                for kid in slice {
                    if list_keeps(pick, kid, sets) {
                        splice_into(kid.elem, sets, out);
                    }
                }
            }
        }
    }
}

/// Pushes `elem`, or, for a container node, its significant non-skip
/// descendants through nested containers, with an explicit stack.
fn splice_into<'t, K: Copy + Eq + TokenKind>(
    elem: &'t Element<K>,
    sets: &dyn KindSets<K>,
    out: &mut Vec<&'t Element<K>>,
) {
    let Element::Node(node) = elem else {
        out.push(elem);
        return;
    };
    if !sets.is_container(node.kind()) {
        out.push(elem);
        return;
    }
    let mut stack = Vec::new();
    stack.push(node.children());
    while let Some(top) = stack.last_mut() {
        let Some(child) = top.next() else {
            let _ = stack.pop();
            continue;
        };
        match child {
            Element::Token(t) => {
                if !t.kind().is_trivia() && !sets.is_skip(t.kind()) {
                    out.push(child);
                }
            }
            Element::Node(n) => {
                if sets.is_container(n.kind()) {
                    stack.push(n.children());
                } else if !sets.is_skip(n.kind()) {
                    out.push(child);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use syntax_lang::{Span, Token};

    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
    enum K {
        Root,
        List,
        Ident,
        Comma,
        Open,
        Close,
        Space,
    }
    impl TokenKind for K {
        fn is_trivia(&self) -> bool {
            matches!(self, K::Space)
        }
    }

    struct Sets;
    impl KindSets<K> for Sets {
        fn is_skip(&self, kind: &K) -> bool {
            *kind == K::Comma
        }
        fn is_container(&self, kind: &K) -> bool {
            *kind == K::List
        }
    }

    fn tok(kind: K, at: u32) -> Element<K> {
        Element::Token(Token::new(kind, Span::new(at, at + 1)))
    }

    /// `( a , [b , c] )` where `[...]` is a container node.
    fn sample() -> Node<K> {
        let inner = Node::new(
            K::List,
            vec![tok(K::Ident, 5), tok(K::Comma, 6), tok(K::Ident, 7)],
        );
        Node::new(
            K::Root,
            vec![
                tok(K::Open, 0),
                tok(K::Space, 1),
                tok(K::Ident, 2),
                tok(K::Comma, 3),
                Element::Node(inner),
                tok(K::Close, 9),
            ],
        )
    }

    #[test]
    fn test_positional_picks_skip_trivia() {
        let root = sample();
        let mut kids = Vec::new();
        collect_kids(&root, None, &Sets, &mut kids, None);
        assert_eq!(kids.len(), 5);
        let span = |p| single(p, &kids, &Sets).map(|e| e.span().start().to_u32());
        assert_eq!(span(Pick::Nth(1)), Some(2));
        assert_eq!(span(Pick::Back(0)), Some(9));
        assert_eq!(span(Pick::NthNode(0)), Some(5));
        assert_eq!(span(Pick::Kind(K::Ident)), Some(2));
        assert_eq!(span(Pick::After(K::Comma)), Some(5));
        assert_eq!(span(Pick::Before(K::Comma)), Some(2));
        assert_eq!(span(Pick::Before(K::Open)), None);
        assert_eq!(span(Pick::First(K::Open)), Some(0));
        assert_eq!(span(Pick::First(K::Ident)), None);
        assert_eq!(span(Pick::Nth(40)), None);
        assert_eq!(span(Pick::Back(40)), None);
        assert_eq!(span(Pick::Absent), None);
    }

    #[test]
    fn test_list_picks_splice_containers_and_drop_skips() {
        let root = sample();
        let mut kids = Vec::new();
        collect_kids(&root, None, &Sets, &mut kids, None);
        let starts = |p| {
            let mut out = Vec::new();
            list(p, &kids, &Sets, &mut out);
            out.iter()
                .map(|e| e.span().start().to_u32())
                .collect::<Vec<_>>()
        };
        assert_eq!(starts(Pick::Range(1, 1)), [2, 5, 7]);
        assert_eq!(starts(Pick::Between(K::Open, K::Close)), [2, 5, 7]);
        assert_eq!(starts(Pick::AllKind(K::Ident)), [2]);
        assert_eq!(starts(Pick::Nodes), [5, 7]);
        assert_eq!(starts(Pick::AllAfter(K::Comma)), [5, 7, 9]);
        assert_eq!(starts(Pick::Range(9, 9)), Vec::<u32>::new());
        assert_eq!(starts(Pick::Nth(1)), [2]);
    }

    #[test]
    fn test_labels_come_from_the_labeler() {
        struct Odd;
        impl Labeler<K> for Odd {
            fn label(&self, _: &Node<K>, index: usize) -> Option<u16> {
                (index % 2 == 0).then_some(3)
            }
        }
        let root = sample();
        let mut kids = Vec::new();
        collect_kids(&root, Some(&Odd), &Sets, &mut kids, None);
        let first = single(Pick::Label(3), &kids, &Sets).map(|e| e.span().start().to_u32());
        assert_eq!(first, Some(0));
        let mut out = Vec::new();
        list(Pick::Label(3), &kids, &Sets, &mut out);
        // `(`, `a`, and the container (spliced into `b`, `c`).
        assert_eq!(out.len(), 4);
    }

    #[test]
    fn test_deeply_nested_containers_splice_without_recursion() {
        let mut node = Node::new(K::List, vec![tok(K::Ident, 0)]);
        for _ in 0..100_000 {
            node = Node::new(K::List, vec![Element::Node(node)]);
        }
        let root = Node::new(K::Root, vec![Element::Node(node)]);
        let mut kids = Vec::new();
        collect_kids(&root, None, &Sets, &mut kids, None);
        let mut out = Vec::new();
        list(Pick::All, &kids, &Sets, &mut out);
        assert_eq!(out.len(), 1);
    }
}
