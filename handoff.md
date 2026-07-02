# Handoff

**本轮:lkrt string-map 性能靶(待办1)完成 + MIR builtin 长尾吸收 +
零捕获闭包切片 + bench runner dist 对齐**。
workspace 全绿(cargo test --workspace --all-features exit 0),
ASan/UBSan 差分(含 120 fuzz)零报告,Miri lkrt 23/23,fuzz 200 例×2 种子干净。

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

1. **MIR 捕获闭包 + 一等函数值**(阶段 4 剩余):cell 环境建模、闭包作
   参数/容器元素(需 FnRef 进单态化格)、list HOF(map/filter/reduce 接
   静态 lambda)。closure.lk/higher_order.lk 解锁依赖这些。
2. **VM 字符串 interning/hash 缓存**(任务24)+ **循环 Move 消除**(任务25):
   本地 Lua 已可预筛(噪声 ±20% 仍在,最终判定仍需门禁环境);README
   历史反例多,小步验证。
3. histogram_group_count 仍 ≈1.04x(let-bound 模板 key 每迭代一次分配),
   要 <1x 需 key 构造进一步下沉或 VM 侧对齐比较。
4. examples 覆盖长尾:GetGlobal 模块函数(json/datetime/io/stream/tcp)、
   LoadHeapConst 常量容器、match/struct/NewRange;math_demo 报
   "r3 read before def"(module 常量 `math.pi` 形状)值得单看。

## 上轮成果(仍有效)

bench 全套 20 workload MIR 原生化;方法分派/bool map/void fn/模块
builtin/可变全局;clang -O2 默认;`.lkm` v5(-83%);correctness CI
(GC stress/sanitizer 差分/fuzz/Miri);budget 特化 dispatch(-6%);
legacy text 后端已退役,MIR 唯一后端。
