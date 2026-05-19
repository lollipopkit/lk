# Language Overview

This document describes the LK language as implemented in this repository (parser, evaluator, statements, types, and standard library wiring).

### Comments
- Line comments: `// ...`
- Block comments: `/* ... */`

### Identifiers
- Consist of letters, digits, `_`, and `-`. Keywords are reserved. (Be mindful that `-` within identifiers is allowed by the lexer.)

### Literals
- String: `"..."` or `'...'` UTF‑8 strings. Supports escapes `\n \r \t \\ \" \' \$ \0`.
- Raw string (Rust‑style, no escapes/interpolation): `r"..."`, `r#"..."#`, `r##"..."##` (multi‑line allowed).
- Int: 64‑bit signed, supports leading sign and scientific notation for floats.
- Float: 64‑bit floating point, supports scientific notation.
- Bool: `true`, `false`
- Nil: `nil`

### Collections
- List: `[a, b, c]` (heterogeneous allowed). Indexing: `list[0]`. Negative indexing: `list[-1]`. Slice with range: `list[1..3]`. Safe access helpers via stdlib/meta‑methods.
- Map: `{ key: value, ... }`. Bare keys are string keys: `{name: "Alice", age: 30}` is equivalent to `{ "name": "Alice", "age": 30 }`. Keys are evaluated expressions and coerced to strings at runtime (string/int/float/bool); access with `map.key` or `map["key"]`.

### Template Strings
- Interpolation only with `${expr}` inside normal quotes (both `"..."` and `'...'`).
- Raw strings do not support interpolation.
- Escape `$` with `\$`: `"Price: \$100"`.
- `println` and `print` support `{}` format placeholders: `println("{} + {} = {}", a, b, a + b)`.
- Examples: `"Hello, ${user.name}!"`, `"Sum: ${1 + 2}"`.

### Input and Variables
- There is no implicit runtime context. Identifiers must be defined in the lexical environment (e.g., via `let` in statements, function params, or imports).
- Read external input explicitly with stdlib: `io.read()` (string). Parse manually: `json.parse(...)`, `yaml.parse(...)`, `toml.parse(...)`.
- Example: `import io; import json; let data = json.parse(io.read()); return data.req.user.id == 1;`

### Constants
- `const name = expr;` — like `let` but immutable. Attempting to reassign a `const` variable is a runtime error.

### Function Calls and Methods
- Call any expression: `f(x, y)`, `(g)(z)`.
- Property access: `expr.field` or `expr[expr]`. Optional chaining: `expr?.field` and `expr?[index]`.
- Method sugar: `value.method(args...)` dispatches as:
  1) If `value.method` yields a callable (closure/native), call it.
  2) Else dispatch a registered meta‑method for the value's runtime type, passing the receiver as the first argument (e.g., `"abc".len()`; see stdlib).

### Closures
- Expression form only: `|a, b| a + b`.
- Block form: `|x| { let y = x + 1; y }` — the last expression is the return value.
- Closures capture and can mutate variables from the enclosing scope.

### Ranges
- `a..b` and `a..=b` produce integer lists when evaluated (inclusive/exclusive end). Used in patterns as well.
- Explicit step: `a..b..step` — e.g., `0..10..2` produces `[0, 2, 4, 6, 8]`.

### Nullish Coalescing and Ternary
- `lhs ?? rhs` yields `lhs` unless it is `nil`, then `rhs`.
- `cond ? then : else` (right‑associative). In expressions, `cond` must be Bool. In `if`/`while`, truthiness is used (see below).

### Bitwise Operators
- `a & b` — bitwise AND
- `a | b` — bitwise OR
- `~a` — bitwise NOT

### String and Collection Operators
- `+` supports String + String concatenation. Other string/number mixes are feature‑gated and not enabled by default.
- `"ha" * 3` produces `"hahaha"` (String × Int repetition). `Int × String` also works.
- `-` between lists returns a new list with elements of the right list removed.
- `+` between lists concatenates. `+` between a list and a value appends.
- `+` between maps merges them (right-side wins on key overlap).
- `in` supports: substring `str in str`, element membership in lists, and key existence in maps. For `list in list`, it checks all elements of the left are contained in the right.

## Operators (by precedence)
- Postfix: call `()`, dot `.field`, index `[expr]`, optional `?.field`, optional `?[expr]`
- Unary: `!` (logical not), `~` (bitwise not)
- Multiplicative: `* / %`
- Additive: `+ -`
- Range: `.. ..=` (and `..step` variant)
- Comparison/membership: `== != < > <= >= in`
- Bitwise AND: `&`
- Bitwise OR: `|`
- Logical: `&& ||`
- Nullish coalescing: `??`
- Ternary: `? :` (lowest among expression operators)

### Notes
- Division: `Int / Int` returns `Int` when evenly divisible, `Float` otherwise. `math.pow(2, 10)` returns `Float`.
- Numeric auto‑promotion: `Int + Float → Float`, `Int * Float → Float`.

## Expressions
- Literals, lists, maps, variables, calls, property/index access, closures, ranges, logical/comparison, `??`, and `?:`.
- Concurrency expressions (feature‑gated `concurrency`):
  - `spawn(fn_or_closure)` → Task
  - `chan(capacity?, type?)` → Channel (type is a string like `"Int"`)
  - `send(channel, value)` → Bool
  - `recv(channel)` → `[ok, value]`
  - `select { case recv(c) => expr; case send(c, v) => expr; default => expr }`

### Match Expression
- `match value { pattern => expr, ... }` (`,` or `;` separators allowed). Returns the chosen arm's value. Patterns below.

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

### For‑loop Patterns
- Support an extended pattern set:
  - Variable: `x`
  - Ignore: `_`
  - Tuple (comma‑separated): `for i, item in pairs { ... }` — destructures iterable pair items.
  - Array: `[a, b, ..rest]`
  - Object: `{ "k": v, ... }` (string keys)

## Statements
- Program is a sequence of statements. Semicolons `;` terminate simple statements and expression statements.

### Control Flow
- `if (cond) stmt` or `if cond stmt` (parentheses optional). Truthiness: `false` and `nil` are false; everything else (including `0`, `""`) is true.
- `if let pattern = expr stmt [else stmt]`
- `while (cond) stmt` or `while cond stmt`
- `while let pattern = expr stmt`
- `for pattern in expr stmt` where `expr` is iterable: List, String (chars), or Map (iterates `[key, value]`).
- `break;`, `continue;`
- `return;` or `return expr;`

### Variables
- Declaration/destructuring: `let pattern [: Type] = expr;`
- Constant declaration: `const name = expr;` — immutable binding, reassignment is a runtime error.
- Assignment: `name = expr;`
- Compound assignment: `name += expr;`, `-=`, `*=`, `/=`, `%=`
- Index assignment: `arr[i] = expr;`, `arr[i] += expr;`
- Dot assignment: `obj.field = expr;`, `obj.field += expr;`, `map.key = expr;`, `map.key += expr;`
- Short definition: `name := expr;` (define and initialize)
- Lexical scoping: blocks `{ ... }` introduce a new scope.

### Structs
- Define: `struct User { id: Int, name: String? }`
- Instantiate (literal): `User { id: 1, name: "Ann" }`
-Instantiate (call sugar): `User(id: 1, name: "Ann")`
- Access: `user.name`
- Update syntax: `User { ..existing, field: value }` — copies all fields from `existing`, overriding specified ones.

### Traits and Impl
- Trait definition: `trait Area { fn area(self) -> Int; }`
- Implementation: `impl Area for Rect { fn area(self) -> Int { return self.w * self.h; } }`
- Methods defined in `impl` blocks are dispatched when calling `value.method()` if no direct property/method matches.
- Auto‑display: if a type implements `fn show(self) -> String` or `fn display(self) -> String` or `fn to_string(self) -> String`, `println("{}")` and template `${value}` automatically use it for formatting.

### Functions
- Definition: `fn name(param1[: Type], param2[: Type]) [-> Type] { statements }`
- Parameters and return type are optional; functions return `nil` by default unless `return` is used.
- First‑class: closures and function values can be passed, returned, and called.
- Default positional parameters: `fn greet(name, greeting = "hello") { ... }` — parameters with defaults must come after all required positional parameters.
- Named parameters live in an optional trailing block: `fn f(a, b, { flag: Bool = true, label: String }) { ... }`.
- Defaults are lazily evaluated inside the callee when the argument is omitted; expressions can reference other parameters.
- Call sites supply named arguments with `name: expr` after the positional tail: `f(1, 2, label: "demo", flag: false)`. Named arguments may appear in any order but must follow all positional ones.

### Imports
- Forms:
  - `import math;` — stdlib module as a namespace
  - `import "path/to/file.lk";` — file module as a namespace (name is the file stem)
  - `import { abs, sqrt } from math;` — selected items
  - `import { f as g } from "m.lk";` — with alias
  - `import * as m from math;` — namespace alias
  - `import math as m;` — module alias

- File import resolution and safety:
  - Files are not automatically visible to each other. Import every cross-file dependency explicitly.
  - Quoted file imports do not require `Lk.toml`; they are resolved from the importing file's directory.
  - Paths are relative-only and sanitized: absolute paths and any `..` components are rejected.
  - Resolution attempts, in order: `${MOD_NAME}.lk`, then `${MOD_NAME}/mod.lk` (relative to the current file directory).
  - If you pass a quoted path with `.lk` already (e.g., `"lib/foo.lk"`), it must be relative and will be used directly if it exists.
  - In a package, bare module imports first check stdlib modules, then `Lk.toml` workspace/dependency packages. Package imports resolve to `src/mod.lk` or `src/<package-name>.lk`.
  - Because `..` is rejected, code in a nested directory cannot import a parent-directory file with `../...`; use a package/workspace module when nested code must depend on code outside its subtree.

#### File Import Example

```text
a.lk
b.lk
c/c1.lk
c/d/d1.lk
```

From `a.lk`:

```lk
import "b";       // b.lk, available as b
import "c/c1";    // c/c1.lk, available as c1
import "c/d/d1";  // c/d/d1.lk, available as d1
```

From `c/c1.lk`:

```lk
import "d/d1";    // c/d/d1.lk, available as d1
// import "../a"; // rejected: parent-directory imports are not allowed
```

## Packages
- `Lk.toml` defines `[package]`, `[dependencies]`, `[workspace]`, and `[workspace.dependencies]`.
- String dependencies default to GitHub, e.g. `util = "owner/repo"`.
- `Lk.lock` stores fetched git sources at concrete revisions.
- See `docs/packages.md` for package manager commands and manifest examples. The runnable workspace example lives in `examples/lk-example-workspace`.

## Builtins and Stdlib
- Builtin globals: `print(fmt, ...args)`, `println(fmt, ...args)`, `panic([msg])`, `typeof(value)`.
- `typeof(value)` returns the runtime type name as a string: `"Int"`, `"Float"`, `"String"`, `"Bool"`, `"Nil"`, `"List"`, `"Map"`, or the struct type name.

### Stdlib Modules
Import as needed: `math`, `string`, `list`, `map`, `iter`, `stream`, `datetime`, `os`, `io`, `json`, `yaml`, `toml`, `tcp`. With `concurrency` feature: `task`, `chan`, `time`.

- `math`: constants `pi`, `e`; functions `abs`, `sqrt`, `floor`, `ceil`, `round`, `min`, `max`, `pow`, `exp`, `sin`, `cos`.
- `string`: methods (see meta‑methods below).
- `list`: methods (see meta‑methods below).
- `map`: methods (see meta‑methods below), plus `map.set(m, key, val)` (returns updated map), `map.delete(m, key)` (returns `[updated_map, removed_value]`), `map.mutate(m, |guard| ...)` (batch mutations with a guard).
- `iter`: `range([start,] end [, step])`, `enumerate(list)`, `zip(list1, list2)`, `take(list, n)`, `skip(list, n)`, `chain(list1, list2)`, `flatten(list)`, `unique(list)`, `chunk(list, size)`, and generic higher-order ops `map(list, fn)`, `filter(list, fn)`, `reduce(list, init, fn)`.
- `stream`: lazy pipelines. `stream.from_list(list)`, `stream.range(start, end)`, `stream.iterate(seed, fn)`, `stream.repeat(val)`, `stream.from_channel(ch)`. Methods: `.map(fn)`, `.filter(fn)`, `.take(n)`, `.skip(n)`, `.chain(other)`, `.subscribe()`, `.collect()`, `.collect_block()`.
- `datetime`: `now()` (microseconds), `format(secs, fmt)`, `add(secs, delta)`, `sub(secs, delta)`, `day_of_week(secs)`, `day_of_year(secs)`, `is_weekend(secs)`.
- `os`: `hostname()`, `arch()`, `os()`, `clock()`, `epoch()`, `env.get(key)`, `env.set(key, val)`, `env.unset(key)`, `dir.current()`, `dir.temp()`, `dir.list(path)`.
- `io`: `io.read()` (stdin), `io.stdout.write(s)`, `io.stdout.writeln(s)`, `io.stdout.flush()`, `io.stderr.write(s)`.
- `json`: `json.parse(string)`.
- `yaml`: `yaml.parse(string)`.
- `toml`: `toml.parse(string)`.
- `tcp`: `tcp.connect(host, port)`, `tcp.write(conn, data)`, `tcp.read(conn, len)`, `tcp.close(conn)`.
- `time` (concurrency): `time.now()`, `time.sleep(ms)`, `time.since(start, end)`.

### Meta‑methods (usable as `value.method()` without importing)
- String: `len`, `lower`, `upper`, `trim`, `starts_with`, `ends_with`, `contains`, `replace`, `substring`, `split`, `join`, `reverse`, `repeat`, `chars`, `char_at`, `byte_at`, `find`, `is_empty`
- List: `len`, `push`, `set`, `concat`, `join`, `get`, `first`, `last`, `map`, `filter`, `reduce`, `take`, `skip`, `chain`, `flatten`, `unique`, `chunk`, `enumerate`, `zip`, `to_stream`, `mutate` (guard: `push`, `pop`, `replace`, `remove`, `reserve`, `commit`, `as_list`)
- Map: `len`, `keys`, `values`, `has`, `get`, `set`, `delete`, `mutate` (guard: `len`, `has`/`contains`, `set`/`insert`, `delete`/`remove`, `commit`, `as_map`)
- Iterator: `map`, `filter`, `reduce`, `next`, `collect`
- Stream: `map`, `filter`, `take`, `skip`, `chain`, `subscribe`, `collect`, `collect_block`
- StreamCursor: `next`, `collect`, `next_block`, `collect_block`
- Channel: `to_stream`

### Indexed Access and Slicing
- Lists and strings support integer indexing with negative indices: `xs[-1]` gets the last element.
- Lists and strings support range slicing: `xs[1..3]`, `s[1..3]`.
- Map dot assignment and compound assignment: `m.key = val`, `m.count += 2`, `p.x += 9`.
- List index assignment and compound assignment: `arr[1] = 10`, `arr[1] += 5`.

### List Spread
- Spread an existing list into a new list: `[0, ..spread_a, 3]` — inserts all elements of `spread_a`.

## CLI Output
- REPL and CLI print evaluation results only when the value is not `nil`. This avoids extra lines after statements that return `nil` by default (e.g., `let`, `fn` definitions, `println(...)`). If you need to display `nil`, print it explicitly via `println(nil)` or include it in formatted output.

## Types and Annotations
### Primitive and Composite Types
- `Int`, `Float`, `String`, `Bool`, `Nil`, `Any`
- `List<T>`, `Map<K, V>`
- `Task<T>`, `Channel<T>` (concurrency)
- Function types: `(T1, T2) -> R`
- Union: `A | B | Nil`; Optional: `T?` (sugar for `T | Nil`; prefix form `?T` is accepted for compatibility)
- Named and generic types are parsed (e.g., `List<Int>`, `Map<String, Int>`)

### Annotations
- `let x: Int = 1;`
- `fn f(a: Int, b: String) -> Bool { ... }`
- Type checking/inference is best‑effort and conservative; runtime remains dynamic.

## Grammar (EBNF‑style)

### Expressions (precedence from low to high)
```
expr        ::= conditional
conditional ::= nullish [ '?' expr ':' expr ]
nullish    ::= or { '??' or }
or          ::= and { '||' and }
and         ::= bit_or { '&&' bit_or }
bit_or      ::= bit_and { '|' bit_and }
bit_and     ::= cmp { '&' cmp }
cmp         ::= range { ('==' | '!=' | '<' | '>' | '<=' | '>=' | 'in') range }
range       ::= addsub [ ('..' | '..=') addsub? [ '..' addsub ] ]
addsub      ::= muldiv { ('+' | '-') muldiv }
muldiv      ::= unary { ('*' | '/' | '%') unary }
unary       ::= { '!' | '~' } postfix
postfix     ::= primary { call | dot | opt_dot | opt_index | index }
call        ::= '(' args ')'
dot         ::= '.' field
opt_dot     ::= '?.' field
index       ::= '[' expr ']'
opt_index   ::= '?[' expr ']'
primary     ::= nil | false | true | int | float | string | template | list | map | var | paren
             | closure | spawn | chan | send | recv | select | match | struct_lit
closure     ::= '|' [id {',' id}] '|' expr
             | '|' [id {',' id}] '|' '{' statement* '}'
template    ::= string_with_${...}
field       ::= id | int | string
list        ::= '[' [ (expr | '..' expr) { ',' (expr | '..' expr) } [ ',' ] ] ']'
map         ::= '{' [ (id | string) ':' expr { ',' (id | string) ':' expr } [ ',' ] ] '}'
var         ::= identifier
paren       ::= '(' expr ')'
args        ::= [ expr { ',' expr } [ ',' name ':' expr { ',' name ':' expr } ] ]
struct_lit  ::= id '{' [ '..' expr ',' ] id ':' expr { ',' id ':' expr } '}'
             | id '{' '}'
```

### Statements
```
program      ::= statement*
statement    ::= import_stmt | if_stmt | if_let_stmt | while_stmt | while_let_stmt
               | for_stmt | let_stmt | const_stmt | define_stmt | assign_stmt | compound_assign_stmt
               | index_assign_stmt | dot_assign_stmt | return_stmt | break_stmt | continue_stmt
               | fn_stmt | struct_stmt | trait_stmt | impl_stmt | expr_stmt | block_stmt

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
for_stmt     ::= 'for' for_pattern [ ',' for_pattern ]* 'in' expr statement

let_stmt     ::= 'let' pattern [ ':' type ] '=' expr ';'
const_stmt   ::= 'const' identifier '=' expr ';'
define_stmt  ::= id ':' '=' expr ';'
assign_stmt  ::= id '=' expr ';'
compound_assign_stmt ::= id ( '+=' | '-=' | '*=' | '/=' | '%=' ) expr ';'
index_assign_stmt ::= expr '[' expr ']' ( '=' | '+=' | '-=' | '*=' | '/=' | '%=' ) expr ';'
dot_assign_stmt    ::= expr '.' id ( '=' | '+=' | '-=' | '*=' | '/=' | '%=' ) expr ';'
return_stmt  ::= 'return' [ expr ] ';'
break_stmt   ::= 'break' ';'
continue_stmt ::= 'continue' ';'
fn_stmt      ::= 'fn' id '(' [ param { ',' param } ] [ ',' '{' named_param { ',' named_param } '}' ] ')' [ '->' type ] block_stmt
             | 'fn' id '(' [ param { ',' param } [ '=' expr ] { ',' param [ '=' expr ] } ] ')' [ '->' type ] block_stmt
param        ::= id [ ':' type ]
named_param  ::= id [ ':' type ] [ '=' expr ]
struct_stmt  ::= 'struct' id '{' ( id [ ':' type ] { ',' id [ ':' type ] } )? '}'
trait_stmt   ::= 'trait' id '{' fn_sig { ',' fn_sig } '}'
impl_stmt    ::= 'impl' id 'for' type '{' fn_stmt { fn_stmt } '}'
fn_sig       ::= 'fn' id '(' [ param { ',' param } ] ')' [ '->' type ] ';'
expr_stmt    ::= expr ';'
block_stmt   ::= '{' statement* '}'
```

### Patterns
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

## Notes for CLI Usage
- Run REPL: `lk`
- Execute a file (statements): `lk FILE`
- Compile to bytecode: `lk compile [FILE]` → `FILE.lkb`; when `FILE` is omitted, the CLI uses `./main.lk`, package `./src/main.lk`, or a single workspace app entry.
- Only relative, sanitized paths are allowed
- CLI prints a result only when it is not `nil`

## Runtime Value Types
- `String` — UTF‑8 strings
- `Int` — 64‑bit signed integers
- `Float` — 64‑bit floating point
- `Bool` — Boolean values
- `Nil` — Null/undefined value
- `List` — Ordered collections
- `Map` — Key‑value maps
- `Function` — First‑class functions
- `Object` — Struct instances (with type name and fields)
- `Task` — Concurrency task handle (feature‑gated)
- `Channel` — Concurrency channel (feature‑gated)
- `Stream` — Lazy stream pipeline (feature‑gated)
- `StreamCursor` — Stream cursor for consuming stream elements
- `Iterator` — Iterator state
- `MutationGuard` — Guard for batch mutations (lists and maps)