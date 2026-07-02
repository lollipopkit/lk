# Handoff

**当前状态(2026-07-03,一等函数值收官 + list 相等 + 跨块 cell,PR 待开)**:
- **RFC 阶段 4 一等函数值全部落地**:多身份克隆特化(上限 8)、捕获闭包
  作参数(env 隐藏尾实参)、返回闭包(静态摘要,零运行时开销)。
- **closure.lk 全文件 native==VM**(7 节含 list HOF 断言);examples
  差分 **10→11**。
- **list 结构相等**:lkrt eq helpers(i64/f64/str + Int/Float coercion,
  NaN 破相等);跨型非数值对仅在两侧物化非空时折叠 false(`list_base_len`
  下界),否则响亮拒绝(空 list 跨型恒等)。
- **跨块 cell 状态**:cell 升级为 Braun SSA 虚拟槽(`reg_count+cid`),
  分支 mutation / loop-carried 更新 / 分支 helper + 闭包实参全部获得 phi;
  循环隔离靠 ref 一致性在 loop header 终止,无需额外 pin。
- **修掉两个 VM 真 miscompile**(均为 cell promotion 生命周期):
  ① inline 恢复丢外层 promotion 记录;② 循环内 mid-body promotion
  (修复=循环入口预提升 + for 循环变量快照 cell(copy 不 move))。
  回归测试 4 个;semantics.md 已钉循环捕获语义。
- 防线全绿:workspace 95 套 / 三套差分(手工 13 组)/ fuzz 7 种子累计
  (新形状:list 相等、分支 mutation、闭包工厂、branchy-helper)/
  ASan+UBSan 三套 / Miri lkrt 25 / `-D warnings` test targets /
  fmt+clippy 0 / AOT bench 20/20 checksum(VM/Lua 1.008x 零回归,
  AOT/LK 0.259x)。
- dev 分支 12 个 commit 待 PR(7 个上轮 + 5 个本轮)。

## 能力面速览(AOT/MIR)

标量/控制流/直接调用单态化;**一等函数值**:零捕获去虚化、多身份克隆
特化、捕获闭包作参数(调用点解析 env)、返回闭包(静态摘要)、list HOF
(fn-pointer ABI);容器 + composite key + list display + **list 结构
相等**;**跨块 cell 状态(虚拟槽 phi)**;builtin:println/print/
assert(_eq/_ne)/panic/typeof/IsNil;模块:os/time/env/math/fs/process/
datetime/io.std;跨块 builtin/closure ref 回溯(Move/Move2)。
仍拒绝:闭包变异自身捕获、闭包进容器、跨迭代闭包逃逸、map 相等、json。
VM:CallMethodK 免装箱 + DetachedStr 免分配分派。

## 下一轮候选

1. map 结构相等(补齐容器相等面)、闭包进容器。
2. VM dispatch 密度专项(fraud/cart 剩 ~2x 的本质差距,重大专项)。
3. json/动态 tagged 值(独立立项)、Move 消除(上限 6-8%)、
   histogram AOT 1.04x、clippy `--all-targets`(剩 ~33 条 test lint)。

## 数据驱动判定存档(仍有效)

方法调用净成本 ~25ns(inline cache 不做);interning `heap_clones=0`
(不做);Move 占 16% 步数但廉价,消除上限 6-8%;`.lkm` v5 被 v6 干净
拒绝(设计如此);返回闭包的 runtime `{fn_ptr,env}` 表示不需要(静态
摘要覆盖观测语料)。
