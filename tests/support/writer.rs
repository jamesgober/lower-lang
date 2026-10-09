//! A generic tree writer for tests: kinds are a `u16` newtype, tokens are
//! written with their text so the source and the spans always agree.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use syntax_lang::{Builder, Node, Span, Token, TokenKind};

/// A kind: `K(0)` is whitespace (trivia); tests give the rest meaning.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct K(pub u16);

impl TokenKind for K {
    fn is_trivia(&self) -> bool {
        self.0 == 0
    }
}

pub const SPACE: K = K(0);

/// Writes a tree and its source together.
pub struct W {
    b: Builder<K>,
    pub src: String,
}

impl Default for W {
    fn default() -> Self {
        Self::new()
    }
}

impl W {
    pub fn new() -> Self {
        Self {
            b: Builder::new(),
            src: String::new(),
        }
    }

    pub fn open(&mut self, k: K) -> &mut Self {
        self.b.start_node(k);
        self
    }

    pub fn close(&mut self) -> &mut Self {
        self.b.finish_node();
        self
    }

    pub fn t(&mut self, k: K, text: &str) -> &mut Self {
        if !self.src.is_empty() {
            let at = self.src.len() as u32;
            self.src.push(' ');
            self.b.token(Token::new(SPACE, Span::new(at, at + 1)));
        }
        let start = self.src.len() as u32;
        self.src.push_str(text);
        self.b
            .token(Token::new(k, Span::new(start, self.src.len() as u32)));
        self
    }

    pub fn finish(self) -> (Node<K>, String) {
        (self.b.finish().expect("balanced"), self.src)
    }
}
