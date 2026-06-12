# LK for Zed

Zed extension for LK language support.

## Features

- `.lk` language registration.
- Tree-sitter syntax highlighting, indentation, folding, locals, and injection queries.
- LK Language Server (`lk-lsp`) integration for diagnostics, completion, hover, goto definition, document symbols, semantic tokens, and inlay hints.

VS Code-specific UI affordances such as the LK status bar menu, QuickPick actions, and `LK: Analyze Current File` are not portable to Zed. Use Zed's LSP settings and command palette for equivalent editor-level control.

## Requirements

Build or install `lk-lsp`:

```sh
cargo build -p lk-lsp
```

The extension searches for `lk-lsp` in:

- `target/debug` and `target/release` in the current worktree or its ancestors.
- `~/.cargo/bin`
- Homebrew paths on macOS.
- `PATH`.

You can also set a custom binary path in Zed settings:

```json
{
  "lsp": {
    "lk-lsp": {
      "binary": {
        "path": "/absolute/path/to/lk-lsp"
      }
    }
  }
}
```

## Development

From the repo root:

```sh
cargo check --manifest-path ecosystem/zed-ext/Cargo.toml --target wasm32-wasip1
```

Load `ecosystem/zed-ext` as a Zed dev extension and open `examples/lk-example-workspace`.

The grammar source in `extension.toml` points at `https://github.com/lollipopkit/lk` with `path = "ecosystem/tree-sitter-lk"`. Replace `REPLACE_WITH_RELEASE_COMMIT` with the published commit that contains this ecosystem layout before releasing or installing the extension from Git.
