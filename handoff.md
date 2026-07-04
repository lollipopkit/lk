# Handoff

**目标(`/goal`)**:把 `plan.md` 划分为多步、逐个完成、每步 push。**用户:允许短期回归、fix-forward。**
细节台账在 `progress.md`。已推送 **113 commit** 到 `dev`。
**🎯 plan.md v1.0 定义六项全部达成(2026-07-04)**:VM 规范测试 ✓ · Tier 0 全覆盖+Tier 1 混合 ✓ · pcall 错误
模型 ✓ · 多实例嵌入 API ✓ · 三 profile(VM 核心裸机可编译)✓ · git 包管理 ✓。**本会话追加完成**:M2.5
stackless 四子步全部完成(①②③,④此前已提前落地)+ **协程/`yield`(plan.md 4.5 真正目标收益)三子步全部
完成**——LK 现已支持 Lua 风格的 `coroutine_create/resume/status` + `yield` 表达式。

## ✅ 已完成/大幅推进(遍及全部 6 相;M0–M4 五相 Exit 均达成,M4 为程序粒度口径)
- **Phase 0** 完整;**M3 完整**(嵌入 API + register_fn + 多实例 + 沙箱 builder + C ABI 端到端)。
- **M0–M1**:去全局状态 · lk-values/lk-hal 抽取 · lk-core VM 核心 `#![no_std]` flip · VM(source)==
  VM(bytecode) 差分 · `.lkm` 字节码缓存。
- **M2**(Exit 三项均有证据 + **M2.5 stackless 现已完整**):pcall/error · 一等错误值 · try/catch ·
  traceback · 三沙箱 · M2.7 验证器 fuzz · **M2.5 四子步全完成**(commits `5884829`/`4e86dd5`/`5e2432f`)——
  `CallDirect`/`Call`/`CallNamed` 从 Rust 递归改为堆上 `Vec<CallFrame>`,协程地基就绪。
- **M4**:Tier 0(`lk bundle`)· Tier 1 逐函数混合五子步 · 覆盖 14/50 · AOT==VM 差分门禁。
- **M5**:WASM · lk fmt · M5.4 删中心化注册表 · LSP 双轨 · M5.2 依赖手术(VM 核心过裸机 thumbv7em)。
- **协程/`yield`**(commits `a5f6725`/`5cf2a32`/本次):新 `HeapValue::Coroutine` + `Yield` opcode +
  `yield` 关键字语言层语法 + `coroutine_create/resume/status` 全局内建,详见下方「本轮」。
- 全量 **1454+ tests 0 失败**(核心 957)。

## 本轮完成:协程/`yield`(plan.md 4.5 真正目标收益)
M2.5 stackless 给出的 `CallFrame` 堆栈基础设施是直接使能条件——挂起的协程现在只是"一份 CoroutineState,
装着它自己的 frames/寄存器栈/pc",不需要 Rust 栈拷贝技巧。三子步(A 核心机制/B 语言语法/C AOT兜底+文档)
全部完成并逐步验证推送,细节见 `progress.md`「Post-v1.0 — 协程/yield」章节,用户可读文档在
`docs/coroutines.md`。

**关键设计/实测要点**:
- `Yield` opcode(106/128,7-bit 编码仍余 21 个槽位)单寄存器原地读写(yield 值 = 下次 resume 值的落点)。
- `Executor.active_coroutine` 只在 resume 专用 Executor 上设置,原生 re-entry(pcall/HOF 回调)永远不设置
  它——"跨原生调用边界 yield 报错"因此**零额外记账**天然成立,已用测试验证。
- **GC 踩坑并修复**:resume 期间把 resumer 自己的栈整体换出会让其活跃寄存器(含持有协程值本身的寄存器)
  在运行期间对 GC 不可见——`LK_GC_STRESS=1` 抓到 4 个测试失败,修复=新增 `Executor.extra_gc_roots`。
- 语言层 `yield expr` 只在表达式顶层合法(`yield a+b` = `yield (a+b)`,不支持嵌进 `1 + yield 2`),仿
  Rust nightly yield 的限制;任意函数可含 yield,非 resume 直接调用时运行时报错(非编译期)。
- **AOT 零新增代码**:`Yield` 天然命中 `aot/lower` 既有 `_ => Unsupported::Opcode` 兜底,`lk compile`
  自动回退 Tier 0 VM bundle,已验证产出可执行文件正确运行。
- `MODULE_ARTIFACT_VERSION` 7→8。

**验证**:workspace 全量 + `LK_GC_STRESS=1` 全绿 · clippy/fmt 0 · no_std 构建 0/0 · dist bench 门禁三轮
均不劣于基线(0.991x/0.988x)。conformance 语料 `examples/syntax/coroutines.lk` 自动纳入 VM==bytecode 与
VM==AOT 差分门禁。

## 剩余(深度架构工作,均已裁决)
- **[~] M4.2 AOT 深覆盖**:缺 mixed/动态类型系统,Tier 1 桥已供函数级出路,压力不紧迫。
- **✅ callable trait 反转**:裁决不做(留档)。
- **✅ M5.1/M5.2**:依赖手术完成。真机/QEMU demo、细粒度 float/unicode feature 建议不做。

## 护栏 & 续接
全量 1454+ tests 0 失败 / clippy 0 / fmt 0 / no_std 构建 0/0 / GC-stress 全绿 / bench 门禁全过。
**下一会话最连贯续接**:协程地基已就位,下一个自然的大步是把 `chan`/`task`(现有 tokio 异步并发)与
协程整合,或征询用户新方向;plan.md 4.5 的 select 语句语法(`Expr::Select`)在探索中发现是**已有前端解析
但零后端**的悬空构造(无 Instr lowering、无 runtime Channel 值类型、无 typecheck/resolve 处理)——如果
未来要做 channel-based select,那是另一块独立的、目前完全未开工的地基,不与本轮协程工作共享代码。
