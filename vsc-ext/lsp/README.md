# LK Language Support for VS Code

This extension provides language support for LK in Visual Studio Code, including syntax highlighting, snippets, language configuration, and language server features.

## Language Server Implementation Guide

This extension implements the VS Code Language Server Protocol (LSP) based on the [official VS Code Language Server Extension Guide](https://code.visualstudio.com/api/language-extensions/language-server-extension-guide).

### Language Server Architecture

The extension follows the standard VS Code Language Server pattern with two main components:

- **Language Client**: A normal VS Code extension written in TypeScript that has access to the VS Code API
- **Language Server**: The LK language analysis tool (`lk-lsp`) running in a separate process

### Key Benefits

1. **Native Language Integration**: The language server is implemented in Rust, not JavaScript/TypeScript
2. **Performance**: The server runs in a separate process, avoiding performance costs on VS Code's main thread
3. **Standardization**: Uses the Language Server Protocol (LSP) for standardized communication

### Implementation Structure

```
vscode-lk/
├── src/
│   └── extension.ts          # Language Client implementation
├── language-configuration.json
│                             # Brackets, comments, indentation rules
├── syntaxes/
│   └── lk.tmLanguage.json   # TextMate grammar for syntax highlighting
├── snippets/
│   └── lk.code-snippets.json # LK snippets
├── package.json              # Extension manifest
└── ...                       # Other configuration files
```

The LK LSP server is built separately in the `lsp/` directory of the main LK project.
The previous standalone `lk-highlight` extension has been merged into this package; install only this extension for both TextMate highlighting and LSP features.

## Features

- Syntax highlighting for LK files
- Language configuration for `.lk` files
- Snippets for common LK constructs
- Language Server Protocol (LSP) integration for:
  - Real-time error detection and diagnostics
  - Code completion
  - Markdown hover information with LK signatures, doc comments, package docs, and type links
  - Go to definition
  - Document symbols
- Inlay hints (parameter + type hints)

### Type diagnostics

- Strict type diagnostics use whole-program call-site constraints before reporting implicit `Any` parameters.
- If a function parameter remains unresolved, the error is attached to the parameter name rather than the file header.

### Hover docs and type links

- Hovering LK declarations and calls shows the declaration in a fenced `lk` code block.
- `///` and `/** ... */` comments immediately above `fn`, `struct`, `trait`, or `type` declarations render as Markdown in hover.
- Top-of-file package docs from `//!` and `/*! ... */` in a package root render when hovering `use package` or `use package as alias`.
- Hover type links open LK-defined types through the internal `lk.openLocation` command. Built-in and stdlib/Rust-backed types are intentionally not linked.

### Stdlib awareness
- The client queries the Rust LK language server for stdlib modules and exports. Module-aware completions support:
  - `use <module>` / `from <module>` name completion
  - `<alias>.` namespace member completion for stdlib exports (e.g. `iter.zip`, `iter.take`, `iter.map`, ...)
- Stdlib function hover uses Rust-side metadata generated from function-level `#[stdlib_export]` annotations, including signatures and Markdown docs when present.
- Recent updates synced with the server include:
  - `iter` exports generic higher-order ops: `map(list, fn)`, `filter(list, fn)`, `reduce(list, init, fn)`
  - `list` exposes method sugar delegating to `iter`: `take`, `skip`, `chain`, `flatten`, `unique`, `chunk`, `enumerate`, `zip` in addition to `map/filter/reduce`
  - See examples in `examples/list_iter_sugar.lk` in the repo root.

## Status Bar and Inlay Hints

- The status bar shows LK LSP state, including a spinner during analysis (Checking…). Click it for actions.
- Quick actions include restart/disable and toggles for inlay hints.
- Configure inlay hints via settings:
  - `lk.lsp.inlayHints.enabled`
  - `lk.lsp.inlayHints.parameters.enabled`
  - `lk.lsp.inlayHints.types.enabled`

## Requirements

- The LK LSP server (`lk-lsp`) must be built and available in the system PATH or in the expected locations.

## Installation

1. Clone this repository
2. Install dependencies: `npm --prefix vsc-ext/lsp install`
3. Compile the extension: `npm --prefix vsc-ext/lsp run compile`
4. Build the LK LSP server: `cargo build -p lk-lsp`
5. Run `make debug-lsp-ext` to open an Extension Development Host, or run `make vsix` to build the single VSIX package.

## Development

- `npm run compile`: Compile the TypeScript source
- `npm run watch`: Compile in watch mode
- `make vsix`: Build the merged VS Code extension package from `vsc-ext/lsp`
- `make debug-lsp-ext`: Launch VS Code with the merged extension and a repo-local `lk-lsp`

## LK Language Features

The extension supports the LK language with the following features:

### Syntax Highlighting
- Keywords: `if`, `else`, `while`, `let`, `return`, `fn`, `go`, `select`, `case`, `default`, `break`, `continue`, `use`, `from`, `as`, `in`
- Operators: `||`, `&&`, `==`, `!=`, `<=`, `>=`, `<`, `>`, `+`, `-`, `*`, `/`, `%`, `=`, `!`, `<-`
- Member access: `variable.path`
- Strings and numbers
- Comments

### Language Server Features
- Real-time error checking
- Code completion for keywords, functions, named arguments, receiver methods, and repeated string argument values
- Hover information for symbols
- Document symbols for navigation
- Identifier root analysis

## License

Apache-2.0
