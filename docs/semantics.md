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
| `return 20 / 4;` | `5` | 成功 | `/` 对 Int/Int **返回 Float**(类型层面);Float 值为整数时显示省略小数部分 |
| `return 1.0 / 7.0;` | `0.14285714285714285` | 成功 | Float 显示 = Rust `f64` 的 `Display`(VM-exact,native 侧经 `lkrt_f64_to_str` 逐字节对齐) |
| `return 5 + 7.5;` | `12.5` | 成功 | Int/Float 混合算术提升为 Float |

注:`/` 产 Float 是整数中点必须写成 `math.floor((lo + hi) / 2)` 的原因
(VM 侧 lower 为 `MidInt`)。

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

- `while` 条件**必须**带括号;`if` 条件可不带,但 `if (expr) op rhs` 形式会把
  首个括号组解析为整个条件——生成器 / 工具生成的 `if` 条件应整体加一层括号。
- 语句以 `;` 结尾。
- `try`/`catch`、`select`、`go`、后缀 `!` 均为 **parse 时糖**(分别降到隐藏
  native `try$call`、`select$block`、`spawn(闭包)`、nil 检查 Conditional),
  不存在专用 AST 节点;`select`/并发语义见 `docs/concurrency.md`。
- **后缀 `!`(force unwrap)**:`expr!` 在 nil 时 raise "unwrap of nil value"
  (可 catch),否则原值。两条边界:`!` 紧跟 `(`/`[`/`{` 是**宏调用**语法
  (`name!(...)`),解包后调用/索引需加括号 `(x!)(...)`;lexer 贪婪 `!=`→Ne,
  `x!==1` 是 parse 错误,写 `x! == 1`。
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

## `unique()` 等值语义(2026-07-06 裁决)

`list.unique()` 走 VM `core_methods` 的 `runtime_values_equal`:数值按 `to_bits`
(`1 == 1.0` 去重、`0.0 != -0.0` 保留)、≤7 字节字符串(`ShortStr`)按内容、
**列表/map/长字符串按 heap 句柄**。句柄同一性是 VM 内部表示细节,长字符串
(>7 字节)在 typed String 列表里每次读出重新 alloc(`[s, s].unique()` 保留两个),
在 Mixed 列表里直存句柄(`[1, s, s].unique()` 去重)。native 侧字符串常量 intern,
指针无法区分这两种,**裁决:native 对长字符串永不去重**——对齐字面量重复与
typed 列表两种常见形状;Mixed 列表同变量长串重复是已知分歧,不进差分子集。
列表元素的句柄同一性 native 以「NewList 窗口内同寄存器装箱一次」保持
(`let l=[7]; [l,l].unique()` 去重,两个 `[1]` 字面量不去重)。

## `in` 操作符等值语义(2026-07-06 裁决)

`needle in list` 走 VM `list_contains`,**与 `==`/unique 都不同**——第三套 eq:
typed 列表严格同型(`1.0 in [1, 2]`、`1 in [1.0]` 均 false,无数值 coercion;
String 列表按内容,长短一致);Mixed 列表是 `RuntimeVal` 的 derive `PartialEq`
(同变体严格、float 按值 `==`(`0.0==-0.0` true、NaN 永 false)、ShortStr 内容、
heap 对象按句柄)。native:typed 列表跨型 needle 编译期折叠 false,Mixed
(`ListDyn`)走 lkrt `contains_eq`(同款 strict 语义);长字符串/嵌套列表的句柄
同一性限制与 unique() 同款(intern/转换边界,已留档,不进差分子集)。

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
