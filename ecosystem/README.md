# LK Editor Ecosystem

This directory contains editor and parser integrations for LK.

- `tree-sitter-lk/`: Tree-sitter grammar and queries for LK.
- `vsc-ext/lsp/`: VS Code extension with TextMate highlighting, snippets, and LK LSP client support.
- `zed-ext/`: Zed extension using the same Tree-sitter grammar and `lk-lsp`.

## Verification

```sh
cargo check -p tree-sitter-lk
cd ecosystem/tree-sitter-lk && npm test
npm --prefix ecosystem/vsc-ext/lsp run compile
cargo check --manifest-path ecosystem/zed-ext/Cargo.toml --target wasm32-wasip1
```
