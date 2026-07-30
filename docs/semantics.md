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

失败路径的契约是**响亮失败 + stdout 为空**,**两个后端退出码都是 1**
(2026-07-30 收紧)。差分测试仍只比较 `success()` 与 stdout,不比较 stderr
文本 —— 但退出码不再"不作为契约"。

此前 native 用 `abort()` 结束(SIGABRT,壳层显示 134,还可能落 core dump),
理由是"退出机制不是契约"。可这让同一个程序在 `lk prog.lk` 下 `$? = 1`、编译
之后 `$? = 134`,壳层多打一行 `Aborted` —— 调用方拿脚本判退出码时两者不通用。
未捕获的 raise 是**程序**失败,不是运行时故障,所以走 `exit(1)`;`panic()` 同理
(仍然不可捕获,只是退出码对齐)。真正的运行时/链接故障(ABI 版本不匹配)
仍然 abort。

| 程序 | 期望 | 说明 |
|------|------|------|
| `x % 0` | 失败,stdout 空 | 整数模零。native 侧禁止直接依赖 LLVM `srem` 的 UB,必须走 `lkrt` 的 guard |
| `1 << 64` | 失败,stdout 空 | 移位量越界(`0..63`),两端文本逐字一致 |
| `let m = {"a": 1}; return m["z"] + 1;` | 失败,stdout 空 | 缺失值(nil)参与算术 = halt。VM 报 `Add expected numbers…got Nil` |

**这张表里曾经有两条退休的裁决,2026-07-30 删掉**:"整数除零 → 失败" 和
"浮点除零是响亮失败,**不是** IEEE `inf`"。`/` 早就是**浮点除法**了(见上面的数值
表),所以 `2 / 0` 和 `1.0 / 0.0` 都是 `inf`,`0.0 / 0.0` 是 `NaN`,两端逐字一致
(2026-07-30 复核)。留在原地的退休裁决不是无害的注释 —— `lkrt` 的通道容量就是照着
一条退休的裁决写的(见 channel 容量那节),两端因此给了不同答案。

**宿主错误是语言错误。** `fs.read_dir("/nope")` 这类 IO 失败在 VM 里是可以
`try`/`catch` 住的 raise;native 侧此前经 `aborting()` 直接终止进程,同一个程序
解释执行能恢复、编译之后必死。现在它们统一 raise(`lkrt::abi::raising`),
没人接就按上面的规则 exit 1。

## 可空不是数值属性(2026-07-30 裁决)

`Int?` **不能**赋给 `Int`,和 `String?` 不能赋给 `String` 一样。

此前只有非数值类型有这条规则。`is_assignable_to` 里数值提升那条分支
(`Int → Float`)问的是 `numeric_class`,而 `numeric_class` 会**穿过**
`Optional` —— 它必须穿过,因为它回答的是"这个值参与算术产出什么"。于是
`Int?` 和 `Int` 被判成同一类,`let n: Int = xs.index_of(x);` 一路放行,
nil 流到下一个读 `n` 的地方才炸 —— 报的是错的行,而那个标注正是为了拦住
它才写的。

现在可空性在数值提升**之前**判定:源可空、目标不可空 = 拒绝。`!`、`??`、
`let n: Int?` 和不写标注仍然都成立 —— 说明"我处理过 nil 了"的四种写法
一个没少。

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

## 负数位置只有一个意思(2026-07-30 裁决)

`-1` 是最后一个,`-2` 是倒数第二个 —— `[i]`、`get(i)`、`slice(start, end)`
在 List / Slice / String / Bytes 上一律如此,超界仍然钳到 `0..=len`。

此前 `slice` 有四份实现三种答案:List 和 Bytes 报错,String 和 Slice 悄悄把
负数当 0(于是切出一个没人要的窗口),而 **native 的字符串 slice 早就从尾部
数** —— `"abcde".slice(1, -1)` 解释执行给 `""`,编译之后给 `"bcd"`。同一个
程序两个答案,差分语料里没有负数用例才一直没抓到。

裁决取"从尾部数",理由是语言在隔壁一个操作符上已经这么说了:`xs[-1]` 是
最后一个元素。四份实现现在共用 `slice_position`(VM)与 `resolve_position`
(lkrt),差分语料补了负数窗口。

**`bytes` 模块是第五处,2026-07-30 才接上。** 上面说"四份实现现在共用
`slice_position`",而 `bytes` 模块的 `get`/`slice` 仍然 raise ——
它在 stdlib crate 里,够不到 core 那个 `pub(super)` 的辅助函数,于是自己写了
`usize_arg`。可观测的是**同一个操作两个答案**:`b.slice(1, -1)`(方法拼写,走 VM 的
方法分发)给 `Bytes([98,99,100])`,而 `bytes.slice(b, 1, -1)`(模块拼写,走 stdlib
自己的代码)报 "expects a non-negative integer"。

判据因此提成了 `core::val::position` 的公开 API(`read_position` /
`element_position` / `write_position`),VM 侧现在真的只有一份;`element_position` 与
`read_position` 的区别正是"元素**没有**可编的答案,窗口有" —— `xs[9]` 是 nil 而不是
最后一个元素。native 侧保持自己的镜像(lkrt 不能依赖前端),这是既有的模式。

**写也一样。** `xs[-1] = 9`、`xs.set(-1, 9)`、`remove_at(-1)`、`insert(-1, v)`
都从尾部数;此前读能负、写报 "list index must be non-negative" —— 同一个下标
表达式,一个方向能用。解析之后仍然越界的写是**响亮失败**(读越界是 nil,写越界
不是一个程序能表达的意思);native 的 `lkrt_lklist_dyn_set` 此前会**把列表撑大**
去容纳越界下标,`xs[9] = 1` 在三元素列表上解释执行报错、编译之后追加六个 nil。

## 顶层 `let` 只有一份存储(2026-07-30 裁决)

函数能看见的顶层 `let` 是**一个**变量,不是两个。

此前是两个:顶层把它缓存在寄存器里,函数读写的是全局槽,两者只在初始化那一刻
一致。`let n = 0; fn bump() { n = n + 1; } bump();` 之后函数看到 1、顶层看到 0;
顶层写 `n = 5` 函数也看不见。两个后端做的是同一件事,所以差分测试永远抓不到 ——
这是语言 bug,不是分歧。

`const` 仍然保留寄存器缓存:没有东西能写它,副本不会走味。而且那不只是优化 ——
机器整数的**位宽**记在寄存器上,只走全局槽的 `const PAGE_NX: u64 = 0x8000…`
会打印成负的 `i64`。

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
- **catch-all 后面的臂永不运行,所以拒绝**(2026-07-30 裁决):
  `match n { _ => "any", 1 => "one" }` 以前静默接受 —— 你写了一个你认为会发生的
  情况,而它不会,并且没有任何东西说过一句话。核心检查器没有 warning 通道,而
  LK 对同类错误的做法是响亮拒绝(步长 0 的范围、占用声明名字的 `let`)。
  "catch-all" 的判据与上面那条落空检测**共用同一个谓词** —— 一个模式一旦算
  "匹配一切",就不能同时"对类型是全的、对可达性不是"。带守卫的 catch-all 是有
  条件的,不遮蔽任何东西;or-pattern 里有一个全的分支就算全。
  这条也不是假想:`examples/syntax/unsupported.lk` 里 `match 99 { n => n, _ => 0 }`
  的 `_` 就是死的。
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

## 块就是作用域(2026-07-30 裁决)

块里的 `let` **不会活过那个块**。`if`、`while`、`for`、裸块、`match` 分支体
—— 每一种都是作用域,遮蔽外层同名变量之后,块外读到的还是外层那个值。

此前每一种都漏,而且是静默的:

```lk
let x = 1;
if c { let x = 2; }
x            // 曾经是 2
```

三个独立的洞喂着它:

1. **语句路径复用寄存器。** 遮蔽用的 `let` 直接写外层绑定的寄存器,块退出恢复
   的是"名字 → 旧寄存器"的映射,而那个寄存器里的值已经被改掉了。现在遮蔽
   外层作用域的绑定一律分配新寄存器(`local_declared_in_current_scope`)。
2. **内联器根本不恢复绑定。** `Stmt::Block` 那一臂只调了 rebind 抑制。后果比
   泄漏更糟 ——
   `fn f(c) { let y = 1; if c { let y = 2; } let s = 45; return y; }` 内联后
   答 **45**:`y` 还指着内层那个寄存器,而 `s` 正好被分到它。
3. **块表达式没有作用域。** `match` 分支体就是块表达式。

寄存器不因此变多:遮蔽在真实代码里罕见,而基准反而快了一点点(geomean
1.001x → 0.996x)。

## 闭包与捕获

| 程序 | 期望 stdout | 说明 |
|------|-------------|------|
| `let k = 3; let f = \|x\| x * k; k = 5; println(f(1));` | `5` | **捕获是共享可变 cell**:闭包创建后对被捕获变量的赋值对闭包可见(native 在调用点解析 cell 当前值) |
| `let f = \|x\| x + 1; println(f(1)); f = \|x\| x * 10; println(f(2));` | `2` `20` | 闭包变量重绑定按程序序生效 |
| `let i=0; while (i<3) { let f=\|x\| x+i; println(f(10)); i=i+1; }` | `10` `11` `12` | 循环体内捕获**循环外变量**:cell 在循环入口预提升,单一共享 cell,条件/自增读也走 cell(曾因 mid-body promotion 在第 2 迭代报 "expected Int, got Obj") |
| `for i in 0..3 { let f=\|x\| x+i; println(f(10)); }` | `10` `11` `12` | **for 循环变量**捕获为每站点快照 cell(fused 循环 opcode 驱动原始寄存器,不可重绑);快照是 copy 而非 move(曾把计数器 move 成 Nil) |
| 循环内 `g = \|x\| x+i` 逃逸循环后调用 | 共享 cell 终值 | native 侧跨迭代闭包 ref 逃逸响亮拒绝(ref 一致性在 loop header 处终止) |

## 导入的类型可以构造(2026-07-30 裁决)

`module.Type { field: value, … }` 成立。字段、trait 方法、按声明序的 display
全都对。

难点在于:类型的身份带着定义它的模块(`vm::TypeScope`) —— 两个模块里同名的
`Pt` **不是**同一个类型,`impl` 也按这个身份注册。而 `NewObject` 只带类型
**名**,`declared_type` 用的是"当前执行模块"的 scope。在导入方直接构造出来的
`Pt` 会是另一个类型,`p.norm()` 找不到方法(实测如此)。

裁决:**让定义方模块来建这个对象**。每个 `struct S { a, b }` 旁边自动多一个

```lk
fn S$new({a: A, b: B}) -> S { return S { a: a, b: b }; }
```

而 `m.S { a: 1, b: 2 }` 是 parse 时糖,降到 `m.S$new(a: 1, b: 2)` —— 一次普通
的跨模块调用,在**那边**执行。于是 scope、声明的字段序、trait 分发全部自然
正确:没有新 opcode,没有 artifact 版本变化,AOT 降低也不用学任何新东西。

参数是**具名**的,所以调用点不需要知道声明:传的就是字面量里那些
`field: value`,漏一个或拼错是构造函数自己的 arity 报错。`$` 不可词法化,
所以这个名字撞不上任何程序能写出来的东西 —— `try$call` / `select$block`
用的是同一招。

**脱糖不能漏出来**:那两句报错走的是具名参数的措辞("Missing required named
argument: y"),而读者写的是字段。类型检查器现在认得 `Type$new` 这个 callee,
说的是 `Missing required field 'y' for struct 'Pt'` —— 和本地字面量逐字一样。

`use { Pt } from "types";` 仍然拿不到类型:模块导出的是**值**,声明不是值。
那句报错也说清了这条,并指向构造函数。

代价是每个声明 struct 的模块也声明了一个函数 —— 于是"把程序当**单个
`Function`** 执行"这条路(`compile_source` + `execute`,测试用的便利路径)对
这类程序不再可用。它本来连 `fn` 都装不下,所以 `compile_source` 改走模块路径,
反而更能干。基准 geomean 1.001x → 1.008x,无系统性回退。

**原生降低已补齐**(同日):`CallNamed` 整条 opcode 此前没有 AOT 降低,所以
每一个具名调用都会把模块拖回 VM。现在它和位置调用一样 devirtualize,多一步是
实参**顺序** —— 每个名字都是编译器发出的常量,所以排列是编译期事实
(`FunctionData::param_names` 给槽序,`positional_param_count` 给分界)。
构造函数的**返回值**也带上了类型出身,否则
`types.Pt { … }.x` 之后的方法调用又会因为接收者无类型而掉出去。

顺带把 `emit_trait_call` 改名成 `emit_call_with_args`:它并不特属 trait,而是
"实参已按帧序排好"的那个发射器 —— trait 分发把 `self` 放在最前,具名调用按名字
排列,两者要的是同一件事。

仍不降低的是**跨模块 trait 方法**(`types.make(3,4).norm()`):AOT 的 trait
环境只扫主模块,导入模块里注册的 impl 不在其中。与构造方式无关 —— 用老的
构造函数写法一样不降低。

## 模块与 IO

| 程序 | 期望 stdout | 说明 |
|------|-------------|------|
| `datetime.now()` | — | 返回 Unix epoch **秒**(非微秒;datetime_demo 曾因此假设而自身断言失败) |
| `std.write(out, "a")` | `a`,返回 `1` | `write`/`writeln` 返回写入字节数(writeln 含换行 = len+1);`flush` 恒返回 `true`。`std` 是 `io` 的**子模块** —— 写 `use { std } from io;` 或 `io.std.write(…)`,裸 `std` 不是全局(2026-07-30 更正) |
| `std.write` 与 `println` 交错 | 程序序 | **stdout 顺序契约**:native 侧 Rust 写者先 `fflush(NULL)` 再写、写后 flush 自身流,保证与 C `printf` 缓冲的输出保持程序序 |
| `math.sqrt(-4.0)` | 响亮失败 | 负参是致命错误(双方 loud),不是 NaN |

## 容器 display

| 程序 | 期望 stdout | 说明 |
|------|-------------|------|
| `println([1,2,3])` | `[1,2,3]` | 逗号分隔无空格;float 元素用 Rust `to_string`(`2.0`→`2`) |
| `println(["a","b c"])` | `["a","b c"]` | 字符串元素 **Rust `{:?}` 引号+转义**(`"`→`\"`、tab→`\t`) |
| `println("${xs}")`(xs 是 list) | `[1,2,3]` | 模板插值**显示容器**(2026-07-30 更正:此前这条写的是"响亮失败,标量 only",而 VM 早已不是那样)|
| `println("a${xs}b")` / `"m=${m}"` / `"${[P{v:1}]}"` | `a[1,2,3]b` / `m={"k":1}` / `[P{v:1}]` | 多段模板、map、结构体列表同样 |
| `println(map)` | hash 迭代序 | map display 顺序 = 底层 hash map 迭代序,**跨运行稳定但不可移植**(依赖 hasher+增长历史)——native 侧不进子集,响亮拒绝 |

### 一条过时的裁决(2026-07-30 更正)

上面那条曾经写着:`ToString` / 模板插值 / `+` 拼接走"标量 only"的显示路径,
容器在那里是响亮失败。VM 后来改了 —— `"${xs}"` 就是 `[1,2,3]` —— 而**AOT 一侧
一直照着退休了的规则**传 `containers: false`,于是任何模板里带 list / 结构体
列表的程序都掉回 VM。答案一致,只是慢,所以差分门禁抓不到;是探针撞上的。

现在两边都显示容器,差分语料补了这条。留在原生子集外的只有两个,各有自己的
理由:**map**(hash 迭代序不可移植,见下)和 **Set**。

### map 迭代序:为什么没有跟着结构体一起改(2026-07-30 记)

结构体字段序当初从 hasher 序改成声明序,理由是"换个 hasher 会静默重排每一个
结构体"。**同一条理由对 map 成立**,而且插入序正是 Python / JavaScript 给的
东西。这里没有跟着改,是权衡后的决定,不是漏掉:

- `TypedMap` 的五个变体都是 `FastHashMap`,全仓 260 处 `TypedMap::` 匹配点;
  `lkrt` 还有一份自己的 map,由 `lkrt/src/vm_mirror.rs` 逐条比对迭代序 ——
  两边得一起换。
- 插入序要么是 `Vec<entry>` + 索引表(IndexMap 布局),要么额外一条顺序向量。
  代价落在 **`delete`**:保序删除是 O(n),而交换删除会毁掉顺序。Python 用
  墓碑加周期性压缩解决,那是另一套实现。
- 基准里 map 是最热的容器(`two_sum_map`、`histogram_group_count`、
  `event_join_by_id`、`config_defaults_merge`),而性能门禁是硬的 10%。

结构体那次的代价是"记录一个字段顺序",这次的代价是换掉最热容器的表示。
所以现状保留:hash 迭代序,**跨运行稳定、两个后端一致**(有 `vm_mirror`
一致性测试兜着),但不可移植。要改就当一个独立项目做,连着基准一起。

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

## 结构体按字段比较(2026-07-30 裁决)

`P { x: 1, y: 2 } == P { x: 1, y: 2 }` 是 **true**。同一个声明的类型
(模块 + 名字,不是名字)加上每个字段相等,递归走同一份深度受限的
`runtime_values_equal`。

此前结构体按**句柄**比较,于是它是这门语言里唯一不按内容比较的聚合:
`[1] == [1]`、`{"a":1} == {"a":1}`、`Set([1]) == Set([1])` 全是 true,只有
结构体是 false。而且是静默的 —— `xs.contains(p)`、`index_of`、`unique` 全部
继承了它,一个结构体列表根本搜不了。

类型身份用 `scope` + `name`,不用整个 `DeclaredType`:后者的 `fields` 在声明
够不到时(跨模块、宿主构造的对象)是空的,连它一起比会让同一个类型跨边界
不等于自己。

结构体**仍然不能**作 map 键或 set 成员 —— 那是另一条裁决(键只有 nil / Bool /
Int / String),相等不蕴含可哈希。

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

`lkrt`(AOT 的运行时)直到 2026-07-30 还照着**退休前**的那条规矩写:
`capacity <= 0` 当无界、负数不报错。这不是"native 慢一点"那类看不见的差异,
是两端**给不同答案**:`chan.new(0)` 连发两次 `try_send`,VM 给 `true/false`
(队列上界 1),native 给 `true/true`;`chan.new(-1)` VM 报错,native 递回一个
无界通道。裁决改在 VM 侧、运行时侧没跟上 —— 一条裁决落在两份实现上就是这个
下场,和"退休的容器裁决"那条同一个病(见上文模板串一节)。

`capacity` 报的是**要的那个数**,不是队列的上界:`chan.new(0)` 的
`chan.capacity` 是 `0`,而里面的 mpsc 上界是 1。两个数得分开记(VM 记在
`ChannelValue::capacity`,lkrt 记在 `ChanInner::requested`)。

**`use chan;` 会遮蔽 `chan()` 全局**,因为模块名和构造函数同名。此前这是条
死路:导入模块之后**没有任何办法**创建 channel。现在模块里有
`chan.new(capacity[, type])`,和全局 `chan(…)` 共用一份实现 —— 导入之后用
模块拼写,不导入就用全局。

模块**要自己完整**,不能只完整一半(2026-07-30)。`chan.new` 补上之后,模块里
仍然只有 `try_send`/`try_recv`:阻塞的 `send`/`recv` 只作为**不带前缀的全局**
存在。也就是 `use chan;` 之后拿到的是个只能轮询的通道,要阻塞就得去写
`send(c, v)` —— 一个和 `chan` 前缀无关的名字。现在 `chan.send`/`chan.recv`
和那两个全局共用一份实现(`blocking_send_value`/`blocking_recv_value`),和
`new` 是同一个办法:一份实现,两个名字。

`chan` 同名的这件事也让 AOT 少降低了一半(2026-07-30):`chan` 既是内建构造
函数又是模块,`builtin_for_name` 先命中构造函数,成员读取就找不到值了 ——
`chan.new(1)` 悄悄掉回 VM,`chan(1)` 正常降低,两边打印同一个答案。字节码分得
清这两件事(构造函数是 `GetGlobal chan` + `Call`,模块多一步 `GetIndex`),所以
判据放在 `GetIndex` 那侧:能走到那里就是模块拼写。

导入之后仍然写 `chan(1)` 的报错也说人话了(2026-07-30):**"this value is not
a function: it is a Map — an imported module is a map of its members, so call
one of them"**。此前是 `Call callee is not callable` —— 说的是操作数,读者
没有任何可动作的信息。类型检查器抓得住普通 map(`{"a":1}(1)` 报
"Cannot call non-function type"),但它不给导入的名字建模,所以这条路是运行时的。

同理 `task.await(h)` 第二次报 **"this task has already been awaited"**,不再是
`Task not found`(那说的是任务表)。VM 与 lkrt 两侧用同一句话。

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

**trait 方法可以带默认实现**(2026-07-30 补):

```lk
trait Greet {
    fn name(self) -> String;
    fn hi(self) -> String { return "hi ${self.name()}"; }   // 默认
}
```

没写 `hi` 的实现者拿到这一份,写了的覆盖它。实现方式是**按实现类型逐份复制**
(`stmt::trait_defaults`,在宏之后、任何分发之前),因为分发本来就按目标类型
索引,而默认体里的 `self` 就是那个实现类型 —— 复制既是最简单的降低也是正确的
那个,类型检查器、VM 编译器、AOT 降低都不需要知道"默认"这回事。trait 写在
impl **后面**也算数:先扫全程序收集,再填。

此前 trait 只能写签名,于是每个实现者都得把同一段方法抄一遍 —— 语言逼着用户
干实现里一直在消灭的那件事(一个概念 N 份拷贝,靠记性同步)。而且那时报错是
把 token 流倒出来:`Invalid type: String { Return "hi" Semicolon}} Struct P …`。

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

## `url` 的 component 一对要能往返(2026-07-30 裁决)

```lk
url.encode_component("a b&c=d")   // 以前:"a+b%26c%3Dd"
url.decode_component(那个)         // 以前:"a+b&c=d" —— 不等于原串
```

编码用的是 **form** 编码(空格变 `+`,`form_urlencoded::byte_serialize`),解码只
撤 `%XX`。一对的两个方向得先**互相**同意,再谈和别的东西同意 —— 这和 datetime 的
`format`/`parse` 不往返(见那条)是同一个形状。

裁决:**component 就按 component 编码**,空格是 `%20`,`+` 是字面的 `+` —— 也就是
`encodeURIComponent` / `decodeURIComponent` 的规矩。未保留集取
`A-Za-z0-9-_.!~*'()`。form 编码是 query body 要的东西,而 `query_stringify` /
`query_parse` 本来就是那一对(两端都走 `form_urlencoded`),不受影响。

编码器因此改成**手写**的,和本来就手写的解码器并排放着:一对的两个方向应该是**一份
实现的两个方向**,而不是两个 crate 的两种约定。

`base64.encode` / `hex.encode` / `url.encode_component` / `url.decode_component`
同时有了原生实现(lkrt 用**与 stdlib 同一个 crate**,所以文本逐字节相同,和
`datetime`/`json` 的理由一样)。`base64.decode` / `hex.decode` 给的是 `Bytes`,原生
还没有那个承载类型,继续回落 —— 表里没有行的成员是普通回落,不是错答案。

## 子模块经父模块访问也要原生降低(2026-07-30)

`encoding.json.parse(s)` 和 `use { json } from encoding; json.parse(s)` 是同一个
成员的两种拼法。后者一直原生降低,前者**整个程序掉回 VM** —— 答案一样,慢三倍,
所以差分门禁看不见,是探针撞上的(和模板串里的容器、`chan.new` 同一类)。

缺的是两件事:

1. 从父模块读一个**子模块**,给出的是父模块的一个"函数"(`ModuleFn`),而不是
   另一个模块对象。链子因此停在第一个点上。`is_submodule` 谓词早就有了 —— 选择性
   导入那条路一直在用它 —— 只是 `GetIndex` 那侧没用。
2. `encoding.json.parse(s)` 编译成 **`CallMethodK`**,接收者是模块对象。那条路上
   没有模块分支,于是去 `ssa.read` 一个只存在于降低期的 ref,报
   "register r7 is read before any definition"。

顺带补齐了 `MODULE_TABLE`:父模块(`encoding`/`net`/`io`)此前根本没有行,名字都绑
不上;子模块补了 `base64`/`hex`/`url`/`udp`/`file`。名字**绑得上**和成员**降得下**
是两件事 —— 前者归这张表,后者归 `MODULE_ABI`;`encoding.base64.encode` 现在是后者
缺(lkrt 里没有符号),报的也是那句话。

## 排序说的是排序的规矩(2026-07-30 裁决)

`<` / `<=` / `>` / `>=` 排的是**数字和字符串**,两边要**同类**。规矩没变,报错以前
说的是别的:

- `1 < "a"` 报 "**the left operand** must be numeric types" —— 一句话怪错了两次:
  这里的左操作数**就是**数字,而换成字符串本来是合法的。现在报
  "an ordering compares two of a kind: a String orders against a String, a number
  against a number"。
- `[1,2] < [1,3]` 报 "must be numeric types(expected `Int | Float | Box<Any>`)" ——
  期望集合里**漏了 String**(字符串早就可排序了),而且答的是错的问题:列表的问题
  是它根本**没有序**,不是它不是数字。现在报 "`<`, `<=`, `>` and `>=` order numbers
  and strings; this type has no ordering"。

根因是排序复用了**算术**那条判据(`ensure_numeric_operand`)。算术里"必须是数值"是
对的(字符串走的是拼接那条臂),排序里不对 —— 所以排序现在有自己的
`ensure_orderable_operand`。一条规矩变了(字符串可排序),而复用它的第二个地方没跟上,
这是本会话反复出现的那个形状。

## 只有裸名字能起一个宏调用(2026-07-30 裁决)

后缀 `!` 是解包,`name!(…)` / `name![…]` / `name!{…}` 是宏调用。判据以前**只看后面
那个开括号**,于是:

```lk
m["a"]![0]     // 以前:"a macro invocation reached the parser"
xs[0]![0]      // 同上
m.field![0]    // 同上
```

宏名是**标识符**,`m["a"]` 不可能是宏名 —— 这些拼写里根本没有歧义,却要靠加括号
(`(m["a"]!)[0]`)或者拆成两行绕过去。现在判据是"`!` 前面是不是一个裸 `Expr::Var`":
是,才可能是宏。

真正有歧义的只有裸名字一种(`f![0]`:是宏 `f!` 还是解包 `f` 再索引?),那一个归宏,
要解包就写 `(f!)[0]` —— 报错里也把这条说出来。

## 关键字可以当成员名(2026-07-30 补)

关键字以前在**所有**位置都被保留,这比语法需要的多。一个**成员**总是经 `.` 到达,
或者声明在 `struct` / `impl` / `trait` 的体里,而这些位置**都不能起一条语句** ——
所以下面这些以前是语法错误,没有任何读者能据以行动的理由:

```lk
struct Row { type: String, select: Int }
impl Row { fn match(self) -> Int { return self.select * 2; } }
trait Runner { fn go(self) -> Int; }
db.select()
parser.match(x)
```

放开的位置一共四处:`.` 之后的成员读取、结构体**字段声明**、结构体**字面量**的
字段名、`impl`/`trait` 体里的方法名。值字面量(`true`/`false`/`nil`)故意不在里面
—— 它们是值不是关键字,`p.nil` 读不出意思。

**顶层 `fn` 保留限制**:调用它是表达式位置上的一个裸名字,`select(1)` 和 `select { … }`
就得靠上下文区分了。报错也跟着说清:"`select` is a keyword, so it cannot name a
top-level function — a call to one is a bare name, where `select(…)` could not be
told from the `select` statement. It *can* name a method or a field"。

"这个 token 能不能当名字"只有一份判据(`token::keyword_as_name`),四个位置共用。

## 一个名字一个意思:方法只声明一次(2026-07-30 裁决)

三种撞名以前都是**静默取最后一个**:

```lk
impl Show for P { fn show(self) -> String { return "a"; } }
impl Show for P { fn show(self) -> String { return "b"; } }   // 静默赢

impl P { fn get(self) -> Int { return 1; } }
impl P { fn get(self) -> Int { return 2; } }                  // 静默赢
```

字段那一种更糟 —— 拿到哪个取决于**实参个数**:

```lk
struct P { get: Int }
impl P { fn get(self) -> Int { return 9; } }
P { get: 1 }.get()      // 以前:1(字段),方法永不可达

struct Q { f: (Int) -> Int }
impl Q { fn f(self) -> Int { return 9; } }
q.f(3)                  // 以前:走方法,报 "Method expects 0 arguments",字段闭包永不可达
```

`p.get(…)` 说不出它指哪个,所以在**声明处**拒绝。两个同名顶层 `fn` 早就是报错的,
这是同一条规矩。两个**不同 trait** 各声明一个同名方法也拒绝:LK 没有
`Trait::method(x)` 那种消歧写法,`p.run()` 会没有答案。

判据是**程序级的一遍**,不是有序遍历累积出来的 —— 理由和 `collect_function_names`
一样:问的是声明的**集合**,而检查器的注册表对重复注册的 `impl` 是**替换**的
(REPL 的上下文跨次复用),所以它分不出"这里声明了两次"和"又见到一次"。

## trait 必需方法在 `lk check` 就该报(2026-07-30 补)

`TypeRegistry::validate_trait_impl` 一直存在,但只在 **VM 注册 impl 时**跑 ——
也就是运行时。于是 `lk check`(预检命令)放过一个跑不起来的程序,一句话不说。
现在检查器的 `Impl` 分支自己查:trait 声明的每个方法都得在。trait 默认实现在这之前
已经由 `stmt::trait_defaults` 拷进去了,所以"在不在"就是全部问题。

## 顶层 `let` 不能占用声明已经绑走的名字(2026-07-30 裁决)

```lk
fn pick() -> String { return "fn"; }
let pick = || { return "let"; };      // 以前:静默地是 let 那个
```

两行**调换顺序也一样**是 `let` 赢。原因是 `fn` 和类型声明是被 **hoist** 的 ——
相互递归能写,说明一个 `fn` 在它那一行之前就可见了 —— 所以源码顺序对它们不适用,
"`let` 遮蔽了它"这句话没有连贯含义。两个同名 `fn` 早就是报错的
("Compiler duplicate function"),`fn` + `let` 却静默。

现在拒绝,并说清为什么:"`pick` is already declared as a function in this module:
a function is visible before the line it is written on, so a `let` of the same
name cannot shadow it — rename one of them"。覆盖 `fn`、`struct`、`type` 别名。

**两个 `let` 仍然是正常遮蔽**(两者都是顺序敏感的),**可调用体里面的 `let` 也是**
—— 局部量在自己作用域内顺序敏感,而声明在作用域外面。

这条不是假想的:它正是 `examples/syntax/closure.lk` 里一个死掉的 `fn apply` 挨着
一个活的 `let apply` 的来由 —— VM 跑得通(两条断言碰巧都被 lambda 那版满足),
native 拒绝,而没有任何东西说过一句话。

## 函数不能声明在另一个可调用体里面(2026-07-30 裁决)

`fn outer() { fn helper(n) { … } return helper(1); }` 以前**语法上收下**,然后
编译期报 "Compiler undefined function `helper`" —— 函数下标只从顶层语句收集
(`collect_function_names`),嵌套的 `fn` 从没拿到过下标。语法接受、后端拒绝,而且
是用后端的话说的,这是最糟的那种组合。

现在在类型检查里用语言的话拒绝,并且把两条替代路都说出来:挪到顶层,或者
`let name = |…| …;` 用闭包(需要外层作用域时)。判据是"当前是否在某个可调用体
里",直接读 return frame —— 每个函数/闭包体开一个,别的都不开,所以不需要第二
份记账。`impl` 方法不受影响:它们是顶层的 `fn` 声明。

**支持它是一个特性,不是这条修复**:嵌套 `fn` 捕获不了外层(那是 Rust 的规矩),
所以做法是 hoist 加一个带作用域的名字 —— 见 todos。

顺带:递归因此**只有顶层 `fn` 写得出来** —— 和 Rust 一样(Rust 的闭包也不能递
归)。`let fact = |n| … fact(n-1) …` 里 `fact` 在自己的初始化式里还不可见;手写
Lua 那套 `let fact = nil; fact = |n| …;` 过不了类型检查(`fact` 是 Nil,"Cannot
call non-function type")。

这条规矩现在**自己说出来**:以前报 "Compiler undefined callable `fact`" ——
一句关于操作数的话,讲的是关于作用域的规矩,读者拿它没有任何可做的事。现在报
"`fact` is not in scope inside its own initializer, so this closure cannot call
itself; write a recursive function as a top-level `fn fact(…)`"。判据是"被调的
名字正是当前正在初始化的那个绑定",所以拼错的名字仍然读作拼错;外层同名绑定是
另一个函数,调它不受影响。

## lambda 可以写自己的类型(2026-07-30 补)

`|x: Int, y: Int| -> Int { … }`。此前两者都是**语法错误**,而 `Type::Function`
一直是有形参类型和返回类型两半的 —— 也就是 lambda 是语言里唯一一个类型写不出来
的可调用体,它的形参类型只能从调用点**猜**。

- 形参类型和返回类型各自独立,都可省。
- 谁说了算:**闭包自己写的 > 上下文期望的 > 新类型变量**。写下来的是作者的
  声明,上下文只是关于它的一个推断。
- 声明的返回类型是**用来检查**的,不只是记下来:体里每个 `return` 都要能赋给
  它,不然报 "Return type mismatch in closure"。
- 形参里写不了 union,因为那个位置的 `|` 是参数列表的收尾;括号也不行(带括号
  的类型不在语法里),所以走 `type` 别名 —— 那是同一个类型的第二个拼法。

**每个"声明了函数类型"的位置都收得下 lambda**(2026-07-30):一条规矩落在多个
地方,漏掉的那些就静默拒绝为它写的 lambda。清出来的有七处 —— `let`、结构体字段、
`fn` 形参(报 "got `('T1) -> Int`")、命名实参、声明的返回类型(报 "Return type
mismatch")、`List<(Int) -> Int>` 的元素、`Map` 的值。现在是**一个**
`check_expr_against` 回答所有这些位置。

它会往聚合字面量里**分发**期望(`[|x| …]` 对 `List<(Int) -> Int>`),但只在那个
位置真的坐着一个 lambda 时才走这条路 —— 否则普通通路的推断原样保留(混类型列表
字面量是 `Tuple`,那条规矩不是这个 helper 该推翻的)。`Optional` 收下它的载荷,
括号不是类型层面的构造。

**期望流进去只是一半,答案还得回查**:无条件返回声明的类型是一句断言而不是描述,
它一度让 `let fs: List<(Int) -> String> = [|x| { return x + 1; }];` 通过。

"哪串 token 是一个类型"这件事以前只在语句 parser 里写了一份,lambda 需要同一
件事而又到不了那个 parser。现在抽在 `core/src/type_syntax.rs`,两边共用;位置
之间唯一的差别是**类型在哪结束**,由 `StopAt` 说:语句位置 `|` 是 union、`=`
收尾;lambda 形参位置顶层 `|` 和 `,` 收尾;lambda 返回位置顶层 `{` 收尾。

它放在 `type_syntax` 而不是 `token` 里,因为它要提 `Type`,而 `token` 不能伸进
`val` —— 那条边会把 `token` 也拖进 `val` ↔ `vm` 的环(见 `docs/module-cycles.md`)。

**`Expr` 的大小是有门禁的**:`Expr` 是递归解析的,把 `Type` 按值塞进
`Expr::Closure` 会让每个解析栈帧变大,大到深嵌套时在 parser 的深度守卫**之前**
就爆栈 —— 那条守卫只有先跳才有用。所以返回类型装箱,并且加了
`the_expression_node_stays_small_enough_to_recurse_over` 把这条说出来。

## 块是要过类型检查的(2026-07-30 裁决)

`Expr::Block` 以前直接返回 `Any`,**不往里看** —— 理由是块大多来自 desugar,
在拼出来之前已经查过了。这个理由对 desugar 成立,但闭包体也是块,于是:

```lk
let f = |x| { let s: String = 1; return x; };   // 以前:接受
let s: String = 1;                              // 同一条语句在顶层:报错
```

也就是**块体 lambda 里的所有语句从来没过类型检查**,一整类代码对检查器不可见。
现在 `Expr::Block` 和别的表达式一样查自己的语句,值是尾表达式的类型
(`check_statements_value`),并且**自带一层作用域** —— 块是作用域这条规矩在
别处已经立过了(见"块不是作用域"那条)。

`unsafe { … }` 曾是唯一往里看的地方(`check_block_value`),因为最需要 scrutiny
的构造反而一点都拿不到。它现在只是那条通路的入口。

## 闭包的类型(2026-07-30 裁决)

**块体闭包的 `return` 就是闭包的返回类型。** 收集 `return` 的那个 frame 以前
被 pop 掉就丢了,于是这类闭包一律是 `… -> Any`,而 `Any` 满足任何注解:

```lk
let f = |x| { return x + 1; };
let s: String = f(1);            // 以前:接受,运行时打印 2
```

命名 `fn` 一直是把收集到的 return join 起来的 —— 这不是新规矩,是闭包补上了
同一条。体自身的类型只在"它说了点什么"时才 join 进来:以 `return` 语句结尾的
块没有尾表达式,类型是 `Any`,放进去会把 union 吞掉。

**函数类型注解会流进 lambda。** 孤立推断的 lambda 是 `('T0) -> Any`,和为它写
的注解不 unify,所以 lambda 根本没法被注解 —— 而同一个绑定换成命名 `fn` 就通:

```lk
let f: (Int) -> Int = |x| { return x + 1; };   // 以前:类型不匹配
fn inc(x: Int) -> Int { return x + 1; }
let f: (Int) -> Int = inc;                      // 一直是通的
```

现在 `let` 的函数类型注解把形参类型压进 `check_closure`(调用点早就这么做了,
`calls.rs`),和 machine-int 字面量那条一样是**窄的**双向检查,理由也一样:
另一条路是一个没人能用的特性。

函数类型的拼法是 `(Int) -> Int`,**不带 `fn`**;`fn(Int) -> Int` 不是合法类型
(类型位置的 token 收集器根本不收 `fn`)。返回位置同理:`fn mk() -> () -> Int`。

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
