# Handoff

**目标(`/goal`)**:把 `plan.md` 划分为多步、逐个完成、每步 push。**用户:允许短期回归、fix-forward。**
细节台账在 `progress.md`(33 步逐项状态)。本会话已推送 **35 commit** 到 `dev`,进展**触及全部 6 相**。

## 完成/推进(按 phase)
- **✅ Phase 0**(完整):plan → 33 步分解 + Caveats 就地核实。
- **M0**:🎯 **去全局状态里程碑**(G1/G2/G3 消除,G4/G5 按设计留)· **抽 `lk-values` crate + 真 no_std**(wasm32)·
  **`lk-hal`**(no_std)· CI no_std 冒烟。**剩 M0.7/8 = 抽 `lk-vm-core`**(从单体分离 VM 核心,多天)。
- **M1**:**VM(source)==VM(bytecode) 差分** · M1.1 conformance 声明 · M1.3 部分(.lkm 标注)。
- **M2**:**pcall/error** · **可捕获 assert** · **LK_FUEL 沙箱**。剩 M2.2(traceback)/M2.4(try 糖)/M2.5(stackless)。
- **✅ M3**(完整):**lk-api 嵌入 crate**(eval + 多实例隔离 + fuel + **register_fn**)+ **C ABI**
  (`lk.h` + `embed.c` 端到端跑出 `42`)。
- **M4**:**M4.3 AOT==VM 差分门禁**(现状核实,已在 CI + ASan/UBSan/fuzz)。剩 M4.1 Tier 0 / M4.2 Tier 1。
- **M5**:**M5.2 WASM**(lk-wasm 编到 wasm32,getrandom 修好,进 CI)· **M5.5 LSP+tree-sitter 双轨保留**。
  剩 M5.1 profile / M5.3 lk fmt / M5.4 去中心化 pkg(移除中心 registry)。

## 新 crate 布局(渐进接近 plan L0/L5 目标)
`values/`(lk-values L0 no_std)· `hal/`(lk-hal L0 no_std)· `api/`(lk-api L5,ffi feature + lk.h)。

## 方法论:fix-forward 攻破大改造
M0.1(crate 拆分)、M0.8(no_std,HashMap API 涟漪 typ→stmt→vm 逐点收敛)、M3 全套——**全部拆成
可收敛、green、可 push 的连贯子步**,35 commit 全程 workspace `-D warnings` 0/0、tests 0 失败,不推破碎态。

## 剩余(真正的多天工程,真实难度已勘清记入 progress.md)
- **M0.7/8** = 抽 `lk-vm-core`(单体 core 含 package/net/process/tokio 本质 std,不能整体 no_std;要抽 VM 核心)。
- **M2.2/2.4/2.5**(traceback 入 call 热路径 + 一等错误值需 GC rooting / try 糖 parser+lowering / stackless VM 重写)。
- **M4.1/4.2**(Tier 0 生成 cargo 工程+构建 / Tier 1 MIR 逐函数回退)· **M5.1/5.3/5.4**(profile / fmt AST pretty-printer / 删 registry)。

## 护栏 & 续接
全量 workspace tests 绿 / `-D warnings` 0 / fmt+clippy 0 / bench 非热路径不受影响 / C ABI 端到端验证。
**下一会话最连贯续接 = 抽 `lk-vm-core`**,复用本会话验证的渐进解耦法(解耦→分离→抽 crate→no_std)。
