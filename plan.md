# 下一轮计划:list display + 一等函数值收官

> **状态:待开工**。上一轮(正确性优先计划,九步)与其后的五轮优化/特性
> session 已全部完成并合并(PR #15,`cbc2932`):CallMethodK opcode
> (artifact v6)、闭包四切片、模块吸收、lkrt string-map、clippy 清零 +
> CI lint 门禁。现状见 `handoff.md`,历史细节见 `progress.md`。

目标:补齐 examples 长尾的公共依赖(list display),然后完成 RFC 阶段 4
的最后一块(一等函数值)。VM 性能项(dispatch 密度专项)不在本轮。

原则(沿用):
- VM 语义是唯一锚;native 侧任何形状先钉 VM 行为(必要时 dump 字节码),
  再实现,差分用例锁定。
- 拒绝面永远响亮;单态化冲突整模块回退,不允许静默错编。
- 每步过:相关 crate 测试 → 差分三套 → 收尾全量 + sanitizer。

## 步骤

### 1. list display 格式化(中)
`println("{}", list)` / `println(list)`:VM 的 list/map display 格式先钉死
(嵌套、字符串元素引号与否、分隔符——写进 `docs/semantics.md`),lkrt 加
递归格式化 helper(handle → 拼接 Str),lower 的 display 分派
(`to_display_str`/print_parts)接容器句柄类型。
验证:差分用例(i64/f64/str/嵌套 list、map)+ examples 差分覆盖变化。

### 2. 多身份 lambda 参数:按身份克隆特化(大)
现状:`sig.lambda_params` 单身份擦除已落地,两个调用点传不同 lambda →
conflict 整模块回退(closure.lk 4 节的形状)。
- 特化键:`(callee, 各 lambda 参数的身份向量)` → 克隆 MIR 函数体
  (FuncId 映射需扩展:克隆函数的 id 分配与 `lk_fn_N` 命名)。
- 上限防爆炸:每函数特化数硬上限(如 8),超限响亮回退。
- 与既有 param_obs fixpoint 的交互:克隆体各自参与类型推导。
验证:closure.lk 4 节形状差分;fuzz 生成器加"同 helper 两个 lambda"形状。

### 3. 捕获闭包作参数(中,依赖 2)
零捕获擦除 + cell 建模已就位:捕获闭包的 env 是调用点已解析的值向量,
作参数 = 擦除 lambda 身份 + 把 env 值追加为隐藏实参(克隆体按
capture 数扩参)。跨块限制沿用。

### 4. 返回闭包(大,可切到下轮)
需要运行时闭包表示:`{ fn_ptr, env… }` 结构(lkrt 侧 boxed env +
codegen 间接调用)。若 2/3 消化后预算不足,立项留档不硬做。

### 5. 收尾验证
- 三套差分 + fuzz(双种子放大)+ sanitized-differential + Miri lkrt
- `RUSTFLAGS="-D warnings" cargo test --workspace --all-features --no-run`
  (CI parity,test targets 也要干净)
- AOT bench 20/20 checksum;fmt + clippy 门禁
- 更新 handoff.md / progress.md / backend.md

## 非本轮(记录不做)
- VM dispatch 密度专项(超级指令/computed goto)—— fraud 剩 2.0x 的
  本质差距,重大专项单独立项。
- json/动态值表示 —— 需要 native 侧 tagged 动态值,独立立项。
- Move 消除(上限 6-8%,README 历史反例多)、histogram AOT 1.04x。
- clippy `--all-targets` gate(先清 test 代码剩余 ~33 条 lint)。
