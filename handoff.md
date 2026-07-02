# Handoff

**本轮(续):VM 方法分派免分配优化 + math 模块 MIR 吸收**。
workspace 全绿,ASan/UBSan 差分零报告,Miri lkrt 23/23,GC stress 全绿。

## 本轮新增(2026-07-02 下半场)

- **VM 方法分派 generic 优化**(profiling 驱动):`__lk_call_method` 每调用
  3 次分配(方法名 ArcStr、字符串 receiver `Arc::from` 全拷、参数试探×4 次
  with_slice)。修复:`DetachedStr` 载体(ShortStr Copy / Arc refcount clone,
  零字节拷贝)统一方法名/receiver/字符串参数;receiver 类型先行定向分派
  (四个 dispatcher 互斥,只试探一个)。**fraud_rule_scoring 49→33ms(-32%)、
  cart_pricing_rules 9.2→6.1ms(-34%)、template_render 1.82→1.40x;
  全套 VM/Lua geomean 1.175x→1.120x(本地 dist)**,其余 workload 无回归。
  - 归因数据(留档):fraud 每迭代 ~40ms/44ms 在字符串方法调用
    (`starts_with` 85k 次 ×350ns);map 查找仅 ~2ms、调用机制 ~0.2ms。
  - **下一杠杆(需专门轮次,ISA 级)**:方法调用仍是
    GetGlobal+NewList 装箱+泛型 Call(每调用一次堆分配 args list)。
    专用 CallMethod opcode(receiver+名字常量+参数窗口)可免装箱与
    global 查找,惠及全语言方法调用;涉及 compiler/exec/verifier/facts/
    artifact v6/AOT lower,参照 GetIndexStrI 先例。
- **math 模块 MIR 吸收**(任务#7 首模块):常量 lower 期解析
  (pi/e/inf/nan/max_int/min_int/max_float/epsilon)、floor/ceil/round
  静态类型分派、abs/min/max 保型 Select、sqrt(负参 abort 同 VM 响亮
  失败)/sin/cos/exp/pow 经 lkrt(Number→F64 提升)。native 链接补 `-lm`
  (Linux)。**math_demo 解锁,examples 6/44**,地板 ≥6;差分 +2 例。
  - math_demo 原报错 "r3 read before def" 已解:是模块常量成员读
    (ModuleFn ref 无 SSA 值)的误导性报错,非 bug。

## 里程碑

- **5 个动态字符串键 map workload:2.0–3.5x → 0.79–1.04x(几何平均
  ≈0.89x);全套 20 workload AOT/VM 几何平均 0.329x → ≈0.26x**,20/20
  checksum 一致。手段(全部通用,无 workload 特化):
  - lkrt 运行时 arena 从全局 `Mutex` 改 **thread_local RefCell**(AOT 单线程,
    单测反而获得线程隔离);arena 注册表与全部 map 句柄换 **FxHash**。
  - map `set` 命中走 `get_mut` 就地更新(免每次 `to_string` 分配)。
  - 新 ABI `str.concat_i64`:`prefix ++ decimal(i64)` 单次分配(免 NUL 扫描,
    `from_vec_unchecked`);`ConcatString`/`ConcatN` 的 int 操作数经
    `concat_display` 通用融合(模板串 `"b${i}"` 少一次分配+注册+free)。
  - 新 ABI `map_h.str_i64/f64_set_ik`:`SetIndexStrI` 直接传 (prefix, suffix),
    key 在 lkrt 栈上拼(88B inline,超长 spill),存储路径零 key 分配。
- **MIR 新 builtin**:`panic`(空格 join display,致命)、`assert_eq`/
  `assert_ne`(标量相等 + Int/Float 交叉、Str 字节比较,VM 同格式失败消息,
  消息 eager 构造)、`typeof`(静态标量名;Maybe 载体 Select "Nil" vs 值名)、
  `IsNil`(标量恒 false / Nil 恒 true / Maybe 取 !present)。
- **examples 差分覆盖 3/44 → 5/44**(named_args、named_params 解锁),
  地板断言 ≥3 → ≥5;手写差分新增 14 例(assert_eq/ne/panic/typeof/lambda 组)。
- **零捕获闭包(RFC 阶段 4 第一切片)**:`MakeClosure(capture_count==0)` →
  `GlobalRef::Lambda(fidx)`,间接 `Call` 去虚化为直接调用(复用
  per-callsite 单态化);顶层 `let f = |x|…` 经 prescan(entry 前缀 +
  全模块唯一写)升为静态 lambda 全局,任意函数可读;可达性沿 MakeClosure
  边扩展。捕获闭包/一等函数值仍拒绝。fuzz 生成器 30% 概率把 helper 发成
  lambda 形式。lower 单测 +4。
- **bench runner 对齐 dist**(原待办4):`resolve_lk_bin` 现选
  dist/release 中更新者(CI perf gate 本就 pin dist);本机源码构建
  Lua 5.4.7(`~/.local/bin/lua`),本地 VM/Lua 基线 **dist geomean
  ≈1.18x**(WSL2 + Lua 5.4.7,与 CI 的 Lua 5.5/机器不可直接比,仅作
  本地相对参照)。

## 待办(需专门轮次)

1. **CallMethod 专用 opcode**(见上,本轮 profiling 的头号发现):方法
   调用免 NewList 装箱 + 免 GetGlobal,预计再砍每调用 ~100-200ns;
   fraud/cart 仍 4.1x/4.4x behind Lua(本地),这是主要剩余差距。
2. **MIR 捕获闭包 + 一等函数值**(阶段 4 剩余):cell 环境建模、闭包作
   参数/容器元素(需 FnRef 进单态化格)、list HOF(map/filter/reduce 接
   静态 lambda)。closure.lk/higher_order.lk 解锁依赖这些。
3. **VM 字符串 interning/hash 缓存**(任务24)+ **循环 Move 消除**(任务25):
   本地 Lua 已可预筛;README 历史反例多,小步验证。
4. examples 覆盖长尾:模块函数(json/datetime/io/stream/tcp,含解构
   import 形状、句柄返回值)、LoadHeapConst 常量容器、match/struct/
   NewRange。
5. histogram_group_count AOT 仍 ≈1.04x(let-bound 模板 key 每迭代一次
   分配),收益小,最低优先级。

## 上轮成果(仍有效)

bench 全套 20 workload MIR 原生化;方法分派/bool map/void fn/模块
builtin/可变全局;clang -O2 默认;`.lkm` v5(-83%);correctness CI
(GC stress/sanitizer 差分/fuzz/Miri);budget 特化 dispatch(-6%);
legacy text 后端已退役,MIR 唯一后端。
