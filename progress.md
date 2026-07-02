# 实现进度

**当前**:"先补再删"轮**已完成**——MIR 补形状后 legacy text 后端整体退役,
workspace **1460 测试全绿**(-4.8 万行,~240 legacy 测试退役)。

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
