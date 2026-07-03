# Handoff

**目标(`/goal`)**:把 `plan.md` 划分为多步、逐个完成、每步 push。**用户:允许短期回归、fix-forward。**
细节台账在 `progress.md`。已推送 **89+ commit** 到 `dev`。完成度:**✅35 · [~]1 · [ ]1 · [!]1**。
本轮:**M0.7/8 no_std flip、M2.2 堆错误值、M4.2(Exit 达成:覆盖 14/50 >11 + 程序粒度回退)**(三里程碑)+
**8 个 AOT 覆盖 win**(IsList/SliceFrom×3/StringSplit/IsMap+map-Contains+MapRest/Raise,列表·map·不可反驳 let 解构+str.split 原生化,覆盖 11→14)+ conformance/健壮性/文档。

## ✅ 已完成/大幅推进(遍及全部 6 相)
- **Phase 0** 完整;**M3 完整**(嵌入 API + register_fn + 多实例 + 沙箱 builder + C ABI 端到端跑出 42 + `eval_value` 类型化结果)。
- **M0**:🎯 去全局状态 · **lk-values 抽取 + 真 no_std**(wasm32)· **lk-hal**(no_std)· CI no_std 冒烟 + **lk-core 无 async 可构建守卫**。
- **M1**:VM(source)==VM(bytecode) 差分 · conformance 声明 · **`.lkm` 字节码缓存**(`LK_CACHE=1`,坐实其为缓存非分发)。
- **M2**(错误/沙箱模型完整):**pcall/error · 可捕获 assert · 一等基本错误值 · try/catch · fuel+内存+模块白名单三沙箱**;
  **traceback 完整**(`Function.debug_name` 下沉字节码 + 错误传播分支 push ctx 调用栈 + pcall 捕获清空,CLI 打印命名调用链;
  仅 Err 路径零热成本、不碰 to_string 断言)。**唯一遗留**:堆对象(String/List)一等错误值(需 GC rooting 跨展开)。
- **M4**:**AOT Tier 0**(`lk bundle`→自包含 ELF)· AOT==VM 差分门禁(CI+ASan/UBSan/fuzz)。
- **M5**:**WASM(wasm32+CI)· lk fmt · M5.4 删中心化注册表(-5000 行,收敛为 git+lockfile 去中心化依赖)· LSP 双轨**。
- **新 crate**:`values/`(L0 no_std)· `hal/`(L0 no_std)· `api/`(L5,ffi+lk.h+eval_value)。
- 端到端验证:C ABI、Tier 0 exe、三沙箱、一等错误值、try/catch、WASM、fmt、git 依赖 fetch、字节码缓存。**全量 1449 tests 0 失败。**

## 本轮完成 ①:M0.7/8 —— lk-core VM 核心翻转为 `#![no_std]` ✅
- **地基**(commit `0b2b284`):`core/src/compat.rs`(collections/sync::Mutex/path/prelude 兼容层,std 默认逐字节不变);
  140+ 文件 std→core/alloc/compat + prelude;错误 2481→~20。
- **flip 完成**(commit `2ec839a`):`cargo build -p lk-core --no-default-features` 现为**真 no_std 构建 0/0**。
  std-only 叶子按 `std` feature gate(no_std 语义正确不可用):`stmt::import` ModuleResolver 文件/包解析(保留
  registry 内存解析)、macro_system 文件加载宏(保留 builtin+声明宏)、proc_deps 指纹、procedural/proc_function/derive
  外部 proc-macro 进程(3 个 external 叶子在 provider 检查后 gate)、ResourceValue.handle 走 compat Mutex。
  dead-under-no_std 机器用 `#[cfg_attr(not(std), allow(dead_code, unused_imports))]` 于 mod 声明消警告。CI 守卫升级为真 no_std。
- **⚠️ 更正上轮误判**:flip **不需**依赖级 no_std 改造。host 上 `#![no_std]` 只禁 lk-core **自身源码**用 `std::`;
  其 std 依赖(anyhow/dashmap/serde_json)自身链接 std、在 host 编过 → 无需改依赖 Cargo、无需抽新 crate。
  (真 bare-metal 跑 VM 才需 no_std 依赖替代,那是 M5.2 future。)

## 本轮完成 ②:M2.2 堆对象一等错误值 ✅
`error(String/List/…)` 一等携带(`RuntimeModuleState.pending_raise_root` GC-root pin 跨展开),pcall 原样取回;
uncaught 用 `LkRaisedValue.rendered` 出消息。commit `a3533a4`。GC-stress 1095 验证 rooting。**M2.2 无遗留**。

## 本轮完成 ③:M4.2 程序粒度回退(消除「全有或全无」问题 2)✅
`lk compile FILE`(native)遇 `Unsupported` 时不再整程序报错,warn + 回退 **Tier 0 VM bundle**(内嵌解释器)
→ 任何有效程序都能 compile。先解析(真错误暴露)再试 native;`LK_AOT_NO_FALLBACK=1` 关回退供 strict native-only。
commit `3c0a83e`。cli 93 / 全量 1451 全绿。**Exit「任意 .lk 可 compile(Tier 0 保底)」达成(程序粒度)**。

## 本轮另完成:conformance + 健壮性 + 文档
- **M1 conformance**:`examples/syntax/error_model_edges.lk`(嵌套 pcall/堆错误值/多帧传播/运行时错误,锁定 M2 语义,走三重 gate)。
- **M4.2 健壮性**:Tier 0 回退失败时给组合错误(native 不可 lower + Tier 0 不可用)。
- **文档**:README pkg 速查 git+lockfile 化(M5.4);`docs/llvm/backend.md` 更正「无回退」为记录 M4.2 Tier 0 回退。

## 剩余(真正的深度架构工作,均确认无干净子单元)
- **[~] M4.2 AOT 覆盖 typed-subset 扩展**(**可复用模式已验证**):type+ops 已存在、仅缺某 opcode lowering 时,加该
  opcode 是有界低风险 win。**两法**:(a) const-fold opcode(零 runtime,如 `IsList`);(b) 小 lkrt 函数+abi 声明+lower arm
  (如 `SliceFrom`:lkrt `lkrt_lklist_{i64,f64,str}_slice_from` 类比 `map_fn` 的 arena_handle、negative abort 匹配 VM)。
  均由 **native==VM 差分 + ASan/UBSan** 守卫。本轮 **8 个 win**:`IsList`、`SliceFrom`(i64/f64/str)、`StringSplit`
  (lkrt `str::split`零语义风险)、`IsMap`+map `Contains`(str+int key,`key in map`=`MapGetMaybe`→`MaybePresent`)、
  **`Raise`→abort**(不可反驳 `let [a,b,c]=xs` 形状守卫;安全因 `TryBegin` unsupported→有 try/catch 的程序已回退 Tier 0→
  原生模块必无 handler)。commit `ef55604`/`6b52a3a`/`47199c1`/`8755e02`/`fbcb2d9`/`23845c0`。
  → 列表形状/rest 解构、`str.split()`、`key in map`+map-shape 解构、不可反驳 let 解构均原生编译。**覆盖 11→14/50**。
  **下一候选**:`MapRest`(`{..rest}` 类比 slice)、int-key map、**return-type 统一**(I64 vs MaybeI64 `ReturnTypeConflict`;
  经核实 0 现有例受阻,价值低)。**更深 blocker(需扩类型系统,多天)**:`LoadHeapConst` mixed/heap 常量(mixed list `[1,"a"]`
  无同构类型)、`NewObject` 结构体、`NewRange`、`ToIter` 迭代器、**`try/catch`(`TryBegin` 需原生错误处理→解锁 pcall/error 原生)**、
  动态 `Call`/method/GetGlobal builtin(pcall/error/string 方法)、`operand type outside subset`(operators/control_flow 的动态类型)。
- **[ ] M4.2 逐函数 Tier 1 混合**:同一程序 native + VM-executed 函数 + native↔VM ABI 桥——多天(程序粒度回退已达成)。
  MIR lower 已按 CallDirect 可达性处理多函数(dead 函数已跳过)。
- **[ ] M2.5 stackless**:VM 执行模型重写(trampoline `Sequence::step`)——多天,触碰最热路径+bench 门禁,partial 不可安全提交。
- **[!] callable trait 反转**:`CallableValue::Runtime(Arc<vm::RuntimeCallable>)` @ `val/runtime_model.rs`,内嵌
  `Arc<Module>`。改 `dyn` 需同步改 GC 追踪/跨模块传递/调用点——枚举变体一变全部 match 原子断裂。lk-core **内部**优化,非阻塞。
- **[~] M5.1 三 profile**:lk-core 现已承载 alloc(no_std,`--no-default-features`)↔ full(std,默认)二档 feature;
  bare=lk-hal、alloc(L0)=lk-values 已 CI 冒烟。**待做**:M5.2 full-VM-on-MCU(需 dashmap/anyhow/tokio 的 no_std 替代)。

## 护栏 & 续接
全量 **1451 tests** 0 失败 / **GC-stress 1095** / `-D warnings` 0 / fmt+clippy 0 / **no_std 构建 0/0** / bench 不受影响
(compat 在 std 下路由到 std HashMap/Mutex 零行为变化;M2.2 rooting 仅 Err 冷路径;flip gate 在 std 路径全 active)。
**下一会话最连贯续接**:M4.2 Tier 1(逐函数 VM 回退,修「全有或全无」)或 M2.5 stackless(执行模型重写)——均为多天深度项。
