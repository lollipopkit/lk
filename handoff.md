# Handoff

**目标(`/goal`)**:把 `plan.md` 划分为多步、逐个完成、每步 push。**用户:允许短期回归、fix-forward。**
细节台账在 `progress.md`。已推送 **106 commit** 到 `dev`。完成度:**✅36 · [~]2 · [ ]2 · [!]1**
([~] M4.2 覆盖、M5.1;[ ] Tier 1 混合、M2.5;[!] callable)。
**🎯 plan.md v1.0 定义六项全部达成(2026-07-04)**:VM 规范测试 ✓ · Tier 0 全覆盖+Tier 1 混合 ✓ · pcall 错误
模型 ✓ · 多实例嵌入 API ✓ · 三 profile(VM 核心裸机可编译)✓ · git 包管理 ✓。剩余项全为 post-v1.0,
均已数据驱动裁决并留档(见「剩余」)。近几轮:Tier 1 五子步 · 深递归修复(bench 1.012x)· M2.7 验证器
fuzz · **M5.2 依赖手术(VM 核心编译过 thumbv7em 裸机,crate graph 无 std)** · 死依赖清理 ×5。

## ✅ 已完成/大幅推进(遍及全部 6 相;M0–M4 五相 Exit 均达成,M4 为程序粒度口径)
- **Phase 0** 完整;**M3 完整**(嵌入 API + register_fn + 多实例 + 沙箱 builder + C ABI 端到端 + `eval_value`)。
- **M0**:🎯 去全局状态 · lk-values/lk-hal 抽取(真 no_std,wasm32+thumbv7em CI 冒烟)· **lk-core VM 核心
  `#![no_std]` flip**(`--no-default-features` 真 no_std 构建 0/0,CI 守卫)。
- **M1**:VM(source)==VM(bytecode) 差分 · conformance 声明 · `.lkm` 字节码缓存(坐实缓存非分发)。
- **M2**(Exit 三项均有证据):pcall/error · 可捕获 assert · **一等错误值(含 String/List 堆对象,GC-root pin
  跨展开)** · try/catch · traceback(命名调用链,仅 Err 路径零热成本)· fuel+内存+模块白名单三沙箱 ·
  **M2.7 验证器 fuzz(本轮新)**:畸形 `.lkm` 三路生成器+定向敌意语料,断言 decode+verify 只 Err 不 panic,
  本地 2 万例 0 panic,correctness.yml 挂 5 万例 scaled + run-id 种子 2 万例。
- **M4**:Tier 0(`lk bundle` 自包含 ELF)· **程序粒度回退**(native 失败 warn+回退 Tier 0,任何有效程序可
  compile,`LK_AOT_NO_FALLBACK=1` 关)· 覆盖 **14/50**(8 个 opcode win,clean「复用现有类型」win 已穷尽)·
  AOT==VM 差分门禁(CI+ASan/UBSan+fuzz)。
- **M5**:WASM(wasm32+CI)· lk fmt · M5.4 删中心化注册表(-5000 行,git+lockfile)· LSP 双轨。
- 新 crate:`values/`(L0)· `hal/`(L0)· `api/`(L5,ffi+lk.h)。全量 **1453 tests 0 失败**(核心 943+新 fuzz 2)。

## 本轮完成 ①:M2.7 字节码验证器 fuzz ✅(M2 Exit「fuzz 验证器无 panic」证据闭合)
`core/src/vm/verify_fuzz_tests.rs`(commit `9d7fedd`):字节级破坏/JSON 结构感知变异/随机垃圾三路 +
定向敌意语料(entry 越界、指令字全 1、寄存器数清零、fact 表长度炸弹、深嵌套 JSON),断言
`from_json_str`→`into_module`→`verify_module` 只 Err 不 panic。**M2 Exit 三项(pcall/fuzz/沙箱)均有证据**
(M2.5 stackless 是超出 Exit 的 deliverable)。

## 本轮完成 ②:Tier 1 逐函数混合设计定稿 ✅(实现未动,commit `83c8b4a`)
实测绘 lower(final pass 单点 `?`)/abi(lkrt 一致性表)/codegen/链接(clang+liblkrt.a)/cli 五处后,
`docs/llvm/tier1-hybrid.md` 定稿:**单向 native→VM 桥、桥居 lk-api(lkrt 铁律不破)、lowered 代码内不出现
VM/动态值表示(VM 链接期经 wrapper 进入)**。backend.md(含其「no VM bridge」禁令)已按用户裁决整体删除。
v1 资格=调用点标量参数+结果
全废弃(dead_writes)+传递闭包无用户 globals+无 captures;不满足感染调用者,及 entry 回退 Tier 0。
硬约束:stdio flush 顺序(lkrt C 缓冲 vs VM Rust 行缓冲)、未捕获错误 abort 对齐、artifact 复用。
**5 个可提交子步**:① lk-api hybrid 运行时+单测 → ② lower 标记+资格分析+MIR 快照 → ③ codegen declare+
桥调用+.ll 快照 → ④ cli 混合链接+端到端差分 → ⑤ fuzz 生成器扩展。

## 剩余(深度架构工作)
- **✅ M4.2.2 逐函数 Tier 1 混合:五子步全部完成**(commits `2e19e94`/`e194d11`/`27745be`/`2427323`)。
  `LK_AOT_HYBRID=1`:不可 lower 的合格函数跑嵌入 VM,混合 exe 输出与 VM 逐字节一致(含跨 stdio 顺序)、
  uncaught 错误行为对齐;fuzz 生成器半数程序带 hybrid 帮手,800 例 0 分歧(首轮即抓到并修掉 flush 打错
  缓冲区的真顺序 bug:C printf 缓冲 vs lkrt Rust stdout → 改 fflush(NULL))。资格 v1=标量参数+结果废弃
  (dst 不绑定,被读即回退)+子树 global-free+无 captures。**默认仍 opt-in**,correctness.yml 数轮全绿后翻。
- **[~] M4.2 AOT 深覆盖**:clean opcode win 已穷尽;剩余全撞同一根(缺 mixed/动态类型系统:mixed 常量、
  ToIter map 迭代=[key,value] mixed pair、动态 operators)或需原生 try/catch(解锁 pcall/error,高价值)或
  动态分派——Tier 1 桥落地后,这些函数可先走 VM-executed,压力大减。
- **[~] M2.5 stackless**:设计定稿 + **子步④提前落地**(`238324f`):分段栈+可捕获深度上限+traceback 截断,
  深递归从「~150 帧 abort」变为「20 万层可跑、超限可 pcall」,bench 1.012x(噪声级过门禁)。
  **数据驱动建议:①-③(Frame-Vec 重写)缓做**——洞已关、只剩协程地基收益、门禁风险高;协程排期时再启。
  设计保留于 `docs/vm-stackless.md`。**待用户裁决是否接受缓做。**
- **✅ callable trait 反转:裁决不做(留档)**——no_std 动机已被单体 no_std 化+裸机编译完全满足,
  反转只剩分层纯洁性收益,成本是热路径 dyn 分派+原子重构;未来有真实 L0 运行时值消费场景再重估。
- **✅ M5.1/M5.2**:依赖手术完成(`db5b376`),**VM 核心全量编译过 thumbv7em 裸机**,CI 守卫固化。
  遗留 nice-to-have:真机/QEMU demo 固件(allocator+panic handler+HAL 接线);细粒度 float/unicode feature
  **建议不做**(收益仅无 FPU MCU,成本=Float 遍布 VM 的 cfg 面)。

## 护栏 & 续接
全量 1453 tests 0 失败 / clippy(CI 口径,无 --tests)/ fmt / no_std 构建 0/0 / bench 不受影响(本轮零 VM
热路径改动:fuzz 是纯测试,Tier 1 是纯设计文档)。
**下一会话最连贯续接**:Tier 1 子步⑤(fuzz 生成器扩展 + 默认开关裁决)收尾 M4.2.2;其后 M2.5 stackless 或
M5.1/M5.2。实现细节与代码锚点全在 progress.md 的 Tier 1 小节。
