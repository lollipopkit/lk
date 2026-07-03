# Handoff

**目标(`/goal`)**:把 `plan.md` 划分为多步、逐个完成、每步 push。**用户:允许短期回归、fix-forward。**
细节台账在 `progress.md`(33 步逐项状态)。本会话已推送 **28 commit** 到 `dev`,进展横跨 Phase 0 → M3。

## 已完成/推进(按 phase)
- **Phase 0** ✅ plan → 33 步分解 + Caveats 就地核实。
- **M0**:🎯 **去全局状态里程碑**(G1/G2/G3 消除,G4/G5 按设计留)· **M0.1 抽 `lk-values` crate**
  (val→typ 解耦 → 前端/运行时值模型分离 → 抽 crate)· **M0.2 `lk-hal`**(no_std)· **M0.8 lk-values 真 no_std**
  (wasm32 验证)· **M0.9 CI no_std 冒烟**(lk-hal+lk-values)。
- **M1**:M1.1 声明(examples 自验证 golden)· **M1.2 VM(source)==VM(bytecode) 差分**· M1.3 部分(.lkm 标注)。
- **M2**:**M2.1 pcall/error**· **M2.3 可捕获 assert**· **M2.6 fuel(`LK_FUEL`)**。
- **M3**:**M3.1 `lk-api` 嵌入 crate**(`Vm`+`eval`+`with_fuel`,多实例隔离测试证 M0 之效)· **M3.3 C ABI**
  (`ffi` feature:`lk_vm_new/eval/free`+`lk_string_free`)。

## 方法论:fix-forward 攻破大结构改造
M0.1(crate 抽取)、M0.8(no_std,HashMap API 涟漪 typ→stmt→vm 逐点收敛)、M3.1/M3.3——均拆成
**可收敛、可 push 的连贯子步**,全程 workspace `-D warnings` 0/0、1479+ tests 0 失败,不推破碎态。

## L0/L5 新 crate 布局(渐进接近 plan 目标)
`values/`(lk-values L0 no_std)· `hal/`(lk-hal L0 no_std)· `api/`(lk-api L5,ffi feature)· 其余 core/stdlib/aot/…

## 剩余(真正的多天/大工程,L0/嵌入快赢已用尽)
- **core 主体 no_std(M0.7 + M0.8 剩余)**:78k 行 + tokio + 102 use std,不可增量的大爆炸——**最大一块**。
- **M2.2**(traceback:`push_call_frame` 需入 call 热路径,perf 敏感)· **M2.4**(try/? 糖:parser+lowering)·
  **M2.5**(stackless VM 重写)。
- **M3.2**(register_fn/module:需 Value 转换 ergonomics 或 VM 支持 boxed 闭包 builtin,core 改)· M3.3 剩(cbindgen lk.h)。
- **M4**(AOT Tier 0 自包含 exe / Tier 1 逐函数回退)· **M5**(bare/alloc/full profile + MCU + `lk fmt` + git 去中心化 pkg)。

## 护栏
全量 workspace tests 绿 / `-D warnings` 0 / fmt+clippy 0 / bench 非热路径不受影响。
原则:按 phase;大步拆收敛子步、先跑通编译再迁逻辑;改 async/runtime/热路径用全量测试+端到端 .lk 核对。
