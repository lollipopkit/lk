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

## 下一步:G2 是「大步」(勘察已记 progress.md M0.5)
**G2** `rt/runtime.rs::GLOBAL_RUNTIME`(tokio 执行器)是**活代码**,~30 调用点跨 9 crate。
基础设施部分就位(native 函数已可 `NativeRuntime::ctx() -> &VmContext`),但落地需:
VmContext 加 `Arc<tokio::Runtime>` + 迁 30 调用点 + 处理无 ctx 分支 + 全量 async 测试/perf。
**并含设计权衡**:每 VM 独立 tokio 反应堆 vs 进程共享反应堆(仅 VM 堆/状态隔离)——
影响 M3 isolate 语义,需定夺。其后 G4/G5(lkrt)、M0.1 抽 `lk-values`(与 RuntimeVal 迁移合流)
均为大步,须先拆子步、先跑通编译再迁逻辑。

## 护栏(每步 exit gate,不回退基线)
workspace 95 套 / 三套差分(手工 13 组)/ fuzz 7 种子 / ASan+UBSan / Miri lkrt 25 /
fmt+clippy 0 / AOT bench 20/20 checksum、**VM/Lua≈1.008x、AOT/LK≈0.259x**。
原则:严格按 Phase 顺序;渐进抽离 crate 不重排 workspace;动代码即跑对应测试+全量门禁。

## git
在 dev(6427901「md」= plan.md 改动)。plan.md 本轮又改了 Caveats(已核实事实),
progress.md 新建、handoff.md 刷新——待与用户确认后 commit。
