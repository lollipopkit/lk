English | [简体中文](README.zh-CN.md)

<div align="center">
    <h2>LK</h2>
    <h5>a lightweight, efficient, modern language written in Rust</h5>
</div>

## Features

- Rust-inspired syntax with first-class named parameters
- **Go-style concurrency**: `go` statements spawn true-parallel goroutines (isolate semantics — no data races by construction), blocking channels, and `select`
- **Swift-style error handling**: errors raise and are caught with `try`/`catch`; postfix `!` force-unwraps nil
- Rust-shaped `macro_rules!` declarative macros with function-like calls, explicit macro exports/re-exports, file/package imports, standard `macros` imports, item attributes, built-in `#[derive(Debug|Show)]`, isolated external derive/attribute/function-like providers, dependency-aware proc macro cache invalidation, LSP macro-origin hover/symbols plus same-file/imported macro and generated item goto-definition, and token-level macro origin/source-map inspection (see [docs/macros.md](docs/macros.md))
- VM interpreter and a Cranelift native-compiler backend, supporting cross-platform native compilation and browser WASM
- Built-in standard library and syntax sugar
- Package manager and REPL, with VS Code LSP extension support

## A Taste

```lk
use task;
use chan as ch;

// Goroutines + channels (Go-style, but isolate: crossing values are
// deep-copied — no shared mutable state, no data races by construction).
fn producer(c, n) {
    for i in 0..n {
        send(c, i * i);
    }
    ch.close(c);
}

let c = chan(4);
go producer(c, 5);

// Errors raise; try/catch is the error-handling surface (Swift-style).
let total = 0;
try {
    while (true) {
        total += recv(c);        // raises once c is closed and drained
    }
} catch e {
    // drained: 0 + 1 + 4 + 9 + 16
}

// select multiplexes channels; postfix `!` force-unwraps nil.
let done = chan(1);
send(done, "ok");
let status = select {
    case v <- recv(done) => v;
    default => "pending";
};
let m = {"total": total};
println("{} (total: {})", status, m["total"]!);   // ok (total: 30)
```

See `docs/concurrency.md` and `docs/semantics.md` for the full semantics.

## Examples

Details: [lang.lollipopkit.com](https://lang.lollipopkit.com).

## On bare metal

LK compiles to a kernel. `bare-metal-x86/` boots on QEMU with no OS underneath
and no `std` in the graph: long mode, interrupts, preemptive tasks whose
*scheduling policy is LK*, PCI, a framebuffer with a font this repository wrote,
PS/2 keyboard and mouse, an ATA disk, a read-only tar filesystem, a free-list
allocator, a window manager with dragging and stacking — and ring 3, with
checked syscalls and an address space per user task.

The drivers are LK modules (`drivers/*.lk`), and so is the interrupt table
itself — LK builds all 256 gates and loads them with `lidt`. What is Rust is the
part a language should not own: the linker script, the boot path, and the
interrupt trampolines, because an interrupt is not a call and the code it lands
in has to have every register spilled before a compiled handler can run.

Eleven QEMU checks run in CI, and each asserts what the machine *scanned out* or
what the disk image holds afterwards — not what the program believes it did.
`bare-metal-x86/README.md` is the long version, including the mistakes: a
soft-float ABI that computed wrong numbers in silence, a shared-page constant
that overlapped a window descriptor, a mouse packet misframed into permanent
stillness.

## Installation

Install the latest GitHub release:

```bash
curl -fsSL https://raw.githubusercontent.com/lollipopkit/lk/main/scripts/install.sh | sh
```

Install a specific release:

```bash
curl -fsSL https://raw.githubusercontent.com/lollipopkit/lk/main/scripts/install.sh | LK_VERSION=v0.1.3 sh
```

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
- Type-check without executing: `lk check FILE` (the same check the executors run; `--strict` also requires every signature to resolve to something other than `Any`)
- Format sources in place: `lk fmt [PATH...]` (no path = the whole project; `--check` reports instead of writing, for CI)
- Compile to a native executable: `lk compile [FILE]` (Cranelift backend; omitting `FILE` uses `./main.lk`, package `./src/main.lk`, or a single workspace app entry; shapes outside the native slice fall back to the Tier 0 VM bundle)
- Compile to a bytecode module artifact: `lk compile bytecode [FILE]` → `FILE.lkm`
- Bundle a self-contained executable that embeds the program *and* the VM: `lk bundle FILE` (AOT Tier 0 — every program bundles, at VM speed)
- Report which instructions a file exercises: `lk coverage FILE` (`--disassemble` prints the bytecode)
- Inspect macro expansion: `lk macro expand FILE` (`--trace`, `--deps`, `--origins`; see [docs/macros.md](docs/macros.md))
- Create packages and manage decentralized git + lockfile dependencies (no central registry): `lk pkg init`, `lk pkg add`, `lk pkg fetch`, `lk pkg update`, `lk pkg check`, `lk pkg tree` (see [docs/packages.md](docs/packages.md))

Note: command-line argument paths must be sanitized relative paths.

### Editor Support

Editor integrations live under `ecosystem/`.

- VS Code support is a single merged extension under `ecosystem/vsc-ext/lsp`. It includes `.lk` language registration, TextMate highlighting, snippets, and the LK LSP client with smart completion for stdlib modules, imported aliases, local symbols, named arguments, repeated string argument values, and common receiver methods. Use `make install` to install the CLI, `lk-lsp` and the extension into every VS Code-family editor found (VS Code / Insiders / VSCodium / Cursor / Windsurf, remote windows included), `make debug-lsp-ext` for a local Extension Development Host, or `make vsix` to only build the VSIX.
- Zed support lives under `ecosystem/zed-ext`. It uses `ecosystem/tree-sitter-lk` for Tree-sitter highlighting and starts `lk-lsp` for diagnostics, completion, hover, goto definition, document symbols, semantic tokens, and inlay hints. Use `make zed-ext-check` to validate the extension crate.

Working on LK itself: [docs/testing.md](docs/testing.md) lists the gates and what each one is the only thing that catches — several are outside `cargo test --workspace`.

## License

```plaintext
Apache-2.0 lollipopkit
```

## Acknowledgements

- Part of the design inspiration came from a handwritten Lua VM/compiler tutorial I read during college.
- Six months of ChatGPT Pro provided through OpenAI OSS.
