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

Outputs `200`. More language details: [lang.lollipopkit.com](https://lang.lollipopkit.com).

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
- Project website source lives in `website/` and powers [lang.lollipopkit.com](https://lang.lollipopkit.com).

## Features

### Usage

#### Integration (library)

```rust
use lk_core::{stmt::stmt_parser::StmtParser, token::Tokenizer, vm::VmContext};

// Parse and execute through the bytecode VM.
let source = r#"
let data = {
    "req": { "user": { "name": "foo" } },
    "files": [ { "name": "file1", "published": true } ],
};
return data.req.user.name in "foobar" && data.files.0.published == true;
"#;
let tokens = Tokenizer::tokenize(source)?;
let program = StmtParser::new(&tokens).parse_program()?;
let mut ctx = VmContext::new();
let result = program.execute_with_ctx(&mut ctx)?;

assert_eq!(result.display_first_return(), "true");
```

#### CLI

- Run REPL: `lk`
- Execute a source file: `lk FILE`
- Type-check without executing: `lk check FILE` (reports compile-time diagnostics)
- Compile to an executable module artifact: `lk compile [FILE]` → `FILE.lkm` (omitting `FILE` uses `./main.lk`, package `./src/main.lk`, or a single workspace app entry)
- Compile to LLVM IR: `lk compile llvm [FILE]` (see [docs/llvm/backend.md](docs/llvm/backend.md) for backend details)
- Compile to an executable: `lk compile exe [FILE]` (native for LLVM-lowerable shapes; unsupported shapes fail; see [docs/llvm/backend.md](docs/llvm/backend.md))
- Create packages and manage dependencies: `lk init`, `lk pkg add`, `lk pkg fetch`, `lk pkg tree` (see [docs/packages.md](docs/packages.md))

Note: command-line paths must be relative and sanitized.

#### VS Code

The VS Code support is a single merged extension under `vsc-ext/lsp`. It includes `.lk` language registration, TextMate highlighting, snippets, and the LK LSP client. Use `make debug-lsp-ext` for a local Extension Development Host, or `make vsix` to build the VSIX.

## License

```plaintext
Apache-2.0 lollipopkit
```
