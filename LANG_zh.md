## 语言概览

本文档描述了本仓库所实现的 LKR 语言（解析器、求值器、语句、类型以及标准库的对接）。

注释
- 行注释：`// ...`
- 块注释：`/* ... */`

标识符
- 由字母、数字、`_` 与 `-` 组成。关键字保留不可用。（注意：词法分析器允许标识符中包含 `-`。）

字面量
- 字符串：`"..."` 或 `'...'` 的 UTF‑8 字符串。支持转义 `\n \r \t \\ \" \' \$ \0`。
- 原始字符串（Rust 风格，无转义/插值）：`r"..."`、`r#"..."#`、`r##"..."##`（可多行）。
- 整数：64 位有符号；支持前导符号。（浮点数支持科学计数法。）
- 浮点数：64 位浮点，支持科学计数法。
- 布尔：`true`、`false`
- 空值：`nil`

集合
- 列表：`[a, b, c]`（允许异质元素）。索引：`list[0]`。通过标准库/元方法提供安全访问辅助。
- 映射：`{ key: value, ... }`。键是求值表达式，运行期会被转为字符串（string/int/float/bool）；访问可用 `map.key` 或 `map["key"]`。

模板字符串
- 仅在普通引号（`"..."` 和 `'...'`）内用 `${expr}` 做插值。
- 原始字符串不支持插值。
- 示例：`"Hello, ${user.name}!"`，`"Sum: ${1 + 2}"`。

输入与变量
- 不再存在隐式的运行期上下文。标识符必须在词法作用域中定义（例如通过语句中的 `let`、函数参数或导入）。
- 通过标准库显式读取外部输入：`io.read()`（字符串）。解析请手动调用 `json.parse(...)`、`yaml.parse(...)`、`toml.parse(...)`。
- 示例：`import io; import json; let data = json.parse(io.read()); return data.req.user.id == 1;`

函数调用与方法
- 可调用任意表达式：`f(x, y)`，`(g)(z)`。
- 属性访问：`expr.field` 或 `expr[expr]`。可选链：`expr?.field` 与 `expr?[index]`。
- 方法语法糖：`value.method(args...)` 的分派规则：
  1) 若 `value.method` 产生可调用对象（闭包/原生），则直接调用；
  2) 否则按值的运行时类型分派已注册的元方法，并将接收者作为第一个参数（如：`"abc".len()`；参见标准库）。

闭包
- 仅表达式形式：`|a, b| a + b`。

区间
- `a..b` 与 `a..=b` 在求值时产生整数列表（末端分别为开区间/闭区间）。亦可用于模式。

空合并与三元
- `lhs ?? rhs`：若 `lhs` 非 `nil` 则为 `lhs`，否则为 `rhs`。
- `cond ? then : else`（右结合）。在表达式中 `cond` 必须是布尔；在 `if`/`while` 中使用“真值”语义（见下）。

## 运算符（按优先级）
- 后缀：调用 `()`、点 `.field`、索引 `[expr]`、可选链 `?.field`、可选索引 `?[expr]`
- 一元：`!`（逻辑非）
- 乘法：`* / %`
- 加法：`+ -`
- 区间：`.. ..=`
- 比较/成员：`== != < > <= >= in`
- 逻辑：`&& ||`
- 空合并：`??`
- 三元：`? :`（表达式运算符中最低）

注意
- `+` 支持字符串与字符串拼接。其它“字符串/数字”混合受功能门控，默认未启用。
- `in` 支持：子串（`str in str`）、列表成员、映射键存在性。对 `list in list`，检查左侧所有元素是否都包含于右侧。

## 表达式
- 字面量、列表、映射、变量、调用、属性/索引访问、闭包、区间、逻辑/比较、`??` 与 `?:`。
- 并发表达式（功能门控 `concurrency`）：
  - `spawn(fn_or_closure)` → Task
  - `chan(capacity?, type?)` → Channel（类型为如 `"Int"` 的字符串）
  - `send(channel, value)` → Bool
  - `recv(channel)` → `[ok, value]`
  - `select { case recv(c) => expr; case send(c, v) => expr; default => expr }`

匹配表达式（Match）
- `match value { pattern => expr, ... }`（分隔符可用 `,` 或 `;`）。返回匹配分支的值。模式见下。

## 模式（Patterns）
用于 `match`、`if let`、`while let` 与 `let` 解构。
- 字面量：`1`、`3.14`、`"x"`、`true`、`nil`
- 变量绑定：`name`
- 通配：`_`
- 列表解构：`[p1, p2, ..rest]`
- 映射解构：`{ "key": pat, other: pat, ..rest }`（键可为字符串字面量或标识符；`rest` 绑定剩余字段）
- 或模式：`p1 | p2 | p3`
- 带守卫：`pat if expr`
- 区间模式：`1..10`、`0..=n`

for 循环模式
- 支持扩展形式：
  - 变量：`x`
  - 忽略：`_`
  - 元组：`(a, b, c)`
  - 数组：`[a, b, ..rest]`
  - 对象：`{ "k": v, ... }`（字符串键）

## 语句
- 程序由语句序列组成。分号 `;` 结束简单语句与表达式语句。

控制流
- `if (cond) stmt` 或 `if cond stmt`（括号可选）。真值语义：`false` 与 `nil` 为假，其余为真。
- `if let pattern = expr stmt [else stmt]`
- `while (cond) stmt` 或 `while cond stmt`
- `while let pattern = expr stmt`
- `for pattern in expr stmt`，其中 `expr` 可迭代：列表、字符串（字符）、或映射（迭代 `[key, value]`）。
- `break;`、`continue;`
- `return;` 或 `return expr;`

变量
- 声明/解构：`let pattern [: Type] = expr;`
- 赋值：`name = expr;`
- 复合赋值：`name += expr;`、`-=`、`*=`、`/=`、`%=`
- 简写定义：`name := expr;`（定义并初始化）
- 词法作用域：块 `{ ... }` 引入新作用域。

结构体（Struct）
- 定义：`struct User { id: Int, name: String? }`
- 实例化（字面量）：`User { id: 1, name: "Ann" }`
- 实例化（构造语法糖）：`User(id: 1, name: "Ann")`
- 访问：`user.name`

函数
- 定义：`fn name(param1[: Type], param2[: Type]) [-> Type] { statements }`
- 参数与返回类型可选；函数默认返回 `nil`，除非显式 `return`。
- 一等公民：闭包与函数值可被传递、返回与调用。
- 具名参数通过位置参数后的尾随块声明，例如 `fn f(a, b, { flag: Bool = true, label: String }) { ... }`。
- 默认值仅在调用方省略该具名参数时惰性求值，表达式可以访问同一调用帧内已绑定的其它参数。
- 调用时使用 `name: expr` 语法置于位置参数之后，如 `f(1, 2, label: "demo", flag: false)`；具名参数之间无顺序要求，但不得夹在位置参数之前。

导入
- 形式：
  - `import math;` —— 将标准库模块作为命名空间导入
  - `import "path/to/file.lkr";` —— 将文件模块作为命名空间导入（命名为文件名的主干）
  - `import { abs, sqrt } from math;` —— 挑选条目导入
  - `import { f as g } from "m.lkr";` —— 带别名
  - `import * as m from math;` —— 命名空间别名
  - `import math as m;` —— 模块别名

- 文件导入解析与安全：
  - 仅允许相对且净化后的路径：拒绝绝对路径与任何包含 `..` 的路径。
  - 解析顺序：优先尝试 `${MOD_NAME}.lkr`，若不存在再尝试 `${MOD_NAME}/mod.lkr`（相对于当前工作目录）。
  - 若传入已带 `.lkr` 的相对路径（如 `"lib/foo.lkr"`），在存在时将被直接使用。

内建与标准库
- 内建全局：`print(fmt, ...args)`、`println(fmt, ...args)`、`panic([msg])`。
- 标准库模块（按需导入）：`math`、`string`、`list`、`map`、`iter`、`datetime`、`os`、`tcp`。启用 `concurrency` 功能后：`task`、`chan`、`time`。
- `iter` 模块要点：`enumerate(list)`、`range([start,] end [, step])`、`zip(list1, list2)`、
  `take(list, n)`、`skip(list, n)`、`chain(list1, list2)`、`flatten(list)`、`unique(list)`、`chunk(list, size)`，
  以及通用高阶操作：`map(list, fn)`、`filter(list, fn)`、`reduce(list, init, fn)`。
- 元方法（可用 `value.method()` 直接调用而无需导入）：
  - 字符串：`len, lower, upper, trim, starts_with, ends_with, contains, replace, substring, split, join`
  - 列表：`len, push, concat, join, get, first, last, map, filter, reduce, take, skip, chain, flatten, unique, chunk, enumerate, zip`
  - 映射：`len, keys, values, has, get`

## CLI 输出行为
- REPL 与 CLI 仅在结果值非 `nil` 时打印输出。这样可以避免对默认返回 `nil` 的语句（如 `let`、函数定义、`println(...)` 等）多输出一行。若需要展示 `nil`，请显式调用 `println(nil)` 或在格式化输出中包含它。

## 类型与标注
原始与复合类型
- `Int`、`Float`、`String`、`Bool`、`Nil`、`Any`
- `List<T>`、`Map<K, V>`
- `Task<T>`、`Channel<T>`（并发）
- 函数类型：`(T1, T2) -> R`
- 联合：`A | B | Nil`；可选：`T?`（是 `T | Nil` 的语法糖；同时兼容前缀 `?T`）
- 支持命名与泛型类型（如 `List<Int>`、`Map<String, Int>`）

类型标注
- `let x: Int = 1;`
- `fn f(a: Int, b: String) -> Bool { ... }`
- 类型检查/推断尽力而为且保守；运行时仍为动态类型。

## 文法（EBNF 风格）

表达式（从低到高的优先级）
```ebnf
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

语句
```ebnf
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

模式
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
- 进入 REPL：`lkr`
- 执行脚本（语句）：`lkr FILE`
- 编译为字节码：`lkr compile FILE` → `FILE.lkrb`
- 仅允许相对且净化后的命令行路径
- CLI 仅在结果值非 `nil` 时打印输出


### 类型
- `String` —— UTF‑8 字符串
- `Int` —— 64 位有符号整数
- `Float` —— 64 位浮点数
- `Bool` —— 布尔值
- `Nil` —— 空/未定义
- `List` —— 有序集合
- `Map` —— 键值映射
- `Function` —— 一等函数
