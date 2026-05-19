English | [简体中文](README.zh-CN.md)

<div align="center">
    <h2>LK</h2>
    <h5>a Rust-like scripting language written in Rust</h5>
</div>

## Intro

### Example

```lk
fn draw_rect(x: Int, y: Int, {width: Int, height: Int? = 100}) -> Int {
    let h = height ?? 0;
    return width * h;
}

print(draw_rect(0, 0, width: 20, height: 10));
```

Outputs `200`. More language details: [LANG.md](LANG.md).

### Examples

```
examples/
├── syntax/          # Language feature demos
│   ├── closure.lk        # Closures & higher-order functions
│   ├── struct.lk          # Struct definition & instantiation
│   ├── trait_impl.lk      # Trait definition & impl
│   ├── ...               # and many more
├── stdlib/           # Standard library demos
│   ├── string_methods.lk # String operations & methods
│   ├── math_demo.lk      # Math module (sqrt, sin, pow, ...)
│   ├── ...               # and more
├── general/          # Practical examples
│   ├── word_count.lk     # Text processing & word frequency
│   ├── config_parser.lk  # JSON/YAML/TOML config loading
│   ├── ...
└── _references/      # Cross-language references (Dart, Lua, C)
```

Run any example: `lk examples/syntax/closure.lk`

### Highlights
- Rust-inspired syntax with first-class named parameters
- Deterministic bytecode VM with optional concurrency runtime
- Batteries-included standard library and LSP-backed tooling

## Features

### Usage

#### Integration (library)

```rust
use lk_core::{expr::Expr, vm::VmContext, val::Val};

// Parse expr
let expr_src = "data.req.user.name in 'foobar' && data.files.0.published == true";
let expr = Expr::try_from(expr_src)?;

// Provide variables in VmContext (lexical environment)
let mut ctx = VmContext::new();
let data_val: Val = serde_json::json!({
    "req": { "user": { "name": "foo" } },
    "files": [ { "name": "file1", "published": true } ]
}).into();
ctx.set("data", data_val);

// Eval
let result = expr.eval_with_ctx(&mut ctx)?; // Val::Bool(true)
assert_eq!(result, Val::Bool(true));
```

#### CLI

- Run REPL: `lk`
- Execute a file: `lk FILE` (auto-detects `.lk` source vs `.lkb` bytecode)
- Type-check without executing: `lk check FILE` (reports compile-time diagnostics)
- Compile to bytecode: `lk compile [FILE]` → `FILE.lkb` (omitting `FILE` uses `./main.lk`, package `./src/main.lk`, or a single workspace app entry; see [docs/lkb.md](docs/lkb.md) for bundling details)
- Compile to LLVM IR: `lk compile llvm [FILE]` (see [docs/llvm/backend.md](docs/llvm/backend.md) for backend details)
- Compile to ELF executable: `lk compile exe [FILE]` (requires LLVM tools + system linker; see [docs/llvm/backend.md](docs/llvm/backend.md))
- Create packages and manage dependencies: `lk init`, `lk pkg add`, `lk pkg fetch`, `lk pkg tree` (see [docs/packages.md](docs/packages.md))

Note: command-line paths must be relative and sanitized.

#### VS Code

The VS Code support is a single merged extension under `vsc-ext/lsp`. It includes `.lk` language registration, TextMate highlighting, snippets, and the LK LSP client. Use `make debug-lsp-ext` for a local Extension Development Host, or `make vsix` to build the VSIX.

## License

```plaintext
Apache-2.0 lollipopkit
```
