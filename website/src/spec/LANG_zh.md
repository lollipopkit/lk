# 语言总览

本文件描述了本仓库实现的 LK 语言（解析器、求值器、语句、类型与标准库绑定）。

### 注释
- 行注释：`// ...`
- 块注释：`/* ... */`
- 工具文档注释：`/// ...` 与 `/** ... */` 会在紧邻 `fn`、`struct`、`trait` 或 `type` 声明时附着到该声明，用于 LSP hover。文件顶部的 `//! ...` 与 `/*! ... */` 用于 package root hover 文档。这些注释不改变运行时语义。

### 标识符
- 由字母、数字、`_`、`-` 组成。关键字保留。
- 注意词法分析器允许标识符中出现 `-`。

### 字面量
- 字符串：`"..."` 或 `'...'`，UTF‑8 字符串。支持转义 `\n \r \t \\ \" \' \$ \0`。
- 原始字符串（Rust 风格，无转义/插值）：`r"..."`、`r#"..."#`、`r##"..."##`（支持多行）。
- 整型：64 位有符号整数，支持前置符号以及浮点数科学计数法。
- 浮点数：64 位浮点数，支持科学计数法。
- 布尔：`true`、`false`
- 空值：`nil`

### 集合
- 列表：`[a, b, c]`（允许异构）。下标访问：`list[0]`。负索引：`list[-1]`。区间切片：`list[1..3]`。安全访问帮助函数在标准库/元方法中。
- 映射：`{ key: value, ... }`。裸 key 为字符串键：`{name: "Alice", age: 30}` 等价于 `{ "name": "Alice", "age": 30 }`。键使用运行时 key 值（nil/bool/int/string/object；float 会被拒绝）。可用 `map.key` 或 `map["key"]` 访问。
- 集合：`Set()` 创建空集合；`Set([items])` 从列表构造集合。Set 元素使用与 Map 相同的 key 规则。

### 模板字符串
- 仅支持在普通引号字符串中使用 `${expr}` 插值（`"..."` 与 `'...'` 都可）。
- 原始字符串不支持插值。
- 使用 `\$` 转义 `$`：`"Price: \$100"`。
- `println` 和 `print` 支持 `{}` 格式占位符：`println("{} + {} = {}", a, b, a + b)`。
- 示例：`"Hello, ${user.name}!"`、`"Sum: ${1 + 2}"`。

### 输入与变量
- 没有隐式运行时上下文。标识符必须在词法环境中定义（例如通过语句中的 `let`、函数参数或模块 `use`）。
- 通过标准库显式读取外部输入：`use { std } from io;` 后使用 `std.read_to_string(std.stdin())`。通过 `encoding` 手动解析：`encoding.json.parse(...)`、`encoding.yaml.parse(...)`、`encoding.toml.parse(...)`。
- 示例：`use { std } from io; use { json } from encoding; let data = json.parse(std.read_to_string(std.stdin())); return data.req.user.id == 1;`

### 常量
- `const name = expr;` —— 类似 `let`，但不可变。尝试重新赋值 `const` 变量会在运行时错误。

### 函数调用与方法
- 任意表达式可调用：`f(x, y)`、`(g)(z)`。
- 属性访问：`expr.field` 或 `expr[expr]`。可选链：`expr?.field` 与 `expr?[index]`。
- 方法糖：`value.method(args...)` 会按如下分发：
  1) 如果 `value.method` 返回可调用对象（闭包/原生函数），则直接调用。
  2) 否则按值类型查找注册的元方法，将接收者作为第一个参数传入（例如：`"abc".len()`；详见标准库）。

### 闭包
- 仅表达式形式：`|a, b| a + b`。
- 块形式：`|x| { let y = x + 1; y }`，最后一个表达式作为返回值。
- 函数字面量形式：`fn(a: Int, b) => a + b`。
- 闭包会捕获并可变更外层作用域变量。

### 范围（Range）
- `a..b` 与 `a..=b` 计算后产生整型列表（前者不含右端点，后者含右端点）。也可用于模式匹配。
- 显式步进：`a..b..step`，例如 `0..10..2` 产生 `[0, 2, 4, 6, 8]`。

### 空值合并与三元表达式
- `lhs ?? rhs` 在 `lhs` 非 `nil` 时返回 `lhs`，否则返回 `rhs`。
- `cond ? then : else`（右结合）。在表达式中 `cond` 必须是 Bool。`if`/`while` 中按真值判断（见下）。

### 位运算符
- `a & b` —— 按位与
- `a | b` —— 按位或
- `~a` —— 按位非

### 字符串与集合运算符
- `+` 支持 String + String 拼接。其他字符串与数字混合默认未开启（通过功能开关控制）。
- `"ha" * 3` 产生 `"hahaha"`（字符串 × 整数重复）。`Int × String` 也支持。
- 列表之间的 `-` 产生一个新列表，移除右侧列表中的元素。
- 列表之间 `+` 表示拼接。列表与值相加表示追加。
- 映射之间 `+` 表示合并，遇到重复键以右侧为准。
- `in` 支持字符串子串判断 `str in str`、列表/集合成员检查、映射键存在性。对于 `list in list`，会检查左侧所有元素都包含在右侧。

## 运算符（按优先级）
- 后缀：调用 `()`、点 `.` 字段、下标 `[expr]`、可选链 `?.field`、可选下标 `?[expr]`
- 一元：`!`（逻辑非）、`~`（按位非）
- 乘除余：`* / %`
- 加减：`+ -`
- 区间：`.. ..=`（以及 `..step` 变体）
- 比较/成员：`== != < > <= >= in`
- 按位与：`&`
- 按位或：`|`
- 逻辑：`&& ||`
- 空值合并：`??`
- 三元：`? :`（表达式运算符中优先级最低）

### 说明
- 除法：`Int / Int` 若可整除则返回 `Int`，否则返回 `Float`。`math.pow(2, 10)` 返回 `Float`。
- 数值自动提升：`Int + Float → Float`，`Int * Float → Float`。

## 表达式
- 字面量、列表、映射、变量、调用、属性/下标访问、闭包、范围、逻辑/比较、`??` 和 `?:`。
- 并发 helper（功能开关 `concurrency`）是普通函数调用：
  - `spawn(fn_or_closure)` → Task
  - `chan(capacity?, type?)` → Channel（type 为字符串如 `"Int"`）
  - `send(channel, value)` → Bool
  - `recv(channel)` → `[ok, value]`
- `select` 是针对通道操作的专用表达式：
  - `select { case recv(ch) => expr; case value <- recv(ch) if guard => expr; case send(ch, value) => expr; default => expr }`

### Match 表达式
- `match value { pattern => expr, ... }`（允许 `,` 或 `;` 作为分隔）。返回匹配分支的值。见下文模式定义。

## 模式
用于 `match`、`if let`、`while let` 与 `let` 解构。
- 字面量：`1`、`3.14`、`"x"`、`true`、`nil`
- 变量绑定：`name`
- 通配符：`_`
- 列表解构：`[p1, p2, ..rest]`
- 映射解构：`{ "key": pat, other: pat, ..rest }`（key 可为字符串字面量或标识符，rest 捕获剩余字段）
- 或模式：`p1 | p2 | p3`
- 守卫模式：`pat if expr`
- 区间模式：`1..10`、`0..=n`

### For 循环模式
- 支持扩展模式集合：
  - 变量：`x`
  - 忽略：`_`
  - 元组（逗号分隔）：`for i, item in pairs { ... }` —— 解构可迭代的成对元素。
  - 数组：`[a, b, ..rest]`
  - 对象：`{ "k": v, ... }`（字符串 key）

## 语句
- 程序是语句序列。分号 `;` 终止简单语句和表达式语句。

### 控制流
- `if (cond) stmt` 或 `if cond stmt`（括号可选）。真值规则：`false` 和 `nil` 为假；其余值（包括 `0`、`""`）都为真。
- `if let pattern = expr stmt [else stmt]`
- `while (cond) stmt` 或 `while cond stmt`
- `while let pattern = expr stmt`
- `for pattern in expr stmt`，其中 `expr` 可迭代：列表、字符串（字符）、映射（迭代 `[key, value]`）、集合（迭代 value）。
- `break;`、`continue;`
- `return;` 或 `return expr;`

### 变量
- 声明/解构：`let pattern [: Type] = expr;`
- 常量声明：`const name = expr;` —— 不可变绑定，重赋值会触发运行时错误。
- 赋值：`name = expr;`
- 复合赋值：`name += expr;`、`-=`、`*=`、`/=`、`%=`
- 下标赋值：`arr[i] = expr;`、`arr[i] += expr;`
- 点赋值：`obj.field = expr;`、`obj.field += expr;`、`map.key = expr;`、`map.key += expr;`
- 短定义：`name := expr;`（定义并初始化）
- 词法作用域：块 `{ ... }` 会引入新作用域。

### 结构体
- 定义：`struct User { id: Int, name: String? }`
- 字面量实例化：`User { id: 1, name: "Ann" }`
- 实例化（调用糖）：`User(id: 1, name: "Ann")`
- 访问：`user.name`
- 更新语法：`User { ..existing, field: value }` 或 `User { ..existing }` —— 从 `existing` 复制全部字段；提供字段时覆盖指定值。

### Trait 与 Impl
- Trait 定义：`trait Area { fn area(self) -> Int; }`
- 实现：`impl Area for Rect { fn area(self) -> Int { return self.w * self.h; } }`
- 当调用 `value.method()` 且未匹配直接属性/方法时，会使用 `impl` 中定义的方法分发。
- 自动展示：若类型实现了 `fn show(self) -> String`、`fn display(self) -> String` 或 `fn to_string(self) -> String`，则 `println("{}")` 与模板 `${value}` 会自动使用该实现进行格式化。

### 函数
- 定义：`fn name(param1[: Type], param2[: Type]) [-> Type] { statements }`
- 参数与返回类型可省略；若无 `return`，默认返回 `nil`。
- 一等函数：函数值可传递、返回与调用。
- 位置参数默认值：`fn greet(name, greeting = "hello") { ... }`，带默认值参数必须放在所有必填位置参数之后。
- 命名参数放在可选尾部块中，且必须写类型标注：`fn f(a, b, { flag: Bool = true, label: String }) { ... }`。该块也可以是整个参数列表：`fn configure({host: String}) { ... }`。
- 默认值延迟在被调端计算；表达式可以引用其他参数。
- 调用时使用 `name: expr` 传入命名参数：`f(1, 2, label: "demo", flag: false)` 或 `f(label: "demo")`。命名参数可任意顺序；一旦出现命名参数，其后不能再出现位置参数。

### 属性（Attributes）
- item 声明可以带 Rust 风格的保留属性：`#[derive(Debug)] struct User { id: Int }` 或 `#[inline] fn answer() { return 42; }`。
- 当前属性可以附着在 item 声明上（`fn`、`struct`、`type`、`trait`、`impl`），也可以附着在 `impl` block 内的方法上。把属性加到 `let`、`return` 或表达式语句上会产生解析错误。
- 普通 attribute wrapper 对解析、类型检查、slot resolution、VM 执行、REPL 绑定收集、LSP 命名参数分析与 tree-sitter 语法都是透明的。结构体上的 `#[derive(Debug)]` 与 `#[derive(Show)]` 会在解析后展开为内部 display trait 实现，因此 template string 与格式化输出可以使用 `${value}`。
- `#[cfg(...)]` 会在 AST macro expansion 阶段过滤 item。当前支持的 predicate 包括 `true`、`false`、`feature = "name"`、`feature("name")`、`not(...)`、`any(...)` 与 `all(...)`。`lk macro expand --feature name FILE` 可为展开检查启用 feature predicate。

### 宏
- LK 支持 Rust 形态、LK 语义的声明式宏：`macro_rules! name { (matcher) => { template }; ... }`。
- 函数式宏调用写作 `name!(...)`、`name![...]` 或 `name!{...}`。宏定义是编译期 item，不会成为运行时语句。
- 支持的 fragment kind：`expr`、`stmt`、`block`、`item`、`ident`、`literal`、`tt`、`pat`、`ty`、`path`。
- `expr`、`stmt`、`item`、`pat`、`ty` 与 `path` fragment 使用 parser-discovered 或 grammar-guided capture boundary，因此 fragment 可以在后续 block metavariable 前停止，不强制依赖逗号分隔。
- `expr`、`stmt`、`pat`、`ty` 与 `path` matcher 位置会执行 follow-set 诊断，以拒绝未来语法不兼容的歧义 matcher。LK 也允许在 grammar-guided capture 需要时紧跟 `block` fragment。
- 重复语法支持 Rust 风格的 `$( ... )*`、`$( ... )+` 与 `$( ... )?`，并支持可选分隔符，例如 `$( $x:expr ),*`。
- 宏展开发生在普通解析与类型检查之前。捕获的标识符按调用点解析；宏模板中新引入的局部绑定会 freshen，避免常见命名冲突。
- 使用 `export macro_rules! name { ... }` 或 `export { internal as public };` 从文件/package 导出宏。普通 `macro_rules!` 定义只在定义所在文件/模块内可用，对外部宏导入保持私有。
- 已导出的宏可以通过 `$crate::helper!()` 调用定义所在文件/package 内的私有 helper 宏。
- 文件、package 与标准宏导入使用 LK `use` 语法：`use { answer as ans } from "macros"; ans!();`、`use { answer } from util; answer!();`、`use { vec, matches } from macros; vec![1];`、`use "macros"; macros::answer!();`、`use * as m from macros; m::matches!(x, 1);`。外部宏导入只能看到已导出的宏名。命中宏的命名导入与标准 `macros` 命名空间都是编译期导入，会在运行时 import 执行前移除。运行时 item 导入与命名宏导入应拆成不同 `use` 语句。
- 内置编译期 `macros` 模块当前导出 `vec!`、`assert!`、`assert_eq!`、`assert_ne!`、`matches!`、`panic!`、`todo!` 与 `unreachable!`。
- 宏展开错误会包含逐条 rule 的 mismatch notes，并附带 expansion stack 展示嵌套宏调用链；由宏生成 token 触发的 parse error 会包含 macro origin stack。LSP diagnostics 会保留这些宏展开消息，token origin 在语义诊断中的进一步使用仍在计划中。
- 当前实现覆盖 `macro_rules!`、函数式宏调用、item attribute preservation、结构体内置 `Debug`/`Show` derive、内置 `cfg` item filtering、版本化 procedural macro protocol 数据模型、隔离进程 host、外部 derive provider、外部 `#[attr] item` 与 impl-method transform provider，以及通过 `ProcMacroProviders` 或 `Lk.toml` 注册的外部 function-like provider。
- `Lk.toml` 可以用 `[macros.derive.NAME]`、`[macros.attribute.NAME]` 与 `[macros.function_like.NAME]` 表声明进程型 provider。每个 provider 使用 `command`、可选 `args`、可选 `timeout_ms` 与可选 `max_output_bytes`；目前 derive、attribute 与 function-like provider 都已接入解析器。
- Procedural macro 输出 token 会保留 provider 提供的 span，用于后续 parse diagnostics；缺失输出 span 时会回退到宏调用或 attribute span。展开后的 token stream 还会暴露逐 token origin，用于区分 call-site capture、macro definition token、`$crate` anchor 与 function-like proc macro output。
- 使用 `lk macro expand <file> --trace --deps --origins` 可以查看展开后的 token stream、token 展开 trace、收集到的 procedural macro dependencies、token origin JSON，以及 parse 后的 AST derive/attribute 展开结果。
- 示例：

```lk
macro_rules! vec {
  ($($value:expr),*) => { [$($value),*] };
}

export { vec };

let values = vec![1, 2 + 3, 4];
return values.1;
```

### Use 导入
- 形式：
  - `use math;` —— 标准库模块作为命名空间。
  - `use { file, std } from io;` —— 从父标准库模块选择性导入子命名空间。
  - `use io;` —— 父标准库模块命名空间，可通过 `io.file`、`io.std` 访问子命名空间。
  - `use "path/to/file.lk";` —— 文件模块作为命名空间（名称来自文件名）。
  - `use { abs, sqrt } from math;` —— 选择性导入。
  - `use { f as g } from "m.lk";` —— 带别名。
  - `use * as m from math;` —— 命名空间别名。
  - `use math as m;` —— 模块别名。
- 裸模块导入直接绑定模块名：`use net;` 会定义 `net`。
- 对宏而言，引号文件导入、package 模块导入与内置编译期 `macros` 模块也会先参与宏解析。宏命名空间调用使用 `::`，例如 `m::assert_ok!()`。

- 文件导入与安全：
  - 文件不会自动对外可见。跨文件依赖必须显式 `use`。
  - 引号路径导入不依赖 `Lk.toml`；按当前文件所在目录解析。
  - 路径仅允许相对路径，并经过清洗：绝对路径和任意 `..` 组件会被拒绝。
  - 解析顺序：`${MOD_NAME}.lk`，再 `${MOD_NAME}/mod.lk`（相对于当前文件目录）。
  - 如果引号路径已经包含 `.lk`（如 `"lib/foo.lk"`），需为相对路径且若存在则直接使用。
  - 在 package 中，裸模块 `use` 先查标准库，再查 `Lk.toml` 的工作区/依赖包。package 模块解析到 `src/mod.lk` 或 `src/<package-name>.lk`。
  - 由于拒绝 `..`，嵌套目录中的代码不能通过 `../...` 使用父目录文件；当子目录依赖树外文件时，请使用 package/workspace 模块。

#### 文件导入示例

```text
a.lk
b.lk
c/c1.lk
c/d/d1.lk
```

来自 `a.lk`：

```lk
use "b";       // b.lk，导出名为 b
use "c/c1";    // c/c1.lk，导出名为 c1
use "c/d/d1";  // c/d/d1.lk，导出名为 d1
```

来自 `c/c1.lk`：

```lk
use "d/d1";    // c/d/d1.lk，导出名为 d1
// use "../a"; // 被拒绝：父目录 use 不允许
```

## 包
- `Lk.toml` 定义 `[package]`、`[dependencies]`、`[workspace]`、`[workspace.dependencies]`，以及可选的 `[macros.*]` procedural macro provider 表。
- 字符串依赖默认来自 GitHub，例如：`util = "owner/repo"`。
- `Lk.lock` 保存具体 revision 的已抓取 git 源码。
- 包管理命令和清单示例见 `docs/packages.md`。可运行的 workspace 示例见 `examples/lk-example-workspace`。

## 内置与标准库
- 全局内置：`print(fmt, ...args)`、`println(fmt, ...args)`、`panic([msg])`、`assert(cond[, msg])`、`assert_eq(actual, expected[, msg])`、`assert_ne(actual, expected[, msg])`、`typeof(value)`。
- `typeof(value)` 返回运行时类型名字符串：`"Int"`、`"Float"`、`"String"`、`"Bytes"`、`"Bool"`、`"Nil"`、`"List"`、`"Map"`、`"Set"`、`"Slice"`、`"File"`/`"TcpStream"` 等 resource 类型名，或结构体类型名。

### 标准库模块
按需导入：`math`、`string`、`bytes`、`iter`、`stream`、`datetime`、`os`、`fs`、`path`、`env`、`process`、`io`、`net`、`slice`、`encoding`、`hash`、`regex`、`random`、`uuid`、`http`。启用 `concurrency` 后支持：`task`、`chan`、`time`。

- `math`：常量 `pi`、`e`、`inf`、`nan`、`max_int`、`min_int`、`max_float`、`epsilon`；函数 `abs`、`sqrt`、`floor`、`ceil`、`round`、`min`、`max`、`pow`、`exp`、`sin`、`cos`、`tan`、`asin`、`acos`、`atan`、`atan2`、`log`、`log10`、`log2`、`clamp`、`random`、`hypot`、`cbrt`、`sinh`、`cosh`、`tanh`、`trunc`、`fract`、`sign`、`to_int`、`to_float`、`is_nan`、`is_inf`。
- `string`：方法（见下方元方法）。
- `bytes`：二进制数据类型，底层按字节保存。`from_list(list)`、`from_string(str)`、`len(bytes)`、`is_empty(bytes)`、`get(bytes, index)`、`slice(bytes, start[, end])`、`to_list(bytes)`、`to_string_utf8(bytes)`、`to_string_lossy(bytes)`、`concat(a, b)`、`eq(a, b)`。
- `iter`：仅提供模块级列表工具：`range([start,] end [, step])`、`enumerate(list)`、`zip(list1, list2)`、`take(list, n)`、`skip(list, n)`、`chain(list1, list2)`、`flatten(list)`、`unique(list)`、`chunk(list, size)`，以及高阶操作 `map(list, fn)`、`filter(list, fn)`、`reduce(list, init, fn)`。
- `stream`：模块级懒执行管道。`stream.from_list(list)`、`stream.range(start, end)`、`stream.iterate(seed, fn)`、`stream.repeat(val)`、`stream.from_channel(ch)`、`stream.map(s, fn)`、`stream.filter(s, fn)`、`stream.take(s, n)`、`stream.skip(s, n)`、`stream.chain(a, b)`、`stream.subscribe(s)`、`stream.next(cursor)`、`stream.collect(stream_or_cursor)`、`stream.next_block(cursor[, timeout_ms])`、`stream.collect_block(stream_or_cursor[, n][, timeout_ms])`。
- `datetime`：`now()`（微秒）、`format(secs, fmt)`、`parse(str, fmt)`、`add(secs, delta)`、`sub(secs, delta)`、`day_of_week(secs)`、`day_of_year(secs)`、`is_weekend(secs)`。
- `os`：平台/时间辅助函数：`hostname()`、`arch()`、`os()`、`clock()`、`time()`、`epoch()`。
- `fs`：基于路径的文件系统 API。`read(path) -> Bytes`、`read_to_string(path)`、`write(path, data)`、`append(path, data)`、`exists(path)`、`is_file(path)`、`is_dir(path)`、`metadata(path)`、`read_dir(path)`、`create_dir(path)`、`create_dir_all(path)`、`remove_file(path)`、`remove_dir(path)`、`remove_dir_all(path)`、`rename(from, to)`、`copy(from, to)`、`canonicalize(path)`、`temp_dir()`。
- `path`：`join(parts...)`、`parent(path)`、`file_name(path)`、`file_stem(path)`、`extension(path)`、`with_extension(path, ext)`、`is_absolute(path)`、`normalize(path)`、`components(path)`、`sep()`、`delimiter()`。
- `env`：`get(key)`、`get_or(key, default)`、`has(key)`、`vars()`。不暴露进程环境变量 mutation。
- `process`：`id()`、`cwd()`、`set_cwd(path)`、`exit(code)`、`status(cmd[, args])`、`output(cmd[, args]) -> {status, success, stdout: Bytes, stderr: Bytes}`、`output_string(cmd[, args])`。
- `io`：父命名空间。可用 `use { std, file } from io;` 导入子命名空间，或通过 `io.std`、`io.file` 访问。
- `io.std`：`stdin()`、`stdout()`、`stderr()`、`read(reader[, max_bytes]) -> Bytes`、`read_to_string(reader)`、`read_line(reader)`、`write(writer, data)`、`writeln(writer, data)`、`flush(writer)`。`write`/`writeln` 接受 `Bytes` 或 `String`。
- `io.file`：基于 `File` resource 的 API。`open(path, mode)`、`create(path)`、`read(file[, max_bytes]) -> Bytes`、`read_to_string(file)`、`read_line(file)`、`write(file, data)`、`writeln(file, data)`、`write_all(file, data)`、`flush(file)`、`close(file)`。基于路径的操作在 `fs` 中。
- `slice`：`from_list(list)`、`from_string(str)`、`len(slice)`、`is_empty(slice)`、`get(slice, index)`、`sub(slice, start[, end])`、`to_list(slice)`、`to_string(slice)`。
- `encoding`：父命名空间。可用 `use { json, yaml, toml, base64, hex, url } from encoding;` 导入子命名空间，或通过 `encoding.json`、`encoding.base64` 等访问。`json.parse(string)`、`yaml.parse(string)`、`toml.parse(string)`、`base64.encode(data)`、`base64.decode(string) -> Bytes`、`hex.encode(data)`、`hex.decode(string) -> Bytes`、`url.encode_component(string)`、`url.decode_component(string)`、`url.query_parse(string)`、`url.query_stringify(map)`。
- `hash`：`sha256(data)`、`sha1(data)`、`crc32(data)`、`fnv64(data)`。`data` 接受 `Bytes` 或 `String`。
- `regex`：`is_match(pattern, text)`、`find(pattern, text)`、`find_all(pattern, text)`、`captures(pattern, text)`、`replace(pattern, text, replacement)`、`split(pattern, text)`。
- `random`：`int(min, max)`、`float()`、`bool([probability])`、`bytes(len)`、`choice(list)`、`shuffle(list)`。
- `uuid`：`v4()`、`parse(string)`、`is_valid(string)`。
- `http`：同步 client API：`request(method, url[, opts])`、`get(url[, opts])`、`post(url, body[, opts])`；响应为包含 `status`、`headers`、`body: Bytes` 的 map。
- `net`：父命名空间。可用 `use { socket, tcp, udp } from net;` 导入子命名空间，或通过 `net.socket`、`net.tcp`、`net.udp` 访问。
- `net.socket`：`addr(host, port)`、`close(resource)`。
- `net.tcp`：`connect(addr)`、`bind(addr)`、`accept(listener)`、`write(stream, data)`、`read(stream, len?) -> Bytes`、`close(resource)`，以及 `connect_task`、`accept_task`、`read_task`、`write_task`。`write` 接受 `Bytes` 或 `String`。
- `net.udp`：`bind(addr)`、`recv_from(socket, len?) -> {data: Bytes, addr: String}`、`send_to(socket, data, addr)`，以及 `recv_from_task`、`send_to_task`。`send_to` 接受 `Bytes` 或 `String`。
- `time`（并发）：`time.now()`、`time.sleep(ms)`、`time.timeout(ms)`、`time.after(ms)`、`time.since(start, end)`。

### 元方法（可直接通过 `value.method()` 使用，无需导入）
- String：`len`、`lower`、`upper`、`trim`、`starts_with`、`ends_with`、`contains`、`replace`、`substring`、`split`、`join`、`reverse`、`repeat`、`chars`、`char_at`、`byte_at`、`find`、`is_empty`、`format`
- List：`len`、`push`、`set`、`concat`、`join`、`get`、`first`、`last`、`map`、`filter`、`reduce`、`take`、`skip`、`chain`、`flatten`、`unique`、`chunk`、`enumerate`、`zip`、`to_stream`、`sort`、`reverse`、`pop`、`insert`、`remove_at`、`contains`、`index_of`、`slice`、`is_empty`
- Map：`len`、`is_empty`、`keys`、`values`、`has`、`get`、`set`、`delete`、`clear`
- Set：`len`、`is_empty`、`has`、`contains`、`add`、`delete`、`remove`、`values`、`clear`
- Stream：`map`、`filter`、`reduce`、`take`、`skip`、`chain`、`subscribe`、`collect`、`collect_block`
- StreamCursor：`next`、`collect`、`next_block`、`collect_block`
- Channel：`to_stream`

### 索引访问与切片
- 列表和字符串支持整数索引与负索引：`xs[-1]` 获取最后一个元素。
- 列表和字符串支持区间切片：`xs[1..3]`、`s[1..3]`。
- 映射的点赋值与复合赋值：`m.key = val`、`m.count += 2`、`p.x += 9`。
- 列表下标赋值与复合赋值：`arr[1] = 10`、`arr[1] += 5`。

### List Spread
- 将已有列表展开到新列表中：`[0, ..spread_a, 3]`，会将 `spread_a` 中全部元素按顺序插入。

## CLI 输出
- REPL 与 CLI 仅在求值结果不为 `nil` 时打印输出。这可以避免在默认返回 `nil` 的语句后额外输出行（例如 `let`、`fn` 定义、`println(...)`）。若要显示 `nil`，请通过 `println(nil)` 或在格式化输出中显式包含。

## 类型与注解
### 原始与复合类型
- `Int`、`Float`、`String`、`Bool`、`Nil`、`Any`
- `List<T>`、`Map<K, V>`、`Set<T>`
- `Task<T>`、`Channel<T>`（并发）
- 函数类型：`(T1, T2) -> R`
- 联合类型：`A | B | Nil`；可选类型：`T?`（`T | Nil` 的语法糖）
- 支持命名与泛型类型（如 `List<Int>`、`Map<String, Int>`、`Set<String>`）

### 注解
- `let x: Int = 1;`
- `fn f(a: Int, b: String) -> Bool { ... }`
- 类型检查/推断为保守实现；运行时仍为动态类型。

## 语法（EBNF 风格）

### 表达式（优先级从低到高）
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

### 语句
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

## CLI 使用说明
- 运行 REPL：`lk`（`LK_REPL_TUI=always|never|auto` 控制是否强制启用 Reedline 补全 UI、禁用它，或按终端能力自动选择）
- 通过 bytecode VM 执行文件（语句）：`lk FILE`
- 编译为 native 可执行文件：`lk compile [FILE]`
- 编译为 bytecode 模块产物：`lk compile bytecode [FILE]` -> `FILE.lkm`
- 执行模块产物：`lk FILE.lkm`
- 只允许相对且经过清洗的路径。
- CLI 只在结果非 `nil` 时打印。

## 运行时值类型
- `String` —— UTF‑8 字符串
- `Int` —— 64 位有符号整数
- `Float` —— 64 位浮点数
- `Bool` —— 布尔值
- `Nil` —— 空值/未定义值
- `List` —— 有序集合
- `Map` —— 键值映射
- `Set` —— 唯一值集合
- `Function` —— 一等函数
- `Object` —— 结构体实例（含类型名与字段）
- `Task` —— 并发任务句柄（功能开关）
- `Channel` —— 并发通道（功能开关）
- `Stream` —— 懒执行流管线（功能开关）
- `StreamCursor` —— 流元素消费器
