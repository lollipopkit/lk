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

## 本轮追加(用户指示:修小项+文档+LSP 补齐+AOT 排查)
- ✅ spawn 复用 shared Arc<Module>(免每次深克隆)+ `task.stats()` 观测面(commit `3bca22e`)
- ✅ LSP/编辑器补齐 v2 语法(commit `eea81f0`):lk-lsp 语义 token + completion 关键字
  (go/try/catch,后两者是既有缺口);tree-sitter 新增 go_statement/try_statement/
  unwrap_expression(**踩坑**:macro_invocation 静态 prec(21) 会压过 unwrap 且跳过 GLR,
  改 prec.dynamic + conflict 对;语料 9/9);tmLanguage/highlights.scm 同步。
  zed-ext-check 失败是既有工具链问题(futures-core@wasm32-wasip1,基线同样失败)
- ✅ README/README.zh-CN「A Taste/一瞥」可运行示例(commit `5c5ec5f`,实测输出锁定)
- ✅ **M4.2 排查完成**(本 commit):`scripts/aot_coverage.sh` 可复现扫描,14/51,
  阻塞排行+路线图入 progress.md「M4.2 AOT 深覆盖」章节

## 剩余
- **[~] M4.2 AOT 深覆盖(排查已毕,实现待启)**:GetGlobal 14(try$call/并发/模块白名单)·
  operand 超子集 9 + LoadHeapConst 4 + NewObject 2(共同根因=**Dyn 装箱值地基**,M4.2 本体)·
  Call 5(方法 ABI 长尾)· NewRange 1。**下会话从 Dyn 地基开始**(路线细节见 progress.md)。
- **✅ 裁决不做**:callable trait 反转 · 真机/QEMU demo · 细粒度 feature 拆分。
- **可选后续**:native raise 前缀统一(catch 到的 native 错误带 "native ... failed:" 前缀,
  error() 一等值无)· goroutine 泄漏之外的死锁检测。

## 护栏 & 续接
全量 tests 0 失败 / clippy 0 / fmt 0 / no_std 0/0 / GC-stress 全绿 / bench 1.021x(基线内)/
差分门禁全过。**下一会话首选:M4.2 Dyn 装箱值地基**(MIR Ty::Dyn + lkrt tagged value,
注意 display/错误信息 VM-exact 逐字节 + semantics.md 已裁决混合 map display 不进子集)。
