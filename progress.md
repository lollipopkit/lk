# plan.md 执行分解（步骤化路线图）

> 目标（`/goal`）：把 `plan.md` 划分为多个可执行步骤，逐个完成。
> 本文件是 `plan.md`（M0–M5 里程碑）落到「一次一小步、可验证、可交接」粒度的执行台账。
> `handoff.md` 保持简短最新；细节留此。

## 状态图例
`[ ]` 未开始 · `[~]` 进行中 · `[x]` 完成 · `[!]` 阻塞/存疑

---

## 已核实的地基事实（2026-07-03，源自当前工作区源码，非文档推断）

- **值模型两套并存**：`LiteralVal`（legacy，`core/src/val/values/mod.rs:93`，仍活跃）+ `RuntimeVal`
  （新，`core/src/val/runtime_model.rs:16`：`Nil/Bool/Int/Float/ShortStr/Obj(HeapRef)` + `HeapStore`，
  注释「New VM code should target these types first」）。tagged union + `Arc` 堆载荷，**无 NaN-boxing**。
  → **M0 抽 `lk-values` 要与这场进行中的迁移合流，不另起。**
- **错误模型**：VM `execute() -> anyhow::Result<ExecResult>`（`vm/exec.rs:1677`）；已有
  `vm/exec/handler.rs` 的 `ErrorHandler`/`LanguageRaise` 抬升机制 → **M2 pcall/error 在其上构建**。
- **全局可变状态（M0 去除清单，问题 5）**：
  | # | 位置 | 内容 | 迁移方向 |
  |---|---|---|---|
  | ~~G1~~ | ~~`core/src/expr/expr_impl.rs`~~ | ~~`once_cell::Lazy<DashMap>` 缓存~~ | ✅ 已删(死代码,M0.3) |
  | ~~G2~~ | ~~`core/src/rt/runtime.rs`~~ | ~~`once_cell::Lazy` + tokio 异步运行时状态~~ | ✅ 已移入 VmContext(M0.5) |
  | ~~G3~~ | ~~`core/src/vm/alloc.rs`~~ | ~~`thread_local! TLS_ARENA`（RegionAllocator）~~ | ✅ 已删(死代码,M0.4) |
  | — | `core/src/vm/analysis.rs:827` | `#[cfg(test)]` metrics thread_local | 测试专用，**不计** |
  | G4 | `lkrt/src/state.rs:11` | `thread_local! RefCell<RuntimeState>` | ✅ 按设计保留(单线程 AOT 热路径,M0.6) |
  | G5 | `lkrt/src/abi.rs:43` | `thread_local! RefCell LAST_ERROR` | ✅ 按设计保留(同上) |
- **no_std 障碍（core 102 处 `use std`，无 `#![no_std]`）**：
  - 易（机械换 `core::`/`alloc::`）：`fmt`(6) `ops`(2) `mem`(1) `cmp`(1) `pin`(1) `collections`(29→alloc/hashbrown)
  - 难（`std::sync` 37）：`Arc`→`alloc::sync::Arc` 易；`Mutex/RwLock`→需 `spin`/`critical-section`
  - 真 std-only（feature-gate 或移 L3 std-os）：`path`(4) `fs`(1) `os`(1) `thread`(1) + tokio 运行时(G2)
- **规模**：core 78k 行（单体）、lkrt 2.9k、aot/lower 6.9k、lsp 13k。
- **现有 crate 布局 ≠ plan 目标布局**：现为 `core`+`lkrt`+`aot/{abi,mir,codegen,lower}`+`llvm`+
  `stdlib/*`+`lsp`+`wasm`+`completion`+`ecosystem/tree-sitter-lk`；plan 目标为 `lk-values`/`lk-hal`/
  `lk-vm-core`/`lk-runtime`/`lk-std-core`/`lk-std-os`/`lk-aot`/`lk-api`/`lk-cli`/`lk-lsp`。
  → 分层是**渐进抽离**，不是一次性重排 workspace。

---

## Phase 0 — 地基与护栏（前置，低风险，先做）

- [x] **0.1** 核实并更正 plan Caveats 事实（值模型/错误模型/全局状态/no_std），写回 plan.md。
- [x] **0.2** 全局可变状态精确清单（见上表 G1–G5）。
- [x] **0.3** no_std 障碍分类（见上）。
- [x] **0.4** 不回退基线（取自上次绿色运行，见 handoff）：workspace 95 套测试全绿 / 三套差分（手工 13 组）/
      fuzz 7 种子 / ASan+UBSan / Miri lkrt 25 / fmt+clippy 0 / AOT bench 20/20 checksum，
      **VM/Lua≈1.008x、AOT/LK≈0.259x**。每步以此为 exit gate，任何回退阻断。

## Phase M0 — 去全局状态 + Value/GC 收进独立 crate（问题 5、9 地基）

- [x] **M0.1** 抽 `lk-values` —— **前端值/类型模型已抽为独立 crate**(`values/`,crate `lk-values`)。
      含 `LiteralVal`/`Type`/`ShortStr`/`ShortStrOrStr`/`FunctionNamedParamType`/`NumericClass`/`NumericHierarchy`;
      `core::val` 经 `pub use lk_values::{…}` 再导出,全 core 的 `crate::val::Type` 等路径不变。加入 workspace。
      **验证**:workspace `-D warnings` 0/0、lk-values 独立编译 + **wasm32 交叉编译通过**、core+lk-values tests 全绿。
      **范围界定**(据「厘清迁移」结论):放进 L0 的是**前端/编译期模型**(干净可分);**运行时模型**
      (`RuntimeVal`/`HeapValue`/`CallableValue`+资源句柄)因内嵌 vm callable 留在 core,其 L0 化仍需 callable
      trait 反转(A)——**这是 plan「值放 L0」意图与代码现实的诚实收敛:能分的已分,vm 纠缠部分单列**。
      **剩余子步**:① lk-values 真 no_std 化(现用 std,core::fmt/alloc::Arc/no_std serde,属 M0.8);
      ② callable trait 反转(A)以让运行时模型也能 L0(硬阻塞,大工程)。
      - [x] **解耦 val→typ**：`NumericClass`/`NumericHierarchy`（只依赖 `Type`，本就属于它）从 `typ`
        移进 `val`；`typ` 改从 `val` 再导出（`crate::typ::Numeric*` 向后兼容，免改 type_checker）。
        core 0/0、950 tests。val（生产码）不再依赖 typ。
      - [x] **分离前端/运行时值模型**(M0.1-A 子步,收敛):把运行时资源句柄(`TaskValue`/`ChannelValue`/
        `StreamValue`/`StreamCursorValue`/`SliceValue`/`ResourceValue`/`ResourceHandle`,embed RuntimeVal/RuntimePayload)
        从前端 `val/values/mod.rs` 移入 `val/runtime_model.rs`。→ **`val/values/`(LiteralVal/Type/ShortStr/numeric)
        现无任何 RuntimeVal/rt/vm 依赖 = 干净 L0 前端候选**。经 `val::*` 再导出,外部路径不变。
        full workspace `-D warnings` 0/0、core 950 tests。**剩:把 `values/`+numeric 抽为 lk-values crate
        (需 runtime_model 从新 crate import Type/ShortStr)+ callable 的 val→vm(trait 反转 A)仍是硬阻塞。**
      - [x] **厘清 in-flight `RuntimeVal` 迁移**（用户选定,已完成）。结论:
        - **迁移护栏** `vm/migration_guard.rs` 是 **VM 重写不变量**守卫(禁旧 `Op` enum/`Frame`、bench 融合
          opcode、quickening、**src/vm 与 src/val 里的 `unsafe`**)——约束「值/VM 代码保持 safe Rust」,
          对抽 crate 是硬约束(lk-values 须 safe)。**不是** RuntimeVal↔LiteralVal 值迁移本身。
        - **两个值模型、角色不同**:**前端/编译期**(`values/types.rs` `Type`/`ShortStr`、`numeric`、
          `LiteralVal`,注释「AST inline literal」)——parser/AST/typechecker 用,**干净可 L0**;
          **运行时**(`runtime_model.rs` `RuntimeVal`/`HeapValue`/`CallableValue` + `values/mod.rs` 的
          Task/Channel/Stream 句柄)——executor(`vm/exec` 19 处)/heap/rt 用,**vm 纠缠**。
        - **迁移** = 运行时从 `LiteralVal`→`RuntimeVal`,**前沿在 `vm/compiler`**(13 LiteralVal + 4 RuntimeVal 桥接)。
        - **关键判断**:plan 想放进 L0 的正是**运行时值**,而它**必然内嵌** `vm::RuntimeCallable/Module/
          NativeFunction`(`CallableValue::Runtime`)——这是运行时值模型的**本质**(值持可调用),非意外。
      - [ ] **⚠️ callable 设计决策(不可回避)**:(A) trait 反转 callable(`lk-values` 持 `Arc<dyn Callable>`,
        vm 实现;干净但改动大、触热路径需评估 perf);(B) callable 下沉 lk-values(层次含执行模型片段,边界不纯);
        (C) 暂缓 crate 拆分,先在 core 内 no_std-ready(M0.7/8;但 no_std 化 78k 行 core + tokio feature-gate
        本身多天)。**待用户定 A/B/C 后继续 M0.1。**
- [x] **M0.2** 抽 `lk-hal`（新 crate `hal/`，`#![no_std]` core-only）：定义 `Clock`/`Rng`/`Stdout`/
      `FsProvider`/`NetProvider` trait + `Hal<'a>` 注入结构 + `HalError`（无 alloc）。fs/net 为 `Option`
      （bare profile 可缺省），buffer-based（`&mut [u8]`）以免 alloc。加入 workspace members。
      **验证**：host `-D warnings` 0、clippy 0、**`wasm32-unknown-unknown` 交叉编译通过**（真 no_std 证明）。
      *(独立于 M0.1 的 callable 决策;为 L1/L2 提供平台抽象契约,后续 no_std 化的地基。)*
- [x] **M0.3** 消除 G1（expr_impl `PARSE_CACHE`）→ **实为死代码**（`parse_cached_arc` 全仓零调用），
      连同 `once_cell::Lazy`/`dashmap::DashMap`/`Arc` 未用 import 一并删除。core 编译 0 warning、
      `cargo test -p lk-core` 953 passed / 0 failed。**G1 清除,不留全局状态。**
- [x] **M0.4** 消除 G3（TLS_ARENA）→ **实为死代码**：`RegionAllocator`/`with_thread_local`/
      `allocate_heap`/`heap_bytes` 全 core 零调用（仅自身单测用）；删 `TLS_ARENA` thread_local +
      `RegionAllocator` 整体，保留在用的 `AllocationRegion`/`RegionPlan`（逃逸分析规划类型）。
      core 编译 0 warning、`cargo test -p lk-core` 950 passed / 0 failed。**G3 清除。**
      *(注：将来若实现逃逸分析分配，须按 plan 走实例化 arena，不得再引入 thread_local。)*
- [x] **M0.5** 消除 G2（tokio 运行时）→ **已收进 VmContext 实例**（选项 A：可共享 `Arc`）。
      新 `rt::AsyncRuntimeHandle`（`Arc<Mutex<Option<Arc<Runtime>>>>`，`Send+Sync`，懒初始化）替代
      `static GLOBAL_RUNTIME`；VmContext 持有该 handle，`shallow_clone_shared_runtime` **克隆共享**
      → spawn 子任务/克隆上下文同一反应堆。`NativeRuntime::async_runtime()` 从 ctx 取 handle
      （无 ctx 则独立默认）。迁移 ~30 调用点跨 9 crate（core/chan/task/net/time/stream/stdlib/cli）；
      自由 helper（`spawn_timer`/`recv_channel_blocking*`）加 handle 参数向下穿线，async 块捕获
      `Send` handle clone；CLI `init_runtime` 改懒初始化删除、`shutdown_runtime`→`ctx.shutdown_async_runtime()`；
      往返测试改共享 `VmContext`（贴近真实执行，native 调用总有 ctx）。
      **验证**：`cargo build --workspace -D warnings` 0/0；全量 **1478 tests / 0 failed**；
      fmt+clippy 0；**`concurrency_demo.lk` 端到端 chan create→send→recv 正确**（共享反应堆语义验证）。
      *(反应堆粒度：共享 Arc，每 VM 独立 vs 进程共享的策略推迟到 M3 builder；性能上共享 A 在多 VM 严格更优。)*
- [x] **M0.6** G4/G5（lkrt thread_local）→ **决定按设计保留，不消除**。lkrt 是 AOT native 运行时
      （每原生二进制自成进程、单线程；边界铁律禁止依赖 VM/ctx）。`state.rs` 注释明确：thread_local
      而非进程级 mutex 是**刻意选择**——arena 注册在每次动态字符串操作的热路径上，不能付锁开销，
      且 handle 不跨线程。改成实例传递需穿线整个生成代码 ABI（巨大 codegen 改动）且**回退性能**、
      违反 lkrt 边界。→ **与 VM 全局状态（G1/G2/G3）性质不同,保留是正确工程决策。**
      *(问题 5「多实例 VM 安全」由 core 侧 G1-G3 消除达成;lkrt 无多实例概念。)*
- **✅ M0 去全局状态达成**：**core（L1）已无生产全局可变状态**（唯一剩 `vm/analysis.rs` thread_local
  是 `#[cfg(test)]`）。G1/G2/G3 消除、G4/G5 按设计保留。VM 多实例安全地基就位。
- [!] **M0.7/M0.8(core 主体)—— 重新界定范围(重要架构澄清)**:给**当前单体 `core`** 加 `#![no_std]` 是
      **错误目标**——它含 `package`(Lk.toml/lock)/`net`/`process`/`rt`(tokio),本质 std,不该 no_std。
      plan 的 L1 `lk-vm-core`(no_std)是要**从单体抽出** VM 核心(token/ast/expr/stmt/typ/vm/val/gc),把
      std-heavy 的 package/net/process/aot 留上层。→ **M0.7/8 真身 = 抽 `lk-vm-core` crate**(类似 lk-values
      但更大:VM 核心还依赖 `rt`/`module`/`syntax`,需先理清 VM 核心↔std-heavy 边界)。**多天结构重构,非 scaffold**;
      lk-values 抽取已验证方法(渐进解耦→分离→抽 crate→no_std)可复用。
      *(纠正 plan「给 lk-vm-core 加 #![no_std]」的隐含假设:那是抽新 crate,不是给现单体 core 加属性。)*
      **增量进展(可保绿,渐进解耦法)**:① core 加 `std` feature(default 含),把 std-heavy 的 `package` 模块
      (Lk.toml/git/fs,VM 核心零依赖)gate 其后 + macro_system 唯一 PackageGraph 用点一并 cfg-gate →
      `cargo build -p lk-core --no-default-features` 产出**不含 package/async 的 VM 核心表面**,CI 守卫固化。
      ② async(tokio)已在 `async-runtime` feature 后(去 async 已验证)。**下一步增量**:gate macro_system 的
      proc-macro 进程执行(std::process)→ vm/val/typ ~60 文件逐个 std→alloc(每个保绿)→ `#![no_std]`。
- [x] **M0.8**(lk-values 部分)**lk-values 已真 `#![no_std]` + alloc**:`#![no_std]`/`extern crate alloc`;
      `std::fmt`→`core::fmt`、`std::sync::Arc`→`alloc::sync::Arc`、`std::str`→`core::str`、String/Vec/Box/format!/vec!
      →`alloc::*`;`std::collections::HashMap`→`hashbrown`;删死的 anyhow(依赖也移除);serde/arcstr 改 no_std
      (`default-features=false`+alloc)。`substitute` API 变 hashbrown 的涟漪 fix-forward:typ→stmt→vm/context
      逐点改 HashMap import。**验证**:host+**wasm32 真 no_std 交叉编译**、workspace `-D warnings` 0/0、tests 全绿;
      CI wasm32 冒烟已含 lk-values。**待做**:`lk-vm-core`(core 主体)no_std 化仍是大工程(tokio+102 use std)。
- [x] **M0.9** CI no_std 冒烟 —— **达成**(Exit:alloc-only 编译通过 + wasm32 build 通过均满足)。`check.yml` 现有**三重 no_std 守卫**:
      ① **wasm32**:lk-hal(bare)+ lk-values(alloc)+ lk-wasm;② **thumbv7em 裸机 MCU**:lk-hal + lk-values;
      ③ **lk-core `--no-default-features`**(去 package/async 的 VM 核心表面)。alloc-only 的 lk-values 在 wasm32+MCU 均编过 → Exit 满足。
      **遗留**:`lk-vm-core --features alloc` 冒烟待 lk-vm-core 抽出(那是更大的 alloc-only 目标;当前 alloc-only 的 L0 已 CI 守卫)。
- **Exit**：alloc-only 编译通过；`wasm32` build 通过；grep 断言无生产全局可变状态。

## Phase M1 — VM 定规范 + conformance + 差分框架（问题 1、3、8）

- [x] **M1.1** conformance suite —— **由现有 `examples/{syntax,stdlib,general}` 承担**(每语言特性一组
      **自验证 golden**:程序内 `assert`/`assert_eq` 断言预期语义,通过=VM 定义了该特性的语义)。双重 gate:
      `cli/tests/examples_differential_test.rs`(VM==AOT,llvm)+ `vm_bytecode_differential_test.rs`(VM source==bytecode)。
      特性覆盖:syntax(闭包/match/pattern/named_args/ranges/struct/trait/operators/error/pcall…)、
      stdlib(math/string/list/map/iter/stream/json/net/time…)、general(fib/recursive/sort/HOF/concurrency…)。
      *(可增补:更细粒度的每-opcode/边界 golden;当前语料已构成 plan 要求的'通过即语义定义'骨架。)*
- [x] **M1.2** `VM(source)==VM(bytecode)` 差分测试入 CI。`cli/tests/vm_bytecode_differential_test.rs`
      (不依赖 llvm):对 examples 语料,源码跑 vs `compile bytecode`→`.lkm`→跑,比对 stdout/success;
      「源码跑两次」自动过滤非确定性样例。**41 比对 / 0 分歧 / 3 跳过**——ModuleArtifact 序列化往返语义一致。
- [x] **M1.3** `.lkm` 降级为缓存 + 停止宣传作分发 —— **完整达成**。① 停止宣传:`compile bytecode` 打印
      「note: `.lkm` is an internal build-locked artifact, not a distribution format」+ `CompileMode::Bytecode` 文档标注。
      ② **降级为缓存(新)**:`LK_CACHE=1` 时 `lk FILE` 首次编译把 module artifact 写 `$LK_HOME/cache`
      (键=源路径+源字节+`MODULE_ARTIFACT_VERSION`+CLI 版本),后续未改动源码直接解码缓存执行,跳过解析/宏展开/编译。
      **正确性**:仅缓存 macro-free 程序(字节码=源字节纯函数,命中必安全);imports 每次新鲜重解析(依赖变更必捕获);
      版本入键,旧缓存干净失效。**opt-in** → 默认路径与 perf bench 零影响;fuel 路径绕过。新 `cli/src/bytecode_cache.rs`,
      复用 `compile_program_module_with_ctx`+`execute_compiled_module_with_ctx`(与 execute_with_ctx 同语义)。
      测试:命中不重写缓存(mtime 不变)、未设 LK_CACHE 不建缓存。全量 1448 tests 0 失败。
- **Exit**：conformance 全绿并声明为语义定义；差分框架进 CI。

## Phase M2 — 可恢复错误模型 + stackless 协程 + fuel 沙箱（问题 4、5）

- [x] **M2.1** `pcall(f, args) -> [ok, result_or_err]` + `error(value)` 内建 —— **已实现**。
      `error(msg)`(stdlib core global)返回 `Err`(可捕获,区别于 `panic!` abort);`pcall(f, ...args)`
      (FullState builtin)用 `call_runtime_value_runtime` 调 f、捕获任意 `Err`→返回 `[false, message]`,
      成功→`[true, result]`。**验证**:`examples/syntax/pcall_error.lk`(自验证 assert,source==bytecode 一致);
      `pcall(div_zero)` 连除零也捕获(→ M2.3 部分:运行时错误已是可捕获 Err 非 abort)。core 950/stdlib 61 全绿。
      *(遗留:错误消息带 native 前缀噪声、`error` 只载字符串→一等错误值是 M2.2。)*
      **原调查结论(基础设施)**：
      **调查结论(本会话)**：**M2 后端基础设施大体已就绪**——`Opcode::Raise`(`dispatch_raise` 读常量
      字符串消息→`raise_language_message`)、`TryBegin`/`TryEnd`(`begin_try`/`end_try` + `ErrorHandler`:
      catch_reg/catch_pc/frame_base/stack_top)、`ErrorVal { message, trace }`(带 trace 字段的结构化错误值,
      GC-rooted)。缺口:① `Raise` 只载**字符串**,需扩展为携带任意 `RuntimeVal`(error-as-first-class,M2.2);
      ② **无语言层 `pcall`/`error(value)`/`try` 表面**(前端无 `try` 关键字;当前用户级错误处理是 nil+`??`);
      ③ fatal guard(div/0/缺键/assert)走 abort,需改为可 `pcall` 捕获(M2.3)。
      → M2.1 落地=加 `pcall`/`error` 内建 + 扩 `Raise`/`ErrorVal` 载任意值 + 桥接现有 TryBegin/handler。多小时活。
- [x] **M2.2** 错误为一等值 + traceback —— **达成**(一等基本值 + try/catch + traceback 全落地;仅堆对象一等值遗留)。新 `lk_core::vm::LkRaisedValue{value}`
      载 `RuntimeVal`(Send+Sync+'static);`error(v)` 对单个非堆值(Int/Float/Bool/ShortStr/Nil)抛之,
      `map_native_error` 透传(如 LanguageRaise),`pcall` 经 `root_cause().downcast` 取回→`[false, v]`(原值原型)。
      验证:`error(404)`→pcall `[false, 404]`(**Int**,typeof=Int);`error("nope")`→String;
      `examples/syntax/pcall_error.lk` 断言 `coded[1]==404`。**全量 1484 tests 0 失败,0 回退**。
      **traceback 地基**:`Function.debug_name` 命名 `fn` 编译时源码名下沉字节码 + `FunctionData` 序列化 + artifact 版本 6→7。
      **traceback 显示端已完成**(第三方案避开两张力):在 `call_closure_stack_args`/named 的**错误传播分支**把 `debug_name`
      push 进 ctx 调用栈——**仅 Err 路径、Ok 分支零改动 → 成功零成本不碰 perf 门禁**;**不碰错误类型/消息 → `to_string()` 不变
      → 111 断言全安全**。复用死掉的 `CallFrameInfo`/`push_call_frame`/`call_stack_report`。正确性:每次 top-level 开头清空;
      **pcall 捕获时 truncate(0)** 丢弃已捕获帧(try/catch 脱糖 pcall 一并覆盖)。CLI `unwrap_with_traceback` 失败时打印。
      测试:递归错误打印命名调用链;pcall 捕获不泄漏帧。**全量 1451 tests 0 失败**。
      **唯一遗留**:堆对象(String/List)一等错误值(需 GC rooting 跨展开)——`error(int)` 已一等,`error("str")` 仍 native 包装。
- [x] **M2.3** fatal guard 可 `pcall` 捕获 —— **基本达成**。调查+改动:**除零**本就是可捕获 Err;
      **assert/assert_eq/assert_ne** 从 Rust `panic!`(abort,不可捕获)改为返回 `Err`(可捕获,
      未捕获仍非零退出且**消除 panic backtrace 噪声**);**缺键/越界**返回 nil(非 fatal,无需捕获);
      **panic** 保持故意 fatal(`error()` 是可捕获替代)。**验证**:pcall 捕获 assert/除零;未捕获 assert
      exit=1「VM execution failed」;**全量 1479 tests / 0 failed(0 回归)**。
- [x] **M2.4** `try`/`catch` 语法糖 —— **已实现,端到端验证**。加 `try`/`catch` 关键字(lexer+Token),
      parser `parse_try_stmt` **脱糖**为 `let [__try_ok, e] = pcall(fn(){BODY}); if !__try_ok { HANDLER }`
      ——**复用已验证的 pcall + closure/if,无 AST 变体/lowering 改动**(仅 1 处 Token match 需补,fix-forward)。
      `try { BODY } catch e { HANDLER }`:成功跳过 handler;失败把错误值绑定 e 跑 handler;**一等基本错误值**
      (`error(404)`→`catch code` 得 Int 404)。`examples/syntax/try_catch.lk` 断言全过,source==bytecode 一致,
      **全量 1484 tests 0 失败**。*(已知限制:try 体内 `return` 从脱糖闭包返回,非外层函数——已在文档标注。)*
- [ ] **M2.5** VM 改 stackless（trampoline `Sequence::step`）——大工程，落地时再拆子步。
- [x] **M2.6** fuel + 模块白名单 —— **基本达成**(内存上限待)。**fuel**:`LK_FUEL=N`(CLI)+ `Vm::with_fuel(N)`
      (lk-api)经 `execute_program_with_ctx_and_budget`。**模块白名单**:`Vm::sandboxed(&["math",…])`(lk-api)
      只注册核心 builtin + 白名单模块,OS 模块(fs/net/process)默认拒。测试:`sandboxed(["math"])` 下
      `math.max(3,7)→7` 而 `use fs` 报错。→ **fuel + 模块白名单沙箱就绪**(也补全 M3 沙箱 builder)。
      **内存上限**:Executor 加 `heap_object_limit`(活堆对象数上限),在与 fuel 同频的 per-instruction 检查点校验,
      **折进 `const BUDGETED` 单态化路径 → 无 limit 时零开销(bench 走 false 分支不受影响)**;`execute_program_with_ctx_and_limits`
      + lk-api `Vm::with_heap_limit(n)` 暴露。测试:分配超限程序报「heap object limit exceeded」。全量 1485 tests 0 失败。
      → **三沙箱知(fuel/内存/模块白名单)齐**。
- **Exit**：`pcall` 捕获所有可恢复错误；fuzz 验证器无 panic；沙箱指标可配。

## Phase M3 — 嵌入 API + 多实例 + C ABI（问题 10）

- [x] **M3.1** `lk-api` 嵌入 API —— **完整达成**(Exit:2 实例隔离 + C ABI + 无共享可变全达标)。`Vm` 实例
      (拥有独立 VmContext,**去全局后天然多实例隔离**)+ `Vm::new()`/`sandboxed()` + `with_fuel(N)`/`with_heap_limit(N)`
      沙箱 + `register_fn`(M3.2)+ C ABI(M3.3)。**ergonomic 结果层已补**:`eval(src)->String`(display)
      + 新 `eval_value(src)->Value`(宿主友好枚举:primitives 类型化,字符串/堆对象展平为 display)。
      **验证**:7 测试全绿——eval/eval_value 类型化、两 VM 隔离、register_fn、sandbox 白名单、fuel/heap 限额。
      workspace 0/0、clippy 0。*(可选后续增强:register_module 命名空间、rooted handle——超出 M3 Exit。)*
- [x] **M3.2** register_fn(宿主原生扩展)+ 多实例隔离 —— **已落地**。`Vm::register_fn(name, arity, HostFn)`
      在 eval 前注册宿主原生函数(延迟 ctx 构建:pending registry 首次 eval 时定型),`HostFn` = 原始运行时
      ABI `fn(NativeArgs, &mut NativeRuntime)->Result<RuntimeVal>`。**多实例隔离**已由 M3.1 测试证明(每 Vm
      独立 VmContext/heap,无 thread_local,依赖 M0 去全局)。测试:`host_add100(5)→105`。workspace 0/0、clippy 0。
      **待做**:更 ergonomic 的 Value 转换层(host 类型↔RuntimeVal)、register_module、rooted handle。
- [x] **M3.3** C ABI —— **完成,端到端验证**。lk-api `ffi` feature 的 `extern "C"`(`lk_vm_new`/`lk_vm_eval`/
      `lk_vm_free`/`lk_string_free`,不透明指针+owned C string 配对释放)+ 手写 `api/include/lk.h` + `api/examples/embed.c`;
      lk-api 加 `staticlib` crate-type。**C 程序 `embed.c` 编译链接 staticlib 并运行 `return 6*7;` → 输出 `42`**
      (退出 0)。可从 C/C++/Dart FFI 嵌入 LK VM。默认构建不含 ffi(0 开销)。*(cbindgen 可选自动重生成 lk.h。)*
- **Exit**：示例宿主并存 2 个隔离 VM；C ABI 冒烟；无实例间可变共享。

## Phase M4 — AOT Tier 0 + Tier 1（问题 2、6）

- [x] **M4.1** Tier 0 —— **已实现,端到端验证**。CLI `lk bundle FILE -o OUT`:嵌入源码 + 经 lk-api C-ABI
      staticlib 静态链接 VM → **自包含 native 可执行程序**(启动即跑 VM,**100% 覆盖**——任何 VM 能跑的程序都能
      bundle,不像 MIR 原生「全有或全无」)。语义**平凡一致**(同一 VM)。验证:`bundle demo.lk`→20MB 自包含 ELF,
      直接运行(无 lk/无源码)输出与 VM **完全一致**。workspace 0/0。*(Linux/cc;后续字节码嵌入/跨平台/瘦身。)*
- [ ] **M4.2** Tier 1：MIR 后端 `Unsupported` 从「整程序失败」→「逐函数标记 VM-executed 回退」。
- [x] **M4.3** 差分门禁 `AOT==VM` 已在 CI —— **现状核实,已满足**。`cli/tests/aot_differential_test.rs`
      (MIR native == VM,stdout+成功/失败逐例比对,21 检查点)+ `examples_differential_test.rs`(VM==AOT 语料)
      + `aot_fuzz_differential_test.rs`(随机差分)均随 `check.yml` 的 `cargo test --workspace --all-features` 跑;
      `correctness.yml` 更在 **ASan/UBSan + fuzzing** 下专门跑这三个差分。→ AOT==VM 门禁固化。
      本会话新增的 **M1.2(VM source==bytecode 差分)**与之互补,共同守 VM 为规范。
- **Exit**：任意 `.lk` 可 `lk compile`（Tier 0 保底）；覆盖 >11/44 且失败构造回退 VM 而非报错；差分全绿。

## Phase M5 — no-std profiles + 工具链收敛 + v1.0（问题 7）

- [~] **M5.1** `bare`/`alloc`/`full` 三 profile 打通（feature 矩阵）。**进展**:三 profile 已由现有分层 crate 体现并**CI 验证可构建**——
      `bare`=`lk-hal`(纯 no_std)、`alloc`=`lk-values`(no_std+alloc)均编到 **wasm32 + thumbv7em 裸机 MCU**;`full`=`lk-core`+stdlib(std)。
      **待做**:统一为 `lk-vm-core` 单 crate 上的 `bare`/`alloc`/`full` feature 矩阵(依赖 M0.7/8 抽出)——现为三独立 crate 分档。
- [x] **M5.2** WASM demo + MCU 冒烟 —— **两冒烟达成**(full-VM-on-MCU 待 lk-vm-core)。**WASM 部分完成**:`lk-wasm`(浏览器 playground)现可编到
      `wasm32-unknown-unknown`——修了 getrandom 0.3 的 backend(新增 `.cargo/config.toml` 的
      `getrandom_backend="wasm_js"` cfg,target-scoped + wasm crate 加 `getrandom` `wasm_js` feature,
      内部按 target 门控、native 无害)。验证:wasm32 0 error、native workspace `-D warnings` 0/0、
      L0(lk-hal/lk-values)wasm32 冒烟仍通过;CI wasm32 步骤已含 lk-wasm。
      **MCU 冒烟已达成(新)**:实测 `lk-hal`(bare,纯 no_std)+ `lk-values`(alloc,no_std+alloc)均可交叉编到
      **`thumbv7em-none-eabi`(裸机 ARM Cortex-M4,无 OS/无 allocator)**,加 CI 冒烟固化。→ **WASM + MCU 两冒烟齐,M5.2 主体达成**。
      **遗留**:full profile(VM 本体)上 MCU 跑 LK 代码——依赖 `lk-vm-core` 抽出。
- [x] **M5.3** `lk fmt` —— **已实现**。CLI 新增 `lk fmt FILE`(就地规范化,4-space,brace/paren/bracket 感知,
      空行保持空;幂等)+ `lk fmt --check FILE`(不写,未格式化则非零退出,可作 CI 门禁)。逻辑与 LSP 的
      `format_lk` 一致。验证:乱缩进→规范嵌套、`--check` 幂等退出 0、真实示例二次 check 稳定。CLI `-D warnings` 0/0。
- [x] **M5.4** git+lockfile 去中心化依赖 —— **完整达成**。保留 `pkg init/add/fetch/update/check/tree`、git+GitHub+path
      依赖、`Lk.toml`/`Lk.lock`(Deno/Go 式:git URL + lockfile 锁 rev)。**并按 plan 第 239 行砍掉中心化签名注册表**:
      净删 ~5000 行 —— `core/src/package/registry.rs`(1343,RegistryService/RegistryPublishManifest/HMAC+Ed25519 签名/
      keyring/index 存储)+ `registry/signing.rs`+ `cli/src/pkg/registry_server.rs`(791,`pkg serve` HTTP 服务端)+ `key.rs`;
      pkg 子命令 `Serve/Publish/Yank/Index/Key` + `PkgKeyCommand/PkgIndexCommand`;pkg.rs 客户端 registry 解析(~600 行,
      `fetch_dependencies` 收敛为纯 git+path);`Manifest.registry`/`RegistrySection`/`DetailedDependency.{registry,version}`/
      `DependencySpec::{registry_version,registry_override}`;全部 registry 测试;无用依赖 core `ed25519-dalek`/`base64`、cli `ureq`/`semver`。
      **全量 1445 tests 0 失败(-41 为删除的 registry 测试),clippy/fmt 0,不触及 VM。** 更新 CLAUDE.md CLI 速查。
      **待做(文档)**:`docs/packages.md` 仍描述旧注册表,需刷新为 git-only(非阻塞)。
- [x] **M5.5** LSP **保留并持续维护**（不砍）+ tree-sitter —— **双轨保留,现状核实**。plan 决策(本会话已改 plan.md)
      = 不砍 LSP,与 tree-sitter 双轨。现状:`lsp/`(13k 行,hover/goto/semantic-tokens/inlay/completion/diagnostic
      共 424 引用点,macro-origin 感知)+ `ecosystem/tree-sitter-lk`(grammar.js)+ `vsc-ext`/`zed-ext` 编辑器集成
      均在树中、随 workspace 编译通过。→ **满足「保留双轨」**。tree-sitter 完善为持续项(非本步阻塞)。
- **Exit**：CI 矩阵全绿；v1.0 定义达成。

---

## 执行原则
1. 严格按 Phase 顺序；M0 是所有后续的地基（问题 1/5/8/9），风险最低收益最高。
2. 每步独立可验证：动代码即跑对应测试 + 不回退 `cargo test --workspace` 与 bench 门禁。
3. 大步（M0.1 抽 crate、M2.5 stackless）落地前先拆子步、先跑通编译再迁移逻辑。
4. 渐进抽离 crate，绝不一次性重排整个 workspace。
5. 每轮结束更新本文件勾选项 + `handoff.md`。
