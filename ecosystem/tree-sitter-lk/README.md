# tree-sitter-lk

[Tree-sitter](https://tree-sitter.github.io/tree-sitter/) grammar for the [LK](https://github.com/lollipopkit/lk) programming language.

## Features

- Full grammar coverage for LK: expressions, statements, types, patterns, concurrency primitives
- Incremental parsing via tree-sitter
- Supports all LK language features: structs, closures, named parameters, match, ranges, ternary, nullish coalescing, optional chaining

## Installation

### Node.js / npm

```bash
npm install tree-sitter-lk
```

### Rust

Add to your `Cargo.toml`:

```toml
[dependencies]
tree-sitter-lk = { path = "path/to/tree-sitter-lk" }
tree-sitter = "0.24"
```

In this repository, the grammar lives at `ecosystem/tree-sitter-lk`.

### Python

```bash
pip install tree-sitter-lk
```

## Usage

### JavaScript

```javascript
const Parser = require('tree-sitter');
const LK = require('tree-sitter-lk');

const parser = new Parser();
parser.setLanguage(LK);

const tree = parser.parse('let x = 1 + 2;');
console.log(tree.rootNode.toString());
```

### Rust

```rust
use tree_sitter::{Parser, Language};
use tree_sitter_lk::language;

let mut parser = Parser::new();
parser.set_language(&language()).expect("Error loading LK grammar");

let tree = parser.parse("let x = 1 + 2;", None).unwrap();
println!("{}", tree.root_node().to_sexp());
```

## Development

### Generate parser

```bash
npx tree-sitter generate
```

### Run tests

```bash
npx tree-sitter test
```

### Parse a file

```bash
npx tree-sitter parse ../../examples/syntax/control_flow.lk
```

### Build WASM

```bash
npx tree-sitter build --wasm
```

## Language Features Covered

- **Literals:** integers, floats, strings (double, single, raw), template strings with `${}` interpolation, booleans, nil
- **Operators:** arithmetic, comparison, logical, nullish coalescing (`??`), range (`..`, `..=`), ternary (`?:`)
- **Collections:** lists, maps
- **Control flow:** if/else, while, for-in, match, break, continue, return
- **Functions:** named functions, closures (`|x| x + 1`), named parameters
- **Structs:** definition and literals
- **Types:** Int, Float, String, Bool, Nil, Any, List<T>, Map<K,V>, function types, optional types, union types
- **Patterns:** literals, wildcards, identifiers, list/map destructuring, or-patterns, guarded patterns, ranges
- **Uses:** module uses, selective uses, namespace aliases
- **Concurrency:** spawn, chan, send, recv, select
- **Comments:** line (`//`) and block (`/* */`)

## License

Apache-2.0
