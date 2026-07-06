# Handoff

**目标(`/goal`)**:把 `plan.md` 划分为多步、逐个完成、每步 push。**用户:允许短期回归、fix-forward。**
细节台账在 `progress.md`。已推送 **119 commit** 到 `dev`。
**🎯 plan.md v1.0 定义六项全部达成(2026-07-04)**。**Post-v1.0 已追加**:M2.5 stackless ✓ ·
协程/`yield` ✓ · `sched` 协作式调度器(chan/task × 协程整合)✓ · **`select` 语句 lowering ✓(本轮)**。

## ✅ 已完成/大幅推进(遍及全部 6 相;M0–M4 五相 Exit 均达成)
- **Phase 0** 完整;**M3 完整**(嵌入 API + 多实例 + 沙箱 + C ABI)。
- **M0–M1**:去全局状态 · lk-values/lk-hal 抽取 · VM 核心 `#![no_std]` · 差分门禁 · `.lkm` 缓存。
- **M2 + M2.5**:pcall/error · 一等错误值 · traceback · 三沙箱 · 验证器 fuzz · stackless `Vec<CallFrame>`。
- **M4**:Tier 0 + Tier 1 逐函数混合 · AOT==VM 差分门禁。**M5**:WASM · lk fmt · 删注册表 · LSP 双轨。
- **协程/`yield`** + **`sched` 调度器**(commits `a5f6725`…`a36ca6d`):`yield` 关键字 + coroutine_*
  全局 + yield-descriptor 调度器(`sched.recv/send/sleep/pause/spawn/join/await` + `sched.run`)。
- **`select` 语句(本轮,commits `ad02dfe`/本次)**:见下。
- 全量 **1508+ tests 0 失败**(核心 960)。

## 本轮完成:`select` 语句 lowering(原"前端 parse 通过、后端为零"的悬空构造)
与 try/catch → pcall 同款 **parse 时 desugar**:操作数/守卫按源序急切求值进 `__select{n}_*` 合成
局部变量,调用 `select$block` 老 native(tokio SelectOperation),Conditional 链分派 case body。
resolver/typecheck/compiler/AOT **零专用代码**;`Expr::Select`/`SelectCase`/`SelectPattern` 整个删除,
**compiler 的"不支持表达式"兜底分支从此不可达并删除**(每个 Expr 变体都有 lowering)。
用户文档 docs/coroutines.md「The select statement」· 语料 `examples/syntax/select.lk`(差分门禁)·
细节 progress.md「`select` 语句 lowering」章节。

**语义定案**(全部有测试钉住):急切求值(Go 规则)· 守卫先于 binding(真值归一化 Bool)·
binding=接收值(closed→nil)· case body 单表达式(同 match arm)· 无 default 阻塞线程(sched 协程内
禁用,park 用 sched 原语)· closed channel 参与 → 可捕获错误 · 全守卫禁用+无 default → nil
(不同于 Go 死锁 panic,留档)。

**顺手修复**:resolver 对裸 `Expr::Block` 空处理 → 按语句块正常 resolve;老 resolver Send 分支
不 resolve 的 bug 随删除消失。

**验证**:workspace 1508+ 全绿 · GC-stress 全绿 · clippy/fmt 0 · no_std 0/0 · dist bench 1.014x
(基线内)· 差分门禁(examples+bytecode)含新语料全过 · AOT Tier 0 bundle 实测跑通。

## 剩余(均已裁决/留档)
- **[~] M4.2 AOT 深覆盖**:缺 mixed/动态类型系统;Tier 1 桥已供出路,不紧迫。
- **既有断点(留档)**:全局 `spawn(闭包)` 不工作(闭包无 promote 到 CallableValue::Runtime 的路径,
  `task.spawn_blocking` 同因 bail,顶层即复现)——若修需 Arc state 快照机制,独立工作项。
- **可选后续**:`sched.any`(协程内协作式多路等待,select 的 sched 版)——sched 描述符协议天然可扩展。
- **✅ 裁决不做**:callable trait 反转 · 真机/QEMU demo · 细粒度 float/unicode feature。

## 护栏 & 续接
全量 tests 0 失败 / clippy 0 / fmt 0 / no_std 0/0 / GC-stress 全绿 / bench 与差分门禁全过。
**下一会话候选**:① 修全局 `spawn(闭包)` 断点(并发故事最后一块拼图);② `sched.any` 协作式
select;③ 征询用户 v2 新方向。
