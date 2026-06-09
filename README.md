English | [简体中文](README.zh-CN.md)

<div align="center">
    <h2>LK</h2>
    <h5>a lightweight, efficient, modern language written in Rust</h5>
</div>

## Intro

### Example

More language details: [lang.lollipopkit.com](https://lang.lollipopkit.com).

### Example Files

```
examples/
├── syntax/          # Language feature demos
│   ├── closure.lk        # Closures & higher-order functions
│   ├── match.lk          # Match expressions and patterns
│   ├── pattern_matching.lk # if-let, while-let, destructuring
│   ├── operators.lk       # Arithmetic, comparison, logic, ??
│   ├── ...               # and many more
├── stdlib/           # Standard library demos
│   ├── list_ops.lk        # List methods (map, filter, reduce)
│   ├── json_demo.lk       # JSON parsing and processing
│   ├── stream_demo.lk     # Lazy stream pipelines
│   ├── ...               # and more
├── general/          # Practical examples
│   ├── sort_search.lk    # Insertion sort and search algorithms
│   ├── word_count.lk     # Text processing & word frequency
│   ├── config_parser.lk  # JSON/YAML/TOML config loading
│   ├── ...
└── _references/      # Cross-language references (Dart, Lua, C)
```

Run any example: `lk examples/syntax/closure.lk`

## Features

- Rust-inspired syntax with first-class named parameters
- Deterministic bytecode VM with optional concurrency runtime
- Browser-playground wasm facade with a safe stdlib subset
- Standard library, CLI, LSP, and website source maintained in one repository

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
- REPL completion: press Tab for commands, keywords, stdlib modules/exports, receiver methods, and symbols defined earlier in the same REPL session.
- Execute a source file or module artifact: `lk FILE` (supports `.lk` and `.lkm`)
- Type-check without executing: `lk check FILE` (reports compile-time diagnostics)
- Compile to an executable module artifact: `lk compile [FILE]` → `FILE.lkm` (omitting `FILE` uses `./main.lk`, package `./src/main.lk`, or a single workspace app entry)
- Compile to LLVM IR: `lk compile llvm [FILE]` (see [docs/llvm/backend.md](docs/llvm/backend.md) for backend details)
- Compile to an executable: `lk compile exe [FILE]` (native for LLVM-lowerable shapes; unsupported shapes fail; see [docs/llvm/backend.md](docs/llvm/backend.md))
- Create packages and manage dependencies: `lk init`, `lk pkg add`, `lk pkg fetch`, `lk pkg tree` (see [docs/packages.md](docs/packages.md))

Note: command-line paths must be relative and sanitized.

#### VS Code

VS Code support is a single merged extension under `vsc-ext/lsp`. It includes `.lk` language registration, TextMate highlighting, snippets, and the LK LSP client with smart completion for stdlib modules, imported aliases, local symbols, named arguments, and common receiver methods. Use `make debug-lsp-ext` for a local Extension Development Host, or `make vsix` to build the VSIX.

## License

```plaintext
Apache-2.0 lollipopkit
```
