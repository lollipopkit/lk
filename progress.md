# 实现进度

**当前**:CallMethodK opcode + list HOF + datetime/io.std 轮完成
(examples 10/44,VM/Lua 1.033x)。细节见 handoff.md。

## 归档:2026-07-02 session 各轮 handoff 摘要

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


## CallMethodK / list HOF / 重模块轮(本轮第四场)

- **CallMethodK 实现要点**:opcode=105(追加,artifact v5→v6,版本测试
  同步改 6/拒绝 5);编译器在 `lower_dynamic_method_call` 先试
  `push_string`(去重)拿名字常量,≤u8 才发新形状(receiver+args 搬进
  连续窗口,`clear_register(base)`);exec 端 `dispatch_call_method_k`
  借 `function.consts`(与 `self.state` 无借用冲突,方法名不需 detach),
  args 拷进 8 槽 inline buffer(超出 spill Vec),`NativeRuntime::new`
  后进 `core_call_method_windowed`。**语义顺序保持**:filter/map/reduce
  整体委托旧路径;runtime_access(属性优先)在 builtin 前;trait 尾物化
  list。老 `__lk_call_method` 路径保留(名字索引溢出回退 + 兼容)。
- **AOT 侧**:`lower_method_call` 拆出共享 `lower_method_dispatch`;
  `lower_method_call_k` 直接读窗口。**跨块 builtin ref 回溯**
  (`builtin_ref_at`,mirror `reg_const_str`:visited 防环、全前驱一致、
  本块 SSA def 遮蔽)解决 `assert(a || b)` merge 块调用。
- **list HOF**:`Const::FnAddr(FuncId)` → `getelementptr i8, ptr @lk_fn_N`;
  lkrt 回调签名过 conformance(fn 指针加 ClassOf=Ptr impl);filter 回调
  Rust `bool` ↔ LLVM i1(icmp 产出 0/1,实测 UBSan 干净)。HOF arm 在
  泛型参数读取**之前**(lambda 寄存器是 builtin_regs ref 无 SSA 值)。
- **datetime**:lkrt 引 chrono(workspace 版本);`datetime_utc` 越界
  abort;demo 的微秒假设是 example 自身 bug(VM 也断言失败)已修。
- **io.std**:`std` 进 MODULE_GLOBALS(解构 import 绑定名);句柄
  0/1/2 编译期常量;lkrt `write_std_stream` 返回字节数(对齐 VM 的
  written count)且 **fflush(NULL) 前置 + 自身 flush 后置**(两套
  stdout 缓冲的交错错序,差分 io_std_write 用例当场抓出)。
- Bool==Bool:ZextBool→i64 icmp(codegen 整型 icmp 硬编码 i64,i1
  直接喂会 IR 类型错)。
- 验证:workspace 全绿、三套 sanitized 差分 + 200 fuzz、Miri 23/23、
  AOT 20/20 checksum、GC stress(CallMethodK 轮跑过)。

## 捕获闭包轮(本轮第三场)

- **关键语义发现**:编译器对被捕获局部变量总是发 UpvalCell(LoadHeapConst
  堆常量 `{"UpvalCell":"Nil"}`)+ StoreCellVal 初始化;MakeClosure 捕获的是
  cell 句柄;lambda 体 LoadCapture(取 cell)+ LoadCellVal(解引用)。
  **共享可变**:`let f=|x|x*k; k=5; f(1)` VM 打印 5——MakeClosure 时快照是
  错的,必须调用点解析。用 /tmp 微例 + `lk compile bytecode` dump JSON
  (`python3 -c ... heap_values`)确认,先于实现。
- 实现要点见 handoff;capture 隐藏参数复用 param_obs fixpoint(不同调用点
  类型冲突 → conflict → 整模块 fallback,安全)。`cell_move` fact
  (move_value)不影响正确性:fact 只在源寄存器死后成立,SSA 读旧值无观察者。
- 单测:zero-capture 4 个原有;捕获行为由差分 5 例锁定(含突变语义)。
  fuzz 未加捕获形状(生成器 helper 作用域清空 vars,捕获需要外层变量,
  留给后续)。

## 模块吸收续(os/fs/process/time)

- lkrt 新 helper:os_hostname(HOSTNAME/COMPUTERNAME/localhost 链)、
  os_arch/os_name(env::consts)、fs_read_dir_list(排序 UTF-8 名单
  ListStr = `Vec<*const c_char>`,与 lklist 表示一致;旧 count 版
  `lkrt_fs_read_dir` 保留未映射)。process.cwd/fs.temp_dir 映射既有 helper
  (注意 process.cwd:VM 失败返回 Nil,native abort——极端边缘,未建模)。
- `time.since` 内联 IntBin Sub(I64 限定;stdlib numeric_millis 的 Float
  分支不进子集)。
- **nil 值化比较**是 os_demo 的隐性门槛(`assert(x != nil)` 产生 Bool 值,
  与 BrNil 分支形式不同);放在 read_scalar 前处理避免 Maybe 误 unwrap。

## VM 方法分派免分配轮(本轮下半场)

- **Profiling 方法**:`cargo build --profile dist -p lk-cli --features
  vm-profile` + `LK_VM_PROFILE=1 LK_WORKLOAD_FILTER=x`(直接跑,不必走
  runner);归因用手写 .lk 微基准逐成分剥离(全量/换索引/去 map/去字符串),
  比 opcode 计数更快定位——fraud 的 40ms/44ms 在 `starts_with` 方法调用,
  不在 map。
- **core_methods.rs 改动**:`DetachedStr { Short(ShortStr), Heap(Arc<str>) }`
  取代三处 per-call 分配(`method_name_arc`→`method_name_detached`、
  string receiver 的 `Arc::<str>::from` 全拷、`extract_string_arc`→
  `extract_string_detached`);`dispatch_builtin_method` 按 receiver kind
  (`builtin_receiver_kind`)定向调用四个互斥 dispatcher 之一(原顺序试探
  map→set→string→list,每次 with_slice 都拷参数)。**语义保持**:
  runtime_access(属性优先)仍在 builtin 之前、trait 尾路径不变,
  ArcStr 仅在 trait 尾惰性构造。
- 验证:workspace 全绿、GC stress core 全绿、三套 sanitized 差分零报告、
  Miri lkrt 23/23;全套 bench checksum 一致,geomean 1.175→1.120(dist,
  本地),无 workload 回归。

## math 模块 MIR 吸收(本轮下半场)

- lkrt(host.rs):`lkrt_math_{ceil,round}`(`integer_round` 语义,
  `as i64` saturating cast)、`lkrt_math_sqrt`(负参 eprintln+
  flush_and_abort,对应 stdlib bail)、`lkrt_math_{sin,cos,exp,pow}`。
  abi 8 条新表项(sqrt 标 ReadsHost 防 DCE,其余 Pure)。
- lower:`module_const(module, name)`(GetIndex 的 Module 成员读 arm 先查
  常量表再落 ModuleFn);floor arm 泛化为 floor/ceil/round;abs = 保型
  Select(Int 用 `0 - x` sub 自然 wrap,对齐 VM release 的 wrapping_abs);
  min/max = 同型 Select(返回原值,fcmp olt/ogt 与 Rust `<`/`>` NaN 语义
  一致);module_call_abi 的 F64 形参统一接受 I64 提升(IntToFloat,
  对应 stdlib `number_arg`)。
- **native 链接 `-lm`**(Linux;powf 未内联导致 undefined reference,
  macOS/Windows 由 libSystem/CRT 覆盖)。
- math_demo 解锁(native==VM),examples 6/44,地板 ≥6;差分 +2
  (math_consts_and_fns、sqrt 负参响亮失败保留已刷 stdout)。

## lkrt string-map 性能轮(本轮)

**运行时(lkrt)**:
- `state.rs`:全局 `Mutex<RuntimeState>` → `thread_local RefCell`(const 初始化,
  `with_runtime(f)` 统一入口)。前提:AOT 二进制单线程(lowered 子集无线程);
  句柄/arena 串不得跨线程。单测因此天然隔离,Miri 23/23 依旧全绿。
- arena `owned_strings`/`resources` 与 lkmap 四种句柄 map 全部 FxHash
  (rustc-hash 2,workspace 已有);**map 无 iter/keys ABI,无迭代序依赖**。
- `set` 走 `get_mut` 命中就地更新,miss 才 `to_string()`(更新型 workload
  免每 op key 分配)。
- `lkrt_str_concat_i64(prefix, i64)`:手写 `i64_decimal`(栈上 [u8;20])+
  `CString::from_vec_unchecked`(输入是 C 串,无内部 NUL,免扫描免 realloc),
  单次分配;`str_concat`/`i64_to_str` 同样改 unchecked 路径。
- `lkrt_lkmap_str_{i64,f64}_set_ik(handle, prefix, suffix, value)`:key 在
  lkrt 栈上拼(88B inline + 超长 spill 到 Vec),存储零 key 分配;
  非法 UTF-8 与 `key_str` 一致降级为空 key。
- `lkrt_panic(msg)`:eprintln + `flush_and_abort`。

**lower(aot/lower)**:
- `GetIndexStrI`:key 改 `concat_i64` 单调用(原 from_i64+concat+free 三连);
  `SetIndexStrI`:直接 `set_ik`,完全不物化 key。
- 新 helper `concat_display(acc, v, ty)`:`ty==I64` 融合成 `concat_i64`,
  否则 to_display_str+concat+eager free。`ConcatString`/`ConcatN` 重写为
  逐元素折叠(原先全量预转换再折叠;display 转换无用户可见副作用,重排安全)。
- 新 builtin:`Builtin::{Panic,AssertEq,AssertNe,Typeof}` + `IsNil` opcode。
  - panic:参数空格 join(`join_runtime_display` 语义),0 参 = "panic"。
  - assert_eq/ne:相等按 `runtime_values_equal` 子集(同型标量、Int/Float
    互转 `IntToFloat`、Str 走 str.cmp==0);失败消息 **eager 构造**
    ("expected {b}, got {a}"[+" - {extra}"])免控制流,过 `rt.assert_msg`。
  - typeof:静态标量名(Int/Float/Bool/String/Nil);Maybe 载体
    MaybePresent + Select(值名, "Nil")。**注意**:lower_builtin_call 尾部
    统一写 nil 返回值,Typeof 提前 return 写 Str 结果。
  - IsNil:标量→Const false、Nil→true、Maybe→Not(MaybePresent);
    fused_bool fact 只是 VM 执行捷径,后续分支指令仍在码流中被正常
    lower,直线化安全。
  - SetGlobal 黑名单同步(写这些名字的程序整体拒绝)。

**abi**:`("str","concat_i64")`、`("map_h","str_{i64,f64}_set_ik")`、
`("rt","panic")` 共 4 条新表项(conformance 编译期自动跟随)。

**测量**(dist,min-of-3,AOT/VM):two_sum 0.894 / histogram 1.044 /
log_parse 0.787 / inventory 0.947 / event_join 0.807;对照组标量 workload
0.02–0.24 不变;20/20 checksum 一致。逐步归因:免锁+fxhash+set 免分配 →
2.0–2.4x 降到 1.24–1.51x;concat_i64+模板融合 → 0.76–1.10x;set_ik →
最终值。剩余:histogram 的 let-bound key(`let key = "b${b}"` 复用于
get+set)每迭代仍一次 key 分配。

**测试/防线**:workspace 全绿;手写差分 +9 例(assert_eq pass/fail/msg、
assert_ne、panic_after_output、typeof 标量+map Maybe);examples 地板 3→5
(named_args/named_params 解锁);fuzz 200 例;ASan/UBSan 差分零报告;
Miri lkrt 全绿。

**踩坑**:println 首参为**动态** Str 且带额外参数是既有的拒绝形状
(动态格式串仅单参),差分用例里 `println(typeof(a), typeof(b))` 要拆行;
`materialize_key` 实为通用常量串物化 helper(名字историч)。

## 零捕获闭包切片(本轮,RFC 阶段 4 第一步)

- `GlobalRef::Lambda(u32)`:`MakeClosure`(a=dst, b=fn 索引, c=捕获窗口)
  在 `capture_count == 0` 时按静态函数引用进 builtin_regs(Move/Move2
  传播);捕获闭包拒绝(原样)。
- **间接 Call 去虚化**:`Call` 的 callee 寄存器命中 Lambda → 走抽出的
  `lower_user_call`(与 `CallDirect` 完全同窗布局:结果=base,参数
  [base+1, base+1+c)),进同一 per-callsite 单态化(param_obs/ret_types
  fixpoint)。
- **顶层 lambda 全局**(`let f = |x|…` 顶层是模块全局):
  `prescan_lambda_globals` 认"entry 前缀内、全模块唯一一次 SetGlobal、
  写入值是零捕获 MakeClosure(经寄存器追踪,Move/Move2 传播)"的槽位;
  GetGlobal 命中 → Lambda ref(初始化序安全:前缀写先于一切用户调用);
  SetGlobal(Lambda) 与 prescan 不符 → 响亮拒绝。**entry 局部** lambda
  重赋值天然正确((block,reg) 按 pc 序覆盖);跨块用局部 lambda 拒绝。
- 可达性:`reachable_functions` 沿零捕获 MakeClosure 边扩展(lambda 体
  必须被 lower/emit)。
- fuzz 生成器:helper 30% 概率发成 `let f = |…| expr;`(调用点语法同名
  函数,天然覆盖去虚化);200 例×2 种子干净。
- 差分 +5(顶层/跨函数/函数局部/float 单态/局部重赋值);lower 单测 +4
  (全局 lambda 调用、局部调用、捕获拒绝、重赋值全局拒绝)。
- **验证过的语义边界**:`let f=|x|x; fn g(n){return f(n);} f=|x|x*2;` —
  VM 自身把函数内 `f` 静态绑定到首个赋值(输出一致);双写全局在手写
  artifact 层面必须拒绝(单测锁定)。

## bench runner / 环境(本轮)

- `resolve_lk_bin`:优先 dist/release 中 mtime 更新者(CI perf.yml 本就
  pin dist bin;本地默认原是 release,数字系统性偏慢);`LK:` 行回显真实
  路径。
- 本机源码构建 Lua 5.4.7 → `~/.local/bin/lua`(runner 用 `LUA_BIN` 指定),
  解锁任务 24/25 本地预筛;本地 dist VM/Lua geomean ≈1.18x(仅本地参照,
  CI 用 Lua 5.5 且机器不同)。

此前:全量优化轮(CI 化/-O2/模块 builtin/可变全局/方法分派/.lkm v5/
budget 特化),bench 全套 MIR 原生化 0.329x;"先补再删"轮——legacy text
后端整体退役(-4.8 万行)。

## "先补再删"轮(本轮)

**补(MIR 新形状)**:
- `LoadHeapConst::LongString` → 与 `LoadString` 同构(interned global + hex 转义),
  记入 `const_strs`。
- `GetGlobal` runtime builtin:`builtin_regs: (block, reg) → Builtin` 侧表
  (`Ssa::write` 失效,`Move` 传播,不写 SSA 值——其他用途读到 undefined 即拒);
  `Call`(A=窗口基址,C=参数数)命中 builtin 时下降:
  - println/print:`print_parts` 在 lower 期精确复刻 `format_variadic_runtime`
    (`{}` 逐个消耗、缺参保留字面 `{}`、多余参数空格追加且"格式部分非空才加前导
    空格"——唯一运行时相关的 case(纯 Str 占位符且无字面量)拒绝);动态格式串仅
    单参形状(输出=原串);非 Str 首参=空格 join。`emit_print` 合并相邻字面量、
    display 转换、concat 折叠(eager free),新 `Inst::PrintStr{value,newline}`
    (codegen printf `@lk_str_fmt`/新增 `@lk_str_raw_fmt`)。返回 nil 写窗口基址。
  - **循环坑**:编译器把循环体格式串外提(loop-literal cache),循环体读到的是
    未 seal 的 header phi param,`const_strs` 查不到 → `Ssa::reg_const_str`
    只读回溯到达定义(命中 phi 时**重定向到 phi 自己的 reg/block**——Move 换寄存器;
    visited 按 (block,reg) 防环;多路径必须同一常量)。
  - assert(1-2 参):cond 限 `Ty::Bool` → `ZextBool` + `lkrt_assert(_msg)`。
- `TestEqIntI2`(bench 全套第一道墙):`Exit::FusedCmp2` + MIR `Inst::BoolAnd`,
  false 边走尾随 Jmp,true 边 fallthrough(与 VM false-branch 应用一致)。

**本轮抓到的真实分歧(差分立刻命中)**:native abort 不 flush C stdio →
assert 失败/除零前已 printf 的 stdout 整体丢失(VM 保留)。修复:lkrt 所有
abort 路径统一 `flush_and_abort()`(`fflush(NULL)`),FFI 面 `lkrt_abort`,
codegen `Term::Abort` 调它。差分用例 `assert_false_after_output` /
`div_zero_after_output` 锁定。

**删(legacy 退役)**:
- llvm crate:scalar/(23.8k)、dynamic_containers、straightline_*、subfunction、
  callee_eval、const_display、diagnostics、intrinsics、ir_text、known_key、
  map_mutate、output、stdlib_catalog 全删;backend.rs 重写为纯 MIR 管线;
  options 删 `use_mir_pipeline`/`allow_legacy_fallback`(连同两个 env 开关)。
- 测试:先把 helper 改为过渡 shim 跑测试分类——26 过(MIR 覆盖,保留)/
  224 失败(legacy 覆盖,脚本按失败名批量删,注意别吞文件尾的 `mod` 声明);
  空文件 modules/objects/runtime_builtins 删除。CLI 的 const list/map(混合元素)
  测试改写为"响亮失败 + MIR reason"断言;long_string 测试改断 MIR 特征。
- lkrt:containers.rs(~2000 行 legacy 线性容器 helper)+ abi schema 60 条
  (list.i64/f64/str、map.i64/str、fmt)删除;conformance 自动跟随。
- 文档:backend.md 重写(单管线架构);aot-redesign.md 状态=已退役;
  bench README AOT 段更新(历史 AOT 数字标注为退役后端所测,不可复现直到
  MIR 吸收 os.clock 等模块形状)。

**验证**:workspace 1460 全绿;手写差分 7 组(新增 builtins 16 例)、examples
3/44(地板断言 ≥3)、fuzz 92/100 两种子、sanitizer 差分零报告、GC stress 全绿。

## 正确性优先加固轮(上轮,plan.md)

## 正确性优先加固轮(本轮,plan.md)

按 plan.md 九步全部落地。两个真实 bug + 一处测试 UB:

- **`.lkm` facts 丢失(严重,差分思路当场抓出)**:`FunctionData.performance`
  是 `#[serde(skip)]` → `.lkm` 加载后 facts 为空。实测:`for i in 0..10` 的
  `.lkm` 运行报 `ForLoopI missing performance fact`;`while (a < b)` 的 `.lkm`
  **死循环**(compare-test 无 fact 时 fallback 把下一条指令按 Jmp 解码)。
  `GetIndexStrI`/`SetIndexStrI` 同样硬依赖 fact。修复:`analysis.rs` 全部
  Perf* 类型加 serde derive,`performance` 改 `#[serde(default)]` 序列化,
  `MODULE_ARTIFACT_VERSION` 3→4(旧 artifact 本就半坏,拒绝优于半工作)。
  回归:`module_artifact_round_trips_performance_facts`(编译→JSON→加载→执行=45)。
- **lkrt 测试 UB(Miri 抓出)**:`host.rs` fs 测试用 `&CStr::as_ptr().cast_mut()`
  喂 `CString::from_raw`——SharedReadOnly provenance 不能 Unique 回收。改存
  原始 owned 指针。lkrt 37 测试 Miri 全绿(`-Zmiri-disable-isolation
  -Zmiri-ignore-leaks`;ignore-leaks 因 arena 设计:句柄由 exit 前
  `lkrt_cleanup` 统一释放,单测共享全局 arena 不能各自 cleanup)。
- **bytecode verifier**(`core/src/vm/verify.rs`,新):`.lkm` 是不可信输入,
  而执行器热路径 `stack_index_unchecked`/`relative_pc_unchecked` 在 release
  下无检查 → 损坏 artifact 可静默跨帧读写/跳出函数。加载期逐指令验证:
  寄存器 < register_count、寄存器窗口(call/容器构造/Return/ConcatN)、跳转
  目标 ∈ [0, len]、常量池/函数/native/global/capture 索引;facts 验证:
  for_loop/compare_test/fused_bool 目标、call_base 窗口、global slot、key
  fact 常量索引;`ForLoopI` 必须有 fact,compare-test 无 fact 时 pc+1 必须是
  Jmp。`into_module()` 无条件跑;`compile_module` 在 `debug_assertions` 下
  自验(整个测试套变成 verifier 的防误杀语料)。13 个单测。
  - 操作数语义逐 opcode 对照执行器核实(exec.rs 主循环 + dispatch/const_load/
    callable_ops/cell/handler/container/support)。关键点:Call/CallDirect/
    CallNamed 的 A=窗口基址(B 只有 7 位会截断)、CallNamed bx=pos(7b)|named、
    MakeClosure 的捕获窗口大小取 callee.capture_count、GetFieldK/SetFieldK 的
    C 是字符串常量索引、branch_i4 的 12 位偏移。
- **MIR validate() 进生产路径**:`backend.rs` 在 lower 成功后、render 前无条件
  `lk_aot_mir::validate()`(此前只在 codegen 测试里跑,"renders a validated
  module" 前置条件生产路径无人保证)。llvm crate 新增 `lk-aot-mir` 依赖。
- **legacy fallback 改 opt-in**:`allow_legacy_fallback: Option<bool>`(env
  `LK_AOT_LEGACY`),默认关——MIR 拒绝直接报错(带 Unsupported reason +
  opt-in 提示);`use_mir_pipeline=Some(false)` 仍是显式直选 legacy。
  **~200 个 llvm 测试原来靠静默 fallback 存活**(251 中 199 失败),批量改为
  `legacy_fallback_options()` 显式 opt-in(tests.rs helper,与 legacy 同退役);
  CLI 3 个 legacy-only 形状测试(const list/map、long string)加
  `LK_AOT_LEGACY=1`。错误信息保持 "LLVM native lowering does not support" 前缀。
- **GC stress**:`LK_GC_STRESS=1` 时 `collect_pending_garbage` 每安全点强制
  collect(不在分配点——新句柄未入根)。core/stdlib/cli 全测 + 复杂 examples
  stress 下全绿。
- **examples/ 差分**(`cli/tests/examples_differential_test.rs`):整树拷贝到
  temp(相对 import 可用),逐 .lk:MIR 编译→VM vs native 比对;不可 lower
  记录 reason 快照。当前 2/44 可 lower(fib、numeric_auto_promotion),42 个
  卡 GetGlobal(println/assert 也是 global!)/LoadHeapConst/MakeClosure/
  NewObject/IsNil/NewRange。地板断言 ≥2 防退化。
- **生成式差分 fuzz**(`cli/tests/aot_fuzz_differential_test.rs`):splitmix64
  种子化生成器,限定 MIR 子集(标量、计数 while、直接调用、List<i64>、const
  key map、模板串插值)。观察面=把全部活变量插进 `return "${v0}|${acc2}"`
  模板(println 会引入 GetGlobal 不可 lower,仅留 8% 概率作 Unsupported 探针)。
  VM 必须接受生成程序;AOT 拒绝必须是 graceful Unsupported(lower totality,
  stderr 含 panic 即 fail)。默认 40 例(30 可比较),`LK_FUZZ_CASES`/`LK_FUZZ_SEED`
  放大:400+200 例两种子全部干净。
  - 生成器踩坑:LK `/` 是 Int/Int→Float(整数表达式只用 %);`if (expr) != x`
    会把首个括号组当条件(if 条件必须整体加括号)。已记入 docs/semantics.md。
- **sanitizer**:`native_executable.rs` 支持 `LK_NATIVE_SANITIZE` 透传
  `-fsanitize=`;native cache key 加入该变量与 `LK_AOT_LEGACY`(缓存正确性),
  并注释 import-content-not-in-key 地雷。全部差分语料 ASan/UBSan 零报告
  (arena cleanup 无泄漏顺带被 LSan 验证)。Makefile:`miri-lkrt`、
  `sanitized-differential`、`gc-stress`。
- **docs/semantics.md**(新):golden vectors 第三仲裁(div/0、缺失键算术、
  nil 静默返回、float 显示、`/`→Float、负索引、退出语义 VM exit-1 vs native
  abort-134 等),含维护约定:分歧先查表,裁决后加条目+差分用例。
- **backend.md** 顶部两层架构章节更新(validate 强制 + fallback opt-in)。

## AOT 重设计收官轮(本轮)

- **差分 harness 一等公民(§6)**:`cli/tests/aot_differential_test.rs`,69 例
  (标量/控制流/函数/list/map/字符串)。每例:VM 运行 vs MIR 管线 native 运行,stdout +
  成功/失败逐项比对;`Path::New` 断言确实走 MIR 管线(IR 含 `ModuleID = 'lk_aot'`)。
  含失败语义用例(div/0、缺失键算术 → 双方都响亮失败、stdout 均空)。
- **MIR 快照(§6)**:`lk_aot_mir::render()` 稳定行式文本;`aot/lower/tests/mir_snapshots.rs`
  6 形状 golden(直线除法 / if-else / 循环 / 直接调用 / 列表+动态索引 / map 查找)。
- **ABI 单一真相闭环(§3.3)**:表 → `for_each_abi_fn!` 数据宏(140 条,`aot/abi`)。
  `ABI_FUNCTIONS` const 与 lkrt `abi_conformance_test` 从同一宏展开:符号存在/`extern "C"`/
  arity 由 fn-pointer coercion **编译期**强制;参数/返回寄存器类(i64/f64/ptr/void)测试期
  与 schema 比对(StrPtr/Ptr 同 class——LLVM 不透明指针下调用约定相同)。
- **所有权(§3.4)**:默认 arena——`arena_c_string`/`arena_handle` 注册所有字符串+容器句柄
  (state.rs `owned_containers: Vec<(usize, drop_fn)>`);codegen 在 entry 各退出点打印后发
  `lkrt_cleanup()`。lower 对 ConcatString/ConcatN 已知死亡的 display 临时串与中间累加串发
  eager `lkrt_string_free`(`to_display_str` 返回 `(ValueId, fresh)`);循环内插值不再累积。
- **`Unsupported::reason()` + Display(§3.5)**:每变体一句解释;双后端都拒时错误带双原因。
- **Maybe<Str>(元素矩阵补齐)**:`lkrt_lklist_str_get_pair -> LkMaybeStr {ptr,i64}` +
  `lkrt_maybe_str_unwrap`;MIR `Ty::MaybeStr` + `ListGetMaybeStr`/`UnwrapMaybeStr`;
  `MaybePresent.float: bool` 重构为 `maybe_ty: Ty`(三载体)。差分:动态索引 concat 循环
  `"abc"`、越界→nil、负索引→"c"、`==nil` 全 =VM。
- **翻默认**:gate 默认开;退出通道 `LK_AOT_MIR=0` **或** `LlvmBackendOptions::
  use_mir_pipeline: Option<bool>`(测试用选项 pin,避免进程内 env 竞态)。
  - llvm crate:34 个 legacy-IR 结构断言测试(`*_without_shell` 等)pin
    `legacy_text_backend_options()`(tests.rs 的 helper;它们是 legacy 后端的覆盖,随删除退役)。
  - CLI:7 个集成测试改写为 MIR 断言(`ModuleID='lk_aot'`、`phi i64`、`call i64 @lk_fn_1`、
    `@lkrt_f64_to_str`、`@lk_str_0`),并加真实运行断言(loop=6、call=42、f64=3.75)。
  - **差分抓出 legacy 真实分歧**:`return nil;` legacy native 打印 `nil` / VM 与 MIR 打印空。
    nil 测试改为锁定 VM 行为(空输出)并注明。
- **文档**:RFC 状态改"已实现" + §9.5 收官记录;`backend.md` 顶部加两层架构章节
  (MIR 默认 + legacy fallback 的能力面)。

### 本轮踩坑/决策

- fish 下 `sed` BRE 捕获组把 `\1` 写成字面量 → 语料文件损坏,重写文件解决;LK `while`
  条件必须带括号(`if` 不用)。
- CLI 差分/集成测试里 exit code 只比 `success()`(VM 错误 exit 1 vs native abort 134,
  语义等价"响亮失败")。
- 零参小函数会被前端内联 → CLI 测试不能断言 `call @lk_fn_1()`,改断言 display helper。
- lkrt 测试回收 arena 字符串用 `lkrt_string_free`(不再 `CString::from_raw`,避免注册表
  悬挂条目)。
- `render_ret` 的 cleanup 必须在 printf **之后**(打印值可能是 arena 串)。

## 剩余(RFC §1 非目标 / §7 约定后续)

1. 阶段 4:闭包/间接调用/可变全局 + `__lk_call_method` 方法分派(`.sort()`/`.pop()` 等)。
2. legacy text 后端整体退役(待 MIR 吸收其独有形状;连同 34 pinned 测试 +
   `dynamic_containers/`)。

## AOT 重设计(aot-redesign.md,更早轮次)

新 crate 家族(`aot/`):`lk-aot-abi`(schema 单一真相,零依赖)、`lk-aot-mir`(类型化
SSA + `validate`)、`lk-aot-lower`(bytecode→`Result<MirModule,Unsupported>`,总函数)、
`lk-aot-codegen`(total `render_module`→LLVM)。

- **阶段 0**:abi crate(表迁自 intrinsics.rs;llvm/intrinsics.rs 缩为薄适配;lkrt 复用
  `ABI_VERSION`)。除零守卫 `lkrt/src/arith.rs`(`lkrt_{i64,f64}_{div,mod}_checked`),
  emit 主标量/混合浮点/call-slot 改调 helper —— `x/0` native 由 UB 变确定性 abort(exit
  134),float `/0` 由静默 inf 变 abort。`lkrt_abi_check` 在 main 入口(不改 entry CFG)。
  修了 3 个断言旧 `sdiv/fdiv/lk_divisor_zero` 的测试(gcd/mixed-float/float-guard)。
- **阶段 1**:mir(block 参数替代 phi;容器=`Inst::Call{AbiRef}`;`Ty` 封闭枚举=可 lower
  子集定义)+ codegen(block 参数→phi 扫前驱;Div/Mod→helper;entry→`@main` 打印)。
  `examples/demo.rs` `20/4`→clang→liblkrt→运行 `5` 端到端验证。
- **阶段 1b**:lower 首切片(无参无捕获单入口 + 标量直线整数);`lowers_straightline_
  integer_division` 全链路(手写 artifact→lower→validate→render→guarded-div LLVM)。
- **验证**:`cargo test --workspace` **1622 passed / 0 failed**;四 crate 各自单测
  (abi 2 / mir 4 / lower 3 / codegen 2)。
- **下一步**:绞刑架切换 `lk-llvm` 入口到 `lower→codegen`(Unsupported 回退旧后端)+
  扩片(float/比较/分支/调用),见 aot-redesign §9.5;阶段 2-4(容器句柄化 / 控制流块 /
  闭包+间接调用+可变全局)见 §7。

### 关键决策/踩坑

- **ABI 版本 assert 不能拆 entry 块**:条件分支会破坏下游 `[x,%entry]` phi;改用 lkrt
  `lkrt_abi_check(expected)` 内部 abort,main 入口只加一条 call。
- **除零 helper 不标 Pure**:标 `ReadsHost` 防 codegen 未来把"可 abort"当纯函数 DCE 掉
  (当前 text 路径无属性,是元数据前瞻)。
- **迁 intrinsics 表用 `cp` 复制再改**,避免手抄 1200 行;类型名 replace_all
  (`NativeIntrinsic*`→`Abi*`),llvm 侧 re-export 旧名减少 crate 内改动。

---

# (更早)LLVM 容器操作下沉到 lkrt

目标:落地 `docs/llvm/aot-gaps-and-lkrt.md` —— 把单态动态容器操作从 llvm crate
的手写 LLVM IR 下沉为 `lkrt` 的 typed ABI helper,llvm 侧只声明 intrinsic + 生成调用。

## 架构关键点(供后续 batch 复用)

- LLVM 后端是**文本 IR 生成**(非 inkwell)。入口 `llvm/src/llvm/backend.rs:66`
  `compile_native_scalar_main_artifact`,全有或全无,任意未覆盖形状整程序 `bail!`。
- 容器 helper 原本是"**手写 LLVM IR 纯函数**(`lk_*_i64_list`,只操作 `ptr+len`)
  + `emit_*` 生成 `call`"。见 `llvm/src/llvm/dynamic_containers/*.rs`。
- **intrinsic 声明**:`llvm/src/llvm/intrinsics.rs` 的 `NATIVE_INTRINSICS` 数组是
  registry;`native_intrinsic_declarations()`(被 `ir_text.rs:69` 调用)为所有
  `lkrt_` 符号自动生成 `declare`。往数组加条目即可。
- **helper IR 注入点**:`llvm/src/llvm/scalar/blocks/finalize.rs:27-31`
  push `native_dynamic_*_helpers()` 返回的 IR 定义到最终 .ll。
- **链接**:`llvm/src/native_executable.rs` 用 clang 编译 .ll 并
  `--whole-archive liblkrt.a`(macOS 用 `-force_load`)。`link_anchor()` 防裁剪。
  新增 `lkrt_*` 符号自动链上。
- lkrt ABI 基础设施在 `lkrt/src/abi.rs`(`c_str`/`owned_c_string`/`status`/
  `write_out`),资源管理在 `state.rs`。`ABI_VERSION=1`。

## 下沉一个布局的标准步骤(模板)

1. `lkrt/src/containers.rs` 用 Rust `#[unsafe(no_mangle)] pub unsafe extern "C"`
   实现纯函数,签名与旧手写 IR helper **逐字对齐**(`ptr, i64 len, ...`)。
2. `lkrt/src/lib.rs` `pub use` 导出。
3. `intrinsics.rs` 往 `NATIVE_INTRINSICS` 加条目(effect=Pure,module 用
   `list.i64` 之类)。
4. 对应 `dynamic_containers/<layout>.rs`:`native_dynamic_*_helpers()` 返回 `""`
   (helper 已下沉),`emit_*` 里 `@lk_*` 调用符号改成 `@lkrt_*`。
5. 同步 `llvm/src/llvm/tests/modules.rs` 里断言的符号名。

## 语义对齐备忘(i64 list,已核对手写 IR)

- `contains`→1/0;`index_of`→找到 index 否则 -1。
- `reverse`:dst[i]=src[len-1-i];`sort`:升序(i64 相等无差别,用 sort_unstable)。
- `pop`:返回末元素,空返回 0,**不改 list**。
- `slice_range`:start/end 各 clamp 到 `0..=len`,再 `end=max(end,start)`。
- `push`:追加,新长 len+1。
- `insert`:index clamp `0..=len`,新长 len+1。
- `remove_at`:紧凑复制,返回被移除值(越界返回 0 且全量复制)。
- `set`:复制并替换 index,返回旧值(越界返回 0),长度不变。

## 已完成(batch 1:DynamicList<i64>)

- `lkrt/src/containers.rs`:10 个 `lkrt_list_i64_*` 函数 + 单元测试(4 个,全过)。
- `lkrt/src/lib.rs`:导出。
- `intrinsics.rs`:10 条 registry 条目(module=`list.i64`,Pure)。
- `dynamic_containers/i64_lists.rs`:helper 返回 `""`,emit 改调 `@lkrt_list_i64_*`。
- `tests/modules.rs`:断言符号名同步。
- 注意 `@lk_concat_i64_list`(bool-list concat 走 i64-slot)**未**下沉,保留。

## 关键 bug 与修复:in-place 别名(重要)

首版用 `copy_from_slice` 实现 slice/sort/push,端到端跑 `xs.slice(1,3)` 时 native
panic:`ptr::copy_nonoverlapping requires ... non-null ... ranges do not overlap`。

- **根因**:LLVM 把 `xs.slice(..)` / `xs.sort()` lower 成 `src == dst` 同一 buffer
  (in-place)。`copy_from_slice` 走 `copy_nonoverlapping`,重叠即 panic;而且在
  Rust 里同时持有别名的 `&[T]` 和 `&mut [T]` 本身是 UB(debug 断言直接 abort)。
- **修复**:`containers.rs` 全面改用**裸指针** + `ptr::copy`(memmove);只向更低/
  相等目标索引前向写;`sort` 先 memmove 物化再取单一 `&mut` 排序;`insert` 先右移
  尾段。新增 `in_place_aliasing` 单测锁定 `src == dst`。
- 详见 `docs/llvm/aot-gaps-and-lkrt.md` §7「关键教训」——后续 batch 必读。

## 验证结果

- [x] `cargo test -p lkrt`:5 tests pass(含 `in_place_aliasing`)。
- [x] `cargo test -p lk-llvm`:251 passed,0 failed(含 IR 符号断言)。
- [x] AOT 端到端:`lk compile` native 输出与 VM 逐项一致
      (contains=true / index_of=1 / pop=2 / reverse=2 / slice=1);
      `nm` 确认 10 个 `lkrt_list_i64_*` 符号已链接进二进制。
- [~] `cargo test --workspace --all-features`:进行中(后台 buhvcrw5n)。

## batch 2/3/4 进展

- **batch 2 `list.f64`**(14 方法):`containers.rs` 内部 helper 泛型化后复用;
  sort/contains 用手写 selection sort / `==` 匹配 `fcmp` NaN 语义。lkrt+llvm 全绿,
  端到端 contains/index_of/sort 一致。
- **batch 3 `list.str`**(14 方法):元素 `*const c_char`;结构操作移动指针,
  push/insert/set 用 `dup_cstr`(Box::leak 泄漏,匹配 strdup);空/越界返回稳定空
  C 串;`CStr` 比较(strcmp 字节序)。注意 slice/take/concat 原在
  `dynamic_containers.rs` 顶层混合池 + `subfunction.rs`,已一并下沉。lkrt+llvm 全绿。
  str list receiver 形状受既有 lowering 长尾限制无法 CLI 端到端触发(非下沉问题)。
- **batch 4 `map.i64`**(6 helper:lookup/set × int/f64/ptr):用泛型 `map_lookup`/
  `map_set`;present-bit 由 emit 侧处理(helper 返回 found + 写 out),strdup 也在
  emit 侧,下沉很干净。map 的 has/delete/iter/values/keys 是**内联 IR**(非 helper
  池),不在下沉范围。lkrt 11 tests 全过;llvm 测试进行中。

## batch 5 进展(string-key map + decimal_len,本轮完成)

`map.str` 复合短字符串 key(prefix 串 + 整数后缀)。分三步,全部 drop-in / 镜像
int/f64 版本,签名与旧 `@lk_*` 手写 IR 逐字对齐:

- **Phase A(int/f64 + split_key + decimal_len)**:`dynamic_containers.rs` 的
  `emit_dynamic_string_{int,f64}_map_{set,get}` 早已是 helper-call 形式,但调的是
  `native_dynamic_container_helpers()` 里的**手写 IR** `@lk_{lookup,set}_string_{int,f64}_map`
  / `@lk_split_string_int_key` / `@lk_i64_decimal_len`。本步:删这 6 个手写定义(该 fn
  现只剩注释),call site 换成 `@lkrt_map_str_{int,f64}_{lookup,set}` / `@lkrt_map_str_split_key`
  / `@lkrt_i64_decimal_len`。
- **Phase B(ptr map set/get)**:`containers.rs` 复用泛型 `str_map_lookup/set` 加
  `lkrt_map_str_ptr_{lookup,set}`;`string_maps.rs` 的 `emit_dynamic_string_ptr_map_{set,get}`
  从内联循环(`emit_string_map_{set,get}_loop`)改调 helper,删这两个 loop fn。
- **Phase C(has/delete)**:加 `lkrt_map_str_contains` + 泛型 `str_map_delete` 的
  `lkrt_map_str_{int,f64,ptr}_delete`(压缩式,`out_value`+`out_present`,容忍 `src==dst`,
  `dst_i<=i` 前向写);`emit_dynamic_string_map_has`/`_delete` 改调 helper,删
  `emit_string_key_match`/`emit_string_map_copy_item`。

改动文件:`lkrt/src/{containers.rs,lib.rs}`、`llvm/src/llvm/intrinsics.rs`(map.str
新增 ptr_lookup/ptr_set/contains/int_delete/f64_delete/ptr_delete 6 条)、
`dynamic_containers.rs`(删 helper 池 6 定义 + 6 call site 改名)、
`dynamic_containers/string_maps.rs`(ptr set/get + has/delete 改调 helper,删 4 个内联
fn)、`tests/{basic,strings,modules,direct_calls}.rs`(符号断言同步 + 修复样板断言)。

### 关键坑:helper 池样板导致的假断言

`native_dynamic_container_helpers()` 无条件注入每个模块 IR,所以 `@lk_i64_decimal_len`
的 `select i1` 和 map helper 的 `call i32 @strcmp` 出现在**所有**模块里。删除下沉后,
3 个测试(bool 直接调用、closure、模板比较)的断言(`select i1`/`strcmp`)失效——它们
其实在测样板。已改断被测程序自身:bool/字符串比较常量折叠成静态串经 `@lk_str_fmt`
打印(`@lk_block_return_static`);模板 `${x+y}==N` 比较分解为 `icmp eq i64`。

### 语义核对(map.str,已对旧手写 IR 逐字核对)

- `split_key`:尾部 ASCII 数字为后缀;"raw"(空/全数字/无尾数字)保留原指针 + number 0,
  否则 leak 截断 prefix。与 `@lk_split_string_int_key` 的 raw/parse 分支等价。
- lookup/set/contains:`strcmp(prefix)==0 && number==`;lookup 只在命中写 `out` 返回 1/0,
  emit 侧存 present。
- delete:非匹配项拷进 dst(prefix/number/value),命中写 `out_value`+`out_present=1`,
  返回 dst 长度;emit 侧预存 missing 默认 + present=0。
- decimal_len:0→1,负数含 `-`;单测对 `i64::MIN/MAX` 与 `v.to_string().len()` 逐项核对。

## 验证结果(本轮)

- [x] `cargo test -p lkrt`:21 passed(+4:split_key / str-map int+ptr set/lookup/
      contains/delete / decimal_len)。
- [x] `cargo test -p lk-llvm`:251 passed。
- [x] `cargo test --workspace --all-features`:**1609 passed,0 failed**(cargo 退出码 0)。
- [x] `nm` 确认 `lkrt_map_str_*` + `lkrt_i64_decimal_len` 链接进 native 二进制;
      string-int-map `m.k=v` set/get 端到端与 VM 一致。
- [~] 非折叠 native RUN string map helper:受阻(`map` 模块非 CLI 可达 + 常量折叠 +
      env.get_or 既有 bug `@lk_const_str_0` undefined)。靠 llvm 结构断言(动态词频 /
      has/delete over f64/bool/str map)+ lkrt 单测 + list helper 已证同款调用约定覆盖。
- [—] bench:改动仅在 llvm/ + lkrt/(AOT),core/VM 解释器零改动,perf 门禁(VM LK/Lua
      比)不受影响,未跑重型 bench。

## 剩余待下沉

- `@lk_concat_i64_list`(bool-list concat 借 i64-slot ABI)仍手写 IR。
- 两种 map 的 `iter`/`values`/`keys`(索引拷贝 / snprintf 重建 key,收益低)。
- 结构性硬限制(aot-gaps §2.1:闭包 / 间接调用 / 可变全局)—— 单独立项。

---

# 2026-07-02/03:一等函数值收官轮(list display + 闭包四步)

## 落地内容(5 commits,dev 待 push)

1. **docs: plan** — 本轮计划落 plan.md。
2. **feat: list display lowering** — lkrt `lkrt_lklist_{i64,f64,str}_display`
   (VM-exact `[`+逗号+`]`,str 元素 `{:?}` 引号);lower `to_display_str`
   增 `containers: bool` 分叉:print/panic/assert 语境渲染,ToString/模板
   插值/concat 语境拒绝(VM 的 scalar-only 路径,曾抓到真分歧:native 把
   `"${xs}"` 渲染成功而 VM 报 "object cannot be converted to string")。
   `docs/semantics.md` 新增容器 display 节(map 顺序不可移植,排除子集)。
3. **feat: 多身份 lambda 克隆特化** — `sig.specializations:
   (orig, identity_vec) → clone`;克隆 = FunctionData 逐字节拷贝 +
   `lambda_params` 预填;每原函数上限 8;specialized×plain_called 双态
   响亮拒绝。closure.lk 第 4 节形状 native==VM。
4. **fix: VM inline cell-promotion 丢失(真 miscompile)** —
   `inline_direct_function_body` 恢复 `cell_locals` 时把 inline 实参 lambda
   触发的外层变量 promotion 记录一并回滚,但发射的代码已把变量寄存器就地
   重绑为 cell;第二个调用点重新 promotion,把旧 cell(Obj)当初值存入新
   cell → 闭包体内 Int + Obj 报错。触发条件:helper 带分支(可 inline 的
   多语句体)+ 两个内联捕获 lambda 实参。修复:恢复时保留"绑定未被 inline
   遮蔽"的新 promotion(binding-equality 判据,别名传参场景同样正确)。
   回归测试断言 StoreCellVal 恰好 1 次 + 执行值 1707。
5. **feat: 捕获闭包作参数** — `LambdaIdentity { fidx, captures }`(env 值
   不进身份键,同 fidx 不同 env 共享克隆);调用点把 Closure ref 的 cell
   按当前块解析成值,追加为隐藏尾实参(签名序:可见参数 → 各擦除闭包的
   env 块 → callee 自身捕获);`lower_function` 为擦除的捕获身份分配隐藏
   env 参数并播种 `Closure(fidx, [Value(env_param)…])`,调用间 mutation
   可见;跨 helper 转发自然嵌套。Move/Move2 的 builtin ref 查找升级为
   `builtin_ref_at` 跨块回溯(cell 内容仍块局部,分支形状响亮拒绝)。
6. **feat: 返回闭包(静态摘要)** — `sig.ret_closures[f] = (fidx,
   [Param(k)…])`:非 entry、唯一 return、纯函数体(白名单:常量加载/
   Move/cell 建立/MakeClosure/Return1,任何可 abort/副作用指令取消资格)、
   捕获全映射到自身参数 → 调用点用实参值构造 Closure ref 播种结果寄存器,
   不发射调用,函数体永不发射(fixpoint/final 双跳过)。工厂结果直通闭包
   实参路径(`apply(multiplier(3), 5)` ✓)。**关键修复**:call-site 事实
   (specialized/plain_called/conflict)每 pass 重置重推导——摘要在 pass 1
   尚未发现时实参是普通值,pass 2 变 ref,陈旧标记曾触发假 function-vs-
   value 冲突;收敛后的 conflict 才是真的。counter/多 return/副作用工厂
   全部响亮拒绝(手工验证 neg1/neg2/neg3)。

## 验证矩阵(全绿)

- workspace 全量(95 个 test result: ok,0 failed)
- 手写差分 8 组(新增 closure_as_argument / _forwarding /
  closure_returned / _as_argument)+ examples corpus + fuzz
- fuzz 新形状:变体 12(同 helper 捕获/零捕获身份混跑 + 调用间 mutation;
  闭包工厂 25%)、变体 13(分支 helper + 两内联捕获 lambda = inline 回归
  形状,35%);4 个种子(默认、20260702、777、20260703、99881)×200-300 例
- ASan/UBSan:三套差分全过(手写/examples/300 例 fuzz)
- Miri lkrt 24/24;`RUSTFLAGS="-D warnings" cargo test --no-run` 干净
- AOT bench 20/20 checksum;AOT/LK geomean **0.251x**、AOT/Lua 0.265x、
  VM/Lua 1.055x(RUNS=1 参考值)
- fmt + clippy 0 警告

## 关键发现/边界

- closure.lk 全文件只差 **list 结构相等**(第 6 节 `evens == [2,4,...]`,
  CmpInt over (ListI64, heap-const list))——独立特性,下轮候选 #1。
- cell 内容跨块不流动是当前主要闭包限制(`cell_vals` 按 (block, cid)
  键),升级路径:cell 并入 Braun 虚拟槽做 phi(候选 #2)。
- 返回闭包的静态摘要覆盖了观测语料,runtime `{fn_ptr, env}` 表示
  **不需要**(plan 原设想的重方案避免了)。
