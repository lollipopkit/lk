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
      - [x] **callable 设计决策 —— 数据驱动裁决:选 (C) 且 (A) 不做**(2026-07-04)。事实依据:M0.7/8 已把
        **单体 lk-core 整个 no_std 化,且 M5.2 后 VM 核心全量编译过裸机 thumbv7em**——「抽 lk-vm-core/运行时值
        L0 化」的原始动机(no_std 可用性)已由单体路线完全满足,trait 反转只剩分层纯洁性收益,而成本是
        热路径 dyn 分派(bench 门禁风险)+ 枚举变体原子重构(全 match 断裂)+ GC 追踪改造。**收益已无实际
        消费者 → 不做,留档;若未来出现真实的 L0 运行时值消费场景再重估。**
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
- [x] **M0.7/M0.8(core 主体)—— ✅ 完成:lk-core VM 核心 `#![no_std]`**(commit `2ec839a`)。
      `cargo build -p lk-core --no-default-features` 现为**真 no_std 构建**(0 error 0 warning):无 std feature 时
      lk-core `#![no_std]`(+alloc),VM 核心(token/ast/expr/stmt/typ/val/vm/gc/resolve + 声明宏展开)不依赖 std。
      **关键澄清**:host 上 no_std 只禁 lk-core **自身源码**用 `std::`,其 std 依赖(anyhow/dashmap/serde_json)仍
      链接 std → flip **无需**改依赖 Cargo 配置,也无需抽新 crate(渐进 gate 优于 big-bang 移动)。
      std-only 叶子按 `std` feature gate(no_std 上语义正确不可用):`stmt::import` 文件/包解析(保留 registry 内存解析)、
      macro_system 文件加载宏(保留 builtin+声明宏)、proc_deps 指纹、procedural/proc_function/derive 外部 proc-macro
      进程执行(3 个 external 叶子在 provider 检查后 gate,no_std 返回「requires std」)、ResourceValue.handle 走 compat Mutex。
      dead-under-no_std 的进程/fs 机器用 `#[cfg_attr(not(std), allow(dead_code, unused_imports))]` 于 mod 声明消警告。
      验证:default std + **1451 tests** / clippy / fmt 全绿;no_std 构建 0/0。CI check.yml 守卫升级为真 no_std 检查。
      *(历史范围澄清见下,保留供参考)*
- [~] **M0.7/M0.8(历史范围澄清)**:给**当前单体 `core`** 加 `#![no_std]` 是
      **错误目标**——它含 `package`(Lk.toml/lock)/`net`/`process`/`rt`(tokio),本质 std,不该 no_std。
      plan 的 L1 `lk-vm-core`(no_std)是要**从单体抽出** VM 核心(token/ast/expr/stmt/typ/vm/val/gc),把
      std-heavy 的 package/net/process/aot 留上层。→ **M0.7/8 真身 = 抽 `lk-vm-core` crate**(类似 lk-values
      但更大:VM 核心还依赖 `rt`/`module`/`syntax`,需先理清 VM 核心↔std-heavy 边界)。**多天结构重构,非 scaffold**;
      lk-values 抽取已验证方法(渐进解耦→分离→抽 crate→no_std)可复用。
      *(纠正 plan「给 lk-vm-core 加 #![no_std]」的隐含假设:那是抽新 crate,不是给现单体 core 加属性。)*
      **增量进展(可保绿,渐进解耦法)**:① core 加 `std` feature(default 含),把 std-heavy 的 `package` 模块
      (Lk.toml/git/fs,VM 核心零依赖)gate 其后 + macro_system 唯一 PackageGraph 用点一并 cfg-gate →
      `cargo build -p lk-core --no-default-features` 产出**不含 package/async 的 VM 核心表面**,CI 守卫固化。
      ② async(tokio)已在 `async-runtime` feature 后(去 async 已验证)。
      ③ **(本轮,大进展)VM 核心 no_std 就绪地基落地**:新 `core/src/compat.rs` 兼容层
      (collections:std HashMap / no_std hashbrown;sync::Mutex:保留 `.lock()->Result` 形状,no_std 走 spin;
      path:no_std 下 `PathBuf=String`;prelude:no_std 补 Vec/String/Box/format!/vec!)。**140+ 文件机械转换**:
      `std::{mem,fmt,cmp,error,...}`→`core::`、`std::sync::Arc`→`alloc::sync::Arc`、
      `std::collections::{BTree*,VecDeque}`→`alloc::`、HashMap/HashSet→compat、RuntimeModuleState 的 Mutex→compat;
      每个用 Vec/String 的 VM-core 文件加 `#[cfg(not(std))] use compat::prelude::*`。按 std feature gate 掉
      no_std 无意义叶子(ResourceHandle 的 File/Tcp/Udp 变体、gc_stress env)。**no_std 错误从 2481 降到 ~20**
      (全在 std-only 叶子:`stmt::import` 文件导入 resolver + macro_system 文件/proc-macro 函数)。
      ~~**`#![no_std]` flip 暂缓**~~ → **flip 已在后续 commit `2ec839a` 完成(见上方 [x] M0.7/8)。**
- [x] **M0.8**(lk-values 部分)**lk-values 已真 `#![no_std]` + alloc**:`#![no_std]`/`extern crate alloc`;
      `std::fmt`→`core::fmt`、`std::sync::Arc`→`alloc::sync::Arc`、`std::str`→`core::str`、String/Vec/Box/format!/vec!
      →`alloc::*`;`std::collections::HashMap`→`hashbrown`;删死的 anyhow(依赖也移除);serde/arcstr 改 no_std
      (`default-features=false`+alloc)。`substitute` API 变 hashbrown 的涟漪 fix-forward:typ→stmt→vm/context
      逐点改 HashMap import。**验证**:host+**wasm32 真 no_std 交叉编译**、workspace `-D warnings` 0/0、tests 全绿;
      CI wasm32 冒烟已含 lk-values。~~**待做**:lk-vm-core no_std~~ → **已达成**:lk-core 主体现 `#![no_std]`(见 M0.7/8)。
- [x] **M0.9** CI no_std 冒烟 —— **达成**(Exit:alloc-only 编译通过 + wasm32 build 通过均满足)。`check.yml` 现有**三重 no_std 守卫**:
      ① **wasm32**:lk-hal(bare)+ lk-values(alloc)+ lk-wasm;② **thumbv7em 裸机 MCU**:lk-hal + lk-values;
      ③ **lk-core `--no-default-features`** —— **现为真 no_std 构建**(lk-core 主体 `#![no_std]`,commit `2ec839a`),
      不再只是「去 package/async 的表面」,而是完整 VM 核心的 alloc-only no_std 编译验证。→ **M0.9 遗留清除**。
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
      **堆对象一等值(本轮收尾,遗留清除)**:`error(v)` 现对**任意单值(含 String/List 等堆对象)**一等携带,
      pcall 原样取回,不再对堆值做 native 字符串包装。关键 GC 安全:展开路径上 `collect_direct_native_garbage_
      after_result` 的 native-call safepoint 在错误路径也会 collect → 堆错误值须 pin 为 GC root 才不被回收。
      新 `RuntimeModuleState.pending_raise_root`(纳入 `gc_roots` 基础 roots),error() 置位 / pcall 取回后清除;
      `LkRaisedValue` 加 `rendered`(raise 时捕获 display,供 uncaught 展开后堆已消失时出消息,不再 `<error value>`)。
      stdlib io/net crate 启用 `lk-core/std`(构造 ResourceHandle 的 OS 变体,按 lk-core std feature gate)。
      验证:pcall_error.lk 增 String/List 原样取回断言;全量 **1451** / **GC-stress(LK_GC_STRESS=1)1095** /
      clippy / fmt / no_std 构建全绿;uncaught 堆错误正确出消息(long string、`[1,2,3]`)。→ **M2.2 完成,无遗留。**
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
- [x] **M2.5** VM 改 stackless —— **四子步全部完成**(设计 `docs/vm-stackless.md`,实测绘 exec.rs/call.rs/handler.rs)。
      **✅ 子步④ 提前落地**(commit `238324f`):stacker 分段栈(红区 128KiB/段 2MiB)+ 可捕获深度上限(默认
      10 万,`LK_MAX_CALL_DEPTH`)+ traceback 深栈截断;30k 递归过测试线程、20 万层过 env 提额;
      bench 1.012x vs 基线 1.008x(噪声级)。
      **✅ 子步①**(commit `5884829`):新 `CallFrame` struct(`exec/frame.rs`)+ `Executor.frames: Vec<CallFrame>`
      —— `CallDirect` 与命中闭包目标的泛型 `Call` 不再 `self.run_function_inner` 递归,改为
      `push_call_frame`(存 caller 的 function_index/pc/frame_base/register_count/captures/handler_depth/
      window)后原地 `continue`("trampoline":`run_function_inner_impl` 外层 loop + `dispatch_within_frame`
      内层循环,`FrameOutcome::{Switch,Done}` 传递控制)。`Return*` 经 `finish_return` 按
      `frames.len()==base_frame_depth` 判定 pop 回调用点或真正返回。错误路径新增 `unwind_flat_run`:逐帧
      pop 补 `push_traceback_frame`(对任意错误,多级 traceback 命名靠此),仅 `LanguageRaise` 且
      immediate-caller 有 `try` 时经 `handler_stack` 恢复(与旧递归"仅一次机会"语义等价——**关键实测澄清**:
      `TryBegin`/`handler_stack`/`LanguageRaise` 对真实 `.lk` 程序是死代码,`try/catch` 在 parse 期就糖化成
      `pcall(closure)`,只有手写字节码单测用到 TryBegin,故展开逻辑不必支持真正的多帧 handler 搜索)。
      GC `root_refs` 补 `frames` 各级 captures root(比旧递归更保守正确:旧实现祖先帧 captures 只活在 Rust
      局部变量里未显式 root)。`CallFrame` 命名(非 `Frame`)避开 `migration_guard.rs` 的 `"struct Frame"` 禁用
      token。bench geomean **0.989x**(不劣于基线,示例还更快——省了 flattened 路径每次调用的
      `stacker::maybe_grow` 检查)。
      **✅ 子步②**(commit `4e86dd5`):`CallNamed` 同款改造(`push_call_frame_named` 复用
      `move_named_args_to_frame_from_stack`,`CallFrame` 加 `named_count` 字段供 pop 时对齐 named k/v 临时
      寄存器清理)。**实测澄清 design doc 原假设**:`CallMethodK` 查明并不走 `call_closure_stack_args` 这条
      同 Executor 递归路径——`core_call_method_windowed` 命中可调用属性/trait 方法/list HOF 时,走
      `call_runtime_value_runtime_list_args` 系,每次调用 new 一个临时 `Executor`(runtime_callable.rs 的
      `call_closure_value`/`_typed_map`,状态整体 move 进出),本就是 native re-entry 形态,天然在决策①
      "native re-entry 保持递归"范围内,无需改造。bench geomean 0.997x。
      **✅ 子步③**(本轮):`call_closure_stack_args`/`call_closure_named_stack_args` 在①②实现时已直接删除
      (非只标记死代码);唯一剩余工作是文档准确性——更新 `max_call_depth`/`grow_stack_if_needed` 的doc注释
      (深度 guard 经 `enter_lk_call`/`exit_lk_call` 在 push/pop 处调用,子步④机制不改代码即自动覆盖
      `frames.len()`,无需新写)+ `docs/vm-stackless.md` 补"Implementation notes"记录与原设计的偏差
      (`CallFrame` 存整个 `CallWindow` 而非仅 `ret_dst`、单跳 catch 语义等)。
      **验证(每子步)**:workspace 全量测试 + `LK_GC_STRESS=1` 全绿(951 lk-core 测试,0 回归)+
      `traceback_test` 两多级用例(`uncaught_error_prints_named_call_stack`/
      `pcall_caught_error_leaves_no_stale_frames`)+ clippy/fmt 0 + dist bench 门禁(①0.989x/②0.997x,
      均不劣于 1.008-1.033x 历史基线)。→ **M2.5 stackless 完整达成,协程/`yield` 地基就绪**(留作独立后续项)。
- [x] **M2.6** fuel + 模块白名单 —— **基本达成**(内存上限待)。**fuel**:`LK_FUEL=N`(CLI)+ `Vm::with_fuel(N)`
      (lk-api)经 `execute_program_with_ctx_and_budget`。**模块白名单**:`Vm::sandboxed(&["math",…])`(lk-api)
      只注册核心 builtin + 白名单模块,OS 模块(fs/net/process)默认拒。测试:`sandboxed(["math"])` 下
      `math.max(3,7)→7` 而 `use fs` 报错。→ **fuel + 模块白名单沙箱就绪**(也补全 M3 沙箱 builder)。
      **内存上限**:Executor 加 `heap_object_limit`(活堆对象数上限),在与 fuel 同频的 per-instruction 检查点校验,
      **折进 `const BUDGETED` 单态化路径 → 无 limit 时零开销(bench 走 false 分支不受影响)**;`execute_program_with_ctx_and_limits`
      + lk-api `Vm::with_heap_limit(n)` 暴露。测试:分配超限程序报「heap object limit exceeded」。全量 1485 tests 0 失败。
      → **三沙箱知(fuel/内存/模块白名单)齐**。
- [x] **M2.7** 字节码验证器 fuzz（Exit「fuzz 验证器无 panic」证据闭合）：`core/src/vm/verify_fuzz_tests.rs`——
      三路生成器（字节级破坏/JSON 结构感知变异/随机垃圾）+ 定向敌意语料（entry 越界、指令字全 1、寄存器数清零、
      fact 表长度炸弹、深嵌套 JSON），断言 `from_json_str`→`into_module`→`verify_module` 只 Err 不 panic。
      本地 2 万例 + 新种子 5 千例 0 panic；correctness.yml 挂 5 万例 scaled + run-id 种子 2 万例。commit `9d7fedd`。
- **Exit**：`pcall` 捕获所有可恢复错误 ✓；fuzz 验证器无 panic ✓（M2.7）；沙箱指标可配 ✓。
  → **M2 Exit 三项均有证据**（M2.5 stackless 是超出 Exit 的 deliverable，现已完整达成，见上）。

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
- [x] **M4.2** —— **Exit 达成**(程序粒度);逐函数混合是超出 Exit 的 future 优化。
      **Exit 逐条**:① 任意 `.lk` 可 `lk compile`(Tier 0 保底)✓;② **覆盖 >11/44 ✓**(现 **14/50** native,
      baseline 11——本轮 4 个 AOT win 使 `list_destructure.lk`(IsList+SliceFrom)、`string_split.lk`(StringSplit)进入
      可原生编译集);③ 失败构造回退 VM 而非报错 ✓;④ 差分全绿 ✓(1451 tests + ASan/UBSan)。
      **已做(消除「全有或全无」问题 2 的程序粒度)**:`lk compile FILE`(native)在 MIR/LLVM lowering 返回
      `Unsupported` 时,不再整程序报错,而是 warn + 回退 **Tier 0 VM bundle**(内嵌解释器)→ **任何有效程序都能 compile**
      (可 lowering 走原生,否则 VM 内嵌)。先解析(真源码错误暴露、不被 Tier 0 掩盖)再试 native。`LK_AOT_NO_FALLBACK=1`
      关闭回退供 strict native-only 验证。验证:算术→原生 42;pcall→回退 Tier 0 exe 跑对;语法错误→exit 1 不产 exe;
      AOT 差分(可 lowering 用例走原生不触发回退)全绿;cli 93 tests。→ **Exit「任意 .lk 可 compile(Tier 0 保底)、
      失败回退 VM 而非报错」达成(程序粒度)**。
      **typed-subset 覆盖增量(本轮,找到可增量路径)**:当 AOT 类型系统已有 type+ops、仅缺某 opcode 的 lowering 时,
      加该 opcode 是有界低风险 win。本轮两组:**① `IsList`**(const-fold,类比 IsNil;commit `ef55604`);
      **② `SliceFrom`**(rest 尾切片,lkrt `lkrt_lklist_{i64,f64,str}_slice_from` 类比 map_fn arena_handle + abi + lower;
      negative start abort 匹配 VM;commit `6b52a3a`/`47199c1`)。**③ `StringSplit`**(`str.split(sep)`→ListStr;lkrt 用
      Rust `str::split`——与 VM `string_split` 同函数**零语义风险**;parts 经 `arena_c_string` 永生;commit `8755e02`;
      `examples/stdlib/string_split.lk`)。→ `if let [a,b,c]=xs` / `[head,..tail]=xs` 列表形状/rest 解构 + `str.split`
      现对所有 typed list 原生编译(均经 native==VM 差分 + **ASan/UBSan** 验证)。
      **可复用模式**:const-fold opcode(零 runtime)或小 lkrt 函数+abi+lower(差分/ASan 守卫)。→ 朝 Exit「覆盖 >11/44」落地。
      **待做(逐函数 Tier 1 混合)**:同一程序内 native 函数 + VM-executed 函数混合 + native↔VM ABI 桥——多天架构工程。
      **→ 设计已定稿 `docs/llvm/tier1-hybrid.md`(commit `83c8b4a`,实测绘 lower/abi/codegen/link/cli 后)**:
      单向 native→VM 桥、桥居 lk-api(lkrt 铁律不破)、.ll 级无 VM 不变量保留(VM 链接期经 wrapper 进入)、
      v1 资格=标量参数+结果全废弃(dead_writes)+传递闭包无用户 globals+无 captures、不满足则感染调用者、
      及 entry 回退 Tier 0;stdio flush 顺序/未捕获错误 abort 对齐/artifact 复用(M2.7 加固面)为硬约束。
      **5 个可提交子步**:① lk-api hybrid 运行时+单测 → ② lower 标记+资格分析+MIR 快照 → ③ codegen declare+桥调用
      +.ll 快照 → ④ cli 混合链接+端到端差分 → ⑤ fuzz 生成器扩展。
      **✅ 子步②③④ 完成(端到端打通)**:② lower 标记(commit `e194d11`):final pass 收集失败→资格审查→
      native-reachable 重算(不进 VM 函数,try/catch 脱糖闭包免 lower)→重跑;CallVm 发射 + **dst 不绑定**
      (ssa.read 未绑定寄存器天然 Unsupported → 结果被读=回退,零 liveness 证明);`lower_with_hybrid` 显式参数,
      默认关 + `LK_AOT_HYBRID=1` opt-in。③ mir CallVm/vm_functions/render/validate + codegen(%LkHybridArg +
      单一 global 参数缓冲防循环 alloca 长栈 + **先 lkrt_io_std_flush(1) 再桥调**)。④ cli 混合链接(commit
      `27745be`):LlvmModule.vm_function_count → wrapper C(constructor 注册嵌入 artifact)+ liblk_api.a 链接;
      ensure_lk_api_staticlib 改总是 cargo build(修旧 staticlib 缺符号)。**端到端验证:混合 exe 输出与 VM
      完全一致(含跨 native/VM stdio 顺序),uncaught 桥错误 exit 非零+消息达 stderr**。测试:hybrid_lowering 4
      + hybrid_compile 2;既有快照/差分全绿。
      **✅ 子步⑤ 完成**(commit `2427323`):fuzz 生成器约半数程序带 hybrid 帮手(try/catch 包 println、标量参数、
      语句位调用),compile 走 LK_AOT_HYBRID=1 → hybrid 二进制入差分语料(correctness.yml 自动覆盖)。
      **首轮 120 例即抓到真 bug**:桥前 flush 用 lkrt_io_std_flush(冲 lkrt Rust stdout)而 codegen println 走
      C printf(C stdio 缓冲)——两缓冲区,native 输出滞后桥内 VM 输出;修复=桥前 `fflush(NULL)`(libc)。
      500+300 例 100% 对比 0 分歧。**默认开关裁决:保持 opt-in**,correctness.yml 数轮全绿后翻默认
      (翻时确认 aot fuzz/differential 断言与 ensure_lk_api_staticlib 的 CI 成本)。→ **M4.2.2 五子步全部完成**。
      **✅ 子步① 完成**(commit `2e19e94`):core `call_module_function_with_ctx`(exec/program.rs,seed globals 同
      模块运行、CallableValue::Closure+call_runtime_value_runtime 调 fidx、ModuleFunctionArg 标量 marshal)+
      lk-api `HybridModule`(from_artifact_json→imports→verify;find_function 按 debug_name;call_discard)+
      ffi `lk_hybrid_register/lk_hybrid_call_v`(进程单例,错误即 exit(1))+ lk.h。core 947/api 11 全绿。
      **子步② 实现方案(已测绘定稿,代码级锚点)**:
      - **关键更正**:`dead_writes` fact 只标纯字面量表达式语句(compiler/builder.rs `mark_last_dead_write`),
        **不覆盖调用结果** → 设计中「用 dead_writes 证明结果废弃」不可用。**替代方案(更优,零证明)**:VM-executed
        被调方在 `lower_user_call` 里**不 `ssa.write` dst 寄存器**(并清 `current_def[block][dst]`+`builtin_regs`)——
        `ssa.read` 对未绑定寄存器本就返回 Unsupported(lib.rs:3116 注释明说)→ 结果被读=整模块回退,sound。
      - **两阶段结构**(改 `lower()` final pass,lib.rs:560-589):final pass 从逐函数 `?` 改为收集
        `Vec<Result<MirFunction>>`;全 Ok→照旧;有 Err→对每个失败函数跑资格审查,全合格→设
        `sig.vm_functions: HashSet<u32>` 后**整体重跑 final pass**(globals 重建),否则返回首个错误(现行为)。
      - **资格审查**:f≠entry;capture_count==0;!sig.specialized[f];lambda_params[f] 全 None;param_obs[f] 全
        Some(I64|F64|Bool|Str);globals 扫描:从 f 经 CallDirect/MakeClosure(b 操作数)BFS 全子树,任何 SetGlobal
        →拒;GetGlobal 到「全模块任意处被 SetGlobal 的 slot」→拒(未写 slot=builtin 读,VM 侧 ctx seed 等价,安全)。
      - **lower_user_call 桥分支**(lib.rs:2219,在 clone/summary 机制后、CallFn 发射前):callee∈vm_functions →
        读标量 args(仅 I64/F64/Bool/Str,容器句柄拒)→ 发 `Inst::CallVm { func, args }` → dst 不绑定。
      - **MIR**:`Inst::CallVm` 变体 + `MirModule.vm_functions: Vec<VmFunction{id,params:Vec<Ty>}>`(构造点:
        lower、mir tests div_module、codegen tests/examples demo);render 仅非空时打印(既有快照稳定);validate 加臂。
      - **codegen**:vm_functions 非空时 prelude 加 `%LkHybridArg = type {i8,i64}` + `declare void
        @lk_hybrid_call_v(i32,ptr,i64)`;CallVm 渲染:alloca 数组+逐 arg store(tag i8@field0,值@field1,Bool zext
        i64,F64 store double,Str store ptr)+ **先 `call @lkrt_io_std_flush(1)`(stdio 顺序)** + call_v。
      - **门控**:`pub fn lower_with_hybrid(artifact, hybrid: bool)`,`lower()`=默认关+`LK_AOT_HYBRID=1` env 开
        (edition 2024 set_var unsafe→测试用显式参数不用 env)。**在 ④ 前不能默认开**:aot fuzz 断言失败必含
        「does not support」,hybrid 后 clang 会因 lk_hybrid_* 未链接报 undefined symbol,破坏该断言。
      - **测试**:lower 单测(hybrid on:混合程序产出 vm_functions+CallVm;结果被读→仍 Unsupported;SetGlobal 子树
        →拒)+ mir_snapshots 加 hybrid 快照(调 lower_with_hybrid)。
      **backend.md 已整体删除(用户裁决)**:活引用全清(README×2、CLAUDE.md、correctness.yml、tier1-hybrid、
      aot-gaps/aot-redesign 死链);顺带修 README 过时描述(「不支持即失败」→回退 Tier 0;中文版 pkg 行仍在
      宣传 M5.4 已删的 publish/key/serve → git+lockfile)。子集清单不再单独维护,历史见 git。
      更深 blocker(Raise 需 catch 处理、NewObject/NewRange/StringSplit/map-access/动态 Call/GetGlobal builtin)需扩类型系统+lkrt。
- [x] **M4.3** 差分门禁 `AOT==VM` 已在 CI —— **现状核实,已满足**。`cli/tests/aot_differential_test.rs`
      (MIR native == VM,stdout+成功/失败逐例比对,21 检查点)+ `examples_differential_test.rs`(VM==AOT 语料)
      + `aot_fuzz_differential_test.rs`(随机差分)均随 `check.yml` 的 `cargo test --workspace --all-features` 跑;
      `correctness.yml` 更在 **ASan/UBSan + fuzzing** 下专门跑这三个差分。→ AOT==VM 门禁固化。
      本会话新增的 **M1.2(VM source==bytecode 差分)**与之互补,共同守 VM 为规范。
- **Exit**：任意 `.lk` 可 `lk compile`（Tier 0 保底）✓；覆盖 >11/44 ✓（14/50）且失败构造回退 VM 而非报错 ✓；差分全绿 ✓。
  → **M4.2 Exit 达成**（程序粒度）。唯 M4.2.2 逐函数原生+VM 混合(native↔VM ABI 桥)是超出 Exit 的 future 优化。

## Phase M5 — no-std profiles + 工具链收敛 + v1.0（问题 7）

- [~] **M5.1** `bare`/`alloc`/`full` 三 profile 打通（feature 矩阵）。**进展**:三 profile 已由分层 crate 体现并**CI 验证可构建**——
      `bare`=`lk-hal`(纯 no_std)、`alloc`=`lk-values`(no_std+alloc)均编到 **wasm32 + thumbv7em 裸机 MCU**;
      **`alloc`(VM 核心级)= `lk-core --no-default-features`**(现真 no_std,M0.7/8 已达成)、`full`=`lk-core`(默认 std)+stdlib。
      → **M0.7/8 flip 后,`lk-core` 单 crate 已承载 alloc(no_std)↔full(std)两档**(`std` feature 切换)。
      **✅ M5.2 依赖手术完成(commit `db5b376`):VM 核心全量编译过 thumbv7em-none-eabi 裸机目标,crate graph
      全程无 std**。手术内容:删 5 死依赖(once_cell/chrono/sha2/rand/tracing)+ tempfile→dev-deps;anyhow/serde/
      serde_json 切 no_std 模式;toml/serde_yaml/dashmap optional 绑 std(val/de.rs YAML/TOML gate、compat
      SharedMap 双态);compat::float(libm);AtomicU64→AtomicUsize;crate-type 去 staticlib。CI thumbv7em 加
      lk-core 守卫。**重要更正:此前「真 no_std」构建靠依赖把 std 连进 graph(f64 inherent 方法因此可解析),
      本次才彻底**。workspace 106 套测试/wasm32/clippy 全绿,std 路径热代码零变化。
      **真机运行遗留(超出编译可行性)**:#[global_allocator]+panic handler+HAL 接线的演示固件 + QEMU 冒烟——
      需要嵌入式 demo crate,属 nice-to-have。
      **细粒度 feature 矩阵(float/unicode)数据驱动建议:不做**——float 可关的收益仅限无 FPU MCU,成本是
      Float 遍布 VM 的巨大 cfg 面;coroutines 已由 async-runtime optional feature 承载。
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
      **文档遗留已闭合**:`docs/packages.md` 已是 git-only 描述;`README.md` 速查行(过时引用 `pkg publish`/`key`/`serve`/
      signed registry)已刷新为 git+lockfile 命令(init/add/fetch/update/check/tree)。全仓无残留 registry 过时引用。
- [x] **M5.5** LSP **保留并持续维护**（不砍）+ tree-sitter —— **双轨保留,现状核实**。plan 决策(本会话已改 plan.md)
      = 不砍 LSP,与 tree-sitter 双轨。现状:`lsp/`(13k 行,hover/goto/semantic-tokens/inlay/completion/diagnostic
      共 424 引用点,macro-origin 感知)+ `ecosystem/tree-sitter-lk`(grammar.js)+ `vsc-ext`/`zed-ext` 编辑器集成
      均在树中、随 workspace 编译通过。→ **满足「保留双轨」**。tree-sitter 完善为持续项(非本步阻塞)。
- **Exit**：CI 矩阵全绿；v1.0 定义达成。
  → **✅ v1.0 定义六项全部达成(2026-07-04 盘点)**:VM 规范测试全过 ✓ · AOT Tier 0 全覆盖 + Tier 1 混合 ✓ ·
  pcall 错误模型 ✓ · 多实例嵌入 API ✓ · bare/alloc/full 三 profile(VM 核心裸机可编译)✓ · git 最小包管理 ✓。
  剩余项均为 post-v1.0:M2.5 ①-③(2026-07-04 后已完整完成,见上)、callable 反转(建议不做,见下)、
  M4.2 深覆盖(mixed 类型系统,Tier 1 已供函数级出路)、MCU 真机 demo/细粒度 feature(nice-to-have/建议不做)。

## Post-v1.0 — 协程/`yield`(plan.md 4.5 真正目标收益,M2.5 stackless 之后落地)

- [x] **子步A** 核心 VM 机制 —— `HeapValue::Coroutine(Box<CoroutineState>)` + 新 opcode `Yield`(106/128)+
      `Executor.active_coroutine`(跨原生调用边界 yield 天然报错,靠"专用 Executor 才设置该字段"免记账)+
      `coroutine::resume_coroutine_runtime`(check-out/check-in,heap/globals 与 resumer 共享、stack/frames/
      call_depth 协程私有)。**实测踩坑**:`LK_GC_STRESS=1` 下 resumer 自己的栈整体换出会导致其活跃寄存器
      (含持有协程值本身的寄存器)运行期间不被 GC 扫到——修复=新增 `Executor.extra_gc_roots`。手写字节码
      测试 6 个(含跨原生边界 yield 报错、协程挂起时 GC 存活两个高价值场景),commit `a5f6725`。
- [x] **子步B** 语言层语法 + stdlib 内建 —— `yield` 关键字(lexer/parser,`parse_expr` 顶层识别,绑定尽量
      松)、`Expr::Yield` 补齐 18 处编译期分析穷尽匹配、`lower_yield`(值落新寄存器防污染 local,yield 后
      重置寄存器类型 fact 为 Unknown)、`coroutine_create/resume/status` 三个全局内建(同 pcall/error 待遇,
      不需要 `use`)。`MODULE_ARTIFACT_VERSION` 7→8。新增 `examples/syntax/coroutines.lk`(自动纳入
      VM==bytecode 与 VM==AOT 差分语料库)。**端到端一次跑通**:生成器循环、双向传值、协程内错误捕获、
      协程外裸 yield 报错,commit `5cf2a32`。
- [x] **子步C** AOT 兜底 + 文档 —— **实测确认无需新增 AOT 代码**:`Yield` 天然命中 aot/lower 既有的
      `_ => Unsupported::Opcode` 兜底分支,`lk compile` 自动回退 Tier 0 VM bundle(验证:
      `lk compile examples/syntax/coroutines.lk` 产出自包含可执行文件,跑通输出正确)。新增
      `docs/coroutines.md`(API、示例、v1 限制:yield 仅顶层表达式位置合法、任意函数可含 yield 但非
      resume 直接调用时运行时报错、跨原生边界 yield 报错、AOT 暂不原生支持)。
- **验证(贯穿三子步)**:workspace 全量 + `LK_GC_STRESS=1` 全绿(957 lk-core 测试)· clippy/fmt 0 ·
  `lk-core --no-default-features` 真 no_std 构建 0/0 · dist bench 门禁三次测量均不劣于基线
  (0.991x/0.988x/—,新增字段/opcode 未进入非协程程序热路径)。
  → **协程/`yield` 完整达成**,plan.md 4.5 的 stackless 协程目标全部兑现。

## Post-v1.0 — `sched` 协作式调度器(chan/task × 协程整合,handoff 记录的"下一自然大步")

**设计裁决**:native 不能 yield(协程轮已定死的结构性限制)⇒ Go 式"阻塞原语自动挂起协程"走不通;
采用 stackless 经典的 **yield-descriptor + 调度器** 模式——`sched.recv/send/sleep/pause/spawn/join/await`
只构造等待描述符(≤7 字节 ShortStr tag 的 tagged list,零堆分配 tag),`yield sched.recv(c)` 是显式挂起点
(类似 await),`sched.run` 解释描述符驱动 N 个协程。

- [x] **子步A** core 支撑 —— `resume_coroutine_runtime` 新增 `extra_roots: &[RuntimeVal]` 参数(调度器在
      Rust 局部变量里跨 resume 持有的工作集对 GC 安全点不可见,必须显式进 root;上轮 GC 坑的正面延伸)+
      `rt::Runtime::take_task(id)`(取出 JoinHandle 所有权,`&mut` 跨 select 轮 cancel-safe 重试;
      `join_task` 的 remove+await 形态在 select 半途 drop 会丢 task)。顺手修复 lsp integration_test 漏掉的
      `Token::Yield` match 臂。commit `c6057c1`。
- [x] **GC bug(fix-forward)** —— stress 跑 sched 测试暴露**死协程 GC trace panic**:协程 Done/Errored 时
      清空 stack 但未重置 stack_top,`gc_edges` 对 `stack[..stack_top]` 切片越界;任何"死了但句柄仍被引用"
      的协程被 GC 追踪即 panic(上轮遗留,调度器把完成协程句柄存 results 表后继续跑 VM 恰好触发)。
      修复 + `tracing_a_dead_coroutine_does_not_panic` 回归测试,commit `3560927`。
- [x] **子步B** `lk-stdlib-sched` crate —— 描述符 natives(plain)+ `sched.run`(full_state,先例:stream 的
      `kind = "full_state"` 模块导出)。调度核心:round-robin 队列 + parked 表(Recv/Send/Sleep/Await)+
      joiners 表;try_recv/try_send 快路径,不 ready 才 park;全员 parked 时自建 `(index, Wake)` futures
      走 `select_all`(**不用**现成 `SelectOperation`——它 send-closed 直接 Err 丢 case index),sleep 最早
      deadline 做 timeout 上界;**join-only 死锁可证明**(join 只能由本调度器完成)→ 确定性报错,而
      channel/await 阻塞合法(外部 tokio task 可投递,同 Go)。**实测踩坑**:`tokio::time::sleep` 在
      runtime 上下文外创建即 panic("no reactor running")——必须包进 `block_on(async {...})` 内创建。
      测试 17 个:crate 内 9(描述符构造/校验 + 手工字节码驱动 await park/recv 外部唤醒/join 环死锁;
      注意手工双函数需错开 pc——global inline cache 按 pc 共享,真实编译码有 per-function facts 不受影响)
      + umbrella 8 个 LK 源码行为测试(`stdlib/src/sched_test.rs`)。bench 门禁 1.007x。commit `b077544`。
- [x] **子步C** 语料 + 文档 —— `examples/stdlib/sched_demo.lk`(全场景确定性输出,自动进 VM==bytecode 与
      VM==AOT 差分门禁;AOT 兜底实测:GetGlobal 不可 lower → Tier 0 bundle,产物跑通)。`docs/coroutines.md`
      新增 sched 章节(API/示例/语义要点,示例逐字验证可跑)+ `docs/stdlib.md` 并发模块边界(chan/task/sched
      三分)。
- **已知边界(留档)**:全局 `spawn(闭包)` 是**既有断点**(闭包无 promote 到 `CallableValue::Runtime` 的
  路径,`task.spawn_blocking` 同因 bail;顶层就复现,与本轮无关)——`sched.await` 的 LK 级用例因此只能用
  task-returning natives(net.tcp 等),Rust 级测试已覆盖 await park/wake。协程值不可穿 channel(深拷贝
  边界既有守卫),句柄经共享 map/list 传递。
- **验证(贯穿)**:workspace 全量 1498+ 0 失败 · `LK_GC_STRESS=1`(core 959 + stdlib sched 8)全绿 ·
  clippy/fmt 0 · no_std 构建 0/0 · dist bench 1.007x 基线内。

## Post-v1.0 — `select` 语句 lowering(悬空构造收编,sched 之后的自然候选)

**设计裁决**:与 try/catch → pcall 同款 **parse 时 desugar**,不是给 Expr::Select 写专用后端——
case 的 channel/send 值/守卫按源序急切求值进 `__select{n}_*` 合成局部变量(parser 计数器保证嵌套
hygiene),调用 `select$block` 老 native(名字含 `$` 用户无法碰撞;底层 tokio SelectOperation 完好),
尾部 Conditional 链按 index 分派。resolver/typecheck/compiler/AOT **零专用代码**。

- [x] **子步A** desugar + AST 删除(commit `ad02dfe`)—— 删除 `Expr::Select`/`SelectCase`/
      `SelectPattern` 及 11 个文件的匹配臂/辅助函数;**compiler lower_expr 的"不支持表达式"兜底分支
      从此不可达并删除**(每个 Expr 变体都有 lowering)。顺手修复:resolver 对裸 `Expr::Block`
      此前是空处理(只有闭包体路径),现在按语句块正常 resolve;老 resolver 对 Send 分支 channel/value
      不 resolve 的 bug 随删除消失。**语义定案**(详见 docs/coroutines.md):急切求值(Go 规则)·
      守卫先于 binding 求值(真值归一化 Bool)· binding=接收值(closed→nil)· case body 单表达式
      (同 match arm,外层环境求值,赋值/return 语句不可入)· 无 default 阻塞线程 · closed channel
      参与 → 可捕获错误 · 全守卫禁用+无 default → nil(留档,不同于 Go 死锁 panic)。
      测试:core 4 个 desugar 形态 + stdlib 10 个 LK 行为(`stdlib/src/select_test.rs`)。
- [x] **子步B** 语料 + 文档 —— `examples/syntax/select.lk`(9 场景确定性,进 VM==bytecode 与
      VM==AOT 差分门禁;AOT Tier 0 bundle 实测跑通)。docs/coroutines.md「The select statement」
      章节 + docs/semantics.md 语法边界补记(try/catch 与 select 均 parse 时糖)。
- **验证(贯穿)**:workspace 全量 1508+ 0 失败 · GC-stress 全绿 · clippy/fmt 0 · no_std 0/0 ·
  dist bench 1.014x 基线内 · 差分门禁含新语料全过。

## v2 语言面重设计(用户裁决 2026-07-06):Swift 式错误模型 + Go 式并发

**用户四项裁决**:① 协程/yield/sched **全删**(Go 无此概念);② 上 **`go` 关键字**;③ `!` 走
"出错即抛"(整体从 `[ok,value]` 对迁移到 raise);④ 数据语义由 agent 从架构裁决 → **保留深拷贝
isolate**(单线程无锁 GC 是热路径底线,无数据竞争,推翻=重写堆/GC)。plan.md 4.4/4.5 已加裁决注记。

- [x] **子步1 全删协程/yield/sched**(commit `33d3fb9`,-2587 行):上一轮加法的精确逆操作——
      yield 关键字/Expr::Yield/Opcode::Yield/HeapValue::Coroutine/coroutine.rs/coroutine_* 全局/
      sched crate/相关语料文档。`MODULE_ARTIFACT_VERSION` 8→9。M2.5 CallFrame 地基保留;
      select/chan/task/spawn 存活(= Go 并发组成部分)。
- [x] **子步2 修 spawn(闭包)**(commit `a16da0e`):`spawnable_callable` 快照 promote——
      `Arc::new(module.clone())` + 捕获与 globals **同模块结构深拷贝**(core 新增
      `copy_runtime_value_same_module`,`ClosureCopy::SameModule` 模式;跨模块路径保持 Reject)
      → `RuntimeCallable::with_state`。**关键修复** `Runtime::block_on`:goroutine(tokio worker)
      内阻塞 send/recv 直接 block_on 会 panic,多线程 flavor 改走 `block_in_place`——Go 惯用法
      (goroutine 里阻塞收发)从此成立。spawn 改 FullState 注册;删除从未工作的 task.spawn_blocking。
- [x] **子步3 go 关键字**(commit `cbf0e10`):`go <expr>;` → `spawn(|| expr)` parse 时糖,火忘。
      顺手修 send/recv typecheck 对未推断操作数(fn 参数类型变量)的误报(约束 Channel 而非拒绝)。
- [x] **子步4 错误模型**(commit `a910eb3`):
      - pcall → 隐藏名 `try$call`(select$block 同款 `$` 先例),try/catch desugar 改目标,
        用户面 pcall 消失
      - 后缀 `!` force-unwrap(parse 时糖):nil → raise "unwrap of nil value"。**消歧**:`!` 紧跟
        `(`/`[`/`{` 保留给宏调用(`name!(...)`,宏系统三种定界符都在用——stress 测试先撞出冲突);
        `x!==1` 因 lexer 贪婪 Ne 是 parse 错误(写 `x! == 1`)
      - raise 迁移:recv → 值|closed 抛;send → Nil|closed 抛;chan.try_recv → 值|nil(空)|closed 抛;
        task.try_await → 值|nil。**chan.close 改 Go 语义**:标记+丢 sender、条目保留——缓冲可排空
        (旧行为 remove,缓冲丢失、后续 "Channel not found");**select 对 closed channel 变
        always-ready**(排空后 recv arm 以 nil binding 命中,Go 零值模拟)
      - 语料全迁:pcall_error.lk → error_unwrap.lk,error_model_edges/try_catch/select.lk 改写,
        api/traceback/aot-hybrid 测试迁移,新增 chan_semantics_test ×5
- [x] **子步5 文档收尾**(本 commit):新 `docs/concurrency.md`(go/spawn/chan/select 全貌 +
      与 Go 差异表)· semantics.md(v2 错误模型 + `!` 边界)· stdlib.md 并发段 · plan.md 4.4/4.5
      裁决注记 · concurrency_demo.lk 补 goroutine 段。
- **验证(贯穿)**:workspace 全量 1499+ 0 失败 · GC-stress 全绿 · clippy/fmt 0 · no_std 0/0 ·
  差分门禁全过 · dist bench 见子步5 记录。

## M4.2 循环轮记录:空[]重猜(2026-07-07)

- EmptyListGuessWrong{pcs} retriable:与 DynLoopPhi 同模式(错误值携带
  发现 → fixpoint 记录 → 重跑改物化)。两个机制合起来,"猜测型 lowering"
  有了统一的证伪-重试通路。
- **时序死锁教训**:猜测 handle 的 provenance(empty_guess 表)想通过 phi
  传承,但消费点失败(整函数 Err)早于 loop header 的 seal_block——传承
  永远来不及。裁决:检测放宽到"函数内全部未定案猜测"(over-mark),正确性
  由 Dyn 万能装箱兜底,typed 性能损失实测为零(覆盖率无倒退)。精确传承
  仍保留在 phi 同型路径(能命中时单标)。
- 覆盖率 25/51 持平;全门禁绿。

## M4.2 循环轮记录:fixpoint 重猜 + sanitizer(2026-07-07)

- **DynLoopPhi retriable 机制**(loop phi 混型的正解):seal_block 时体内
  已按旧类型消费,不能就地宽化——把发现编码进 Unsupported::DynLoopPhi
  {block,slot}(错误值携带数据),fixpoint 调用点捕获记入 sig.dyn_loop_phis
  (纳入 snapshot 收敛判断)重跑;重跑时 read_recursive 创建 phi 直接
  Ty::Dyn,体内全走 Dyn 臂,初值边装箱(add_phi_operands 对 phi_ty==Dyn
  的异型边装箱不算宽化)。集合单调增长且有限 → 终止。**该模式可复用于
  空[]混合 push 的重猜**(留档)。
- **lookahead 猜测的教训**:LoadHeapConst 一刀切进 Dyn 源会让 push 长
  字符串的列表错猜 ListDyn——display 引号差分(ListStr 引号 vs Mixed
  裸文)。堆常量必须按种类分流。猜测类机制的每次扩面都要过"display/eq
  语义是否随类型变化"这一关。
- **sanitizer 验证补齐**(plan 验证清单第 5 条,M4.2 全程欠账):
  LK_NATIVE_SANITIZE=address,undefined,6 个翻转例 + 200 迭代全特性
  压力 probe(混合构造/字符串模板/HOF/chunk/zip/struct/切片),ASan+UBSan
  0 报告,压力 probe 真原生与 VM 逐字节一致。注意 lkrt 静态库本身未
  instrument(堆越界经 malloc 拦截仍可捕,栈/全局不可)——完整 instrument
  需 lkrt 以 -Zsanitizer 重编,留档。
- 覆盖率 25/51 持平;全门禁绿。

## M4.2 循环轮记录:Dyn 折叠点安全审计(2026-07-07)

- **两个静默语义 bug**(比 reject 危险一个量级——产出错误结果的原生二进制):
  1. IsList/IsMap 对 Ty::Dyn 编译期折叠 Const false——rest 解构
     'for [head, ..tail]' 的模式守卫恒失败,原生错误 Raise(幸 loud)。
  2. Exit::NilBranch 的 \`_ =>\` 臂把 Dyn 折叠为'恒非 nil'——struct
     optional 缺省字段 'if (b.v != nil)' **静默走错分支**输出错误结果,
     若非 probe 实测无法发现(examples 差分面没有该形状)。
- **方法论教训**:引入新的"运行时才知道内容"的类型(Dyn)时,必须**全量
  审计既有的类型驱动编译期折叠点**——折叠假设"类型即语义"对 Dyn 不成立。
  本次 grep 'value: Const::Bool' 全扫,四处折叠点(IsNil/IsList/IsMap/
  NilBranch)全部 Dyn-aware 化。
- SliceFrom 补 Dyn(as_list 拆箱)/ListDyn(dyn_slice_from) 臂;
  空 [] 猜测 lookahead 源扩展 SliceFrom/ToIter。
- 再踩二进制新鲜度坑:compile 失败被 grep -c 吞掉,/tmp/rp 跑的是上一个
  probe 的旧二进制(输出驴唇不对马嘴才察觉)。**probe 流程固化:rm -f 目标
  → compile → ls 确认存在 → 跑**。
- 覆盖率 25/51 持平(修正确性,非扩面);全门禁绿。

## M4.2 循环轮记录:迭代/空列表/字符索引(2026-07-07)

- **s[i] 单字符索引**:VM index_string_at = char 索引、越界 nil、负索引按
  **字节** len 回数(怪癖,ascii 下等价)。native str_char_at → Dyn(nil 自含)。
  'for ch in "abc"' desugar 成按索引循环,连锁需要 AddInt 的 (Str, Dyn)
  **拆箱特化**:VM 里 Str+非Str 必错,as_str guard 拆箱 = 同款 loud,
  发射 typed concat 保住 acc 的 Str 类型(loop header phi 禁 widen,
  装箱路由会把累加器变 Dyn 导致 loop 混型 reject——拆箱是唯一通路)。
- **ToIter**:编译器仅对"证明是列表"的 for-in 发索引循环,其余(嵌套列表/
  字符串变量/map)发 ToIter 规范化。native:列表 identity、Str→chars、
  Dyn→as_list guard;map reject(hash 序)。
- **空 [] 猜测升级**:实测发现空 [] 走 LoadHeapConst(常量池)而非 NewList
  ——之前 NewList 空臂白修(保留无害)。empty_list_is_str_elem 泛化为三态:
  首 push 的字节码级来源 str→ListStr / 索引读→ListDyn / 默认 ListI64。
  ListDyn 猜测语义安全(一切可装箱,错猜只可能亏性能不亏正确性)。
- **ListPush Maybe unwrap**:flat.push(xs[i]) 的 xs[i] 是 MaybeI64,
  旧 read_typed 不 unwrap 直接挂——全臂换 read_scalar 族。
- 教训:probe diff IDENTICAL 前必须确认 Warning=0(本轮两次差点被
  双 VM 假象骗过);字节码级 dump 时注意空 [] 与 [x,y] 走不同 opcode。
- 覆盖率 24→25/51;全门禁绿。

## M4.2 循环轮记录:iter 转发 + NewObject + Str 批次(2026-07-07)

- **iter 模块函数版 = 方法的模块拼写**:Call 臂对 iter.{map,filter,reduce,
  enumerate,zip,take,skip,chain,flatten,unique,chunk} 转发方法 lowering,
  HOF 复用 lambda-aware 的 lower_list_hof_k(窗口 base 右移一格对齐 lambda
  寄存器);iter.range 补 1 参(0..n)。iter_pipeline/list_iter_sugar 两例
  翻转——注意 [left,middle,right](NewList 列表元素)+ iter.flatten(ListDyn)
  + ListDyn==ListI64 Cmp 是三轮改动的组合生效。
- **NewObject 裁决落地**:MapStrDyn 承载(GetFieldK 零改动、缺省 optional
  字段=str_dyn_get Nil 与 VM absent-field 同构);type_name 丢弃(整对象
  display/typeof 不进子集)。连锁补齐:AddFloat 家族 Dyn 装箱路由(struct
  字段类型注解让编译器发射 typed float opcode,而字段读回 Dyn)、IsNil Dyn
  臂(tag==0,`p.s ?? "none"` 的 nil 测试)。struct.lk 翻转;struct_trait
  卡 trait 方法分发(GetGlobal 大项)。
- **Str 方法批次**(9 lkrt helper):字节语义钉齐——find 返回字节 index、
  substring 字节切片(非 char 边界 VM panic → native abort)、ends_with
  字节后缀;unicode 语义——lower/upper 用 Rust to_lowercase、reverse
  char-wise;**chars() 返回 VM Mixed 列表 → native 必须 ListDyn**
  (ListStr 的引号 display 会差分,裸文才对)。string_methods.lk 翻转。
- comprehensive.lk 裁决留档:Set 内建类型(has/add/delete,native 无 Set
  表示)是独立特性,加上 fs/path/string 模块长尾,整例翻转性价比出子集。
- 覆盖率 20→24/51;全门禁绿(1505 tests/四套差分/clippy/fmt 0)。

## M4.2 循环轮记录:phi 装箱 + in 语义 + range 切片(2026-07-06)

- **phi 混型合流装箱**:add_phi_operands 重构(收集全边→决策),混型且全
  dyn-boxable 时 phi 宽化 Ty::Dyn、每边 edge_insts 装箱(dyn_box_on_edge)。
  安全边界:仅 read_recursive sealed 臂新建的前向 join phi(allow_widen);
  loop header(seal_block)与自引用边 reject——param 已按旧类型被体内消费。
  解锁 a ?? "default" 与 match 混型臂;match.lk 翻转。
- **'in' 第三套 eq 勘探**(VM 真源,三套语义各不同):
  - `==`(exec runtime_values_equal):数值跨型 true、结构比较
  - `unique()`(core_methods 版):数值 to_bits、Obj 句柄
  - `in`(list_contains):typed 列表**严格同变体**(1.0 in [1,2] false!)、
    Mixed=RuntimeVal derive PartialEq(float 按值 ==、NaN 永 false)
  - **发现既有 native bug**:(ListF64,I64) coerce 臂与 VM 分歧,probe 实测
    2 in [1.0,2.0] native true / VM false。教训:**每个消费点的 eq 语义
    必须单独 probe,不能假设复用**。
- **range 切片双 VM bug**:字符串切片按字节 panic('héllo'[1..3] char 边界)
  ——改 char 语义与 s[i]/len 统一;GetIndex range-key 识别 gate 在物化列表
  len<=3(跨度>3 的切片 s[8..20] 直接 error)——去掉限制+空 range 切空前缀。
  native:NewRange 全常量 step==1 记 range_def side-table(ValueId→
  (start,end_excl)),GetIndex 查表发射 str.slice_chars/list_h.i64_slice。
  非常量 range 索引 reject(回退面,VM 修后语义一致)。
- 覆盖率 18→20/51(match/operators);门禁:1505 tests、四套差分、
  clippy/fmt 0、bench 0.995x(8 workloads checksum 一致)。

## M4.2 循环轮记录:HOF 方法批次 + unique 句柄语义(2026-07-06)

- **list_ops.lk 翻转**(chunk/enumerate/zip/unique/flatten 五方法),覆盖率
  16→18/51(另 list_destructure 被 NewList 列表元素装箱连带解锁)。
- **VM unique 语义勘探**(真源实测):core_methods 的 runtime_values_equal
  与 exec/arithmetic 的同名函数**语义不同**——前者数值 to_bits + Obj 句柄
  相等(不结构比较),后者(`in` 操作符)做结构比较。unique/contains 方法用
  前者。长字符串在 typed String 列表中每次读出重新 alloc(无同一性),在
  Mixed 列表直存 handle(有同一性)。native 指针无法区分"同 handle"与
  "intern 常量",裁决:长串永不去重(semantics.md),Mixed 同变量重复留档分歧。
- **VM panic bug**:into_iter_owned 的 String 臂对 >7B 字符串 double-unwrap
  同一 None——['长串'].unique()/zip/chunk/... 必崩。换 list_runtime_items
  (heap-aware),删病灶方法。
- **-0.0 bug**:codegen F64 常量 `fadd double 0.0, x` 物化,0.0+(-0.0)=+0.0
  丢符号;恒等元应为 -0.0。probe [0.0,-0.0] 显示 [0,0] 暴露。
- **NewList 静默不物化坑**:混合臂元素过滤集合不含列表类型时,[l,l] 只记
  ArgList ref 不写 SSA,报错却在下游消费点("r6 read before definition"),
  定位要回看 NewList 而非报错 pc。窗口内 to_dyn memo 保住 [l,l] 去重的
  句柄同一性。
- 门禁:1505 tests、四套差分、lkrt 31、clippy/fmt 0、bench 1.038x(基线内)。

## M4.2 AOT 深覆盖 —— 阻塞点排查(2026-07-06,数据驱动裁决)

**可复现扫描**:`bash scripts/aot_coverage.sh`(compile llvm 全 examples + 原因排行)。
**现状 14/51**,37 个失败按频次:

| 频次 | 阻塞 | 实质 | 路线 |
|---|---|---|---|
| 14 | `GetGlobal` | 白名单外全局:v2 错误模型(`try$call`/`error`——**每个 try/catch 程序**)、并发全局(chan/send/recv/spawn/select$block)、未识别模块(json/encoding/stream/net/task) | (a) try$call 原生=保护调用/unwinding,大;(b) 并发进 lkrt=无 VM 复刻 rt::Runtime,大;(c) module_call_abi 增量补条目,小 |
| 9 | operand 超类型子集 | **混合/动态类型**——真正的 M4.2 核心 | **Dyn 装箱值地基**:MIR `Ty::Dyn` + lkrt tagged value + 装箱运算(display/错误信息须 VM-exact 逐字节) |
| 4 | `LoadHeapConst` | 混合/嵌套常量容器(`{"name":…,"age":30,"active":true}`)——同上 Dyn 根因(同质容器/长串/UpvalCell 已支持) | 随 Dyn 地基解决(`map_h str_dyn_*` 等 ABI) |
| 5 | `Call` | 方法 ABI 长尾:分发表仅 6 对(Str contains/len、ListI64 contains、MapStr* set、list map/filter HOF);`.first()/.last()/.push()` 即倒 | lkrt 薄封装逐个补,增量;但 list_ops 类"deep dive"语料需大量方法才翻转 |
| 2 | `NewObject` | struct 字面量 → 对象模型 | 依赖 Dyn/对象表示 |
| 1 | `NewRange` | range 值物化 | 独立小项 |

**下一步建议(优先序)**:① **Dyn 地基**(9+4+2 例的共同根因,plan.md M4.2 的"mixed/动态类型系统"本体;
注意 semantics.md 裁决:混合 map display 明确不进子集,创建+类型化读取可进);② 方法 ABI 快速增量
(翻转难但每条目独立可验);③ NewRange。try$call/并发进 lkrt 属大工程,建议单独立项。
本会话交付排查+路线(context 预算不足以安全落地 Dyn 子步);实现从下会话新鲜上下文开始。

---

## 执行原则
1. 严格按 Phase 顺序；M0 是所有后续的地基（问题 1/5/8/9），风险最低收益最高。
2. 每步独立可验证：动代码即跑对应测试 + 不回退 `cargo test --workspace` 与 bench 门禁。
3. 大步（M0.1 抽 crate、M2.5 stackless）落地前先拆子步、先跑通编译再迁移逻辑。
4. 渐进抽离 crate，绝不一次性重排整个 workspace。
5. 每轮结束更新本文件勾选项 + `handoff.md`。

## 深覆盖收尾大计划:阶段①(2026-07-07)

- **A1 参数格点 join→Dyn**(`e34841e`):param_obs 冲突不再 reject,
  join 到 Dyn + 调用点装箱;Nil/Maybe 观测归一 Dyn(from_maybe_* 走
  值+present 两标量参,**免新 AbiType/codegen 零改动**)。顺手修
  潜在分歧:实参路径原用 read_scalar 的 Maybe unwrap-abort,VM 语义
  是传 nil。dyn.truthy + Exit::Cond 全类型真值表(VM truthy:仅
  nil/false 假;`if (0)` 走 then!)。**坑**:全 Nil 入边同质 phi 建成
  `phi void`(非法 LLVM)→ Nil phi 强制 Dyn 宽化。
- **A2 返回 join→Dyn**(`ed284d1`):dyn_rets retriable(镜像
  DynLoopPhi 模式,入收敛快照);bare return/implicit-ret 装箱 nil。
  **真因修复**:recursive.lk 卡点不是异型返回,是 ret_types 默认 I64
  被自递归调用点读到 → 首个具体 return 类型即时发布进 sig.ret_types。
- **A3 Dyn 全局 + VM 路由 bug**(`7925690`):SetGlobal 混型/容器
  join→Dyn 槽(zeroinitializer {0,0}=nil tag,豁免 initialized 守卫);
  global_tys 入快照。**VM bug(既有)**:`let nums=[…]; fn f(){nums.len()}`
  被 lower_call_expr 当外部模块对象编成 GetIndex(list,"len") → executor
  必崩("expected Int, got ShortStr");map 全局同坑(读 nil 属性)。
  修复=collect_function_visible_let_names → user_let_globals 穿线
  5 个编译器构造点,分类判定排除用户 let。**注意**:VM 前端 checker
  拒绝混型全局赋值(变量类型不变式),混型全局只能经动态值产生——
  Dyn 全局的主要价值是容器/可空值跨函数。
- **A4 MapRest→MapStrDyn**(`56db77f`):map_h.str_dyn_without 一条
  ABI + 一臂。pattern_matching 翻转。
- 差分新组:differential_dyn_cross_function ×8 + differential_global_containers ×3。
- bench 观察:1.035/1.044/1.076/1.046x 波动(同二进制复测 ±3%,
  机器负载噪声;10% 门禁内)。

## 深覆盖收尾:阶段②(2026-07-07)

- **B1 Set**(`27ce681`):Ty::Set(ptr)+ lkrt lkset.rs(FxHashSet<RtKey>,
  键 Nil/Bool/Int/Str,Float 键 abort,迭代/display 不进子集)+ SetCtor
  builtin + 方法臂全套。**坑**:lower_builtin_call 尾部统一写 nil 结果
  (println/assert 约定),值返回 builtin 必须 early-return(Typeof 先例)
  ——SetCtor 结果被覆写 Nil,probe 差分抓出。
- **B2 模块小批**(同 commit):string/path 进 MODULE_GLOBALS;
  strip_prefix/suffix 返回 String?→DynVal;count 空模式=字节 len+1;
  capitalize/title unicode 状态机逐字节抄 stdlib;math sign 按类型分派;
  通用尾部 Bool 收窄;assert_eq 兜底=双侧 to_dyn_any→dyn.eq(深比较+
  数值 coercion=runtime_values_equal;absent Maybe=nil)。
- **B3 HOF 泛化**(`0ea88ca`):三 ABI 家族按 receiver 元素类型+lambda
  收敛签名选择;**dyn 家族对 map/reduce 强制 sig.dyn_rets.insert(fidx)
  → lambda 返回装箱 → 回调签名 fn(LkDyn,..)->LkDyn 成立**(A2 机制
  复用,retriable 转一圈收敛)。Dyn receiver 对 list-only 方法名预拆箱
  (as_list guard;与 str/map 共享名不入列表防误拆)。
  **dyn_lt/le/gt/ge 补双字符串字典序**——VM typechecker 拒静态 Str 比较
  但动态路径放行(number_compare_value string 臂),sort_search 排字符串
  时 native abort 抓出。sort_search 的 insertion_sort 同时吃 int 列表和
  str 列表(跨函数 join→Dyn),阶段①机制在此全链路兑现。
- 覆盖率 29→31/51;bench 1.003/1.010x。

## 深覆盖收尾:阶段③(2026-07-07)

- **C1**(`a32b530`):macro_system/imports.rs 拒 `..` 是 use.lk 连 VM
  都跑不了的真因(每条 use 都过宏导入扫描);放行父目录、绝对路径仍拒。
- **C2**(`5e99f07`):bundling 设计落地——依赖 entry 守卫(仅
  LoadFunction/SetGlobal/Return0)、fidx 重写仅 CallDirect.b/
  MakeClosure.b/LoadFunction.bx(**CallNamed.bx 是计数 payload 不是
  fidx,勿动**)、全局槽按名重映射(pc 不变故 facts 有效)。
  ImportEnv 挂在 SigInfer 上免穿线;文件命名空间绑定名=file_stem。
  string.len(模块)=字节长 ≠ .len()(方法)=char 数,新 byte_len ABI。
- 覆盖率 31→33/51;bench 1.007x。

## 深覆盖收尾:阶段④(2026-07-07)

- **VM 死循环 miscompile**(`ad39276`,本轮最重发现):lower_let/
  lower_define 把 let-循环缓存字面量直接别名到共享缓存寄存器;COW
  只对同深度赋值健全,嵌套循环内重赋值(min_idx=i)rebind 后内层
  回边读旧寄存器 → 静默死循环。word_count 在 VM 挂死却被差分语料
  当 VM-timeout skipped 放行多轮——**timeout-skip 是差分门禁盲区**。
  修复=删别名快路径(通用路径 Move-from-cache 保 hoisting 收益,
  bench 1.010x 零代价)。
- **D1 镜像**(`43aab93`):迭代序=f(键hash+操作序) 论题由
  order-conformance(lkrt dev-dep lk-core,64 键)首跑即过实证。
  lit 两段协议镜像 const_load 的 stage1(RtKey 按序插)+
  typed_map_from_entries(stage1 迭代序重建)。
- **D2/D3 连锁修复群**:str-map 键 Maybe unwrap;空 {} lookahead
  全函数扫描重构(寄存器成员随 Move 三集合维护——旧版被覆写寄存器
  不退场,r1 从 map 换 str list 后仍被当 map);HOF ret_known 门
  (pristine I64 默认误判成真失配,str lambda 被永久 Dyn 化);
  **phi 原地宽化删除,统一 DynLoopPhi retriable**(同 pass 内层
  if-join 已按旧型读参并装箱 → from_str 收到 {i64,i64} 的 clang
  类型错;预置化是唯一健全通路);fixpoint pass 上限计入发现数
  (match.lk 一度因预算耗尽回归)。
- 覆盖率 33→36/51;bench 1.010x。

## 深覆盖收尾:再修 G(try$call 原生化,2026-07-07)

- **G1**(`4fd02d7`):lkrt panic.rs——handler 栈(Box<JmpBuf> 512B,
  glibc _setjmp/_longjmp BSD 对)+ CURRENT_ERROR + rt.cell_*。
  raise 签名用 () 留在 ABI 词表(conformance 的 fn-ptr 强制不认 !)。
- **G2/G3**(`9b9aebe`):设计从「字节码内联」改为 **MIR TryCall 单
  指令 + codegen 文本 diamond**(codegen 可自由开 label,免 lower 的
  块结构手术);**运行时 cell 只在 try 边界物化**(SSA cell 模型的
  phi/loop 优化保留;物化=当前值装箱播种,调用后 cell_get 写回)。
  踩坑集:hybrid 测试的不可 lower 样本=try/catch,G 后全 lower 导致
  测试反转——换 trim() 动态格式串(**常量折叠会吃掉 "x"+"" 拼接**);
  cell 内容为 MapStrI64 时 to_dyn 不可装箱 → typed map→StrDynMap
  转换 ABI(迭代序=重放插入,保序);`!x` 语义≠truthiness(Bool 取反/
  Nil→true/其余 error)。LK_AOT_DEBUG_FAILURES=1 列出 final-pass
  全部失败函数——「callee 真因藏在 caller 瞬态 ret 检查后」是常态。
- 覆盖率 36→39/51;bench 0.998x。

## 深覆盖收尾:再修 H(并发原生化,2026-07-07)

- **H1**(`8c364cb`):chan.rs 进程级注册表(Mutex+双 Condvar,与
  thread_local arena 分离);OwnedVal 深拷贝(**map 条目按迭代序捕获、
  收方按序重放 → Fx 布局跨线程保序**,D1 论题的延伸应用)。
- **H2/H3**(`2b0fa77`):spawn 免 wrapper 合成——捕获全 join→Dyn 后
  签名统一,lkrt spawn0..4 按 arity trampoline;isolate=线程私有虚拟
  SSA 槽(spawned_isolate 集合;直接调用路径 cell 写仍拒,同函数两种
  语义并存时靠 reject 保护)。select 用 spin-poll(200µs park)——
  时序不进可观察契约,语料全部单臂就绪/default 确定性。
  连锁:module::member 全局名路由(chan::close 等两级导出)·
  Cmp (ListDyn, typed) 归一 dyn_eq · **空 [] 逃逸进 capture cell →
  猜 ListDyn**(闭包内 push 对 entry lookahead 不可见,typed 猜测
  跨函数 ping-pong——select eager-trace 场景)。
- 覆盖率 39→41/51;bench 0.986x。
