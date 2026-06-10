# LK Macros Roadmap

LK macros follow a Rust-shaped surface syntax with LK semantics. The goal is a full macro ecosystem that feels familiar to Rust users while staying compatible with LK's parser, module system, type checker, VM, WASM, LSP, and future native backends.

## Current State

- Implemented: same-file `macro_rules! name { ... }` definitions, `export macro_rules! name { ... }`, `export { internal as public };` macro re-exports, `$crate::helper!()` definition-module anchors for macro helper calls, function-like calls with `name!(...)`, `name![...]`, and `name!{...}`, recursive expansion, repetition with arity/zero-width safeguards, Rust-style follow-set diagnostics for ambiguous `expr`/`stmt`/`pat`/`ty`/`path` matcher positions with LK's block-fragment extension, parser-discovered or grammar-guided capture boundaries for `expr`/`stmt`/`item`/`pat`/`ty`/`path` with validation fallback for all primary fragment kinds, named file/package/std macro imports with aliases, file/package/std macro namespaces through `namespace::macro!`, standard compile-time macros (`vec!`, `assert!`, `assert_eq!`, `assert_ne!`, `matches!`, `panic!`, `todo!`, `unreachable!`), item attribute parsing and preservation for `#[attr] fn/struct/type/trait/impl` plus impl methods, built-in `#[derive(Debug)]` and `#[derive(Show)]` for structs through a post-parse AST expansion pass, built-in `#[cfg(...)]` item filtering with `true`/`false`, `feature = "name"`, `feature("name")`, `not`, `any`, and `all`, a versioned procedural macro protocol data model, isolated process hosting with timeout/output limits/protocol checks, external derive providers, external `#[attr] item` and impl-method transform providers, and function-like procedural macro providers registered through `ProcMacroProviders` or `[macros.derive.NAME]` / `[macros.attribute.NAME]` / `[macros.function_like.NAME]` in `Lk.toml`, process-provider output token span preservation with generated-span fallback, structured procedural macro dependency collection through `ProgramExpansion` / `SourceExpansion` and `lk macro expand --deps`, token-level macro origin/source-map metadata through `SourceExpansion::origins`, nested declarative/proc macro origin stacks, parse diagnostics enriched with macro-origin backtraces after token expansion, `lk macro expand --origins` JSON inspection, a recursion limit, rule-level mismatch notes for unmatched invocations, expansion-stack notes on macro expansion errors, LSP-visible macro expansion diagnostics, a small hygiene pass for template-introduced `let`/`const` locals, and `lk macro expand <file> --trace`.
- Integrated: direct execution, VM compilation, runtime imports, CLI file execution, REPL, coverage, WASM, LSP AST cache, tree-sitter, VSCode grammar, README, and website language specs now parse through the macro-aware pipeline.
- Not implemented yet: exhaustive nested matcher validation beyond the current follow-set checks, full hygiene beyond locals and `$crate` helper references, complete source maps for post-parse AST-generated items, type-checker/LSP semantic diagnostics that directly consume macro-origin stacks, package dependency provider discovery/trust policy, dependency-aware recompilation/watch integration, and stable macro package distribution.

## Rust Comparison

| Rust macro capability | LK status | LK target |
| --- | --- | --- |
| `macro_rules!` definitions | Partial | Complete Rust-shaped declarative macros with LK fragment semantics |
| Function-like calls | Partial | `name!(...)`, `name![...]`, `name!{...}` everywhere LK accepts item/stmt/expr fragments |
| Fragment specifiers | Partial | Parser-driven or grammar-guided fragment boundaries plus follow-set diagnostics for ambiguous matchers; continue expanding edge-case coverage |
| Repetition | Partial | Full nested repetition, separator edge cases, and Rust-grade nesting validation |
| Scoping/import/export | Partial | LK module-aware macro namespace, package imports, std `macros` imports, explicit macro exports/re-exports, and definition-site anchors |
| `$crate` | Partial | Definition-module anchor for private helper macro calls; later extend to generated runtime item references |
| Item attributes | Partial | Parse and preserve `#[attr] item` wrappers; recognized struct derives already route through AST expansion, with attribute item transforms still planned |
| Hygiene | Partial | Mixed-site hygiene for generated locals and definition-site references where LK needs it |
| Diagnostics/backtrace | Partial | Expansion trace, call stack, rule mismatch notes, token origin maps, parse-error macro backtraces, and later type/LSP semantic backtraces |
| Function-like proc macros | Partial | Process-backed `name!(...)` / `name![...]` / `name!{...}` providers expand to token streams; provider output spans and token origins are preserved, dependency-aware rebuilds next |
| Derive macros | Partial | Built-in `Debug`/`Show` derive for structs plus external derive process providers that append generated items; provider output spans are preserved, type derives next |
| Attribute macros | Partial | Built-in `cfg` item filtering plus external `#[attr] item` and impl-method transform providers; provider output spans are preserved |
| Tooling | Partial | CLI expand command with `--trace`, `--deps`, and `--origins`, LSP expansion diagnostics, tree-sitter/VSCode/website parity |

## Implementation Phases

### 1. Declarative Macro Foundation

- Keep the macro implementation split into focused modules as it grows so each file stays under the 1500-line limit.
- Replace the remaining scanner fallback paths with dedicated fragment parsers where practical, and keep extending follow-set diagnostics for nested matcher edge cases.
- Complete nested repetition support beyond the current arity checks, separator validation, optional repetition behavior, and zero-width repetition rejection.
- Preserve span mapping and token origin stacks from call-site tokens, template tokens, `$crate` anchors, and provider output so diagnostics point to the useful source location and explain the macro stack that produced it.
- Add negative tests for ambiguous matchers, unknown fragment kinds, unmatched rules, repetition arity mismatch, recursion limit, and hygiene collisions.

### 2. Macro Namespace and Modules

- Keep file and package macro imports aligned with LK `use`: `use { name as alias } from "file"` and `use { name as alias } from package` import compile-time macro bindings; `use "file"` / `use package` expose `namespace::name!`; `use * as ns from "file"` / `use * as ns from package` expose `ns::name!`. Do not mix runtime item imports and named macro imports in the same `use`; split them into separate statements.
- Keep growing the standard compile-time-only `macros` module beyond the current assertion/control-flow baseline while preserving LK package semantics rather than copying Rust attributes exactly.
- Keep explicit macro export controls strict: external file/package/std macro imports only see `export macro_rules!` definitions and `export { name as alias };` re-exports. Ordinary `macro_rules!` definitions stay usable in the defining file/module and through `$crate::helper!()` from exported macros, but remain private to external importers.
- Extend the current `$crate` anchor from helper macro calls to generated references to definition-module runtime items once macro/module name resolution can preserve that information through later compiler phases.
- Add a definition-module anchor for generated references so macros can reliably refer to helpers defined beside the macro.
- Ensure runtime imports, package imports, REPL sessions, LSP workspace caches, and WASM all share the same macro resolver behavior.

### 3. Hygiene and Diagnostics

- Keep tracking whether each token originates from the call site, macro definition, `$crate` anchor, or process-backed provider output.
- Implement hygienic freshening for generated locals beyond simple `let`/`const` names.
- Extend macro-origin diagnostics from parse errors into strict type checking, LSP semantic diagnostics, hover/definition metadata, and generated-symbol navigation.
- Keep the current expansion trace API and CLI command aligned with structured origin backtraces; `lk macro expand --origins` is the canonical token-level source-map dump.

### 4. Procedural Macro Protocol

- Keep the implemented versioned protocol data structures stable enough for early experimentation.
- Input already models macro kind, macro name, token stream, spans, current package/module identity, feature flags, and protocol version.
- Output already models token stream, diagnostics, optional notes, and deterministic dependency metadata; expansion APIs collect dependency metadata from derive, attribute, and function-like providers.
- Output tokens preserve provider-supplied spans through token expansion and post-parse AST macro expansion; missing output spans fall back to the macro call or attribute span. Function-like provider output also carries token-level `proc_macro_output` origins.
- Keep the process host enforcing timeout, output size limit, protocol version checks, process error isolation, and deterministic failure messages.
- Keep `ProcMacroProviders` as the compiler-facing registry and `[macros.derive.NAME]`, `[macros.attribute.NAME]`, and `[macros.function_like.NAME]` as the manifest-facing provider schema.
- Keep proc macros outside the compiler process; do not use `unsafe` outside the LLVM boundary.

Manifest provider shape:

```toml
[macros.derive.MakeAnswer]
command = "./tools/derive-make-answer"
args = ["--json"]
timeout_ms = 5000
max_output_bytes = 1048576

[macros.attribute.route]
command = "./tools/route-attr"

[macros.function_like.sql]
command = "./tools/sql-macro"
```

### 5. Derive and Attribute Macros

- Keep the implemented attribute preservation layer transparent across parser, type checking, slot resolution, VM compilation, REPL, LSP, tree-sitter, and display.
- Keep the built-in `#[derive(Debug)]` / `#[derive(Show)]` struct expansion generating the internal `show(self) -> String` method that template strings and formatted output already use.
- Keep the built-in `#[cfg(...)]` item filter aligned with Rust-style predicates and wire package feature resolution into `ParseOptions::macro_features`.
- Keep attributes on impl methods routed through the same post-parse attribute macro expansion pass; method macros must expand to zero or one function method.
- Keep `#[derive(Name)]` accepting built-in names and registered external derive providers; unregistered external derives must report a provider diagnostic instead of failing during attribute parsing.
- Extend external derives from struct item append-only generation to type declarations and richer generated spans through the proc macro protocol.
- Keep registered `#[attr] item` macros transforming, replacing, or removing a single item through the proc macro protocol; unregistered attributes remain preserved wrappers.
- Keep `#[attr]` impl-method macros constrained to method-level output so generated impl bodies remain structurally valid.
- Update type checking and symbol collection so generated items participate normally in later compiler phases.

### 6. Ecosystem Tooling

- Keep improving `lk macro expand <file>` with optional token trace output, `--deps` procedural dependency output, `--origins` token source-map output, and AST derive/attribute expansion output as the canonical expansion inspection command.
- Keep improving LSP diagnostics for macro expansion failures, macro definitions, and macro calls.
- Keep tree-sitter, VSCode grammar, website specs, README, and examples in sync with each macro phase.
- Document how macro packages are authored, tested, versioned, and imported through `Lk.toml`.

## Acceptance Matrix

- `macro_rules!` examples equivalent to `vec!`, assertion macros, `matches!`, panic-family macros, and control-flow helpers execute through CLI, VM compilation, WASM, and imports.
- Cross-file exported macros work from package dependencies and workspace members.
- Macro expansion and macro-generated parse failures include rule names, call-site spans, token origins, and an expansion stack.
- Procedural macro crashes or timeouts never crash the LK compiler.
- Generated code is visible to type checking, LSP symbols, semantic tokens, and native/LLVM compilation.

## Verification Commands

Run these after each macro phase:

```sh
cargo fmt --all -- --check
cargo test -p lk-core
cargo test -p lk-core macro_system::origin_tests
cargo test -p lk-cli
cargo test -p lk-cli --test macro_origins_cli_test
cargo test -p lk-lsp
cargo test -p lk-wasm
cargo test -p lk-llvm
cd tree-sitter-lk && npm test
cd website && bun run build
```

`tree-sitter` and website/WASM builds may need host cache permissions when sandboxed.
