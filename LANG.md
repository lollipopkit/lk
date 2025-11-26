## Language Overview

This document describes the LKR language as implemented in this repository (parser, evaluator, statements, types, and standard library wiring).

Comments
- Line comments: `// ...`
- Block comments: `/* ... */`

Identifiers
- Consist of letters, digits, `_`, and `-`. Keywords are reserved. (Be mindful that `-` within identifiers is allowed by the lexer.)

Literals
- String: `"..."` or `'...'` UTF‑8 strings. Supports escapes `\n \r \t \\ \" \' \$ \0`.
- Raw string (Rust‑style, no escapes/interpolation): `r"..."`, `r#"..."#`, `r##"..."##` (multi‑line allowed).
- Int: 64‑bit signed, supports leading sign and scientific notation for floats.
- Float: 64‑bit floating point, supports scientific notation.
- Bool: `true`, `false`
- Nil: `nil`

Collections
- List: `[a, b, c]` (heterogeneous allowed). Indexing: `list[0]`. Safe access helpers via stdlib/meta‑methods.
- Map: `{ key: value, ... }`. Keys are evaluated expressions and coerced to strings at runtime (string/int/float/bool); access with `map.key` or `map["key"]`.

Template Strings
- Interpolation only with `${expr}` inside normal quotes (both `"..."` and `'...'`).
- Raw strings do not support interpolation.
- Examples: `"Hello, ${user.name}!"`, `"Sum: ${1 + 2}"`.

Input and Variables
- There is no implicit runtime context. Identifiers must be defined in the lexical environment (e.g., via `let` in statements, function params, or imports).
- Read external input explicitly with stdlib: `io.read()` (string). Parse manually: `json.parse(...)`, `yaml.parse(...)`, `toml.parse(...)`.
- Example: `import io; import json; let data = json.parse(io.read()); return data.req.user.id == 1;`

Function Calls and Methods
- Call any expression: `f(x, y)`, `(g)(z)`.
- Property access: `expr.field` or `expr[expr]`. Optional chaining: `expr?.field` and `expr?[index]`.
- Method sugar: `value.method(args...)` dispatches as:
  1) If `value.method` yields a callable (closure/native), call it.
  2) Else dispatch a registered meta‑method for the value’s runtime type, passing the receiver as the first argument (e.g., `"abc".len()`; see stdlib).

Closures
- Expression form only: `|a, b| a + b`.

Ranges
- `a..b` and `a..=b` produce integer lists when evaluated (inclusive/exclusive end). Used in patterns as well.

Nullish Coalescing and Ternary
- `lhs ?? rhs` yields `lhs` unless it is `nil`, then `rhs`.
- `cond ? then : else` (right‑associative). In expressions, `cond` must be Bool. In `if`/`while`, truthiness is used (see below).

## Operators (by precedence)
- Postfix: call `()`, dot `.field`, index `[expr]`, optional `?.field`, optional `?[expr]`
- Unary: `!` (logical not)
- Multiplicative: `* / %`
- Additive: `+ -`
- Range: `.. ..=`
- Comparison/membership: `== != < > <= >= in`
- Logical: `&& ||`
- Nullish coalescing: `??`
- Ternary: `? :` (lowest among expression operators)

Notes
- `+` supports String + String concatenation. Other string/number mixes are feature‑gated and not enabled by default.
- `in` supports: substring `str in str`, element membership in lists, and key existence in maps. For `list in list`, it checks all elements of the left are contained in the right.

## Expressions
- Literals, lists, maps, variables, calls, property/index access, closures, ranges, logical/comparison, `??`, and `?:`.
- Concurrency expressions (feature‑gated `concurrency`):
  - `spawn(fn_or_closure)` → Task
  - `chan(capacity?, type?)` → Channel (type is a string like `"Int"`)
  - `send(channel, value)` → Bool
  - `recv(channel)` → `[ok, value]`
  - `select { case recv(c) => expr; case send(c, v) => expr; default => expr }`

Match Expression
- `match value { pattern => expr, ... }` (`,` or `;` separators allowed). Returns the chosen arm’s value. Patterns below.

## Patterns
Used in `match`, `if let`, `while let`, and `let` destructuring.
- Literal: `1`, `3.14`, `"x"`, `true`, `nil`
- Variable binding: `name`
- Wildcard: `_`
- List destructuring: `[p1, p2, ..rest]`
- Map destructuring: `{ "key": pat, other: pat, ..rest }` (keys may be string literals or identifiers; rest binds remaining fields)
- Or‑pattern: `p1 | p2 | p3`
- Guarded pattern: `pat if expr`
- Range pattern: `1..10`, `0..=n`

For‑loop Patterns
- Support an extended pattern set:
  - Variable: `x`
  - Ignore: `_`
  - Tuple: `(a, b, c)`
  - Array: `[a, b, ..rest]`
  - Object: `{ "k": v, ... }` (string keys)

## Statements
- Program is a sequence of statements. Semicolons `;` terminate simple statements and expression statements.

Control Flow
- `if (cond) stmt` or `if cond stmt` (parentheses optional). Truthiness: `false` and `nil` are false; everything else is true.
- `if let pattern = expr stmt [else stmt]`
- `while (cond) stmt` or `while cond stmt`
- `while let pattern = expr stmt`
- `for pattern in expr stmt` where `expr` is iterable: List, String (chars), or Map (iterates `[key, value]`).
- `break;`, `continue;`
- `return;` or `return expr;`

Variables
- Declaration/destructuring: `let pattern [: Type] = expr;`
- Assignment: `name = expr;`
- Compound assignment: `name += expr;`, `-=`, `*=`, `/=`, `%=`
- Short definition: `name := expr;` (define and initialize)
- Lexical scoping: blocks `{ ... }` introduce a new scope.

Structs
- Define: `struct User { id: Int, name: String? }`
- Instantiate (literal): `User { id: 1, name: "Ann" }`
- Instantiate (sugar): `User(id: 1, name: "Ann")`
- Access: `user.name`

Functions
- Definition: `fn name(param1[: Type], param2[: Type]) [-> Type] { statements }`
- Parameters and return type are optional; functions return `nil` by default unless `return` is used.
- First‑class: closures and function values can be passed, returned, and called.
- Named parameters live in an optional trailing block: `fn f(a, b, { flag: Bool = true, label: String }) { ... }`.
- Defaults are lazily evaluated inside the callee when the argument is omitted; expressions can reference other parameters.
- Call sites supply named arguments with `name: expr` after the positional tail: `f(1, 2, label: "demo", flag: false)`. Named arguments may appear in any order but must follow all positional ones.

Imports
- Forms:
  - `import math;` — stdlib module as a namespace
  - `import "path/to/file.lkr";` — file module as a namespace (name is the file stem)
  - `import { abs, sqrt } from math;` — selected items
  - `import { f as g } from "m.lkr";` — with alias
  - `import * as m from math;` — namespace alias
  - `import math as m;` — module alias

- File import resolution and safety:
  - Paths are relative-only and sanitized: absolute paths and any `..` components are rejected.
  - Resolution attempts, in order: `${MOD_NAME}.lkr`, then `${MOD_NAME}/mod.lkr` (relative to the current directory).
  - If you pass a quoted path with `.lkr` already (e.g., `"lib/foo.lkr"`), it must be relative and will be used directly if it exists.

Builtins and Stdlib
- Builtin globals: `print(fmt, ...args)`, `println(fmt, ...args)`, `panic([msg])`.
- Stdlib modules (import as needed): `math`, `string`, `list`, `map`, `iter`, `datetime`, `os`, `tcp`. With `concurrency` feature: `task`, `chan`, `time`.
- `iter` module highlights: `enumerate(list)`, `range([start,] end [, step])`, `zip(list1, list2)`,
  `take(list, n)`, `skip(list, n)`, `chain(list1, list2)`, `flatten(list)`, `unique(list)`, `chunk(list, size)`,
  and generic higher-order ops `map(list, fn)`, `filter(list, fn)`, `reduce(list, init, fn)`.
- Meta‑methods (usable as `value.method()` without importing):
  - String: `len, lower, upper, trim, starts_with, ends_with, contains, replace, substring, split, join`
  - List: `len, push, concat, join, get, first, last, map, filter, reduce, take, skip, chain, flatten, unique, chunk, enumerate, zip`
  - Map: `len, keys, values, has, get`

## CLI Output
- REPL and CLI print evaluation results only when the value is not `nil`. This avoids extra lines after statements that return `nil` by default (e.g., `let`, `fn` definitions, `println(...)`). If you need to display `nil`, print it explicitly via `println(nil)` or include it in formatted output.

## Types and Annotations
Primitive and composite types
- `Int`, `Float`, `String`, `Bool`, `Nil`, `Any`
- `List<T>`, `Map<K, V>`
- `Task<T>`, `Channel<T>` (concurrency)
- Function types: `(T1, T2) -> R`
- Union: `A | B | Nil`; Optional: `T?` (sugar for `T | Nil`; prefix form `?T` is accepted for compatibility)
- Named and generic types are parsed (e.g., `List<Int>`, `Map<String, Int>`)

Annotations
- `let x: Int = 1;`
- `fn f(a: Int, b: String) -> Bool { ... }`
- Type checking/inference is best‑effort and conservative; runtime remains dynamic.

## Grammar (EBNF‑style)

Expressions (precedence from low to high)
```
expr        ::= conditional
conditional ::= nullish [ '?' expr ':' expr ]
nullish    ::= or { '??' or }
or          ::= and { '||' and }
and         ::= cmp { '&&' cmp }
cmp         ::= range { ('==' | '!=' | '<' | '>' | '<=' | '>=' | 'in') range }
range       ::= addsub [ ('..' | '..=') addsub? ]
addsub      ::= muldiv { ('+' | '-') muldiv }
muldiv      ::= unary { ('*' | '/' | '%') unary }
unary       ::= { '!' } postfix
postfix     ::= primary { call | dot | opt_dot | opt_index | index }
call        ::= '(' args ')'
dot         ::= '.' field
opt_dot     ::= '?.' field
index       ::= '[' expr ']'
opt_index   ::= '?[' expr ']'
primary     ::= nil | false | true | int | float | string | template | list | map | var | paren
             | closure | spawn | chan | send | recv | select | match | struct_lit
closure     ::= '|' [id {',' id}] '|' expr
template    ::= string_with_${...}
field       ::= id | int | string
list        ::= '[' [ expr { ',' expr } [ ',' ] ] ']'
map         ::= '{' [ expr ':' expr { ',' expr ':' expr } [ ',' ] ] '}'
var         ::= identifier
paren       ::= '(' expr ')'
args        ::= [ expr { ',' expr } ]
struct_lit  ::= id '{' ( id ':' expr { ',' id ':' expr } )? '}'
```

Statements
```
program      ::= statement*
statement    ::= import_stmt | if_stmt | if_let_stmt | while_stmt | while_let_stmt
               | for_stmt | let_stmt | define_stmt | assign_stmt | compound_assign_stmt
               | return_stmt | break_stmt | continue_stmt | fn_stmt | struct_stmt | expr_stmt | block_stmt

import_stmt  ::= 'import' ( module | string | items_from_source | namespace_import | module_alias ) ';'
module       ::= identifier
string       ::= string_literal
items_from_source ::= '{' import_item { ',' import_item } '}' 'from' ( module | string )
import_item  ::= id [ 'as' id ]
namespace_import ::= '*' 'as' id 'from' ( module | string )
module_alias ::= module 'as' id

if_stmt      ::= 'if' ( '(' expr ')' | expr ) statement [ 'else' statement ]
if_let_stmt  ::= 'if' 'let' pattern '=' expr statement [ 'else' statement ]
while_stmt   ::= 'while' ( '(' expr ')' | expr ) statement
while_let_stmt ::= 'while' 'let' pattern '=' expr statement
for_stmt     ::= 'for' for_pattern 'in' expr statement

let_stmt     ::= 'let' pattern [ ':' type ] '=' expr ';'
define_stmt  ::= id ':' '=' expr ';'
assign_stmt  ::= id '=' expr ';'
compound_assign_stmt ::= id ( '+=' | '-=' | '*=' | '/=' | '%=' ) expr ';'
return_stmt  ::= 'return' [ expr ] ';'
break_stmt   ::= 'break' ';'
continue_stmt ::= 'continue' ';'
fn_stmt      ::= 'fn' id '(' [ param { ',' param } ] ')' [ '->' type ] block_stmt
struct_stmt  ::= 'struct' id '{' ( id [ ':' type ] { ',' id [ ':' type ] } )? '}'
param        ::= id [ ':' type ]
expr_stmt    ::= expr ';'
block_stmt   ::= '{' statement* '}'
```

Patterns
```
pattern      ::= literal | '_' | id | list_pat | map_pat | or_pat | guard_pat | range_pat
list_pat     ::= '[' pattern { ',' pattern } [ ',' '..' id ] ']'
map_pat      ::= '{' ( (string|id) ':' pattern ) { ',' (string|id) ':' pattern } [ ',' '..' id ] '}'
or_pat       ::= pattern '|' pattern { '|' pattern }
guard_pat    ::= pattern 'if' expr
range_pat    ::= literal ('..' | '..=') expr

for_pattern  ::= '_' | id | '(' for_pattern { ',' for_pattern } ')' | '[' for_pattern { ',' for_pattern } [ ',' '..' id ] ']'
               | '{' string ':' for_pattern { ',' string ':' for_pattern } '}'
```

## Notes for CLI usage
- Run REPL: `lkr`
- Execute a file (statements): `lkr FILE`
- Compile to bytecode: `lkr compile FILE` → `FILE.lkrb`
- Only relative, sanitized paths are allowed
- CLI prints a result only when it is not `nil`



### Types
- `String` - UTF-8 strings
- `Int` - 64-bit signed integers
- `Float` - 64-bit floating point
- `Bool` - Boolean values
- `Nil` - Null/undefined value
- `List` - Ordered collections
- `Map` - Key-value maps
- `Function` - First-class functions
