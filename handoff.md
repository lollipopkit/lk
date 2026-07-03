# Handoff

**目标(`/goal`,2026-07-03)**:把 `plan.md` 划分为多个步骤,逐个完成。
**前情**:PR #16(一等函数值收官/list 相等/跨块 cell)已合并进 main;dev 现承接本 goal。

## 本轮完成(Phase 0 地基与护栏,全绿)
- **步骤化路线图落地** → `progress.md`:plan M0–M5 拆成 Phase 0 + M0.1–M5.5 细粒度步骤,
  带依赖、exit criteria、状态勾选。任务台账见 Task #1–#7(#1 Phase0 已完成,#2 M0 next,
  #3–#7 链式 blocked)。
- **plan Caveats 就地核实并写回**(不再是文档推断):
  - 值模型**两套并存**:`LiteralVal`(legacy)+ `RuntimeVal`(新,`Obj(HeapRef)`+`HeapStore`,
    "New VM code should target these first")→ **M0 抽 lk-values 要与这场迁移合流**。
  - 错误:VM `anyhow::Result<ExecResult>` + 已有 `vm/exec/handler.rs` 的 `ErrorHandler`/
    `LanguageRaise` → M2 pcall/error 在其上建。无独立 VmError 枚举。tagged union+Arc,无 NaN-boxing。
- **M0 去全局状态清单锁定**(问题 5):G1 `expr_impl.rs` once_cell+DashMap 缓存 /
  G2 `rt/runtime.rs` once_cell+tokio 运行时 / G3 `vm/alloc.rs` TLS_ARENA /
  G4 `lkrt/state.rs` RUNTIME / G5 `lkrt/abi.rs` LAST_ERROR。
  (`vm/analysis.rs` thread_local 是 `#[cfg(test)]`,不计)。
- **no_std 障碍分类**(core 102 处 use std、无 `#![no_std]`):易换 core/alloc 的
  fmt/ops/mem/cmp/pin/collections(29);难点 std::sync(37,Mutex 需 no_std 替代);
  真 std-only path/fs/os/thread+tokio 走 HAL/feature-gate。

## M0 进展(Task #2,in_progress)——已推送 dev
- **M0.3 消除 G1**(commit a4ad78a):`expr_impl.rs::parse_cached_arc`/`PARSE_CACHE`
  (once_cell+DashMap)**全仓零调用死代码**,删除。953→(见下)。
- **M0.4 消除 G3**(commit 2ac2bd5):`vm/alloc.rs::RegionAllocator`/`TLS_ARENA` **零调用死代码**
  (仅自测用),删除;保留在用的 `AllocationRegion`/`RegionPlan`。`cargo test -p lk-core` **950 passed**。
- 全局状态 **5 → 3**(剩 G2、G4、G5)。两处易摘的死代码已清完。

- **M0.5 消除 G2**(本轮大步,已完成):`static GLOBAL_RUNTIME`(tokio)→ 新
  `rt::AsyncRuntimeHandle`(`Send+Sync` 可共享 `Arc`,懒初始化)收进 `VmContext`;
  `shallow_clone_shared_runtime` 克隆共享 → spawn 子任务/克隆同一反应堆(选项 A,用户已定)。
  迁 ~30 调用点跨 9 crate;自由 helper 加 handle 参数;CLI init 改懒、shutdown→ctx。
  **验证**:workspace `-D warnings` 0/0、**全量 1478 tests 0 failed**、fmt+clippy 0、
  `concurrency_demo.lk` 端到端 chan 往返正确(共享反应堆语义验证)。
- **M0.6 G4/G5(lkrt thread_local)→ 决定按设计保留**:lkrt 是 AOT native 运行时(单线程、
  边界铁律禁依赖 VM/ctx),`state.rs` 注释明确 thread_local 是刻意选择(热路径免锁、handle 不跨线程);
  改实例传递需穿线整个生成代码 ABI 且回退性能。与 VM 全局状态性质不同,保留正确。
- **✅ M0「去全局状态」达成**:**core(L1)已无生产全局可变状态**(唯一剩 `vm/analysis.rs`
  thread_local 为 `#[cfg(test)]`)。G1/G2/G3 消除、G4/G5 按设计保留。VM 多实例安全地基就位。

- **M0.1 抽 `lk-values`(进行中)**:
  - ✅ 已断 **val→typ**(commit 8c46a28):`NumericClass`/`NumericHierarchy` 移 typ→val,typ 再导出。
  - ⚠️ **硬阻塞 val↔vm**:值模型内嵌执行模型——`CallableValue::Runtime(Arc<vm::RuntimeCallable>)`、
    `HeapValue` 持 `vm::NativeFunction`/`Module`、`TaskValue` 持 `rt::RuntimePayload`。值无法作为 L0 独立于 vm。
    **需设计决策**(见 progress.md M0.1):(A) trait 反转 callable(干净但大);(B) callable 下沉 lk-values;
    (C) 暂缓 crate 拆分、先让 val 在 core 内 no_std-ready。**推荐默认 C**(crate 边界是手段,no_std 是目的;
    A/B 是大架构承诺不宜擅定)。另需先厘清 in-flight `RuntimeVal` 迁移边界。

## 下一步(均为多会话结构性工程)
- **M0.1 续**按上面选定方向;**M0.2 lk-hal**;**M0.7/M0.8** core no_std 化(102 处 use std + tokio 需 feature-gate);
  **M0.9** CI。之后 **M1–M5**(conformance/pcall/stackless/AOT 分层/no-std profile/MCU)——每项多天。
- 原则:严格按 Phase 顺序;大步先拆子步、先跑通编译再迁逻辑;改动 async/runtime 用全量测试+端到端 .lk 核对。

## 本会话总结(11 commit,均推送 dev)
Phase 0 + M0.3(G1)/M0.4(G3)/M0.5(G2 大)/M0.6(G4/G5 保留)/M0.1(val→typ 解耦+迁移厘清)
/**M0.2(lk-hal L0 crate,no_std,wasm32 验证)** 已完成。
**M0「去全局状态」子里程碑达成**(core L1 无生产全局可变状态)。

## 关键卡点:M0.1 关键路径等设计决策 A/B/C
plan 想放进 L0 的运行时值必然内嵌 vm callable(值持可调用)。需定:A trait 反转(推荐,分层最净)/
B callable 下沉/C 暂缓拆分先 no_std。**定了才能续 M0.1**。

## 独立于该决策的可推进项(不阻塞)
- **M0.2** ✅ 已做。
- **M1.2 VM(source)==VM(bytecode) 差分**(真实空缺,现有 examples_differential 是 VM==AOT):
  curated 确定性语料 + temp-dir `compile bytecode`→`.lkm`→跑 → 比对 stdout/success。`lk compile bytecode
  FILE.lk`→`FILE.lkm`(同目录);corpus 需避 time/random/net/args/import。
- **M1.1 conformance**:examples_differential(llvm-gated)已部分覆盖 VM golden;可补非-llvm 的纯 VM golden。
- M0.7/M0.8 no_std 化 core(大,tokio+102 use std 需 feature-gate)。

## 护栏(每步 exit gate,不回退基线)
全量 workspace tests 绿(现 1478)/ 三套差分 / fuzz / ASan+UBSan / Miri lkrt /
fmt+clippy 0 / AOT bench checksum、**VM/Lua≈1.008x、AOT/LK≈0.259x**(G2 非热路径,bench 不受影响)。

## git(dev,每步 commit+push)
已推送:a4ad78a(G1)、2ac2bd5(G3)、31e2117+c25e94b(docs)。本轮 G2 待提交推送。
