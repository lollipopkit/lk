# LKR Language Server

A Language Server Protocol (LSP) implementation for the LKR (Query Check Language) domain-specific language.

## Features

- **Syntax Diagnostics**: Real-time error detection for LKR expressions and statement programs
- **Hover Information**: Shows type information, identifier roots, and symbol counts
- **Code Completion**: Auto-complete for LKR keywords, operators, common variables, and standard library functions
- **Document Symbols**: Navigate through variables, functions, and imports in LKR programs
- **Identifier Analysis**: Detects and analyzes top-level identifier roots used (req, record, etc.)

## Architecture

The LSP server consists of:

- `main.rs`: Core LSP server implementation using tower-lsp
- `analyzer.rs`: LKR language analysis engine that provides:
  - Expression and statement parsing
  - Symbol extraction (variables, functions, imports)
  - Identifier root collection
  - Diagnostic generation

## Supported Language Features

### LKR Expressions
- Identifier/property access (`req.user.role`)
- Arithmetic operations (`+`, `-`, `*`, `/`, `%`)
- Logical operations (`&&`, `||`, `!`)
- Comparison operations (`==`, `!=`, `<`, `>`, `<=`, `>=`, `in`)

### LKR Statements
- Variable declarations (`let x = value;`)
- Function definitions (`fn name(params) { body }`)
- Import statements (`import math;`, `import { abs } from math;`)
- Control flow (`if`, `while`, `break`, `continue`, `return`)
- Concurrency primitives (`go`, `select`, channel operations)

### Completions Provided

#### Keywords
- Control flow: `if`, `else`, `while`, `let`, `fn`, `return`, `break`, `continue`
- Imports: `import`, `from`, `as`
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
cargo build -p lkr-lsp
```

### Running
```bash
cargo run -p lkr-lsp
```

The server communicates via stdin/stdout using the LSP JSON-RPC protocol.

### One‑shot File Analysis (CLI)

Analyze a single file from the command line and print JSON containing diagnostics, symbols, identifier roots, and semantic tokens:

```bash
cargo run -p lkr-lsp -- --analyze path/to/file.lkr
```

Notes:
- The file path must be relative (no absolute paths or `..`).
- Output is prettified JSON suitable for piping to `jq`.

### Integration with Editors

#### VS Code
Create a VS Code extension that launches the LSP server:
```json
{
  "name": "lkr",
  "engines": { "vscode": "^1.50.0" },
  "contributes": {
    "languages": [{
      "id": "lkr",
      "extensions": [".lkr"]
    }]
  },
  "activationEvents": ["onLanguage:lkr"]
}
```

#### Neovim
Use nvim-lspconfig:
```lua
require'lspconfig'.configs.lkr = {
  default_config = {
    cmd = {'lkr-lsp'},
    filetypes = {'lkr'},
    root_dir = require('lspconfig.util').root_pattern('.git'),
  }
}
```

## Development

The LSP server leverages the LKR core library for parsing and analysis:
- Expression parsing via `lkr_core::expr::Expr`
- Statement parsing via `lkr_core::stmt_parser::StmtParser`
- Tokenization via `lkr_core::token::Tokenizer`

### Testing
Test the LSP server with a LKR file containing:
```lkr
// Expression example
req.user.role == 'admin' && req.user.level >= 5

// Statement program example
import math;
let result = math.sqrt(req.user.score);
fn validate_user(user) {
    return user.role == 'admin' || user.level >= 10;
}
if (validate_user(req.user)) {
    return true;
}
```
