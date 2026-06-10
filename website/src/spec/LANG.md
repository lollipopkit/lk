# Language Overview

This document describes the LK language as implemented in this repository (parser, evaluator, statements, types, and standard library wiring).

### Comments
- Line comments: `// ...`
- Block comments: `/* ... */`
- Documentation comments for tooling: `/// ...` and `/** ... */` attach to the next `fn`, `struct`, `trait`, or `type` declaration when immediately adjacent. Top-of-file `//! ...` and `/*! ... */` document the package root for LSP hover. These comments do not change runtime semantics.

### Identifiers
- Consist of letters, digits, `_`, and `-`. Keywords are reserved. (Be mindful that `-` within identifiers is allowed by the lexer.)

### Literals
- String: `"..."` or `'...'` UTF-8 strings. Supports escapes `\n \r \t \\ \" \' \$ \0`.
- Raw string (Rust-style, no escapes/interpolation): `r"..."`, `r#"..."#`, `r##"..."##` (multi-line allowed).
- Int: 64-bit signed, supports leading sign and scientific notation for floats.
- Float: 64-bit floating point, supports scientific notation.
- Bool: `true`, `false`
- Nil: `nil`

### Collections
- List: `[a, b, c]` (heterogeneous allowed). Indexing: `list[0]`. Negative indexing: `list[-1]`. Slice with range: `list[1..3]`. Safe access helpers via stdlib/meta-methods.
- Map: `{ key: value, ... }`. Bare keys are string keys: `{name: "Alice", age: 30}` is equivalent to `{ "name": "Alice", "age": 30 }`. Keys are runtime key values (nil/bool/int/string/object; float is rejected); access with `map.key` or `map["key"]`.
- Set: `Set()` creates an empty set; `Set([items])` builds a set from a list. Set elements use the same key rules as Map.

### Template Strings
- Interpolation only with `${expr}` inside normal quotes (both `"..."` and `'...'`).
- Raw strings do not support interpolation.
- Escape `$` with `\$`: `"Price: \$100"`.
- `println` and `print` support `{}` format placeholders: `println("{} + {} = {}", a, b, a + b)`.
- Examples: `"Hello, ${user.name}!"`, `"Sum: ${1 + 2}"`.

### Input and Variables
- There is no implicit runtime context. Identifiers must be defined in the lexical environment (e.g., via `let` in statements, function params, or module uses).
- Read external input explicitly with stdlib: `std.read_to_string(std.stdin())` after `use { std } from io;`. Parse manually through `encoding`: `encoding.json.parse(...)`, `encoding.yaml.parse(...)`, `encoding.toml.parse(...)`.
- Example: `use { std } from io; use { json } from encoding; let data = json.parse(std.read_to_string(std.stdin())); return data.req.user.id == 1;`

### Constants
- `const name = expr;` - like `let` but immutable. Attempting to reassign a `const` variable is a runtime error.

### Function Calls and Methods
- Call any expression: `f(x, y)`, `(g)(z)`.
- Property access: `expr.field` or `expr[expr]`. Optional chaining: `expr?.field` and `expr?[index]`.
- Method sugar: `value.method(args...)` dispatches as:
  1) If `value.method` yields a callable (closure/native), call it.
  2) Else dispatch a registered meta-method for the value's runtime type, passing the receiver as the first argument (e.g., `"abc".len()`; see stdlib).

### Closures
- Expression form only: `|a, b| a + b`.
- Block form: `|x| { let y = x + 1; y }` - the last expression is the return value.
- Function-literal form: `fn(a: Int, b) => a + b`.
- Closures capture and can mutate variables from the enclosing scope.

### Ranges
- `a..b` and `a..=b` produce integer lists when evaluated (inclusive/exclusive end). Used in patterns as well.
- Explicit step: `a..b..step` - e.g., `0..10..2` produces `[0, 2, 4, 6, 8]`.

### Nullish Coalescing and Ternary
- `lhs ?? rhs` yields `lhs` unless it is `nil`, then `rhs`.
- `cond ? then : else` (right-associative). In expressions, `cond` must be Bool. In `if`/`while`, truthiness is used (see below).

### Bitwise Operators
- `a & b` - bitwise AND
- `a | b` - bitwise OR
- `~a` - bitwise NOT

### String and Collection Operators
- `+` supports String + String concatenation. Other string/number mixes are feature-gated and not enabled by default.
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
- Numeric auto-promotion: `Int + Float → Float`, `Int * Float → Float`.

## Expressions
- Literals, lists, maps, variables, calls, property/index access, closures, ranges, logical/comparison, `??`, and `?:`.
- Concurrency helpers (feature-gated `concurrency`) are regular function calls:
  - `spawn(fn_or_closure)` → Task
  - `chan(capacity?, type?)` → Channel (type is a string like `"Int"`)
  - `send(channel, value)` → Bool
  - `recv(channel)` → `[ok, value]`
- `select` is a dedicated expression over channel operations:
  - `select { case recv(ch) => expr; case value <- recv(ch) if guard => expr; case send(ch, value) => expr; default => expr }`

### Match Expression
- `match value { pattern => expr, ... }` (`,` or `;` separators allowed). Returns the chosen arm's value. Patterns below.

## Patterns
Used in `match`, `if let`, `while let`, and `let` destructuring.
- Literal: `1`, `3.14`, `"x"`, `true`, `nil`
- Variable binding: `name`
- Wildcard: `_`
- List destructuring: `[p1, p2, ..rest]`
- Map destructuring: `{ "key": pat, other: pat, ..rest }` (keys may be string literals or identifiers; rest binds remaining fields)
- Or-pattern: `p1 | p2 | p3`
- Guarded pattern: `pat if expr`
- Range pattern: `1..10`, `0..=n`

### For-loop Patterns
- Support an extended pattern set:
  - Variable: `x`
  - Ignore: `_`
  - Tuple (comma-separated): `for i, item in pairs { ... }` - destructures iterable pair items.
  - Array: `[a, b, ..rest]`
  - Object: `{ "k": v, ... }` (string keys)

## Statements
- Program is a sequence of statements. Semicolons `;` terminate simple statements and expression statements.

### Control Flow
- `if (cond) stmt` or `if cond stmt` (parentheses optional). Truthiness: `false` and `nil` are false; everything else (including `0`, `""`) is true.
- `if let pattern = expr stmt [else stmt]`
- `while (cond) stmt` or `while cond stmt`
- `while let pattern = expr stmt`
- `for pattern in expr stmt` where `expr` is iterable: List, String (chars), Map (iterates `[key, value]`), or Set (iterates values).
- `break;`, `continue;`
- `return;` or `return expr;`

### Variables
- Declaration/destructuring: `let pattern [: Type] = expr;`
- Constant declaration: `const name = expr;` - immutable binding, reassignment is a runtime error.
- Assignment: `name = expr;`
- Compound assignment: `name += expr;`, `-=`, `*=`, `/=`, `%=`
- Index assignment: `arr[i] = expr;`, `arr[i] += expr;`
- Dot assignment: `obj.field = expr;`, `obj.field += expr;`, `map.key = expr;`, `map.key += expr;`
- Short definition: `name := expr;` (define and initialize)
- Lexical scoping: blocks `{ ... }` introduce a new scope.

### Structs
- Define: `struct User { id: Int, name: String? }`
- Instantiate (literal): `User { id: 1, name: "Ann" }`
- Instantiate (call sugar): `User(id: 1, name: "Ann")`
- Access: `user.name`
- Update syntax: `User { ..existing, field: value }` or `User { ..existing }` - copies all fields from `existing`, overriding specified ones when fields are provided.

### Traits and Impl
- Trait definition: `trait Area { fn area(self) -> Int; }`
- Implementation: `impl Area for Rect { fn area(self) -> Int { return self.w * self.h; } }`
- Methods defined in `impl` blocks are dispatched when calling `value.method()` if no direct property/method matches.
- Auto-display: if a type implements `fn show(self) -> String` or `fn display(self) -> String` or `fn to_string(self) -> String`, `println("{}")` and template `${value}` automatically use it for formatting.

### Functions
- Definition: `fn name(param1[: Type], param2[: Type]) [-> Type] { statements }`
- Parameters and return type are optional; functions return `nil` by default unless `return` is used.
- First-class: closures and function values can be passed, returned, and called.
- Default positional parameters: `fn greet(name, greeting = "hello") { ... }` - parameters with defaults must come after all required positional parameters.
- Named parameters live in an optional trailing block and require type annotations: `fn f(a, b, { flag: Bool = true, label: String }) { ... }`. The block may also be the whole parameter list: `fn configure({host: String}) { ... }`.
- Defaults are lazily evaluated inside the callee when the argument is omitted; expressions can reference other parameters.
- Call sites supply named arguments with `name: expr`: `f(1, 2, label: "demo", flag: false)` or `f(label: "demo")`. Named arguments may appear in any order; once a named argument appears, positional arguments cannot follow it.

### Uses
- Forms:
  - `use math;` - stdlib module as a namespace
  - `use { file, std } from io;` - selected child namespaces from a parent stdlib module
  - `use io;` - parent stdlib module namespace, with child namespaces such as `io.file` and `io.std`
  - `use "path/to/file.lk";` - file module as a namespace (name is the file stem)
  - `use { abs, sqrt } from math;` - selected items
  - `use { f as g } from "m.lk";` - with alias
  - `use * as m from math;` - namespace alias
  - `use math as m;` - module alias
- Bare module uses bind the module name directly: `use net;` defines `net`.

- File use resolution and safety:
  - Files are not automatically visible to each other. Use every cross-file dependency explicitly.
  - Quoted file uses do not require `Lk.toml`; they are resolved from the current file's directory.
  - Paths are relative-only and sanitized: absolute paths and any `..` components are rejected.
  - Resolution attempts, in order: `${MOD_NAME}.lk`, then `${MOD_NAME}/mod.lk` (relative to the current file directory).
  - If you pass a quoted path with `.lk` already (e.g., `"lib/foo.lk"`), it must be relative and will be used directly if it exists.
  - In a package, bare module uses first check stdlib modules, then `Lk.toml` workspace/dependency packages. Package uses resolve to `src/mod.lk` or `src/<package-name>.lk`.
  - Because `..` is rejected, code in a nested directory cannot use a parent-directory file with `../...`; use a package/workspace module when nested code must depend on code outside its subtree.

#### File Use Example

```text
a.lk
b.lk
c/c1.lk
c/d/d1.lk
```

From `a.lk`:

```lk
use "b";       // b.lk, available as b
use "c/c1";    // c/c1.lk, available as c1
use "c/d/d1";  // c/d/d1.lk, available as d1
```

From `c/c1.lk`:

```lk
use "d/d1";    // c/d/d1.lk, available as d1
// use "../a"; // rejected: parent-directory uses are not allowed
```

## Packages
- `Lk.toml` defines `[package]`, `[dependencies]`, `[workspace]`, and `[workspace.dependencies]`.
- String dependencies default to GitHub, e.g. `util = "owner/repo"`.
- `Lk.lock` stores fetched git sources at concrete revisions.
- See `docs/packages.md` for package manager commands and manifest examples. The runnable workspace example lives in `examples/lk-example-workspace`.

## Builtins and Stdlib
- Builtin globals: `print(fmt, ...args)`, `println(fmt, ...args)`, `panic([msg])`, `assert(cond[, msg])`, `assert_eq(actual, expected[, msg])`, `assert_ne(actual, expected[, msg])`, `typeof(value)`.
- `typeof(value)` returns the runtime type name as a string: `"Int"`, `"Float"`, `"String"`, `"Bytes"`, `"Bool"`, `"Nil"`, `"List"`, `"Map"`, `"Set"`, `"Slice"`, resource names such as `"File"`/`"TcpStream"`, or the struct type name.

### Stdlib Modules
Use as needed: `math`, `string`, `bytes`, `iter`, `stream`, `datetime`, `os`, `fs`, `path`, `env`, `process`, `io`, `net`, `slice`, `encoding`, `hash`, `regex`, `random`, `uuid`, `http`. With `concurrency` feature: `task`, `chan`, `time`.

- `math`: constants `pi`, `e`, `inf`, `nan`, `max_int`, `min_int`, `max_float`, `epsilon`; functions `abs`, `sqrt`, `floor`, `ceil`, `round`, `min`, `max`, `pow`, `exp`, `sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `atan2`, `log`, `log10`, `log2`, `clamp`, `random`, `hypot`, `cbrt`, `sinh`, `cosh`, `tanh`, `trunc`, `fract`, `sign`, `to_int`, `to_float`, `is_nan`, `is_inf`.
- `string`: methods (see meta-methods below).
- `bytes`: binary data backed by bytes. `from_list(list)`, `from_string(str)`, `len(bytes)`, `is_empty(bytes)`, `get(bytes, index)`, `slice(bytes, start[, end])`, `to_list(bytes)`, `to_string_utf8(bytes)`, `to_string_lossy(bytes)`, `concat(a, b)`, `eq(a, b)`.
- `iter`: module-level list utilities only: `range([start,] end [, step])`, `enumerate(list)`, `zip(list1, list2)`, `take(list, n)`, `skip(list, n)`, `chain(list1, list2)`, `flatten(list)`, `unique(list)`, `chunk(list, size)`, and higher-order ops `map(list, fn)`, `filter(list, fn)`, `reduce(list, init, fn)`.
- `stream`: module-level lazy pipelines. `stream.from_list(list)`, `stream.range(start, end)`, `stream.iterate(seed, fn)`, `stream.repeat(val)`, `stream.from_channel(ch)`, `stream.map(s, fn)`, `stream.filter(s, fn)`, `stream.take(s, n)`, `stream.skip(s, n)`, `stream.chain(a, b)`, `stream.subscribe(s)`, `stream.next(cursor)`, `stream.collect(stream_or_cursor)`, `stream.next_block(cursor[, timeout_ms])`, `stream.collect_block(stream_or_cursor[, n][, timeout_ms])`.
- `datetime`: `now()` (microseconds), `format(secs, fmt)`, `parse(str, fmt)`, `add(secs, delta)`, `sub(secs, delta)`, `day_of_week(secs)`, `day_of_year(secs)`, `is_weekend(secs)`.
- `os`: platform/time helpers `hostname()`, `arch()`, `os()`, `clock()`, `time()`, `epoch()`.
- `fs`: path-level filesystem APIs. `read(path) -> Bytes`, `read_to_string(path)`, `write(path, data)`, `append(path, data)`, `exists(path)`, `is_file(path)`, `is_dir(path)`, `metadata(path)`, `read_dir(path)`, `create_dir(path)`, `create_dir_all(path)`, `remove_file(path)`, `remove_dir(path)`, `remove_dir_all(path)`, `rename(from, to)`, `copy(from, to)`, `canonicalize(path)`, `temp_dir()`.
- `path`: `join(parts...)`, `parent(path)`, `file_name(path)`, `file_stem(path)`, `extension(path)`, `with_extension(path, ext)`, `is_absolute(path)`, `normalize(path)`, `components(path)`, `sep()`, `delimiter()`.
- `env`: `get(key)`, `get_or(key, default)`, `has(key)`, `vars()`. Mutating process environment is intentionally not exposed.
- `process`: `id()`, `cwd()`, `set_cwd(path)`, `exit(code)`, `status(cmd[, args])`, `output(cmd[, args]) -> {status, success, stdout: Bytes, stderr: Bytes}`, `output_string(cmd[, args])`.
- `io`: parent namespace. Import children with `use { std, file } from io;` or access them through `io.std` and `io.file`.
- `io.std`: `stdin()`, `stdout()`, `stderr()`, `read(reader[, max_bytes]) -> Bytes`, `read_to_string(reader)`, `read_line(reader)`, `write(writer, data)`, `writeln(writer, data)`, `flush(writer)`. `write`/`writeln` accept `Bytes` or `String`.
- `io.file`: resource-level file APIs. `open(path, mode)`, `create(path)`, `read(file[, max_bytes]) -> Bytes`, `read_to_string(file)`, `read_line(file)`, `write(file, data)`, `writeln(file, data)`, `write_all(file, data)`, `flush(file)`, `close(file)`. Path-level operations live in `fs`.
- `slice`: `from_list(list)`, `from_string(str)`, `len(slice)`, `is_empty(slice)`, `get(slice, index)`, `sub(slice, start[, end])`, `to_list(slice)`, `to_string(slice)`.
- `encoding`: parent namespace. Import children with `use { json, yaml, toml, base64, hex, url } from encoding;` or access them through `encoding.json`, `encoding.base64`, etc. `json.parse(string)`, `yaml.parse(string)`, `toml.parse(string)`, `base64.encode(data)`, `base64.decode(string) -> Bytes`, `hex.encode(data)`, `hex.decode(string) -> Bytes`, `url.encode_component(string)`, `url.decode_component(string)`, `url.query_parse(string)`, `url.query_stringify(map)`.
- `hash`: `sha256(data)`, `sha1(data)`, `crc32(data)`, `fnv64(data)`. `data` accepts `Bytes` or `String`.
- `regex`: `is_match(pattern, text)`, `find(pattern, text)`, `find_all(pattern, text)`, `captures(pattern, text)`, `replace(pattern, text, replacement)`, `split(pattern, text)`.
- `random`: `int(min, max)`, `float()`, `bool([probability])`, `bytes(len)`, `choice(list)`, `shuffle(list)`.
- `uuid`: `v4()`, `parse(string)`, `is_valid(string)`.
- `http`: sync client APIs `request(method, url[, opts])`, `get(url[, opts])`, `post(url, body[, opts])`; responses are maps with `status`, `headers`, and `body: Bytes`.
- `net`: parent namespace. Import children with `use { socket, tcp, udp } from net;` or access them through `net.socket`, `net.tcp`, and `net.udp`.
- `net.socket`: `addr(host, port)`, `close(resource)`.
- `net.tcp`: `connect(addr)`, `bind(addr)`, `accept(listener)`, `write(stream, data)`, `read(stream, len?) -> Bytes`, `close(resource)`, plus `connect_task`, `accept_task`, `read_task`, `write_task`. `write` accepts `Bytes` or `String`.
- `net.udp`: `bind(addr)`, `recv_from(socket, len?) -> {data: Bytes, addr: String}`, `send_to(socket, data, addr)`, plus `recv_from_task`, `send_to_task`. `send_to` accepts `Bytes` or `String`.
- `time` (concurrency): `time.now()`, `time.sleep(ms)`, `time.timeout(ms)`, `time.after(ms)`, `time.since(start, end)`.

### Meta-methods (usable as `value.method()` without importing)
- String: `len`, `lower`, `upper`, `trim`, `starts_with`, `ends_with`, `contains`, `replace`, `substring`, `split`, `join`, `reverse`, `repeat`, `chars`, `char_at`, `byte_at`, `find`, `is_empty`, `format`
- List: `len`, `push`, `set`, `concat`, `join`, `get`, `first`, `last`, `map`, `filter`, `reduce`, `take`, `skip`, `chain`, `flatten`, `unique`, `chunk`, `enumerate`, `zip`, `to_stream`, `sort`, `reverse`, `pop`, `insert`, `remove_at`, `contains`, `index_of`, `slice`, `is_empty`
- Map: `len`, `is_empty`, `keys`, `values`, `has`, `get`, `set`, `delete`, `clear`
- Set: `len`, `is_empty`, `has`, `contains`, `add`, `delete`, `remove`, `values`, `clear`
- Stream: `map`, `filter`, `take`, `skip`, `chain`, `subscribe`, `collect`, `collect_block`
- StreamCursor: `next`, `collect`, `next_block`, `collect_block`
- Channel: `to_stream`

### Indexed Access and Slicing
- Lists and strings support integer indexing with negative indices: `xs[-1]` gets the last element.
- Lists and strings support range slicing: `xs[1..3]`, `s[1..3]`.
- Map dot assignment and compound assignment: `m.key = val`, `m.count += 2`, `p.x += 9`.
- List index assignment and compound assignment: `arr[1] = 10`, `arr[1] += 5`.

### List Spread
- Spread an existing list into a new list: `[0, ..spread_a, 3]` - inserts all elements of `spread_a`.

## CLI Output
- REPL and CLI print evaluation results only when the value is not `nil`. This avoids extra lines after statements that return `nil` by default (e.g., `let`, `fn` definitions, `println(...)`). If you need to display `nil`, print it explicitly via `println(nil)` or include it in formatted output.

## Types and Annotations
### Primitive and Composite Types
- `Int`, `Float`, `String`, `Bool`, `Nil`, `Any`
- `List<T>`, `Map<K, V>`, `Set<T>`
- `Task<T>`, `Channel<T>` (concurrency)
- Function types: `(T1, T2) -> R`
- Union: `A | B | Nil`; Optional: `T?` (sugar for `T | Nil`)
- Named and generic types are parsed (e.g., `List<Int>`, `Map<String, Int>`, `Set<String>`)

### Annotations
- `let x: Int = 1;`
- `fn f(a: Int, b: String) -> Bool { ... }`
- Type checking/inference is best-effort and conservative; runtime remains dynamic.

## Grammar (EBNF-style)

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
             | closure | select | match | struct_lit
closure     ::= '|' [id {',' id}] '|' expr
             | '|' [id {',' id}] '|' '{' statement* '}'
             | 'fn' '(' [ param { ',' param } ] ')' '=>' expr
select      ::= 'select' '{' { select_case | default_case | ';' } '}'
select_case ::= 'case' select_pattern [ 'if' expr ] '=>' expr [ ';' ]
default_case ::= 'default' '=>' expr [ ';' ]
select_pattern ::= [ (id | '_') '<-' ] 'recv' '(' expr ')'
                 | 'send' '(' expr ',' expr ')'
template    ::= string_with_${...}
field       ::= id | int | string
list        ::= '[' [ (expr | '..' expr) { ',' (expr | '..' expr) } [ ',' ] ] ']'
map         ::= '{' [ map_key ':' expr { ',' map_key ':' expr } [ ',' ] ] '}'
map_key     ::= id | expr
var         ::= identifier
paren       ::= '(' expr ')'
args        ::= [ positional_args [ ',' named_args ] | named_args ]
positional_args ::= expr { ',' expr }
named_args  ::= name ':' expr { ',' name ':' expr }
struct_lit  ::= id '{' [ struct_update [ ',' struct_field_list [ ',' ] ]
                       | struct_field_list [ ',' ] ] '}'
struct_update ::= '..' expr
struct_field_list ::= struct_field { ',' struct_field }
struct_field ::= id ':' expr
```

### Statements
```
program      ::= statement*
statement    ::= import_stmt | if_stmt | if_let_stmt | while_stmt | while_let_stmt
               | for_stmt | let_stmt | const_stmt | define_stmt | assign_stmt | compound_assign_stmt
               | index_assign_stmt | dot_assign_stmt | return_stmt | break_stmt | continue_stmt
               | fn_stmt | struct_stmt | trait_stmt | impl_stmt | expr_stmt | block_stmt

import_stmt  ::= 'use' ( module | string | items_from_source | namespace_import | module_alias ) ';'
module       ::= identifier { '/' identifier }
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
fn_stmt      ::= 'fn' id '(' [ fn_param_list ] ')' [ '->' type ] block_stmt
fn_param_list ::= positional_param { ',' positional_param } [ ',' named_param_block ]
                | named_param_block
positional_param ::= id [ ':' type ] [ '=' expr ]
named_param_block ::= '{' [ named_param { ',' named_param } [ ',' ] ] '}'
param        ::= id [ ':' type ]
named_param  ::= id ':' type [ '=' expr ]
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
- Run REPL: `lk` (`LK_REPL_TUI=always|never|auto` controls whether the Reedline completion UI is forced, disabled, or terminal-detected)
- Execute a file (statements) through the bytecode VM: `lk FILE`
- Compile to an executable module artifact: `lk compile [FILE]` -> `FILE.lkm`
- Execute an module artifact: `lk FILE.lkm`
- Only relative, sanitized paths are allowed
- CLI prints a result only when it is not `nil`

## Runtime Value Types
- `String` - UTF-8 strings
- `Int` - 64-bit signed integers
- `Float` - 64-bit floating point
- `Bool` - Boolean values
- `Nil` - Null/undefined value
- `List` - Ordered collections
- `Map` - Key-value maps
- `Set` - Unique value collections
- `Function` - First-class functions
- `Object` - Struct instances (with type name and fields)
- `Task` - Concurrency task handle (feature-gated)
- `Channel` - Concurrency channel (feature-gated)
- `Stream` - Lazy stream pipeline (feature-gated)
- `StreamCursor` - Stream cursor for consuming stream elements
