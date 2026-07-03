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

## 下一步:Phase M0 续(结构性大步)
- **M0.1 抽 `lk-values`**(把 `RuntimeVal`/`LiteralVal`/`HeapValue`/`HeapStore`/GC 类型移出 core
  到 `no_std`+`alloc` 新 crate,**与进行中的 RuntimeVal 迁移合流**)——大步,先拆子步、先跑通编译再迁。
- **M0.2 抽 `lk-hal`** trait;**M0.7/M0.8** core 换 `core::`/`alloc::` + `#![no_std]`;**M0.9** CI alloc-only+wasm32。
- 原则:严格按 Phase 顺序;大步先拆子步、先跑通编译再迁逻辑;改动 async/runtime 用全量测试+端到端 .lk 核对。

## 护栏(每步 exit gate,不回退基线)
全量 workspace tests 绿(现 1478)/ 三套差分 / fuzz / ASan+UBSan / Miri lkrt /
fmt+clippy 0 / AOT bench checksum、**VM/Lua≈1.008x、AOT/LK≈0.259x**(G2 非热路径,bench 不受影响)。

## git(dev,每步 commit+push)
已推送:a4ad78a(G1)、2ac2bd5(G3)、31e2117+c25e94b(docs)。本轮 G2 待提交推送。
