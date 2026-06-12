# Hello LK

LK is a Rust-like scripting language written in Rust. It's lightweight, expressive, and practical. Open a terminal, type `lk` for a REPL, or run `lk hello.lk` to execute a file.

```lk
println("Hello, LK!");
return 42;
```

`println` is a builtin function that uses `{}` placeholders for formatted output:

```lk
let name = "LK";
println("Welcome, {}!", name);
```

The REPL and CLI only print a result when it is not `nil`. Functions return `nil` by default unless you use `return`.

- Line comments: `// ...`
- Block comments: `/* ... */`
- Doc comments: `/// ...` (attaches to the next declaration) and `//! ...` (package root)

## Values & Types

LK has six primitive types and several collection types. Use `typeof(value)` to check the runtime type name.

```lk
typeof(42)        // "Int"
typeof(3.14)      // "Float"
typeof("hello")   // "String"
typeof(true)      // "Bool"
typeof(nil)       // "Nil"
```

- `Int` — 64-bit signed integer. `10`, `0xFF`, `1_000`
- `Float` — 64-bit float. `3.14`, `1e10`
- `String` — UTF-8 string. `"hello"` or `'world'`. Escapes: `\n \t \\ \" \' \$ \0`
- `Bool` — `true` or `false`
- `Nil` — null/undefined

Numeric auto-promotion: `Int + Float → Float`, `Int * Float → Float`. Integer division returns `Int` when exact, `Float` otherwise.

```lk
let a = 10 / 2;   // Int 5
let b = 10 / 3;   // Float 3.3333...
let c = 1 + 2.0;  // Float 3.0
```

Raw strings don't process escapes or interpolation: `r"raw\nstring"`, `r#"raw with "quotes""#`.

## Variables & Scope

Declare with `let`, constants with `const`, short definition with `:=`:

```lk
let name = "LK";
const VERSION = 1;
count := 0;       // equivalent to let count = 0;
```

`const` cannot be reassigned; `let` can:

```lk
let x = 1;
x = 2;            // OK

const Y = 3;
Y = 4;            // runtime error
```

Blocks `{ ... }` introduce a new scope:

```lk
let a = 1;
{
  let a = 2;
  println(a);     // 2
}
println(a);       // 1
```

Destructuring:

```lk
let [first, ..rest] = [1, 2, 3];
let { "name": n, "age": age } = { "name": "LK", "age": 1 };
```

## Operators & Expressions

### Arithmetic & Comparison

```lk
1 + 2       // 3
10 % 3      // 1
3 == 3      // true
3 != 4      // true
1 < 2       // true
```

### Logic & Bitwise

```lk
true && false   // false
!true           // false
0xA & 0xF      // bitwise AND
0xA | 0x5      // bitwise OR
~0xFF           // bitwise NOT
```

### Template Strings

```lk
let user = "LK";
println("Hello, ${user}!");          // Hello, LK!
println("2 + 3 = ${2 + 3}");         // 2 + 3 = 5
let price = "Price: \$100";           // escape $
```

### Nullish Coalescing & Ternary

```lk
let name = nil;
let display = name ?? "anonymous";   // "anonymous"

let status = true;
let label = status ? "active" : "inactive";  // "active"
```

### Optional Chaining

```lk
let user = { "name": "LK" };
user?.name       // "LK"
nil?.name        // nil
user?["name"]    // "LK"
nil?["name"]     // nil
```

### Ranges

```lk
let nums = 1..5;     // [1, 2, 3, 4]
let full = 1..=5;    // [1, 2, 3, 4, 5]
let even = 0..10..2; // [0, 2, 4, 6, 8]
```

### String & Collection Operators

```lk
"ha" * 3            // "hahaha"
3 * "ab"            // "ababab"
[1, 2] + [3, 4]     // [1, 2, 3, 4]
[1, 2, 3] - [2]     // [1, 3]
{ "a": 1 } + { "b": 2 }  // { "a": 1, "b": 2 }
"ell" in "hello"    // true
2 in [1, 2, 3]      // true
```

## Collections

### Lists

```lk
let fruits = ["apple", "banana", "cherry"];
fruits[0]          // "apple"
fruits[-1]         // "cherry"
fruits[1..3]       // ["banana", "cherry"]
```

List meta-methods (no import needed):

```lk
fruits.len()       // 3
fruits.push("date");
fruits.contains("apple")  // true
fruits.sort()
fruits.reverse()
fruits.map(|f| f.upper())
fruits.filter(|f| f.starts_with("a"))
```

Spread syntax:

```lk
let more = ["date", "elderberry"];
let all = [..fruits, ..more, "fig"];
```

### Maps

Bare keys are string keys:

```lk
let profile = { name: "LK", version: 1 };
// equivalent to { "name": "LK", "version": 1 }
profile.name                    // "LK"
profile["name"]                 // "LK"
profile.name = "LK Lang";
profile.count += 1;
```

Map methods: `len`, `is_empty`, `keys`, `values`, `has`, `get`, `set`, `delete`, `clear`.

### Sets

```lk
let s = Set([1, 2, 3, 2]);  // {1, 2, 3}
s.has(2)      // true
s.add(4)
s.delete(1)
s.values()    // [2, 3, 4] (order not guaranteed)
```

## Control Flow

### Conditionals

Parentheses are optional. `false` and `nil` are falsy; everything else (including `0`, `""`) is truthy:

```lk
if score > 90 {
  println("A");
} else if score > 80 {
  println("B");
} else {
  println("C");
}
```

### Loops

```lk
let i = 0;
while i < 5 {
  i += 1;
}

for item in [1, 2, 3] {
  println(item);
}

for ch in "hello" {
  println(ch);
}

for entry in { "a": 1, "b": 2 } {
  println(entry);  // ["a", 1]
}
```

### break / continue / return

```lk
for item in [1, 2, 3] {
  if item == 2 { continue; }
  if item == 3 { break; }
  println(item);
}

fn first_positive(list) {
  for item in list {
    if item > 0 { return item; }
  }
  return nil;
}
```

## Pattern Matching

`match` is an expression that returns the matched arm's value:

```lk
let label = match 404 {
  200 => "OK",
  301 | 302 => "Redirect",
  404 => "Not Found",
  _ => "Unknown",
};
```

### Destructuring

```lk
let [first, second, ..rest] = [10, 20, 30, 40];
// first=10, second=20, rest=[30, 40]

let { "name": n, "age": a, ..other } = { "name": "LK", "age": 1, "lang": "script" };
// n="LK", a=1, other={"lang":"script"}
```

### if let / while let

```lk
if let { "user": { "id": uid } } = payload {
  println("User ID: {}", uid);
}

while let [item, ..tail] = remaining {
  println(item);
  remaining := tail;
}
```

### Guards & Ranges

```lk
match score {
  n if n >= 90 => "A",
  n if n >= 80 => "B",
  1..59 => "F",
  _ => "C",
}
```

## Functions & Closures

### Definition

```lk
fn add(a, b) {
  return a + b;
}

fn greet(name, greeting = "hello") {
  return "${greeting}, ${name}!";
}

greet("LK")              // "hello, LK!"
greet("LK", greeting: "hi")  // "hi, LK!"
```

### Named Parameters

Named parameters live in a trailing block and require type annotations:

```lk
fn draw_rect(x: Int, y: Int, { width: Int, height: Int? = 100 }) -> Int {
  return width * (height ?? 0);
}

draw_rect(0, 0, width: 50);
draw_rect(0, 0, width: 50, height: 200);
```

### Closures

```lk
let double = |x| x * 2;
let add = |a, b| { let sum = a + b; sum };

double(5)      // 10
add(3, 4)      // 7
```

Closures capture and mutate enclosing variables:

```lk
let count := 0;
let inc = || { count += 1; };
inc();
inc();
println(count);  // 2
```

Function-literal form: `fn(a, b) => a + b`

### First-class Functions

```lk
fn apply(f, x) {
  return f(x);
}

apply(|n| n * 3, 7)  // 21
```

## Structs & Traits

### Definition & Instantiation

```lk
struct Rect { w: Int, h: Int }

let shape = Rect { w: 8, h: 5 };
shape.w             // 8
```

Call sugar (equivalent to `Rect(w: 8, h: 5)`) and update syntax:

```lk
let bigger = Rect { ..shape, h: 10 };
```

### Traits & Impl

```lk
trait Area {
  fn area(self) -> Int;
}

impl Area for Rect {
  fn area(self) -> Int {
    return self.w * self.h;
  }
}

shape.area()   // 40
```

Auto-display: implement `show`, `display`, or `to_string` and `println("{}")` and `${value}` will use it:

```lk
impl Area for Rect {
  fn area(self) -> Int { return self.w * self.h; }
  fn show(self) -> String { return "Rect(${self.w}x${self.h})"; }
}

println("shape = {}", shape);  // shape = Rect(8x5)
```

### derive

`#[derive(Debug)]` or `#[derive(Show)]` auto-generates a display implementation:

```lk
#[derive(Show)]
struct Point { x: Int, y: Int }

let p = Point { x: 1, y: 2 };
println("{}", p);  // Point { x: 1, y: 2 }
```

## Strings & Bytes

String meta-methods (no import needed): `len`, `lower`, `upper`, `trim`, `starts_with`, `ends_with`, `contains`, `replace`, `substring`, `split`, `join`, `reverse`, `repeat`, `chars`, `char_at`, `byte_at`, `find`, `is_empty`, `format`

```lk
"Hello".len()                    // 5
"hello".upper()                  // "HELLO"
"  hi  ".trim()                  // "hi"
"a,b,c".split(",")              // ["a", "b", "c"]
["a", "b"].join("-")            // "a-b"
"hello".replace("l", "r")      // "herro"
"hello".contains("ell")         // true
"hello".find("ll")              // 2
```

The `bytes` module handles binary data (requires `use bytes`):

```lk
use bytes;

let raw = bytes.from_string("hello");
bytes.len(raw)                   // 5
bytes.slice(raw, 1, 3)          // bytes
bytes.to_string_utf8(raw)       // "hello"
bytes.concat(raw, bytes.from_string("!"))
```

## Iterators & Streams

The `iter` module provides list utilities:

```lk
use iter;

let nums = iter.range(1, 10);
let doubled = iter.map(nums, |n| n * 2);
let evens = iter.filter(doubled, |n| n % 3 == 0);
let total = iter.reduce(evens, 0, |acc, n| acc + n);
```

Also: `enumerate`, `zip`, `take`, `skip`, `chain`, `flatten`, `unique`, `chunk`

The `stream` module provides lazy evaluation pipelines (requires `use stream`):

```lk
use stream;

let s = stream.from_list([1, 2, 3, 4, 5]);
let cursor = stream.subscribe(
  stream.filter(
    stream.map(s, |n| n * 10),
    |n| n > 20
  )
);
stream.collect(cursor)  // [30, 40, 50]
```

## Modules & Packages

### use Imports

```lk
use math;                          // entire module as namespace
use { abs, sqrt } from math;       // selective import
use math as m;                     // alias
use * as m from math;              // namespace alias
use { std } from io;               // import child from parent
use "path/to/file";               // file module (name is file stem)
```

File imports are relative-only and reject `..`. Resolution order: `name.lk`, then `name/mod.lk`.

### Packages

`Lk.toml` defines packages, dependencies, and workspaces:

```toml
[package]
name = "myapp"
version = "0.1.0"
edition = "2026"

[dependencies]
util = "owner/repo"
```

CLI commands: `lk pkg init`, `lk pkg fetch`, `lk pkg check`, `lk pkg publish`, `lk pkg tree`

## Macros

### Declarative Macros

```lk
macro_rules! vec {
  ($($value:expr),*) => { [$($value),*] };
}

let values = vec![1, 2 + 3, 4];  // [1, 5, 4]
```

Export macros with `export macro_rules! name { ... }` or `export { internal as public }`.

Import: `use { vec } from macros;`, `use "my_macros";`, `use * as m from macros;`

Built-in macros: `vec!`, `assert!`, `assert_eq!`, `assert_ne!`, `matches!`, `panic!`, `todo!`, `unreachable!`

### Attributes

```lk
#[derive(Show)]
struct Point { x: Int, y: y: Int }

#[cfg(feature = "debug")]
fn debug_log(msg) { println(msg); }
```

### Procedural Macros

Declare in `Lk.toml`:

```toml
[macros.derive.MyDerive]
command = "./tools/my-derive"

[macros]
trusted_dependencies = ["helper_macros"]
```

## Concurrency

Concurrency features require a feature gate:

```lk
// spawn creates a task
let handle = spawn(|| {
  return 42;
});

// chan creates a channel
let ch = chan(10, "Int");
send(ch, 1);
let [ok, val] = recv(ch);

// select chooses
select {
  case value <- recv(ch) => println("got {}", value),
  case send(ch, 42) => println("sent"),
  default => println("none ready"),
}
```

Time module (concurrency):

```lk
use time;

let start = time.now();
time.sleep(100);  // milliseconds
time.since(start, time.now());
```

## CLI Quick Reference

| Command | Description |
|---------|-------------|
| `lk` | Start REPL |
| `lk FILE` | Execute a file |
| `lk compile [FILE]` | Compile to native executable |
| `lk compile bytecode [FILE]` | Compile to `.lkm` bytecode artifact |
| `lk FILE.lkm` | Execute bytecode artifact |
| `lk check FILE` | Type check |
| `lk macro expand FILE` | Expand macros |
| `lk pkg init/fetch/check/publish/tree` | Package management |

## Type Annotations

Type annotations are optional; runtime remains dynamically typed:

```lk
let x: Int = 1;
fn greet(name: String) -> String { return "Hi, ${name}"; }
```

Supported types: `Int`, `Float`, `String`, `Bool`, `Nil`, `Any`, `List<T>`, `Map<K,V>`, `Set<T>`, `Task<T>`, `Channel<T>`, `(T1, T2) -> R`

Union types: `Int | String`, optional: `String?` (sugar for `String | Nil`)
