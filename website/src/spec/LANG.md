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

### Attributes
- Item declarations can carry preserved Rust-style attributes: `#[derive(Debug)] struct User { id: Int }` or `#[inline] fn answer() { return 42; }`.
- Attributes currently attach to item declarations (`fn`, `struct`, `type`, `trait`, `impl`) and to methods inside `impl` blocks. Applying an attribute to `let`, `return`, or an expression statement is a parse error.
- Ordinary attribute wrappers are transparent to parsing, type checking, slot resolution, VM execution, REPL binding collection, LSP named-parameter analysis, and tree-sitter syntax. `#[derive(Debug)]` and `#[derive(Show)]` on structs are expanded after parsing into an internal display trait implementation, so template strings and formatted output can use `${value}`.
- `#[cfg(...)]` filters items during AST macro expansion. Supported predicates are `true`, `false`, `feature = "name"`, `feature("name")`, `not(...)`, `any(...)`, and `all(...)`. `lk macro expand --feature name FILE` enables feature predicates for expansion inspection.

### Macros
- LK supports Rust-shaped declarative macros with LK semantics: `macro_rules! name { (matcher) => { template }; ... }`.
- Invoke function-like macros as `name!(...)`, `name![...]`, or `name!{...}`. Macro definitions are compile-time items and do not become runtime statements.
- Supported fragment kinds: `expr`, `stmt`, `block`, `item`, `ident`, `literal`, `tt`, `pat`, `ty`, and `path`.
- `expr`, `stmt`, `item`, `pat`, `ty`, and `path` fragments use parser-discovered or grammar-guided capture boundaries, so a fragment can stop before a following block metavariable without requiring a comma delimiter.
- `expr`, `stmt`, `pat`, `ty`, and `path` matcher positions enforce follow-set diagnostics to reject ambiguous future-incompatible matcher shapes. LK also permits an immediately following `block` fragment where its grammar-guided capture needs that form.
- Repetition supports Rust-style `$( ... )*`, `$( ... )+`, and `$( ... )?`, with an optional separator such as `$( $x:expr ),*`. Nested repetitions preserve their capture shape during template substitution, including empty `*`/`?` captures and optional nested repetitions. LK rejects zero-width repetition patterns and trailing separators in macro invocations, renders nested separators without trailing separators, and validates duplicate matcher bindings plus matcher/template repetition-depth mismatches at parse time.
- Macro expansion happens before normal parsing/type checking. Captured identifiers resolve at the call site; local binding positions introduced by the macro template are freshened to avoid common name collisions, including `let`/`const` destructuring, short declarations `name := value`, `for`/`if let`/`while let`, `match` arm patterns and guards, `select case value <- recv(...)` bindings without freshening the generated `recv`/`send` operation names, and positional/named/default function plus closure parameters. Generated semantic name and definition-name positions such as member-access fields (`object.item`), map literal bare keys, struct literal field names, named argument keys, `fn`/`struct`/`trait`/`type` definition names, and struct declaration field names are preserved and are not freshened as local binding references. Generated type-reference positions such as `let`/`const` type annotations, function parameter and return types, type alias targets, and `impl Trait for Type` headers are also preserved.
- Export macros from a file/package with `export macro_rules! name { ... }` or `export { internal as public };`. Ordinary `macro_rules!` definitions remain private to the defining file/module for external macro imports.
- Exported macros can call private helper macros from their defining file/package with `$crate::helper!()` and can generate runtime item references such as `$crate::helper()` that resolve against the macro definition module. Imported file/package macros inject an internal runtime namespace import for this anchor; same-file anchors rewrite to local item references.
- File, package, and std macro imports use LK `use` syntax: `use { answer as ans } from "macros"; ans!();`, `use { answer } from util; answer!();`, `use { vec, matches } from macros; vec![1];`, `use "macros"; macros::answer!();`, and `use * as m from macros; m::matches!(x, 1);`. External macro imports only see exported macro names. Named macro imports and the std `macros` namespace are compile-time-only and are removed before runtime import execution. Split runtime item imports and named macro imports into separate `use` statements.
- The built-in compile-time `macros` module currently exports `vec!`, `assert!`, `assert_eq!`, `assert_ne!`, `matches!`, `panic!`, `todo!`, and `unreachable!`.
- Macro expansion errors include rule-level mismatch notes and an expansion stack showing nested macro calls; parse errors and strict type-check diagnostics caused by macro-generated tokens include a macro origin stack. LSP type diagnostics use the same token origins. Post-parse AST macro inputs also expose hover text and document-symbol entries for generated items. Same-file and imported file/package macro invocations support goto-definition to their `macro_rules!` definition. Ordinary references can also jump to `macro_rules!`-generated item definitions through expanded token spans, to manifest-backed proc/AST generated item definitions through expanded token spans plus AST generated item origins, and to external or built-in generated members, field-access expressions, call-callee references, variable references, assignment target references, binding origins, semantic-name origins, generated declaration labels, and type references such as `value`, `show`, `user.profile.id`, `user.profile.render`, `seed`, `current`, `kind`, `Alias`, `Boxed`, `Reader`, `read`, and `User` through AST generated member origins, with broader generated-reference navigation still planned.
- Current implementation covers `macro_rules!`, function-like invocations, item attribute preservation, built-in struct derives for `Debug`/`Show`, built-in `cfg` item filtering, the versioned procedural macro protocol data model, isolated process hosting, external derive providers, external `#[attr] item` and impl-method transform providers, and external function-like providers registered through `ProcMacroProviders` or `Lk.toml`.
- `Lk.toml` can declare process-backed providers with `[macros.derive.NAME]`, `[macros.attribute.NAME]`, and `[macros.function_like.NAME]` tables. Each provider uses `command`, optional `args`, optional `timeout_ms`, and optional `max_output_bytes`; derive, attribute, and function-like providers are wired to the parser today. Dependency providers are discovered only when the current package lists the dependency in `[macros].trusted_dependencies`; trusted dependency function-like providers are invoked as `package::name!()`.
- Procedural macro dependency metadata participates in cache invalidation. LK fingerprints reported `path`/`digest` entries plus readable dependency file state; direct native execution stores a `.proc-macro-deps.json` sidecar beside cached native executables, and the LSP workspace cache invalidates preloaded analysis when dependency fingerprints become stale or when saving a dependency path through its reverse dependency graph.
- Procedural macro output tokens preserve provider-supplied spans for later parse diagnostics; missing output spans fall back to the macro call or attribute span. Expanded token streams also expose per-token origins for call-site captures, macro-definition tokens, `$crate` anchors, and function-like proc macro output. Post-parse derive/attribute/cfg expansion records item-level AST macro origins, structured generated-item origins, external generated function/trait/impl member origins, generated static field-access expression origins such as `expr user.profile.id`, generated call-callee reference origins such as `call helper` and `call user.profile.render`, generated variable reference origins such as `ref seed`, generated assignment target reference origins such as `assign_ref current` and `compound_assign_ref current`, generated binding origins such as `binding self` and `binding current`, generated semantic-name origins such as `struct_field id`, `map_key kind` for map literals and map/object destructuring patterns, and `named_arg current`, generated type reference origins such as `type_ref User` from item signatures, field declarations, aliases, impl targets, statement-local type annotations, struct literals, and generated declarations in top-level items or nested statement/block bodies, generated declaration labels such as `type Alias`, `struct Boxed`, `trait Reader`, and `fn read`, and built-in derive generated member/expression origins such as `fn show` and `expr self.field` with source-map spans that are visible through `lk macro expand --origins`, LSP hover, and LSP document symbols.
- Use `lk macro expand <file> --trace --deps --origins` to inspect the expanded token stream, token expansion trace, collected procedural macro dependencies, token origin JSON, AST macro origin JSON, structured AST generated-item/member origin JSON, and any post-parse AST derive/attribute expansion.
- Example:

```lk
macro_rules! vec {
  ($($value:expr),*) => { [$($value),*] };
}

export { vec };

let values = vec![1, 2 + 3, 4];
return values.1;
```

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
- For macros, quoted file uses, package module uses, and the built-in compile-time `macros` module also participate in macro resolution before runtime execution. Use `::` for macro namespaces, such as `m::assert_ok!()`.

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
- `Lk.toml` defines `[package]`, `[dependencies]`, `[workspace]`, `[workspace.dependencies]`, optional `[registry]` publishing metadata, optional `[macros.*]` procedural macro provider tables, and `[macros].trusted_dependencies` for explicit dependency macro provider trust. `lk pkg check` validates macro provider names, path-like provider command paths, trust entries, and package graph metadata before sharing macro packages.
- String dependencies default to GitHub, e.g. `util = "owner/repo"`. Registry dependencies use `{ version = "x.y.z" }` and resolve through `[registry].url`, or through a dependency-level `registry = "https://..."` override.
- `Lk.lock` stores fetched Git sources at concrete revisions, including registry versions resolved from `GET [registry].url/api/v1/packages/<name>/<version>` responses.
- `lk pkg publish --dry-run` validates `[package].version`, `[registry].url`, include globs, dependency version/revision metadata, and macro provider listings, then prints the registry publish manifest JSON with an `integrity.sha256` digest over the canonical payload. `lk pkg publish` verifies that digest before uploading the manifest as authenticated JSON to `[registry].url/api/v1/packages` using `LK_REGISTRY_PUBLISH_TOKEN`, `LK_REGISTRY_TOKEN`, or `LK_PUBLISH_TOKEN`. Registry version responses and index entries may include `publish_manifest`; LK validates its package, version, registry URL, and integrity digest before trusting that version.
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
- Compile to a native executable: `lk compile [FILE]`
- Compile to a bytecode module artifact: `lk compile bytecode [FILE]` -> `FILE.lkm`
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
