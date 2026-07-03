# lk（lkr）推倒重写技术规划：以 VM 为语义基准的双后端、可嵌入且可降级到 no-std 的脚本语言

> **重要事实更正（贯穿全文）**：本规划基于对当前仓库 `github.com/lollipopkit/lk` 的实测。原始需求假设 lk 用 Go 实现、"Go 无法做 no-std 需考虑改写"。**实测证明 lk 当前已经用 Rust 实现**——Cargo workspace，crate 含 `core`/`lkrt`/`aot/{abi,mir,lower,codegen}`/`llvm`/`stdlib/*`/`lsp`/`wasm`/`ecosystem/tree-sitter-lk`，用 `serde`/`ed25519-dalek`/`tokio`/`llvm-tools`，Rust edition 2024。因此"用 Go 无法做 no-std"这一前提不成立——**Rust 原生支持 `#![no_std]`+`alloc`**。此外实测发现 lk 已相当成熟：register VM、手写 tracing GC（`alloc_heap_value`/`collect_pending_garbage`/`LK_GC_STRESS` root 压测）、load-time 字节码验证器、MIR-based AOT（`lower→mir→codegen→clang+liblkrt.a`）、约 1460 个测试、Miri/ASan/UBSan/fuzz CI。本规划据此把"推倒重写"重新定义为**重整架构 + 补齐能力 + 无情砍范围**，而非从零重写。

---

## TL;DR（三点核心结论）

- **实现语言保留 Rust（唯一理性选择）；VM 定为唯一语义真源（reference implementation），AOT 逐位追赶、差分测试当门禁。** 这与 Lua（PUC-Rio 解释器定义语义、LuaJIT 追赶）、OCaml（`ocamlc`/`ocamlopt` 共享前端）、Dart（JIT/AOT 共享 kernel IR）、V8（Ignition baseline + TurboFan）完全一致。重写为 Go 会摧毁 no-std 能力，重写为 C/Zig 会丢掉借用检查与 Miri 这些正是保证"双后端一致"的安全网。
- **"推倒重写"应执行为四件事，而非从零**：(1) 分层去全局状态（修复并发/耦合）；(2) 引入 `pcall`/`error`/`try` 可恢复错误模型；(3) AOT 先做"字节码嵌入"这一 100% 覆盖、语义平凡一致的形态，再做逐函数回退 VM 的混合模式（彻底消除"全有或全无"）；(4) 把中心化签名注册表包管理器换成 git+lockfile 去中心化依赖（Deno/Go 模式）——LSP 与 tree-sitter 双轨编辑器支持均保留。
- **no-std 是 Rust 原生优势，落地为 `bare`/`alloc`/`full` 三档 feature profile + `lk-hal` 平台抽象层**；v1.0 最小目标定为 WASM + 一类带 allocator 的 MCU（ESP32/Cortex-M+alloc），不追真裸机。

---

## Key Findings（关键结论）

1. **VM 应为规范、AOT 为被验证方，这是行业标准且逻辑上唯一自洽**：AOT 覆盖是渐进的（当前实测 10/44 examples 可原生编译），让覆盖不完整的一方定义语义不可行。Lua 官方甚至明确其测试套件"目标是测试他们的参考实现，而非作为其它实现的一致性测试"（lua.org/tests 表述其测试"main goal is to try to crash Lua … not intended for general use"；Mascarenhas 等《Decoding Lua》arXiv:1706.02400 转述："the goal of the test suite is to test their reference implementation of Lua, and not to serve as a conformance test for alternative implementations"）——即**参考实现即规范**。

2. **十大问题全部可被有序里程碑修复**（见第十章映射表）。最高杠杆的是 M0+M1：确立 VM 真源 + 消除全局状态 + 差分门禁，一次性修复问题 1/5/8/9 的地基。

3. **现有资产宝贵，不应丢弃**：register VM（性能实测 VM/Lua geomean≈1.033×，接近 Lua）、手写 tracing GC、load-time 验证器、3 套差分语料 + MIR 快照 + golden 语义向量、Miri/ASan/UBSan/fuzz CI——这些是"双后端语义一致"的现成安全网，重写为其它语言等于全部推翻。

4. **AOT 的"全有或全无"是当前最严重的架构缺陷**：实测 MIR 后端是"total capability predicate，子集外整程序 `Unsupported` 失败，无回退后端"。修复路径是先做 Tier 0（字节码嵌入，平凡一致、100% 覆盖），再把 `Unsupported` 从"整程序失败"改为"逐函数回退 VM"。

5. **范围收敛是 solo 维护成败的关键**：中心化签名注册表（HMAC/Ed25519 keyring、`lk pkg serve`、publish/yank）对一个 14-star 单人项目属过度工程，应替换为 git+lockfile（Deno/Go 模式）的去中心化依赖。**LSP 保留、不砍**——现有 macro-origin hover/goto-definition/semantic tokens/inlay hints 是已建成的资产，与 tree-sitter 互补（前者语义、后者容错高亮），持续维护。

---

## Details（详细设计）

### 一、目标与非目标

**定位陈述**：lk 是一门 **Rust 风格语法、动态类型脚本语言**，同时满足三种形态——(1) 可嵌入脚本语言（像 mlua/rhai/gopher-lua 嵌入宿主）；(2) 独立脚本语言（CLI：REPL + 跑 `.lk` + 编译原生可执行文件）；(3) 核心可降级到 no-std（WASM、嵌入式 MCU），核心运行时不依赖 OS/文件系统/网络。

**语言表面（保留，不改设计）**：Rust 风格语法、一等命名参数（`named_args`/`named_params`）、`macro_rules!` 式声明宏与 proc-macro provider、`match`/pattern matching（if-let/while-let/解构）、闭包与高阶函数（map/filter/reduce）、惰性 stream 管道、模板字符串插值、列表/映射/range、`in` 运算符、Int(i64)/Float(f64) 分离语义（`/` 返回 Float）。

**目标**：单一语义真源；可恢复错误模型；分层可裁剪（`bare`/`alloc`/`full`）；多实例隔离无全局状态；AOT 混合模式永不"全有或全无"；solo 可维护的收敛范围。

**非目标**：不做运行时 JIT（仅 AOT+解释）；不保证向后兼容；`.lkm` 不作对外分发格式；不做中心化签名注册表（改 git+lockfile 去中心化依赖）；v1.0 不做真正裸机（core-only 无 allocator），最小目标为 WASM + 带 allocator 的 MCU。

### 二、技术选型决策与理由

**决策 A：保留 Rust，不重写为 C/Zig/Go。** 诚实权衡表：

| 维度 | Rust（现状，推荐） | TinyGo/Go | Zig freestanding | C（ANSI C99，如 Lua/Berry） |
|---|---|---|---|---|
| no-std/裸机 | 原生 `#![no_std]`+`alloc`，`core`/`alloc`/`std` 三层切分是语言内建 | TinyGo 可出 WASM/MCU，但 GC、goroutine、reflection 受限，标准 Go 不可裸机 | freestanding 原生，手动内存 | 原生，标杆见下 |
| 内存安全 | 安全子集 + 局部 unsafe，可 Miri/ASan 验证 | GC 安全但不可控 | 手动，`comptime` 强但无借用检查 | 完全手动，靠海量测试兜底 |
| GC 实现 | gc-arena 提供**安全 Rust 内做 tracing GC**的成熟范式 | 复用 Go GC（snarfing） | 手写 | 手写（Lua 5.4 增量标记清除） |
| 嵌入 C ABI | `extern "C"`+cbindgen，零成本 | cgo 摩擦大 | 一等 C 互操作 | 天生 |
| 生态/作者熟悉度 | 已投入、workspace 成型、~1460 测试在跑 | 作者熟 Go 但需重写 | 学习曲线 | 需重写 |
| 单人维护成本 | 沉没成本最低，clippy/Miri/fuzz CI 已就位 | 重写=巨大倒退 | 重写 | 重写 |

C 的体积标杆很有说服力（也说明 Rust 的代价）：据 Berry 官方 README（github.com/berry-lang/berry），"The Berry interpreter-core's code size is less than 40KiB and can run on less than 4KiB heap (on ARM Cortex M4 CPU, Thumb ISA and ARMCC compiler)… one-pass compiler and register-based VM, all the code is written in ANSI C99." Lua core 亦在 ~40KiB 量级。Rust 二进制会更大，故 no-std 体积目标设为"可用"（`bare` core 目标 <150KiB，最小可运行 heap <32KiB）而非"击败 Berry"。

**结论**：Rust 是唯一理性选择——同时满足可嵌入 + no-std + 内存安全 + 已有大量可复用资产。作者深谙 Go 且偏好 Dart/Flutter，但：重写为 Go 会因 TinyGo 的 GC/goroutine/reflection 限制而无法承载语义完整的 VM；重写为 C/Zig 会丢掉 Miri/借用检查这些保证双后端一致的工具。对"偏好 Flutter"的回应：Flutter 集成走 C ABI（`dart:ffi`）即可嵌入 lk 的 Rust 核心，与 Rust 决策不冲突且更干净。

**决策 B：VM 作为语义真源，AOT 必须匹配。** 先例：
- **Lua**：PUC-Rio 参考解释器（ANSI C，register VM）是标准参考实现，LuaJIT 是"实现参考 Lua 5.1 语义"的另一实现——解释器定义语义，JIT 追赶。
- **OCaml**：`ocamlc`（bytecode+`ocamlrun`）与 `ocamlopt`（native）**共享 parser/typechecker 前端**；学界做调试解释器时明确"必须保证解释器匹配 native 编译器语义……我们共享前端，故只需担心求值语义"（arXiv:1905.06545 / 2411.00637）。
- **Dart**：JIT 与 AOT"share the same Dart front-end and the same abstract representation（kernel IR），只在生成机器码时分道"；AOT pipeline 复用 JIT pipeline 部件。
- **V8**：Ignition 解释器 baseline，TurboFan 必须产生等价可观察行为；JS 引擎 JIT bug 的定义就是"解释器与 JIT 输出分歧"，用差分测试 oracle 检测（以解释器为基准）。

**为何选 VM 而非 AOT**：VM 完整覆盖全语言、易形式化推理、易在 REPL/no-std 运行；AOT 覆盖渐进（10/44）。**把二者关系从对称（现状"两条独立推导路径互校"）收紧为非对称"VM 是规范、AOT 是被验证方"——分歧时 VM 永远对（除非 VM 违反语言规范测试集）。**

**决策 C：全量重写允许，但十大问题必须被新设计逐一修复。**

### 三、总体架构

**分层与依赖铁律（严格单向，下层不依赖上层）**：

```
┌─────────────────────────────────────────────────────────┐
│  L6  cli / repl / fmt        (std, 二进制)                │
│  L6  lsp / tree-sitter       (std, 可选)                  │
├─────────────────────────────────────────────────────────┤
│  L5  embed-api (lk-api)  宿主嵌入：Rust API + C ABI       │
├─────────────────────────────────────────────────────────┤
│  L4  aot (lower→mir→codegen)  仅 std，AOT 工具链          │
├─────────────────────────────────────────────────────────┤
│  L3  stdlib-os (fs/net/process/time…) 经 HAL trait       │
│  L3  stdlib-core (string/math/list/map/iter/json) alloc  │
├─────────────────────────────────────────────────────────┤
│  L2  runtime-services (错误/traceback/协程/fuel/sandbox)  │
├─────────────────────────────────────────────────────────┤
│  L1  vm-core (lexer/parser/compiler/verifier/interp/GC)   │
│                          仅需 core + alloc                │
├─────────────────────────────────────────────────────────┤
│  L0  lk-hal (trait 定义) + lk-values (Value/GC 类型)      │
│                          仅需 core (+可选 alloc)          │
└─────────────────────────────────────────────────────────┘
```

**依赖铁律**：
- L0/L1 **禁止** `use std`，禁止 `once_cell`/`lazy_static`/`thread_local!` 等全局可变状态（直接修复问题 5、9）。现状 `lkrt` 用 `thread_local! RefCell` 假设单线程——新设计把所有运行时状态收进 `VmContext` 实例。
- 所有 OS 能力经 L0 `lk-hal` trait 注入，L1/L2 只见 trait 不见实现。
- AOT（L4）**不定义任何语义**；它消费 L1 字节码 + 复用 L2/L3 运行时函数，语义完全来自共享运行时（借鉴 Pallene "share the Lua runtime … same semantics" 与 mruby `mrbc`）。

**Crate 布局**（重整现有 workspace）：
```
crates/
  lk-values/    # L0 Value + GC 类型（no_std, alloc-opt）
  lk-hal/       # L0 平台抽象 trait（no_std）
  lk-vm-core/   # L1 词法/语法/编译/验证器/解释器/GC（no_std+alloc）
  lk-runtime/   # L2 错误/traceback/协程/fuel/内存上限（no_std+alloc）
  lk-std-core/  # L3 纯 stdlib（no_std+alloc）
  lk-std-os/    # L3 OS stdlib（std, 经 hal）
  lk-aot/       # L4 lower→mir→codegen（std）
  lk-api/       # L5 嵌入 Rust API + extern "C"
  lk-cli/       # L6 CLI/REPL/fmt
  lk-lsp/       # L6 完整 LSP（诊断/补全/hover/goto/semantic tokens/inlay hints）
tools/tree-sitter-lk/
```

**Feature-flag 矩阵**（借鉴 Rust `no-std-compat`、mlua feature 模型）：

| feature | 含义 |
|---|---|
| `std` | 启用 std、`lk-std-os`、线程、fs/net |
| `alloc` | 需全局 allocator（Vec/String/BTreeMap） |
| `float` | 启用 f64（MCU 可关只留 i64） |
| `coroutines` | 协程/`yield` |
| `unicode` | 完整 Unicode（否则 UTF-8 字节） |
| `aot` | AOT 工具链（隐含 std） |
| `ffi` | C ABI 导出 |

三档 profile：**`bare`**（no_std+alloc，WASM/MCU）、**`alloc`**（no_std+alloc+float+coroutines，完整语义无 OS）、**`full`**（std+aot+ffi+全 stdlib，桌面/服务器/CLI）。

### 四、核心设计

**4.1 Value 表示**——决策：tagged union 为默认，NaN-boxing 作为 `full` profile 下 x86-64/aarch64 的可选优化，二者行为等价。依据 Crafting Interpreters/SpiderMonkey/JSC 经验，NaN-boxing 依赖 CPU 浮点/指针低层细节"probably works on most CPUs but you can never be totally sure"，故必须保留 tagged-union 后备（尤其 no-std MCU）。做成编译期可切换（`cfg(feature="nanbox")`）。保留现有带 `Arc` 堆载荷的 tagged enum + 小字符串优化（SSO）+ 堆字符串驻留。变体：`Nil/Bool/Int(i64)/Float(f64)/Str/List/Map/Fn/Closure/Coroutine/Userdata/Error`。保留 Int/Float 分离语义。

**4.2 垃圾回收**——决策：采用 gc-arena 式"生成式生命周期 + Mutation-XOR-Collection"的安全 tracing GC。据 gc-arena README（github.com/kyren/gc-arena），其"collection algorithm is an incremental mark-and-sweep algorithm very similar to the one in PUC-Rio Lua, and is optimized primarily for low pause time … pointers held in arenas (spelled `Gc<'gc, T>`) are zero-cost raw pointers. They implement Copy and are pointer sized"，设计"borrows heavily from the incremental mark-and-sweep collector in Lua 5.4"；被 Ruffle（ActionScript VM）与 piccolo（纯 Rust Lua VM）采用。**no-std 兼容**：GC 只需 `alloc`（自定义 `GlobalAlloc`），不需 OS。**迁移建议**：现状已有手写 tracing GC，建议评估迁移到 gc-arena 以获得编译期 root 安全保证（消除"漏枚举 root"整类 bug）；若成本过高（作者曾因 Mutation-XOR-Collection 认知负担放弃 piccolo 数年），Plan B 是保留手写 GC，仅把类型收进 L0 + 用 `Collect` derive 宏保证 trace 正确性 + 保留 `LK_GC_STRESS` 压测。

**4.3 指令集**——决策：保留 register VM（现状即是），定长/近定长编码，`match` dispatch。register 优于 stack（Lua 5.0 是首个广泛使用的 register VM，一条 RTL 指令抵 3+ 条 stack 指令；Berry MCU 目标亦用 register VM）。Rust **无**稳定 computed-goto，也无稳定 guaranteed tail-call（`become` 仍 nightly/incomplete），现实标准是 `match`（编译为单跳转表）。v1.0 用 `match`；tail-call threaded dispatch 仅作 nightly `full` 可选实验——据 Matt Keeter《A tail-call interpreter in (nightly) Rust》（mattkeeter.com, 2026-04-05）："Overall speedups were significant: 40-50% faster on ARM64, and about 2× faster on x86-64. Unfortunately, it requires maintaining about 2000 lines of code, and is incredibly unsafe."（WASM 在 wasmtime 下反而慢 4.6×）——故不进主线。保留 call-base window + 操作数分类元数据 + load-time 验证器。

**4.4 错误模型（修复问题 4）**——决策：错误为一等值 + `pcall`/`try` 保护调用 + 结构化 traceback；宿主边界用 Rust `Result`。语言层引入 `pcall(f, args...) -> (ok, result_or_err)` 与 `error(value)`，错误可携带任意 lk 值（Lua："the error message does not have to be a string … `error({code=121})`"）。`try`/`?` 作为语法糖。traceback 在栈展开前采集（Lua `xpcall`+`debug.traceback` 语义——pcall 返回时销毁栈，故需返回前构建）。**宿主边界绝不做 longjmp 式跨 Rust 栈帧跳转**（rlua 曾因 `lua_error` longjmp 跨 Rust 栈帧构成 UB）；lk 用纯 Rust `Result<Value, LkError>` 在 VM 内传播，`Executor::step` 返回 `Result`。保留现有 fatal guard（div/0、缺键、assert）但使其**可被 `pcall` 捕获**——从"响亮失败直接 abort"升级为"抛可捕获错误值，未捕获才 abort/exit"。

**4.5 协程/并发**——决策：stackless（trampoline）协程 + 单 VM 单线程；多线程=多 VM 实例（isolate 模型）。采用 piccolo 式 stackless VM——据 luster/piccolo README，"VM executions and callbacks are constructed as `Sequence` state machines via combinators … The interpreter simply loops, calling `Sequence::step` … This 'stackless' style allows for some interesting concurrency patterns that are difficult or impossible to do using PUC-Rio Lua." 好处：(1) 协程/`yield` 天然支持；(2) 长运行不阻塞 GC（Mutation-XOR-Collection 要求周期性返回）；(3) fuel 可精确中断。并发仿 **Dart isolate**：每 VM 独占堆与状态，跨实例走消息/序列化，**彻底消除全局/线程局部可变状态**（修复问题 5）。所有状态收进 `VmContext`，`Send` 的实例可在线程间移动但不共享。

**4.6 字符串**——UTF-8 默认（`full`/`alloc`），`bare` 可退化字节串；驻留表随 VM 实例走（非全局）保证隔离。

**4.7 模块/import + 字节码策略（修复问题 3）**——决策：`.lkm` 仅作内部缓存，版本锁定 + 源哈希失效，明确非分发格式（类比 Python `.pyc`）。现状已有 `MODULE_ARTIFACT_VERSION`（v3→v6，旧版本干净拒绝）+ load-time 验证器（把 `.lkm` 当不可信输入校验寄存器越界/跳转目标/常量索引/call 窗口）。保留验证器，把 `.lkm` 降级为 `$LK_HOME/cache` 编译缓存，源哈希不匹配即重编译。CLI 不再宣传 `lk FILE.lkm` 作分发。import 沿用 `use pkg;`/`use "file";`（相对路径拒 `../`）/`use { x } from mod;`。

### 五、嵌入 API 设计（修复问题 10）

决策：Rust 一等 API（handle/rooting 保证 GC 安全）+ 可选 `extern "C"` ABI；多实例隔离；fuel/内存/模块白名单三重沙箱。

```rust
use lk_api::{Vm, Value, Sandbox, LkError};

// 多实例隔离：无全局状态，可并存
let mut vm = Vm::builder()
    .sandbox(Sandbox {
        fuel: Some(1_000_000),               // 指令预算（piccolo/wasmtime 模型）
        max_heap_bytes: Some(16 << 20),
        allow_modules: &["math", "string"],  // 白名单，默认禁 fs/net/process
    })
    .hal(MyHal)                              // 注入平台能力（no-std 也走这里）
    .build();

// 注册宿主原生函数（干净扩展，修复问题 10）
vm.register_fn("host_add", |_ctx, args: &[Value]| -> Result<Value, LkError> {
    Ok(Value::int(args[0].as_int()? + args[1].as_int()?))
})?;

// 注册原生模块
vm.register_module("mymod", |m| {
    m.function("greet", |_, _| Ok(Value::str("hi")));
    m.constant("VERSION", Value::int(1));
})?;

// 执行（stackless，返回 Rust Result；错误是安全值不是 longjmp）
let out: Value = vm.eval("return host_add(1, 2)")?;

// GC 安全：宿主持有的 Value 通过 rooted handle 管理
let rooted: Rooted<Value> = vm.root(out);
```

**沙箱**：fuel（`Executor::step(fuel)` 中断长运行，piccolo 模型）；内存上限（自定义 allocator 计账，超限抛可捕获 `LkError::OutOfMemory`——注意 Lua `LUA_ERRMEM` 不调用 error handler 的坑，内存错误路径需专门处理）；模块白名单（默认只给纯 stdlib-core，fs/net/process 需显式 allow，借鉴 Luau sandbox 移除 io/os）。**多实例**：每 `Vm` 独立堆/驻留表/注册表，无 `thread_local`（对照 goja/tengo/rhai/mlua）。**C ABI**（`ffi` feature）：`extern "C"`+cbindgen 生成 `lk.h`，供 C/C++/Dart FFI（Flutter）；handle 用不透明指针 + 显式 root/unroot，不跨 ABI 传 GC 裸指针。

### 六、AOT 路线（修复问题 1、2、6）

决策：分阶段，永远以 VM 为真源、永远可回退 VM、差分测试当门禁。

**Tier 0（v1.0 基线）：AOT = 预编译到同一字节码 + 嵌入 VM。** 类比 Lua `luac`/mruby `mrbc -B`（字节码 dump 成数组嵌进可执行文件）。语义**平凡一致**——跑的就是同一 VM 解释同一份字节码。产物：`lk compile` 生成"字节码 + 静态链接 lk-vm-core"单文件可执行程序，启动即跑。**这是修复问题 1、2 的关键第一步**：先保证有一个 100% 覆盖、语义必然一致的 AOT 形态。

**Tier 1（v1.x）：混合模式原生编译。** 现状 MIR 后端是"total capability predicate，子集外整程序 `Unsupported` 失败，无回退后端"——这正是问题 2。**修复**：把 `Unsupported` 从"整程序失败"改为"该函数/构造标记为 VM-executed，其余原生"。混合模式先例：LuaJIT NYI 回退解释器、Dart VM 分层。产物内嵌 VM，未覆盖构造运行时落到 VM，**永不整程序失败**。codegen 复用 `lkrt` 运行时函数（语义同源，Pallene 模型），差分测试校验每个 example"原生输出==VM 输出"。

**Tier 2（可选，v2+）：transpile-to-C 或 Cranelift。** 优先 **transpile 到 C**（mruby/Pallene/Nelua 路线）：可移植到任意有 C 编译器的平台（含 no-std MCU），复用 `lkrt`，语义一致性最强。**关于 Cranelift vs LLVM**：据 Perry 博客《From Cranelift to LLVM: How Perry Got 24x Faster》（perryts.com），"Cranelift is intentionally a fast, single-tier optimizing compiler. Its mandate is 'produce decent code quickly,' not 'produce the best possible code given unlimited time.' That's the right tradeoff for a JIT. It's the wrong tradeoff for an AOT compiler whose entire selling point is native performance"——该项目从 Cranelift 换 LLVM 得 24× 提速，因"compile rarely, execute always … is exactly the regime where LLVM's heavier optimizer pays for itself"（forcing function 是 Apple Watch arm64_32，Cranelift 不支持）。**建议**：solo 维护下 Tier 2 优先 transpile-to-C（可移植+简单），LLVM 保留作长期峰值性能选项，Cranelift 仅在需要快速 AOT 编译时考虑。这也**修复问题 6**——手写 LLVM IR 从"紧耦合的唯一后端"降为"可选 Tier"。

**差分测试门禁（强制）**：每次 AOT 改动 CI 跑"同程序→VM==AOT"，任何分歧阻断合并（V8/Dafny/Kotlin 编译器差分测试模型）。保留现有 3 套差分语料 + MIR 快照 + golden 向量 + ASan/UBSan/Miri。

### 七、no-std 支持路线（Rust 原生优势）

**7.1 HAL/port 层（`lk-hal`，L0，no_std）**——借鉴 MicroPython port 层、mruby platform abstraction、JerryScript port API、Lua `luaconf.h`：
```rust
pub trait Clock  { fn now_millis(&self) -> u64; }
pub trait Rng    { fn fill(&self, buf: &mut [u8]); }
pub trait Stdout { fn write(&self, s: &[u8]); }
pub trait FsProvider  { /* 可选，bare profile 可空实现 */ }
pub trait NetProvider { /* 可选 */ }
pub struct Hal<'a> {
    pub clock: &'a dyn Clock, pub rng: &'a dyn Rng, pub stdout: &'a dyn Stdout,
    pub fs: Option<&'a dyn FsProvider>, pub net: Option<&'a dyn NetProvider>,
}
```
VM/runtime 只见 trait；`full` 提供 std 实现，MCU 提供裸机实现。OS 能力全部可插拔，core 不含任何 OS 依赖（修复问题 9）。

**7.2 每 profile 的 stdlib 子集**：

| profile | 依赖 | stdlib | 目标 |
|---|---|---|---|
| `bare` | core+alloc | string/math/list/map/iter（纯计算） | WASM、Cortex-M(带 alloc)/ESP32 |
| `alloc` | core+alloc+float+coroutines | 上 + json/regex(可选) | 沙箱嵌入、边缘 |
| `full` | std | 上 + fs/net/process/time/os | 桌面/服务器/CLI |

**7.3 内存预算 & 示例目标**：标杆 Lua ≈40KiB / Berry <40KiB core、<4KiB heap（Cortex-M4）。lk `bare` core 目标 <150KiB（Rust 会略大），最小可运行 heap 目标 <32KiB。示例目标 1=**WASM**（现有 `wasm/` crate，编到 `wasm32-unknown-unknown`）；示例目标 2=**一类 MCU**（ESP32 或 Cortex-M+allocator，验证 `bare` + 自定义 `#[global_allocator]` + HAL）。受限堆可关 `no_global_oom_handling` 禁隐式 infallible 分配。

**7.4 no-std CI**：加 `cargo build -p lk-vm-core --no-default-features --features alloc,float` + `wasm32` target + `thumbv7em` 交叉编译冒烟，防 std 依赖回潜（Effective Rust 建议）。

### 八、测试与质量基建（修复问题 8）

1. **规范测试集（conformance suite）**：手写覆盖每个语言特性的 golden 测试（仿 Lua 官方套件、Wren tests、mruby spec）。通过 = VM 定义了语义。
2. **差分测试（核心门禁）**：`VM(program)==AOT(program)` 且 `VM(source)==VM(bytecode)`，以 VM 为 oracle。
3. **Fuzzing**：parser fuzzing（grammar-based）；字节码验证器 fuzzing（喂畸形 `.lkm`，验"干净拒绝、绝不 panic/UB"）；差分 fuzzing（同随机程序喂 VM 与 AOT，libAFL/cargo-fuzz）。
4. **属性测试**：cross-backend property（同程序同输出）+ GC 属性（`LK_GC_STRESS` 每次分配即收集暴露漏 root）。
5. **内存安全**：Miri（VM+lkrt）+ ASan/UBSan（AOT 原生产物）。现状在跑，保留。
6. **基准**：对标 gopher-lua/tengo/mruby/Lua/piccolo，用 `script-bench-rs` 式框架（覆盖求值 + Rust 互操作）。现状 VM/Lua≈1.033×、AOT/VM≈0.26×，保留防回归。
7. **CI 矩阵**：{stable,nightly}×{full,alloc,bare}×{x86-64,aarch64,wasm32,thumbv7em(build-only)} + Miri + fuzz(nightly) + clippy(0 warn) + 差分门禁。

### 九、工具链与生态（去中心化包管理保 solo，修复问题 7）

- **CLI**：`lk`（REPL）、`lk FILE`、`lk check`、`lk compile`（Tier 0/1）、`lk fmt`。
- **tree-sitter 语法与完整 LSP 双轨保留**：tree-sitter 提供快速、容错（含语法错误时产部分解析树）的高亮 + 结构查询，Zed/Neovim/Helix 开箱即用；完整 LSP 提供 macro-origin hover/goto-definition/semantic tokens/inlay hints 等语义能力。二者互补，均保留。
- **完整 LSP 继续维护，不冻结不降级**：现有 `lk-lsp`（诊断/补全/hover/goto-definition/符号/semantic tokens/inlay hints，且 macro-origin 感知）是已建成的资产。它与 `lk check` 共享编译器前端，增量维护成本可控——语言语义变更时前端一处修改即被 LSP、type-check、VM 共用，不构成额外的"重实现"负担。
- **最小包管理（修复问题 7）**：**砍掉签名注册表 + `.lkm` 分发 + HMAC/Ed25519 keyring + `lk pkg serve`**，代之以 **Deno/Go 式 git-based imports**：依赖=git URL+版本 tag，`Lk.lock` 锁 rev+sha256 校验（现状已有此能力）。依据：Deno 初期用 HTTP/git imports 无中心注册表，缺点（长 URL/版本漂移）用 import map+lockfile 缓解。签名若保留，仅在源码归档层做 sha256 完整性校验（对 solo 项目已够）。这把包管理从"注册表+签名+发布+yank+keyring 轮换"缩减为"git fetch + lockfile + 哈希校验"。

---

## Recommendations（分阶段里程碑与可执行建议）

### 里程碑 M0–M5（solo 排序，每个独立可发布）

**M0：地基收敛（无全局状态 + Value/GC 收进 core）**
- 抽出 `lk-values`（no_std Value+GC）、`lk-hal`（trait）；`lk-vm-core` 去 std、去 `once_cell`/`thread_local`；运行时状态收进 `VmContext`。
- *Exit*：`lk-vm-core` 在 `--no-default-features --features alloc` 编译通过；`wasm32` build 通过；grep 断言无全局可变状态。**修复问题 5、9 地基。**

**M1：VM 定为规范 + 规范测试集 + 差分框架**
- 编写 conformance suite（每特性 golden）；建 `VM(source)==VM(bytecode)` 差分；`.lkm` 降级为 cache（源哈希失效）。
- *Exit*：conformance 全绿并声明"通过即语义定义"；`.lkm` 不再作分发；差分框架进 CI。**修复问题 1、3、8。**

**M2：可恢复错误模型 + stackless 协程 + fuel 沙箱**
- 实现 `pcall`/`error`/`try` + 结构化 traceback；VM 改 stackless；fuel/内存上限/模块白名单。
- *Exit*：`pcall` 捕获所有可恢复错误（含 div/0、缺键），未捕获才 abort；fuzz 验证验证器无 panic；沙箱指标可配。**修复问题 4、5。**

**M3：嵌入 API + 多实例 + C ABI**
- `lk-api` Rust API（register_fn/module、rooted handle、多实例）；`ffi`+cbindgen `lk.h`。
- *Exit*：示例宿主并存 2 个隔离 VM；C ABI 冒烟（含 Dart FFI 示例）；无实例间可变共享。**修复问题 10。**

**M4：AOT Tier 0（字节码嵌入）+ Tier 1（混合模式）**
- Tier 0 `lk compile`（字节码+嵌 VM，100% 覆盖）；MIR 后端 `Unsupported` 改为**逐函数回退 VM**；差分门禁"AOT==VM"。
- *Exit*：任意 `.lk` 都能 `lk compile` 成功（Tier 0 保底）；混合模式覆盖从 10/44 提升且失败构造回退 VM 而非报错；差分全绿。**修复问题 2、6。**

**M5：no-std profile 落地 + 工具链收敛 + v1.0**
- `bare`/`alloc`/`full` 三 profile 打通；WASM + 一类 MCU 冒烟；`lk fmt`；tree-sitter 完善；完整 LSP 持续维护；包管理缩减为 git+lockfile 去中心化依赖。
- *Exit*：WASM demo 可跑；MCU（ESP32/Cortex-M+alloc）冒烟通过；CI 矩阵全绿。
- **v1.0 定义** = {VM 规范测试全过 + AOT Tier 0 全覆盖/Tier 1 混合模式 + pcall 错误模型 + 多实例嵌入 API + bare/alloc/full 三 profile + git-based 最小包管理}。**修复问题 7。**

### 十大问题 → 修复映射表

| # | 原问题 | 修复的里程碑 / 设计决策 |
|---|---|---|
| 1 | 双后端无单一语义真源 | M1：VM 定为规范参考实现；差分测试非对称门禁（决策 B） |
| 2 | AOT 全有或全无、覆盖部分 | M4：Tier 0 字节码嵌入 100% 覆盖 + Tier 1 逐函数回退 VM |
| 3 | `.lkm` 当分发格式 | M1：`.lkm` 降级为版本锁定 + 源哈希失效的内部 cache |
| 4 | 无可恢复错误模型 | M2：`pcall`/`error`/`try` + 结构化 traceback，错误为一等值 |
| 5 | 全局/线程局部状态并发不安全 | M0+M2：状态收进 `VmContext`；isolate 多实例模型 |
| 6 | 手写 LLVM IR 紧耦合 LLVM 版本 | M4：LLVM 降为可选 Tier 2；优先 transpile-to-C；codegen 复用共享运行时 |
| 7 | 范围过大 | M5：砍中心化签名注册表→git+lockfile 去中心化依赖；LSP 与 tree-sitter 双轨保留 |
| 8 | 测试弱、无跨后端差分 | M1+全程：conformance + 差分 + fuzz + Miri/ASan CI 矩阵 |
| 9 | 前端/运行时/stdlib 分离差 | M0：L0–L6 严格单向分层，依赖铁律 |
| 10 | 嵌入扩展难 | M3：`lk-api` register_fn/module + rooted handle + C ABI |

### 顶层执行建议（Bottom line）
1. **不要重写语言，重构架构**：Rust 决策已定，现有 register VM/GC/验证器/差分语料是宝贵资产。"推倒重写"=重整分层 + 补齐错误模型/隔离/混合 AOT + 砍范围，而非从零。
2. **先做 M0+M1**：确立 VM 唯一真源 + 消除全局状态 + 差分门禁，修复问题 1/5/8/9 地基，风险最低收益最高。
3. **AOT 先保证 Tier 0 平凡一致**，再谈混合模式与原生性能——修复"全有或全无"最稳路径。
4. **砍中心化，不砍能力**：中心化签名注册表包管理器是 solo 维护最大拖累，换成 git+lockfile 去中心化依赖；LSP 与 tree-sitter 均保留。

---

## Caveats（重要限定与不确定性）

- **本规划最大修正是"lk 是 Rust 不是 Go"**。原始需求关于 Go/TinyGo vs Rust/Zig/C 的整个权衡框架因此被重构为"确认继续用 Rust"。若读者手中确有一个 Go 版 lk（历史上 lollipopkit/lk 可能有过 Go 实现），则决策 A 的权衡表仍适用于"是否从 Go 迁移到 Rust"的问题，答案同样是"迁移到/保留 Rust"。
- **现状实现细节已就地核实（2026-07-03，源自当前工作区源码）**：(a) 运行时值**两套并存**——`LiteralVal`（legacy，仍活跃）与 `RuntimeVal`（新 VM-rewrite 目标：`Nil/Bool/Int/Float/ShortStr/Obj(HeapRef)` + `HeapStore`，注释明确 "New VM code should target these types first"），均在 `core/src/val/`；为 tagged union + `Arc` 堆载荷，**无 NaN-boxing**（印证 4.1 决策）。**这意味着 M0 抽 `lk-values` 时正踩在一场进行中的值模型迁移上，应与之合流而非另起。** (b) VM 执行走 `anyhow::Result<ExecResult>`，已有 `vm/exec/handler.rs` 的 `ErrorHandler`/`LanguageRaise` 错误抬升机制（M2 的 `pcall`/`error` 可在其上构建），无独立 `VmError`/`RuntimeError` 枚举。 (c) **`core` 确实含全局可变状态**（修复问题 5 的 M0 清单据此确定）：`once_cell::sync::Lazy` ×2（`expr/expr_impl.rs` 的 `DashMap` 缓存、`rt/runtime.rs` 的 tokio 异步运行时状态）+ `thread_local!` ×1 生产（`vm/alloc.rs` 的 `TLS_ARENA` region 分配器；`vm/analysis.rs` 的 metrics 为 `#[cfg(test)]` 不计）；`lkrt` 有 `thread_local! RefCell` ×2（`state.rs` 的 `RUNTIME`、`abi.rs` 的 `LAST_ERROR`）。core 无 `#![no_std]`、102 处 `use std`。
- **AOT 覆盖数字（10/44 examples）与性能数字（VM/Lua≈1.033×、AOT/VM≈0.26×）来自仓库 progress 文档**，是调研时点快照，会随开发变化。
- **性能声明均为设计目标或历史基准，非承诺**：Rust `match` dispatch 能否长期保持 ≈Lua 性能、`bare` profile 能否达 <150KiB/<32KiB heap，均需实测验证；tail-call/NaN-boxing 优化的收益因架构而异（如上引 Matt Keeter 数据在 WASM 上反而是负收益）。
- **gc-arena 迁移是建议非强制**：其 Mutation-XOR-Collection 模型认知负担高（gc-arena 作者本人曾因此放弃 piccolo 数年）；若评估后成本过高，保留现有手写 GC 是完全可接受的 Plan B。
- **no-std MCU 目标是"带 allocator 的 MCU"（ESP32/Cortex-M+alloc），非真正裸机 core-only**。真裸机无堆环境需要重写所有 stdlib 为零分配，超出 v1.0 范围。
- **砍中心化签名注册表是范围取舍判断**，非技术必然。若项目后续获得商业/多人协作需求，中心化注册表可按需恢复；本规划立场是"中心化签名注册表对当前 solo + 14-star 规模属过度工程，而去中心化 git+lockfile 依赖已足够"。**LSP 不在裁剪范围内**——它与编译器前端共享代码、增量成本可控，作为已建成资产保留并持续维护。