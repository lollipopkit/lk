# Handoff

**目标(`/goal`)**:把 `plan.md` 划分为多步、逐个完成、每步 push。**用户:允许短期回归、fix-forward。**
细节台账在 `progress.md`。已推送 **125 commit** 到 `dev`。
**🎯 plan.md v1.0 六项全部达成(2026-07-04)**。**v2 语言面重设计已落地(2026-07-06,用户裁决)**:
**Swift 式错误模型**(删 pcall,try/catch 唯一捕获面 + 后缀 `!` 解包,错误一律 raise)+
**Go 式并发**(删协程/yield/sched,`go` 关键字 + spawn goroutine + 阻塞 channel + select)。

## ✅ 主线状态
- **Phase 0 / M0–M5** 全部达成(v1.0 定义);M2.5 stackless `Vec<CallFrame>` 地基保留。
- **v2 错误模型**:`error(v)` raise 一等错误值 · try/catch(→隐藏 `try$call`)· 后缀 `!`
  (nil → raise "unwrap of nil value";`!` 紧跟 `(`/`[`/`{` 留给宏调用;`x!==1` 需写 `x! == 1`)·
  无 `[ok,value]` 对(非错误的"暂无"用 nil:chan.try_recv 空 / task.try_await 未完成)。
- **v2 并发**(`docs/concurrency.md`):`go f(x);` / `spawn(闭包)`(快照 promote:module Arc +
  捕获/globals 同模块结构深拷贝)· goroutine 内阻塞 send/recv(block_in_place)· chan.close
  Go 语义(缓冲可排空)· select 对 closed channel always-ready(nil binding)· **isolate 深拷贝**
  (裁决:单线程无锁 GC 是底线;通信走 channel,比 Go 更严格的 CSP)。
- 全量 **1499+ tests 0 失败**(核心 953)· `MODULE_ARTIFACT_VERSION` = 9。

## 本轮(v2)五子步,commits `33d3fb9`/`a16da0e`/`cbf0e10`/`a910eb3`/本次
1. 全删协程/yield/sched(-2587 行,artifact v9);select/chan/task/spawn 存活。
2. 修 spawn(闭包):`copy_runtime_value_same_module`(ClosureCopy 模式)+ 快照 promote;
   **Runtime::block_on 多线程 flavor 走 block_in_place**(goroutine 内阻塞收发成立的关键)。
3. `go` 关键字(parse 时糖 → spawn);顺手修 send/recv typecheck 对 fn 参数类型变量误报。
4. 错误模型迁移:pcall→try$call · `!` 解包(与宏调用消歧)· recv/send/try_* raise 语义 ·
   chan.close 改可排空 · 语料/测试全迁(error_unwrap.lk 替代 pcall_error.lk)。
5. 文档:docs/concurrency.md 新写 · semantics.md/stdlib.md · plan.md 4.4/4.5 裁决注记。

**实测踩坑留档**:goroutine 内 block_on panic("runtime within runtime")→ block_in_place;
`!` 与宏调用 `name!(...)` 冲突(宏三种定界符都在用)→ 消歧规则;chan.close 旧行为 remove
导致缓冲丢失 + "Channel not found" → 改标记式关闭;native raise 带 "native ... failed:" 前缀
(LK 层 catch 到的字符串,error() 一等值无此前缀)。

## 剩余(均已裁决/留档)
- **[~] M4.2 AOT 深覆盖**:缺 mixed/动态类型系统;Tier 1 桥供出路,不紧迫。
- **✅ 裁决不做**:callable trait 反转 · 真机/QEMU demo · 细粒度 feature 拆分。
- **可选后续**:goroutine 泄漏诊断(阻塞 goroutine 不回收,同 Go)· spawn 的 module.clone()
  按 spawn 频率高时可缓存 Arc(现无热路径)· native raise 前缀统一(见踩坑)。

## 护栏 & 续接
全量 tests 0 失败 / clippy 0 / fmt 0 / no_std 0/0 / GC-stress 全绿 / bench 1.021x(基线内)/
差分门禁全过。**下一会话候选**:① goroutine/错误模型的深度语料与文档打磨(README 语言示例还是
旧语法?待查);② M4.2 AOT 深覆盖;③ 征询用户新方向。
