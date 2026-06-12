# Hello LK

LK 是一门用 Rust 编写的脚本语言，语法接近 Rust，但更轻量、更灵活。打开终端，输入 `lk` 进入 REPL，或用 `lk hello.lk` 运行文件。

```lk
println("Hello, LK!");
return 42;
```

`println` 是内置函数，用 `{}` 占位符格式化输出：

```lk
let name = "LK";
println("Welcome, {}!", name);
```

REPL 和 CLI 只在结果不为 `nil` 时打印返回值。如果函数没有 `return`，默认返回 `nil`。

- 行注释：`// ...`
- 块注释：`/* ... */`
- 文档注释：`/// ...`（附着到紧邻的声明）和 `//! ...`（文件级）

## 值与类型

LK 有六种原始类型和几种复合类型。用 `typeof(value)` 查看运行时类型名。

```lk
typeof(42)        // "Int"
typeof(3.14)      // "Float"
typeof("hello")   // "String"
typeof(true)      // "Bool"
typeof(nil)       // "Nil"
```

- `Int` — 64 位有符号整数。`10`、`0xFF`、`1_000`
- `Float` — 64 位浮点数。`3.14`、`1e10`
- `String` — UTF-8 字符串。`"hello"` 或 `'world'`。支持转义 `\n \t \\ \" \' \$ \0`
- `Bool` — `true` 或 `false`
- `Nil` — 空值

数值会自动提升：`Int + Float → Float`，`Int * Float → Float`。整除返回 `Int`，否则返回 `Float`。

```lk
let a = 10 / 2;   // Int 5
let b = 10 / 3;   // Float 3.3333...
let c = 1 + 2.0;  // Float 3.0
```

原始字符串不处理转义和插值：`r"raw\nstring"`、`r#"raw with "quotes""#`。

## 变量与作用域

用 `let` 声明变量，`const` 声明常量，`:=` 短定义：

```lk
let name = "LK";
const VERSION = 1;
count := 0;       // 等价于 let count = 0;
```

`const` 不可重新赋值，`let` 可以：

```lk
let x = 1;
x = 2;            // OK

const Y = 3;
Y = 4;            // 运行时错误
```

块 `{ ... }` 引入新作用域：

```lk
let a = 1;
{
  let a = 2;
  println(a);     // 2
}
println(a);       // 1
```

解构赋值：

```lk
let [first, ..rest] = [1, 2, 3];
let { "name": n, "age": age } = { "name": "LK", "age": 1 };
```

## 运算符与表达式

### 算术与比较

```lk
1 + 2       // 3
10 % 3      // 1
3 == 3      // true
3 != 4      // true
1 < 2       // true
```

### 逻辑与位运算

```lk
true && false   // false
!true           // false
0xA & 0xF      // 按位与
0xA | 0x5      // 按位或
~0xFF          // 按位非
```

### 模板字符串

```lk
let user = "LK";
println("Hello, ${user}!");          // Hello, LK!
println("2 + 3 = ${2 + 3}");         // 2 + 3 = 5
let price = "Price: \$100";           // 转义 $
```

### 空值合并与三元

```lk
let name = nil;
let display = name ?? "anonymous";   // "anonymous"

let status = true;
let label = status ? "active" : "inactive";  // "active"
```

### 可选链

```lk
let user = { "name": "LK" };
user?.name       // "LK"
nil?.name        // nil
user?["name"]    // "LK"
nil?["name"]     // nil
```

### 范围

```lk
let nums = 1..5;     // [1, 2, 3, 4]
let full = 1..=5;    // [1, 2, 3, 4, 5]
let even = 0..10..2; // [0, 2, 4, 6, 8]
```

### 字符串与集合运算

```lk
"ha" * 3            // "hahaha"
3 * "ab"            // "ababab"
[1, 2] + [3, 4]     // [1, 2, 3, 4]
[1, 2, 3] - [2]     // [1, 3]
{ "a": 1 } + { "b": 2 }  // { "a": 1, "b": 2 }
"ell" in "hello"    // true
2 in [1, 2, 3]      // true
```

## 集合

### 列表

```lk
let fruits = ["apple", "banana", "cherry"];
fruits[0]          // "apple"
fruits[-1]         // "cherry"
fruits[1..3]       // ["banana", "cherry"]
```

列表方法（无需导入）：

```lk
fruits.len()       // 3
fruits.push("date");
fruits.contains("apple")  // true
fruits.sort()
fruits.reverse()
fruits.map(|f| f.upper())
fruits.filter(|f| f.starts_with("a"))
```

展开语法：

```lk
let more = ["date", "elderberry"];
let all = [..fruits, ..more, "fig"];
```

### 映射

裸键为字符串键：

```lk
let profile = { name: "LK", version: 1 };
// 等价于 { "name": "LK", "version": 1 }
profile.name                    // "LK"
profile["name"]                 // "LK"
profile.name = "LK Lang";
profile.count += 1;
```

Map 方法：`len`、`is_empty`、`keys`、`values`、`has`、`get`、`set`、`delete`、`clear`。

### 集合

```lk
let s = Set([1, 2, 3, 2]);  // {1, 2, 3}
s.has(2)      // true
s.add(4)
s.delete(1)
s.values()    // [2, 3, 4]（顺序不保证）
```

## 控制流

### 条件

括号可选。`false` 和 `nil` 为假，其余（包括 `0`、`""`）为真：

```lk
if score > 90 {
  println("A");
} else if score > 80 {
  println("B");
} else {
  println("C");
}
```

### 循环

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

## 模式匹配

`match` 是表达式，返回匹配分支的值：

```lk
let label = match 404 {
  200 => "OK",
  301 | 302 => "Redirect",
  404 => "Not Found",
  _ => "Unknown",
};
```

### 解构

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

### 守卫与范围

```lk
match score {
  n if n >= 90 => "A",
  n if n >= 80 => "B",
  1..59 => "F",
  _ => "C",
}
```

## 函数与闭包

### 定义

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

### 命名参数

命名参数放在尾部块中，必须标注类型：

```lk
fn draw_rect(x: Int, y: Int, { width: Int, height: Int? = 100 }) -> Int {
  return width * (height ?? 0);
}

draw_rect(0, 0, width: 50);
draw_rect(0, 0, width: 50, height: 200);
```

### 闭包

```lk
let double = |x| x * 2;
let add = |a, b| { let sum = a + b; sum };

double(5)      // 10
add(3, 4)      // 7
```

闭包捕获并修改外层变量：

```lk
let count := 0;
let inc = || { count += 1; };
inc();
inc();
println(count);  // 2
```

函数字面量形式：`fn(a, b) => a + b`

### 一等函数

```lk
fn apply(f, x) {
  return f(x);
}

apply(|n| n * 3, 7)  // 21
```

## 结构体与 Trait

### 定义与实例化

```lk
struct Rect { w: Int, h: Int }

let shape = Rect { w: 8, h: 5 };
shape.w             // 8
```

调用糖（等价于 `Rect(w: 8, h: 5)`）和更新语法：

```lk
let bigger = Rect { ..shape, h: 10 };
```

### Trait 与 Impl

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

自动展示：实现 `show`、`display` 或 `to_string` 方法后，`println("{}")` 和 `${value}` 自动使用它：

```lk
impl Area for Rect {
  fn area(self) -> Int { return self.w * self.h; }
  fn show(self) -> String { return "Rect(${self.w}x${self.h})"; }
}

println("shape = {}", shape);  // shape = Rect(8x5)
```

### derive

`#[derive(Debug)]` 或 `#[derive(Show)]` 自动生成 display 实现：

```lk
#[derive(Show)]
struct Point { x: Int, y: Int }

let p = Point { x: 1, y: 2 };
println("{}", p);  // Point { x: 1, y: 2 }
```

## 字符串与字节

String 元方法（无需导入）：`len`、`lower`、`upper`、`trim`、`starts_with`、`ends_with`、`contains`、`replace`、`substring`、`split`、`join`、`reverse`、`repeat`、`chars`、`char_at`、`byte_at`、`find`、`is_empty`、`format`

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

`bytes` 模块处理二进制数据（需要 `use bytes`）：

```lk
use bytes;

let raw = bytes.from_string("hello");
bytes.len(raw)                   // 5
bytes.slice(raw, 1, 3)          // bytes
bytes.to_string_utf8(raw)       // "hello"
bytes.concat(raw, bytes.from_string("!"))
```

## 迭代器与流

`iter` 模块提供列表工具：

```lk
use iter;

let nums = iter.range(1, 10);
let doubled = iter.map(nums, |n| n * 2);
let evens = iter.filter(doubled, |n| n % 3 == 0);
let total = iter.reduce(evens, 0, |acc, n| acc + n);
```

其他：`enumerate`、`zip`、`take`、`skip`、`chain`、`flatten`、`unique`、`chunk`

`stream` 模块提供懒执行管道（需要 `use stream`）：

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

## 模块与包

### use 导入

```lk
use math;                          // 整个模块作为命名空间
use { abs, sqrt } from math;       // 选择性导入
use math as m;                     // 别名
use * as m from math;              // 命名空间别名
use { std } from io;               // 从父模块导入子模块
use "path/to/file";               // 文件模块（名称为文件名）
```

文件导入只允许相对路径，禁止 `..`。解析顺序：先 `name.lk`，再 `name/mod.lk`。

### 包

`Lk.toml` 定义包、依赖和工作区：

```toml
[package]
name = "myapp"
version = "0.1.0"
edition = "2026"

[dependencies]
util = "owner/repo"
```

CLI 命令：`lk pkg init`、`lk pkg fetch`、`lk pkg check`、`lk pkg publish`、`lk pkg tree`

## 宏

### 声明式宏

```lk
macro_rules! vec {
  ($($value:expr),*) => { [$($value),*] };
}

let values = vec![1, 2 + 3, 4];  // [1, 5, 4]
```

用 `export macro_rules! name { ... }` 或 `export { internal as public }` 导出宏。

导入：`use { vec } from macros;`、`use "my_macros";`、`use * as m from macros;`

内置宏：`vec!`、`assert!`、`assert_eq!`、`assert_ne!`、`matches!`、`panic!`、`todo!`、`unreachable!`

### 属性

```lk
#[derive(Show)]
struct Point { x: Int, y: Int }

#[cfg(feature = "debug")]
fn debug_log(msg) { println(msg); }
```

### 过程宏

在 `Lk.toml` 中声明：

```toml
[macros.derive.MyDerive]
command = "./tools/my-derive"

[macros]
trusted_dependencies = ["helper_macros"]
```

## 并发

并发功能需要启用 feature gate：

```lk
// spawn 创建任务
let handle = spawn(|| {
  return 42;
});

// chan 创建通道
let ch = chan(10, "Int");
send(ch, 1);
let [ok, val] = recv(ch);

// select 选择
select {
  case value <- recv(ch) => println("got {}", value),
  case send(ch, 42) => println("sent"),
  default => println("none ready"),
}
```

时间模块（并发）：

```lk
use time;

let start = time.now();
time.sleep(100);  // 毫秒
time.since(start, time.now());
```

## CLI 速查

| 命令 | 说明 |
|------|------|
| `lk` | 启动 REPL |
| `lk FILE` | 执行文件 |
| `lk compile [FILE]` | 编译为 native 可执行文件 |
| `lk compile bytecode [FILE]` | 编译为 `.lkm` bytecode 产物 |
| `lk FILE.lkm` | 执行 bytecode 产物 |
| `lk check FILE` | 类型检查 |
| `lk macro expand FILE` | 宏展开 |
| `lk pkg init/fetch/check/publish/tree` | 包管理 |

## 类型注解

类型注解是可选的，运行时仍为动态类型：

```lk
let x: Int = 1;
fn greet(name: String) -> String { return "Hi, ${name}"; }
```

支持的类型：`Int`、`Float`、`String`、`Bool`、`Nil`、`Any`、`List<T>`、`Map<K,V>`、`Set<T>`、`Task<T>`、`Channel<T>`、`(T1, T2) -> R`

联合类型：`Int | String`，可选类型：`String?`（等价于 `String | Nil`）
