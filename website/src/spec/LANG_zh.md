# 语言总览

本文件描述了本仓库实现的 LK 语言（解析器、求值器、语句、类型与标准库绑定）。

### 注释
- 行注释：`// ...`
- 块注释：`/* ... */`

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
- 映射：`{ key: value, ... }`。裸 key 为字符串键：`{name: "Alice", age: 30}` 等价于 `{ "name": "Alice", "age": 30 }`。键为运行时表达式，会被转为字符串（string/int/float/bool）。可用 `map.key` 或 `map["key"]` 访问。

### 模板字符串
- 仅支持在普通引号字符串中使用 `${expr}` 插值（`"..."` 与 `'...'` 都可）。
- 原始字符串不支持插值。
- 使用 `\$` 转义 `$`：`"Price: \$100"`。
- `println` 和 `print` 支持 `{}` 格式占位符：`println("{} + {} = {}", a, b, a + b)`。
- 示例：`"Hello, ${user.name}!"`、`"Sum: ${1 + 2}"`。

### 输入与变量
- 没有隐式运行时上下文。标识符必须在词法环境中定义（例如通过语句中的 `let`、函数参数或导入）。
- 通过标准库显式读取外部输入：`io.read()`（字符串）。手动解析：`json.parse(...)`、`yaml.parse(...)`、`toml.parse(...)`。
- 示例：`import io; import json; let data = json.parse(io.read()); return data.req.user.id == 1;`

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
- `in` 支持字符串子串判断 `str in str`、列表成员检查、映射键存在性。对于 `list in list`，会检查左侧所有元素都包含在右侧。

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
- 并发表达式（功能开关 `concurrency`）：
  - `spawn(fn_or_closure)` → Task
  - `chan(capacity?, type?)` → Channel（type 为字符串如 `"Int"`）
  - `send(channel, value)` → Bool
  - `recv(channel)` → `[ok, value]`
  - `select { case recv(c) => expr; case send(c, v) => expr; default => expr }`

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
- `for pattern in expr stmt`，其中 `expr` 可迭代：列表、字符串（字符）、映射（迭代 `[key, value]`）。
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
- 更新语法：`User { ..existing, field: value }` —— 从 `existing` 复制全部字段并覆盖指定值。

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
- 命名参数放在可选尾部块中：`fn f(a, b, { flag: Bool = true, label: String }) { ... }`。
- 默认值延迟在被调端计算；表达式可以引用其他参数。
- 调用时使用 `name: expr` 传入命名参数：`f(1, 2, label: "demo", flag: false)`。命名参数可任意顺序，但必须在位置参数之后。

### 导入
- 形式：
  - `import math;` —— 标准库模块作为命名空间。
  - `import "path/to/file.lk";` —— 文件模块作为命名空间（名称来自文件名）。
  - `import { abs, sqrt } from math;` —— 选择性导入。
  - `import { f as g } from "m.lk";` —— 带别名。
  - `import * as m from math;` —— 命名空间别名。
  - `import math as m;` —— 模块别名。

- 文件导入与安全：
  - 文件不会自动对外可见。跨文件依赖必须显式 `import`。
  - 引号路径导入不依赖 `Lk.toml`；按导入文件所在目录解析。
  - 路径仅允许相对路径，并经过清洗：绝对路径和任意 `..` 组件会被拒绝。
  - 解析顺序：`${MOD_NAME}.lk`，再 `${MOD_NAME}/mod.lk`（相对于当前文件目录）。
  - 如果引号路径已经包含 `.lk`（如 `"lib/foo.lk"`），需为相对路径且若存在则直接使用。
  - 在 package 中，裸模块导入先查标准库，再查 `Lk.toml` 的工作区/依赖包。包导入解析到 `src/mod.lk` 或 `src/<package-name>.lk`。
  - 由于拒绝 `..`，嵌套目录中的代码不能通过 `../...` 导入父目录文件；当子目录依赖树外文件时，请使用 package/workspace 模块。

#### 文件导入示例

```text
a.lk
b.lk
c/c1.lk
c/d/d1.lk
```

来自 `a.lk`：

```lk
import "b";       // b.lk，导出名为 b
import "c/c1";    // c/c1.lk，导出名为 c1
import "c/d/d1";  // c/d/d1.lk，导出名为 d1
```

来自 `c/c1.lk`：

```lk
import "d/d1";    // c/d/d1.lk，导出名为 d1
// import "../a"; // 被拒绝：父目录导入不允许
```

## 包
- `Lk.toml` 定义 `[package]`、`[dependencies]`、`[workspace]` 与 `[workspace.dependencies]`。
- 字符串依赖默认来自 GitHub，例如：`util = "owner/repo"`。
- `Lk.lock` 保存具体 revision 的已抓取 git 源码。
- 包管理命令和清单示例见 `docs/packages.md`。可运行的 workspace 示例见 `examples/lk-example-workspace`。

## 内置与标准库
- 全局内置：`print(fmt, ...args)`、`println(fmt, ...args)`、`panic([msg])`、`typeof(value)`。
- `typeof(value)` 返回运行时类型名字符串：`"Int"`、`"Float"`、`"String"`、`"Bool"`、`"Nil"`、`"List"`、`"Map"` 或结构体类型名。

### 标准库模块
按需导入：`math`、`string`、`list`、`map`、`iter`、`stream`、`datetime`、`os`、`io`、`json`、`yaml`、`toml`、`tcp`。LK 源码模块：`alg`、`collections`、`func`、`assert`、`math_ext`。启用 `concurrency` 后支持：`task`、`chan`、`time`。

- `math`：常量 `pi`、`e`、`inf`、`nan`、`max_int`、`min_int`、`max_float`、`epsilon`；函数 `abs`、`sqrt`、`floor`、`ceil`、`round`、`min`、`max`、`pow`、`exp`、`sin`、`cos`、`tan`、`asin`、`acos`、`atan`、`atan2`、`log`、`log10`、`log2`、`clamp`、`random`、`hypot`、`cbrt`、`sinh`、`cosh`、`tanh`、`trunc`、`fract`、`sign`、`to_int`、`to_float`、`is_nan`、`is_inf`。
- `string`：方法（见下方元方法）。
- `list`：方法（见下方元方法）。
- `map`：`map.len(m)`、`map.keys(m)`、`map.values(m)`、`map.has(m, key)`、`map.get(m, key)`、`map.set(m, key, val)`（返回更新后的映射）、`map.delete(m, key)`（返回 `[updated_map, removed_value]`）。
- `iter`：仅提供模块级列表工具：`range([start,] end [, step])`、`enumerate(list)`、`zip(list1, list2)`、`take(list, n)`、`skip(list, n)`、`chain(list1, list2)`、`flatten(list)`、`unique(list)`、`chunk(list, size)`，以及高阶操作 `map(list, fn)`、`filter(list, fn)`、`reduce(list, init, fn)`。
- `stream`：模块级懒执行管道。`stream.from_list(list)`、`stream.range(start, end)`、`stream.iterate(seed, fn)`、`stream.repeat(val)`、`stream.from_channel(ch)`、`stream.map(s, fn)`、`stream.filter(s, fn)`、`stream.take(s, n)`、`stream.skip(s, n)`、`stream.chain(a, b)`、`stream.subscribe(s)`、`stream.next(cursor)`、`stream.collect(stream_or_cursor)`、`stream.next_block(cursor[, timeout_ms])`、`stream.collect_block(stream_or_cursor[, n][, timeout_ms])`。
- `datetime`：`now()`（微秒）、`format(secs, fmt)`、`parse(str, fmt)`、`add(secs, delta)`、`sub(secs, delta)`、`day_of_week(secs)`、`day_of_year(secs)`、`is_weekend(secs)`。
- `os`：`hostname()`、`arch()`、`os()`、`clock()`、`time()`、`epoch()`、`exit(code)`、`exec(cmd, args?, stream?)`、`env_get(key, default?)`、`env`、`dir_current()`、`dir_temp()`、`dir_list(path)`、`file_read(path)`、`file_write(path, content)`、`file_append(path, content)`、`file_exists(path)`、`file_size(path)`、`file_delete(path)`、`mkdir(path)`、`path_join(parts...)`、`path_sep()`。
- `io`：`io.read()`（stdin）、`io.stdin_read([bytpes])`、`io.stdin_read_line()`、`io.stdin_read_all()`、`io.stdout_write(s)`、`io.stdout_writeln(s)`、`io.stdout_flush()`、`io.stderr_write(s)`、`io.stderr_writeln(s)`、`io.stderr_flush()`。
- `json`：`json.parse(string)`。
- `yaml`：`yaml.parse(string)`。
- `toml`：`toml.parse(string)`。
- `tcp`：`tcp.connect(host, port)`、`tcp.bind(host, port)`、`tcp.accept(listener)`、`tcp.write(conn, data)`、`tcp.read(conn, len?)`、`tcp.close(conn)`。
- `time`（并发）：`time.now()`、`time.sleep(ms)`、`time.timeout(ms)`、`time.after(ms)`、`time.since(start, end)`。

#### LK 源码标准库模块
这些模块用 LK 语言本身编写，补充 Rust 原生模块的算法、数据结构和工具：

- `alg`：排序（`insertion_sort`、`merge_sort`、`quick_sort`）、搜索（`binary_search`、`linear_search`、`bisect`）、经典算法（`gcd`、`lcm`、`is_prime`、`sieve`、`fib`、`factorial`、`comb`、`pow_int`）、字符串算法（`kmp_search`、`kmp_table`）、`shuffle`、`bisect`。
- `collections`：`stack`/`stack_push`/`stack_pop`/`stack_peek`/`stack_is_empty`/`stack_len`、`queue`/`queue_push`/`queue_pop`/`queue_peek`/`queue_is_empty`/`queue_len`、`set`/`set_add`/`set_remove`/`set_has`/`set_values`/`set_len`/`set_union`/`set_intersection`/`set_difference`/`set_symmetric_difference`、`heap`/`heap_push`/`heap_pop`/`heap_peek`/`heap_len`/`heap_is_empty`、`deque`/`deque_push_front`/`deque_push_back`/`deque_pop_front`/`deque_pop_back`/`deque_peek_front`/`deque_peek_back`/`deque_len`/`deque_is_empty`。
- `func`：`compose`、`pipe`、`compose_all`、`pipe_all`、`curry2`、`curry3`、`partial1`、`partial2`、`id`、`constant`、`complement`、`both`、`either`、`iterate`、`unfold`、`scan`、`group_by`、`count_by`、`partition`、`flat_map`、`zip_with`、`memoize`、`tap`。
- `assert`：`assert(cond, msg?)`、`assert_eq(actual, expected, msg?)`、`assert_ne(actual, expected, msg?)`、`assert_nil(value, msg?)`、`assert_not_nil(value, msg?)`、`assert_approx(actual, expected, epsilon?, msg?)`、`assert_false(cond, msg?)`。
- `math_ext`：`ext_gcd`、`pow_mod`、`mod_inverse`、`totient`、`divisor_count`、`divisor_sum`、`perm`、`collatz_len`、`triangular`、`pentagonal`、`hexagonal`、`is_perfect`、`catalan`、`sign`、`clamp`、`lerp`、`inverse_lerp`、`map_range`。从 `alg` 重新导出 `gcd`、`lcm`、`is_prime`、`factorial`、`comb`。

### 元方法（可直接通过 `value.method()` 使用，无需导入）
- String：`len`、`lower`、`upper`、`trim`、`starts_with`、`ends_with`、`contains`、`replace`、`substring`、`split`、`join`、`reverse`、`repeat`、`chars`、`char_at`、`byte_at`、`find`、`is_empty`、`format`
- List：`len`、`push`、`set`、`concat`、`join`、`get`、`first`、`last`、`map`、`filter`、`reduce`、`take`、`skip`、`chain`、`flatten`、`unique`、`chunk`、`enumerate`、`zip`、`to_stream`、`sort`、`reverse`、`pop`、`insert`、`remove_at`、`contains`、`index_of`、`slice`、`is_empty`
- Map：`len`、`keys`、`values`、`has`、`get`、`set`、`delete`
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
- `List<T>`、`Map<K, V>`
- `Task<T>`、`Channel<T>`（并发）
- 函数类型：`(T1, T2) -> R`
- 联合类型：`A | B | Nil`；可选类型：`T?`（`T | Nil` 的语法糖）
- 支持命名与泛型类型（如 `List<Int>`、`Map<String, Int>`）

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
             | closure | spawn | chan | send | recv | select | match | struct_lit
closure     ::= '|' [id {',' id}] '|' expr
             | '|' [id {',' id}] '|' '{' statement* '}'
template    ::= string_with_${...}
field       ::= id | int | string
list        ::= '[' [ (expr | '..' expr) { ',' (expr | '..' expr) } [ ',' ] ] ']'
map         ::= '{' [ (id | string) ':' expr { ',' (id | string) ':' expr } [ ',' ] } '}'
var         ::= identifier
paren       ::= '(' expr ')'
args        ::= [ expr { ',' expr } [ ',' name ':' expr { ',' name ':' expr } ] ]
struct_lit  ::= id '{' [ '..' expr ',' ] id ':' expr { ',' id ':' expr } '}'
             | id '{' '}'
```

### 语句
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
while_let_stmt ::= 'while' 'let' pattern = expr statement
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

## CLI 使用说明
- 运行 REPL：`lk`
- 执行文件（语句）：`lk FILE`
- 编译为可执行模块产物：`lk compile [FILE]` -> `FILE.lkm`
- 执行模块产物：`lk FILE.lkm`
- 编译为 LLVM 可 native lowering 形状的 native 可执行文件：`lk compile exe [FILE]`
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
- `Function` —— 一等函数
- `Object` —— 结构体实例（含类型名与字段）
- `Task` —— 并发任务句柄（功能开关）
- `Channel` —— 并发通道（功能开关）
- `Stream` —— 懒执行流管线（功能开关）
- `StreamCursor` —— 流元素消费器
