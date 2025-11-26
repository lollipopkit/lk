English | [简体中文](README.zh-CN.md)

<div align="center">
    <h2>LKR</h2>
    <h5>a Rust-like scripting language written in Rust</h5>
</div>

## Intro

### Example

```lkr
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

### Documentation
- Language spec: [docs/spec/functions.md](docs/spec/functions.md)
- Runtime and bytecode: [docs/runtime.md](docs/runtime.md), [docs/bytecode.md](docs/bytecode.md)
- LKRB packaging: [docs/lkrb.md](docs/lkrb.md)
- CLI guide: [docs/cli.md](docs/cli.md), LSP guide: [docs/lsp.md](docs/lsp.md)

## Features

### Usage

#### Integration (library)

```rust
use lkr_core::{expr::Expr, vm::VmContext, val::Val};

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

- Run REPL: `lkr`
- Execute a file: `lkr FILE` (auto-detects `.lkr` source vs `.lkrb` bytecode)
- Type-check without executing: `lkr check FILE` (reports compile-time diagnostics)
- Compile to bytecode: `lkr compile FILE` → `FILE.lkrb` (see [docs/lkrb.md](docs/lkrb.md) for bundling details)
- Compile to LLVM IR: `lkr compile llvm FILE` (see [docs/llvm/backend.md](docs/llvm/backend.md) for backend details)
- Compile to ELF executable: `lkr compile exe FILE` (requires LLVM tools + system linker; see [docs/llvm/backend.md](docs/llvm/backend.md))

Note: command-line paths must be relative and sanitized.

## License

```plaintext
Apache-2.0 lollipopkit
```
