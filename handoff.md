# Handoff

**当前状态(2026-07-03,一等函数值收官轮完成,待 push)**:
- **RFC 阶段 4 一等函数值全部落地**:多身份 lambda 克隆特化(上限 8)、
  捕获闭包作参数(env 隐藏尾实参)、返回闭包(静态摘要,零运行时开销,
  无需 `{fn_ptr, env}` 表示)。closure.lk 1-5 节 native==VM,全文件只差
  list 结构相等(第 6 节 `evens == [...]`)。
- **修掉一个 VM inline 真 miscompile**:inline 恢复丢外层 cell promotion
  记录 → 第二个调用点二次 promotion 把旧 cell 当初值(Int + Obj 运行期
  报错);修复=绑定未被遮蔽的 promotion 在恢复时保留。回归测试
  `compiler_inline_arg_closure_promotion_survives_scope_restore`。
- **list display**(println 语境 VM-exact 渲染,ToString/插值语境照 VM
  拒绝)。`docs/semantics.md` 已钉死两条 display 路径语义。
- **AOT/LK geomean 0.251x**,20/20 checksum 一致;VM/Lua 1.055x(单次
  运行参考)。artifact 仍 v6。
- 防线全绿:workspace 全量 / 三套差分 / fuzz 4 种子(新形状:捕获闭包
  身份混跑、分支 helper inline 回归形状、闭包工厂)/ ASan+UBSan 三套 /
  Miri lkrt 24/24 / `-D warnings` test-target / fmt+clippy 0。
- 本轮 5 个 commit(inline fix、闭包实参、返回闭包、list display、
  多身份克隆,见 git log);文档 backend.md/plan.md 已同步。

## 能力面速览(AOT/MIR)

标量/控制流/直接调用单态化;**一等函数值**:零捕获去虚化、多身份克隆
特化、捕获闭包作参数(调用点解析 env)、返回闭包(静态摘要)、list HOF
(fn-pointer ABI);容器 + composite key + **list display**;builtin:
println/print/assert(_eq/_ne)/panic/typeof/IsNil;模块:os/time/env/math/
fs/process/datetime/io.std;跨块 builtin/closure ref 回溯(Move/Move2)。
限制:cell **内容**跨块不流动(分支/循环中创建或变异的捕获闭包拒绝)、
容器相等、json。VM:CallMethodK 免装箱 + DetachedStr 免分配分派。

## 下一轮候选(按解锁面排序)

1. **list/容器结构相等**(`xs == [1,2]`):closure.lk 全文件的最后一块;
   heap-const list 物化 + lkrt eq helper + CmpInt 容器分派。
2. **跨块 cell 状态**(cell 并入 Braun 虚拟槽做 phi):解锁分支/循环内
   的捕获闭包,Ssa 通用化改造。
3. VM dispatch 密度专项(fraud/cart 剩 2.0x/2.3x 的本质差距,重大专项)。
4. json/动态 tagged 值(独立立项)、Move 消除(上限 6-8%)、
   histogram AOT 1.04x、clippy `--all-targets`(剩 ~33 条 test lint)。

## 数据驱动判定存档(仍有效)

方法调用净成本 ~25ns(inline cache 不做);interning `heap_clones=0`
(不做);Move 占 16% 步数但廉价,消除上限 6-8%;`.lkm` v5 被 v6 干净
拒绝(设计如此)。
