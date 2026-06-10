English | [简体中文](README.zh-CN.md)

<div align="center">
    <h2>LK</h2>
    <h5>a lightweight, efficient, modern language written in Rust</h5>
</div>

## Features

- Rust-inspired syntax with first-class named parameters
- Rust-shaped `macro_rules!` declarative macros with function-like calls, explicit macro exports/re-exports, file/package imports, standard `macros` imports, item attributes, and built-in `#[derive(Debug|Show)]`; see [docs/macros.md](docs/macros.md) for the macro ecosystem roadmap
- VM interpreter and LLVM compiler backend, supporting cross-platform native compilation and browser WASM
- Built-in standard library and syntax sugar
- Package manager and REPL, with VS Code LSP extension support

## Examples

Details: [lang.lollipopkit.com](https://lang.lollipopkit.com).

### Example Files

```
examples/
├── syntax/          # Language feature demos
│   ├── closure.lk        # Closures & higher-order functions
│   ├── match.lk          # Match expressions and patterns
│   ├── pattern_matching.lk # if-let, while-let, destructuring
│   ├── ...               # More
├── stdlib/           # Standard library demos
│   ├── list_ops.lk        # List methods (map, filter, reduce)
│   ├── stream_demo.lk     # Lazy stream pipelines
│   ├── ...               # More
├── general/          # Practical examples
│   ├── sort_search.lk    # Insertion sort and search algorithms
│   ├── config_parser.lk  # JSON/YAML/TOML config loading
│   ├── ...
└── _references/      # Cross-language references (Dart, Lua, C)
```

Run any example: `lk examples/syntax/closure.lk`

## Usage

### Integration (library)

```rust
use lk_core::{syntax::{parse_program_source, ParseOptions}, vm::VmContext};

// Parse and execute through the bytecode VM.
let source = r#"
let data = {
    "req": { "user": { "name": "foo" } },
    "files": [ { "name": "file1", "published": true } ],
};
return data.req.user.name in "foobar" && data.files.0.published == true;
"#;
let program = parse_program_source(source, ParseOptions::default())?;
let mut ctx = VmContext::new();
let result = program.execute_with_ctx(&mut ctx)?;

assert_eq!(result.display_first_return(), "true");
```

### CLI

- Run REPL: `lk`
- Execute a source file or module artifact: `lk FILE` (supports `.lk` and `.lkm`)
- Type-check without executing: `lk check FILE` (reports compile-time diagnostics)
- Compile to a native executable: `lk compile [FILE]` (omitting `FILE` uses `./main.lk`, package `./src/main.lk`, or a single workspace app entry; unsupported LLVM-native shapes fail)
- Compile to a bytecode module artifact: `lk compile bytecode [FILE]` → `FILE.lkm`
- Compile to LLVM IR: `lk compile llvm [FILE]` (see [docs/llvm/backend.md](docs/llvm/backend.md) for backend details)
- Create packages and manage dependencies: `lk pkg init`, `lk pkg add`, `lk pkg fetch`, `lk pkg tree` (see [docs/packages.md](docs/packages.md))

Note: command-line argument paths must be sanitized relative paths.

### VS Code

VS Code support is a single merged extension under `vsc-ext/lsp`. It includes `.lk` language registration, TextMate highlighting, snippets, and the LK LSP client with smart completion for stdlib modules, imported aliases, local symbols, named arguments, repeated string argument values, and common receiver methods. Use `make debug-lsp-ext` for a local Extension Development Host, or `make vsix` to build the VSIX.

## License

```plaintext
Apache-2.0 lollipopkit
```
