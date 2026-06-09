# LK Language Server

A Language Server Protocol (LSP) implementation for the LK (Query Check Language) domain-specific language.

## Features

- **Syntax Diagnostics**: Real-time error detection for LK expressions and statement programs
- **Hover Information**: Shows type information, identifier roots, and symbol counts
- **Code Completion**: Auto-complete for LK keywords, operators, common variables, and standard library functions
- **Document Symbols**: Navigate through variables, functions, and imports in LK programs
- **Identifier Analysis**: Detects and analyzes top-level identifier roots used (req, record, etc.)

## Architecture

The LSP server consists of:

- `main.rs`: Core LSP server implementation using tower-lsp
- `analyzer.rs`: LK language analysis engine that provides:
  - Expression and statement parsing
  - Symbol extraction (variables, functions, imports)
  - Identifier root collection
  - Diagnostic generation

## Supported Language Features

### LK Expressions
- Identifier/property access (`req.user.role`)
- Arithmetic operations (`+`, `-`, `*`, `/`, `%`)
- Logical operations (`&&`, `||`, `!`)
- Comparison operations (`==`, `!=`, `<`, `>`, `<=`, `>=`, `in`)

### LK Statements
- Variable declarations (`let x = value;`)
- Function definitions (`fn name(params) { body }`)
- Use statements (`use math;`, `use { abs } from math;`)
- Control flow (`if`, `while`, `break`, `continue`, `return`)
- Concurrency primitives (`go`, `select`, channel operations)

### Completions Provided

#### Keywords
- Control flow: `if`, `else`, `while`, `let`, `fn`, `return`, `break`, `continue`
- Imports: `use`, `from`, `as`
- Concurrency: `go`, `select`, `case`, `default`
- Literals: `true`, `false`, `nil`

#### Operators
- Comparison: `==`, `!=`, `<=`, `>=`
- Logical: `&&`, `||`
- Membership: `in`
- Channel: `<-`

#### Common Variables
- `req.user.id`, `req.user.role`, `req.user.name`
- `record.id`, `record.owner`, `record.granted`
- `env`, `time`

#### Standard Library Functions
- Math: `abs`, `sqrt`, `sin`, `cos`
- String: `len`, `substr`
- Concurrency: `make_chan`, `send`, `recv`

## Usage

### Building
```bash
cargo build -p lk-lsp
```

### Running
```bash
cargo run -p lk-lsp
```

The server communicates via stdin/stdout using the LSP JSON-RPC protocol.

### One‑shot File Analysis (CLI)

Analyze a single file from the command line and print JSON containing diagnostics, symbols, identifier roots, and semantic tokens:

```bash
cargo run -p lk-lsp -- --analyze path/to/file.lk
```

Notes:
- The file path must be relative (no absolute paths or `..`).
- Output is prettified JSON suitable for piping to `jq`.

### Type Diagnostics

The analyzer runs strict type checking after parsing a full statement program. Unannotated function parameters are checked after the whole program has contributed call-site constraints, so a parameter such as `fn should_run(name)` is accepted when later calls consistently pass `String` values. If no concrete call-site or body constraint resolves the parameter, the diagnostic is reported on the parameter name instead of at the top of the file.

### Integration with Editors

#### VS Code
Create a VS Code extension that launches the LSP server:
```json
{
  "name": "lk",
  "engines": { "vscode": "^1.50.0" },
  "contributes": {
    "languages": [{
      "id": "lk",
      "extensions": [".lk"]
    }]
  },
  "activationEvents": ["onLanguage:lk", "workspaceContains:Lk.toml"]
}
```

#### Neovim
Use nvim-lspconfig:
```lua
require'lspconfig'.configs.lk = {
  default_config = {
    cmd = {'lk-lsp'},
    filetypes = {'lk'},
    root_dir = require('lspconfig.util').root_pattern('.git'),
  }
}
```

## Development

The LSP server leverages the LK core library for parsing and analysis:
- Expression parsing via `lk_core::expr::Expr`
- Statement parsing via `lk_core::stmt_parser::StmtParser`
- Tokenization via `lk_core::token::Tokenizer`

### Testing
Test the LSP server with a LK file containing:
```lk
// Expression example
req.user.role == 'admin' && req.user.level >= 5

// Statement program example
use math;
let result = math.sqrt(req.user.score);
fn validate_user(user) {
    return user.role == 'admin' || user.level >= 10;
}
if (validate_user(req.user)) {
    return true;
}
```
