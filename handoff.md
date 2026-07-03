# Handoff

**目标(`/goal`)**:把 `plan.md` 划分为多步、逐个完成、每步 push。**用户指令:允许短期回归、fix-forward。**
细节台账在 `progress.md`(33 步逐项状态/发现/决策)。本会话已推送 **20 commit** 到 `dev`。

## 已完成/推进的步骤
- **Phase 0** ✅:plan → `progress.md` 33 步分解 + Caveats 就地核实。
- **🎯 M0「去全局状态」里程碑** ✅:G1/G3(死代码)、**G2**(tokio 全局→VmContext 的可共享
  `AsyncRuntimeHandle`,16 文件/9 crate 大改)消除;G4/G5(lkrt)按设计保留。→ core L1 无生产全局可变状态。
- **M0.2** ✅ `lk-hal` L0 crate(`#![no_std]`,wasm32 交叉编译验证)。
- **M0.1**(部分):val→typ 解耦;**RuntimeVal 迁移厘清**(前端 Type/LiteralVal 干净可 L0;运行时
  RuntimeVal/callable vm 纠缠)。
- **M1**:M1.1 声明(examples 自验证 golden)、**M1.2** ✅(`vm_bytecode_differential_test.rs`,41 比对/0 分歧)、
  M1.3 部分(.lkm 标注为内部产物)。
- **M2**:**M2.1** ✅ `pcall`/`error` 内建、**M2.3** ✅ assert 可捕获(改 Err)、**M2.6** 部分(`LK_FUEL=N` 暴露指令预算)。
  全程 1479 tests 0 失败、0 回归。

## 关键发现(codebase 比 plan 假设成熟 → 多步是「暴露已有能力」)
- M2 后端就绪:`Raise`/`TryBegin`/`TryEnd`/`ErrorHandler`/`ErrorVal{message,trace}`。
- **fuel 已实现**(`execute_program_with_ctx_and_budget`,wasm 在用)→ M2.6 经 `LK_FUEL` 暴露。
- 无 `try`/`catch` 前端(`?` 仅 optional 类型);无 `lk fmt` 格式化器;包管理是中心化 registry(M5.4 要移除)。

## 🎉 M0.1 突破(本会话 fix-forward 攻破关键路径)
**`lk-values` L0 crate 已抽取**(commit a702c88):前端值/类型模型(LiteralVal/Type/ShortStr/
ShortStrOrStr/FunctionNamedParamType/NumericClass/NumericHierarchy)独立成 crate,`core::val` 经
`pub use lk_values::{…}` 再导出→全 core 路径不变。workspace `-D warnings` 0/0、wasm32 编译、tests 全绿。
- 前置子步(均已 push):val→typ 解耦 → 分离前端/运行时值模型 → 抽 crate。
- **范围收敛**:L0 装**前端/编译期模型**(干净);**运行时模型**(RuntimeVal/HeapValue/CallableValue+资源句柄)
  因内嵌 vm callable 留 core。

## 剩余(均多小时/多天专注工程)
- **lk-values 真 no_std**(M0.8,清晰下一步):`#![no_std]`+alloc;障碍已勘清——`std::fmt`→`core::fmt`、
  `std::sync::Arc`→`alloc::sync::Arc`、`std::str`→`core::str`(机械);`std::collections::HashMap` 2 处
  (含 `Type::substitute` 公共 API 参数,9 调用点)→ hashbrown(有涟漪);`use anyhow::Result` 疑似死 import。
- **callable trait 反转(A)**:让运行时模型也能 L0——深改 call 热路径+GC(`RuntimeCallable` 内嵌
  `Module`+`RuntimeModuleState`,executor 热路径直接访问),硬阻塞。
- **M0.7–9** no_std 化 78k 行 core(tokio + 102 use std 需 feature-gate)。
- **M2.2**(error 载一等值 + traceback:`push_call_frame` 未接入执行,需入 call 热路径,perf 敏感)、
  **M2.4**(try/? 糖:parser+lowering)、**M2.5**(stackless VM 重写)。
- **M3**(embed API + C ABI + 多实例 + 内存上限/模块白名单沙箱)、**M4**(AOT Tier 0 自包含 exe / Tier 1 逐函数回退)、
  **M5**(bare/alloc/full profile + MCU + `lk fmt` + git+lockfile 去中心化包管理)。

## 护栏
全量 workspace tests 绿(现 1479)/ fmt+clippy 0 / bench 非热路径不受影响。原则:按 phase 顺序;大步先拆子步、
先跑通编译再迁逻辑;改 async/runtime/热路径用全量测试+端到端 .lk 核对。
