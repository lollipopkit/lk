# LK 语义裁决(golden vectors)

本文档是 VM 与 native(AOT)行为分歧时的**第三仲裁**:差分测试
(`cli/tests/aot_differential_test.rs` 等)只能锁定"双方一致",不能回答
"哪一方是对的"。这里逐条写下已裁决的语义与期望输出;修改任何一条都必须
是显式的语言决策,而不是实现巧合。

除特别注明外,每条的期望输出都用当前 VM 实测值锁定,并被差分语料
(手写 69 例 + `examples/` 语料 + 生成式 fuzz)持续验证与 native 一致。

## 数值

| 程序 | 期望 stdout | 期望退出 | 说明 |
|------|-------------|----------|------|
| `return 7 % 3;` | `1` | 成功 | `%` 是 Int→Int(截断取余,同 Rust `%`) |
| `return 20 / 4;` | `5` | 成功 | `/` 对 Int/Int **返回 Float**;Float 值为整数时显示省略小数部分 |
| `return 7 / 2;` | `3.5` | 成功 | 同上 —— 值也是 Float,不只是类型 |
| `return 1 / 0;` | `inf` | 成功 | `/` 是浮点除法,除零按 IEEE 给 inf/NaN;`%` 对 Int 除零仍 raise |
| `return 1.0 / 7.0;` | `0.14285714285714285` | 成功 | Float 显示 = Rust `f64` 的 `Display`(VM-exact,native 侧经 `lkrt_f64_to_str` 逐字节对齐) |
| `return 5 + 7.5;` | `12.5` | 成功 | Int/Float 混合算术提升为 Float |
| `let a = 9223372036854775807; return a + 1;` | `-9223372036854775808` | 成功 | Int 溢出**回绕**,不 raise;两端一致(2026-07-29 核对) |
| `math.abs(-9223372036854775808)` | `-9223372036854775808` | 成功 | 同一条回绕规则 —— 没有正的 `Int::MIN`。此前 `i64::abs` panic,进程 abort |

注:`/` 产 Float 是整数中点必须写成 `math.floor((lo + hi) / 2)` 的原因
(VM 侧 lower 为 `MidInt`)。更一般地,`math.floor(a / b)` **就是**整数除法
——语言里没有别的写法——所以它 lower 为单条 `FloorDivInt`(向下取整,
`math.floor(-7 / 2)` 是 `-4`;非 Int 操作数按 f64 除后取整)。

这条规则曾经只有类型检查器和常量折叠认,两个执行器都做整数除法,
于是同一个表达式字面量给 `3.5`、变量给 `3`;折叠还会看值定类型
(`20 / 4` 折成 Int,`7 / 2` 折成 Float)。四条路径现已一致。

## 位运算与移位

| 程序 | 期望 stdout | 期望退出 | 说明 |
|------|-------------|----------|------|
| `return 3 << 8;` | `768` | 成功 | `<<` 是 Int→Int |
| `return (0 - 16) >> 2;` | `-4` | 成功 | `>>` 是**算术**移位:Int 有符号,符号位复制 |
| `return 1 << 2 + 3;` | `32` | 成功 | 优先级同 Rust:紧于比较、松于 `+` |
| `let n = 64; return 1 << n;` | (空) | 失败 | 移位量必须在 `0..=63`,否则报错 |

移位量越界是**错误**,不是掩码、也不是回绕。硬件会把它掩成 63 并给出一个
没人要的数,Rust 在 debug 下 panic,C 说是未定义 —— 三者里只有报错在每个
目标上说同一句话。两个后端的报错文本一致:VM 在 `__lk_shl`/`__lk_shr`
内建里检查,native 走 `lkrt_i64_sh*_checked`。

词法上 `<<` / `>>` **不是**独立 token,而是两个相邻的比较 token,在表达式
文法里成对识别 —— 否则 `Map<String, List<Int>>` 结尾的两个 `>` 就会被吃掉
(Rust 有同样的问题,做法是在类型位置把 token 拆回来)。加空格写成
`1 < < 3` 不是移位,是语法错误。

## 响亮失败(loud failure)

失败路径的契约是**响亮失败 + stdout 为空**;具体退出机制不作为契约:
VM 以 `exit 1` + stderr 错误信息结束,native 以 guard `abort()`(SIGABRT,
壳层显示 134)结束。差分测试只比较 `success()` 与 stdout,不比较退出码数值
与 stderr 文本。

| 程序 | 期望 | 说明 |
|------|------|------|
| `let x = 2; let y = 0; return x / y;` | 失败,stdout 空 | 整数除零。native 侧禁止直接依赖 LLVM `sdiv` UB,必须走 `lkrt_i64_div_checked` guard |
| `x % 0` | 失败,stdout 空 | 整数模零,同上 |
| `1.0 / 0.0` | 失败,stdout 空 | 浮点除零是响亮失败,**不是** IEEE `inf`(native guard 与 VM 对齐) |
| `let m = {"a": 1}; return m["z"] + 1;` | 失败,stdout 空 | 缺失值(nil)参与算术 = halt。VM 报 `Add expected numbers…got Nil`,native abort |

## nil 与缺失值

| 程序 | 期望 stdout | 说明 |
|------|-------------|------|
| `return nil;` | (空) | **nil 返回值静默**。曾有真实分歧:legacy native 打印 `nil` 而 VM 静默,差分抓出后裁决为 VM 行为 |
| `let m = {"a": 1}; return m["z"];` | (空) | 缺失键读返回 nil,按 nil 返回处理(native 侧为 `Maybe` present-bit 模型) |
| `let xs = [10]; return xs[9];` | (空) | 列表越界读返回 nil,不是错误 |
| `xs[9] == nil`(越界) | `true` 分支 | nil 判等测试 present 位,可用于探测缺失 |

## 索引

| 程序 | 期望 stdout | 说明 |
|------|-------------|------|
| `let xs = [10, 20, 30]; return xs[-1];` | `30` | 负索引从尾部计数 |
| `let xs = ["a"]; return xs[5];` | (空) | 字符串列表越界同样返回 nil |

## 语法边界(影响差分语料生成器)

- `if` / `while` / `for` 的条件(被迭代对象)都**不需要**括号,写了也行 ——
  括号只是一个表达式,解析后即剥掉。三者一律扫到**顶层** `{` 为止,所以条件里
  要写结构体字面量或 map 字面量得自己加一层括号。
- 语句以 `;` 结尾,**但以 `}` 收尾的表达式语句不需要** ——
  `match x { … }`、`unsafe { … }`、`if c { … }` 作语句时都不用分号,写了也行。
  作为操作数时不适用:`return match x { … } == nil;` 比较的是 match 的值。
- `try`/`catch`、`select`、`go`、后缀 `!` 均为 **parse 时糖**(分别降到隐藏
  native `try$call`、`select$block`、`spawn(闭包)`、nil 检查 Conditional),
  不存在专用 AST 节点;`select`/并发语义见 `docs/concurrency.md`。
- **后缀 `!`(force unwrap)**:`expr!` 在 nil 时 raise "unwrap of nil value"
  (可 catch),否则原值。两条边界:`!` 紧跟 `(`/`[`/`{` 是**宏调用**语法
  (`name!(...)`),解包后调用/索引需加括号 `(x!)(...)`;lexer 贪婪 `!=`→Ne,
  `x!==1` 是 parse 错误,写 `x! == 1`。
- **`?.` 可以调方法**:`a?.m(args)` 在 parse 时脱糖成
  `{ let t = a; t == nil ? nil : t.m(args) }` —— 接收者只求值一次,
  为 nil 时**调用根本不发生**,结果类型是 `T?`。此前只有字段访问
  (struct / map)走得通,`s?.len()` 会把 `OptionalAccess` 当成索引,
  运行时报 "String index must be Int"。
- **可能落空的分支,类型是 `T?`**:`match` 无匹配时给 nil、`if` 没有
  `else` 时给 nil —— 这是运行时规则,类型必须跟着说。所以
  `let r: String = match x { 1 => "one" };` 是类型错误(它是 `String?`),
  `let r = if c { "a" };` 也是 `String?` 而不是"String 与 Nil 冲突"。
  判定"不会落空":match 有无守卫的 catch-all(`_` 或绑定名),或者被匹配
  值是 Bool 且两个字面量都在;`if` 有 `else`。判定是**保守的**:
  判错方向的代价是多写一个 `?`,反方向的代价是 String 变量里装着 nil。
- **`if` 是表达式**:`if c { a } else { b }` 取所在分支块的最后一个表达式为值;
  没有 `else`、或分支块以语句结尾,值为 nil。`else if` 链按嵌套展开。它与
  `match`、三元 `? :` 降到同一个节点(`Expr::Conditional`),所以三者不会走散。
  两条边界:(1) 条件与 `match` 的被匹配值一样,在第一个**顶层** `{` 处截止,
  条件里要写结构体字面量得加括号;(2) 分支里的 `return`/`break`/`continue`
  仍是控制流,不是值 —— 因此语句位置的 `if` 依旧按语句解析,只有**块尾**的
  `if` 才作为块的值。
- **条件是 truthiness,不是 `Bool`**:`if`/`while`/`? :` 的条件接受任何值,
  只有 nil 和 false 为假。以前 `? :` 单独要求 `Bool`,与 `if` 不一致
  (`fn g(x) { return x ? "y" : "n"; }` 报错而 `if x` 不报),现已统一。
- **一元 `-`**:`-expr` 是真取负,不是 `0 - expr` 的糖——浮点有两个零,
  `-(0.0)` 是 `-0.0` 而 `0.0 - 0.0` 是 `+0.0`。字面量的负号仍由 lexer 折叠
  (这是 `-9223372036854775808` 唯一的写法,它的绝对值放不进 i64),两条路径
  产出同一棵 AST。操作数须是 Int / Float / 有符号机器整数;无符号取负报错。
- **v2 错误模型**:错误一律 **raise**(Swift 式),try/catch 是唯一捕获面,
  无用户级 `pcall`、无 `[ok, value]` 状态对。`error(v)` 抛一等错误值;
  并发原语失败即抛(`recv`/`send` on closed),非错误的"暂无"用 nil 表达
  (`chan.try_recv` 空、`task.try_await` 未完成),配合 `!` 断言。

## 闭包与捕获

| 程序 | 期望 stdout | 说明 |
|------|-------------|------|
| `let k = 3; let f = \|x\| x * k; k = 5; println(f(1));` | `5` | **捕获是共享可变 cell**:闭包创建后对被捕获变量的赋值对闭包可见(native 在调用点解析 cell 当前值) |
| `let f = \|x\| x + 1; println(f(1)); f = \|x\| x * 10; println(f(2));` | `2` `20` | 闭包变量重绑定按程序序生效 |
| `let i=0; while (i<3) { let f=\|x\| x+i; println(f(10)); i=i+1; }` | `10` `11` `12` | 循环体内捕获**循环外变量**:cell 在循环入口预提升,单一共享 cell,条件/自增读也走 cell(曾因 mid-body promotion 在第 2 迭代报 "expected Int, got Obj") |
| `for i in 0..3 { let f=\|x\| x+i; println(f(10)); }` | `10` `11` `12` | **for 循环变量**捕获为每站点快照 cell(fused 循环 opcode 驱动原始寄存器,不可重绑);快照是 copy 而非 move(曾把计数器 move 成 Nil) |
| 循环内 `g = \|x\| x+i` 逃逸循环后调用 | 共享 cell 终值 | native 侧跨迭代闭包 ref 逃逸响亮拒绝(ref 一致性在 loop header 处终止) |

## 模块与 IO

| 程序 | 期望 stdout | 说明 |
|------|-------------|------|
| `datetime.now()` | — | 返回 Unix epoch **秒**(非微秒;datetime_demo 曾因此假设而自身断言失败) |
| `std.write(out, "a")` | `a`,返回 `1` | `write`/`writeln` 返回写入字节数(writeln 含换行 = len+1);`flush` 恒返回 `true` |
| `std.write` 与 `println` 交错 | 程序序 | **stdout 顺序契约**:native 侧 Rust 写者先 `fflush(NULL)` 再写、写后 flush 自身流,保证与 C `printf` 缓冲的输出保持程序序 |
| `math.sqrt(-4.0)` | 响亮失败 | 负参是致命错误(双方 loud),不是 NaN |

## 容器 display

| 程序 | 期望 stdout | 说明 |
|------|-------------|------|
| `println([1,2,3])` | `[1,2,3]` | 逗号分隔无空格;float 元素用 Rust `to_string`(`2.0`→`2`) |
| `println(["a","b c"])` | `["a","b c"]` | 字符串元素 **Rust `{:?}` 引号+转义**(`"`→`\"`、tab→`\t`) |
| `println("${xs}")`(xs 是 list) | 响亮失败 | **两条 display 路径**:print/println/panic/assert 消息走 stdlib `runtime_display`(容器可显示);`ToString`/模板插值/`+` 拼接走 exec `runtime_value_display_string`(标量 only,容器 loud error)。native 对后者拒绝编译 |
| `println(map)` | hash 迭代序 | map display 顺序 = 底层 hash map 迭代序,**跨运行稳定但不可移植**(依赖 hasher+增长历史)——native 侧不进子集,响亮拒绝 |

## `unique()` 等值语义(2026-07-29 修订)

`list.unique()` 与 `==`、`in` **用同一条规则**:数值按值(`1 == 1.0` 去重、
`0.0 == -0.0` 去重、NaN 永不去重所以一串 NaN 原样保留)、字符串按内容
(不分长短)、列表/map 按结构。native 侧 `lkrt_lklist_dyn_unique` 直接调
`dyn_eq_inner`,与 `==` 同一个函数。

typed Float 列表仍是**单次哈希查找**而非 O(n²) 扫描:键在插入前规范化
(NaN 给一个递增序号,零统一成 `+0.0` 的位型),所以既跟得上 `==` 又没丢
性能。

此前这里是**第三套 eq**:VM 按 `to_bits`(`0.0 != -0.0`、NaN 自等),
native 另有一份 `unique_eq`(数值 to_bits、>7 字节字符串永不相等、
列表按句柄)。后者写的是**当时**的 VM;VM 的相等后来改成 heap-aware,
这份没跟上,于是 `[s, s].unique()`、`[[1], [1]].unique()` 两条后端答案
不同 —— 而它们恰好被这份文档划在差分子集之外,所以没人发现。现已并入
`cli/tests/aot_differential_test.rs` 的 `differential_equality_and_unique`。

## `in` 操作符等值语义(2026-07-29 修订)

`needle in list` 与 `==` **用同一条规则**:Int/Float 跨类型按数值比较
(`1 in [1.0]`、`1.0 in [1, 2]` 均 true),String 按内容(长短一致),
其余按 `runtime_values_equal`。native 侧新增 `list_h.i64_contains_f64` /
`f64_contains_i64` 两个 helper 与之逐条对齐。

此前这里是**第三套 eq**:typed 列表要求 needle 与元素同变体,Mixed 列表却
按值比较——于是 `a == b` 为 true 而 `a in [b]` 为 false,且答案取决于列表的
内部表示(程序看不见的东西)。同期常量折叠对 `==` 走 `LiteralVal` 的 derive
`PartialEq`(结构相等),所以 `println(1 == 1.0)` 是 false 而变量版是 true,
还与折叠器自己的序比较矛盾(`1 <= 1.0 && 1 >= 1.0` 折成 true)。

`in` 的类型检查此前只认 List/Map/Set,漏了 `String`(含子串)和 `Tuple`
(异构列表**字面量**推出来的类型)——两者在索引、`len()`、方法分发处都是
容器。于是 `"a" in "abc"` 作为字面量折叠可用,换成变量就是类型错误。

长字符串/嵌套列表的句柄同一性限制与 unique() 同款(intern/转换边界,
已留档,不进差分子集)。

`==`、`in`、`unique()` 三者现已同规则,常量折叠亦然。

## 值遍历深度(2026-07-29 裁决)

比较和渲染都按值的**形状**递归,深度就是数据的嵌套深度。一个循环就能造出
超过 Rust 栈的链:

```lk
let node: Any = [1];
for i in 0..200000 { node = [node]; }
```

裁决:**脚本不能让进程 abort**。超过 `MAX_VALUE_DEPTH`(512)时,比较和
渲染 raise 普通的可捕获错误(Python / Lua 同款)。512 对数据足够宽松 ——
JSON 嵌套是个位数,手写树是几十层。

GC **没有**这个上限,也不能有:回收不允许失败。`HeapStore::collect` 用显式
工作表标记,深度不上 Rust 栈。

## 结构体 display 字段序(2026-07-29 裁决)

`println(p)` 的字段按 **`struct` 声明的顺序**,与构造时写的顺序无关:

```lk
struct Range { start: Int, end: Int }
println(Range { end: 9, start: 1 })   // Range{start:1,end:9}
```

字段存在 hash map 里,所以此前顺序是 hasher 的 —— `struct Range { start, end }`
先打 `end`,而且换个 hasher 会静默重排每一个结构体。声明顺序随类型走
(`DeclaredType::fields`,编译期从 `Stmt::Struct` 收进 `TypeInfo.structs`,
`MODULE_ARTIFACT_VERSION` 15)。够不到声明时(别的模块的结构体、host 造的
对象)按字段名排序 —— 任意但稳定,hash 序两样都不是。

字段值是容器里的数据,**加引号**,和列表元素、map 值一致:
`P { name: "a, b" }` 打印成 `P{name:"a, b"}`,此前是 `P{name:a, b}`(读起来
像两个字段)。

## map 键与 set 成员(2026-07-29 裁决)

**`Set` 是 map 的键集,回答同一个问题**:只有 nil / Bool / Int / String 能
做键。Float 不行(`0.0` 与 `-0.0` 相等但哈希不同,NaN 不等于自己),容器
也不行(可变的东西做键,改了它就找不回那条记录)。

此前这里是**两套转换**且分歧就在这一点上:`m[k] = v` 走执行器那份,拒绝
列表;`s.add(...)` / `Set([...])` 走容器方法那份,接受为 `Obj(handle)`,
按**句柄**比较。于是 set 悄悄留下了它永远找不回的成员:

```text
let s = Set([]);
s.add([1, 2]);  s.has([1, 2])   → false
s.add([1, 2]);  s.len()         → 2
println(s)                      → Set([<object:80>,<object:82>])
```

现在只有一份(`RuntimeMapKey::from_value`),两条路都拒绝,错误文本相同。

统一之后 `RuntimeMapKey::Obj(HeapRef)` 就**没人造得出来**了,已删除
(`MODULE_ARTIFACT_VERSION` 15 一并覆盖)。于是这条规则变成了结构性的:键
一律自包含,不带堆句柄 —— set 因此**完全没有 GC 出边**,键跨堆搬运就是
`clone()`,两侧原本各有一份追句柄的翻译函数也一并没了。

## 字符串序比较(2026-07-29 裁决)

`"a" < "z"` 可用,按**字节字典序**,长短字符串一视同仁 —— 与 `list.sort()`
的排序、常量折叠的 `cmp_literal_ordering`、执行器 `number_compare` 的字符串
分支都是同一条规则。混合类型仍然拒绝(`1 < "a"` 是类型错误)。

此前只有**类型检查器**不许:运行时一直支持,折叠器一直支持,于是问"哪个
字符串在前"的唯一办法是排一个两元素列表。native 侧那条禁令的注释还写着
"VM 只支持字符串的 ==/!=" —— 从来不是真的。`lkrt_str_cmp` 返回 -1/0/1,拿
**同一个**运算符跟 0 比就同时实现了六种,所以放开禁令即可。

差分语料:`differential_equality_and_unique` 的 `str_lt_long` /
`str_ge_long` / `str_le_equal` / `str_gt_prefix`。

## 源缩短之后的窗口(2026-07-29 裁决)

窗口(`xs.slice(a, b)`)不复制,所以源可以在它脚下变短。裁决:**窗口按源
此刻还够得着的部分算长度**(`SliceValue::live_len`),越界读仍然给 nil ——
和语言其他地方一样,不报错。

此前每个读者各答各的。一个长度为 3 的窗口,源 `pop()` 掉最后一个之后:

```text
s.len()     → 3        s.to_list()  → [1,2,nil]
println(s)  → [1,2]    s.last()     → nil
s == [1,2]  → false    s.get(2)     → nil
```

一个问题六个答案。

## `List.clear()`(2026-07-29 补齐)

`clear()` 三个容器都有,原地清空、答容器本身(可链)。此前 map 和 set 有,
list 没有 —— 而 `docs/stdlib.md` 的方法表里写着它。签名表里还有一条测试
断言 list **不**该有,类型检查器里又有个特例分支给 list 的 `clear` 返回
`Nil`(而表里 map/set 是 `Self`)。三处各说各的。

三个 `clear` 都还不能原生降低,行为一致,不算新洞。

## 字符串的读取面按字符(2026-07-29 裁决)

`len()` 数字符、`s[i]` 取字符,所以 `slice` / `take` / `skip` / `first` /
`last` / `index_of` / `s[-i]` 全按字符。native 侧 `str.slice_chars` 一直就是
VM 的语义,补上降低即可。

修掉的两处两端不一致:

- **负下标从字节长度往回数**(两端都是,lkrt 的注释还把它当"VM 的 quirk"
  照抄了)。`"中文abc"` 五个字符九个字节,于是 `[-1]` 问的是第 8 个字符 →
  nil,`[-5]` 答 `"c"`。VM 里这条规则还抄了三份,其中两份靠 `usize` 下溢
  碰巧对。现在收在 `index_string_at` 一处,按字符回绕。
- **已删的 `substring` / `find` 方法**原生降到 `lkrt_str_substring` /
  `lkrt_str_find`,两者按**字节**;miss 时 native 给 -1 而 VM 给 nil。多字节
  文本上两端答案不同,而差分语料全是 ASCII,所以没人发现。方法删了,两个
  byte 版 helper 也删了。

`index_of` miss 给 **nil** 不给 -1:-1 是合法下标(最后一个字符),
`s[s.index_of(x)]` 会静悄悄答出最后一个字符而不是失败。

差分语料:`str_slice_multibyte`、`str_take_skip_multibyte`、
`str_index_of_multibyte`、`str_index_of_miss`、
`str_negative_index_multibyte`、`str_first_last_multibyte`。

`List` / `Slice` / `Bytes` 同此:miss 给 nil。差分语料
`index_of_miss_is_nil`。

## `try` 是表达式(2026-07-29 裁决)

`try { … } catch e { … }` 有值,和 `if`、`match` 一样:值是 body 末尾的表达式,
body raise 了就是 handler 末尾的表达式;以语句结尾的那一半没有值,给 nil ——
与 `if` 的分支同规则。类型是两半的并,一边 nil 一边不是就是 `T?`
(`unify_branch_values`,`if` 和它共用)。

语句位置**不变**:值被丢弃,`if`/`match` 在语句位置也是这样。而且语句位置
根本不分配值寄存器 —— 这不只是省一条指令:在保护区域**内部**写的值要靠
装箱进 cell 才能带出来,预留一个没人读的值会让
`try { f(); } catch e { … }` 整个掉出原生路径。

AST 里只有一个节点(`Expr::Try`),`Stmt::Try` 删了。语句位置是
`Stmt::Expr(Expr::Try)`,后续所有访问者本来就会递归进表达式,所以删掉语句
节点比留着两个**改动更少**。它仍然是真节点而不是 parse 期糖:当年从
`let [ok, e] = try$call(|| { body })` 改过来,就是因为那样每一层都看见的是
闭包和解构 `let`,body 里赋值的带标注局部变量出来会变成一个新类型变量。

块尾的 `try` 交给表达式解析器(`try_parse_tail_expression_stmt`),这是 `if`
早就有的机制,现在两者共用 —— 所以
`try { try { … } catch e { … } } catch e { … }` 里层也是值。

native 侧:cell 读回原本按**区域入口**的类型定型,而值寄存器进去是 nil
(`Ty::Nil` 没有 unboxer),于是整个表达式形式被拒。改成入口为 `Nil` 时按
`Dyn` 读回 —— body 本来就是按自己的类型装箱写进 cell 的,`Dyn` 才是诚实的
描述。顺带让 `let x = nil; try { x = 5; } catch e {}` 也能原生降低了,它此前
是直接拒绝的。

差分语料:`try_expression_value`、`try_expression_nil_branch`。

**已知边界**:同一函数里多个 try 区域时,值寄存器在汇合处的 phi 还可能定不
出类型(一边是 cell 带回的 `Dyn`,一边是 handler 写的具体类型),那种程序
会退回混合/Tier-0。

## 字符串转义(2026-07-29 补 `\u`)

认这些:`\n` `\r` `\t` `\\` `\'` `\"` `\$` `\0`,以及
**`\u{XXXX}`** —— 一到六位十六进制,写一个按码点指定的字符。
`\u{4e2d}` → `中`,`\u{1F600}` → 😀。代理区码点和超过 U+10FFFF 的报错,
不是静默产出一个不是字符的东西。

此前没有 `\u`,所以打不出来的字符(零宽连接符、不换行空格、星平面 emoji)
只能直接粘进源码 —— 而 `"\u{4e2d}"` 会原样打印它自己,因为:

**未知转义保留反斜杠,不报错。** 这是有理由的,不是宽松:regex 模式在 LK 里
就是普通字符串,`"\d+\s*"` 必须原样活到引擎手上。所以加 `\u` 是一个行为
变更(此前 `"\u"` 是字面量),不是纯新增。

## `sort()` 的序(2026-07-29 补容器)

数值按值(Int/Float 跨类型),字符串按内容(长短一致),**列表按字典序**
—— 逐元素比,前缀排在扩展它的东西前面,和 `==` 把它们当结构相等一致。
窗口(slice)当列表比。其余堆类型之间按**种类**排(String < Bytes <
List/Slice < Map < Set < Object < Callable < Error < 其他):map 和 map 之间
没有自然序,但分组排至少是确定的。

此前容器全是 `Obj`、同一个 rank,于是"相等" —— 排一个列表的列表**原样不动**:

```text
[[1,"b"], [1,"a"], [0,"c"]].sort()   → 不变
```

和"长字符串排不动"是同一个洞的两半。

深度超过 `MAX_VALUE_DEPTH` 时按种类排而不是报错:`sort_by` 要的是
`Ordering` 不是 `Result`,而且排到一半才 raise 会留下一个已经被重排过的
列表。这是全语言唯一一处深度上限**给答案**而不是**报告**的地方。

## channel 容量(2026-07-29 裁决)

`chan(0)` 是**无缓冲**,和每一个读者见过的 channel API 一致 —— 不是无界。
`chan(n)` 是容量 n,`chan(负数)` 报错。

此前 `capacity <= 0` 映射到 `None`,也就是**无界队列**:一个要"最强背压"的
程序拿到的是完全没有背压,队列一直涨到进程死掉。而 0 是唯一通向无界的写法,
所以它同时是最容易误写的那个数。

运行时的 mpsc 没有真正的会合(rendezvous)形式,所以 0 取它能给的最小上界
(1)。和会合的差别是"在途一个值",而另一头的备选是无界 —— 这个取舍值。
无界队列现在语言里够不到了,这是有意的:没有上界的队列是一个涨到进程死掉的
队列。

**`use chan;` 会遮蔽 `chan()` 全局**,因为模块名和构造函数同名。此前这是条
死路:导入模块之后**没有任何办法**创建 channel。现在模块里有
`chan.new(capacity[, type])`,和全局 `chan(…)` 共用一份实现 —— 导入之后用
模块拼写,不导入就用全局。

## 范围步长为 0(2026-07-29 裁决)

步长 0 是错误,**三种拼写一句话**:`Range step cannot be zero`。

此前只有两种说这句话:范围值(`let r = 0..3..0`)和 `iter.range(0, 5, 0)`。
第三种 —— `for i in 0..3..0` —— 静默跳过循环体然后往下走。原因是 `for` 的范围
不走 `NewRange`,而是特化降低:动态步长路径先算 `step > 0`,0 让它为假,于是
进降序分支比较 `0 > 3`,一次都不转。同一件荒谬事,写法不同待遇不同。

现在:字面量 0 **编译期**就拒(步长就摆在那儿,没有理由等到运行时);变量步长
在循环入口断言一次(`step != 0`,不成立就 raise),不进循环体。循环体里每转一次
的开销没有变化。

## `impl Type { … }`(2026-07-29 补)

方法可以直接挂在类型上,不必先有 trait。此前 `impl Type { … }` 是语法错误
("Expected 'for' in impl statement"),而语言也没有 UFCS(`fn f(s: S)` 不能
写成 `s.f()`)—— 于是给结构体加一个方法的唯一办法是声明一个**什么也不说的
trait** 再实现它:

```lk
trait Methods { }
impl Methods for Point { fn norm2(self) -> Int { … } }
```

机制本来就齐:分发按**目标类型**索引,不按 trait,所以缺的只是这个拼写。

固有 impl 和 trait impl 可以并存于同一类型:trait 说这个类型**承诺**什么,
固有块放它自己的东西。trait impl 的一致性检查不变(缺方法、签名不符、arity
不符都照报);固有 impl 没有承诺,所以不检查。

**trait impl 里不能出现 trait 没声明的方法**(同日补)。此前能 —— 而且不得
不能:`impl Type { … }` 是语法错误,方法只能住在 trait impl 里,于是程序声明
一个空 trait 把所有东西挂上去。现在类型能带自己的方法了,trait impl 里多出来
的方法就是个有明确改法的错误,报错直接说改法。这条让 trait 的方法列表重新
有意义:它列的就是全部。

`ImplDecl.trait_name` 因此变成 `Option`,`MODULE_ARTIFACT_VERSION` 16。

**用户方法名可以和内置方法同名**(同日补)。编译器把 `len`/`push`/`set`/
`split`/`join` 降低成专用 opcode 时只看**方法名**,那里还没有接收者的类型 ——
对列表和字符串是对的,对同名的结构体方法是错的:`s.len()` 答
"Len target object is not sized",另外四个带参数的更是在**编译期**就因为 arity
失败,方法根本写不出来。现在:凡是本程序里某个 `impl` 声明过的名字,都不再假定
是内置的,那些调用走普通动态分发。代价只落在给方法起了内置同名的程序上,而且
只落在那个名字上。

**一个 impl 块里不能重复定义同名方法**(同日补)。此前后者静默胜出,前者被
编译出来却永远到不了。语言里没有别的地方允许一个声明被它的兄弟遮蔽。

**内置容器的 impl 目标不能写元素类型**(同日补)。运行时按**擦除元素类型**
之后分发(`heap_dispatch_type` 把每个列表都报成 `List<Any>` —— 一个
`TypedList::Mixed` 也报不出别的),所以 `List<Int>` 和 `List<String>` 到的是
同一个分发口。写 `impl T for List<Int>` 直接报错并说改成 `List`。

而检查器此前按接收者的**静态**类型做键,于是 `impl T for List` 注册在
`List<Any>` 下、对 `[1,2]` 的调用查 `List<Int>` —— 方法存在却找不到,还在运行
时之前就被拒了。`String` 和 `Map` 能用只是因为它们不走这条路(`String` 无
参;`Map` 有"entries 即 fields"的旁路)。两边现在用同一个键。

## 错误文本(2026-07-08 裁决)

`catch e` 绑定的消息 = **裸 cause 文本**,无包装:native(Rust stdlib)函数
失败不再加 `"native `{name}` failed: "` 前缀(曾有,`map_native_error` 处
移除),与 `error(v)` 一等值对称;调用点归因由 traceback 承担,不进消息。

**跨 task 边界也不包装**(2026-07-29 补):`task.await` / `task.join_all` 曾
加 `"Failed to await task: "` 前缀,于是同一个失败在 task 里 raise 和在原地
raise 读出来是两个字符串 —— 而程序是可能按消息分支的。

**算术失败的文本两端逐字一致**(2026-07-29 补):除零、取模零、移位越界是
程序能 `catch` 并据以分支的东西,所以这几条手工对齐。此前 `a % b`(b=0)
VM 说 `ModInt divisor is zero`、native 说 `Division by zero` —— 两个不同的
字符串,而且都说错了是哪个运算符。

**其余跨后端错误文本不保证逐字一致**:VM 与 native 的错误生成机制不同
(如 `recv(999)` VM 报 "recv first argument must be a Channel"(类型检查),
native 报 "Channel not found"(id 查找))。差分语料因此**不打印 catch 到
的错误文本**,只断言 catch 行为(进入 handler、后续状态可用);若未来要
开放文本比对,需先逐条对齐两侧消息(fuzz 差分红为发现机制)。

## 错误文本说语言的话,不说实现的话(2026-07-29 裁决)

用户看得见的错误里不出现 **opcode 名**、**内部表示名**、**desugar 出来的
内部函数名**:

| 写的是 | 曾经说 | 现在说 |
|---|---|---|
| `a % 0` | `ModInt divisor is zero` | `modulo by zero` |
| `a % "s"` | `ModInt expected Int or Float, got …` | `% expects Int or Float, got …` |
| `-s` | `Neg expected Int or Float, got ShortStr` | `unary '-' expects Int or Float, got String` |
| `n << 99` | `__lk_shl shift amount 99 is out of range 0..63` | `shift amount 99 is out of range 0..63` |
| `5[0]` | `GetIndex target expected Obj, got Int` | `Int is not indexable` |
| `5[0] = 1` | `SetIndex target expected Obj, got Int` | `Int cannot be indexed for assignment` |

`ShortStr` 尤其要紧:那是"短到能内联的字符串"这个**表示**,语言里没有这个
类型。它是 `RuntimeValKind` 的变体名,而约四十条消息写的是
`bail!("… got {:?}", v.kind())` —— 所以 `RuntimeValKind` 的 `Debug` 现在是
手写的,打语言的类型名;要看表示用 `repr_name()`。

opcode 名同理:它说的是编译器挑了哪个**融合**形式,源码里没有 `ModInt`,
而且这个选择会随优化变化。`operator_symbol` 把算术 opcode 映回源码运算符;
映不回去的说明是编译器/执行器不匹配,那时 opcode 名才是有用的。

内部不变量被破坏的消息**保留** opcode 名(`GetList target object changed
while reading list`):那是 VM 的 bug,不是程序的。

## trait 方法分发与 auto-Display(2026-07-07 裁决,plan J)

native 侧 struct 实例是普通 string-keyed map(**无 `"$type"` 隐藏键**——
`len()`/迭代/display 与 map 完全一致);运行时类型身份存 arena 句柄侧表
(lkrt `OBJ_TYPE_MARKS`,`NewObject` 时打标记)。两个已知边界:

- **类型标记不跨 channel**:深拷贝(`OwnedVal`)重建 map 时不复制标记,
  收方对该 struct 实例的动态 trait 方法调用会 raise(VM 能成功)。语料无
  此形状;如需支持,`OwnedVal` 捕获/重放需带上标记。
- **auto-Display 只镜像 `show`**:VM `try_runtime_display_show` 硬编码查
  方法名 `"show"`(与 trait 名无关;`#[derive(Debug)]` 展开出的
  `__LKShow::show` 也走它)。native 在 display 上下文(print/println 参数、
  模板插值 `ToString`/`ConcatString`/`ConcatN`)对带 provenance 的 struct
  直调注册的 `show`。**无 `show` impl 的整对象 display 不进子集**(VM 内部
  有 `<Type {...}>` debug 形与 registry 缺失 bail 等多种路径,未统一前不复刻)。

动态分发(boxed receiver,经混合列表/Dyn 参数流动)限 `argc == 0`(self 之外
无参数)且零捕获 impl;静态 devirt(NewObject provenance 已知)支持任意参数。
分发臂按注册序排列,标记无匹配 → raise(VM 的 unknown-method 同为错误)。

## 维护约定

- 新增可下降形状时,先在此登记预期语义(尤其失败路径与显示格式),再写差分用例。
- 当 VM 与 native 出现分歧:先查本表;表内未覆盖的,裁决后**新增条目 + 差分用例**,
  不允许只改一侧实现使测试变绿。
- 退出机制(exit 1 vs SIGABRT)如未来需要统一,属于语言决策,需同时改本表、
  差分 harness 的宽容逻辑(`success()` 对比)与 CLI 文档。
