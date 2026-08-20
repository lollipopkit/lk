# LK 语义裁决(golden vectors)

本文档是 VM 与 native(AOT)行为分歧时的**第三仲裁**:差分测试
(`cli/tests/aot_differential_test.rs` 等)只能锁定"双方一致",不能回答
"哪一方是对的"。这里逐条写下已裁决的语义与期望输出;修改任何一条都必须
是显式的语言决策,而不是实现巧合。

除特别注明外,每条的期望输出都用当前 VM 实测值锁定,并被差分语料
(手写 69 例 + `examples/` 语料 + 生成式 fuzz)持续验证与 native 一致。

## 两个分支的值是并集,不是一次 unify(2026-07-31 记)

`if`/`else` 和 `try`/`catch` 的两臂类型不同时,值的类型是 **`A | B`**,和"一个
函数里两条 `return` 类型不同就是并集"同一条规则。

在这之前两臂是 unify 的,而这条规则和它自己都不一致:
`if c { xs } else { "x" }` 过得了检查(约束记下了,那条路上没人去解),
`try { xs.take(1) } catch e { "${e}" }` 直接被拒 —— 而后者正是 `catch` 最常见
的写法,因为被捕获的值渲染成文本。同一条规则两种结果,取决于走的是哪条路。

两条边界:

- 有一臂类型里还有**类型变量**时仍然 unify —— 那是推断没跑完,不是"这个值有
  两种类型";lambda 形参就是靠它学到自己装什么。
- `Any` 吸收一切:`Any | String` 比真相更窄。`xs[i]!` 展开成的空值检查,raise
  那一半是 `Any`,没有这条的话混合列表里每个 unwrap 都会变成 `Any | Elem`,
  然后算术就报错了。

带注解的位置照样拒绝,而且现在能把两半都说出来
(`expected Int, but expression has type List<Int> | String`)。

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

### 排序里的 NaN:有序,不是"和一切相等"(2026-07-30 裁决)

| 程序 | 期望 stdout | 说明 |
|------|-------------|------|
| `let z = 0.0; let n = z / z; return [n, 1.0].sort();` | `[1,NaN]` | NaN 大于每一个数,一个排好的列表读起来是升序的数后面跟着 NaN |
| `let z = 0.0; let n = z / z; return [n, n].sort();` | `[NaN,NaN]` | NaN 之间相等 |
| `return [0.0, -0.0].sort();` | `[0,-0]` | `-0.0` 与 `0.0` **仍然相等**(所以稳定排序保留输入序)—— `==` 说它们相等,`sort` 不该另立一条规矩 |

之前两个执行器的浮点比较器都是 `partial_cmp(..).unwrap_or(Equal)`,也就是
"NaN 和一切相等"。那**不是全序**(NaN == 1.0 且 NaN == 2.0,而 1.0 < 2.0,
不传递),而 Rust 的 `sort_by` 会检测到并 panic:

    user-provided comparison function does not correctly implement a total order

于是含 NaN 的 `xs.sort()` 会让解释器 Rust panic(`try` 抓不到),native 侧
abort。**打不打得中取决于数据**:601 个元素的列表过去了,60 个的没过 —— 这是最
糟的那种可达。混合列表同样中招,因为 `compare_runtime_values` 的三个涉及 Float
的分支用的是同一个比较器。

现在两端共用一条**全序**规则(`val::compare_floats`,lkrt 侧 `compare_floats`
镜像):NaN 之间相等、每个 NaN 大于每个数、`-0.0 == 0.0`。没有用
`f64::total_cmp`,因为它会把 `-0.0` 和 `0.0` 分开,那样 `sort` 就和 `==` 对同两个
值有两种说法。

装箱载体(混合列表)的 `sort` **不做原生降低**:它的序跨类型,要镜像的是两张
kind rank 表 + 深度受限的递归列表比较 + slice 视图 —— 那种规模的镜像该配自己的
一致性测试(见 `lkrt/src/vm_mirror.rs` 为 map/set 做的那样),不是抄一份。

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

## 越界读在**两端**都是 nil —— 上一条只补了一端(2026-08-01 补)

上面那条裁决把规则写成了 `element_position`:元素读越界给 `None`,不钳位。**这个
函数至今零个调用点。** 规则写下来了,调用点没照办,于是 List 读的负端漏了:

| | 大端越界 | 负端越界 |
| --- | --- | --- |
| `xs[10]` / `xs.get(10)`(List) | nil | **抛** `list index must be non-negative` |
| `s[-10]`(String) | nil | nil |
| `b[-10]`(Bytes) | nil | nil |

而那句消息本身是假的:**负索引是支持的**,`xs[-1]` 就是最后一个元素 ——
`list_dispatch` 自己的测试里,上一行断言 `set(-9)` 报"必须非负",下一行断言
`set(-1, 7)` 写成功。

更糟的是它盖住了一条真分歧:`xs[-10]` 解释执行**抛**、编译执行给 **nil**。两个后端
不一致,而差分语料里两边"错得一样"(都报那句假消息),所以一直是绿的。

现在:`xs[i]` 越界一律 nil,两端、两个后端一致;消息统一成 `list index N out of
bounds`。

**写的消息里那个 N 现在是写下的那个,两端两个后端一致(2026-08-05 补)。** 此前
VM 报的是解析后的下标 —— `xs.set(-9, v)` 在三元素列表上说 `-6`,一个程序从没写过
的数字。原因是负数在构造 key 时就解析掉了,而错误在几步之后的 store 才抛,那时手
里只剩解析值。改成**在解析点抛**:`negative_list_index_from_end` 同时握着原始下标
和长度,所以它是唯一能说真话的位置。lkrt 因此也不必再镜像一个更差的消息。

## 顶层 `let` 里的容器,`Bytes` / `Set` 不算容器(2026-08-01 裁决)

AOT 降低里有一张表 `container_ty`,同时决定两件事:哪些全局**保住自己的类型**,以
及哪些在槽位并到 `Dyn` 时**被拒绝**(回落而不是错编译)。它上面的注释把道理讲得很
清楚 —— 容器是句柄,装进形状不同的槽会造出第二个容器。

表里少了 `Bytes`、`Set`、`MapStrDyn`、`SliceI64`。少一项的后果是**两件事一起失
效**:既没保住类型,也没被拒绝。于是

```lk
let b = "abc".bytes();
fn f(n: Int) -> Int { return b[n] ?? -1; }
```

解释执行印 `98`,编译执行 `Error: runtime type error` —— **任何下标**都是,包括常量
下标。同一个值做参数或局部变量没事,`List` 和 `String` 全局也没事,所以没有任何
example、差分用例或 fuzz 种子碰到过它。AOT 覆盖门禁也看不见:它降低得很成功,只是
降低成了错的代码。

现在这张表写成**穷尽 `match`,没有 `_` 臂** —— 新增一个 `Ty` 必须在这里被归类,不
能默认落进"不是容器"。`every_handle_type_counts_as_a_container_global` 钉住分类本
身,而不是钉一个程序:属性是"每个句柄类型都在表里",一个程序一次只能显示其中一
个。

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
- `select`、`go`、后缀 `!` 均为 **parse 时糖**(分别降到隐藏 native
  `select$block`、`spawn(闭包)`、nil 检查 Conditional),不存在专用 AST
  节点;`select`/并发语义见 `docs/concurrency.md`。`try`/`catch` 曾经也是
  (降到 `try$call`),现在是真节点 `Expr::Try`,编译成 `TryBegin`/`TryEnd`。
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
- **catch-all 臂不带运行时判断**(2026-08-05 裁决):`_` 和绑定名匹配一切,
  为它发一条恒真的 `Test` 既是空耗,也造出一条不存在的"判断失败"边。两个后端
  都读这条边:全臂都 `return` 的函数在原生侧看起来仍能落到末尾,于是拒绝降低。
  绑定臂上它还是错的 —— `lower_pattern_match` 与 `if let` 共用,那里绑定的含义
  是"值非 nil",而 match 臂没有这个意思,`match nil { x => 1 }` 因此答 nil,
  `match nil { _ => 1 }` 答 1,而检查器把两者都当作全覆盖。现在两者都答 1。
  编译器认的 catch-all 比检查器**窄**:只有 `Wildcard` 和 `Variable`,
  or-pattern 保留判断(它不能绑定,省下的只有判断本身,而它的各分支条件可以
  求值任意表达式)。窄的方向是安全的 —— 多留一条检查器已证明不可达的落空边。
- **match 臂里的 `return` 从函数返回,每条臂各算各的**:编译器那个"后面是死
  代码"的标志曾在**臂与臂之间**被读,第一条臂的 `return` 因此跳过了后面每条臂
  的臂体降低,`g(1)` 落出 match、落到声明 `-> Int` 的函数末尾、答 nil。
  `every_match_arm_return_returns` 是它的门禁,其中决定性的一组是 match 之后
  再写 `return 99;`:控制流当时根本没在臂里停下。
- **条件表达式的每条分支各算各的返回**(2026-08-05 裁决):`lower_conditional`
  过去完全不碰"接下来是死代码"的标志,分支块里的 `return` 因此泄漏到整个条件
  表达式之外 —— `let a = if n > 0 { return 1; } else { 2 }; return a + 10;` 里
  后一条 `return` 被当成死代码丢掉,函数落到末尾答 nil。`if` 作语句、
  `try`/`catch`、`match` 都是每条分支各存各的,条件表达式是漏的那处。
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
所以这个名字撞不上任何程序能写出来的东西 —— `select$block` 用的是同一招。

**脱糖不能漏出来**:那两句报错走的是具名参数的措辞("Missing required named
argument: y"),而读者写的是字段。类型检查器现在认得 `Type$new` 这个 callee,
说的是 `Missing required field 'y' for struct 'Pt'` —— 和本地字面量逐字一样。

`use { Pt } from "types";` 绑定的正是那个生成的构造函数(2026-08-06 补):模块
导出的是**值**,而 `Pt$new` 就是一个普通的顶层 `fn`,也就是一个值。导入解析
先找同名导出,找不到再找 `Pt$new`,把它绑到 `Pt` 上。于是裸写法也成立:

```lk
use { Pt } from "types";
let a = Pt { x: 4 };   // 与 types.Pt { x: 4 } 是同一个身份
let b = Pt(x: 4);      // 同一条路,直接调构造函数
```

`Pt { … }` 的降低看编译器已知的两件事:本地 `struct S` 一定带来本地 `S$new`,
所以"没有本地 `Pt$new`、却有一个叫 `Pt` 的全局"就恰好是导入这一种情形,降到
对那个全局的具名调用;否则照旧发 `NewObject`。同名的本地声明优先,因为它带来
了本地 `Pt$new`。

`trait` 没有构造函数可绑,所以 `use { Shape } from "types";` 仍然报错,报的是
这条理由,不是"不是导出"。

只通过命名空间看见的类型(`use "types";` 而没有 item 导入)裸写字面量仍然被
拒:那里 `Pt` 没有绑定任何东西,`NewObject` 会盖上**当前**模块的 scope,建出
一个同名但没有方法的类型。报错点名两条出路:`types.Pt { … }`,或者按名导入。

类型检查器这一侧对应两个标记:`imported_structs`(这个名字是别的模块声明的)
与 `constructible_imports`(而且本文件按名导入了它)。字段仍然按**声明方**的
schema 校验 —— 多字段、少字段、类型不符都在检查期报,措辞与本地字面量一致。

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

## 被捕获的错误:消息是输出(2026-07-30 立)

响亮失败的契约一直是"比成功 + stdout,不比失败的文本"。那对**未捕获**的失败是对的
—— 那段文本是宿主的外壳。它对**捕获**的什么也没说,而在那里消息**就是 stdout**:

```lk
let r = try { xs[9] = 1; "no" } catch e { "${e}" };
println(r);
```

所以规矩是:**可捕获的 raise,两个后端的消息必须逐字一致;未捕获失败的文本不要求。**
`a_caught_errors_message_matches` 是它的门禁。

立这条时两边有七处不一样,包括 `assert` 差一个大写字母、所有动态类型错误在 native
侧都是一句 `runtime type error`(VM 会说出运算符和两个操作数的种类)、越界写说
`runtime error`(VM 说 `list index 9 out of bounds`)。

措辞以 VM 为准。立这条时 VM 自己有两处毛病,随后一并修了(见下),两边一起动 ——
这正是"先立门禁再改"的好处:门禁保证它们不会各改各的。

### 消息里的名字:类型,不是表示;运算符,不是 opcode(2026-07-30 修)

- **类型不是表示。** 消息格式化的是 `RuntimeVal::kind()`,它对堆句柄只会说
  `Object`。于是 `"ab" - 1` 说 `String`(≤7 字节,内联)而
  `"aaaaaaaaaa" - 1` 说 `Object` —— 同一个类型两个名字,分界线是它塞不塞得进七个
  字节;list / map / Set 也全是 `Object`。`RuntimeValKind::Obj` 的注释**早就写着**
  "拿得到堆的调用方应该用 `HeapValue::type_name`",只是没有一个调用点照做。
  现在有 `Executor::value_type_name`。
- **运算符不是 opcode。** `1 < "a"` 报 `CmpLtInt expected ...` —— 那是编译器挑的
  融合形式,源码里没有任何东西叫 `CmpLtInt`,而且它可以在程序没变的情况下改变。
  `operator_symbol` 这个映射**本来就在**,注释里连理由都写好了(算术那批就是这么
  修的),只是比较那批没跟上。现在跟上了。

两条都是同一个模式:规矩已经写下来了,调用点没遵守。

## 容器 display

| 程序 | 期望 stdout | 说明 |
|------|-------------|------|
| `println([1,2,3])` | `[1,2,3]` | 逗号分隔无空格;float 元素用 Rust `to_string`(`2.0`→`2`) |
| `println(["a","b c"])` | `["a","b c"]` | 字符串元素 **Rust `{:?}` 引号+转义**(`"`→`\"`、tab→`\t`) |
| `println("${xs}")`(xs 是 list) | `[1,2,3]` | 模板插值**显示容器**(2026-07-30 更正:此前这条写的是"响亮失败,标量 only",而 VM 早已不是那样)|
| `println("a${xs}b")` / `"m=${m}"` / `"${[P{v:1}]}"` | `a[1,2,3]b` / `m={"k":1}` / `[P{v:1}]` | 多段模板、map、结构体列表同样 |
| `println(map)` | 插入序 | map 的迭代与 display 顺序 = **键第一次被写入的顺序**(2026-08-17 裁决,见下)|
| `println(Set([1,2,10]))` | `Set([1,2,10])` | Set display **按成员值排序**:nil → Bool → Int(按数值)→ String(按内容,不分长短)。2026-07-30 修:此前排的是**渲染后的文本**,于是 `Set([1,2,10,20,3])` 打出 `Set([1,10,2,20,3])`。判据在 `RuntimeMapKey::display_order`,native 逐条镜像 —— 这个序是**强加的**、比的是内容,所以两边不可能因为 hasher 漂移而分开 |
| `for x in Set([5,6])` | hash 迭代序 | **迭代序不是显示序**:显示强加了排序,迭代没有。native 侧也降低了 —— 但它需要镜像纪律(显示不需要),前提是 lkrt 只有一份 `RtKey`,见 `set_iteration_order_matches_the_vm` |

## map 的顺序是插入顺序(2026-08-17 裁决)

`Map` 的迭代序、`.keys()`/`.values()` 的顺序、以及 `println(m)` 的顺序,都是**键第一次
被写入的顺序**。重复写同一个键更新它的值、**保留它原来的位置**;删除保留其余键的相对
顺序。结构体实例同理 —— 它的字段序就是 `struct` 声明的顺序,而 `p.z = v` 追加的新字段
排在最后。

在此之前是**哈希表的迭代序**,而那不是值的属性,是**这张表怎么被建出来的**属性:

```lk
let a = {"zebra": 1, "apple": 2, "mango": 3, "kiwi": 4};
let b = {};
b["zebra"] = 1; b["apple"] = 2; b["mango"] = 3; b["kiwi"] = 4;
// a == b,而两边 println 出来的字段顺序不同。
```

两个 `==` 的值渲染不同,是这条裁决要消掉的东西。

**它同时消掉了一个跨后端的巧合。** 原来两个后端能对上这个序,靠的是 lkrt 用
`vm_mirror` **复刻解释器的哈希布局**:两边链同一个 `hashbrown`、同一个 rustc 推导出
同样的 `derive(Hash)` 判别值、同一个固定种子。现在两边都往一个向量里追加,序是**结构性
相同**,不是碰巧相同。`vm_mirror` 的一致性测试留着,但它守的不再是一个巧合。

**代价与偿还**(基准是 `bench/run_workload_bench.sh`,10% 门禁):有序 map 的载体比裸哈希表宽
8 字节,`HeapValue` 从 64 涨到 72;而有序 map 让 `RuntimeObject::field_slots`(一张手工
维护的平行字段序表)成为冗余,删掉它省回 24 字节。净结果 geomean 与基线持平
(1.015x vs 1.016–1.023x,三轮)。

### 一条过时的裁决(2026-07-30 更正)

上面那条曾经写着:`ToString` / 模板插值 / `+` 拼接走"标量 only"的显示路径,
容器在那里是响亮失败。VM 后来改了 —— `"${xs}"` 就是 `[1,2,3]` —— 而**AOT 一侧
一直照着退休了的规则**传 `containers: false`,于是任何模板里带 list / 结构体
列表的程序都掉回 VM。答案一致,只是慢,所以差分门禁抓不到;是探针撞上的。

现在两边都显示容器,差分语料补了这条。map 两种键都进来了(见下),
Set 的显示、相等、迭代都进来了。迭代那条要的是哈希序的镜像(显示不要),
前提是 lkrt 只有一份 `RtKey` —— 它本来有两份。

### 又一条(2026-07-30 更正):字符串键 map 的显示

上面那条"map 不进原生子集"同样是退休的裁决。它先于 `lkrt/src/vm_mirror.rs`,
而那个模块存在的全部意义就是让两个后端共享 map 的迭代序,
`lit_protocol_matches_vm_iteration_order` 直接拿 `lk-core` 比对。`MapStrDyn`
其实早就放行了 —— 裁决对一个 map 类型解除、对其余的留着,于是
`println({"a": 1})` 让程序丢掉降低,而 `println({"a": 1, "b": "x"})` 不会。

现在字符串键和整数键都显示。整数键这条修的时候顺手挖出一个**潜伏的错答案**:
VM 对非字符串键不做第二阶段(`typed_map_from_entries` 直接返回 `Mixed`),
lkrt 的 `lit_finish_i64_*` 却又 rehash 了一遍,两边顺序真的不一样 ——
只是当时没有哪条路看得见它。载体已改成按 `RtKey::Int` 哈希、按字面量序重放,
细节记在 `docs/aot/aot-gaps-and-lkrt.md` §17.2。

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

`==`、`in`、`unique()` 三者现已同规则,常量折叠亦然。

### 嵌套容器的句柄同一性限制已取消(2026-08-20)

此处曾写着"长字符串/嵌套列表的句柄同一性限制与 unique() 同款(intern/转换
边界,已留档,不进差分子集)"。那句话描述的是 native 侧的实现,不是裁决:
lkrt 的 `contains_eq` 按句柄比较堆值,而 VM 的 `list_contains` 已经改成调
`runtime_values_equal`。两边不一致,而"不进差分子集"这句话正好挡住了发现它
的唯一手段 —— 与上一节 `unique()` 的漂移同一个成因,同一份文档,相隔一节。

七条表达式因此编译后答错、解释执行答对:

| 表达式 | VM | 此前 native |
| --- | --- | --- |
| `[1, 2] in [[1, 2], [3]]` | true | false |
| `{"k": 1} in [{"k": 1}]` | true | false |
| `1.0 in [1, "a"]` | true | false |
| `[[1], [2], [3]] - [[2]]` | `[[1],[3]]` | 原样 |
| `[{"k": 1}, {"j": 2}] - [{"k": 1}]` | `[{"j":2}]` | 原样 |
| `[[1], [2]].index_of([2])` | 1 | nil |
| `[1, "x", 2].index_of(2.0)` | 2 | nil |

现在 `contains_eq` 已删除,native 侧 `in` / `-` / `index_of` / `count` 全部走
`dyn_eq_inner`(即 `==` 那份),嵌套与混合的形状进了
`differential_equality_and_unique`。**这类形状不再有豁免**:两个后端必须一致
的规则,只要被划出差分子集,漂移就无人可见。

唯一仍然不是 `==` 的载体是字节串:VM 的 `in` 只接受 `RuntimeVal::Int`,所以
`97.0 in "ab".bytes()` 是 false,而 `97.0 in [97]` 是 true。这条是裁决,不是
实现细节,已在 lkrt 里显式写出并进差分语料。

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

**这条规则此前只在运行期生效**(2026-08-18 补)。它写在 `TypeRegistry::validate_trait_impl` 里,
而那个函数是 **VM 注册 impl 时**调用的,所以 `lk check`——文档说它是"执行器跑的同一个检查"——
会放过一个第一行就停的程序。缺方法那一半早就补到 `Stmt::Impl` 的类型检查里了,多方法这一半没有。
现在两半在同一处。

发现方式:写了 32 个"应该被拒绝"的程序丢给 `lk check`,看哪些通过了。通过的两个里,
一个是顶层 `return`(合法,模块的返回值),另一个就是这条。

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
- **auto-Display 只有 `show` 这一个钩子**,面向用户的文档曾说有三个。
  `LEARN.md` 写着"实现 `show`、`display` 或 `to_string`,`println` 就会用它",
  而查找是硬编码的 `"show"` —— 写 `display` 的人看到的是默认渲染,没有报错。
  取一个名字而不是三个:一个钩子的第二种拼写正是这个代码库一直在删的东西,
  而 `#[derive(Show)]` 生成的也是 `show`。
  `auto_display_uses_show_and_only_show` 钉住这条,文档与查找不能再各说各的。
- **auto-Display 只镜像 `show`**:VM `try_runtime_display_show` 硬编码查
  方法名 `"show"`(与 trait 名无关;`#[derive(Debug)]` 展开出的
  `__LKShow::show` 也走它)。native 在 display 上下文(print/println 参数、
  模板插值 `ToString`/`ConcatString`/`ConcatN`)对带 provenance 的 struct
  直调注册的 `show`。**无 `show` impl 的整对象 display 不进子集**(VM 内部
  有 `<Type {...}>` debug 形与 registry 缺失 bail 等多种路径,未统一前不复刻)。

动态分发(boxed receiver,经混合列表/Dyn 参数流动)限 `argc == 0`(self 之外
无参数)且零捕获 impl;静态 devirt(NewObject provenance 已知)支持任意参数。
分发臂按注册序排列,标记无匹配 → raise(VM 的 unknown-method 同为错误)。

## 容器方法的拼写:contains / has / delete(2026-07-31 记)

同一个问题在不同容器上叫什么,是查表查出来的,不是猜的:

| 容器 | 在不在里面 | 按键/值删 |
| --- | --- | --- |
| list | `contains(v)`、`v in xs` | `remove_at(i)`(按下标) |
| set | `contains(v)`、`v in st` | `delete(v)` |
| string | `contains(s)`、`s in text` | — |
| map | `has(k)`、`k in m` | `delete(k)` |

规则是:**能不含歧义的地方一律 `contains`,map 用 `has`** —— 因为对 map 而言
"contains 什么,键还是值?"是个真问题(Java 就得分成 `containsKey` /
`containsValue`)。`in` 在四种容器上都可用,是那个统一的写法。

这条记下来,是因为 AOT 的 Set 降低臂曾经同时接受 `has` 和 `remove`,而类型检查器
两个都拒 —— 于是那两个名字永远到不了降低,读代码的人却会以为 `st.has(x)` 能用。
删掉它们时顺手把规则写在这里,免得下一个人朝相反方向"修"。

## `m.x` 在 map 上是取键,而方法优先(2026-07-31 记)

一个 map 的成员访问有两个意思,而它们都成立:

```lk
let m = {"a": 1};
m.a          // 取键 "a" —— m["a"] 的写法糖
m.len()      // map 的方法:条目数
m.f(1)       // 键 "f" 存的是函数时,调用它
```

**同名时方法赢。** `{"len": 5}.len()` 是 `1`(条目数),不是"调用 5";那个键仍
然读得到,写 `m["len"]` 或 `m.len`(不带括号)。

**这条 2026-07-31 当天就被发现只在 `len` 上成立(同日修)**:写下它时只探了
`len`,而 `len` 恰好有自己的 opcode,根本没走到分派器。分派器里键查找排在内建
方法分派**前面**,于是 `{"keys": 5, "z": 1}.keys()` 答 `5`、
`{"is_empty": 5}.is_empty()` 答 `5` —— 拿到方法还是键,取决于编译器有没有给那
个方法单独发指令。现在内建方法先分派,没有同名内建时才查键(键里存的是可调用
值就调它,`m.f(1)` 这条形状不变)。
`a_map_method_is_not_shadowed_by_a_key_of_the_same_name` 钉住。

教训归档:**"读了一遍分派顺序"不等于探过**;一条裁决要按它覆盖的每一类名字各
探一个,否则写下来的是实现在某一个样本上的行为。

为什么是方法赢:另一种选择(键存在就用键)会让内建方法在某些 map 上凭空消失,
而消失得没有任何提示;方法赢至少是**同一个名字在所有 map 上是同一件事**,并且
被遮住的键有一个不含歧义的写法(`m["len"]`)可用。

和结构体那条(见"一个名字一个意思")的区别:结构体的字段名是**声明**出来的,
所以同名可以在声明处直接拒;map 的键是运行时数据,没有声明处可拒,只能定一条
优先级。

## 常量条件不会藏起没走的那一臂(2026-07-31 裁决)

```lk
let x = if false { undefined_fn() } else { 1 };   // 报 undefined_fn
let y = false && undefined_fn();                  // 一样报
let z = -true;                                    // 一样报:Bool 不能取负
```

以上三条此前**全部静默通过 `lk check`**,`-true` 还会打印 `false`。

原因是常量折叠 `Expr::fold_constants` 跑在 **parser 里**,早于名字解析和类型检
查;而它做的不只是"算",还会整棵丢掉一个子树 —— 条件恒真/恒假时选一臂、`&&`
/`||` 短路、`??` 左边是常量。被丢掉的那棵子树后面没有任何人看过。`-true` 是同
一个错误的另一半:折叠不看 op,把任何一元运算作用在 Bool 字面量上都当成 `!`。

**规则:解析期折叠可以"算",不可以"删"。** 一次二选一的折叠只有在**被丢弃的那
一侧本身已经是字面量**时才允许(字面量没有什么可检查的)。因此:

- `false && true` 仍折成 `false`,`nil ?? "a"` 仍折成 `"a"`,`if false { 1 } else { 2 }` 仍折成 `2`;
- `false && f()`、`if false { f() } else { 1 }`、`1 ?? f()` 不折,`f()` 照常过检查;
- `-3` 折,`-true` 不折 —— 交给类型检查器报"取负的操作数必须是数值"。

**短路仍然是运行期语义**:`false && (1 % z == 1)`(z 为 0)不会求值右边、不会
raise,这是执行器做的事,和折不折无关。(探针用 `%` 而不是 `/`:`/` 是浮点除
法,`1 / 0` 是 `inf`,根本不 raise,那样的探针求不求值都一样过。)恒定条件的分支消除属于优化,归类型检查之后的 VM
编译器和 AOT 后端,那里两边都看得见常量。

顺带修掉的:恒定条件的 `if` 表达式此前只报活下来那一臂的类型,于是顶层
`let x: Int = if false { 9.5 } else { "x" };` 说 `String`,函数体里同一行说
`Float | String`。现在两处都说并集。

## 下游关掉管道 = 程序停下,不是 panic(2026-07-31 裁决)

```sh
lk gen.lk | head -1
```

解释器此前打的是:

```
thread 'main' panicked at library/std/src/io/stdio.rs:1166:9:
failed printing to stdout: Broken pipe (os error 32)
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
```

退出码 101。而**原生编译出来的二进制一直是对的** —— 它的 `main` 是 C `main`,
Rust 的启动代码没跑过,于是它按 Unix 惯例被 SIGPIPE 杀掉(shell 报 141),一声
不吭。两个后端不一致,且对的是 native 那边。

原因:Rust 在 `main` 之前把 `SIGPIPE` 设成 `SIG_IGN`,写关闭的管道于是返回
`EPIPE`,`println!` 再把它 unwrap 成 panic。这既违反"错误文本说语言的话,不说
实现的话",管道又恰恰是 shell 对一个会打印的程序最日常的用法。

**裁决:`lk` 在 `main` 开头把 `SIGPIPE` 恢复成 `SIG_DFL`**,和 `head`、`grep`
以及原生二进制一样。行为:下游关掉读端后程序立即停止,不打印任何东西,退出状
态是"被信号 13 终止"。非 Unix 平台不适用。

`cli/tests/broken_pipe_test.rs` 钉住这条,并且正反两向验过。

## `typeof` 说结构体的名字,不说 `Object`(2026-07-31 裁决)

```lk
struct S { a: Int }
typeof(S { a: 1 })   // "S"
typeof({"a": 1})     // "Map"
```

此前 **两个执行器给的都是错答案,而且互不相同**:解释器答 `Object`(堆表示的
名字,不是语言里的类型),原生答 `Map`(结构体和普通 map 共用 `MapStrDyn` 载
体,静态表就照载体答)。`typeof` 对最想问的那类值恰好没用。

这是同一条规矩第三次在下一层被发现有洞:
`RuntimeValKind::scalar_type_name` 的注释写着"手里有堆的调用者请改用
`HeapValue::type_name`",而 `HeapValue::type_name` 自己对每个结构体实例答
`Object`;lkrt 里那份"镜像"(`kind_name`)也一样。三处都改了 ——
`HeapValue::type_name` 答声明名,变体自己的拼写挪到 `representation_name`;
lkrt 读运行时的 type 标记。

原生侧的静态表因此**去掉了 `MapStrDyn`**:它同时是结构体载体,静态答不出来。
降低时若能指名结构体(`ssa.struct_types`)就发字面量,否则装箱调
`dyn.type_name` 让运行时读标记 —— 猜 `Map` 有一半时候是错的。

顺带,所有拿 `HeapValue::type_name` 拼错误消息的地方(三十余处)也跟着说对了:
`p.len()` 现在报 "`len()` has no answer for S",不再是 `Object`。

## `-> T` 是对**每一条路径**的承诺(2026-07-31 裁决)

```lk
fn g(c: Bool) -> Int { if c { return 1; } }   // 现在:lk check 报错
```

此前这段过检查,`g(false)` 答 `nil`,失败在调用点才现形:
`Add expected numbers or strings, got Nil and Int` —— 报的是运算符,离那个承诺
了 `Int` 的函数三个栈帧。

**裁决:声明了返回类型且该类型不接受 nil 的函数,必须每条路径都离开。** 判据只
认**可证明的离开**:`return`、两臂都离开的 `if/else`、带 catch-all 且每臂都离开
的 `match`、`error(...)` / `panic(...)`、没有 `break` 的 `while true`。不认识的
构造一律算"可能落到末尾" —— 那只会要求多写一个 `return`,不会漏放。

不受此约束的三类,因为它们本来就接受落空值:`-> Nil`、`-> Any`、`-> T?`。没写
注解的函数返回类型是**推断**出来的,没有承诺可违反。

实现在 `core/src/stmt/stmt_impl/flow.rs`,`a_declared_return_type_is_a_promise_about_every_path`
钉住(含全部"确实每条路都离开"的写法)。整个 examples / bench / 两个裸机 corpus
零误报。

## `u64` 的上半区,十进制和十六进制都写得出来(2026-07-31 补)

```lk
let a: u64 = 0xFFFFFFFFFFFFFFFF;    // 一直可以
let b: u64 = 18446744073709551615;  // 现在也可以;此前是 `Invalid int`
```

语言有 `u64` 类型,却只有十六进制能写出它 `i64::MAX` 以上的值 —— 同一个数,一
种拼写收下、另一种报语法错。词法器的 radix 路径把字面量按 **u64 位模式**读、超
出 i64 就发 `Token::UInt`;十进制路径只有 `i64::from_str`。补上后者。

边界没有变:`-18446744073709551615` 仍然两条 parse 都不过(负号是 `num` 的一部
分),`let y: u8 = -1` 仍然被拒;超过 `u64::MAX` 的报"integer literal out of
range(Int 是 i64,u64 是最宽的)",而不是原来的 "Invalid int" —— 那个数不是写
错了,是超范围了。

`Token::UInt` 现在带着**写下时的进制**,所以 `lk macro expand` 把它按原样打回
去,不会把十进制的数重拼成十六进制、也不会把二进制掩码打成十六进制。**`Token::Int`
仍然不带进制**(它是最常见的 token,加字段要动 80 处),所以 `0x3F20_0000` 这
种能装进 i64 的掩码经过 `macro expand` 仍会变成十进制 —— 已知的保真缺口,记在这
里而不是假装没有。

## 容器当实参传进去,被调方的写就是调用方的写(2026-07-31 裁决,部分已修)

```lk
fn mk() -> List<Int> { return [1]; }
fn add(xs: List<Int>, n: Int) -> Int { xs.push(n); return xs.len(); }
let xs = mk();
add(xs, 2);
println("${xs}");        // [1,2] —— 两个执行器现在一致
```

原生此前答 `[1]`:**错答案,不是回落**,程序照跑还打出一个看着合理的长度。

根因是**类型化列表是唯一一个装箱时会重建容器的载体**(`list_h.i64_to_dyn` 逐元
素装箱成新表)。集合、bytes、窗口、dyn 列表和五种类型化 map 都是原地打标签装箱
—— `DYN_RAW` 的注释早写下"装箱不得重新表示一个容器",类型化 map 正因违反它被修
过。类型化列表是这条规矩最后没补的一格。

**已修的触发路径(2026-07-31)**:形参之所以被拓宽成 `Dyn`,是因为 fixpoint 的
**第一遍**在被调方返回类型还停在 `I64` 默认值时就记下了实参观测,而形参格是**单
调 join** 的:第一遍的 `I64` 和第二遍真实的 `list<i64>` 一 join 就变 `Dyn`,此后
每个调用点都装箱 —— 由一个从来不成立的事实推出来的。`ret_known` 早就为 HOF 重路
由记下了同一个坑,形参格没跟上。第一遍的观测现在整体不记(它的产物本来就丢弃)。

**第二条未修的触发路径,而且更糟:把类型化列表*存进另一个容器*。**

```lk
let inner = [1];
let outer = [inner];
inner.push(2);
outer[0].len()      // 解释执行 2,编译执行 1
```

两个方向都断 —— 改 `outer[0]` 也照不到 `inner` —— 所以是 `[inner]` **构造那一刻**
就拷贝了。放进 map 一样(`{"k": inner}`)。而把 **map** 放进 list 没事:map 只有
一种表示,装箱是打标签。

和下面那条不同的是观测面:那条是**回落**(慢但对),这条**编译成功且静默答错**。
61 程序扫描和 fuzz 都没这个形状,所以一直没人看见。

**三条临时判据都验过了,都不成立**(记下来,免得下次再走一遍):

1. **一刀切:元素是类型化 list 就拒绝装箱。** AOT 覆盖从 60 掉到 58 ——
   `examples/stdlib/{iter_pipeline,list_iter_sugar}.lk` 造的是
   `[[0,"a"],[1,"b"]]` 这种字面量嵌套(反汇编看到的是 `LoadHeapConst` 经 `Move`
   链喂进 `NewList`),元素是新建的、没有第二个引用,重建不可观测,拒绝它们纯亏。
2. **寄存器活跃性:装箱点之后这个寄存器还被读吗?** 不健全 —— 一个也能从全局到达
   的值,它的寄存器是死的:`let g = [1]; fn f() { let outer = [g]; g.push(2); }`
   里 `g.push` 会重新 `GetGlobal` 到另一个寄存器。
3. **具名槽边界:`reg < 具名局部数` 才算可命名。** 没有这条边界 ——
   `Ssa::slot_count` 就是 `reg_count + cell_capacity + capture_count`,全部寄存器
   一视同仁。

剩下的健全判据是**新鲜性**:这个值是本函数里由 `NewList`/`NewMap`/`LoadHeapConst`
造出来的,**并且**之后没有再被读。两半都要 —— 前一半排掉全局(2 的漏洞),后一半
排掉 `let inner = [1]; let outer = [inner]; inner.push(2);`(inner 是新鲜的,但还被
读)。

**而根治比"五条被推翻的缓解"听起来的要有谱:类型化 map 已经这么修过。**
`lkrt/src/lkdyn.rs` 有一段 `DYN_TMAP_BASE..DYN_TMAP_END` 的标签区间,
`lkrt_dyn_from_typed_map(handle, kind)` 把类型化 map **按标签**装箱,五个
`lkmap::KIND_*` 各占一格 —— 装的是句柄,不重建。类型化 list 要的是同一个形状的
`DYN_TLIST_*`(i64/f64/str 三格),而不是一条没人走过的新路。

**未修的触发路径**:同一个模块里有**两种列表载体**各自流进会改写它的函数时,形参
格 join 成 `Dyn`,装箱又回到重建那条路。最小复现:

```lk
fn mk() -> List<Int> { return [1]; }
fn add(xs: List<Int>, n: Int) -> Int { xs.push(n); return xs.len(); }
fn adds(xs: List<String>, s: String) -> Int { xs.push(s); return xs.len(); }
let xs = mk();  add(xs, 2);           // 单独看:两端一致
let ss: List<String> = [];
println(try { "${adds(ss, "a")}" } catch e { "c" });
println("${xs} ${ss}");
```

**这个复现已经过期(2026-08-06 复核)**:它当时答 `[1,2] []`(原生看不见
被调方的写),现在**回落** —— 形参格不再 join 成 `Dyn`,而是冲突并拒绝降低
(`in \`adds\`: an operand at pc 0 is a str where a i64 is required`)。
显式写 `fn add(xs: List<Any>, …)` 那条也关上了:#184 让可变容器不变,
`List<Int>` 不再能传进 `List<Any>`。

重建那条 ABI 调用**仍在发**(语料里 8 个文件命中 `list_h.*_to_dyn`),所以
表示层的问题照旧;换掉的只是它露头的形状。现在露头的是**拓宽**:两种载体
流进同一个 `Any` 形参再 push,VM 就地拓宽答对、原生 raise。最小复现和四条
被实测否掉的便宜修法见 `docs/aot/aot-gaps-and-lkrt.md` §23。

**这条已经证明不能在格上绕过。** 试过把"元素类型只是猜的空 `[]`"在进 try 区域时
物化成 Dyn 列表:`ss` 修好了,`xs` 反而坏了 —— 新的 `ListDyn` 观测和原有的
`ListI64` 在同一个被调方上 join 成 `Dyn`,于是**两个**调用点都开始装箱。换一个程
序坏而已。

所以唯一的修法是把重建去掉:给 ListI64/F64/Str 各一个 tag(接在 `DYN_SLICE = 15`
之后),**原地打标签装箱**,照 `DYN_TMAP_BASE` 那次做。已知难点:VM 的
`TypedList` 在插入不合型元素时会拓宽成 `Mixed`,而原生的 `Vec<i64>` 不能就地变身
—— 这条语义必须逐字镜像,不能近似。

## 常量折叠只是**捷径**,不是第二门语言(2026-07-31 裁决)

折叠跑在类型检查器**之前**。所以折叠器答得跟执行器不一样的每一条,都是任何诊断
都够不到的一条 —— 检查器根本没见过那棵子树。两种走偏方式,都修了:

**一、它实现了语言里没有的运算。** 类型检查器专门删掉了字符串重复,报错还点名了
真正存在的写法:

```
Type Error: `*` does not repeat a string — write `text.repeat(count)`
```

而折叠器仍然实现着它。于是一个程序碰到哪条规则,取决于计数是不是字面量:

```lk
let a = "ha" * 3;      // 折出 "hahaha"
let n = 3;
let b = "ha" * n;      // Type Error: `*` does not repeat a string
```

三份文档(`examples/syntax/unsupported.lk`、中英两份 LEARN)因此一直宣称这个特性
可用 —— 它们能通过,靠的正是这条本该不存在的折叠。**检查器的规则就是语言的规则**,
折叠里的那条是被删特性的残留。

**二、它的整数运算用裸算符。** 两个执行器的 Int 运算都是回绕(`i64::MAX + 1` 给
`i64::MIN`,`i64::MIN % -1` 给 `0`),而 Rust 的 `a + b` 在 debug 构建里 **panic**、
在 release 里回绕。于是源码里写一个 `9223372036854775807 + 1`,解析器要么崩:

```
thread 'main' panicked at core/src/expr/expr_impl.rs:775: attempt to add with overflow
```

要么碰巧折对 —— 取决于 `lk` 自己是用哪个 profile 编的。现在一律 `wrapping_*`,把
规则写出来。

**三、`a ?? b` 折成 `a`,把 `b` 从检查器眼前删了。** `??` 要求两侧能 unify,所以丢
掉一侧就是丢掉那条类型错误:

```lk
let a = 7 ?? "ab";              // 折成 7
let b = maybe_int() ?? "ab";    // Cannot unify Int with String
```

把折叠器和运行时按「14 个运算符 × 5 种字面量类型」做全矩阵差分,**14 处不一致全部
是这一条**。现在只折 `nil ?? e`(它丢掉的只有字面量 `nil`);`7 ?? 0` 少折一次的代
价,是运行时多走一个分支 —— 而这种写法没人写。

三条是同一件事:**折叠器在替类型系统做决定,而它没有类型系统。**

`constant_folding_answers_what_the_executors_answer` 钉住三条。

## `lk check` 答的必须是执行器答的那个问题(2026-08-01 裁决)

已经有一条裁决说 `lk check` 不能**放过**跑不起来的程序(trait 必需方法那条)。反
向同样成立,而这一边一直是错的:`lk check` 用 `TypeChecker::new_strict()`,两个执
行器都用 `TypeChecker::new()`。于是

```lk
fn process_list(xs) {
    return xs.filter(|x| x > 3).map(|x| x * x).reduce(0, |a, b| a + b);
}
```

- `lk examples/syntax/closure.lk` —— 跑通
- `lk compile examples/syntax/closure.lk` —— 编出原生可执行文件
- `lk check examples/syntax/closure.lk` —— **Type Error: infers implicit Any**

语言自带的 4 个 example 被"跑之前先检查"这条命令拒了,而它们是能跑的。未标注的
形参**是**这门语言收下的写法,所以严格性是一条 rigor 政策,不是"这程序能不能跑"
的答案 —— 政策不能当默认答案。

现在:`lk check` 默认与执行器逐字相同;`lk check --strict` 保留那条 lint。

顺带记一个探这条时的岔路:`VmContext::with_type_checker(Some(TypeChecker::new_strict()))`
在 CLI、REPL、wasm 三处都写着,读起来像"运行也是严格的" —— **不是**。
`Program::execute_with_ctx_from` 给程序的类型检查另建了一个 `TypeChecker::new()`,
上下文里那个只用来登记模块的 trait/impl 表。这三处的 `new_strict()` 就严格性而言
是句空话。(先别删:那个字段本身有人写、有人读,只是没人读它的严格位 ——
`get_type_checker_mut` 至今零调用点,值得单独查。)

## REPL 回显什么,不能由"这串输入碰巧是不是合法语句"决定(2026-08-01 裁决)

REPL 的契约是"敲一个东西,看见它的值"。而它此前是**先试语句解析、失败了才回退到
把输入包成 `return (…)`**。绝大多数表达式需要分号才算语句,所以它们落到回退路径、
回显了;而所有**自成语句**的东西,值算出来就丢:

```
> [1, 2, 3]                       [1,2,3]
> x + 1                           2
> if true { 1 } else { 2 }        (什么也没有)
> S { x: 8 }                      (什么也没有)
> match n { 1 => "one", _ => "" } (什么也没有)
```

回显的那一半是**语法上碰巧**的,不是规矩。

现在:**先试表达式**。整串输入是一个表达式就当表达式求值并回显,否则当程序跑。包
装是 `return (…)`,所以带分号的、`let`、声明、多条语句都不会被当成表达式 —— 这也
正是 `x + 1;` 用尾分号压掉自己回显的机制。

`cli/tests/repl_echo_test.rs` 把三组都钉住:此前静默的、此前正常的、以及
`println("x")` 只印一次(不能又印又回显 nil)。

**"这行输完了吗"也不能数字符。** 同一个文件里,`should_continue_multiline` 逐字符
数 `(`/`{`/`[`,分不清括号和字符串/注释**里**的括号:

```
> let s = "(";
> s
Error: Syntax error: Unexpected tokens at end (found Let) at 2:1-2
```

会话在等一个从没缺过的 `)`,把下一行吞进同一段输入,然后怪那一行。`// (` 结尾同
理。改成问**词法器** —— 定义什么是字符串、什么是注释的正是它。词法不过的输入(比
如没闭合的引号)不算"继续等",那是解析器该报的话:等下去会让任何一个手误挂死会话。

顺带删掉同文件里的 `normalize_binary_signs`(约 70 行):它在解析前把 `a+1` 重写成
`a+ 1`,给一个**并不存在**的词法行为打补丁。带/不带做了 15 条输入的逐条对照
(`a-1`、`a--1`、`-a`、`[1,2][0]-1`、`"a-1 ${a-1}"` 等),输出逐字相同。它只有钉
住"它做了什么"的测试,没有一句说明"为什么需要它"。

## `in` 里的堆值一律按句柄比 —— 每一种载体,不是当时有标签的那两种(2026-08-01 裁决)

```lk
let b = "ab".bytes();
let xs = [b];
b in xs            // 解释执行 true,编译执行 false
```

`Set`、`Bytes`、窗口、类型化 map 四种载体都是这样:**永远不在任何列表里**。原因是
`contains_eq` 的末尾是 `_ => false`,而它写下时的标签空间只有 `DYN_LIST` 和
`DYN_MAP`。后来 `DYN_SET`、`DYN_BYTES`、`DYN_SLICE`、`DYN_TMAP_BASE..` 依次加进
来,这个 match 没人回来看 —— 新标签**静默地**掉进了"跟谁都不相等"那一格。

这四种都是**原地装箱**(标签是唯一变的东西),所以它们的 payload 就是 VM 拿来比的
那个句柄,上面那条 `payload ==` 本来就是对它们的正确答案。补上即可,不需要新规矩。

`DYN_RAW` 不在内:它停放的是一个**不是值**的句柄,把它当值读是设计上的响亮失败。

`==` 那条没有同样的洞(六种载体实测全同)。剩下的 `l in [l]`(List 放进 List)仍然
两边不一致,那是类型化列表装箱重建那条,记在上面。

`every_heap_carrier_is_found_by_handle` 钉住四种载体各自找得到自己、且不同句柄仍然
找不到。

**同一形状在这个运行时里是第三次了**,所以按判据把所有对 tag 的 catch-all 扫了一
遍。`raise` 结尾的那些是响亮失败(可接受);**返回值**的那几个里又有一处:
`json.stringify` 的 `_ => Err("value has no JSON form")` 吞了 `DYN_SLICE`。窗口在
VM 里就是个列表,`json.stringify([xs.slice(0,2)])` 那边给 `[[1,2]]`,这边报"没有
JSON 形式"。`DYN_SLICE` 同样是后加进标签空间的。

三次的名单,留给下一个往标签空间里加东西的人:降低侧的 `container_ty`、`in` 的
`contains_eq`、JSON 的 `to_serde`。加一个 `DYN_*` 就要走一遍这三处 —— 它们都不是
穷尽匹配,编译器不会提醒。

## opcode 判别式必须连续 —— 一个洞值 9%(2026-08-05 裁决)

删掉 `LoadNative`(一个任何生产路径都没发射过的 opcode)之后,工作负载几何均值从
0.99 掉到 1.08。三次量在删除侧:1.075 / 1.086 / 1.089;HEAD 侧:0.991 / 0.986。
不是噪声。

二分到最小改动 —— **只**删 `Opcode` 变体和它的分发臂,`Module.natives`、公开 API、
artifact 检查全部留着 —— 仍然是 1.077 / 1.087。再把 72 号后面的 opcode 依次前移填
洞,回到 0.994 / 0.987。

所以代价是**洞**,不是少一条臂:dispatch 那个 `match` 只在判别式稠密时降低成跳转
表,一个缺口就够它退化。

这一点此前没有任何东西在守 —— 下一次删 opcode 会照付 9%,而且没有测试会红、审阅
的人也看不出为什么。`opcodes_are_contiguous` 现在钉住它,而且钉的是**三个**从字节
解码的枚举(`Opcode` / `InstrFormat` / `CastTarget`)—— 只守被量到的那一个,守的是
这次事故而不是那条性质。

**断言的形式改过一次,因为第一版守错了东西。** 三个解码函数都是手写的字面量
`match`,和判别式无关 —— 只断言"解得出的字节是连续的",在 `Sj = 40` 配
`4 => Some(Self::Sj)` 时照样通过:解码连续、枚举稀疏、跳转表没了。现在断言的是
**往返**:每个解得出的字节 `v`,`decode(v) as u8` 必须等于 `v`。四种破坏方式都反向
验过(三个枚举各自"只改判别式",以及"只改解码臂"),全部变红。

重排编号会改变 artifact 编码,所以连带 bump `MODULE_ARTIFACT_VERSION`(16 -> 17)。

## stdlib 的第一个参数是主体 —— regex 是最后一个例外(2026-08-05 裁决)

`string` 的第一个参数叫 `text`,`bytes` 叫 `value`,`encoding` 叫 `source`,`hash`
叫 `data`,`path` 叫 `path`。`regex` 六个成员全部是 `(pattern, text)`。

这不是风格问题,因为两个模块里有同一个操作:

```
string.split(text, separator)
regex.split(pattern, text)      // 改前
```

同一个操作,参数颠倒,两个参数都是 `String`。写反了类型检查看不出来,运行也不报
错 —— `regex.is_match("a1b2", "[0-9]")` 答 `false`,`regex.replace("a1b2",
"[0-9]", "#")` 原样返回 `"[0-9]"`。第一次探这个模块的十二个用例全部像是模块坏了,
实际上是调用顺序反了。

`replace` 上此前有一条注释承认了这一点,并用 `named(text, replacement)` 让调用方
给参数贴标签绕过去 —— 在错的顺序上打补丁,而不是改顺序。

现在六个成员都是主体在前,其余参数一律 named-eligible,与
`string.replace(text, pattern, with, all)` / `named(pattern, with, all)` 同形。
`lkrt` 侧的六个 extern 函数、AOT 降低表的 `named` 拷贝和 `leading` 起点同步改;
`lowering_named_parameter_lists_match_the_stdlib_declaration` 守着后两者与声明一致。

**探过一条更强的规则,不成立:**"同类型的参数必须可命名"。全 catalog 有 32 个成员
不满足它(`string.contains(text, needle)`、`path.with_extension(path, ext)` …),
所以它是新造的判据,不是仓库现有的约定 —— 对这些成员,"主体在前"本身就定了序。

**成立的是更窄的一条:参数是对等项、没有主体来定序时,调用方必须能贴标签。**
`fs.copy(a, b)` 哪个是源、`math.atan2(y, x)` 哪个是 y、`random.int(min, max)`
哪个是上界,约定回答不了,交换后两边都跑得动且答案不同。按这条筛出十个成员,
第二个参数改为 named-eligible:`bytes.concat`、`fs.copy`、`fs.rename`、
`iter.chain`、`iter.zip`、`math.atan2`、`math.pow`、`random.int`、`stream.chain`、
`time.since`。`math.hypot` / `min` / `max` 也是对等项,但它们对称,交换无影响,
不在此列。

这次同时补上了守卫的另一半。`lowering_named_parameter_lists_match_the_stdlib_declaration`
只走降低表里**已经有名字**的行,所以"声明加了 `named(...)`、表里没加"它看不见 ——
而那种情况是静默的:`CallNamed` 找不到名字,命名拼写停止降低,整程序回落到 VM,
答案照样对。`every_declared_named_list_reaches_the_lowering_table` 补的就是这个方向,
反向验过:十个成员加完声明、表还没改时,它报出正好那五个有降低行的成员。

## 装箱的值必须**每一种读法**都认得两种表示(2026-08-05 裁决)

`#118` 把类型化 map 改成原地打标签,理由写在 `DYN_RAW` 上:装箱不能重新表示
容器。做对了,但只教会了三个消费点 —— `len`、显示、相等。

其余读法仍然先解箱。`dyn.as_map` 的答案是一个 `str_dyn` 句柄,而六种载体里只有
一种是 `str_dyn`,所以下面这些在 `LK_AOT_NO_FALLBACK=1` 下**编译成完整原生**,
运行时 raise `runtime type error`,而 VM 全都答得出:

| 形状 | 改前 | 改后 |
| --- | --- | --- |
| `c[0]["a"]` | raise | 答 |
| `c[0][3]`(整数键 map) | raise | 答 |
| `c[0].keys()` / `.values()` | raise | 答 |
| `c[0].has(k)` | raise | 答 |
| `c[0].delete(k)` | 编译失败 | 答,且原地删 |
| `for k in c[0]` | raise | 答 |
| `for x in [Set/Bytes/Str][0]` | raise | 答 |
| `"a" in c[0]` | 编译失败 | 答 |

**分发按操作,不按解箱。** 让 `dyn.as_map` 在遇到类型化标签时物化一份
`str_dyn` 返回,能答出上表里的四条读,然后**静默丢掉 `delete` 的写** —— 用错
答案换编译通过。所以每个操作各有一个 Dyn 层入口(`dyn.map_keys` /
`map_values` / `map_has` / `map_delete` / `map_pairs`),在运行时按标签分派到
载体自己的访问器,顺序也就是载体自己的顺序。

`for-in` 同理:它发的是 `dyn.as_list`,一个**列表**守卫,所以装箱的 map、Set、
Bytes、字符串在循环里全部 raise。VM 的 `to_iter` 不是"解出一个列表",是"这个值
按什么迭代",`dyn.to_iter` 现在照着写。

`in` 是第三处同形的:降低侧**根本没有 `Dyn` 干草堆这一臂**,所以整程序回落。
`dyn.contains` 按标签分派 —— map 测键(存了 nil 的键也算,所以不能用 get 再判
标签),其余载体测元素。

顺着这条查出 `in` 还漏了两种载体:`Bytes` 和窗口。两者都能索引、都有 `len`、
都能 `for`,`Bytes` 连 `contains` 方法都有 —— `in` 是唯一不把它们当容器的地方,
而且检查器、VM、降低**三处都缺**(所以不是放宽检查就完事)。`Bytes` 里放不下的
针值(`300`、`-1`、非整数)答 `false` 不报错,与"Int 列表里找字符串"同规矩。

连带删掉 `MethodRow::unbox_map` 整列:没有哪个名字再需要"先解箱成 map"了。

整数键那条单列一句,因为它不只是缺一个分支:`{3: 4}[3]` 是 **4**,而没有第 3 个
元素 —— 整数键落在 map 上是键不是位置。常量整数键直接降低到 `dyn.index`,不经过
`dyn.get`,所以规则写在实际到达的那一层。

## 类型化 list 装箱也是原地打标签(2026-08-05 裁决)

`#118` 对类型化 map 立的规矩,同样适用于 list,而这里丢的不只是顺序 —— 是两个
方向的别名:

```
let xs = [1];
let c  = [xs];
xs.push(2);      println(c[0].len());   // VM 2,native 1
c[0].push(9);    println(xs);           // VM [1,2,9],native [1,2]
```

两条都**完整编译成原生**(`LK_AOT_NO_FALLBACK=1` 通过)然后答错。原因是装箱走
`list_h.*_to_dyn`:逐元素重建成 `Vec<LkDyn>`,那是另一个列表。

`DYN_TLIST_BASE..DYN_TLIST_END` 三个标签,一个载体一个,`dyn.from_typed_list`
原地打标签。消费点全部改为认两种表示:类型名、转整数的报错、`+` 拼接、与窗口的
比较、跨表示的列表相等、显示、`len`(直接数载体,不装箱)、索引、`in` 的按句柄
比较、`flatten`、`to_iter`、JSON 序列化、通道深拷贝。

**`dyn.as_list` 是只读的,这条得写下来。** `DYN_LIST` 交回自己的句柄,写得进去;
类型化载体必须物化一份元素,写不进去。两者不能都从这一个口出去。到得了这个守卫
的名字 —— `map` / `filter` / `reduce` / `take` / `skip` / `concat` / `unique` /
`sort` / `reverse` —— 全部构造新列表,不动接收者(这门语言里 `sort` 和 `reverse`
返回新列表,不是原地排)。唯一的写是 `push`,它走 `dyn.list_push`,直接到载体。

规则靠 `no_unbox_list_name_mutates_its_receiver` 钉住:给一个会写的名字加上
`unbox_list`,写就会在这个守卫里被静默丢掉。

`sort()` 在装箱接收者上仍然回落,与本次改动无关,单列(见任务表)。

### 附带裁到的一条:检查器提升过的 push,VM 得把提升物化

补完标签之后探到的,而且**不在装箱路径上** —— 未装箱的同一形状早就答错:

```
let xs = [1.5, 2.5];
xs.push(9);
println(typeof(xs[2]));   // 改前 VM: Int,native: Float
```

完整原生编译,两边都不报错。检查器按数值提升放行了这次 push(`Int` 可以给
`Float`),也就是承诺了元素类型;VM 转头把 `TypedList::Float` 拓宽成 `Mixed` 并把
`9` 原样存成 `Int`。原生存 `9.0`,与承诺一致。

**问题在 VM 侧**:它推翻了自己刚做出的接受。`Float` 载体收到 `Int` 现在存
`value as f64`。反向不对称,保持原样:`Float` 进 `Int` 列表是收窄,检查器会拒,
所以只能从被擦除的类型到达,那里拓宽就是动态语义。

`xs.push("a")` 这类**真正**的拓宽(类型被 `List` / `Any` 擦掉)当时两边还不同:VM
拓宽成 Mixed,原生 raise。原生的 `Vec<i64>` 没法就地变成 `Vec<LkDyn>` —— 别的别名
按静态类型直接读这块内存。未装箱路径在那里回落,装箱路径 raise;两者都不是错答案。
**已裁决并实现,见下文"载体在构造点决定"**。

**2026-08-05 更正**:上一段"别的别名按静态类型读这块内存"这条判据,在容器改成不变
之后不再成立 —— 不变意味着同一个值在每个名字上的元素类型相同,拓宽因此对所有名字
同时可见。剩下的问题不是别名,而是**载体在构造时就定了**:`let xs: List<Any> = [1,2];`
的载体按内容推成 Int,后面 `xs.push("a")` 时 VM 就地拓宽,而原生没有这一步。可判定的
形状是"程序里有一次放入更宽元素",那时就该从构造起用 Dyn 载体。实测(2026-08-05)
这条形状在原生侧是干净回落,不是错答案:

    let xs: List<Any> = [1, 2];
    xs.push("a");
    → MIR lowering: an operand at pc 3 is a str where a i64 is required

## 结构体字面量命名的是一个**已声明**的类型(2026-08-06 裁决)

    let p = Nope { a: 1 };
    改前 → Nope{a:1}          改后 → 类型错误

检查器那句注释就是当时的全部规矩:"If struct is known, enforce field presence and
types; **otherwise, accept as named type**"。于是拼错一个类型名不报错,答一个值。

这条同时是另一个形状的一半:从别的模块导入的类型,写成裸 `P { x: 4 }` 时构造出来的是
**残缺的** P ——

| 构造写法 | typeof | impl 方法 |
| --- | --- | --- |
| 裸 `P { x: 4 }`(只有 `use "geo"`) | `P` | **没有** |
| 裸 `P { x: 4 }`(有 `use { P } from "geo"`) | `P` | 有 |
| `geo.P { x: 4 }` | `P` | 有 |
| 模块导出的构造函数 | `P` | 有 |

第二行是 2026-08-06 补上的;第一行现在是**拒绝**,不再是那个残缺的值。
`use { P as Q } from "geo"` 也算第二行:别名换的是这个文件里的写法,不是类型 ——
`Q { x: 4 }` 按 `P` 的 schema 校验,报错说 `struct 'P'`,`typeof` 答 `P`。

根因是 `NewObject` 盖的类型出身是**当前执行模块**的 `TypeScope`,而方法表按定义方的
scope 建 —— 两个身份,名字碰巧一样。用户在调用点看到 "P has no method 'norm'",离构造
处很远。

两半现在都拒了:名字在本模块没有声明是一种错,名字**只**因为别的模块声明而已知是另一种,
消息各说各的原因。注册表因此记来源(`register_struct` 是本模块声明的,
`register_imported_struct` 是 `seed_imports` 带进来的);本地声明会把标记清掉,所以
本模块声明一个同名类型照旧盖过导入的那个。

`m.P { … }` 与模块导出的构造函数两条路不受影响 —— 它们本来就带着定义方的身份。

**另一半已经补上(2026-08-06)**:`use { P } from "geo"` 现在绑定的是定义方生成的
`P$new`,所以裸 `P { x: 4 }` 与 `P(x: 4)` 都成立,与 `geo.P { x: 4 }` 同一个身份。
改写放在编译器侧:本地 `struct S` 一定带来本地 `S$new`,所以"没有本地 `P$new`、
却有一个叫 `P` 的全局"恰好是导入这一种情形,降到对那个全局的具名调用,别的情形
照旧发 `NewObject` —— 本地构造这条热路径一条指令没动,不必押在内联器上。上面那张
表里"裸 `P { x: 4 }`"因此有了第四种情形:按名导入时有方法,只通过命名空间看见时
仍然是拒绝(不是残缺的值)。细节见 §导入的类型可以构造。

## 时长不能是负数,四个入口一条规矩(2026-08-05 裁决)

    use time;
    println("before");
    time.sleep(-1);        // 打印 before 之后再不返回

`Duration::from_millis(duration_ms as u64)`,`-1 as u64` 是 u64::MAX 毫秒 ≈ 5.8 亿年。

同一个操作四个答案:

| 写法 | `-1` | `-0.5` |
| --- | --- | --- |
| `time.sleep` | **挂起** | 立即返回 |
| `task.sleep` | 报错 | 立即返回 |
| `time.timeout` / `time.after` | 定时器永不触发,静默 | 同上 |

`-0.5` 那一列是关键:`task.sleep` **有**一个 `< 0` 守卫,但它跑在 `as i64` 之后 ——
截断已经把值变成 `0` 了。所以判负必须在截断**之前**。

现在:`lk_stdlib_common::duration_millis` 一处实现,四个入口共用,消息一句
(`{name}() expects a non-negative duration in milliseconds, got {ms}`),lkrt 侧
`lkrt_time_sleep_ms` 的措辞对齐 —— 被捕获的错误消息就是程序的输出,两端必须逐字一样。

**时刻不是时长。** `time.since(start, end)` 取的是钟面上的两个点,差值才有方向,所以
它留用 `numeric_millis`(任意符号),与 `duration_millis` 是两个概念。

门禁:`a_duration_cannot_be_negative`。

## 模块拼写需要 import,两端都要(2026-08-05 裁决)

    let c = chan.new(1);          // 没写 use chan;
    vm   → Error: index target object is not indexable: "Function"
    原生 → 7

`chan` 是唯一一个既是模块名、又是裸全局(通道构造函数)的名字。`math` / `string` /
`time` 这些不是全局,检查期就报 "undefined name",两端一致;`chan` 会走到运行时。

两端跑的是**同一份字节码**,加不加 `use chan;` 一个字都不差(`GetGlobal chan` /
`LoadString "new"` / `GetIndex`)。差别在运行时:import 把全局 `chan` 从构造函数换成模块
对象,VM 的 `GetIndex` 这才成立。

原生侧原来无条件把这个形状解析成模块成员,注释里写着"字节码能区分二者"—— 那句对 VM
的行为判错了。现在它读 `sig.imports`:**import 才是模块拼写的许可**。

根因是 `ImportEnv::build` 里 `ImportStmt::Module { .. } => {}` 一个空分支 —— `use math;`
这种最常见的写法在原生侧被整个丢掉,所以降低器分不清"导入的模块"和"碰巧同名的全局"。

不动的一格:`chan(1)`(裸构造函数)无 import 可用,`use chan;` 之后 `chan(1)` 报
"`chan` names the imported module here, and a module is not a function" —— 两端都对。

门禁:`differential_concurrency_edges` 的 `the module spelling after its import`。

## `format` 与 `println` 是同一个操作(2026-08-05 记录)

占位符是 `{}`,按顺序取实参;**多出来的占位符原样留下,多出来的实参空格分隔追加在
后面**:

```
"{}-{}".format("a", 1)   → a-1
"{}-{}".format("a")      → a-{}
"{}".format("a", "b")    → a b
"plain".format("a")      → plain a
println("{} x", 1)       → 1 x
println("a", 1, true)    → a 1 true
```

这条规矩两边一致,而且"多余实参追加"正是 `println(a, b, c)` 这个主拼写赖以工作的
东西 —— 它不是漏检。

代价是写惯 `%s`(C)或 `{0}`(Python)的人拿到的是一串拼接结果而不是报错:
`"%s-%d".format("a", 1)` 答 `%s-%d a 1`。**这不是缺陷**,是同一条规矩作用在一个没有
占位符的模板上。编号占位 `{0}` / 具名占位 `{name}` 不支持,是特性缺口,不是错答案。

不改成"个数不匹配就报错"的原因:那会同时否掉 `println("a", 1)`(没有占位符、两个
实参)和 `println("{} {}", 1)`(占位符多于实参),而这两条都是被明确写下来的行为;
核心检查器又没有 warning 通道(见"catch-all 后面的臂"那条),只能响亮拒绝或者不说话。

## 每一种赋值都按目标声明的类型校验(2026-08-05 裁决)

除了普通 `name = value`,**没有任何赋值目标被检查过**:

```
let l: List<Int> = [1];
l[0] = "a";
let n: Int = l[0];
println(n + 1);          // 打印 a1
```

同一形状在四个目标上都成立,而方法拼写是拒的:`l.set(0, "a")` 报
"Argument 2 has the wrong type (expected Int)"。`l.set(i, v)` 和 `l[i] = v` 是同一个
操作的两种拼写,只有一种被检查;结构体字段那格最重 —— 字段是这门语言里声明得最多的
东西,而 `s.x = "a"` 之后 `let n: Int = s.x` 照样过。

根在解析期的降解:三种写法各自变成一个不同的隐藏调用,谁也没有校验值。

| 写法 | 降解成 |
| --- | --- |
| `l[0] = v`(下标是整数字面量) | `list.set(l, 0, v)` —— 字节码编译器认的类型化列表快路 |
| `l[i] = v` / `m[k] = v` | `__lk_set_index(c, k, v)` |
| `s.f = v` / `m.f = v` | `__lk_set_field(c, "f", v)` |

现在:三处都调同一个 `check_container_store` —— 三种降解,一条规则。它按容器类型取出
声明的元素/值类型(结构体则取字段声明的类型)与被存的值比;map 还比键的声明类型
(`Map<String,Int>` 上的 `m[7] = 2` 此前收下,map 里就出现了 Int 键)。

载体是四个,不是三个:异质字面量推成 `Tuple`,它的每个位置类型不同,所以按位置校验
—— 下标是整数字面量就比那一位;下标不是字面量时位置未知,值必须适配**每一位**(读
`l[0]` 是按第 0 位定的类型,动态写落在那里不能把它推翻)。`let l = [1, "a"];
l[0] = 2.5;` 此前收下,`let n: Int = l[0]` 随后拿到 2.5。

两条边界照旧:元素类型还是类型变量时**教**它而不是拒它(`let l = []; l.push(1);
l[0] = "a";` 仍然可以,与实参那条同规矩);`Any` 两侧都放行,那是这门语言的动态逃生口。

门禁:`a_store_is_checked_against_what_the_container_declares`,十种拒 + 六种收。

这条修完,"元素类型什么时候是承诺"这张表就一致了:

| 写法 | 后续放入不同类型 |
| --- | --- |
| `let l = []` / `let m = {}` | 收 —— 元素类型还是变量,存进去是教它 |
| `let l = [1]` / `let m = {"k": 1}` | 拒 —— 字面量说了它装什么 |
| `let l = [1, "a"]` | 按位置拒 —— `Tuple`,每位各有类型 |
| `let l: List<Int> = …` | 拒 |
| `let l: List<Any> = …` | 收 |

此前只有 `let m = {"k": 1}` 那格"收",而它收的原因不是拓宽,是压根没检查。

## 可变容器不再是协变的(2026-08-05 记录、裁决、实现)

`values/src/types.rs` 的 `is_assignable_to` 里写着 `// Generic containers with
covariant element types`,是有意为之。实测它可以推翻类型系统自己的保证:

```
fn add_any(xs: List, v: Any) { xs.push(v); }
let a: List<Int> = [1, 2];
add_any(a, "s");
let b: Int = a[2];   // 过检查
println(b);          // 打印 s
```

`lk check` 全过。四种拼写都通:形参写 `List` 或 `List<Any>` 都接受 `List<Int>`;
`Map<String, Int>` 传给 `Map` 后写入,`let b: Int = m["k"]` 同样过检查并拿到字符串。

**洞在每一个拓宽位置,不止调用参数**(2026-08-05 实测,五种写法都答 `s`):

```
let b: List = a;                       // 裸别名
let b: List<Any> = a;                  // Any 别名
let bx = Box { xs: a };                // struct 字段,字段声明 List
let holder: List<List> = [a];          // 容器元素
fn widen(xs: List<Int>) -> List { return xs; }   // 返回类型拓宽
```

**它也是"拓宽"那条的根**:VM 之所以要把 `TypedList` 拓宽成 `Mixed`,正是因为检查器
放进来了它本不该放的元素。

### 为什么还没改

四条路都量过或试过:

| 路 | 代价 |
| --- | --- |
| 元素类型完全不变(Rust 的选择) | 这门语言**没有泛型函数**(`fn first<T>(...)` 语法错误),所以写不出"对任意元素类型的列表"的签名。stdlib 里 15 个 `params(values: List)` 会全部拒绝类型化列表。 |
| 裸 `List`/`Map`/`Set` 改成只读视图,带参数的不变 | LK 代码里裸拼写用了 **0 处**,爆炸半径只在 stdlib 签名。但要求 `List<?>` 与 `List<Any>` 是两个类型,`Type::List(Box<Type>)` 装不下,得加变体并改所有匹配点。 |
| ~~形参可变性推断驱动的按参数型变~~ | **不成立**:它只封住调用参数一处,上面另外四种拼写照旧。 |
| 运行时存储检查(Java 数组的做法) | 需要列表记住**声明的**元素类型;今天的载体是从内容推出来的,`let xs: List<Any> = [1,2]` 的载体是 Int,照这个检查会误拒。 |

没有一条是小改动,而选错会把不健全换成另一种不健全。

### 裁决:容器不变,加一个"元素类型不详"的元素类型

先把第二条路的代价量出来,而不是估。把 `is_assignable_to` 的三条容器规则临时改成
逐元素相等,跑 `lk-core` 的 1221 个测试和 64 个示例:

| 量 | 结果 |
| --- | --- |
| lk-core 测试 | 1215 过 / 6 失败 |
| 示例语料 | 56 过 / 8 失败 |
| 失败的类别 | 两类,没有第三类 |

两类是:

1. **stdlib / builtin 签名里的裸 `List`**(报 "expected List<Any>, got List<Int>")。
   8 个失败示例里没有一处是用户写的 `List<Any>` 注解 —— 全部来自签名。
2. **空字面量的推断**:`let xs: List<Int> = [];` 里 `[]` 是 `List<'T0>`。这是合一,
   不是型变,逐元素相等把它一起挡了。

于是规则是:

- 容器**不变**(`List<Int>` 不可赋给 `List<Float>` / `List<Any>` / `List<Int?>`)。
- 元素里出现类型变量时照旧合一 —— 那是推断,不是型变。
- 新增元素类型 `_`(`Type::Unknown`),在类型参数位置书写:`List<_>`、`Map<String, _>`。
  它的含义是"元素类型不详":**没有任何类型可以赋给它**,所以以它为参数的容器可读
  不可写;而任何 `List<X>` 都可赋给 `List<_>`。只读性由类型本身推出,不是额外规矩。
  `_` 在模式里已经就是"不具名的任意",不引入新概念。
- 裸 `List` 仍然是 `List<Any>`(可写、不变),所以 `returns = List` 一处不动。
- 签名侧改的是**参数**:stdlib 24 个容器参数 + `builtin_method_sig.rs` 5 个,全部
  改成 `List<_>` 之类。这 29 个都只读 —— 唯一可能就地改的 `random.shuffle` 实测是
  拷出来再分配新列表。门禁:stdlib 的容器参数不许是可写容器。

`_` 只可赋给 `Any`,所以 `let n: Int = xs[0]`(`xs: List<_>`)是类型错误,要经过显式
的 `Any` 一跳 —— 与这门语言已有的动态逃生口一致。

### 字面量是新值,所以按协变判

不变性会把 `let xs: List<Any> = [1, 2];` 也挡掉,而那里没有不健全:一个容器**字面量**
没有第二个名字,所以没有东西会因为它被拓宽而看错元素类型。规则因此是
`Type::container_literal_fits`,只在实参表达式确实是容器字面量时走协变,与整数字面量
那条(`let x: u8 = 5` 而不是 `5 as u8`)同形同因,用在同样三个位置:带注解的 `let`、
位置实参、命名实参。经过变量就没有这条路 —— `let a: List<Int> = [1,2]; let b: List<Any> = a;`
仍然拒绝。

`Any` 作为**来源**元素类型照旧流动(`List<Any>` 可赋给 `List<Int>`),那是这门语言的
动态逃生口;被封的是 `Any` 作为**目标**元素类型,也就是拓宽的那个方向。

门禁:`a_container_cannot_be_widened_at_its_element_type` 逐个探七种拓宽写法,并正向
验字面量、`List<_>` 只读视图两侧。

**别名还是拷贝,这个问题必须先答。** 如果拓宽产生的是拷贝,这条就自动消失(写进宽
别名不影响窄的那个)。但 LK 的容器是引用值 —— 被调方 `xs.push(x)` 对调用方可见正是
#170 认定"装箱重建是缺陷"的依据 —— 所以拓宽是别名,只能从类型上封。

**别名还是拷贝,这个问题必须先答。** 如果拓宽产生的是拷贝,这条就自动消失(写进宽
别名不影响窄的那个)。但 LK 的容器是引用值 —— 被调方 `xs.push(x)` 对调用方可见正是
#170 认定"装箱重建是缺陷"的依据 —— 所以拓宽是别名,只能从类型上封。

## 纯序列操作在每个序列载体上都可用(2026-08-05 裁决)

四个序列载体 —— `List` / `Str` / `Bytes` / `Slice`(窗口)。从
`builtin_method_sig.rs` 的声明表算差集,`Bytes` 和窗口已经有 `len`、`is_empty`、
`first`、`last`、`get`、`contains`、`index_of`、`take`、`skip`、`slice`、`min`、
`max`、`sum`、`map`、`filter`、`reduce` —— 列表读取面的每一个,唯独少两个:

| | List | Str | Bytes | Slice |
| --- | --- | --- | --- | --- |
| `reverse` | 有 | 有 | **无** | **无** |
| `count` | **无** | 有 | **无** | **无** |

`count` 那一行的后果具体是:`"aa".count("a")` 答 2,而 `[1, 1].count(1)` 报
"List has no method 'count'"。

**规则:结果能用同一载体表示时答同载体,否则答 `List`。** `b.reverse()` 是
`Bytes`;`w.reverse()` 是 `List`,因为反转后的那段不是源列表的一个区间 —— 与
`w.map(..)` 已经在做的事同规矩,也与 `w.take(1)` 答窗口不矛盾(子区间还是区间)。
`bytes_dispatch` 的文档注释本来就写着判据("含义不依赖元素类型的操作"),这两个
正属于这一类,是漏了不是排除。

`index_of` 与 `count` 现在从**同一个扫描函数**出(`typed_list_scan`),因为规则
才是内容:`Int` 元素等于 `Float` 针值(`1.0 == 1`),`Float` 列表按值比所以
`0.0` 找得到 `-0.0`,`Mixed` 交给 `runtime_values_equal`。分开写就是同一个操作的
两种拼写将来会各自漂移。

`Bytes` 里放不下的针值(`300`、`-1`)`count` 答 0,与 `contains` 给它的答案一致。

`sort` / `unique` 按同一规则:`Bytes` 上答 `Bytes`(字节是有序标量,每个元素仍是
字节),窗口上答 `List`(两种答案都不是源的一个区间)。窗口的这两个路由到
`typed_list_sorted` / `typed_list_unique`,不重写一遍 —— "排序的序"和"后来的重复
被丢掉、顺序保留"是规则,规则抄第二份就是将来漂移。

`enumerate` / `zip` / `chain` / `chunk` 的答案是**元素的列表**,与载体无关,所以
它们是 `List` 的:两个载体各加**一条**委派臂,把元素物化一次再走 `List` 的实现。
六个方法各写两份就是把 `enumerate` 的配对、`chunk` 的分组各抄两遍。降低侧同形 ——
接收者是 `Bytes` 或窗口且方法在这一组时,先物化成 `ListI64` 再让已有的 List 臂跑。

`flatten` **不**在这一组:`Bytes` 和 `i64` 窗口装的是标量,展平是空操作,检查器
拒得对。

`join` 是第三种情况:字节码编译器按名字把它匹配成融合的 `ListJoin`,所以没有叫
`join` 的方法调用能到降低的方法分发。VM 侧和降低侧的载体臂都得加在那个 opcode 上,
而且 VM 侧把五个渲染分支抽成了 `join_typed_list` —— 列表、窗口、`Bytes` 三个接收者
共用它。

`concat` 收尾,而且要分载体:窗口上它是 `chain` 的另一个名字,答 `List`;`Bytes`
上两个字节串接起来还是字节串,保形,有自己的臂。把它一并委派会让一个语言已经
能用 `Bytes` 表示的形状答成 `List` —— 判据因此写成"按载体",不是一张名字表。

四个序列载体的矩阵到此对齐:`Str` 不在这批里是因为它的"元素"是字符,`sum` /
`min` / `max` / `chain` 在字符上没有意义,拼接用 `+`,要列表用 `chars()`。

## 一个 `Set` 得能做集合的事(2026-08-05 裁决)

`Set` 的全部方法是 7 个:`add` / `clear` / `contains` / `delete` / `is_empty` /
`len` / `values`。`a.union(b)` 报 "Set has no method 'union'",`intersection` /
`difference` / `is_subset` 同。只能加、删、判成员、取列表的 `Set` 是去重的袋子。
这门语言把它做进了内建类型、给了构造、显示、迭代和 `in`,唯独没有让它做集合运算。

补齐(Rust 的命名):`union` / `intersection` / `difference` /
`symmetric_difference` 答 `Set`,`is_subset` / `is_superset` / `is_disjoint`
答 `Bool`。

**插入序是契约。** 集合的迭代序就是它的哈希序(见 `DYN_SET` 与镜像纪律),所以
成员相同的两个集合仍可能迭代出不同的顺序 —— 换句话说,"用另一种方式构造同一个
答案"会通过任何成员测试,然后打印得不一样。所以每个运算都按**一个写定的顺序**填
结果:接收者自己的顺序在先,实参的在后。原生侧逐字复现同一序列,差分用例把它钉住。

`is_disjoint` 不是 `intersection().is_empty()` 的展开:它在第一个共同成员处停下,
一次分配都没有。

**同时记一条不改的:** 成员判定在 `List` / `Str` / `Bytes` / `Slice` / `Set` 上
叫 `contains`,在 `Map` 上叫 `has`。这不是"一个操作两个名字" —— `Map` 的
`contains` 语义上真有歧义(键还是值),Rust 用 `contains_key` 正是为避开它;序列
和集合的 `contains` 无歧义。分野保留。

## `-` 从容器里移除 —— 教程写着,VM 实现着,检查器不收(2026-08-05 裁决)

`LEARN.md` 的表达式表里有一行:

```
[1, 2, 3] - [2]     // [1, 3]
```

那一行**从来没被任何东西验过**。逐条跑那张表时它报 "the left operand must be
numeric types" —— 而擦掉类型就答 `[1,3]`,map 也一样(`m - n` 去掉 `n` 的每个键)。
与 `Map + Map` 完全同形,而修那条时我只补了 `+`,没看旁边的 `-`。

`lkrt_dyn_sub` **也从来没实现过移除**,它的报错却写着 "expected numbers or
list/map lhs" —— 借了 VM 的判据来描述自己没有的能力。这条路径此前不可达,所以
没人发现。

三处补齐:检查器的 `Sub` 分出 list / map 两臂(结果保持左侧的元素类型 —— 移除只
拿走不加入,不会变宽)、lkrt 实现移除、降低侧接上。答案保持**左侧自己的顺序**,
理由同上。

## `Map + Map` 合并 —— 两个执行器一直都实现了,只有检查器不收(2026-08-05 裁决)

```
let a = {"a": 1};
let b = {"b": 2};
println(a + b);   // 改前:Type Error: the left operand must be numeric types
```

而把类型擦掉就跑得动:

```
let a: Any = {"a": 1};
let b: Any = {"b": 2};
println(a + b);   // {"a":1,"b":2}
```

VM 的 `Add` 有 map 分支(`merge_typed_maps`),`lkrt_dyn_add` 也有,注释里写着
"两个 map 合并,右侧胜"。**只有检查器不收**,而且只在它知道类型的时候不收。
这正是 `lk check 答的必须是执行器答的那个问题` 的反面 —— 检查器多出一条两个执行器
都没有的规则,和它漏掉一条一样是缺陷。

结果类型按 `check_list_addition` 已有的判据:哪一侧被另一侧包含就取哪一侧,都不
包含取 `Any`;键和值各判一次。判据抽成 `wider_of`,两处共用。

**没有一并放开的:** `Set + Set` 和 `Bytes + Bytes` 在运行时确实报错
(`Add expected numbers or strings, got Set and Set`),所以检查器拒它们是对的。
集合的并集现在是 `a.union(b)`,字节串拼接是 `b.concat(c)`。

### 原生这一侧:先修填充序列,再接上降低

合并建的是一张**新表**,而新表的迭代序由它被填充的顺序决定 —— 所以"同样的成员"
不等于同样的答案。VM 的序列是:左侧的条目按左侧自己的顺序、跳过右侧也有的键,然后
右侧的条目按右侧自己的顺序(`merge_typed_maps` + `typed_map_without_merge_keys`)。

lkrt 那份把**两个无序视图合成第三个**,也就是三种不同的顺序;它还把键一律
`key_str().to_string()`,整数键合并会变成字符串键。两条都不可达(降低侧没有 map
臂),所以一直没人发现,而那个函数的注释还写着"这是一张新表,没有在声称保留它从未
有过的顺序"—— 顺序确实是新的,但它是一个中间哈希表的顺序,不是任何一侧的。

现在:`map_entries_ordered` 按载体自己的顺序读,`str_dyn_from_ordered` 按给定序列
填,非字符串键**报错**而不是转成字符串。降低侧只接字符串键的 map(装箱的 map 载体
就是字符串键的),整数键合并继续回落 —— 与其答 `{"3": 1}` 而 VM 答 `{3: 1}`。

## 报错里的载体表是内容,不是装饰(2026-08-05 裁决)

`[1][5].len()` 答 "`len()` works on a String, List, Map or Set" —— 而同一个函数
的堆分支收 `Bytes` 和窗口,两者都是 `len` 好好答得出的。`for x in 1 {}` 答
"For loop iterable must be List, String, Map, or Set",而检查器那个 `match` 实际
放行九种(还有 `Any`、类型变量、`Slice`、`Bytes`、`Tuple`)。

这类消息**就是规则本身**:读的人拿它当"这个操作能用在什么上"的说明,而它说错的
方向恰好是让一个能跑的程序看起来不可能。载体是后来加的,消息停在了加之前。

四处一起改正:`len` / `Slice target` / `SliceFrom target` / `ToIter target`,加上
检查器那条 for 循环的。

**守卫**:`a_carrier_list_in_an_error_message_matches_what_the_operation_accepts`
逐个载体跑一遍,再拿被拒绝的那个接收者取出消息,要求**恰好**互相对应 —— 收下的
载体必须在消息里,消息里的必须收得下。两个方向都反向验过(把 `Bytes` 从消息里
去掉、把 `Bytes` 从 `match` 臂里去掉,各自变红)。

**这条守卫上一轮漏掉了 `slice`,而我恰好在那里把消息改错了。** 当时的判断是"任何
会被拒绝的接收者都先被方法分发答掉,所以消息不可达",而那只对 `c.slice(a, b)` 这个
**方法**拼写成立;`c[a..b]` 这个**范围索引**拼写走的是另一条路,到得了 opcode。我按
"应该支持"把消息扩成 `string/list/bytes/slice`,而那两个臂当时并不存在 —— 正是这条
裁决要修的毛病,方向相反。

现在两件事一起做对:

- `c[a..b]` 在 `Bytes` 上答 `Bytes`、在窗口上答子窗口,与 `c.slice(a, b)` 一致。
  之前检查器答 "Bytes index must be integer",而同一个操作的方法拼写好好的。
- 守卫加上 `c[0..1]` 这一行,消息与臂重新绑住,反向验过(去掉 `Bytes` 臂变红)。

降低侧顺带补齐:范围索引原先只降低 `Str` 和 `List<Int>`,`List<Float>` /
`List<str>` / 混合列表 / `Bytes` 全部回落 —— 符号早就都在,那张表只列了两行。窗口
仍回落(没有取子窗口的符号),是回落不是错答案。

## 维护约定

- 新增可下降形状时,先在此登记预期语义(尤其失败路径与显示格式),再写差分用例。
- 当 VM 与 native 出现分歧:先查本表;表内未覆盖的,裁决后**新增条目 + 差分用例**,
  不允许只改一侧实现使测试变绿。
- 退出机制(exit 1 vs SIGABRT)如未来需要统一,属于语言决策,需同时改本表、
  差分 harness 的宽容逻辑(`success()` 对比)与 CLI 文档。

## 标量没有内建方法,而声明的可见性不看位置(2026-08-06 裁决)

    let v = 1;
    println(v.nope());
    改前 → lk check 过,运行时报 "Int has no method 'nope'"
    改后 → 类型错误(带位置)

Int / Float / Bool / Nil 的内建方法集是**空的** —— `abs` / `sqrt` / `round` /
`len` / `to_string`,逐个探过,全是 "no method"。所以一个标量接收者上的名字只
可能由用户的 `impl Int { … }` 或 `impl Trait for Float` 解析,而那条路在检查器
里先试。走到"没有签名"这一步就是真的没有。

之前放过它的原因:`BuiltinReceiverKind` 只有 List / Bytes / Slice / Map / Set /
Str 六个,`receiver_kind` 对标量答 None,那条"已知接收者上没有这个方法就是错"
的规矩够不到;后面的容器兜底对标量也一律不答,最后落到 `Any`。同一个错误因此
在 `String` 上是检查期错误、在 `Int` 上要等到运行时。

**Map 仍然豁免**,而且是对的:map 的条目就是它的字段,`m.score(1)` 可以是一次
普通的属性调用,而 map 的类型里没有它有哪些键这件事。

### `impl` 也被提升了

查这条时发现的另一半:`fn` 和 `struct` 都被预扫提升(声明写在使用之后照样能用),
唯独 `impl` 是按语句顺序读的 —— 一个方法是靠"被类型检查"才为检查器所知的。于是

```lk
struct P { x: Int }
let p = P { x: 1 };
println(p.m());                                  // 改前:P has no method 'm'
impl P { fn m(self) -> Int { return self.x; } }
```

把 `impl` 往上挪两行就过。而**导入**的 `impl` 早就有预扫(`typ::imports` 的
`seed_impl_methods`),所以一个 `use` 之外的 impl 能用、三行之下的不能用 ——
这个不对称也正是它一直没被发现的原因。

现在三种声明同一条规矩:签名从**声明**读(未标注的参数是 `Any`),预扫只让方法
更早**可见**,不会收紧任何东西;有序遍历走到定义处时再用推断出的签名覆盖。元数
之类的校验因此从上方也照样生效。

## 顶层调用不能读到还没初始化的顶层绑定(2026-08-06 裁决)

    fn f() -> Int { return LATER; }
    println(f());
    const LATER = 7;
    改前 → nil            改后 → 类型错误

`typeof(f())` 答 `Nil`,而 `f` 声明的返回类型是 `Int`。碰到 nil 之后的操作报的
是它自己的事("Add expected numbers, got Nil and Int"、"`len()` works on a
String, List, …"),从不指向顺序。对照:Python 抛 `NameError`,JavaScript 从
TDZ 抛 `ReferenceError`,Lua 给 nil。这门语言有检查器,给 nil 是三者里最差的。

三种情形现在齐了:

| 写法 | 判定 |
| --- | --- |
| `const B = A + 4;` 在 `const A = 1;` 之上 | 早就拒(直接读) |
| 函数体读后面声明的绑定,但不在上面调用 | **允许** —— 函数体在整个顶层之后才跑,裸机程序到处这么写 |
| 顶层语句**调用**一个(传递地)读到未初始化绑定的函数 | 现在拒 |

分析在 `core/src/stmt/init_order.rs`,**刻意单向**:每个近似都只会漏报,不会
误报。遮蔽是整体相减(函数里任何位置绑定过这个名字,就不算读全局的那个);
间接调用(通过函数值)看不见;闭包和嵌套 `fn` 的体不算"此刻执行";环上的
回边不贡献。唯一会误报的是"读在一条永不执行的分支上",而把声明上移一行永远
可行也永远正确。

两个遍历都对 AST 枚举**穷尽匹配**、没有兜底臂:新增一个 `Stmt` 或 `Expr` 变体
会在这里编译失败,而不是静默掉出分析。传递闭包用显式工作表而不是递归,理由与
`HeapStore::collect` 相同 —— 深度是程序的调用深度,生成出来的文件可以要多深有
多深,放在 Rust 栈上就变成一次没有行号可指的进程 abort。

实测:4000 个函数 `lk check` 0.26s,3000 层调用链 0.12s,5 万层不崩。

## 嵌套深度只有一个预算,两个解析器共用(2026-08-06 裁决)

    fn main() -> Int { if true { if true { … 400 层 … } } }
    改前 → fatal runtime error: stack overflow(2MiB 线程上 abort)
    改后 → Syntax error: Expression nesting too deep

`if c { … }` 在 LK 里既是语句也是表达式,解析时在两个解析器之间来回穿:语句
解析器遇到 `if` 交给表达式解析器,表达式解析器解析块体时又建一个语句解析器。
两侧各有一个深度计数器,而**每次穿越都新建一个子解析器并从零开始计**,于是两
个计数器谁也不累积 —— 400 层嵌套时实测语句深度最大值是 1。

裁决:一个预算,两侧共用。`ast::parser::MAX_PARSE_DEPTH`(std 64,no_std 16)
是唯一的常量,两个解析器都往同一个预算里计数,且每一次穿越都把当前深度传给子
解析器。表达式解析器内部原本就有这条规矩(`sub_parser` 的注释写着"嵌套解析
即使拿到自己的 `Parser` 也仍然是嵌套"),只是没有跨到语句边界上。

两条拒绝消息都可达,对应两种形状:

| 形状 | 每层开销 | 报出的消息 |
| --- | --- | --- |
| `if` / `while` / `for` / `try` / `match` | 略多于 2 帧 | `Expression nesting too deep` |
| 裸块 `{ … }`、嵌套 `fn` | 1 帧 | `nesting too deep (more than 64 levels)` |

实测边界:`if` 嵌套 30 层收、31 层拒;裸块 63 层收、64 层拒。本仓库 `.lk` 语料
里最深的花括号嵌套(算上 `fn`/`impl`/`struct`)是 6 层。

### 附带裁到的一条:投机解析不能吞掉预算错误

`try_parse_tail_expression_stmt` 先拿表达式解析器试一次,失败就当"不是这个形
状",回落到语句路径重新解析同一批 token。这条重试是承重的 ——
`try { … } catch e { }` 的空 catch 体表达式解析器拒、语句解析器收。

但语句路径内部会在下一层再做同样的"先表达式后语句",所以只要表达式那次失败,
每层的工作量就翻倍:`T(k) = 2·T(k+1)`。此前表达式那次总是成功,指数一直没露
面;深度预算开始生效后它立刻露了 —— 32 层嵌套 `if` 跑五分钟不结束,而同一个
文件在表达式那次成功时是 0.1s。任何深层语法错误都能触发,与深度守卫无关。

裁决:语法错误是形状信息,可以吞;预算耗尽不是 —— 每个候选都会栽在它上面。
`ast::parser::NestingTooDeep` 是一个可 downcast 的标记类型,投机解析只放行前
者,后者直接向上传。

## 拓宽一个列表的载体,只有构造点能做(2026-08-06 裁决)

    fn widen(xs: Any) -> Int { xs.push("z"); return xs.len(); }
    let a = [1, 2]; let b = [1.5, 2.5];
    println(widen(a)); println(widen(b)); println(a); println(b);
    改前 → 原生**完整编译**,运行期 `Error: runtime type error`;VM 答对
    改后 → 两端都是 3 / 3 / [1,2,"z"] / [1.5,2.5,"z"]

VM 把 `TypedList::Int` 就地拓宽成 `Mixed`。原生做不到:被调方拿到的是原地打标签的
`DYN_TLIST_*`,标签背后是调用方的 `Vec<i64>`,而调用方的其它别名按静态类型直接读那块
内存。改标签只改了这一份 `LkDyn` 拷贝,改不了别人的。

裁决:载体在**构造点**决定。一个会被拓宽的列表,从字面量起就用 Dyn 载体 —— 这与
`#183`(同函数内的 push 推翻载体)和 `#196`(map 字面量)是同一条规矩,只是跨了函数
边界,因此共用同一条 `LiteralElemTypeContradicted` 重试通路。

需要判定"这个形参会被拓宽",有两种发现方式,少一种就漏一半:

| 形状 | 形参类型 | 谁发现 |
| --- | --- | --- |
| 两个调用点载体不一致 | 并成 `Ty::Dyn` | **调用方**:被调方走 `dyn.list_push`,拓宽与否要到运行期才知道,所以调用方一律悲观 |
| 单个调用点 | 保持 `ListI64` 等 | **被调方**:类型化的 push 臂当场发现元素放不进载体,报 `ParamCarrierContradicted` |

第二条需要一趟额外的往返,原因是 fixpoint 的结构:形参观察在第二趟开头被清一次
(那是为了不让第一趟的临时类型污染单调的 join),而被调方按函数下标先于调用方降低,
所以在整个 fixpoint 里它看到的都是未观察时的 `I64` 默认值,**只有最终趟**才第一次看见
真实载体。最终趟的可重试失败原本只有 `DynLoopPhi` 一种被收回去重跑,现在这条也收。

同一条规矩对 map 也成立,两种形状都覆盖。装箱接收者上的下标写入走 `dyn.index_set` ——
键也装箱传过去,因为"哪种键形状能用"是载体的规矩,由被调方判定:整数键在 map 上是**键**
不是位置(与 `lkrt_dyn_index` 读的那条裁决同一条),在列表上是位置,负数从末尾数,越界是
VM 的那条 halt(`list index N out of bounds`)。写入落到标签背后的载体本身,所以装箱和原件
仍是同一个容器 —— 与 `dyn.list_push` 同一条纪律,而 `dyn.as_list` / `dyn.as_map` 是只读的,
从它们写会写进物化出来的拷贝。

载体放不下的值在这里 raise,不拓宽:那块内存是构造方的,它的别名按静态类型读同一块。

代价是拿类型化换正确性:流进这类形参的容器失去类型化载体。实测 AOT 覆盖仍是 60/60,
VM/原生扫描 identical=61,perf 1.034x(噪声带内)—— 这条形状不在任何基准的热路径上。

## `defer` 在 return 上跑,在 raise 上不跑(2026-07-30 裁决,2026-08-06 记录)

    fn a() -> Int { defer { println("a"); } error("boom"); return 0; }
    fn b() -> Int { defer { println("b"); } return a(); }
    println(try { b() } catch e { -1 });
    → -1(两端一致,"a" 与 "b" 都不打印)

`defer` 是一次 AST 改写,不是运行期机制:它把释放语句复制到每个 `return` 之前,
在解析器之后、类型检查器与两个编译器之前完成,下游看不到 `Stmt::Defer`。
raise 在原生侧是 `longjmp` 离开,不是 `return`,这次改写看不见它。

这条不是遗漏。关掉它的做法(把函数体包进 `try`,在 `catch` 里跑释放再重抛)**建过
两次,两次都被实测退回**:第一次让函数体里每个被赋值的寄存器都变成 try 区域的输出格,
`examples/syntax/defer.lk` 当场失去原生降低;第二次的阻碍是返回值的接线(整个函数的
`return` 都在被包的体内时,它自己没有 `Exit::Ret`,返回类型丢失)。理由与测量在
`core/src/stmt/defer.rs` 的模块注释和 `docs/aot/aot-gaps-and-lkrt.md` §17。

在缺口关掉之前:**必须活过 raise 的资源用 `try`/`catch`,不用 `defer`。**

另一条同源的限制:`defer` 只能出现在函数体的顶层,不能在 `if`、循环或嵌套块里。
顶层的文本顺序就是执行顺序,所以"写在这个 `return` 上面的每个 `defer`"恰好就是
"已经跑过的每个 `defer`";分支里的 `defer` 需要运行期跟踪,那正是这里不做的机制。

## 擦掉类型的容器仍然是容器(2026-08-06 裁决)

    fn has(h: Any, n: Any) -> Bool { return n in h; }
    fn app(xs: Any) -> Int { println(xs + [7]); return 0; }
    改前 → Type Error: 'in' operator requires container type (expected List<Any>, got Any)
           Type Error: List concatenation requires both operands to be lists
    改后 → 两端都答,两端都完整原生

`Any` 上的索引读、索引写、`len()`、`for-in`、方法分发、`push`、相等、`<`、`slice`、
`sort`、`join`、`delete` 一直都收;拒的是四个容器二元运算符:`in`、列表 `+`、列表 `-`、
map `+`。`Any` 的含义是"运行时才检查",而两个执行器在运行时都
一直有答案 —— 拒绝发生在检查器,不在语言里。

这条与 `in` 那条臂此前几次放宽是同一条队列:`Tuple` 与 `String` 曾经"别处都是容器、
只有这里不是",`Bytes` 与 `Slice<T>` 是随后两个(见 #185)。`Any` 是最后一个。

### 附带查到的一条:`"b" in "abc"` 根本没有原生降低

同一次探测发现的,与上面那条无关:字符串作为 `in` 右侧的形状,检查器收、VM 答,而
**任何拼写都不原生降低**,整程序回落到 VM(约 3 倍慢,没有任何提示)。而它的方法拼写
`s.contains("b")` 一直降得下去 —— 又是"同一个操作两种拼写只降一种"。现在两种拼写共用
`str.contains`;`dyn.contains` 也补上了缺的 `DYN_STR` 载体(它此前覆盖 map / list / set /
slice / bytes,独缺字符串)。

## 列表操作数吸收另一个操作数(2026-08-20 裁决)

`x + [列表]` 把 `x` 放到列表前面,`[列表] + x` 放到后面。两个执行器一直都这么
答,`lkrt_dyn_add` 的注释把它写成规则:"列表操作数胜过字符串操作数,所以
`"p=" + [1, 2]` 是列表 `["p=", 1, 2]`,不是文本 `p=[1,2]`"。

| 表达式 | 结果 |
| --- | --- |
| `"p=" + [1, 2]` | `["p=",1,2]` |
| `[1, 2] + "x"` | `[1,2,"x"]` |
| `1 + [2, 3]` | `[1,2,3]` |
| `nil + [1]` | `[nil,1]` |
| `[1] + {"k": 2}` | `[1,{"k":2}]` |
| `{"k": 2} + [1]` | `[{"k":2},1]` |

没有排除任何操作数类型,因为没有一种会 raise —— set、字节串、map、nil 都进列表。
这与 `Set + Set` / `Bytes + Bytes` 不同,那两个运行时确实报错,检查器拒它们是对的。

**此前只有检查器不收**,而且只在它看得见类型的时候不收:`Any` 操作数时同一个
表达式跑得通,类型已知时是 type error。与 `Any + Any` map 合并同一条缺陷,
同一条判据 —— 检查器多出一条两个执行器都没有的规则,和它漏掉一条一样是缺陷。

### 更糟的一半:异构字面量被判成 String

`[1, "a"]` 推出来是 `Tuple`。`Tuple` 在别处都是列表(索引、`len()`、遍历、`in`),
唯独没有进 `+` 的列表分支,于是 `"" + [1, "a"]` 落到字符串拼接那条路,**被判成
`String`**,而两个执行器答的是列表 `["",1,"a"]`。

判错类型比拒绝更糟:它会传播。`let v: String = "" + [1, "a"];` 过了 `lk check`,
运行时 `v` 是列表。

### 拼接的元素类型取更宽的那侧

`wider_of` 的文档一直写着它就是"`check_list_addition` 用的那条规则",而
`check_list_addition` 展开写的是相反的取舍:它取**可赋值给对方**的那一侧,也就是
更窄的那侧。`Int <: Float`,于是

```lk
let v: List<Int> = [1] + [1.5];   // 收
let n: Int = v[1];                // 收
typeof(v[1])                      // Float
```

同一对类型,调 `wider_of` 的 map 合并答的是 `Map<String, Float>`。一条规则两个
答案,文档还指着错的那个。

`wider_of` 自己底下还有一个洞:`is_assignable(Any, Int)` 是 true —— `Any` 可以
**传**到要 `Int` 的地方 —— 所以拿它问包含关系,答案是"`Int` 更宽"。不是。
`Map<String, Any>` 合并后被判成 `Map<String, Int>`,里面装着字符串。现在 `Any`
在问之前就先答掉。

原生侧随之补上:一侧是列表另一侧不是,两边装箱走 `dyn.add` 再拆回列表句柄,
和 map 合并同一形状 —— 否则检查器刚放行的程序会整个回落到 VM。

### `+` 拼接是第四种打印方式,而它是唯一还在失败的那种(2026-08-20 补完)

上面那条 2026-07-30 的更正写着 `ToString` / 模板插值 / `+` 拼接的"标量 only"规则
已经退休,容器该显示。插值改了,`+` 没有:

| 写法 | 改前 | 改后 |
| --- | --- | --- |
| `println(xs)` | `[1,2]` | 不变 |
| `println("{}", xs)` | `[1,2]` | 不变 |
| `println("${xs}")` | `[1,2]` | 不变 |
| `println("" + s)`(Set) | 运行时报错 | `Set([1])` |
| `println("" + b)`(Bytes) | 运行时报错 | `Bytes([97,98])` |
| `println("" + w)`(窗口) | 运行时报错 | `[1]` |
| `println("" + p)`(结构体) | 运行时报错 | `P{x:1}` |
| `println("v=" + m)`(map) | **检查器**拒绝 | `v={"k":1}` |

列表不在表里,因为它不是同一个问题:列表操作数**胜过**字符串操作数,答案是列表
(见上一节)。map + map 仍然是合并。变的只是"一边是字符串、另一边是没有 `+` 含义
的容器"这一种组合。

`1 + {"k": 1}` 仍然报错,两边都是 —— 既不是合并也不是拼接。

三处此前互不一致:VM 的 `runtime_value_display_string` 对堆对象直接 bail,而插值走
`runtime_display_value`;检查器只拒 map,放行 Set / Bytes / 窗口 / 结构体(它们照样
运行时报错);lkrt 的 `lkrt_dyn_add` 第 4 步**一直是对的**,所以类型被擦掉的程序答得
出来,同一个程序类型已知反而不原生降低。现在三处同一条规则。

顺带修好的一条:`runtime_value_display_string` 的另一个调用点是 "X is not a function"
的错误消息,容器 callee 在那里把诊断换成了一条更差的错误。

## 问是全函数,建键不是(2026-08-20 裁决)

`v in c`、`m.has(k)`、`s.contains(v)` 是**谓词**,一律有答案:不能当键的值不是成员,
答 `false`,不报错。而**构造键**的操作仍然报错:`m.set(1.5, x)`、`m.delete(1.5)`、
`s.add([1])`、`m[1.5]`、`m - 1.5`。

改前有两处不一致:

| 表达式 | 改前 | 改后 |
| --- | --- | --- |
| `1.5 in {"k": 1}` | false | 不变 |
| `1.5 in {1: 2}` | **报错** | false |
| `[1] in {"k": 1}` | false | 不变 |
| `[1] in {1: 2}` | **报错** | false |
| `1.5 in Set([1])` | **报错** | false |
| `m.has(1.5)` | **报错** | false |
| `s.contains(1.5)` | **报错** | false |
| `m - 1.5`、`m - [1]` | **报错** | 原样返回 |
| `m.delete(1.5)` | **报错** | nil |

第一处:同一个问题,答案由 map 的**内部载体**决定 —— 字符串键的 map 走
`string_map_contains_key` 答 false,整数键的 map 是 `Mixed`,走
`runtime_map_key_from_value` 报错。载体是程序看不见的东西,这与列表 `in` 那条
"答案取决于列表的内部表示"是同一个成因。

第二处:运算符和方法两种拼写答得不一样。`k in m` 答 false,`m.has(k)` 报错。

**删除也是"问"**:`m - k` 和 `m.delete(k)` 是查一个键再丢掉,不是构造键,所以
不能当键的值删掉的是"没有",不是报错。`-` 的分派按**左**操作数,与解释器一致 ——
读任一侧会把 `{"a":1} - [1]`(map 减一个恰好是列表的键)送去列表规则,然后被告知
左操作数不是列表。

**为什么选"答 false"而不是"一律报错"**:`in` 在列表上已经是全函数 ——
`"s" in [1, 2]` 是 false,不是类型错误。map / set 是同一个谓词换个容器。而且
"一律报错"会破坏现在能跑的程序(`1.5 in {"k": 1}` 现在是 false)。

native 侧同步:`lkrt_lkset_has` 与 `lkrt_dyn_contains` 的 map 分支改用
`key_from_dyn_opt`;`m.has(k)` 在 `k` 类型未知时走 `dyn.contains`(即 `in` 的
运行时分发),两种拼写共用一份实现。

### 谓词的方法拼写也收任何值(2026-08-20 补完)

上一节把 `in` 改成全函数,但方法拼写还按容器自己的元素/键类型声明参数,于是同一个
问题两种拼写两条规则:

| 运算符拼写 | 方法拼写 | 改前 |
| --- | --- | --- |
| `1 in ["a","b"]` 收,答 false | `["a","b"].contains(1)` | 检查器拒 |
| `1 in "abc"` 收,答 false | `"abc".contains(1)` | 检查器拒 + **运行时报错** |
| `"a" in "ab".bytes()` 收 | `"ab".bytes().contains("a")` | 检查器拒 + **运行时报错** |
| `"k" in {1:2}` 收 | `{1:2}.has("k")` | 检查器拒 |
| `{1:2} - "k"` 收 | `{1:2}.delete("k")` | 检查器拒 |

`contains` / `index_of` / `count` / `has` / `delete` 的参数类型改为 `Any`,
`string.*` 与 `bytes.*` 的运行时守卫改为答"不在"而不是报错。**插入不变**:
`push` / `add` / `insert` / `set` 仍然要求元素/键类型 —— 收了就等于让
`List<String>` 这个类型说谎。

原生侧:类型settle得了的直接折成常量(`"abc".contains(1)` 恒为 false),
settle 不了的走装箱后按值比较的 helper。顺带用上了 `list_h.i64_contains_f64` ——
这个 ABI 入口一直在表里没人调用,因为 `[1,2].contains(1.5)` 这个形状被检查器
挡在前面,谁也到不了。

### 反向的一条:归约要求元素类型

`["a"].sum()` 和 `[1.5].to_bytes()` 过检查、运行时必报错。这与 `Set + Set` 同款
—— 运行时对该类型的每个值都报错,检查器先说是对的。现在拒,且逐元素判:
`[1, "a"].sum()` 里的 `String` 就足以拒(`Tuple` 塌成 `List<Any>` 会把这条信息丢掉)。

`min` / `max` 不在其列:字符串有序。

### 索引:读是"问",写是"建"(2026-08-20 补完)

同一条线延伸到 `c[i]`:

| 表达式 | 改前 | 改后 |
| --- | --- | --- |
| `{"k":1}[0]` | 检查器"Cannot unify String with Int" | nil |
| `{1:2}["k"]` | 同上 | nil |
| `{"k":1}[0] = 9` | 拒 | **不变,仍然拒** |
| `[1,"a"][0..2]` | "Tuple index must be integer" | `[1,"a"]` |

读一个别的类型的键是**没查到**,不是错误 —— 解释器答 nil,与"键根本不存在"同一个
答案。检查器原来加的是一条 unify 约束,于是报的是它自己机器的话。约束还在,只在
两边都是具体且互不相容时跳过 —— 那正是"只可能查不到"的情形。

写仍然拒:`m[0] = 9` 会往 map 里放一个它自己的类型说不存在的键。

`Tuple` 切片是漏了一条 guard:每个容器分支都有 `matches!(&field, Expr::Range{..})`,
唯独 `Tuple` 没有,于是异构字面量成了唯一不能切片的列表。

原生侧两处跟上:键类型 settle 得了的直接折成 nil(`{"k":1}[0]`);settle 不了的
(键是 `Dyn`)把 map 装箱走 `dyn.get` 按 tag 分发 —— 原来是把键拆箱成 map 的键类型,
对 Int 键直接报错,而 VM 答 nil。**这条是本次改动引入的分歧,同一轮内修掉。**

`c[[1]]` 这种手写列表下标仍然拒:范围 `0..2` 就是列表 `[0,1]`,所以列表下标是切片
路径的实现细节漏出来,不是特性。

## 条件里的赋值是语法错,不是被丢掉的 token(2026-08-06 裁决)

    let a = 1;
    if a = 2 { println(1); }
    改前 → 通过检查,运行打印 1;编译出来的是 `if a { println(1); }`,常量 `2` 不在字节码里
    改后 → Syntax error: `=` assigns, and an assignment in LK is a statement rather than
           an expression — a comparison is `==`

赋值在 LK 里是语句,不是表达式(`let b = (a = 1);` 一直是语法错)。但**头部表达式**的
解析有两条路,只有一条查剩余:

| 路径 | 何时走 | 剩余 token |
| --- | --- | --- |
| 语句侧(`if`/`while`/`for` 的条件) | 一般情况 | `Parser::parse` 自带检查,报错 |
| 表达式侧(`parse_header_expr_before_brace`) | 尾位置的 `if` / `match` | 调的是 `parse_expr`,**不查** |

于是同一行代码,在文件最后一条时被接受并静默改写语义,往后挪一条就是语法错。这是
`=` 误写成 `==` 这个最常见的手滑,变成了错答案。

两处现在共用一条规矩:`{` 之前的全部 token 就是那个表达式,没消费完就报错。消息也
统一,并给 `=` 单独一句 —— 与 `export fn`(#201)同样的理由:人人会犯的错拼要被点名,
而不是只说"有个多余的 token"。

## 一个绑定器不能把同一个名字绑两次(2026-08-06 裁决)

    fn f(a: Int, a: Int) -> Int { return a; }
    println(f(1, 2));                             改前 → 2(第一个实参没人读得到)
    match t { [a, a] => { println(a); } … }       改前 → 匹配任意两个元素,绑后一个
    struct P { x: Int, x: Int }                   改前 → 接受;字面量填一次就满足"两个"
    改后 → 七个位置全部报错并指名那个名字

同一个绑定器里绑两次,第一次绑定永远读不到 —— 第二次在任何代码运行前就把它盖住了。
`[a, a]` 尤其危险:它**看起来**像"两个元素相等",而那是非线性模式的语言(Prolog、Erlang)
的读法;这里它匹配任意两个元素并绑定后一个。

七个位置一条规矩:`fn` 形参、`impl` 方法形参、lambda 形参、`struct` 字段、`let` 解构、
`for` 循环模式、`match` / `if let` / `while let` 的模式。

反向半边同样是规矩的一部分,因为这条是**按绑定器**判的,不是按作用域:后一条语句里重新
绑定同名是正常的(`let a = 1; let a = 2;`),`Or` 模式的每个分支绑同名是**必须的**
(`1 | 2` 里的绑定要在两臂都存在才可用),所以 `Or` 的各分支各自独立查。

### 附带修到的一条:`if let` / `while let` 的模式检查结果被 `.ok()` 丢掉

那两处的注释写着"拒绝匹配类型产生不了的模式",而 `.ok()` 恰好把那个拒绝扔了。类型不匹配
在 `if let` 上确实不该静态拒绝 —— 测试它正是这个构造的用途;但重名不是那类,没有任何值
能让它成立。所以查重拆成独立的方法,不经过那条被吞的返回值。

## `impl` 方法的签名必须顶得住 trait 的声明,而这句话在 `lk check` 里说(2026-08-06 裁决)

    trait T { fn m(self, a: Int) -> Int; }
    struct S { x: Int }
    impl T for S { fn m(self) -> Int { return 1; } }
    改前 → lk check 通过,一运行就 "Method 'm' arity mismatch for trait 'T'"
    改后 → lk check 就报同一句

判据本身一直存在(`TypeRegistry::validate_trait_impl`),只是在 **VM 注册 impl 时**才跑。
#99 把"方法有没有实现"那一半搬进了检查器,并在注释里写下"present 就是全部问题" ——
那句不准确:元数、形参类型、返回类型同样由那份判据管,同样被 `lk check` 放过。

现在是一条规则两个调用者:`trait_method_conformance` 被注册路径和 `impl` 语句的检查
共同调用。形参逆变、返回协变,是"一个签名要顶替另一个"的通常规矩。

## trait 是一个类型(2026-08-18)

trait 系统有三部分:`trait` 声明方法集、`impl` 为某类型提供实现、调用时按目标类型分发。
前两部分一直在,第三部分——**把 trait 的名字写在类型的位置**——一直报错:

```lk
trait Show { fn show(self) -> String; }
struct P { x: Int }
impl Show for P { fn show(self) -> String { return "P"; } }

fn render(v: Show) -> String { return v.show(); }
render(P { x: 4 })      // 此前:Argument 1 has the wrong type (expected Show, got P)
```

于是唯一能写"任何有 show 的东西"的办法是把参数留成无类型的,也就是什么都不检查。

修好后四个位置都成立:参数类型、返回类型、绑定的类型、容器的元素类型
(`examples/syntax/trait_as_type.lk` 逐条钉住)。查出来的原因有四层,每一层都是独立的:

| 层 | 现象 |
| --- | --- |
| `TypeRegistry.implementations` 在类型检查期是空的 | impl 只在**模块加载**时注册(`VmContext::register_module_types`),那是所有类型检查之后。`predeclare_type_declarations` 现在把 impl 和 struct/trait 一起提前登记 |
| `Type::is_assignable_to` 没有 registry | 它是 `values` 里两个类型之间的纯函数。加了 `TraitOracle` 参数,由类型检查器实现;`NoTraits` 是其他调用方的答案 |
| 返回类型走的是**合一**,不是可赋值性 | 同一个问题的两个答案,只有一个有这条规则。`unify` 补上对应分支 |
| 异构字面量推成 `Tuple` | `container_literal_fits` 只有 List/Set/Map 三个分支,`[p, q]` 写给 `List<Show>` 从来没走到 List 那支 |

顺带删掉了一个重复结构:`TypeInferenceEngine` 自己持有一份 `TypeRegistry` **克隆**,
是构造检查器时拷的,之后所有声明它都看不见。它只用来发 `T{n}` 编号,现在改成一个 `u32`
计数器,registry 由 `solve_constraints` / `unify` 按引用传入。

**跨模块也成立**(同日补)。trait、struct 和 impl 的**方法签名**本来就都过了模块边界,
唯独"这个类型实现了这个 trait"这条关系没过——`seed_declared_types` 只登记 struct / trait / 别名。
于是被导入文件里的 `fn describe(v: Area)` 对它自己声明了 impl 的那个类型报
"expected Area, got Sq":这个特性在一个文件里成立,出了文件就不成立。
`cli/tests/compile_cli_test.rs::test_trait_as_a_type_accepts_an_imported_implementor` 钉住了它。

还有一条:trait 类型的接收者只能调用 trait 声明过的方法。此前 `v.nosuch()` 直接到运行期,
而两个后端"发现"的方式不同,报的话也不同——解释器说 `P has no method 'nosuch'`,
编译版说的是 map 属性那句。

## 容器模式里的子模式(2026-08-18)

`match` 的列表/映射模式此前**只检查形状**——列表的长度、映射的键——里面写别的一律拒绝:
`Compiler does not support nested refutable pattern yet`。也就是说

```lk
match p {
    [0, 0] => "origin",
    [0, y] => "on-y",
    [x, y] => "point",
}
```

这种写法不能编译,而模式匹配大半是这么用的。现在子模式和顶层模式是同一种东西,
字面量、区间、嵌套容器、`|`、`if` 守卫都可以出现在里面。

三处配套:

| 位置 | 内容 |
| --- | --- |
| `pattern_control::lower_container_pattern` | 形状测试 + 逐个子模式。取元素放在形状守卫**内部**——对非列表取下标会抛错 |
| `lower_subpattern` | 容器里的裸名字是**绑定**,不是"非 nil"测试。`lower_pattern_match` 是和 `if let` 共用的,那里裸名字意味着非 nil |
| `type_checker/patterns` | 元组按位置给类型。此前所有位置共用一个元素类型,`match ["a", 2] { ["b", n] => … }` 报的是检查过程自己造出来的 `Cannot unify Int with String` |

模式树用到的寄存器在**第一个形状测试之前**统一分配并置 nil。VM 里没写过的寄存器就是 nil,
原生降级要建 SSA,一条走不到的路径上的读也是读。`..rest` 例外,留在守卫外面:它产出的是
列表或映射句柄,一边 nil 一边句柄没有统一类型。

顺带修了两个原生降级的**静默错答**(不是回退,是编译通过并答错):

- `IsList` / `IsMap` 只比较一个 tag(`DYN_LIST` / `DYN_MAP`)。列表有五种表示、映射有六种,
  解释器还把 `String` 算作列表(`let [a, b] = "ab"` 成立)。落在类型化载体里的列表因此答 `false`,
  跳过了本该走的分支。现在走 `dyn.is_list` / `dyn.is_map`,与 lkrt 其余部分用同一个判定。
- `lkrt_dyn_index` 对装箱的 `String` 抛 `runtime type error`,而未装箱的 `s[0]` 一直是可以的。

**`match` 的分支是提问,不是要求**(同日续)。无类型参数被多个模式匹配时,scrutinee 的类型
还是开放的,而列表/映射模式会给它加一条 `= List<T>` 的约束——于是同一个 `match` 里三个分支
互相报冲突,冲突是检查过程自己造的。类型开放(`Variable` / `Union`)时不再加这条约束:
其它分支存在,正是因为答案可以是"不匹配"。

`..rest` 的绑定位置由此定下来:它在形状守卫**内部**,槽位事先用**同类的空容器**(而不是 `nil`)
初始化。放在守卫外面,一个 Map 走到 `[x, ..rest]` 分支会对它跑 `SliceFrom` 并抛
"not sliceable"——从一个根本不匹配的分支里抛出来;用 `nil` 初始化,则一边 nil 一边句柄,
原生降级没有统一类型可给。

## 位运算补上 `^`(2026-08-18)

`&` `|` `<<` `>>` `~` 都在,只有异或没有拼法——而 `__lk_bit_xor` 这个名字已经出现在
类型检查器的 arity 表和 VM 编译器的 builtin 列表里,是一个**没有东西能产生**的名字。
现在:词法器认 `^`,解析器在 `|` 和 `&` 之间加一层(C 和 Rust 的优先级),VM 装了
`core_bit_xor_builtin`,AOT 走 `IntBinOp::Xor`(`~x` 本来就是拿它和 `-1` 做的)。

顺带补了 tree-sitter 文法:它此前**一个位运算都没有**。加进去暴露一处真实歧义——
`p if x | y` 里的 `|` 是守卫的按位或还是 or-pattern 的分隔符。真实解析器里守卫先赢
(`parse_guard_pattern` 把守卫当作一整个表达式解析,`|` 在 `parse_or_pattern` 看到之前
就被吃掉了),文法里按同样的方式声明了这个冲突。

位运算的复合赋值也补齐了(`&=` `|=` `^=` `<<=` `>>=`)。它们**不**新增 `BinOp` 变体——
`a & b` 本来就是 `__lk_bit_and` 调用,给它第二种拼法就意味着第二套降级、第二条类型规则,
以及两边分歧的可能。`a &= b` 直接脱糖成 `a = a & b` 已经产生的那个调用,
名字、下标、字段三种目标都走同一条路。

`<<=` / `>>=` 是**三个相邻的 token**:词法器从不发射移位,`<<` 是两个 `<`,所以 `<<=` 是
`<` 接 `<=`。靠 span 相邻区分它和 `a < (b <= c)`,与 `Parser::peek_shift` 判断移位本身的方式相同。

## 声明的字段类型是要执行的(2026-08-18)

此前 `struct P { v: Int }` 的 `v` 可以装字符串,解释器不拦:

```lk
fn poison(p) { p["v"] = "s"; }
let a = P { v: 1 };
poison(a);
println(a.v);        // "s"
```

类型检查器只能检查它**看得到**的写入;通过无类型绑定的写入它看不到。于是声明只是注释——
而 AOT 想按声明类型给字段读定型时,这一点立刻变成静默错答(见 `docs/aot/aot-gaps-and-lkrt.md` §41)。

现在四种写法都按声明检查,VM 与原生同一条规则、同一句话:

| 写法 | 位置 |
| --- | --- |
| `p["v"] = x` / `p.v = x` | `exec::container` 的对象写入 / `map_h.str_dyn_set` |
| `P { v: x }` | `read_object_fields` / NewObject 降级(**先打标记再写字段**) |
| `P { ..m }` | `core_make_struct_builtin` / `map_h.obj_mark`(标记时校验已有字段) |

`field \`v\` of P is declared Int, and a String cannot be stored in it`

**检查在能静态判定时不进入运行期。** 降级这一侧知道被存值的类型和字段的声明码,所以
`P { x: 1 }` 这种一个字面 `Int` 存进 `Int` 字段的写法**一条指令都不发**;判不了的时候
声明码是编译期常量,运行期只是一次 tag 比较(`obj_ty.check`),不查表。只有当接收者的结构体类型
连降级也不知道时才走标记查表那条(`obj_ty.check_marked`)。

第一版把标记提到字段写入之前、让 `map_h.str_dyn_set` 自己查表,结果是 20 万次构造从 0.72s 变成
0.88s——**比调试版解释器还慢**。现在构造 0.72s、字段自增循环 0.04s。

两道静态过滤把剩下的运行期检查也基本清空了:接收者是这个函数看着 map **字面量**产生的,
那它不是结构体实例(结构体来自 `NewObject`,那条已经带类型名);以及**没有任何声明的结构体
有这个名字的带类型字段**时,不论这个 map 是什么,这个键都不可能违反声明。于是不声明结构体的程序
一条检查都不发,`m["k"] = v` 这样的循环也不发。

**只检查标量**(`Int` `Float` `Bool` `String` 及其可空形式)。容器的元素类型不是单个值携带的东西——
`List<Int>` 和 `List<String>` 在运行期是同一个 `HeapValue::List`——所以容器字段、`Any`、联合、
命名类型都不检查。剩下的正好是"写错了会静默损坏"的那一组。

`Int` 满足 `Float` 字段,并且**仍然是 Int**:这门语言在类型边界上从不做隐式转换,
`fn f(x: Float)` 收到 `1` 时 `typeof(x)` 也是 `Int`。字段这里保持一致。

**容器的元素类型不按这条办,而且是有意的。** `fn add(xs) { xs.push(B { … }); }` 确实能把一个 `B`
推进 `List<A>`,但那不是同一类问题:

- `struct` 是显式的、按类型声明一次的,而且**跟着值走**(`DeclaredType` 挂在实例上)。列表没有
  对应的东西——运行期的列表只有表示(`TypedList::I64` 之类),没有声明。
- 列表的元素类型多半是**推断**出来的,不是写出来的。`let xs = []; xs.push(1); xs.push("a")`
  是这门语言正常的写法,异构列表是一等公民;把推断出的 `List<Int>` 变成运行期约束会直接废掉它。
- 想只约束**写出来的**那一种(`let xs: List<Int>`),就得让值本身记住"我的元素类型是被声明的",
  那是给每个列表加一个类型字段外加每次 push 一次检查——为一个语言本来就允许的形状付全程代价。

所以这条到此为止。AOT 也因此**不能**按元素类型给读定型(`docs/aot/aot-gaps-and-lkrt.md` §41
撤回的那一半),这是设计上的结论,不是待办。

## 拿"该被拒绝的程序"当 oracle(2026-08-18)

> 这份表留了下来:`cli/tests/check_oracle_test.rs`。35 条"必须拒绝"、15 条"必须接受",
> 后者里包括**有意接受**的那几条,并写明理由——否则下一个读到的人会把它们当 bug"修掉"。

前几轮都是 VM ↔ 原生的差分。换个问法:**哪些明显有错的程序 `lk check` 放过了?**
写了 62 个用例,两个方向都有——32 个"必须报错"、15 个"必须通过",外加边界形状。

放过的里面,三条是真缺口,已修:

| 形状 | 此前 | 现在 |
| --- | --- | --- |
| `impl T for P { fn b… }`,`b` 不是 `T` 声明的 | 只在 VM 注册 impl 时报,`lk check` 放过 | 检查期报 |
| `P { x: 1, x: 2 }` | 静默取后一个 | 报错。理由与"参数名重复""模式里绑定名重复"完全一样:第一个值没有任何东西能读到 |
| `use math as m; m.nope(1)` | 放过,运行期 "nil is not a function" | 报 `` `math` has no member `nope` ``——别名记了名字没记模块,于是成员检查查的是一个叫 `m` 的模块 |

放过但**有意为之**的:`if 5` / `while "a"`(这门语言有真值性)、`{"a":1,"a":2}`
(后写的覆盖先写的,与 `IndexMap::insert` 一致)、`"a".repeat(-1)` 和 `P { ..5 }`
(运行期错误,消息清楚)、顶层 `return`(模块的返回值)。

第四条 `P { ..5 }` 也修了:spread 脱糖成 `__lk_merge_fields(base, overlay)`,而没有人看过 base,
所以运行期抛的是 `__lk_merge_fields base must be Object, Map, or Nil, got Int`——
一句话里说的是脱糖结果,不是读者写的东西。现在检查期就说"`..base` 要的是结构体、映射或 nil"。

**下标的可空性:量过之后决定不改。** `m["z"]` 和 `xs[9]` 运行期给 `nil`,而类型检查器把它们
定为 `V` 而不是 `V?`,所以 `let v: Int = m["z"]` 通过、`v` 是 `nil`。按 `?` 模型这该是 `V?`。

不改的理由是数出来的:examples 和 bench 里共 113 处下标读,只有 3 处所在行有 `??` 或 `!`。
改成 `V?` 意味着其余 110 处、加上标准库和所有用户代码都要改写,而 `assert(listed[0] == 7)`
这种比较也要跟着变。收益是"`let v: Int = m["z"]` 会报错"——而那个 `nil` 本来也会在下一步绊倒程序。
这门语言在这件事上的立场是一致的(`if 5` 有真值性、下标越界给 nil),不是漏了一条规则。

## 列表模式说的是**几个**(2026-08-20)

`match xs { [] => …, [a] => …, [a, b] => … }` 对任何长度的列表都答第一条 ——
`[]` 匹配一切,后面每条都是死的;`[a]` 也匹配两个元素的列表。

`lower_list_pattern_condition` 发的是 `CmpGeInt`,长度**下限**,而且六个调用点
全都把 `rest` 丢掉了。于是"有没有 `..rest`"这件事对形状测试没有影响。

改成:**没有 `..rest` 就是相等,有才是下限**。这正是 `..rest` 存在的意义,
而且语料里每一处需要前缀的地方都已经显式写了它
(`let [first, second, ..rest]`、`let [_, second_item, .._]`、`while let [top, ..rest]`),
文档里也没有任何地方说过裸模式是"至少"。

影响到的不只是 `match`:`if let` / `while let` / `for` 的元组与数组模式、
以及 `let` 解构都走同一条。`let [a, b] = [1, 2, 3]` 现在报
`Pattern does not match value`,`if let [a, b] = [1,2,3]` 现在不匹配 —— 都是对的。

两个后端一致(这条 bug 在 VM 的编译器里,所以此前两边一起错)。
全量测试、61 程序 sweep、覆盖率门禁、性能门禁(geomean 1.038x)全绿;
`examples/syntax/match.lk` 加了两组断言,其中一组把空模式写在长模式**之后**,
这样即使分支顺序救不了它也能被发现。

## 未标注参数的加宽,不该被一个自由变量挡住(2026-08-20)

`fn f(v) { … }` 同时被 `f([])` 和 `f(5)` 调用,报
`Cannot unify Int with List<'T2>`。而 `f([1])` 加 `f(5)` 是接受的 —— 同一个程序,
只是列表里多了一个元素。`Int + String`、`Int + List`、`Map + Int`、`Int + Float`
也全都接受。唯一被拒的是**空**列表(和空 map)。

`widen_rebound_variable` 的作用是"一个变量先被绑成一个类型,现在又要求是另一个,
那就绑成两者的并集"。它两侧都写着"含自由变量就不加宽",理由是
"推导还没决定完的事,不该由加宽来决定"。

对**裸变量**来说这是对的:它还没有形状。但 `List<'T2>` —— 空字面量给出的那个 ——
形状已经定了:它是列表,而列表无论 `'T2` 最后是什么都不可能和 `Int` 统一。
所以拒绝加宽并没有推迟一个决定,它报告了一个冲突。判据改成
**只有裸变量才拒绝**,构造出来的类型照常加宽,里面的变量随并集留下,以后照常代换。

顺带:哪一侧被挡住取决于求解器先弹哪条约束(`constraints.pop()` 是 LIFO),
所以两侧的判据都要改。

真正的类型错误照旧报错(`fn f(v: Int)` 传 `"s"` 仍然是
`Argument 1 has the wrong type`)。

## 一个浮点数怎么变成字符串,和它从哪来无关(2026-08-20)

用**期望值**(而不是拿两个后端互比)逐条探语言语义时出来的两条。上一条列表模式的教训是:
编译器里的 bug 两个后端一起错,差分门禁按定义看不见。

### 常量折叠用的是另一个格式化器

`"" + 3.0` 折叠成 `"3.0"`,而 `let x = 3.0; "" + x` 是 `"3"`。
折叠那一侧用 `ryu`(最短往返格式化器),其余一切用 Rust 的 `Display` ——
后者才是本文档定下的规则,lkrt 那边也是逐字节对着它做的。

差别不只是那个小数点:`"" + 1.0e300` 折叠出来是 `1e300`,运行时是三百位数字。
**同一个表达式两个答案,取决于操作数碰巧是不是字面量。**折叠改用 `to_string()`。

### 浮点操作码没有动态回退,而它的整数孪生兄弟有

`"" + (1.0 + 2.0)` 在**运行时**报 `register 3 expected Int or Float: got String`,
而 `lk check` 一声不吭就放过去了。`"" + (1 + 2)` 是好的。

编译器按**一个**操作数的类型挑 `AddFloat`,不看另一个;括号里那半被折叠成浮点常量之后,
外层就成了"字符串加浮点"。`AddInt` 遇到非整数对会转去 `dynamic_add`,
`AddFloat` 直接报错 —— 这个不对称就是 bug。

改的是**回退**而不是选择:选择是一个关于类型的猜测,而这里是知道答案的地方;
守卫本来就在(它就是报错的那个),所以冷分支不比原来的错误多花什么。
`AddFloat` / `SubFloat` / `MulFloat` / `DivFloat` / `ModFloat` 五个现在都和整数孪生兄弟一样转去动态形式。

原生降级那边同样处理:`Str` 操作数走 `dyn.*`,和它本来就有的 `Dyn` 分支同一条路。

语料在 `examples/syntax/numeric_auto_promotion.lk`:同一个值一次来自字面量一次来自变量,
断言两者相等 —— 这一条不需要知道正确答案是什么就能发现分歧。

## `!` 的操作数从来没被检查过(2026-08-20)

`lk check` 的立场是"执行器跑的就是这一份检查"。拿一批**运行时会报类型错**的程序
逐条对着它跑(检查通过 + 运行失败 = 漏检),15 条里出来一条:

```lk
println(!5);        // lk check 通过,运行时 Not expected Bool or Nil, got Int
println(5 && true); // 两边都在检查时就拒绝
```

`check_unary_op` 的 `Not` 分支只在操作数是类型变量时加一条约束,然后无条件答 `Bool` ——
任何东西都放过。它的兄弟 `Neg` 一直走 `classify_numeric_operand`。

改成:**只有确定不可能是 `Bool` 或 `Nil` 的才拒绝**。类型变量、`Any`、`Optional`、
以及成员里有 `Bool` 或 `Nil` 的联合都照旧接受 —— 在一个大多数值不带标注的语言里,
"证明不了对就拒绝"会把日常写法拒掉。措辞用执行器那句(`Not expected Bool or Nil`),
两边说同一句话。

`cli/tests/check_oracle_test.rs` 两个方向都记了:四条必须拒(`!5`、`!"a"`、
`!f()`、`5 && true`),三条必须收(未标注参数、`Int?`、容器读)。

同一批探测里其余 14 条都是对的(下标类型、调用非函数、缺方法、字段访问、
负号作用于字符串、迭代整数、返回类型、参数个数),记下来免得重探。

## 闭包里的 `defer` 根本没被改写(2026-08-20)

```lk
fn f() -> Int { let t=[]; defer t.push(1); return t.len(); }   // 0
let g =    || { let t=[]; defer t.push(1); return t.len(); };  // 1
```

同样三行,函数里答 0,闭包里答 1。字节码上一目了然:函数那份是
`Len`(算返回值)→ `Move`(存起来)→ `ListPush`(defer)→ `Return`;
闭包那份是 `ListPush` → `Len` → `Return` —— defer 先跑了。

`defer` 是一次**改写**:把延迟语句复制到每个 `return` 之前,并把返回值先存进临时变量。
改写的入口 `descend` 处理 `Stmt::Function`,而 lambda 是一个**表达式**,
所以闭包体既没有被改写,也没有走到那个"`defer` 不能写在分支/循环/嵌套块里"的拒绝 ——
`Stmt::Defer` 直接当普通语句编译,原地就跑了。

补了一个 `descend_expr`,走到闭包体和 `try` 区域。**这个 match 没有 `_` 分支**:
这次出问题的正是一个没人列出来的形状,而 `_` 就是下一个漏网之鱼的入口。
顺带,写在闭包里分支中的 `defer` 现在也会被正确拒绝(此前一声不吭地原地执行)。

`examples/syntax/defer.lk` 里加了一对断言:同样的三行,一次写成闭包一次写成函数,
断言两者相等 —— 和上一条浮点渲染一样,不需要知道正确答案就能发现分歧。

## 解析出来的文档保持文档序(2026-08-20)

map 的迭代与 display 顺序是"键第一次被写入的顺序"——这是契约。对一个**解析出来的**
文档,那就是它在文档里出现的顺序。而 JSON 和 TOML 都是按字母排的:

```lk
use encoding;
encoding.json.parse("{\"b\":1,\"a\":2}").keys()   // 之前 [a, b],现在 [b, a]
encoding.toml.parse("b = 1\na = 2\n").keys()      // 同上
```

两处都不是决定,只是各自中间类型的默认:`serde_json::Value::Object` 与
`toml::Value` 的表都是 `BTreeMap`。YAML 的 `Mapping` 本来就是有序的。

- JSON:不再经过 `serde_json::Value`。`serde_json` 的 `preserve_order` 特性会**显式打开
  `std`**,而这个 crate 必须能不带 std 构建(裸机保留 JSON),所以改成直接反序列化到一个
  对象存 `Vec<(String, _)>` 的中间类型 —— serde 的 `MapAccess` 本来就按文档序交付。
- TOML:用 crate 自己的 `preserve_order`。这里不花钱,TOML 解码本来就只在 std 下。

**写出去的方向不动**,而且那是有意的(`core/src/val/ser.rs` 的模块注释):
`stringify` 排序,所以同一份数据写两次字节相同、diff 得动,JSON 本身也不规定键序。
读与写不冲突:写强加一个顺序让字节稳定,读报告它拿到的顺序。

lkrt 那边跟着改了同一份(它的注释本来就写着"和 VM 逐字节一致"),
第一版只改了 VM,`vm_native_sweep` 当场报 `yaml_toml.lk` 两个后端 stdout 不一致 ——
这正是那条门禁存在的理由。


## 容器字面量在四个位置有三个答案(2026-08-21 裁决)

容器在元素类型上是**不变**的:一次加宽就是一个别名,宽的那个名字能写进窄的
那个名字不允许的元素(`a_container_cannot_be_widened_at_its_element_type` 举了
完整的例子)。字面量是例外:它是新建的,没有第二个名字,所以按元素逐个协变检查。

例外这条以前写了两份——`let` 语句一份、调用实参一份——剩下两个位置一份都没有:

| 写法 | 修复前 |
| --- | --- |
| `let f: List<Any> = [1];` | 接受 |
| `take([1])`,`fn take(v: List<Any>)` | 接受 |
| `S { f: [1] }`,`f: List<Any>` | 拒绝 |
| `fn f() -> List<Any> { return [1]; }` | 拒绝 |

同一个写下来的值,四个位置三个答案。规则收成 `TypeChecker::value_fits` 一处,
四个位置都用它;两份副本删掉。`return` 那处在记录返回类型时直接采用声明类型
(`record_return` 的调用点同时看得见表达式和声明),所以 `pop_return_frame` 仍然
是一串普通类型。

不变性没有松:变量在四个位置仍然全部拒绝,元素类型不合的字面量
(`S { f: [1.5] }`,`f: List<Int>`)也仍然拒绝。

机器整数的字面量规则(`let x: u8 = 5` 不需要写 `5 as u8`)是同一条裂缝的另一半,
缺的是同样那两个位置,一并并进 `value_fits`。越界仍然是拒绝——有范围正是定宽的
意义。三处的越界文案不同(`let` 那处会说"literal 300 is out of range for u8",
另两处说类型不匹配),都准确,没有统一。

把剩下的位置也走了一遍,又缺两个:**重新赋值**(`xs = [1]`)和**字段写**
(`s.f = [1]`)。写进去和第一次绑定是同一件事,两处都改用 `value_fits`。

规则本身还漏了一层:它比较的是**类型**,只往下看一级就停,所以
`let xs: List<List<Any>> = [[1]];` 和 `let xs: List<u8> = [5];` 被拒——这两句
对**变量**来说都成立,对字面量都不成立。改成由**表达式**驱动、递归下降:
元素类型取自已经推好的结果(同构字面量是 `List`,异构的是 `Tuple`),所以没有
任何东西被重新检查,只有豁免往下走。`Type::container_literal_fits{,_with}` 因此
没有调用者了,删掉。
