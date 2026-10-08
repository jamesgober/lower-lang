<h1 align="center">
    <img width="99" alt="Rust logo" src="https://raw.githubusercontent.com/jamesgober/rust-collection/72baabd71f00e14aa9184efcb16fa3deddda3a0a/assets/rust-logo.svg">
    <br>
    <b>lower-lang</b>
    <br>
    <sub><sup>SYNTAX TREE TO HIR LOWERING</sup></sub>
</h1>

<div align="center">
    <a href="https://crates.io/crates/lower-lang"><img alt="Crates.io" src="https://img.shields.io/crates/v/lower-lang"></a>
    <a href="https://crates.io/crates/lower-lang"><img alt="Downloads" src="https://img.shields.io/crates/d/lower-lang?color=%230099ff"></a>
    <a href="https://docs.rs/lower-lang"><img alt="docs.rs" src="https://img.shields.io/docsrs/lower-lang"></a>
    <a href="https://github.com/jamesgober/lower-lang/actions"><img alt="CI" src="https://github.com/jamesgober/lower-lang/actions/workflows/ci.yml/badge.svg"></a>
    <a href="https://github.com/rust-lang/rfcs/blob/master/text/2495-min-rust-version.md"><img alt="MSRV" src="https://img.shields.io/badge/MSRV-1.85%2B-blue"></a>
</div>

<br>

<div align="left">
    <p>
        <strong>lower-lang</strong> turns a lossless syntax tree into HIR. A language describes its constructs declaratively, as instances of a small set of semantic archetypes (binders, scopes, declarations, control forms, expressions) addressed by field labels, plus quasi-quoted HIR templates for sugar. lower-lang expands them hygienically and records the origin of every node it builds.
    </p>
    <br>
    <hr>
    <p>
        <strong>MSRV is 1.85+</strong> (Rust 2024 edition).
    </p>
    <blockquote>
        <strong>Status: pre-1.0, in active development.</strong> The public API is being designed across the 0.x series and frozen at <code>1.0.0</code>. See <a href="./CHANGELOG.md"><code>CHANGELOG.md</code></a>.
    </blockquote>
</div>

<hr>
<br>

## Installation

```toml
[dependencies]
lower-lang = "0.1"
```

<br>

## Status

This is the <code>v0.1.0</code> scaffold: structure, tooling, and quality gates are in place; the implementation lands across the 0.x series per the <a href="./dev/ROADMAP.md"><code>ROADMAP</code></a> and <a href="./docs/API.md"><code>docs/API.md</code></a>.

<hr>
<br>

## Contributing

See <a href="./dev/DIRECTIVES.md"><code>dev/DIRECTIVES.md</code></a> for engineering standards and the definition of done. Before a PR: `cargo fmt --all`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test --all-features` must be clean.

<br>

<div id="license">
    <h2>License</h2>
    <p>Licensed under either of</p>
    <ul>
        <li><b>Apache License, Version 2.0</b> &mdash; <a href="./LICENSE-APACHE">LICENSE-APACHE</a></li>
        <li><b>MIT License</b> &mdash; <a href="./LICENSE-MIT">LICENSE-MIT</a></li>
    </ul>
    <p>at your option.</p>
</div>

<div align="center">
  <h2></h2>
  <sup>COPYRIGHT <small>&copy;</small> 2026 <strong>James Gober.</strong></sup>
</div>
