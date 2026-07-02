# Handoff

**当前状态(2026-07-02,四轮优化+特性 session 完成)**:
- **examples 差分 10/44**(fib、math/os/time/datetime/io_demo、internal、
  named_args/params、numeric_auto_promotion),地板断言 ≥10。
- **VM/Lua geomean 1.033x**(本地 dist;session 起点 1.175x)。fraud/cart
  剩 2.0x/2.3x,已到 dispatch 密度层(5.4ns/步)。
- **AOT/VM geomean ≈0.26x**,20/20 checksum 一致。
- **artifact v6**(CallMethodK opcode;v5 `.lkm` 被干净拒绝)。
- **clippy 0 警告**,check.yml 已启用 fmt+clippy(-D warnings)门禁
  (下次 PR CI 首次执行)。
- 防线全绿:workspace / ASan+UBSan 三套差分 / fuzz(含闭包与 HOF 形状,
  双种子)/ Miri lkrt / GC stress。
- 本 session 四轮详情见 progress.md(方法分派两轮、CallMethodK、闭包三切
  片、模块吸收、lkrt string-map、修复轮)。

## 能力面速览(AOT/MIR)

标量/控制流/直接调用单态化;闭包:零捕获去虚化 + 捕获 cell 建模(调用点
解析)+ list HOF(fn-pointer ABI)+ lambda 作参数(单身份擦除);容器与
composite key(set_ik/concat_i64);builtin:println/print/assert(_eq/_ne)/
panic/typeof/IsNil;模块:os/time/env/math/fs/process/datetime/io.std;
nil/Bool 值化比较、跨块 builtin ref 回溯。VM:CallMethodK 免装箱方法调用
+ DetachedStr 免分配分派。

## 下一轮计划(已定)

1. **list display 格式化**(`println("{}", list)`):iter_sugar、
   for_loop_patterns 等多个 example 的公共依赖;lkrt 递归格式化 helper +
   lower display 分派,先做。
2. **一等函数值收官**:多身份 lambda 参数(按身份克隆特化)→ 捕获闭包作
   参数 → 返回闭包(运行时 env+fn 指针)。

## 待办(需专门轮次)

1. **VM 剩余性能差距(数据驱动重定位,2026-07-02)**:fraud 现在
   16ms/2.99M steps ≈ **5.4ns/步**,方法调用净成本已 ~25ns(starts_with
   微基准 350→25ns,inline cache 边际收益不足,判定不做);interning
   (任务24)profile 显示 `heap_clones=0` **无收益,判定不做**;Move 仍占
   16% 步数但都是廉价操作,消除上限 ~6-8%(任务25 保留,README 反例
   多)。真正接近 Lua 需要 dispatch 密度级架构(超级指令/计算 goto/寄存
   器分配),属重大专项。
2. **MIR 一等函数值(剩余)**:本 session 已落地"零捕获 lambda 作参数"
   (参数擦除特化:全部调用点同一 lambda 身份 → callee 入口播种静态 ref;
   `sig.lambda_params`,不同身份 conflict 回退)。剩余:**多身份**(需按
   lambda 身份克隆特化函数体)、捕获闭包作参数(env 也要传)、返回闭包
   (运行时闭包表示:env 结构 + fn 指针)。closure.lk 4/5 节依赖多身份。
   注:str/mixed list HOF 扩展经评估**不是**独立解锁路径——
   list_iter_sugar/iter_pipeline 还需要 iter 模块函数 + **list display
   格式化**(`println("{}", list)`)+ 嵌套 list,应并入本项整体规划。
3. examples 覆盖长尾:json(动态嵌套值,子集外——需运行时动态值表示,
   单独立项)、stream/tcp_demo(句柄+bytes 流)、LoadHeapConst 常量
   混合容器、match/struct/NewRange、iter/list_iter_sugar(HOF over
   str/mixed list)。
4. histogram_group_count AOT 仍 ≈1.04x(let-bound 模板 key 每迭代一次
   分配),收益小,最低优先级。
5. `.lkm` v6:CallMethodK 后旧 v5 artifact 被拒(设计如此);发布渠道如
   有缓存 artifact 需重编译。

## 上轮成果(仍有效)

bench 全套 20 workload MIR 原生化;方法分派/bool map/void fn/模块
builtin/可变全局;clang -O2 默认;`.lkm` v5(-83%);correctness CI
(GC stress/sanitizer 差分/fuzz/Miri);budget 特化 dispatch(-6%);
legacy text 后端已退役,MIR 唯一后端。
