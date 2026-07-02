# Handoff

**本轮(四):CallMethodK opcode + list HOF + datetime/io.std 模块,
examples 10/44,VM/Lua geomean 1.033x**。
workspace 全绿,ASan/UBSan 差分零报告,Miri lkrt 23/23,fuzz 200 例干净,
AOT 20/20 checksum 一致。

## 本轮新增(2026-07-02 第四场)

- **CallMethodK opcode(ISA 级,artifact v6)**:`a`=窗口基址(receiver 在
  a、args 在 a+1..、结果写 a)、`b`=方法名字符串常量索引、`c`=argc。编译器
  `lower_dynamic_method_call` 直接发射(名字索引 >u8 回退旧 helper 形状);
  exec 主循环热路径直接窗口分派(`core_call_method_windowed`:builtin
  分派吃 slice 零装箱,罕见尾路径〔可调用属性/list HOF/trait〕才物化
  args list);verifier 校验窗口+名字常量;AOT lower 经共享
  `lower_method_dispatch` 消费。**每次方法调用消灭 NewList 堆分配 +
  GetGlobal + 泛型 Call 机制。fraud 33→16ms、cart 6.1→3.2ms(自轮初
  49/9.2ms 累计 -67%);全套 VM/Lua geomean 1.120x→1.033x**,
  template_render 转 ahead(0.98x),零回归。
- **list HOF(阶段 4 第三切片)**:`map`/`filter`/`reduce` over `List<i64>`
  接零捕获 lambda——MIR 新 `Const::FnAddr(FuncId)`(codegen 渲染
  `ptr @lk_fn_N`),lkrt `i64_{map,filter,reduce}_fn` 以 `extern "C"` fn
  指针逐元素回调;lambda 签名过同一单态化格(map: i64→i64、filter:
  i64→Bool、reduce: (i64,i64)→i64,不符响亮拒绝)。链式 pipeline 与
  回调内除零 abort 差分锁定。
- **datetime 模块**:lkrt 引 chrono(与 stdlib 同 crate,格式化/星期
  字节一致);now/format/parse/day_of_week/day_of_year/is_weekend 经
  ABI,add/sub 内联 Int 算术。**发现并修复 example bug**:datetime_demo
  假设 now() 返回微秒(实际秒)→ VM 自己也断言失败(从未被执行过的
  example);已修正 demo。
- **io.std 模块**(`use { std } from io` 的解构 import 绑 "std" 全局):
  stdin/stdout/stderr = 固定句柄 0/1/2,write/writeln 返回 VM 字节数
  (lkrt 返回值已对齐),flush→true。**修复真实分歧**:native 里
  Rust-stdout 缓冲与 printf 的 C 缓冲交错错序——lkrt 写者先
  fflush(NULL) 后 flush 自身流,保持程序序。
- **杂项通用形状**:Bool==Bool 比较(ZextBool→icmp)、跨块 builtin ref
  回溯(`assert(a || b)` 的 merge 块调用,全前驱一致才命中)、
  Str.contains/.len 方法。json 明确记录为子集外(动态嵌套值)。
- **examples 8/44 → 10/44**(datetime_demo、io_demo),地板 ≥10;
  手写差分 +5 组。

## 本轮新增(2026-07-02 第三场)

- **捕获闭包(阶段 4 第二切片)**:编译器把捕获变量装进 **UpvalCell**
  (共享可变盒,VM 语义:捕获后突变对闭包可见——`factor=5` 后调用打印 5)。
  lower 把 cell 建模为虚拟 SSA 槽(`Ssa::cell_vals`,(block, cell_id) 键):
  `LoadHeapConst UpvalCell` → 分配 cell id(初值 Nil);`StoreCellVal` 更新
  追踪值;`LoadCellVal` 读取;`MakeClosure` 记录 `ClosureCapture::Cell(id)`;
  **调用点**解析 cell 当前值追加为隐藏尾参(`LoadCapture k` → CellParam ref,
  `LoadCellVal` 经它读第 param_count+k 个参数)。capture 参数进 param_obs
  单态化格。**拒绝面**(全部响亮):跨块 cell 流、lambda 内改捕获变量、
  闭包作一等值(传参/进容器/返回)、Str 捕获流入 `+` 分派(AddInt 启发式
  看不穿 LoadCellVal)。差分 +5(基本捕获/捕获后突变/双捕获+调用间突变/
  函数内捕获/float 捕获)。
- **模块吸收(任务#7 续)**:os.hostname/arch/os、process.cwd、fs.temp_dir
  (existing lkrt helper 直接映射)、fs.read_dir(新 `lkrt_fs_read_dir_list`,
  排序 UTF-8 文件名 ListStr,旧 count 版本保留)、time.since(内联 end-start)。
  MODULE_GLOBALS += fs/process。
- **`== nil`/`!= nil` 值化比较**:在 read_scalar(会 unwrap Maybe)之前分流
  ——Maybe 载体测 present 位,具体类型折叠常量,Nil==Nil 常量,有序比较拒绝。
  (此前只支持 BrNil/BrNotNil 分支形式。)
- **examples 差分 6/44 → 8/44**(os_demo、time_demo 解锁),地板 ≥8。

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

1. **VM 剩余性能差距(数据驱动重定位,2026-07-02)**:fraud 现在
   16ms/2.99M steps ≈ **5.4ns/步**,方法调用净成本已 ~25ns(starts_with
   微基准 350→25ns,inline cache 边际收益不足,判定不做);interning
   (任务24)profile 显示 `heap_clones=0` **无收益,判定不做**;Move 仍占
   16% 步数但都是廉价操作,消除上限 ~6-8%(任务25 保留,README 反例
   多)。真正接近 Lua 需要 dispatch 密度级架构(超级指令/计算 goto/寄存
   器分配),属重大专项。
2. **MIR 一等函数值(真·一等)**:闭包作参数传给用户函数(需按 lambda
   身份克隆特化或 FnRef 进类型格)、返回闭包(运行时闭包表示:env 结构 +
   fn 指针)。closure.lk 4/5 节、higher_order.lk 其余方法依赖;list HOF
   零捕获切片已落地(`Const::FnAddr` 地基可复用)。
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
