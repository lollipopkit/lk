# Handoff

**本轮:九项计划全部执行完毕**(CI 化 / -O2 / 模块 builtin / .lkm 体积 /
死代码 / fuzz 扩形状 / AOT 基线 / budget 特化 / 方法分派),其中 24/25
(字符串 interning、循环 Move 消除)以评估落档待专门轮次。
workspace **1461 passed / 0 failed**。

## 里程碑

- **bench 全套 20 workload 通过 MIR 编译为 native,20/20 checksum 与 VM
  一致;dist 单样本 AOT/VM 几何平均 0.329x**(退役 legacy 历史值 0.331x,
  现在还带 -O2)。标量/控制流 workload 0.02–0.19x;5 个动态字符串键 map
  workload 慢 2.0–3.5x(lkrt string-map 每操作全局锁 arena 注册 +
  CStr→String 转换,下一个 native 性能靶)。
- MIR 本轮新形状:方法分派(`__lk_call_method` → str.starts_with/map
  get/set/list.contains,经 ArgList 免物化参数包)、Map<str,bool>+MaybeBool、
  void 用户函数、模块 builtin(os/time/env/math)、可变标量全局(entry 前缀
  初始化守卫)、ForLoopI/Move2/融合算术族、Get/SetIndexStrI、动态字符串键
  map 读写、ListStr push、Maybe↔标量 phi 边转换(MaybeValue/MaybeWrap)、
  Maybe display(Select)、Str/句柄参数与 Str 返回。
- clang **-O2 默认生效**(--skip-opt→-O0);fib(32) 17ms→12ms。
- `.lkm` v5:facts 稀疏编码 + compact JSON(bench 577KB→98KB,-83%)。
- CI:`.github/workflows/correctness.yml`(nightly GC stress / sanitizer
  差分 / 新种子放大 fuzz / Miri lkrt)。
- VM:dispatch 主循环按 instruction budget 单态化(非 WASM 路径去掉每指令
  计数),本地 min 对 min 约 -6%,无回归。
- fuzz 扩形状(range-for/动态模板键 map/str list/随机格式串轰炸),
  ~99% 用例可 native 比较;差分新增 modules 组 12 例。
- 本轮差分/CLI 测试抓到并修复:emit 隐式 nil 返回被拒(void 函数支持)、
  `return nil` 的 lower panic(totality 违约)、GetIndex bool map 缺口、
  融合算术不 unwrap Maybe。

## 待办(需专门轮次)

1. **lkrt string-map 性能**:arena 注册免锁/临时 key 不进 arena、map hasher
   换 fxhash——目标把 5 个 map workload 的 AOT 拉回 <1x。
2. **VM 字符串 interning/hash 缓存**(任务24):触碰 heap/eq/GC 全链路,
   需在有 Lua 门禁的环境做 wall-clock 判定(本机无 Lua、噪声 ±20%)。
3. **VM 循环体 Move 消除**(任务25):phi/loop-carried slot lowering,
   同样需要门禁环境;README 记录的历史反例多,须小步验证。
4. bench runner 本机无 Lua;RUN_AOT=1 的 runner 路径用 target/release
   (非 dist),后续对齐。
5. examples 覆盖 3/44:长尾在 assert_eq/typeof/match/closure/struct。

## 上两轮成果(仍有效)

正确性:bytecode verifier、facts 序列化(v4→v5)、GC stress、三大差分
语料、sanitizer/Miri、docs/semantics.md。架构:legacy text 后端已退役
(-4.8 万行),MIR 唯一后端,无 fallback。
