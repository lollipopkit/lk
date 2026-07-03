# 本轮计划:list display + 一等函数值收官

> **状态:已完成(2026-07-03)**。四步全部落地,RFC 阶段 4
> (一等函数值)收官:list display、多身份克隆特化、捕获闭包作参数、
> 返回闭包(静态摘要,无需 runtime `{fn_ptr, env}`)。
> 额外收获:修掉一个 VM inline 真 miscompile(闭包实参触发的
> cell promotion 记录在 inline 恢复时丢失 → 二次 promotion 把旧 cell
> 当初值,运行期 Int + Obj 报错)。现状见 `handoff.md`,细节见
> `progress.md`。

## 已完成步骤

1. **list display 格式化** — lkrt `list_{i64,f64,str}_display` helper,
   VM-exact 分隔符/`{:?}` 引号;print 语境渲染容器,ToString/插值/concat
   语境照 VM 拒绝(`docs/semantics.md` 已钉死语义)。
2. **多身份 lambda 克隆特化** — 特化键 `(callee, 身份向量)`,每原函数
   上限 8 个克隆;函数值/普通值双态响亮拒绝。
3. **捕获闭包作参数** — 身份扩为 `{fidx, capture 数}`(env 不进键),
   调用点解析的 env 值作隐藏尾实参;克隆按 capture 数扩参;跨 helper
   转发自然嵌套。Move/Move2 的 builtin ref 跨块回溯传播。
4. **返回闭包(静态摘要)** — 唯一 return + 纯函数体 + 捕获全映射到
   参数 → 记录摘要;调用点直接播种 Closure ref,不发射调用与函数体。
   工厂结果可直接喂给闭包实参路径。call-site 事实(specialized/
   plain_called/conflict)改为每 fixpoint pass 重推导,消除摘要落地前
   的陈旧标记假冲突。counter(捕获自变)/多 return/带副作用工厂响亮拒绝。
5. **收尾验证** — 全部通过:workspace 全量、三套差分、fuzz(4 个种子,
   200-300 例放大)、ASan/UBSan 差分三套、Miri lkrt 24/24、
   `RUSTFLAGS="-D warnings"` test-target 干净、AOT bench 20/20 checksum
   (AOT/LK geomean 0.251x,单次运行参考值)、fmt + clippy 清零。

## 追加轮(2026-07-03,同 PR):list 相等 + 跨块 cell ✅

- **list 结构相等**已落地(lkrt eq helpers + CmpXx 容器分派 +
  `list_base_len` 非空下界折叠)→ **closure.lk 全文件 native==VM**,
  examples 差分 10→11。
- **跨块 cell 状态**已落地(cell = Braun 虚拟槽,phi 化;循环隔离由
  ref 一致性天然保证)→ 分支 mutation/loop-carried/branchy-helper
  闭包实参全部解锁。
- 又修掉一个 VM 真 miscompile:循环内 mid-body cell promotion(循环
  入口预提升 + for 循环变量快照 cell)。

## 下一轮候选(记录不做)

- map 结构相等、闭包进容器(容器相等/一等函数的收尾长尾)。
- VM dispatch 密度专项(超级指令/computed goto)—— fraud/cart 剩
  ~2x 的本质差距,重大专项单独立项。
- json/动态值表示 —— 需要 native 侧 tagged 动态值,独立立项。
- Move 消除(上限 6-8%)、histogram AOT 1.04x。
- clippy `--all-targets` gate(先清 test 代码剩余 ~33 条 lint)。
