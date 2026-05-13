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

## License

```plaintext
Apache-2.0 lollipopkit
```
