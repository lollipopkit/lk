# Handoff

**目标(`/goal`)**:把 `plan.md` 划分为多步、逐个完成、每步 push。**用户:允许短期回归、fix-forward。**
细节台账在 `progress.md`。已推送 **117 commit** 到 `dev`。
**🎯 plan.md v1.0 定义六项全部达成(2026-07-04)**:VM 规范测试 ✓ · Tier 0 全覆盖+Tier 1 混合 ✓ · pcall 错误
模型 ✓ · 多实例嵌入 API ✓ · 三 profile ✓ · git 包管理 ✓。**Post-v1.0 已追加**:M2.5 stackless 四子步 ✓ ·
协程/`yield` 三子步 ✓ · **`sched` 协作式调度器(chan/task × 协程整合)三子步 ✓(本轮)**。

## ✅ 已完成/大幅推进(遍及全部 6 相;M0–M4 五相 Exit 均达成)
- **Phase 0** 完整;**M3 完整**(嵌入 API + 多实例 + 沙箱 + C ABI)。
- **M0–M1**:去全局状态 · lk-values/lk-hal 抽取 · VM 核心 `#![no_std]` · 差分门禁 · `.lkm` 缓存。
- **M2 + M2.5**:pcall/error · 一等错误值 · traceback · 三沙箱 · 验证器 fuzz · stackless
  `Vec<CallFrame>`(commits `5884829`/`4e86dd5`/`5e2432f`)。
- **M4**:Tier 0(`lk bundle`)· Tier 1 逐函数混合 · AOT==VM 差分门禁。**M5**:WASM · lk fmt ·
  删中心化注册表 · LSP 双轨 · 依赖手术。
- **协程/`yield`**(commits `a5f6725`/`5cf2a32`/`9e98057`):`HeapValue::Coroutine` + `Yield` opcode +
  `yield` 关键字 + `coroutine_create/resume/status` 全局。
- **`sched` 调度器(本轮,commits `c6057c1`/`3560927`/`b077544`/本次)**:见下。
- 全量 **1498+ tests 0 失败**(核心 959)。

## 本轮完成:`sched` 协作式协程调度器(chan/task × 协程整合)
native 不能 yield(结构性限制)⇒ Go 式隐式挂起不可行 ⇒ **yield-descriptor 模式**:
`sched.recv/send/sleep/pause/spawn/join/await` 只构造等待描述符,`yield sched.recv(c)` 显式挂起,
`sched.run(...fns)` 驱动 N 协程 round-robin、全员 parked 时 tokio select 阻塞等待。
用户文档 `docs/coroutines.md`(sched 章节)· 语料 `examples/stdlib/sched_demo.lk`(已进差分门禁)·
细节台账 progress.md「`sched` 协作式调度器」章节。

**关键实测要点**:
- **修复上轮遗留 GC bug**(commit `3560927`):死协程 Done/Errored 清 stack 未清 stack_top,
  `gc_edges` 切片越界 panic——"死了但句柄仍被引用"即触发,stress 下 sched 测试确定性抓到。
- `resume_coroutine_runtime` 增加 `extra_roots` 参数:调度器 Rust 局部工作集必须显式进 GC root。
- `Runtime::take_task`:取出 JoinHandle 所有权,`&mut` 跨 select 轮 cancel-safe(join_task 不行)。
- join-only 死锁可证明 → 报错;channel/await 阻塞合法(外部 tokio task 可投递,同 Go)。
- `tokio::time::sleep` 必须在 `block_on` 内创建(runtime 上下文外创建即 panic)。
- 非描述符 yield 报错(生成器风格协程归裸 resume 管);协程句柄不可穿 channel(深拷贝边界)。

**验证**:workspace 1498+ 全绿 · `LK_GC_STRESS=1` 全绿 · clippy/fmt 0 · no_std 0/0 ·
dist bench 门禁 1.007x(基线内)· 差分门禁(examples+bytecode)含新语料全过。

## 剩余(均已裁决/留档)
- **[~] M4.2 AOT 深覆盖**:缺 mixed/动态类型系统;Tier 1 桥已供出路,不紧迫。
- **既有断点(留档,本轮发现)**:全局 `spawn(闭包)` 不工作(闭包无 promote 到 CallableValue::Runtime
  的路径,`task.spawn_blocking` 同因 bail,顶层即复现)——若修需 Arc state 快照机制,独立工作项。
- **✅ 裁决不做**:callable trait 反转 · 真机/QEMU demo · 细粒度 float/unicode feature。
- **`select` 语法**:前端已 parse、后端为零(compiler 报 "does not support expression yet: Select";
  `select$block` native 是老孤儿)。**现在 sched 原语已就位,select 可考虑 desugar 到 sched/chan 原语**,
  是下一个自然候选。

## 护栏 & 续接
全量 tests 0 失败 / clippy 0 / fmt 0 / no_std 0/0 / GC-stress 全绿 / bench 与差分门禁全过。
**下一会话候选**(按连贯性排序):① `select` 语句 lowering(desugar 到 sched/chan,地基刚好齐了);
② 修全局 `spawn(闭包)` 断点;③ 征询用户 v2 新方向。
