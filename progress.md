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

- [~] **M0.1** 抽 `lk-values`：把 `RuntimeVal`/`LiteralVal`/`HeapValue`/`HeapStore`/GC 类型移出 core
      到新 crate（`no_std`+`alloc`）。**与进行中的 RuntimeVal 迁移合流。**
      - [x] **解耦 val→typ**：`NumericClass`/`NumericHierarchy`（只依赖 `Type`，本就属于它）从 `typ`
        移进 `val`；`typ` 改从 `val` 再导出（`crate::typ::Numeric*` 向后兼容，免改 type_checker）。
        core 0/0、950 tests。val（生产码）不再依赖 typ。
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
- [ ] **M0.7** core 机械换 `core::`/`alloc::`（fmt/ops/mem/cmp/pin/collections），std-only 路径 feature-gate。
- [ ] **M0.8** `lk-vm-core` 加 `#![no_std]` + `extern crate alloc`，std 部分门控到 `std` feature。
- [ ] **M0.9** CI 加 `--no-default-features --features alloc` 编译 + `wasm32-unknown-unknown` build 冒烟。
- **Exit**：alloc-only 编译通过；`wasm32` build 通过；grep 断言无生产全局可变状态。

## Phase M1 — VM 定规范 + conformance + 差分框架（问题 1、3、8）

- [ ] **M1.1** conformance suite 骨架：每语言特性一组 golden（仿 Lua/Wren/mruby）。
- [x] **M1.2** `VM(source)==VM(bytecode)` 差分测试入 CI。`cli/tests/vm_bytecode_differential_test.rs`
      (不依赖 llvm):对 examples 语料,源码跑 vs `compile bytecode`→`.lkm`→跑,比对 stdout/success;
      「源码跑两次」自动过滤非确定性样例。**41 比对 / 0 分歧 / 3 跳过**——ModuleArtifact 序列化往返语义一致。
- [ ] **M1.3** `.lkm` 降级为 `$LK_HOME/cache` 编译缓存（源哈希失效）；CLI 停止宣传 `.lkm` 作分发。
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
- [ ] **M2.2** 错误为一等值（可携带任意 lk 值）+ 栈展开前采集结构化 traceback。
- [x] **M2.3** fatal guard 可 `pcall` 捕获 —— **基本达成**。调查+改动:**除零**本就是可捕获 Err;
      **assert/assert_eq/assert_ne** 从 Rust `panic!`(abort,不可捕获)改为返回 `Err`(可捕获,
      未捕获仍非零退出且**消除 panic backtrace 噪声**);**缺键/越界**返回 nil(非 fatal,无需捕获);
      **panic** 保持故意 fatal(`error()` 是可捕获替代)。**验证**:pcall 捕获 assert/除零;未捕获 assert
      exit=1「VM execution failed」;**全量 1479 tests / 0 failed(0 回归)**。
- [ ] **M2.4** `try`/`?` 语法糖。
- [ ] **M2.5** VM 改 stackless（trampoline `Sequence::step`）——大工程，落地时再拆子步。
- [ ] **M2.6** fuel（`step(fuel)` 中断）/ 内存上限（allocator 计账）/ 模块白名单。
- **Exit**：`pcall` 捕获所有可恢复错误；fuzz 验证器无 panic；沙箱指标可配。

## Phase M3 — 嵌入 API + 多实例 + C ABI（问题 10）

- [ ] **M3.1** `lk-api`：`Vm::builder()`、`register_fn`/`register_module`、rooted handle。
- [ ] **M3.2** 多实例隔离：每 `Vm` 独立堆/驻留表/注册表，无 `thread_local`（依赖 M0）。
- [ ] **M3.3** `ffi` feature + cbindgen 生成 `lk.h`（Dart FFI 示例）。
- **Exit**：示例宿主并存 2 个隔离 VM；C ABI 冒烟；无实例间可变共享。

## Phase M4 — AOT Tier 0 + Tier 1（问题 2、6）

- [ ] **M4.1** Tier 0：`lk compile` → 字节码 + 静态链接 VM 单文件（100% 覆盖，语义平凡一致）。
- [ ] **M4.2** Tier 1：MIR 后端 `Unsupported` 从「整程序失败」→「逐函数标记 VM-executed 回退」。
- [ ] **M4.3** 差分门禁 `AOT==VM` 固化进 CI（现有部分保留强化）。
- **Exit**：任意 `.lk` 可 `lk compile`（Tier 0 保底）；覆盖 >11/44 且失败构造回退 VM 而非报错；差分全绿。

## Phase M5 — no-std profiles + 工具链收敛 + v1.0（问题 7）

- [ ] **M5.1** `bare`/`alloc`/`full` 三 profile 打通（feature 矩阵）。
- [ ] **M5.2** WASM demo 可跑 + 一类 MCU（ESP32/Cortex-M+alloc）冒烟。
- [ ] **M5.3** `lk fmt`。
- [ ] **M5.4** 包管理缩减为 git+lockfile 去中心化依赖（砍中心化注册表/keyring/`lk pkg serve`）。
- [ ] **M5.5** LSP **保留并持续维护**（不砍）+ tree-sitter 完善。
- **Exit**：CI 矩阵全绿；v1.0 定义达成。

---

## 执行原则
1. 严格按 Phase 顺序；M0 是所有后续的地基（问题 1/5/8/9），风险最低收益最高。
2. 每步独立可验证：动代码即跑对应测试 + 不回退 `cargo test --workspace` 与 bench 门禁。
3. 大步（M0.1 抽 crate、M2.5 stackless）落地前先拆子步、先跑通编译再迁移逻辑。
4. 渐进抽离 crate，绝不一次性重排整个 workspace。
5. 每轮结束更新本文件勾选项 + `handoff.md`。
