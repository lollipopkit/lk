# AOT Lowering Gaps & lkrt Sinking Plan

> 设计笔记 / 决策记录。面向维护者,记录当前 LLVM AOT 后端的覆盖形态、长尾缺口的
> 根因,以及把单态容器/字符串/display 操作下沉到 `lkrt` 的路径。规范性 ABI 约束
> 见 [`native-stdlib.md`](./native-stdlib.md)。
>
> **架构级重设计**(类型化 MIR + 结构化发射 + 单一真相 ABI + 句柄化运行时)见
> [`aot-redesign.md`](./aot-redesign.md):本文记录"当前实现的下沉进展",重设计文档记录
> "目标架构与迁移路径"。
>
> **⚠️ 历史文档(legacy 后端已整体退役)**:下文描述的 legacy text 后端、
> `lkrt/src/containers.rs`、`native_dynamic_*_helpers()` 与裸缓冲
> `@lkrt_list_{i64,f64,str}_*` helper 家族均已随 legacy 退役删除。当前唯一实现是
> 句柄化运行时:`lkrt/src/lklist.rs` 的 `lkrt_lklist_i64_*` 等导出(不透明
> `*mut Vec<T>` 句柄,无固定容量),ABI 单一真相在 `aot/abi`。本文仅作决策历史参考。

## 1. 现状:覆盖广,但"全有或全无"

后端并不小(`llvm/src/llvm/` 约 4.3 万行非测试代码),`backend.md` 列举的可
lower 形状清单非常长。问题不在覆盖量,而在**编译形态**:

- 唯一入口是 `compile_native_scalar_main_artifact`(`llvm/src/llvm/backend.rs:66`)。
  它用 `native_scalar_block_facts_*` + 一组 `unsupported_*_reason`
  (`llvm/src/llvm/diagnostics.rs`)判定**整个程序**是否落在可 lower 子集内。
- **任意一个**未覆盖的 opcode / 容器布局 / 调用形状都会让整程序 `bail!`
  (`backend.rs:78`),设计上**禁止部分回退到 VM**(不得嵌入 `.lkm`、不得 call
  back bytecode executor,见 `native-stdlib.md` 的 Binary Boundary)。

后果:真实程序里只要碰到一个未单态化的类型组合或动态调用,就整体掉不出 AOT。
`bench/README.md` 里 full-suite AOT 因**单个** "loop-after dynamic-map
`GetIndex`" 形状而整体 skip,就是这个形态的直接体现。

**结论:不是初级缺失,而是"覆盖广但很脆"。下一步的杠杆在收窄长尾的边际成本,
而不是再补几个 opcode。**

## 2. 长尾缺口的两类根因

### 2.1 结构性硬限制(来自 `diagnostics.rs` 的拒绝原因)

- 入口 `main` 必须**无参、无捕获**(`entry function has N parameters/captures`
  直接拒,`diagnostics.rs:22-25`)。
- 动态 `Call` 需要**静态已知**的 Function/Closure 目标 + 标量参数,否则报
  "native lowering needs a statically known Function/Closure target"
  (`diagnostics.rs:149`)。
- 运行时 global(`runtime globals are not native-lowerable yet`)与非白名单
  runtime return 会被拒。

这类限制是"能力边界",扩展它们需要真正的新 lowering 能力(闭包 ABI、间接调用
ABI、可变全局布局),属于大工程,应单独立项。

### 2.2 组合爆炸(本笔记的主攻点)

动态容器目前是**逐布局手写 IR**:`List<i64|f64|bool|ptr>`、`Map<str,{i64,f64,
bool,str}>`、`Map<i64,{...}>` 等,每种元素类型 × 每种方法(`push/slice/insert/
remove_at/contains/index_of/reverse/pop/set/sort/...`)在 `llvm/src/llvm/
dynamic_containers/` 里各写一份(`f64_lists.rs` 729 行、`i64_maps.rs` 747 行)。

"支持一种新容器组合"因此 = 在 Rust 里手写更多 IR。这是 N(布局) × M(方法) 的
组合爆炸,也是"每加一个 shape 都很贵 / 感觉缺很多"的真正来源。

## 3. 与既有策略的落差

`native-stdlib.md` 已经把方向写死:

> LLVM lowering must not reimplement full stdlib method bodies with ad hoc string
> matches. It may call monomorphized LK stdlib functions or typed `lkrt`
> intrinsics.

且 ABI Rules 已列 `typed list/map handles` 与 `monomorphized container layouts`
为目标 ABI。**也就是说,容器/字符串操作下沉到 `lkrt` 是既定策略,只是尚未落实
到动态容器路径——`dynamic_containers/` 的逐 shape 手写 IR 恰恰是该策略要消除的
"ad hoc reimplement"。** 本笔记不是提新方案,而是把这条已声明的策略推进到容器/
字符串/display。

## 4. 下沉方案

把**单态容器操作、字符串/模板构造、display 格式化**从"llvm crate 内联生成 IR"
迁移为"调用 `lkrt` 的 typed ABI helper":

- 容器:`lkrt_list_i64_push(handle, v)`、`lkrt_map_stri64_get(handle, ptr, len,
  *present)` 之类,按 `native-stdlib.md` 已约定的 typed list/map handle +
  monomorphized layout 表达。
- llvm 侧只负责:① 在 native intrinsic registry(`llvm/src/llvm/intrinsics.rs`,
  记录 `Pure`/`ReadsHost`/`WritesHost`)声明签名;② 生成调用。**不再逐 shape 造
  IR。**

收益:

- 每加一种布局从"几百行 IR"降到"一个 runtime 函数 + 一处调用生成",线性成本取代
  组合爆炸。
- **语义约定集中化**。以下约定目前散落在 IR 生成侧,极易与 VM 语义漂移,应集中到
  `lkrt` 的类型约定里,让 VM/AOT 单一真相:
  - map-get 的 **present-bit**(缺失键返回 `nil` 而非零值);
  - 字符串所有权 / `strdup` 拷贝(loop-local 模板缓冲不得 alias 后续迭代);
  - **divisor-zero 守卫**(与 VM 边界一致,不裸依赖 LLVM `sdiv`/`fdiv`/`frem`);
  - `nil` 返回静默、user-facing 显示拼写与 VM 路径一致。

## 5. 不可逾越的边界

1. **`lkrt` 绝不能反向依赖 `lk-core` / `lk-stdlib`**(否则 AOT 意义消失)。下沉的
   是"类型化数据操作 + host 原语",不是解释器。`lk-llvm` 是编译期 crate,可以依赖
   两者;`lkrt` 是链接期静态库,不行。见 `native-stdlib.md` §Implementation Shape。
2. **ABI 版本化**。已有 `lkrt_abi_version()` 是起点。一旦开始下沉 present-bit /
   optional / 字符串所有权这类表示,需要明确的 ABI 稳定策略;不兼容 ABI 应视为
   链接/配置错误,**不得**成为回退到 VM 的理由。
3. **不引入静默的 generic runtime-value ABI**。不可 lower 的形状必须报出具体
   unsupported reason,而非退回 `RuntimeVal`/`HeapStore`。

## 6. 建议落地步骤

1. **Pilot 一个布局**:选一个已在 `dynamic_containers/` 手写、方法较全的布局(如
   `DynamicList<i64>`),把其方法族迁到 `lkrt` typed helper + intrinsic 声明,验证
   IR 体积、性能(`bench/run_workload_bench.sh` 保持 checksum-clean)、以及现有
   AOT 测试(`llvm/src/llvm/tests/`)不回归。 **✅ 已完成,见 §7。**
2. **抽出共享 ABI 约定**:把 present-bit、字符串所有权、divisor-zero 守卫收敛为
   `lkrt` 的少数约定函数/类型,消除 IR 侧重复实现。**✅ 进行中**:map lookup/delete
   的 present-bit 已集中到 `lkrt` 的 `*_lookup`(写 `out` + 返回 found)/`*_delete`
   (`out_value`+`out_present`)约定;字符串所有权统一为 `dup_cstr`/`strdup`(leaked,
   匹配短命 AOT 二进制);`i64` 显示位数集中到 `lkrt_i64_decimal_len`。
3. **按收益推广**:优先迁移 bench 里导致 full-suite AOT skip 的 dynamic-map
   `GetIndex` 相关形状,让更多真实程序完整掉出 AOT。**✅ 已完成**:`DynamicMap<str,V>`
   的 set/get/has/delete(V ∈ {i64,f64,str-ptr})与运行时 `split_key` 全部下沉,见 §8。
4. 结构性硬限制(§2.1,闭包/间接调用/可变全局)单独立项,不与本次下沉混做。

## 7. 已落地:`DynamicList` 三布局全部下沉(batch 1-3)

统一做法(每布局):`lkrt/src/containers.rs` 用 Rust `extern "C"` 纯函数实现方法族
(内部 helper 泛型化复用);`lkrt/src/lib.rs` 导出;`intrinsics.rs` 加 registry 条目
(module=`list.{i64,f64,str}`,`Pure`,`declare` 自动生成);对应
`dynamic_containers/*.rs` 的 `native_dynamic_*_helpers()` 返回 `""`、`emit_*` 改调
`@lkrt_list_{i64,f64,str}_*`;`tests/{modules,basic,strings}.rs` 断言符号名同步。

- **batch 1 `list.i64`**(10 方法):contains/index_of/reverse/sort/pop/slice_range/
  push/insert/remove_at/set。
- **batch 2 `list.f64`**(14 方法):额外 slice/take/concat/unique;`f64` 比较用
  `PartialEq`/手写 selection sort 匹配 `fcmp` 的 NaN 语义。
- **batch 3 `list.str`**(14 方法):元素为 `*const c_char`;结构操作只移动指针,
  push/insert/set 用 `dup_cstr`(leaked,匹配 `strdup` 不释放);空/越界返回稳定空
  C 串(替代 `@lk_empty_text`);比较用 `CStr`(`strcmp` 字节序)。注意 str 的
  slice/take/concat 原本在 `dynamic_containers.rs` 顶层混合池 + `subfunction.rs`,
  已一并下沉。
- **验证**:`cargo test -p lkrt` 9 tests(含各布局 + 别名 + strdup + empty);
  `cargo test -p lk-llvm` 251 passed;`cargo test --workspace --all-features` 全绿;
  i64/f64 端到端 `lk compile` native 输出与 VM 逐项一致。str list 的 receiver 形状
  受既有 lowering 长尾限制无法 CLI 端到端触发(与下沉无关),其链接机制与 i64/f64
  相同且经 IR 断言验证。
- **未下沉**:`@lk_concat_i64_list`(bool-list concat 借用 i64-slot ABI)仍是手写 IR。

### 关键教训:in-place 别名(后续 batch 必读)

LLVM 会把 `xs.slice(..)`、`xs.sort()` 等**就地**操作 lower 成 `src == dst` 指向
**同一 buffer**。旧手写 IR 用逐元素前向 load/store 天然容忍别名;移植到 Rust 时:

- **禁止**同时持有别名的 `&[T]` 与 `&mut [T]`(即使逻辑正确也是 UB);
- **禁止**对可能重叠的范围用 `copy_from_slice` / `ptr::copy_nonoverlapping`
  (违反 `copy_nonoverlapping` 的不重叠前置条件 —— 这是 **UB**,不是 panic:
  不会有任何报错,只会静默写出错误数据;重叠安全的替代是 `ptr::copy`(memmove)
  与 `slice::copy_within`);
- 范围移动一律用裸指针 + `ptr::copy`(memmove),且只向**更低或相等**的目标索引
  前向写(`slice`/`push`/`remove_at`/`set` 满足;`insert` 先 memmove 右移尾段);
- `sort` 先 `ptr::copy` 物化到 `dst`,再取**单一** `&mut` 排序。

`containers.rs` 的 `in_place_aliasing` 单测专门锁定 `src == dst` 行为。

## 8. 已落地:`DynamicMap` 两布局 + display 位数下沉(batch 4-5)

沿用 §7 模板(helper 内部泛型化 + intrinsic 注册 + emit 改调 `@lkrt_*`)。map 布局是
**并行数组**(keys/values 各一段 `[4096 x T]`),helper 只操作裸 `ptr + len`,`present-bit`
由 `lookup` 返回 + emit 侧写入 `%r.present.slot`(保留 `nil`≠零值)。

- **batch 4 `map.i64`**(6 helper):`lkrt_map_i64_{int,f64,ptr}_{lookup,set}`,泛型
  `map_lookup`/`map_set`(`K=i64`);ptr 值的 `strdup` 在 emit 侧,helper 只移动指针。
  i64-map 的 `has`/`delete`/`iter`/`values`/`keys` 仍为内联 IR(非 helper 池)。
- **batch 5 `map.str`**(复合短字符串 key = `prefix` 串 + 整数后缀,如 `"k12"`→
  `prefix="k",number=12`):
  - `lkrt_map_str_split_key`:扫描尾部 ASCII 数字;"raw" key(空/全数字/无尾数字)
    保留原指针 + `number=0`,否则 leak 一份截断 `prefix` 拷贝。替代 `@lk_split_string_int_key`。
  - `lkrt_map_str_{int,f64,ptr}_{lookup,set}`:泛型 `str_map_lookup`/`str_map_set`,
    key 比较 = `strcmp(prefix)==0 && number==`。替代 `@lk_{lookup,set}_string_{int,f64}_map`
    并**新增** ptr 值布局(原 `string_maps.rs` 内联)。
  - `lkrt_map_str_contains`(`has`)、`lkrt_map_str_{int,f64,ptr}_delete`(压缩式删除:
    非匹配项拷进目标数组,容忍 `src==dst`;`out_value`+`out_present` 报告被删值,返回
    目标长度)。替代 `string_maps.rs` 里 `has`/`delete` 的内联循环。
  - str-map 的 `iter`/`values`/`keys` 仍为内联 IR(纯 index GEP/snprintf,非 ad-hoc
    方法体)。
- **decimal_len**:`lkrt_i64_decimal_len`(module `fmt`)替代 `@lk_i64_decimal_len`,
  给动态模板/文本缓冲算 `i64` 十进制位数。单测对 `i64::MIN/MAX` 与 `v.to_string().len()`
  逐项核对。
- **验证**:`cargo test -p lkrt` 21 tests(新增 split_key/str-map int+ptr set/lookup/
  contains/delete/decimal_len);`cargo test -p lk-llvm` 251 passed;
  `cargo test --workspace --all-features` 1609 passed;`nm` 确认全部 `lkrt_map_str_*` +
  `lkrt_i64_decimal_len` 链接进 native 二进制;string-int-map set/get 端到端
  `lk compile` 输出与 VM 一致。
- **仍是手写 IR / 待下沉**:`@lk_concat_i64_list`(bool-list concat);两种 map 的
  `iter`/`values`/`keys`(索引拷贝/snprintf 重建 key,收益低);`map.str` 的 ptr map
  set/get/has/delete 因 `map` 模块非 CLI 可达且常量折叠,未做非折叠 native RUN(靠
  llvm 结构断言 + lkrt 单测 + 与 i64/f64 同款调用约定覆盖)。

### batch 4-5 关键点:测试断言不得依赖 helper 池样板

旧 `native_dynamic_container_helpers()` **无条件**把 6 个手写 IR helper 注入**每个**模块,
导致个别测试断言(`select i1` 来自 `@lk_i64_decimal_len`、`call i32 @strcmp` 来自 map
helper)其实在测样板而非被测程序。下沉删除这些定义后,应改断被测 lowering 自身产物
(如 bool 常量返回折叠成静态串经 `@lk_str_fmt` 打印;模板比较分解为 `icmp eq i64`)。

## 9. 已落地:impl 方法里的 `self` 带类型出身(2026-07-30)

**`self` 在 `impl T { … }` 的方法里就是一个 `T`。**

devirtualization 此前只认一个来源的类型出身:`NewObject` 写进
`ssa.struct_types` 的那份(`lower_call.rs`)。而 impl 方法里的接收者是**参数**,
它一辈子见不到 `NewObject`,于是 `lower_trait_method_k` 两条路都不匹配 ——
静态那条要 `struct_types` 有记录,动态那条要接收者是 `Dyn`。落到通用分发,
报 `an operand at pc 1 is a str where a i64 is required`。

被挡住的是**「一个方法建立在这个类型的其它方法之上」**这个形状,而方法多半
就是这么写的;trait 的默认方法体更是**只能**这么写(它不能提字段名,不然对
别的实现者就不成立)。所以在这条修好之前,一个有意义的 trait 默认实现示例
根本进不了 `examples/`(coverage 门禁要求每个 example 全原生降低)。

做法:`function.rs` 在给参数建 SSA 值时,若参数 0 的类型是 `MapStrDyn` 且这个
函数是某个 impl 块的方法(`TraitEnv::impl_owner`,`impls` 表的逆),就把该类型
写进 `struct_types`。

**一个函数登记在两个类型名下时不给答案。** 编译器可以共享函数体,而一份被复制
进两个 impl 的默认方法恰好就是两段一模一样的体。这时随便答一个会把
`self.other()` devirt 到**错的** impl —— 那是错答案,不是拒绝。`impl_owner`
因此在发现歧义时返回 `None`。

### 同日续:**没人调用的 impl 方法**也不能用 `I64` 兜底

上面那条修完之后 `t4` 形状还是拒,报的是 `in `B::base`: an operand at pc 1 is a
str where a i64 is required` —— 而 `B::base` 在那个程序里**从来没被调用过**。

两件事凑在一起:每个 impl 方法都是降低的 root(trait 的每条臂都必须存在),
而没有调用点的参数类型走 `param_ty` 的默认值 `I64`。于是一个没人调用的方法
按"参数是整数"降低,体里一读字段就炸,整个模块跟着掉回 Tier 0 —— 起因是一个
谁也没调的方法。

`param_ty` 现在对 impl 方法的参数 0 给 `MapStrDyn`:`self` 是结构体实例,
调用点说什么都不改变这件事,**包括一个调用点都没有的时候**。

同一个洞还有另一半:`self` 之外的参数。`fn add(self, x: String)` 没人调用时
`x` 同样默认成 `I64` —— 而声明里就写着 `String`,`FunctionData` 却没有把参数
类型带下来。与其把声明一路传下去,更正确的修法是**根本不给它降低**:impl
方法之所以是 root,是因为 trait 分发经注册表到达它们、调用扫描看不见;而一个
**没有任何调用点提到其名字**的方法,分发也到不了。`CallMethodK` 是唯一的方法
调用 opcode 且方法名取自常量池,所以"哪些方法名会被调用"是可以精确算出来的
(`called_method_names`)。

**例外必须显式列出。** `show` 由显示点(`"${value}"`)到达,没有任何
`CallMethodK` 提它 —— 把它从 root 里剪掉会留下悬空 callee,模块直接 MIR
验证失败(`examples/syntax/macros.lk` 当场变红)。所以有一份
`IMPLICIT_METHOD_HOOKS`,`lower_method::apply_show` 的查表和 root 计算**用的
是同一个常量**,而不是两处各写一个 `"show"`。以后再加隐式钩子,只有一个地方
要改。

这条也是上面那条能生效的前提:诊断此前指不到人。impl 方法从来没有
`debug_name`,所有关于它们的 AOT 报错都是光秃秃的 `an operand at pc 1 …`;
现在它们叫 `Type::method`(`compile_impl_method_function_indexed`)。**是这个
名字直接指出了真凶**——在那之前我一直在错的函数上找。

## 10. 已落地:bundle 把依赖的 `impl` 也搬过来(2026-07-30)

编译时 bundle(`use "../general/fib"`)把依赖的**函数**、globals、常量都搬进了
合并 artifact,唯独没搬 `type_info.impls`。于是合并出来的 artifact 手里有一个
导入 `impl` 的**函数体**,却没有"它们实现了什么"这条记录 —— AOT 的 trait 环境
(`trait_env_prescan`,读的就是 `type_info.impls`)看不见它们,所以**每一个**
跨模块方法调用都掉出原生子集,而同样的代码写在定义方模块里降低得好好的。

现在 `impl` 声明跟着搬,方法索引用同一张 remap 重写。一个类型在两个 bundle
模块里同名实现会**报错而不是择一** —— VM 靠 `TypeScope` 把它们分得开,bundle
分不开,按这个 bundler 其余地方的规矩:说出原因,不要替人选。

配合上一条(构造函数返回值带类型出身),`types.Pt { x: 3, y: 4 }.norm()` 现在
全原生。`examples/syntax/use_forms.lk` 覆盖了它,coverage 57 → 58。

## 11. 已落地:类型名跨函数边界(2026-07-30)

**根子是"类型名只从 `NewObject` 那一个地方流出来"。** 那条唯一的出口在函数
边界上断掉,于是 `make(3, 4).norm()` 的接收者无类型可用,方法调用掉出
devirtualize 路径 —— 同一个模块里也一样。这是这一族的第四个入口(前三个:
`self` 参数、没人调用的方法、构造函数返回值)。

`SigInfer::ret_structs` 记每个函数**返回的结构体名**,在返回点由
`ssa.struct_types` 读出、跨返回点求交(两个不同结构体、或有一个返回不是结构体
→ 不给答案,因为只有时候对的名字会 devirt 到错的 impl),再由两个调用发射器
(`lower_user_call` / `emit_call_with_args`)在结果上播下。它进了定点的
snapshot,所以先于被调方降低的调用方会在下一趟拿到。

**顺带删掉一个特例**:上一条我给构造函数的返回值按 `$new` 名字播过出身;通用
规则把它包住了(构造函数的体就是 `return S { … }`,`NewObject` 出身直接得到
`ret_structs = "S"`)。一个机制取代一个命名约定。

基准 geomean 1.006x,无回退。

### 仍然开着的一个,**不是**跨模块特有的

**结构体在模板串里显示**:`"${p}"` 不降低,本地同样。`ToString` 走
`apply_display_show`,只有注册了 `show` 的类型有出路;没有 `show` 的结构体在 VM
里有默认显示(`Pt{x:3,y:4}`,按声明序),native 没有对应物。

**试过并撤回的捷径(2026-07-30,别再走同一条)**:在降低点把渲染**内联展开**。
降低点确实什么都知道 —— 类型名在 `ssa.struct_types`,字段序在
`type_info.structs` —— 所以渲染可以写成"常量片段 + 每字段一次
`dyn.display_quoted`"的拼接链,不需要任何运行时表。顶层结构体逐字正确,连
`P{name:"a, b",n:-3,ok:true,f:1.5}` 的引号和负号都对。

**但嵌套结构体给错答案**:字段里的结构体在运行时只是个 `str→Dyn` map,
`dyn.display_quoted` 把它渲染成 `{"ok":true,…}`(hash 序),而 VM 给
`P{name:…}`。而"这个字段会不会装结构体"在降低点**判定不了** —— 字段的类型不在
手上,值是 `Dyn`。也就是说这条捷径是一个我检测不出来的错答案生成器,所以撤回,
恢复成响亮拒绝。

**嵌套不可回避,所以运行时表也不可回避。** 已按这条落地(同日):

- **每个声明的 struct 都拿到 type id**,不再只有带 impl 的那些 —— id 也是
  `display` 找类型名和字段序的钥匙,没有方法的结构体照样要打印。
- **entry 前奏把类型描述交给运行时**:每个类型一次 `obj_ty.begin(tid, name)`,
  之后每个字段一次 `obj_ty.field(tid, name)`。用调用序列而不是静态数据表,
  是因为这样 codegen 不需要任何新东西 —— 用的都是 ABI 已有的 `I64`/`StrPtr`。
- **lkrt 的 display 认这个标记**:`DYN_MAP` 且有标记且类型有描述 → 渲染
  `Name{f:v,…}`,值走同一个 display,于是**嵌套自然递归**。没有标记的 map 仍然
  按 map 渲染(它的序是布局的序,故意不在原生子集里)。
- **`to_display_str` 的 `MapStrDyn` 从"拒绝"改成走 `dyn.display*`**。

逐字核对过的形状:字符串字段带引号(`P{name:"a, b",…}`)、负数、Float、空
结构体 `E{}`、嵌套结构体、结构体里装列表、`println(p)` 与 `"${p}"` 两条路径。
差分语料里那条用例**专门钉住嵌套**,因为那正是上面那条捷径栽的地方。基准
geomean 1.001x。

### 同日续:模板插值里的容器,是照着一条退休的裁决在拒绝

结构体列表 `"${[P{v:1}]}"` 追下去发现:`ToString`/`ConcatString`/`ConcatN` 都传
`containers: false`,而那是 `docs/semantics.md` 里一条**已经过时**的裁决 ——
"`ToString`/模板插值是标量 only,容器是响亮失败"。VM 早就不是那样了
(`"${xs}"` 就是 `[1,2,3]`,`"m=${m}"` 就是 `m={"k":1}`),只有这一侧还在照办。

后果是:任何模板里带 list 或结构体列表的程序都掉回 VM。**答案一致,只是慢**,
所以差分门禁不会红 —— 是手写探针撞上的。四处 `false` 改成 `true`,文档那条改
正,差分语料补上。基准 geomean 1.013x。

留在原生子集外的只剩 map(hash 迭代序不可移植)和 Set,各有自己的理由。

## 12. 形状扫描:门禁看不见的回落(2026-07-30)

上一条("照着退休的裁决拒绝")说明了一类门禁**结构上**看不见的缺口:两个后端
答案一致,只有 native 慢。差分测试锁"双方一致",coverage 锁"每个 example 全
降低",两者都不问"这个形状本来该不该降低"。

所以手写了两轮扫描:**参数类型 × `try`**(9 种)与**常见语言形状**(20 个)。
前者一发命中最后一个缺口(`Float` 参数),后者找出五处,其中已修:

- **`[1,2,3].index_of(2)`** —— VM 在每种序列上都有 `index_of`,降低只在 `Str`
  上有。补 `list_h/i64_index_of`(未命中给 nil,`Int?` 的 boxed 形式)。

**扫描里踩的一次**:同一批里 `[1,2,3].join("-")` 也拒绝,我以为是同类缺口,
补了 `i64_join` 让它降低 —— 然后**逐字比对 VM 才发现 VM 自己是拒绝的**
("ListJoin list must contain only strings",与 Python 一致)。原注释和守卫都是
对的,我的改动会让 native 答出 VM 拒绝的东西。已撤回,并在方法臂旁写明为什么
`join` 不在 `index_of` 旁边。

**教训**:扫描给出的是"native 拒绝"的清单,不是"缺口"的清单。每一条都要先问
VM 怎么答 —— 对照的基准永远是 VM,不是"看起来该能行"。

### 仍开着的三处(见任务 #56/#57 与下)

- **`StoreCellVal`**:闭包写它捕获的变量(`let add = |v| { acc = acc + v; };`)。
  可变捕获编译成 cell,cell 的写入还没有原生降低。
- **`chan.new` + `send`/`recv`**:`register r2 is read at pc 2 before any
  definition`。
- **`"hi".bytes()`**:`Call` 不可降低。

## 13. 闭包改自己捕获的变量(2026-07-30)

`let add = |v| { acc = acc + v; };` —— 一个累加闭包,也就是闭包这件事本身最
常见的用法 —— 让**整个程序**掉回 VM。

捕获走的是隐藏尾参:调用点把 cell 的**当前内容**取出来传进去。对只读的捕获
这是对的,对写的捕获则是"写没有落点",于是 `StoreCellVal` 那条臂上写着"a
by-value capture parameter has no write-back path"直接拒绝。

要的载体其实早就有:`Ty::Cell`(`rt.cell_new/get/set`),`try` 体对外层局部量
赋值就是靠它跨边界的。缺的只是**判据** —— 哪个捕获需要它。

判据没有去字节码上猜寄存器出身(`LoadCapture` 落到哪个寄存器、`Move` 传到
哪),而是用降低本身已有的收敛回路,和 `dyn_rets`/`try_body_params` 同一个
办法:体降低到那条赋值,发现捕获是按值来的,就把 `(函数, 捕获下标)` 记进
`SigInfer::cell_captures` 并请求重试;下一趟调用方看到这条事实,seed 一个
`rt.cell_new`、按 `Ty::Cell` 传、调用后 `cell_get` 读回父函数的槽。事实来自
"体真的降低到了那里",不来自猜。

一个坑:`param_obs` 跨趟只增不清,所以第一趟按值观测到的 `I64` 会和 `Cell`
join 成 `Dyn`,调用点连 cell 指针都塞不进去。因此记事实的同时要把那个参数槽
**pin** 成 `Ty::Cell`(`SigInfer::require_cell_capture`)。

只读捕获仍按值传 —— 一个只读的捕获被拖进 cell 是白付一次装箱。

**还没通的**:内层 lambda 写外层 lambda 的捕获(捕获链要一级级传下去),见
todos #87。

### 13.1 嵌套闭包(2026-07-30)

内层 lambda 写外层 lambda 的捕获:

```lk
let total = 0;
let outer = |v| {
  let inner = |w| { total = total + w; };   // 写的是 outer 捕获的东西
  inner(v);
};
```

`MakeClosure` 只认 `GlobalRef::Cell(cid)`(父函数自己有的 cell)。这里父函数是把
它当**捕获参数**拿着的,没有 cid 可指,`ssa.read` 于是在那个寄存器上找不到值,
报 "register r2 is read at pc 2 before any definition" —— 整个程序回落。

加了 `ClosureCapture::CellParam(k)`:父的第 k 个捕获再传下去。父的捕获已经是
`Ty::Cell` 时**指针直接穿过去** —— 父子共用一个 cell,正是 VM 的语义;还不是
cell 时,子的需求**往上传**:调用点把它记到父身上并请求重试,于是
`cell_captures` 在整条链上收敛。三层也是这么通的。

`spawn`、`try` 区域、以及被擦除的闭包环境这三处一开始不解析 `CellParam`,标了
TODO;2026-08-17 补齐(见 §26)。

## 14. `base64` / `hex` / `url`(2026-07-30)

`String -> String` 的那半边有了原生实现:`base64.encode`、`hex.encode`、
`url.encode_component`、`url.decode_component`。lkrt 用**与 stdlib 模块同一个
crate**(`base64`、`hex`),所以文本逐字节相同 —— 和 `datetime` 用 chrono、
`json` 用 serde_json 是同一个理由。

`url.encode_component` 是先修了才镜像的:它和 `decode_component` 不往返(编码是 form
编码、解码只撤 `%XX`),见 `docs/semantics.md`。所以 lkrt 里那份是手写的百分号编码,
和 stdlib 里手写的那份同一套未保留集。

`base64.decode` / `hex.decode` 给 `Bytes`,原生没有那个承载类型(见 §13 之前的
Bytes 一节 / todos),继续回落。

## 15. `Bytes` 是一个原生值(2026-07-30)

`Bytes` 以前在原生一侧**没有承载类型**,于是 `"hi".bytes()`、`bytes` 模块的每个
成员、`base64.decode` / `hex.decode` 出现任意一个,整个程序回落。

`Ty::Bytes` 是个不透明指针句柄,和 `List`/`Map`/`Set` 同形 —— 十个接点(MIR 的 `Ty`
与渲染、codegen 的类型映射、lower 的两张"这是句柄"表、显示、相等、`len` 快路、
`GetIndex`、方法表、模块表)。它必须是**独立的类型**而不是裸句柄整数,因为显示和相等
都要知道它是字节:`println(b)` 是 `Bytes([104,105])` 而不是一个指针,`==` 比内容。

**lkrt 里曾经有两个 `Bytes`。** 另一个是 `tcp`/`fs` 用的**一次性** host 句柄
(`HandleKind::Bytes`,用 `take_bytes` 读,读走就没了)。对"读一次 socket 然后解一次
码"是对的,对**值**是错的:`bytes.len(b)` 之后再 `bytes.to_string_utf8(b)`,第二次就
找不到句柄了。所以 `tcp.read` / `fs.read` 现在都给 arena 句柄,一次性那套(资源变体、
两个访问器、`bytes.to_string_utf8`/`bytes.free` 两条 ABI 项)整套删掉 —— 让它们分开的
理由消失了,留着就是个陷阱。`fs.read` 因此也第一次拿到了降低行:它一直有 ABI 项而没有
行,因为没有类型可给。

`bytes.from_list` / `to_list` 也接上了(2026-07-30):`List<Int>` 在 lkrt 里就是
`Vec<i64>` 的 arena 句柄,和 `Bytes` 同形,所以两个方向都只是一次转换。不是字节的值
**raise**,和 stdlib 模块一样 —— 一个不是字节的"字节"是个错误,不是要静默截断的东西。
`bytes` 模块的十个成员现在全部原生。

## 16. 闭包调另一个闭包(2026-07-30)

```lk
let f = |x| x + 1;
let g = |x| f(x) * 2;      // 以前:整个程序回落
```

组合两个 lambda 是"有 lambda"这件事本身最主要的用途。`f` 被捕获,所以编译器把它放进
一个 **cell**,而进那个 cell 的是一个**降低期的引用**,不是值 —— `StoreCellVal` 去读
SSA 值,读不到。

两半:

1. `Ssa::cell_refs` —— 整个内容就是一个可调用引用的 cell。存进去时记下引用、不写槽;
   读出来时把引用还回去。一个 cell 只能有一个引用,同时又被赋别的东西就拒绝(回落),
   而不是猜后面那次读想要哪个意思。
2. `SigInfer::ref_captures` —— 被调方那侧。引用在这里没有运行时表示,所以那个捕获仍然
   占着 ABI 槽位(一个死的 `0`),**意思**走这张表。由调用方发现并请求重试,和
   `cell_captures` 同一个回路。

**只收无捕获的可调用**(`Lambda` / `UserFn`)。`Closure(fidx, caps)` 里的 `ValueId` 属于
建造它的那个函数,在读 cell 的人那里什么也不指 —— 记下来就等于把不存在的操作数递给
读者。它拒绝,而且是**故意**拒绝,不是碰巧。

### 16.1 捕获环境全是静态引用时,整个擦掉

`[1,2,3].map(|x| f(x))` 还差一步:那个 lambda 实参**有**一个捕获(被捕获的 `f`),而
列表 HOF 的类型化快路(`i64_map_fn` 等)只认无捕获的可调用 —— 它调回调时只递一个元素,
再没有别的。

但是:一个捕获环境**全是静态引用**的闭包,运行时什么都不需要传 —— 它就等价于一个裸
函数引用。所以 `SigInfer::captures_all_static` 为真时,`lower_function` 干脆不声明那些
参数,`MakeClosure` 直接给 `GlobalRef::Lambda`,于是各处(包括 HOF 快路)都把它当普通
函数引用看。

**全有或全无**,这是有意的:混合环境需要在某一个下标上留个洞,而每个调用点都得同意洞
在哪。死槽位那种形式本来就把混合情形处理对了,只是白费一个寄存器。

还没通的一种:`let n = 2; let f = |x| x * n; let g = |x| f(x) + 1;` —— 被调的那个自己
带捕获,正是上面故意拒绝的那条。

## 17. `try` 体里的 `return`(2026-07-30 调查,未实现)

```lk
fn f(n: Int) -> Int {
  try { return n * 2; } catch e { return -1; }        // 整个程序回落
}
fn g(n: Int) -> Int {
  let v = try { n * 2 } catch e { -1 };  return v;    // 原生
}
```

同一个意思两种写法,一种慢三倍。拒绝的理由在 `try_region.rs` 里写着:body 被外联成
一个函数,里面的 `return` 会变成"从 body 返回"而不是"从外层函数返回",而"然后返回"
这个协议还没有。

**做法(2026-07-30 已实现)**:body 本来就有两条通道 —— 返回值走 `LkDyn`,raise 走
trampoline 的 outcome。加的是第三个信号,用的是现成的**输出 cell** 机制
(`try_body_cells`:父建 cell、当额外实参传进去、调用后读回):

1. 多一对输出 cell:一个"是否返回了"的标志,一个返回值。只给**体里真的有 return**
   的 region 加(`SigInfer::try_body_returns`),别的 region 传的东西一个不变。
2. body 侧:`Return v` 降低成 `cell_set(flag, 1); cell_set(value, box v)`,然后正常
   返回,让 trampoline 报"没有 raise"。
3. 父侧:ok 边不再直接去 fallthrough,而是去一个**检查块**:读标志,真就去一个
   **返回块**,假就转发到原来的 fallthrough。

第 3 步本来的顾虑是"新块会成为 fallthrough / handler 的新前驱,phi 要重排"。**不需要**:
检查块转发时用的是 `args_to(区域块, fallthrough)` —— 也就是区域块本来要传的那一份实参。
目标的 phi 操作数仍然记在区域块名下,而这里正是读它的地方。所以插入是局部的。

**"体里每条路都 return"也通了。** 我一开始把它读成一条独立的拒绝:那种 region 在字节码
里没有跳过 handler 的 `Jmp`,看着像"没有 ok 边"。它不是 —— ok 边照样存在,只是**恒定**
走返回那一支。当时看到的崩溃来自两个自己的 bug(一份 python 补丁在第二处断言失败时整份
没写盘;`Return` 那条臂用 `continue` 跳过了循环末尾存指令的那行),不是 CFG 的性质。
拒绝去掉之后六种"每条路都 return"的形状全部原生。

教训是具体的:**一个自造的 bug 会长得很像一条语言性质**。当"这里需要一条新规矩"这个
念头是从崩溃里冒出来的,先把崩溃归零再判断。

这条**曾被当作** `defer` 在 raise 路径上跑的前置条件。它做完之后,那个改动也做了、也
能跑,然后**因为另一个代价被撤回**:把整个函数体包进 `try`,会让体里赋值的每个寄存器都
变成区域的**输出 cell**,而寄存器复用意味着那是其中大多数;cell 往返对标量有定义,对
**容器句柄故意没有**(`unbox_from_dyn` —— 读回成错的类型化句柄是错答案,不是拒绝)。于是
`examples/syntax/defer.lk` 当场掉出原生。详见 `core/src/stmt/defer.rs`。

所以真正的前置条件是**容器句柄的 cell 往返**,不是这一条。

其中**一半随即做了**(2026-07-30):本来就装箱的容器按指针往返 —— `dyn.from_list` /
`from_map` 只是给句柄打个 tag,`as_list` / `as_map` 查 tag 后把同一个指针还回来,身份和
它捎带的修改都在。于是 `List<Any>` 和 `Map<String, Any>` 现在能跨区域。

**类型化容器随后也通了**(同日):它们要的是**保身份的 cell** —— 句柄原样停在
`DYN_RAW` 这个 tag 下,不装箱,所以拿回来的是同一个指针。两端(`cell_get` /
`cell_get_raw`)都查 tag,所以"把裸的当装箱的读"是**响亮失败**,而不是一个 `Vec<i64>`
被当成 `Vec<LkDyn>` 走 —— 这条是这个设计敢做的前提。判据只有一处
(`function::cell_is_raw`),调用方(seed 和读回)和体(每次赋值时写)读的是同一个函数,
两边不可能各说各话。

覆盖到:类型化 list、类型化 map、`Set`、`Bytes`。

### 17.1 第二个区域(同日,一条独立的旧账)

上面记下的"容器区域 + 第二个区域仍然回落"查清了,而且**先于**容器跨区域就有:两个 dyn
容器区域一样回落。它跟容器无关,是**归因**的问题。

体在自己的帧里写了寄存器又没捎回来,父帧那一份就被 poison;之后谁读到它,失败信息
(`UndefinedOperand`)正是定点用来发现"哪些寄存器需要 cell"的那条线索 —— 这一步是对的,
它把"区域之后还有人读吗"这个活性问题变成了向 SSA 提问,而不是给每个 opcode 写一张读操作数表。

坏在这条线索**只说了寄存器号,没说是谁毒的**。消费端于是把 cell 派给函数里**每一个**
区域。而寄存器号是会被复用的:第一个体拿 r2 当过草稿(存 `[2]` 这个字面量),第二个变量
`b` 也正好分到 r2。第一个区域于是收到一个 r2 的 cell —— 可它在**自己的区域开始处从来没有
定义过 r2**,做种就得在 pc 0 之前读它,于是整个函数回落。也就是说:**给一个函数加第二个
`try`,会让第一个 `try` 丢掉降低**,而两个都单独写时各自都好。

修法是让 poison 记住是谁毒的(`poisoned: Vec<Vec<Option<u32>>>`,存 body id),错误带着它
走,消费端就不用猜了。没有归因的 `UndefinedOperand` 是一次普通的未定义读,cell 修不了它 ——
那条瞎猜的兜底一并删掉,覆盖率一个没掉,说明它从来没干过活。

判据仍然是"读到了才算",没有变成预测。`several_try_regions_share_a_function` 钉住五种形状
(容器区域接标量区域、三种类型三个区域、区域进循环、两个区域都 raise、函数里两个区域串起来)。

## 17.2 map 字面量与 map 显示(2026-07-30)

两个洞在同一处碰头,而且互相遮掩。

**一、`NewMap` 没有降低。** 值不全是常量的字面量 —— `{"k": a}`、`{"a": f(3)}` ——
走的是 `NewMap`,而它根本没有降低,于是"从算出来的东西拼一条记录"这种再普通不过的
程序整个掉回 VM。对应的 list 拼写 `[a, a + 1]` 一直是原生的,这正是它没被发现的原因:
两种字面量读起来一样,只有一种能编译。

修法是走**和常量 map 完全同一条路**:`lit_new` / `lit_set` 按字面量序累积装箱的键值对,
`lit_finish_<形状>` 转成类型化表示。于是这里的形状判据只需要**照抄常量那几条臂**
(并透过它们照抄 VM 的 `typed_map_from_entries`),而不是长出第二套会各自漂移的分类。

**二、显示类型化 map 照着一条退休的裁决拒绝。** 原话是"map 的顺序是底层 hash 迭代序,
两个运行时不共享"。这条**先于 `lkrt/src/vm_mirror.rs`** —— 而那个模块存在的全部意义就是
让两边共享它,`lit_protocol_matches_vm_iteration_order` 直接拿 `lk-core` 比对。更明显的是,
`MapStrDyn` 那条臂早就放行了:裁决对一个 map 类型解除,对其余的留着,于是
`println({"a": 1})` 让程序丢掉降低,`println({"a": 1, "b": "x"})` 不会。

现在字符串键的三种(`str_i64` / `str_f64` / `str_bool`)按载体自身的迭代序在 lkrt 里渲染,
**不重建** —— 顺序这个问题因此只问一次。

**三、顺带撞出来的错答案。** `println([m])` 打的是 `{"a":1}`,而 VM 打 `[{"a":1}]`。
`NewList` 的任何一条臂都装不下类型化 map,于是**什么都没物化**,目标寄存器只剩下
ArgList 那一半视图(`NewList` 同时也是方法调用的实参窗口),读到它的调用就把元素当成了
列表本身。补了装箱之外,还给 `NewList` 加了一条兜底:非空却没有任何一条臂物化出句柄,
是**回落**,不是静默拆包。覆盖率一个没掉。

**四、整数键:一条潜伏的错答案,顺手挖出来修掉。** VM 对非字符串键**不做第二阶段**
—— `typed_map_from_entries` 直接返回 `Mixed`,那就是 stage-1 那张表 —— 而
`lit_finish_i64_i64` / `i64_f64` 又 rehash 进 `FxMap<i64, _>`:哈希不是一回事
(`i64` 对 `RtKey::Int(i64)`),插入序也不是。`{1: 1.5, 2: 2.5}` VM 迭代 `2,1`,
native 迭代 `1,2`。

它当时没人看得见(整数键 map 的 display 和 `.keys()` 都不降低),所以是**潜伏**的 ——
但"潜伏"的意思是:下一个给整数键 map 降低迭代的人会拿到一个错答案,而且没有任何东西
会告诉他。所以修的是载体,不是绕开它:

- `vm_mirror::IntKey` 按 `RtKey::Int` 哈希(判别式写死成常量而不是现构一个 32 字节的
  枚举,`int_key_hashes_like_the_mirror_enum` 负责说这两个是同一个);
- `LitBuilder` 除了 stage-1 表还记一条**字面量序**。这不是冗余:字符串键有 stage 2,
  所以它的 finisher 迭代表;非字符串键**没有** stage 2,它的 finisher 必须重放字面量的
  插入序列 —— 在那里迭代表就等于多跑了一个 VM 没跑过的阶段。
- `int_lit_protocol_matches_vm_iteration_order` 拿 `lk-core` 的
  `typed_map_iteration_int_keys` 逐条比对,五组键(含 64 个键、逼出多次扩容)。

于是整数键的 display 也进了子集,字面量和逐个赋值两条路都钉在差分里。

## 17.3 装箱不能改变表示(2026-07-30)

`DYN_RAW` 的注释早就把这条规矩写下来了 —— 类型化容器的装箱是逐元素转换,往返一趟
拿回来的是另一个容器 —— 但**类型化 map 的装箱一直在违反它**:`str_i64_to_dyn`
把 map 重建成 `str -> Dyn` 的,按迭代序往一张新表里插。

新表由**不同的插入序列**填成,布局就不一样。只要历史里有过删除,两张表的迭代序
就分岔,于是 `println([m])` 打出的条目顺序是 VM 永远不会给的 —— **错答案,不是回落**。
它有两条到达路径:结构体字段持有 map(长期存在),和 map 进 list / map
(补可装箱元素时新加的)。没有删除时两张表的布局重合,所以它藏得住。

改法是照着那条规矩来:类型化 map **原地装箱**,标签说明它是哪个载体(五个载体,
五个标签)。重建只剩一处 —— `typed_map_keyed`,它唯一的消费者是相等,而相等与顺序
无关,这一点写在那个函数头上。

顺带,相等因此要能**跨表示**比较:`{"a": 1} == {"a": 1.0}` 是同一张 map 的两种
拼写,标签不同只是存储细节。所以 `dyn_eq_inner` 在标签相等检查**之前**先处理
"两边都是 map" 这一类,并且仍然先比结构体标记。

整数键 map 顺势全通了(显示 / 相等 / 进容器 / 迭代),形状矩阵(13 种值 × 9 种
操作)因此 **172/172**。

## 18. `task.join_all`(2026-07-30)

变参,所以**没有哪一行能描述它** —— 一行只有一个 arity。三种拼写里,
`join_all(a, b)` 和 `join_all(a)` 是同一个循环(逐个 `rt.task_await`,推进一个 dyn
列表),现在原生;`join_all([a, b])` 传的是一个 `List<Int>` 句柄,长度只有运行时才知道,
需要 lkrt 里的一个循环而不是在这里展开 —— 那一种仍然回落。

元素显示是这条的验收点:dyn 列表对字符串的引号必须和 VM 的类型化列表**逐字**一样
(`["x","y z"]`),混合类型也一样(`[1,"s"]`)。差分语料里两种都在。


## 19. 不可达块(2026-08-05)

不可达的字节码块曾让**整个函数**拒绝降低。这类块没有前驱,`Ssa::read_recursive`
的 `preds.is_empty()` 分支直接答 `UndefinedOperand`;而它同时是后继块的前驱,于是
把定义从后继的交集里也抹掉 —— 报出来的寄存器往往是函数**自己的形参**:

    fn h(n: Int) -> Int {
        if n > 0 { return 1; } else { return 2; }
        let z = n + 1;      // 不可达
        return z;
    }
    → MIR lowering: register r4 is read at pc 12 before any definition

字节码也可以来自 `.lkm` 文件,后端不能建立在"前端从不发不可达代码"这个前提上。
`lower_function` 因此在算完边之后从块 0 做一次可达性:不可达块不贡献前驱边,不被
降低,在 MIR 里留一个空块、终结子指向自己。

同一处还有两个相邻的洞:

- **隐式返回块的分配条件不全。** 它只在有显式跳转越过末尾时才分配,而函数末尾
  可以本来就没有 Ret(全臂都 `return` 的 match 之后不再补隐式返回),于是
  `block_of(code_len)` panic。现在末块没有 exit 时也分配。
- **落到末尾与返回值冲突时报得不清楚。** 一条路径返回值、另一条落到末尾(答 nil),
  过去会生成 `-> i64` 函数里的 `ret void`,由 Cranelift 验证器报错。这与两条
  `return` 互相矛盾是同一件事,现在同样答 `ReturnTypeConflict`,干净回落。

门禁在 `cli/tests/aot_differential_test.rs` 的 `differential_control_flow`:
`match_arms_return`、`code_after_a_total_if`、`binding_arm_catches_nil`。

## 20. 列表字面量的载体也可以被后续 push 推翻(2026-08-05)

`let xs: List<Any> = [1, 2]; xs.push("a");` 此前在原生侧回落:

    MIR lowering: an operand at pc 3 is a str where a i64 is required

VM 的做法是把 `TypedList::Int` 就地拓宽成 `Mixed`;原生的 `Vec<i64>` 做不到这一步。
原来记的判据是"别的别名按静态类型直接读这块内存",在容器改成不变(docs/semantics.md
「可变容器不再是协变的」)之后不成立了 —— 不变意味着同一个值在每个名字上的元素类型
相同。剩下的问题只是**载体在构造时就定了**。

空 `[]` 早就有这条通路:降低时抛可重试的 `EmptyListGuessWrong`,定点记下字面量的 pc,
下一轮把它建成 Dyn 列表。同质字面量与空字面量在这件事上没有区别 —— 两者的元素类型都
是一个后续 push 可以推翻的判断 —— 所以现在共用同一条通路,名字也按这个含义改了:

| 原名 | 现名 |
| --- | --- |
| `Unsupported::EmptyListGuessWrong` | `Unsupported::ListElemTypeContradicted` |
| `Ssa::dyn_empty_pcs` | `Ssa::dyn_list_pcs` |
| `Ssa::empty_guess` | `Ssa::literal_list_ty` |
| `FnSigs::dyn_empty_lists` | `FnSigs::dyn_lists` |

两处字面量都记录并都认这个标记:常量列表(`LoadHeapConst`)和寄存器窗口列表
(`NewList`)。被推翻的那个从构造起就是 `list_h.dyn_new` + 逐元素 `to_dyn` + `dyn_push`。

门禁在 `cli/tests/aot_differential_test.rs` 的 `differential_lists`:
`widened_after_a_typed_literal`(Int/Float/Str 三种载体各推翻一次)、
`widened_from_a_register_window`(变量元素、循环里 push)。

### 20.1 map 载体同理(同日)

`let m: Map<String, Any> = {"a": 1}; m["b"] = "x";` 与空 map 的同一形状都回落。这两条
与列表那条是同一件事在另一个载体族里,而 map 侧一条通路都没有 —— 连"空字面量猜错就
重试"都只在列表侧存在。

补齐三处:

- **存储臂。** `SetIndex` 上没有 `Ty::MapStrDyn` 的臂,所以就算把 map 建成 Dyn 载体,
  也没有东西能存进去(`SetFieldK` 那侧本来就有,给结构体字段用)。
- **字面量记录。** 空 `{}` 与非空字面量都记进 `literal_carrier`,并都认
  `dyn_literal_pcs`:被推翻的那个用 `map_h.str_dyn_new` / `lit_finish_str_dyn` 重建。
- **报重试而不是直接拒。** `SetFieldK` 与 `SetIndex` 的类型化 map 臂原来答
  `TypeMismatch`,现在先问 `carrier_contradicted`。

"谁的载体被推翻了"这条判断从 push 那侧的闭包提成一个函数 `carrier_contradicted`,
列表与 map 共用 —— 一条规则,一处实现。名字也再改了一轮以覆盖两族:
`ListElemTypeContradicted` → `LiteralElemTypeContradicted`,`dyn_list_pcs` →
`dyn_literal_pcs`,`literal_list_ty` → `literal_carrier`,`dyn_lists` → `dyn_literals`。

门禁:`differential_maps` 的 `widened_after_a_typed_literal`、
`widened_from_an_empty_literal`。

## 21. 静态上只可能 raise 的操作:发 raise 还是回落(2026-08-06 裁决)

`let xs = [1]; xs[5].len();` —— `xs[5]` 的静态类型是元素类型 `Int`,`len()` 作用在整数
上只可能 raise。VM 报 "`len()` works on a String, List, Map, Set, Bytes or Slice, got …";
原生侧 `Opcode::Len` 的 `_` 分支答 `TypeMismatch`,整个模块回落。

仓库里有一条相反方向的先例:`SetIndex` 上的 Float 键 —— "no map carrier accepts one,
so the store can only raise. Emitting the raise keeps the rest of the program native —
refusing sent the whole thing back to the VM to produce the same error."

**这里不照做。** 差别在消息的份数:

- Float 键那处是**一句**固定文本,复制一份、两端各写一次是可控的。
- 类型错误是**每个方法一族**:`len` / `push` / `slice` / `sort` / … 每个都有自己的措辞,
  还带被拒类型的渲染。复制它们等于把"被捕获的错误消息就是 stdout"(见
  docs/semantics.md 的两端逐字裁决)这条约束铺到几十处,每一处都会漂。
- 回落这条路产生的是 **VM 自己**那句消息,按构造就是对的,永远不会漂。

而收益是负的:这个形状的程序执行到那一行就死,"其余部分留在原生"没有意义 —— 与
Float 键不同,那条可以在一个正常程序里被数据触发。覆盖率门禁 60/60,语料里也没有程序
命中这条。

所以规矩是:**只可能 raise 的操作,当它的消息是固定一句时发 raise,当它属于一个按类型
措辞的族时回落。** 前者省下的是程序其余部分的速度,后者省下的是两端消息不一致。

## 22. bundler 要合并的不只是 `impl`,还有 `struct` 声明(2026-08-06)

`use "geo"; println(geo.P { x: 4 })` 原生打 `{"x":4}`,VM 打 `P{x:4}`。

链条:`trait_env_prescan` 给**每个已声明的 struct** 发一个 type id,`NewObject`
只在有 id 时发 `map_h.obj_mark`,而 lkrt 的显示靠这个标记去查类型名与字段序。
CLI 的 AOT bundler 把 dep 的 `type_info.impls` 重编号后并进了 merged artifact
(§见 `cli/src/main.rs` 里那段注释:没有它,跨模块方法调用整段掉出原生子集),
却没有并 `type_info.structs`。于是导入来的类型拿不到 id,标记不发,显示退回
到载体本身 —— 一个 str-dyn map。

方法分发看不出来:它读的是编译期的 `ssa.struct_types`,那条路从 `NewObject`
的类型名常量拿名字,与运行时标记无关。所以 `a.norm()` 一直是对的,只有输出
是错的,而且只在类型声明在另一个文件时。**没有任何测试覆盖"跨文件构造 +
显示"这一格**,尽管跨文件构造和跨文件方法调用各自都有测试。

合并规则与 `impls` 那条一致:同名而字段不同则拒绝编译。VM 按 `TypeScope` 把
两个同名类型分得开,bundle 只有一张 id 表,分不开。

同一天的相邻改动让这条从"命名空间写法独有"变成"两种写法都会踩":
`use { P } from "geo"` 此前在 AOT 侧绑不到任何东西(`bundles[b].fns` 里只有
`P$new`,没有 `P`),整个程序回落到 Tier 0,反而打对了。补上 `$new` 回退之后
它开始原生降低,也就开始踩这条。

## 23. 类型化列表的拓宽:VM 就地变 `Mixed`,原生 raise(2026-08-06 实测)

```lk
fn sink(v: Any) -> Int { v.push(7); return 0; }
fn main() -> Int {
    let a: List<Int> = [1];
    let s: List<String> = ["q"];
    sink(a); sink(s);
    println("${a} ${s}");
    return 0;
}
main();
// VM: [1,7] ["q",7]        native: Error: runtime type error
```

**VM 对,原生错。** 判据是逐类探出来的,不是读实现读出来的:

| 写法 | VM | native |
| --- | --- | --- |
| `s.push(7)`(字面量,`s: List<String>`) | 检查期拒(#194) | 同 |
| `let v: Any = 7; s.push(v)` | `["q",7]` | `["q",7]` |
| 单一载体流进 `sink(v: Any)` | `["q",7]` | 回落 |
| **两种载体**流进同一个 `sink(v: Any)` | `["q",7]` | **raise** |

第二行说明"拓宽"就是这门语言的运行时规矩,所以第四行是原生侧的缺陷。

链条:`dyn.list_push` → `lklist::typed_list_push`,它用 `dyn_as_i64` /
`as_f64` / `as_str` 转换元素,不合型就 `raise_str("runtime type error")`。VM 的
`TypedList` 则就地拓宽成 `Mixed`,所有别名都看得见。

### 四条便宜的路都实测否掉了

1. **降低期一律拒绝 `Ty::Dyn` 接收者的 push**。覆盖率仍 60/60,扫描仍
   `identical=61`,分歧变回落 —— 但打掉了
   `clif_differential_test::a_boxed_typed_list_is_the_same_list` 的
   `writes_cross_the_box_both_ways`(`c[0].push(9)`,推入类型匹配、本来正确)。
2. **精确的静态规则**。要区分 `c[0]`(`List<Int>`)和 `sink` 的 `v`(`Any`),
   需要接收者的 **LK 静态类型**,而 AOT 只看到 `Ty::Dyn`;编译器在 `ListPush`
   处没有元素类型的 fact,加一条要动 artifact 版本。
3. **装箱时重建成 `Vec<LkDyn>`**。回到 §"类型化列表装箱是重建"修掉的那条,
   同样打掉 `writes_cross_the_box_both_ways`。
4. **lkrt 里就地拓宽**。句柄是 `*mut Vec<i64>`,而每个别名持有自己那份 tag;
   要让拓宽对所有别名可见,tag 必须在**对象里**而不在 `LkDyn` 副本里 ——
   那就是表示层改造本身。

所以修法唯一:让类型化列表的载荷能改 kind 而句柄保持有效(三个
`DYN_TLIST_*` tag 已经存在,缺的是那层间接和 lkrt 里 29 处读点)。任何修法
必须保住 `writes_cross_the_box_both_ways`。

## 24. 方法名常量先进池子:上限从"第 129 个方法调用"回到"256 个方法名"(2026-08-06)

130 个 struct、各一个方法、`main` 里逐个 `Sk { x: k }.mk()` —— `lk compile` 报
"the call at pc 1293 is not natively lowerable"。128 个可以,130 个不行。

反汇编:

    1287 LoadString r8 #257     ← 方法名常量下标 257
    1293 Call r10 r10 r3        ← 通用 Call,不是 CallMethodK

`CallMethodK` 是 abc 形式(7 位 opcode + 8 位 A + 1 位 K + 8 位 B + 8 位 C,
32 位已占满),`b` 装方法名的常量下标,只有 8 位。`lower_dynamic_method_call`
里 `name_const <= u8::MAX` 不成立就退回 `__lk_call_method` 的通用调用 ——
那条 AOT 降低不了,于是整个程序回落。编译器注释把这称作 "(pathological)"。

**它不是病态输入。** 常量池是**按函数**的,结构体名、字段名、方法名共用一个;
130 个 struct 字面量先占掉 260 个格子,方法名就被挤过了 255。逐类分离过,
单独哪一维都不封顶:同一方法调 400 次、普通函数调 1000 次、一个类型 200 个
方法、200 个类型同一方法名 —— 全都降低。

修法不动编码:降低函数体(以及顶层入口)**之前**,先把这个体里调用到的方法名
压进它的常量池。方法名下标落在 0..N,门槛变成编码本身说的那个数 ——
一个函数里 256 个不同方法名。实测悬崖从 129 移到 250~260,两端答案一致。

`a_function_may_call_two_hundred_distinct_methods` 断言的是**指令**不是能否
编译:退回那条路照样产出可运行的程序,只有 opcode 说得清走的是哪条。
反向验过:去掉预压,它红。

geomean 0.987x。

## §25 包依赖也进 bundle(2026-08-06)

`examples/lk-example-workspace/apps/demo/src/main.lk` 是 VM/原生扫描里**唯一**的回落。
它 `use mathlib;`(一个工作区依赖)然后调 `mathlib.double(n)`;bundler 只认**文件**导入
(`use "./m.lk"`),包导入不进队,于是那次调用落到 `lower_module`,而它只认 stdlib ——
整程序退到 Tier 0 的 VM bundle,约 3 倍慢,没有任何提示。

包依赖就是一个 `.lk` 文件,它产生的绑定与文件导入同形,所以按同一条路走:

1. `package_import_modules`(CLI):`PackageGraph::discover` 把每一种指向包的拼写解析成
   `(绑定名, 入口文件)`,和文件导入一起入队。四种拼写都答:`use dep;`、`use dep as n;`、
   `use { item } from dep;`、`use * as ns from dep;` —— 后两种没有自己的模块对象绑定,
   所以 bundle 按**模块名**做键,与降低那侧查它的方式一致。
2. `ImportEnv::build`(降低):四条臂都先查 bundle,查不到才按 stdlib 的读法绑定 ——
   `Module` / `ModuleAlias` / `Namespace` 绑成文件命名空间,`Items` 把条目绑成合并后的
   函数下标(与文件那条分支同款,含 `S$new` 回退)。缺这一步时 bundle 建好了却没人查。

扫描从 `identical=61 diverged=1 fallback=1` 变成 `identical=62 diverged=1 fallback=0`,
门禁期望值同步更新(`scripts/vm_native_sweep.sh`、`docs/testing.md`)。

## §26 `try` 区域:闭包入参、cell 入参、嵌套(2026-08-17)

三件事一起做,因为它们是同一个问题的三面 —— region body 被外联成独立函数,于是
"外层帧里的什么东西能跨过边界"这个问题要对每一类东西单独答一次。

**闭包入参。** lambda 在原生侧没有运行时表示,它是编译期 `GlobalRef`,所以
`try { r = inner(); }` 里的 `inner` 没有可 marshal 的机器字,body 读它报
`ReferenceAsValue`。按擦除 lambda 实参的老办法走:**身份走编译期**
(`SigInfer::try_body_lambdas`,body 把寄存器 seed 成 ref),**环境走运行时**
(每个捕获一个字,capture 顺序)。两侧都按 `try_body_params` 顺序走,所以布局不用
写在任何地方。

**cell 入参。** 被任意闭包捕获的变量是 cell:寄存器持 `GlobalRef::Cell`,内容在虚拟
slot 里。region 只要**读**一下这种变量就拒绝(body 的 `LoadCellVal` 找不到 ref)。
这不是罕见形状 —— 函数里任何 lambda 提到的参数都是,生成语料里 191 个嵌套 region
程序只有 2 个能原生化。改成跨**运行时 cell**:父从 slot 建一个,body 把它当
capture parameter 收(`inst::global` 本来就会用 `rt.cell_get`/`cell_set` 读写这种),
父在 region 后把 slot 读回来。父自己已经持指针时(它本身是 body 或闭包)直接传下去,
三层帧同一个 cell。

**嵌套。** `try` 里的 `try` 原来直接拒。放开只要两步:`scan` 只认本函数**自己**的
region(内层属于 body,body 自己被扫时才轮到它),外联循环改成在**增长的**表上走。
但放开之后暴露了三处静默错答,每一处都是"某个东西只在指令循环里被处理":

1. 内层 region 的写回发生在 **terminator** 里,而"把改过的寄存器镜像进本体自己的
   cell"只在指令循环里做 —— 内层 body 的赋值就这么丢了。抽出 `mirror_cells`,
   terminator 之后再跑一次,`rebound` 同理。
2. 本体自己的 cell,内层 region 写了也要带回来。原来的判据是"region 之后有人读",
   而那个读者在**上一帧**,本体自己不读。所以 `try_body_cells[本体]` 直接并进内层
   region 的 cell 集合。
3. 内层 body 的 `return` 经 return channel 交给本体,而本体自己**也**是 body 时,
   那个 `return` 还得再往上一层交。check block 原来直接 `Term::Ret`,于是值被读出来
   又丢掉(body 自己的返回类型是 `Nil`)。

还有一个不是静默的:trampoline 的 arity switch `default` 是 `__builtin_trap()`,而
预算只数了 inputs + cells,没数 return channel 的两个 cell,也没把 lambda 入参按
capture 数展开。七个 input 加一个 return 的 body 编译、链接、然后第一次进 region 就
`SIGILL`。预算改成精确计数,codegen 侧再加一道 `LK_TRY_MAX_ARGS` 拒绝,于是那个
`trap` 从构造上不可达。

**cell 入参是有类型的。** cell 本身是动态类型的,读出来是 `Dyn`,而 `Dyn` 算术没有
降低 —— 于是"被捕获的变量在 region 里参与运算"这个最常见的形状还是回落
(`if (p0 % 5 == 0)`,只因为函数里某个 lambda 提到了 `p0`)。父在建 cell 时把**内容
类型**记下来(`try_body_cell_input_tys`),body 的读按那个类型 unbox,写的时候类型对不上
就把这一项并到 `Dyn` 重试 —— 两端不可能对同一个 cell 持两种意见。三处 unbox
(region 输出 cell、return channel、cell 入参的读)合成一个 `unbox_cell_value`,`Bool`
的窄化不会在其中两处记得、第三处忘掉。

这一步把原生化从 176/836 提到 306/919,同时**暴露了上一条留下的一个静默错答**:
跨进 region 的闭包,它的 `Cell` 捕获原来是按值快照的,而 body 之后会通过自己的运行时
cell 写同一个变量 —— `try { a = a * 2; a = clo(); }` 原生答 `3`,VM 答 `6`。改成:
闭包的捕获若命中本 region 的 cell 入参,就**拿那个 cell**(`require_cell_capture` 把
callee 的捕获钉成 `Ty::Cell`,收敛回路照旧)。为此 region 入参的 marshal 拆成两遍 ——
先建 cell,再解析 lambda 环境 —— 每个入参的机器字按位置收集,最后按 `try_body_params`
顺序摊平,所以布局仍然是 body 走的那个。

**`performance` facts 按 pc 重基,不再整个丢掉。** 外联本来把 facts 全清空,理由是
"pc 会重基,读到错位的 fact 比没有更糟"。但这条流水线只读两张表 —— `for_loops`
(`cfg::exit_of`)和 `key_ops`(`inst::container`)—— 而 body 的 pc 就是父的 pc 减
`body_start`,所以重基是一次**切片**,切片不会产生错位。两张表里也都没有 pc
(`PerfForLoopFact` 的跳转是偏移量)。代价是具体的:`for` 循环**必须**有 fact,于是
region 里一句普通的 `for i in 0..n` 就整程序回落 —— 那是生成语料里剩下最多的一类。
其余的表是 VM 执行器的,而外联出来的 body 从不被 VM 执行(它只存在于本 crate 自己的
函数表里),保持 default。

门禁:随机生成的含 `try` 程序,六批不同种子共 1688 个,0 处分歧;facts 重基后原生化
从 306/919 提到 494/1066(约 46%)。fuzzer 加了嵌套 region + 闭包入参的形状;
`examples/syntax/try_catch.lk` 把三种形状钉进覆盖率门禁。剩下的回落主要是 trampoline
的 8 字上限和 cell 之外的 `Dyn` 操作数,都是诚实的拒绝。

## §27 cell 内容类型推广到闭包捕获,以及两处静默错答(2026-08-17)

§26 给 region 的 cell 入参记了内容类型;闭包**自己的**可变捕获有同样的毛病 ——
`clo` 一旦捕获变成 cell(赋值给它,或者把它交给 region 就会),`return p0 + a` 里的
`a` 读出来就是 `Dyn`,整条算术没有降低。同一个概念推广成 `cell_capture_tys`:
建 cell 的那一侧(它知道类型)记下来,callee 按它 unbox,写的时候类型不合就并到 `Dyn`
重试。join 必须**单调**(`join_cell_content`):定点的前几遍看到的是**临时**类型
(callee 的返回类型在它自己被降低一次之前就是默认的 `I64`),直接覆盖会让协议每遍都翻,
snapshot 永不收敛,预算耗尽 —— `examples/syntax/closure.lk` 直接不再原生化。

放开之后暴露了两处**静默错答**,两处都不是这次引入的,是这次才够得着:

1. **`unwrap_or` 吞掉了发现通道。** `acc.push(b)` 读 `b` 失败时,容器载体那条
   "literal 猜错了" 的拒绝会**顶替**原始错误 —— 而 `UndefinedOperand` 不是类型错误,
   它是**发现**,region 的写回 cell(`try_body_extra_cells`)正是按这个变体收集的。
   于是 `try { try { b = clo(); } catch { } } catch { }` 之后的 `acc.push(b)` 读到的是
   进 region 之前的 `b`,原生、无回落、无提示;而 `let t = b; acc.push(t);` ——
   中间多一条 `Move` 的同一个程序 —— 是对的。改成 `keep_discovery`:载体答案只顶替
   *类型*失败。

2. **外层 body 少报了自己重绑了什么。** 内层 region 的写回要等内层有 cell 才发生,
   而外层 body 的 `try_body_rebound` 是在那之前就报出去的 —— 报完之后它就是权威的,
   再也回不来。改成把内层 body 自己的报告并进来(传递闭包)。注意**不能**用语法扫描来并:
   那个扫描取每条指令的 `a`,于是 region 只是**改**了一下的容器(`ListPush a=receiver`)
   会被算作重绑,定点给它发 cell,而 cell 的往返恰恰会丢掉那次修改 —— 两种写法都试过,
   语法那种让 `examples/syntax/closure.lk` 彻底不再原生化。

门禁:六批随机语料(约 1340 个含 `try` 的程序)0 处分歧;coverage 60/60;十个 fuzz
种子;`examples/syntax/try_catch.lk` 把这两处静默错答各钉了一条。

## §28 双寄存器载体按两个字过 region 边界(2026-08-18)

`try` 写在 `for x in <一个 list>` 里面,整程序回落 —— 而这是最普通不过的形状,随手写的
第一个探针就是它。原因:list 的循环变量是**载体**(元素读是带边界检查的,类型是
`Maybe`;异构 list 则是 `Dyn`),而 region 的入参走 trampoline 的 `long long` 缓冲区,
`crosses_as_word` 对两寄存器的类型答"不能"。

两条看起来能走的路都是错的:

- **在边界上 unwrap**:`UnwrapMaybeX` 在缺失时 abort,而 body 可能只写了 `x ?? default`
  —— 那就把一个会答 42 的程序变成 abort。`examples/syntax/try_catch.lk` 的
  `defaulted(false)` 就是这个见证。
- **装箱成 `Dyn` 过去**:`Dyn` 自己也是两寄存器。

正确做法是按它本来的样子过去:**一个载体占两个寄存器,就走两个字**,到对面再拼回来
(`Inst::CarrierWord` / `Inst::CarrierFromParts`)。两条指令都取**裸的两半**而不是用
`MaybeValue`/`MaybePresent` 这类**解释**性的访问器 —— 要活着过去的是比特,而且"取两半、
按同样顺序放回去"这件事**不可能把约定搞反**,也就没有约定要维护。`MaybeF64` 的值半边是
`f64`,按位 bitcast 进出。

`crosses_as_two_words` 写成没有 `_` 臂的 match:漏一个类型会被劈成不存在的两半,多一个
会让 body 多绑一个参数。预算按 2 计。

顺带把"过不去"的诊断从 "an operand at pc N has a type outside the natively lowerable
subset" 改成指名道姓的 `OperandType`(`is a dyn where a machine word is required`)——
是哪个类型过不去,本来就是这个答案的全部内容。

(写这条时发现的另一个缺口 —— 字符串 list 循环 —— 见 §29。)

## §29 载体接收者先 unwrap(2026-08-18)

`for s in ["ab","cde"] { s.len(); }` 整程序回落,**不带 `try` 也一样**。原因:list 的元素
读是带边界检查的,循环变量类型是 `Maybe`,而 `Opcode::Len` 和 `CallMethodK` 都用
`ssa.read` 拿接收者 —— 拿到一个两寄存器的载体,查不到对应的 `len` 实现就拒。

改成 `read_scalar`,它先 unwrap。判据不是"方便",是**与 VM 一致**:VM 里
`m["zz"].len()` 会 raise(可被 catch),而 `lkrt_maybe_*_unwrap` 走的也是
`raise_str`,不是 abort —— 两边同样是可捕获的 raise。`examples/syntax/for_loop_patterns.lk`
两向都钉了:present 的答长度,absent 的被 `catch` 接住。

顺带一条教训:这条的回归覆盖**最初写进了 `closure.lk`**,结果那个文件整体不再原生化
(`opcode CallDirect (at pc 2)`),而两段代码**各自单独**都能原生化 —— 是和文件里已有的
`spawn` 段互相作用。覆盖率门禁要求 61/61,所以例子加在哪里不是随便的:**加完当场跑一次
`AOT_COVERAGE_REQUIRE_FULL=1`**,别假设"能编译的两段拼起来还能编译"。

## §30 闭包作为运行时值(2026-08-18 调查,**未落地** —— 有一个已知的错答)

原生侧每一个闭包都是**编译期事实**:降低知道某个寄存器指的是哪个函数,于是调用去虚拟化、
捕获变成隐藏的尾部实参。这覆盖了"建出来就调用"的闭包,也**只**覆盖那个。把闭包塞进 list、
放进结构体字段、从分支里返回,都没有编译期答案,全部报
`the closure in rN is a compile-time reference, not a runtime value`。

十个常见形状里六个回落:list / map / 结构体字段 / 循环里 push / 从分支返回 / 返回一个
包住参数闭包的闭包。能过的四个都是能**静态解析**的(擦除或特化)。

### 设计(已验证可行)

关键在于 `spawn` 已经证明了这条路:它按地址调用一个 lambda,而那个 lambda 的签名被降低
**强制成全 `Dyn`**,所以一个 arity switch 能调到任何一个。闭包值就是同一件事,只是环境跟在
指针旁边而不是当隐藏实参 —— 而追加环境的顺序**正好就是**原生签名已有的顺序(可见参数,
然后捕获)。

- **lkrt**:`DYN_CLOSURE` 标签 + `LkClosure { code, params, fn_index, env: Vec<OwnedVal> }`;
  `closure_new` / `closure_call` / `closure_arity`。环境按 `OwnedVal` 深拷贝(闭包按定义
  比建它的帧活得久),调用时再 materialize 进调用方 arena —— 和 goroutine 的快照同一套。
  `fn_index` 只为了 display 能打出解释器那句 `<fn #3(1 captures)>`:两个索引是同一个数
  (都是 `module.functions` 里的位置),而**值闭包不会是擦除克隆**,克隆是唯一被重编号的。
- **发现是按需的**:`Unsupported::ReferenceAsValue` 带上 lambda 下标,定点收进
  `SigInfer::value_lambdas` 并重试。只建不存的闭包因此继续去虚拟化,一分钱不多花。
- **`param_ty` 对 value_lambda 一律答 `Dyn`**,参数和捕获都是。
- **在定义点物化**(`MakeClosure` / `LoadFunction`),不是在每个读取点:寄存器从此持一个
  普通 `Dyn`,list 字面量、结构体字段、间接调用都不需要知道这件事。
- callee 是 `Dyn` 的 `Call` 走 `rt.closure_call`。

### 为什么没落地

`let fs = []; for i in 0..2 { fs.push(|| 7); } println(fs.len());` **编译通过但答
`runtime error`**,VM 答 2。同样的 push 放在循环**外面**是对的;循环里 push 一个真正的
`Dyn`(`src[0]`,src 是异构 list)也是对的 —— 所以既不是空 list 载体重试的既有机制坏了,
也不是捕获的问题(无捕获的 `|| 7` 一样错)。是物化本身和循环的相互作用,原因未查明。

一个会**静默答错**的形状比一个回落坏得多,所以这一整块回退了,只留下这份设计。

### 2026-08-18 续:错答的原因查到了,另外两件事也量出来了

**① 错答不在闭包,在 `ListPush`。** `LK_AOT_DUMP_MIR=1` 直接给出答案:

```
v0  = call list_h.i64_new()
v11 = call rt.closure_new(...)
v13 = call dyn.as_i64(v11)      // <-- 这里
      call list_h.i64_push(v12, v13)
```

`[]` 的载体被猜成 `ListI64`,而往里 push 一个 `Dyn` **不会**触发载体重试 —— 因为
`read_typed_scalar` 对 `(Dyn, I64)` 是**静默 unbox**(`dyn.as_i64`),读根本没失败,而
`carrier_contradicted_here_or_at_callers` 只在读失败时才被问。`dyn.as_i64` 拿到任何非整数
都会 raise,于是编译出来的程序答 `runtime type error`。

这对**声明过**的 `List<Int>` 是对的(类型检查器已经保证了元素是 Int),对**猜出来**的 `[]`
是错的。修法是在 `ListI64|ListF64|ListStr` 三个 push 臂里,读之前先问一次:值是 `Dyn` 且
载体是猜的,就直接返回矛盾,让定点把字面量重建成 `Dyn` list;载体是声明的则答 `None`,
照旧 unbox。改完那个复现立刻对了。

**这个修**单独拿出来是**够不着的**:要触发它,需要"静态类型看起来是标量、运行时却是 `Dyn`"
的值,而这正是闭包值才有的组合(静态类型是函数,运行时是 `Dyn`)。所以它必须和闭包值一起
落地,不能单独提交 —— 一段无法触发的防御性检查配一段它自己证明不了的 bug 叙述,比没有更糟。

**② 物化必须与编译期引用并存,不能替换它。** 「某个 lambda 会逃逸」是**函数**的属性,由它的
某一个用法发现;但它**别的**用法可能恰好是能静态解析的那些,而那些要的是引用。把引用换成值
让 `examples/syntax/closure.lk` 丢了降低:`xs.filter(|x| …)` 的类型化 HOF 路径读的是引用,
换掉之后那条臂就没了。`ssa.write` 会清掉 `builtin_regs`,所以引用要在 write **之后**补回去。
代价是每个逃逸 lambda 的构造点多一次可能没人用的 `closure_new`。

**③ 「一个寄存器同时挂引用和值」这个形状本身是错的 —— 这是 2026-08-18 第三轮的结论。**

先说③走到哪:把「值 = 原 lambda 的一个全 `Dyn` 签名**克隆**」(复用 `pending_clones`,
原函数签名不动,于是类型化 HOF 路径不受影响)这一步做完之后,覆盖率回到 61/61 无回归,
`[|x| x+1, |x| x*2]` 和循环里 push 都原生化并与 VM 一致。中间还修掉两处:

- `Move` 只搬引用不搬值 —— 而编译器在每个 `Call` 前都把 callee `Move` 进调用窗口,
  于是值刚物化就够不着了(`GlobalRef::ArgList` 早就有同款"双视图"补丁,就在旁边)。
- `lower_dyn_call` 要用 `read_scalar` 读 callee:从 list 里迭代出来的闭包是 `Maybe`,
  把载体交给 `rt.closure_call` 会答"value is not callable" —— 对一个确实可调用的值。

**然后随机差分扫描找到了第三种错法,而且是致命的那种**:

```lk
let fs = [];
fs.push(|x| x + 2);
fs.push(|x| x * 7);
let t = 0;
for f in fs { t = t + f(2); }
```

降低成 `dyn_push(v0, dyn.from_list(v0))` —— **list 把自己 push 进了自己**,两次。VM 答 18,
原生答 `value is not callable`。方法实参的窗口读到的是接收者,不是 lambda。

三次失败(`Move`、迭代出来的载体、方法实参窗口)是**同一个根因的三个面**:让一个寄存器
同时有"编译期引用"和"运行时值"两种含义,就要求**每一处搬运、每一处读取**都知道该带哪一个,
而它们并不知道。补一处就冒出下一处。

**下次换设计:在需要值的那个消费点物化,而不是在定义点。** 这样寄存器永远只有一种含义,
`Move`/窗口/迭代全都不用改。消费点是可以枚举的,而且正是今天报 `ReferenceAsValue` 的那些:
容器存入、结构体字段、分支里的 `return`、间接调用。做法是给这些位置换一个
`read_value(ssa, insts, sig, reg, …)` 帮手 —— 它在读到 lambda 引用时就地发 `closure_new`。
`Ssa` 拿不到 `insts` 是当初没这么做的原因,但消费点是**在** `insts` 在手的地方。

`①` 的载体修(`ListPush` 里 `Dyn` 值撞上猜出来的载体要先矛盾)仍然成立、仍然够不着,
仍然要和这件事一起落地。

## §31 引用与值的不变量是**单向**的(2026-08-18,第四轮的产出)

`Ssa::write` 会清掉那个寄存器的 `builtin_regs` 条目 —— 值遮住引用。反过来没有:直接
`builtin_regs.insert` **不清** `current_def`,而 `read_slot` 是**先看 `current_def`**。
于是一个从"值"回收成"引用"的寄存器,读回来的是**过期的值**。

这半条不变量本来就该在,和闭包值无关,所以单独落地了:`Ssa::bind_ref` 是记录引用的唯一入口,
它清定义;六个文件里的 `builtin_regs.insert` 全部改走它。`GlobalRef::ArgList` 是唯一例外,
而且是**故意**的 —— 它是一个已物化句柄的*视图*,不是"没有值的名字",两半本来就该同时活着
(`NewList` 和 `Move` 里那段"双视图"注释说的就是它)。漏掉这个例外会让参数包整个失效:
`the argument pack in r0 is a compile-time reference`,一次扫描里 14 个程序掉到 3 个。

**它修掉了一个已经在跑的静默错答(2026-08-18 补,第五轮)。** 反汇编一看就清楚:

```
0000 LoadHeapConst r1 #0     ; []
0001 Move r0 r1              ; fs = r0
0002 MakeClosure r1 …        ; lambda,落在刚才装 list 的那个槽
0003 ListPush r0 r1
```

字节码**复用寄存器**,于是 lambda 的 `MakeClosure` 正好落在 `[]` 字面量刚用过的槽上。
`read_slot` 先看 `current_def`,而记录引用不清它,于是 push 读回来的是**那个 list**,
把它 push 进了自己。`let fs = []; fs.push(|x| x + 1);` 就这样编译通过并且答错:
`typeof(fs[0])` 原生答 `List` 而 VM 答 `Function`,`println(fs)` 原生一路递归到爆栈。
`len()` 两边都是 1,所以从数字上看不出来 —— 这也是它一直没被发现的原因。

第四轮先做了 `bind_ref` 但**转换漏了三处换行写法**(`ssa.builtin_regs\n.insert(...)`),
`MakeClosure` 的零捕获早返回恰好是其中之一 —— 也就是恰好是这个 bug 的现场。补完之后这个程序
从"编译并答错"变成"诚实回落"。`aot/lower/src/tests.rs` 两条单测把规则和它的例外都钉住了。

### 第四轮闭包值走到哪

消费点物化(§30 的结论)按计划做了:`read_value` 是"我这里需要一个值"的唯一入口,
容器字面量、`ListPush`、间接调用三处接上,寄存器全程只有一种含义。十个形状里六个原生化,
覆盖率不回归。但 `fs.push(|x| x + 2)` 仍然降低成 `dyn_push(v0, dyn.from_list(v0))` ——
**push 进去的还是 list 自己**。`bind_ref` 装上之后这条**没变**,所以寄存器复用不是它的原因,
下次要查的是:`fs.push(<lambda>)` 到底走的是不是 `Opcode::ListPush`(`fs.push(1)` 走的是,
MIR 可证),如果不是,那它走的是哪条路、那条路怎么读实参。

## §32 闭包值落地了(2026-08-18,第五轮)

§30 的设计原封不动地成立,挡路的从来不是它 —— 是 §31 那个不变量。修掉之后这条路直接通了。

**发现是按需的。** 需要值的那次读报 `ReferenceAsValue`,它带上 lambda 下标,定点收下来。
只建不调用的闭包因此继续去虚拟化,一分钱不多花。

**值形式是一个克隆,不是那个 lambda 本身。** 闭包值经运行时的一个 arity switch 调用,所以
它必须是全 `Dyn` 签名;而同一个 lambda 的**其它**用法往往正是能静态解析的那些,类型化 HOF
路径按类型化签名取它的地址。把原函数钉成 `Dyn` 会让 `examples/syntax/closure.lk` 丢掉降低。
所以原函数不动,值形式是同一份 body 的第二份拷贝 —— 擦除克隆用的就是这套机制
(`pending_clones`)。

**物化在消费点,不在定义点**(`read_value`)。寄存器因此**永远只有一种含义**,`Move`、调用
窗口、迭代一行都不用改。§30 ③ 记的三种错法全是"一个寄存器两种含义"的后果。

**运行时**(`lkrt/src/lkclosure.rs`)照搬 `spawn` 已经证明过的形状:环境走同一个
`spawn_args_new`/`push` 块,调用时**追加**到实参后面 —— 而那正好就是原生签名已有的顺序
(可见参数,然后捕获)。环境按 `OwnedVal` 深拷贝(闭包按定义比建它的帧活得久),每次调用
再 materialize 进调用方 arena。`fn_index` 只为 display 与解释器一字不差
(`<fn #3(1 captures)>`),而且带的是**原**下标 —— 克隆是这条流水线自己的记账,程序观察不到。

**消费点是可以逐个接的,而且互不干扰(2026-08-18 续)。** 又接了三处:map 字面量的值、
`NewObject` 的字段值、以及普通调用与具名调用的实参(结构体字面量 `H { f: |x| … }` 走的是
后者)。于是十个形状里八个原生化。剩下两个不是"闭包"的问题,是**调用点的拼法**:
`m["inc"](3)` 和 `h.f(2)` 走的是 `CallMethodK`(VM 有一条"属性里放着一个可调用值"的路),
而 `let f = m["inc"]; f(3);` / `let g = h.f; g(2);` 两种写法现在都原生化。同一个语义两种拼法,
一种快一种回落 —— **已接**(见下)。

*键*不走 `read_value`:可调用的东西不是这门语言的 map 键,那条读保持原样。门禁:coverage 62/62(新增 `examples/syntax/closure_value.lk`)、随机闭包语料
350 个程序全原生 0 分歧、try 语料三批 0 分歧、容器惯用法 150/150、八个 fuzz 种子、
workspace、clippy `--all-targets`、no_std。fuzzer 的生成器也加了这一类形状。

## §33 属性里的可调用值(2026-08-18)

`m["inc"](3)` / `h.f(2)` 走 `CallMethodK`,而 `let f = m["inc"]; f(3);` 走 `Call`。两种拼法
一个语义,原来只有后者原生化。现在前者接到 `rt.closure_call_property`:在
`lower_method_dispatch` 那个**大 match 的最后**,等所有真方法臂都拒绝之后才轮到它 —— 所以
它不可能遮住任何方法。十个形状里十个,其中八个原生化;剩两个是"调用一个调用的返回值"
(`pick(true)(5)`),那是另一件事。

**它有自己的运行时入口,而不是给 `closure_call` 加个参数**,理由只有一个:**miss 的措辞**。
map 是唯一一个"没找到"有两种原因的接收者,解释器把两半都说出来
(`a Map has no method \`x\`, and this map has no key \`x\` holding a function either`),
而 `closure_call` 只会说 "value is not callable"。同一个 `catch` 里拿到两种字符串就是分歧,
所以 `lkrt_closure_call_property` 带上名字,自己发那句一模一样的话。
`examples/syntax/closure_value.lk` 把这句话逐字钉住了。

发现它靠的是**顺手探一下 miss**:功能本身的十个形状全绿,是问"那不存在的方法呢"才露出来的。
加一条新的 raise 路径时,**它答错话**和它答对值一样要探。

## §34 从分支返回的闭包(2026-08-18)

`pick(true)(5)` 回落,报的是调用点 `opcode Call not lowerable` —— 但根因在**返回点**。

`Ret` 那条臂里,`ret_closure_candidate` 一旦匹配就**无条件** `return Err`,不管摘要有没有真的
记下来。摘要记下来时那样做是对的:调用点自己用实参把闭包造出来,这个 body 根本不发射,所以
这里没有东西可返回。但两个 return 的函数**记不成摘要**,于是它也被同一条 `Err` 拒掉了 ——
那在"闭包还不能当值"的年代是唯一的答案,现在不是了。

改成只在**真的记下摘要**时才 `Err`,否则落到 `read_value`,返回一个闭包值。调用点那边的
`peek` 守卫本来就认 `Dyn`,于是 `pick(true)(5)` 一起通了。

十个形状里九个原生化。剩下的 `twice(|a| a + 3)(1)` 是"返回一个捕获了**函数参数**的闭包",
它需要参数位置上的闭包值一路传下去,是下一件事。

教训:**报错的位置不一定是原因的位置。** 调用点说"这个 Call 降不了",而它降不了是因为被调用的
东西没有类型;类型没有是因为返回点先拒了。查这类问题要沿着数据流往回走一步。

## §35 全静态环境被擦掉了,而值需要它(2026-08-18,**修的是自己两天前发的错答**)

```lk
let add = |x| x + 1;
let fs = [|y| add(y) * 10];
println(fs[0](2));
```

VM 答 30,原生答 `value is not callable` —— 而这是 §32/§33/§34 发出去之后就存在的分歧。

原因:一个环境**全是静态引用**的 lambda 在运行时不带任何东西,所以 `MakeClosure` 把它记成
一个光秃秃的 `Lambda`(`captures_all_static`)。对"就地解析那些引用"的调用来说这是对的;
对一个**值**来说不对 —— 它的克隆照样有那么多捕获参数,而环境是空的,于是它读过头、
调用了读到的东西。

改法:物化时发现 `captures` 为空而 `capture_count > 0`,就按 `capture_count` 把环境按
`ClosureCapture::StaticRef` 重建,再由下面那个循环逐个从 `sig.ref_captures` 里把被引用的
callable **递归物化成值**。

**这一条是自己的语料没覆盖到的类**:随机闭包语料从来不生成"lambda 捕获另一个 lambda",
所以 200/200 全绿而分歧还在。语料和 fuzzer 的生成器都补了这一类。

教训:**一个功能"十个形状全过"不代表它对**,它只代表那十个形状对。新功能引入的**新组合维度**
(这里是"捕获的东西本身是不是同类值")要单独列一遍,而不是等它出现在随机语料里。

## §36 闭包的身份(2026-08-18)

VM 里闭包按**引用**比较:`let g = f` 是同一个对象,写法相同的两个 lambda 是两个对象。
把 lambda 在**每个使用点**现场物化(§32 的做法)会给每次读取造一个新句柄,于是

```lk
let f = |x| x + 1;
let fs = [f, f];
fs[0] == fs[1]      // 解释器 true,编译后 false
```

三处一起改才对:

| 位置 | 改动 |
| --- | --- |
| `inst/call.rs::bind_lambda` | 一个被当作值使用的 lambda 在**定义点**物化一次(`sig.value_lambdas` 已经记录了这件事),寄存器从此持有句柄 |
| `lkdyn.rs::dyn_eq_inner` | `DYN_CLOSURE` 按 payload 指针比较 |
| `lkdyn.rs::contains_eq` | 把"按句柄比较"改成**默认**分支,只排除 `DYN_RAW`。原来是枚举包含的 tag,这正是 `Set`/`Bytes`/窗口/typed map 各自漏掉过一次的原因 |

定义点物化的代价是这个 lambda 的调用不再去虚拟化,只有程序真的传递它时才付。

三个连带的洞,都是"引用变成值"之后别处的判断依据没了:

1. **列表 HOF 的快路径**要求寄存器里是 `GlobalRef::Lambda`。物化之后没有了,`xs.map(f)`
   整个模块都掉到通用路径(而通用路径对它根本没有降级),`examples/general/sort_search.lk`
   与 `examples/syntax/closure.lk` 一起从门禁里掉出来。补法是 `Ssa::closure_fidx`:
   环境为空的闭包值记住它命名的函数,`lambda_at` 两种写法都认。
2. **空 `[]` 的载体猜测**。`b.push(f)` 原本走的是"寄存器里是 lambda 引用"那一支,那一支会报
   `LiteralElemTypeContradicted` 把字面量拓宽成 `dyn`。物化之后寄存器里是普通 `Dyn`,落到标量支,
   `read_typed_scalar` 用 `dyn.as_i64` 把闭包**拆箱**了——程序照样编译链接,运行时在它唯一要存的
   值上抛错。补法是 `Ssa::closure_values`:闭包值永远不是标量,拆箱请求直接拒,调用方据此拓宽。
3. **`try` 区域的 lambda 输入**。`sig.try_body_lambdas` 是写一次就长期有效的表(两侧靠它对齐),
   而同一个 lambda 后来变成值之后这条记录不再成立,两个事实互相矛盾,定点无法收敛——区域体一直
   索要一个发现分支已经给过的值。`function.rs::try_body_lambda` 在读出时按 `value_lambdas` 过滤。

## §37 不能做键的值要说清是什么(2026-08-18)

`vm_mirror::key_from_dyn` 的兜底分支对所有非键 tag 都抛"Float cannot be a map key or set member",
所以 `Set(["ab".bytes()])` 和 `Set([closure])` 都自称 Float。解释器有两句话:浮点一句,其余一句带类型名;
`Set(...)` 与 `set.add()` 还各自加一个前缀。现在按解释器分:`key_from_dyn_in(v, context)`。

带类型载体的 map **不**走这个函数(键是拆箱存的),所以另加了 `dyn.as_key_i64` / `dyn.as_key_str`
两个 ABI:先按键的可用性拒绝、再拆箱。`m[|x| x] = 1` 原来答"runtime type error"。

## §38 回调是值时的三个 fold(2026-08-18)

`list_h.dyn_map_fn` 等三个 helper 收的是**函数地址**,只有降级时知道寄存器命名哪个 lambda 才有。
`xs.map(fs[0])`、以及回调从参数进来的写法,拿到的是 `DYN_CLOSURE`,于是加了对应的
`dyn_map_closure` / `dyn_filter_closure` / `dyn_reduce_closure`。

filter 的判定按解释器来(`core_methods::list_filter`):`Bool` 取自身、`nil` 为假、其余为真。
`*_fn` 那条路做不到这件事——它在编译期就要求回调返回 `Bool`——而闭包的返回类型这里不知道。

`lkrt_closure_call` 拆成"取参数块"和"调用"两步(`call_with`),这三个 fold 每个元素只造一个
`Vec`,不再为了让同一个函数马上拆开而先造一个参数块。

## §39 按函数索引的表必须一起增长(2026-08-18,**影响面最大的一个**)

`SigInfer` 里有八张按函数索引的并行数组(`param_obs` / `ret_types` / `ret_known` /
`lambda_params` / `specialized` / `plain_called` / `ret_closures` / `ret_closure_poisoned`)。
`funcs` 在三个地方增长:`try` 体外联、lambda 实参特化、闭包值克隆。三处各自 push 自己关心的
那几张,**集合不同**:外联只 push 前三张。

于是模块里只要有一个 `try`,`lambda_params.len()` 就比 `param_obs.len()` 少,
而特化用的是 `let clone = sig.param_obs.len()`——`lambda_params.push(identity)` 落在了
`clone - 1` 上,记到了别人头上。可观察到的现象:

```lk
try { … } catch e { … }
fn ap(xs, f) { return xs.map(f); }
ap([1, 2], |x| x + 1)        // 模块里有 try,这一行就不再原生化
```

现在只有 `SigInfer::push_function` 一个入口,一次给八张表各追加一格,并 `debug_assert!` 长度一致。
教训与 snapshot 元组那条相同:**并行结构的增长点必须只有一个**,否则漏掉一处是静默的。

## §40 被程序写过的槽位就是程序的(2026-08-18)

`GetGlobal` 先按**名字**解析:内建函数名、`module::member`、stdlib 模块名。导入绑定那一段已经
写了正确的规则——"只有槽位从未被写过时才用导入的含义"——但名字那一段没有这个前提。
于是 `SetGlobal` 只能反过来兜:凡是写一个名字被识别的槽位就拒绝整个程序,否则后面的读会解析成
过时的模块含义。

代价是 14 个普通变量名(`time` `env` `hash` `iter` `os` `io` `net` `math` `fs` `bytes`
`regex` `task` `process` `encoding`)一旦被函数读到,整个程序就掉出原生路径:

```lk
let time = [30, 45, 60];
fn first() -> Int { return time[0]; }   // 有这一行就不原生化
```

改法是把导入那一段的规则提到名字那一段之前:`prescan_shadowed_globals` 语法扫出所有被
`SetGlobal` 写过的槽位,被写过的槽位不再按名字解析,`SetGlobal` 那边的名字检查随之删掉。
顺带 `let len = 42` 这类遮蔽内建函数名的写法也一起原生化了。

语法扫描而不是"是否已观察到写入":同一趟里读可能先于写降级,按观察顺序回答会随趟次变化。

## §41 字段的声明类型一直被扔掉(2026-08-18)

`struct P { count: Int }` 的实例是一个字符串键 map,字段读是 `map_h.str_dyn_get`,结果是装箱的
`Dyn`。声明说了 `Int`,而**没有任何东西把这句话带到降级这一层**——`StructDecl` 只存字段名。
于是 `p.count + 1` 两边都装箱、走 `dyn.add`,`p.count >= 0` 走 `dyn.ge`。

**按声明类型给字段读定型这件事不成立,当天就撤回了。** LK 是渐进类型:一个无类型参数可以
往声明为 `Int` 的字段里写字符串,解释器允许:

```lk
struct A { v: Int }
fn poison(p) { p["v"] = "s"; }
let a = A { v: 1 };
poison(a);
println(a.v);        // 解释器 "s";按声明类型拆箱的编译版抛 runtime type error
```

列表元素那一侧同样:`fn add(xs) { xs.push(B { … }); }` 能把一个 `B` 推进 `List<A>`,
类型检查器看不到。所以**声明的字段类型不是运行期保证**,不能用来给读定型。
要让它成立,得在动态字段写入处按声明类型做运行期检查——那是语言语义的改动,单独一项。

留下来的是**按下标读**,这一条成立,而且性能本来就在这里:

| 形状 | 哈希查找 | 按下标 |
| --- | --- | --- |
| `for i in 0..3e6 { total += s.a; }` | 0.32s | 0.06s |

**这两个数字是调试版 lkrt 的**(见 §43 的计时陷阱),所以 5 倍这个**比值**成立,
"~107ns 一次字段读"这个绝对值不成立。声明的字段序随模块走(`StructDecl` 现在带序),下标是编译期常量;
**字段名仍然一起传下去并比较**,因为顺序不是保证——从 hybrid 桥或别处建的实例可能是别的顺序,
比不上就退回按键查找。这一条不依赖任何类型假设,所以是安全的。

改动:`StructDecl.fields` 从 `Vec<String>` 变成 `Vec<StructFieldDecl>`(名字 + 声明类型文本,
沿用 `TraitDecl` / `ImplDecl` 的 `Type::display()` 约定),artifact 版本 17 → 18。
AOT 侧 `TraitEnv::struct_field_tys` 记 `(结构体名, 字段名) → Ty`,字段读之后按它拆箱。

只拆**标量**(`Int` / `Float` / `String`)。容器字段的载体不是声明能钉住的(`List<Int>` 可以是
任何一种列表表示),猜一个载体正是错答的来源。`Bool` 也留在外面:`dyn.as_bool` 返回 ABI 的 `I64`,
而 MIR 的 `Ty::Bool` 是 codegen 会 `uextend` 的一位值,只改类型不加那次比较过不了 Cranelift 校验。

结构体身份也跟着值走了两步:`Ssa::list_elem_struct` 记"这个列表的元素都是结构体 N",
元素读把身份传给结果;`Ssa::inherit_provenance` 在 phi 处继承(此前只有猜测载体 `literal_carrier`
有这个待遇,三张表现在在同一处继承,免得再加第四张时漏掉)。

循环头的 phi 也补上了(同日)。Braun 算法里循环体在 header 封口**之前**降级,所以等所有边到齐
再继承等于身体永远看不到这件事。改成**创建时**从已填充的那个前驱种下(`seed_provenance`)——
`phi_ty` 给类型定型用的正是同一个前驱,同样是乐观的——操作数到齐时校验
(`verify_seeded_provenance`),被某条边推翻就报可重试的 `Unsupported::PhiProvenance`,
下一趟对这个槽位不再种(`no_phi_provenance`)。这是 `dyn_loop_phis` 对**类型**做的同一件事。

于是 `while cursor >= 0 { walked += nodes[cursor].value; cursor = nodes[cursor].next; }`
整段原生化,字段读是 `dyn.as_i64` + `int.add` / `icmp.ge`,不再是 `dyn.add` / `dyn.ge`。
`examples/syntax/struct.lk` 钉住了这个形状。

注意 snapshot 元组:新字段**追加在末尾**(索引 23),不是插在中间。第一版插在索引 5,
把后面每一项都错位成比较别的东西——那正是那段注释警告的事。

## §42 循环里的 format 模板(2026-08-18)

`"{}".format(x)` 在编译期展开,所以模板必须是常量。而字节码编译器会把**循环不变的字面量提到
循环外**(`vm/compiler/loop_consts.rs`),于是循环体内那个模板是一个 **phi 参数**,不是字面量本身。
按 SSA 值查 `const_strs` 什么也查不到,写在循环里的每一个 `"{}".format(x)` 都掉出原生路径:

```lk
let s = "";
for i in 0..n { s = s + "[{}]".format(i); }   // 整段回退
```

`Ssa::reg_const_str` 早就为这件事写好了——"Recovers `println` format strings the compiler's
loop-literal cache hoisted out of the loop body"——但它从**寄存器**出发,而 `format` 这一处手里
只有已经读出来的 SSA 值。补了 `Ssa::const_str_value`:值查不到就找它是哪个 phi 的参数,
改按那个 phi 的寄存器走同一个回溯。`println` 那条路一直是对的,`format` 这条不是。

## §43 map 字面量的两段式构建是遗留的(2026-08-18)

一个 24 项的 map 字面量,建 10 万次(**调试版 lkrt**,见下面那条):

| | 时间 |
| --- | --- |
| 24 元素的**列表**字面量 | 0.08s |
| 24 项的 map 字面量(两段式) | 1.79s |
| 同上,直接建进载体 | 1.14s |

> **计时陷阱:调试版 CLI 的 `lk compile` 链接的是调试版 lkrt。**
> `native_executable::lkrt_staticlib_path` 在 `target/<profile>/` 及其 `deps/` 里找归档,
> `target/debug/lk` 找到的就是调试版的。上面那组数字因此把绝对开销放大了数倍——
> 同一个 lkrt 两边比较的**相对**结论仍然成立,绝对值不成立。
>
> 用 `--profile dist` 的 CLI 重测,100 万次:

| | 每次构建 | 每项 |
| --- | --- | --- |
| 24 元素列表字面量 | 0.38µs | ~16ns |
| 24 项 map 字面量 | 2.1µs | ~88ns |
| 24 字段结构体 | 3.0µs | ~125ns |

所以 map 比列表贵 **5.5 倍**、结构体贵 8 倍,不是调试版数字暗示的 20 倍。

两段式是这样的:`lit_new` 建一个 `RtKey` 键的中间 map,每个键和值都装箱后 `lit_set` 进去,
再由 `lit_finish_str_i64` 之类遍历它、把键 `to_owned()` 一遍插进真正的载体。
**两倍的哈希插入、两倍的键分配,外加每项一次装箱。**

第二段存在的理由是"按 VM 的 stage-1 **哈希序**重放进 stage 2"——那是 map 迁到 IndexMap 之前的事。
现在两边都是插入序,按写的顺序直接插就是同一个结果。而载体形状本来就是**编译期**选定的
(`lit_finish_str_i64` 这个名字就是降级时挑的),所以中间那一步没有任何信息是必需的。

寄存器窗口(`NewMap`)和常量(`LoadHeapConst`)两条路都改成直接建。`lit_*` 保留给形状在运行期才知道的
用法(`MapRest`、解码器)。

两条量过但**没有**收益、已撤回的尝试,记下来免得再试:预留容量(`with_capacity`,1.54s → 1.53s),
以及按声明类型给字段读定型(见 §41)。

下一层的结构性问题还在。50 万次 × 24 项,`--profile dist`:

| 每项 | |
| --- | --- |
| 列表元素 | ~15ns |
| **整数**键 map 项 | ~48ns |
| **字符串**键 map 项 | ~90ns |
| 覆盖一个已有的字符串键 | ~8ns |

三条读法:哈希+探测只值 ~8ns;map 结构本身比列表多 ~33ns;字符串键再多 ~42ns,
那是**每项一次 String 分配**(以及之后一次释放)。

试过并撤回的两条:预留容量(1.10s → 1.07s,在这台机器的噪声里),以及把键换成
`Cow<'static, str>` 让常量键不分配——后者能省掉那 ~42ns,但需要一个"这个指针是常量、活得够久"的
ABI 约定,写错就是 UB 而不是错答。为 45% 换这个,不划算。

**记录表示**才是该走的路:声明的结构体按声明序存一个值向量,字段下标编译期已知
(读的一侧 `str_dyn_get_at` 已经在按下标走了),一个字段能落到列表元素那个量级,而且不存键、
没有借用指针的问题。代价是构造、读写、display、相等、迭代、spread 都要跟着改,
声明不可达的结构体(跨模块、宿主建的)还得保留 map 形式。

## §44 打包对"调用参数上的方法"判得太粗(2026-08-18)

打包会拒绝"可能通过容器参数写入"的模块——打包把调用方的容器**按引用**交过去,而 VM 给每个模块
一份拷贝,写入的可见性因此不同。判定里,**调用参数上的一个用户方法**一律算作写:字节码里没有类型,
名字看不出它做什么。

代价是一个 trait 最典型的用法就打不了包:

```lk
// shape.lk
trait Area { fn area(self) -> Int; }
impl Area for Sq { fn area(self) -> Int { return self.s * self.s; } }
fn describe(v: Area) -> Int { return v.area(); }
```

只要模块里有 `describe` 这一个函数,整个模块拒绝打包,**它导出的每个名字都不再解析**——
连 `Sq { … }` 这种与 trait 无关的构造也一起掉出原生路径。

方法体就在同一个模块里,而同一个定点本来就在判断"它的接收者安不安全"。所以改成查那个结果:
一个方法名的**所有** impl 体的第 0 号参数都安全,这次调用就不算写。

改这一条要先补另一条,否则不成立:污点必须**穿过容器读**。此前只有 `Move` 传递污点,
于是 `fn contains(self) { self.items.push(x); }` 里,`self.items` 之后的 push 落在一个
"没有污点"的寄存器上,方法看起来没碰 `self`。`cli/tests/bundle_container_parameter_test.rs`
里正好有这个用例,它抓住了第一版的错误。现在 `GetFieldK` / `GetIndex` / `GetList` /
`GetIndexStrI` 也传污点,读出来的东西属于被读的容器。

## §45 那条 callable-property 分支从来没跑过(2026-08-18)

`m.thing()`(`thing` 是 map 里一个装着函数的条目)的降级分支,对 `Ty::Dyn` 接收者发的是
`dyn.map_get`——**ABI 里没有这个名字**。于是整个模块过不了 MIR 校验,那是模块级的回退,
所以从来没有人看见过它:这条路一次都没跑过。

打包放宽之后它才被够到。补上之后立刻暴露了它本来就错:`c.name.upper()` 读的是结构体字段,
接收者因此是 `Dyn`,而这条分支是最后兜底的,于是把 `upper` 当成"map 里的可调用条目"去找,
答了 `a Map has no method \`upper\``——解释器答的是 `A`。**这是错答,不是回退。**

正确的做法是这条分支**不接受装箱接收者**:装箱值的运行期类型才决定 `upper` 是哪个方法,
静态这一侧不知道,就该拒绝、回退,而不是猜一个。`Ty::MapStrDyn`(确实是 map)保留。

教训:一个从未成功过的分支,和一个不存在的分支,外部看起来一样。补一个缺失的 ABI 名字之前,
先问它当初为什么缺。

两条后续,免得再靠运气发现:

- `Unsupported::InvalidMir` 现在**带上校验错误本身**。此前只说"没通过 MIR 校验",
  真正的原因(`UnknownAbi { module: "dyn", name: "map_get" }`)藏在一个没人会设的
  `LK_AOT_DEBUG_FAILURES` 后面。整模块回退的诊断不该需要开关。
- `aot/lower/tests/abi_names.rs` 扫源码:`AbiRef::new( … )` 里每一个**当作名字用**的字符串
  字面量都必须在 schema 里。名字可以由条件式选出来
  (`AbiRef::new("dyn", if name == "keys" { "map_keys" } else { … })`),所以两个位置都读,
  只跳过 `==` / `!=` 的操作数。把 `str_dyn_get` 改回 `map_get` 试过,这个测试确实会红。

## §46 把方法分发表逐条对着解释器跑一遍(2026-08-18)

§45 之后的自然一步:`lower_method_dispatch` 声称能降级 63 个方法名,把它们**逐个**在
解释器和原生上跑一遍比对(接收者:String / List / Map / Set / Bytes,以及同一批经过
无类型参数、接收者变成装箱值的版本)。两条错答:

- `m.clear()` 原生答 `nil`,解释器答**那个 map 本身**(同一个句柄,已清空)。列表那条紧挨着,
  写的是"值就是接收者",map 这条却造了个 nil 出来。
- `m.remove(k)` 原生**接受**并返回删掉的值,而解释器根本没有这个方法(map 只有 `delete`)。
  这个方向更糟:一个解释器拒绝运行的程序跑起来了,而且只在一个后端上。

装箱接收者那一批 0 分歧。

把范围再推一层——**空接收者、越界与负下标、以及会改接收者的方法返回什么**——又出两条:

- `Set.clear()` 也答 `nil`。list 和 map 各自修过一次,set 这条落下了:同一个约定分散在三个分支里,
  改的时候没有一处会提醒还有第三处。
- `[1,2,3].chunk(0)`:解释器抛 `list.chunk() size must be positive`,原生把这句**打到 stderr**,
  再抛 `runtime type error`。于是解释和能被 `catch` 到的句子是两个不同的字符串。
  `rt_eprintln!` + `raise_str("runtime type error")` 这个组合全仓只此一处,已改成消息本身就是错误。

stdlib 模块那一面(`math` / `string` / `iter` / `bytes`,约 70 个调用)0 分歧。

两条收尾,免得这类东西再靠临时脚本发现:

- **这份比对留了下来**:`examples/stdlib/method_surface.lk` 把 85 个用例(9 类接收者,
  含空容器与越界下标)各打印方法名、结果、以及**调用之后的接收者**。示例语料本来就被
  VM/native 差分门禁逐个跑两遍,所以这份文件就是那次比对本身,变成常驻的。
- **`clear` 的约定收成一条**:此前三个分支各写一遍"值是接收者",其中两个写错成 `nil`,
  而那条约定只写在 list 那个分支的注释里——另外两处的读者不会看到。现在一个分支加一个
  `clear_helper(ty)` 查表,答案只说一次。

做法本身值得留着:能降级的方法名是从降级表的 match 分支里正则抽出来的,所以"表里写了什么"
和"实际跑起来是什么"是对着的——这正是 §45 那种"写了但从没跑过"的反面。

## §47 `try` 里跳出外层循环:最大的一块 try 回退(2026-08-18)

随机 try 语料 231 个程序,128 个原生化。剩下的 103 个里 **52 个是同一条原因**——
`the try region at pc N cannot be outlined: the body jumps out of the region`。逐形状确认:

| 形状 | |
| --- | --- |
| `for … { try { … } catch … }`,body 里没有跳转 | 原生 |
| 同上,body 里 `return`(从外层函数返回) | 原生 |
| `try { for … { break; } }`(循环整个在 try 里) | 原生 |
| `for … { try { … break; … } catch … }` | **回退** |
| `for … { try { … continue; … } catch … }` | **回退** |

也就是说:`try` 体里的 `break` / `continue` **属于包住 `try` 的那个循环**时,区域没法外联——
外联出来的函数没有那个循环可跳。

`return` 这条已经解决了,而且用的正是可以照搬的办法:body 多收两个 cell(一个标志、一个值),
写完就返回,调用方在检查块里看标志决定是否把返回值再往上递(`SigInfer::try_body_returns`)。
把标志从布尔换成**结果码**(0 落空 / 1 return / 2 break / 3 continue),body 那侧就齐了。

难的是调用方那一侧:检查块要跳到**外层循环**的 break 目标或 latch,所以外联时得知道这两个 pc。
`try_region::scan` 现在只判断"目标落在区域外就拒绝",要改成"落在外面的哪里"——
外层循环的出口、latch,还是别处(那才是真拒绝)。这需要 `cfg` 那边给出循环结构。

**已解决,见 §48。** 没有在这一轮动手:控制流改错正是会产生静默错答的那一类,值得单独一轮从头做,
而不是在一段很长的会话末尾开工。

## §48 `break` / `continue` 跳出外层循环:接上 §47 那条通道(2026-08-19)

§47 写的办法照做了,标志从布尔换成结果码(0 落空 / 1 return / `2 + k` 第 k 个出口),
但**调用方那一侧比 §47 设想的简单**:检查块不需要知道"外层循环的 break 目标和 latch"
在哪里,因此也不需要 `cfg` 给出循环结构。它只需要知道 body 跳到了**哪个 pc**——
那个 pc 在父函数里本来就是一个块,把它记成区域所在块的后继,phi 操作数就由既有的
SSA 构造顺手算出来,检查块用 `args_to` 读回去。一个 `try` 里的 `break` 在父函数里
不是什么特殊东西,只是某个块的终结指令有四个后继而不是两个。

body 那一侧的做法值得单独记:**跳转一直到最后都还是跳转**。每个不同的目的地在 body 的
`Return0` 之后得到一条一指令的 trailer,跳过去的那些 `Jmp` 被改写成指向它。于是 leader
查找、CFG、SSA 构造一路上 `break` 都是普通跳转,只有到了 trailer 才变成一次写标志。
另一种写法——按指令种类拦截跳转本身——要为每种终结指令各写一遍改写,而它漏掉的那一种
会静默错答。

只接受无条件 `Jmp`。`break` / `continue` 编译出来就是这个:前面的条件是它自己的融合分支,
目标在 body **内部**。条件分支只有一条边离开区域的形状会明确报错,不做近似。

### 语料测量(生成 121 个 `try` 程序,每个一个函数)

| | 原生化 | 唯一阻塞原因 |
| --- | --- | --- |
| 改之前 | 2 / 121 | `break`/`continue` 属于外层循环(118 条) |
| 改之后 | 108 / 121 | `too many values cross the region boundary`(13 条) |

VM/native 逐行比对:50 个种子 × 5 个程序,0 分歧。61 程序 sweep、覆盖率门禁(65/65)、
四个种子的生成式差分、`cargo test --workspace --all-features` 全绿。

### 一个静默错答,以及现在拦住它的东西

区域是按**块的 leader** 而不是 `TryBegin` 的 pc 查的,于是
`while c { i = i + 1; try { … } }` 找不到任何出口:body 声明了五个参数,调用点只传四个。
这不是链接错误——trampoline 拿的是 body 的**地址**,再按 switch 选中的元数强转——
所以它跑起来了,并解引用了第五个寄存器里恰好剩下的东西。

`clif.rs` 现在把 try-region 调用的字数和被调 body 声明的字数对一遍。这条检查此前不存在,
而且没有别的东西能替代它:签名不经过 Cranelift 的类型检查。

## §49 try body 自己读参数缓冲,元数上限随之消失(2026-08-19)

§48 之后唯一剩下的阻塞原因是 `too many values cross the region boundary`(13/121)。
`LK_TRY_MAX_ARGS = 8` 不是 ABI 约束,是 `lkrt/src/try_trampoline.c` 里一段手写 switch
的长度:它把 body 的地址强转成九种 `(long long, …)` 原型之一,`default` 是
`__builtin_trap()`,所以降级那一侧必须先算好字数再拒绝。

关键事实是**调用方本来就把参数摊在一个栈缓冲里**——`lkrt_rt_try_region` 收到的就是
那个 `argv`。switch 做的事情只是把已经在内存里的字再装回寄存器。于是让 body 直接收
`argv` 指针、按偏移把自己的参数读出来:

- `body_signature`(`aot/codegen/src/clif.rs`)给 try body 一个 `(i64) -> ()` 的签名。
  哪些函数是 try body 不是记下来的,是**从 `Inst::TryRegionCall` 反推的**——
  成为 try body 的唯一条件就是有人这样调它,所以没有第二处可以对不上。
  同一个函数如果还被普通 `CallFn` 调用,两处签名必然有一处是错的,直接报错。
- 每个参数正好一个机器字(carrier 在 `function.rs` 里已经拆成两个 `I64`,`F64` 声明成
  `I64`),这一点在 `body_signature` 里**检查**而不是假定:缓冲没有办法表示"这一格是两个字"。
- 读出来一律按 `i64` 读,再按声明类型收窄(`Bool` 在 Cranelift 里是 `i8`)。
  按声明宽度直接读会依赖机器把哪一端放在前面。
- switch、`LK_TRY_MAX_ARGS`(两份)、以及 trampoline 的 `argc` 参数一起删掉。

结果:同一份 121 程序语料 **121/121 原生化**;一个跨 8 个输入 + 多个 cell + 出口通道的
区域,body 声明了 **29 个参数**,和解释器逐行一致。50 个种子 × 5 个程序 0 分歧,
覆盖率门禁 65/65,61 程序 sweep、五个种子的生成式差分、
`cargo test --workspace --all-features` 全绿。VM 不受影响(改动只在 `aot/` 和 `lkrt/`)。

顺带被这条检查拦住的一类:参数类型是 `Nil` 的 body。`signature_of` 会给它 0 个 Cranelift
参数,而调用方照样推一个字进缓冲——以前是静默错位,现在直接拒绝。

## §50 生成式差分自己数不出原生化条数(2026-08-19)

`cli/tests/aot_fuzz_differential_test.rs` 的下限断言是
`compared * 4 >= cases`——而 `compared` 数的是**编译成功**的程序。
回退到 Tier 1 桥或 Tier 0 VM bundle 的程序照样编译成功、照样跑、照样和解释器一致,
所以这条断言在"每一个生成的程序都退回 VM 上跑"的情况下**一动不动**。
差分比对按定义看不见降级回归:两边答案本来就该一样。

现在多数一个数:stderr 里既没有 `Tier 1 hybrid` 也没有 `falling back` 的,
才算完全原生化。打印出来,并加第二条下限 `fully_native * 5 >= compared`。
下限取五分之一,远低于实测(六个种子上是 40 里的 13–19):生成器本来就会故意产出
不可降级的 hybrid helper,真实比例是生成器的属性而不是门禁;要拦的是塌方,
塌方会掉到接近零,而不是缓慢漂移。

同一轮里给生成器加了 §48 那个形状:`try` 体里的 `break` / `continue` 属于外层循环,
循环种类(`for` 区间 / `while`)是抽的——`continue` 在两种循环里落点不同,
`while` 是往回跳到条件,而第一版实现正好把这一种写错了。

## §51 "raise 前必须放掉 runtime borrow"这条规矩,现在自己会说话(2026-08-19)

`lkrt/src/panic.rs` 顶上写着一条硬规矩:raise 会 `_longjmp` 过所有 Rust 帧,
所以**不能在 `with_runtime` 的借用还活着的时候 raise** —— `RefMut` 不会析构,
借用标志一直是置位的。

逐个查了一遍,今天这条规矩是成立的,而且成立的方式很整齐:

| 位置 | 怎么保证的 |
| --- | --- |
| `net.rs` / `host.rs` / `io.rs` / `encoding.rs` 等 | 全部包在 `abi::raising` / `status` 里:闭包返回 `Result`,`?` 是普通返回,析构照跑,raise 发生在闭包**外面** |
| `chan.rs` | 直接调 `raise_str`,但每处前面都有显式 `drop(state)`;`own()`(会 raise)在取锁**之前**调用 |
| `lkdyn.rs` 的 41 处 raise | `with_obj_type_marks` / `with_struct_types` 的闭包全是纯 map 操作,raise 都在闭包返回之后 |
| `panic.rs` 自己 | 只碰自己的 `RefCell` |

问题不在有没有错,而在**这条规矩只由"读代码时小心"来保证**。违反了也不会当场炸:
炸的是**下一次** runtime 操作,可能在任意远的地方,报 "already mutably borrowed",
读起来像是那段无关代码的 bug。

所以 `raise_current` 现在先问一句 `state::runtime_borrow_is_live()`,是就打印
"这是 lkrt 的 bug,raise 的那个入口必须先放掉 runtime borrow" 然后 abort。
代价可以忽略:raise 路径本来就在做 `CString` 分配和一次 `longjmp`。

配一个测试(`a_live_runtime_borrow_is_visible_to_the_raise_path`),因为这个谓词唯一
可能的失效方式就是恒返回 `false` —— 那样它不报错、不碍事、也不再是检查。
真正要拦的那件事没法从测试里触发(按设计它会 abort 整个进程)。

## §52 优化过的 `lk` 从来没被任何门禁跑过,里面有两个 bug(2026-08-19)

起因是想给 channel 做个基准,顺手用 `--profile dist` 的 `lk` 编了个 `try` 程序。
它没编出来 —— **编译器自己 panic 了**。

### 一:`debug_assert_eq!` 里藏着副作用

```rust
debug_assert_eq!(body_index, sig.push_function(Vec::new(), Ty::Nil));
```

release 构建里 `debug_assert_eq!` 丢掉的是**整个表达式**,`push_function` 那次调用
一起没了。于是签名表不为 try body 长出那一行,下一趟
`sig.ret_types[body_index]` 越界 panic。

也就是说:**任何含 `try` 的程序,在任何优化构建的 `lk` 上都编不过**,一直如此。
debug 构建是好的 —— 而所有门禁用的都是 debug 构建。

顺手把全仓 20 处 `debug_assert*` 逐个看了一遍,其余全是纯谓词,没有第二处。

### 二:刷新的是 debug 归档,链接的是旁边那个

改完 panic,生成的可执行文件死在 `SIGILL`。

`native_executable.rs` 里 `lkrt_staticlib_path()` 先无条件 `build_lkrt_staticlib()`
再搜索。注释写得很清楚,这次无条件重建是为了防止链到**陈旧**的归档
(工具链换过之后两份 libstd 撞 `rust_eh_personality`)。但重建写死了
`cargo build -p lkrt-cabi`(dev)和 `target/debug`,而搜索取的是**这个二进制自己
所在目录**旁边的那份。于是对 `--release` / `--profile dist` 的 `lk` 来说,
它刷新的归档不是它链接的归档 —— 那件它要防的事情正好发生了:
dist 构建链到了前一天的 lkrt,里面 `lkrt_rt_try_region` 还是三参数带元数 switch 的旧签名
(见 §49),`__builtin_trap()` 就是那声 `SIGILL`。

改成按**自己所在目录对应的 profile** 重建(`cargo_profile_of`:`deps` 上跳一级,
`debug` 目录对应 `dev` profile,其余同名),并且那句
"building lkrt staticlib (one-time)…" 现在报的是真的 profile,也不再撒谎说"一次性"。

### 门禁

`check.yml` 新增一步:`cargo build --release -p lk-cli --features aot`,
用它跑同一份覆盖率门禁(65/65),再把 `try_catch.lk` 和 `concurrency_demo.lk`
的解释器输出与原生输出逐字节比一遍。

用 release 不用 dist:dist 多的是全量 LTO,为的是这里不测的性能数字,却要多花几分钟;
真正会**静默**改变行为的两件事 —— `debug_assertions` 关掉、优化打开 —— 两个 profile 是一样的。

## §53 一个没有 `select` 的 channel 程序,一半时间花在 select 的唤醒上(2026-08-19)

`send`/`recv` 各 300k 次的原生程序,`perf` 的结果:

| | 占比 |
| --- | --- |
| `syscall`(futex) | 40% |
| `lkrt::chan::notify_selects` | 18% |
| `lkrt::chan::channel` | 12% |
| `SipHash`(registry 的默认 hasher) | 2% |

程序里**一个 `select` 都没有**。三处都改了,同一个基准 0.14s → 0.016s(每次操作
230ns → 26ns,**8.75×**)。

### 一:没有 select 在等的时候不广播

`notify_selects()` 每次 send/recv 都取一把进程全局互斥锁、加一、`notify_all`。
现在先读一个 `BLOCKED_SELECTS: AtomicUsize`,为零直接返回。

跳过是安全的,靠的是顺序:通知方走到这里时**已经**放开了自己 channel 的 `state` 锁,
改动已经发布;而 `select` 是在**轮询之前**就把计数加上去的。所以如果这里读到零,
那次加一在 `SeqCst` 全序里排在后面,它之后的轮询要取 channel 锁,
就一定看得到刚才那次改动。要么通知方叫醒它,要么它根本不会睡。

代价是 `select` 里的 raise 会跳过 RAII 的减一(longjmp 不跑析构)。所以顺手把
`select$block` 里所有会 raise 的事情——arm 形状校验、`own` 深拷贝、以及
`channel(id)` 解析——全挪到轮询循环**之前**的预处理里,循环里只剩一处
"send on closed channel",那一处显式 `drop(parked)`。
把 channel 解析提前还顺带把全局 registry 锁从轮询循环里拿掉了(原先每轮每条 arm 一次)。

### 二:没人在等的时候不 futex

`Condvar::notify_one` 在 Linux 上无论有没有人 park 都会发 `futex_wake`。
`ChanState` 现在带 `recv_waiters` / `send_waiters`,只在锁里读写,所以是精确的:
等待方在 `wait` 释放锁**之前**加一,通知方持锁读到零就意味着没人在等、
也没人能在不经过这把锁的情况下开始等。

### 三:registry 是个稠密整数表,不该是 HashMap

id 来自一次 `fetch_add`,而且从不删除(channel 活到进程结束,和 lkrt 其余部分一样)。
`Mutex<HashMap<i64, _>>` + 默认 hasher 换成 `RwLock<Vec<Option<Arc<ChanInner>>>>`,
按 `id - 1` 索引:一次读锁 + 一次边界检查。`Option` 是因为 id 在插入取锁之前就发出去了,
两个线程同时建 channel 可能乱序到达。

### 语料补了一条:真的会 park 的 `select`

`examples/syntax/select.lk` 里此前每一个 `select` 都有一条**已经就绪**的 arm,
所以从来没有真正阻塞过——而阻塞正是上面这三条改动唯一会出错的地方,
出错的形式是**挂住**,不是答错。现在加了两个 producer 对着无缓冲 channel 喂 50 轮、
主线程用无 default 的 `select` 收 100 次:总数确定,交错不确定。
另外本地跑了多生产者/多消费者压测(无缓冲 + 小缓冲 + close 唤醒阻塞接收方)
原生 20 次、阻塞 `select` 20 次,无挂起无错答。

## §54 `try` 在闭包里读不到闭包捕获的东西(2026-08-19)

写 §53 的压测时随手写了 `spawn(|| { try { got = recv(c); } catch e { … } })`,
它不原生化。缩下来是这么一条:

```lk
fn outer(k: Int) -> Int {
    let f = || { let got = 0; try { got = k + 1; } catch e { got = 0 - 1; } return got; };
    return f();
}
```

`try` 的 body 被外联成一个自己的函数,而 `outline` 给它 `capture_count == 0`。
body 里的 `LoadCapture 0` 于是去查 body 自己的 capture 列表——那里面只有区域的
cell 输入。要么查不到(报 `BadConst`,今天就是这样),要么 cell 输入够多、
**查到另一个**。也就是说这条不只是覆盖缺口,它还是一条潜在的静默错答。

由于捕获正是闭包的意义所在,这条实际上等于:**`try` 写在任何有捕获的闭包里都不原生化**。
`spawn(|| { try { … } })` —— 一个自己处理错误的 goroutine —— 正好是最常见的写法。

### 改法

body 的 capture 列表**就是**外层函数的:在 `fn_params` 里(输出 cell 之后、出口通道
之前)按位置声明外层的每一个捕获,并让它们占据 capture 索引 `0..n`,
后面才是这个区域自己的 cell 输入。索引 `k` 必须还是索引 `k`,所以是**按位置、
无条件**传的:一个静态已知的捕获也占一格,传一个死的字——和普通调用点上
`ClosureCapture::StaticRef` 已经在做的交易一样。同时把外层的 `ref_captures`
和 cell 内容类型按新索引复制过去,这样 body 里对捕获变量的算术还是有类型的,
不会退回 `Dyn`。

两种宽度都支持:一个字直接传;两寄存器的 carrier(`Dyn` / `Maybe`)拆成两个字、
在 body 入口拼回去——和区域输入用的是同一套 `entry_carriers`。

一条明确的拒绝:**外层是 goroutine body 时不做**。那里捕获的当前值在线程私有的槽里,
不在参数里,把参数传下去会传成 spawn 那一刻的值,把之后所有写入都藏起来。

### 数

生成 60 个"闭包捕获 1..4 个变量(Int / String / List),`try` 里读它们"的程序:

| | 原生化 | 分歧 |
| --- | --- | --- |
| 改之前 | **0 / 60** | — |
| 改之后 | **60 / 60** | 0 |

第一版漏了一件事,是 §49 那条元数检查当场抓住的:`Dyn` 捕获是两个字,
`body_signature` 报 "a try body parameter is wider than a machine word"。
那条检查写下来不到一天就付清了。

### 还没做的

body 里**写**捕获变量(`StoreCellVal` 打到一个按值传的捕获参数上)仍然拒绝,
消息是既有的 "no write-back path"。按值捕获本来就没有回写路径,
要做就得让区域的写入反过来把外层的捕获提升成 cell(`require_cell_capture`
现在只对函数自己的 body 起作用)。是拒绝而不是错答,单独一轮做。

## §55 越界读出来的 nil,编译后会炸(2026-08-19)

顺着 §54 往下查一条"闭包捕获的容器进 `try` 被拒",落到 `to_dyn` 拒绝 `MaybeI64`。
拒绝本身只是覆盖缺口,但顺手写的差分语料把一件更严重的事情翻了出来:

```lk
let xs = [3, 1, 4];
let out = [];
out.push(xs[9]);        // 解释器:塞进一个 nil。原生:Error: runtime error
```

**同一个程序,解释器跑完,编译版本死掉。**四种落点,四条都是:

| 写法 | 解释器 | 原生(改之前) |
| --- | --- | --- |
| `out.push(xs[oob])` | 追加 nil | raise |
| `m[k] = xs[oob]` | 存 nil | raise |
| `xs[oob] == 4` / `!= 4` | `false` / `true` | raise |
| `"[" + xs[oob] + "]"` | `[nil]` | raise |

原因是同一个:这些位置都用 `read_scalar` 读值,而 `read_scalar` 会把
"可能是 nil"**断言成**"一定在",不在就 raise。那在**要一个数**的地方是对的
(解释器那里也报错),在别的地方全是错的 —— 列表、映射、相等比较、字符串拼接
都收得下 nil,解释器也确实收下了。

静态上什么都看不出来:`Maybe<Int>` 窄化成 `Int`,而 `Int` 正是载体想要的类型。

### 改法

1. `to_dyn` 学会可空载体。此前它和 `to_dyn_any` 是两个函数,区别只有"拒不拒绝载体",
   而除了调用参数的编组,所有地方都拿的是**拒绝**的那个。两者已合并成一个:
   装箱本来就是"这个值在 VM 里是什么"的那一刻,而一个载体在 VM 里就是 nil 或者负载。
2. `ListDyn` 的 push 和 `MapStrDyn` 的存,改成读原始值再装箱,不再断言。
3. 相等比较:任一侧是可空载体时按 nil 比。`(Maybe<Int>, Int)` 这条常见形状走
   `present AND 相等`,多一条 `and` 而不是两次装箱加一次调用 ——
   `xs[i] == k` 在搜索循环里就是这个形状。其余装箱交给 `dyn.eq`。
   **只管相等**:有序比较对 nil 在解释器里也是错误,断言在场失败的是同一批程序。
4. 字符串拼接:`Str + 可空载体` 并入既有的 `Str + Dyn` 分支,`dyn.add` 按 VM 的
   第四条规则显示拼接,缺席就是 `nil`。
5. **类型化容器**是唯一真收不下 nil 的地方。往 `List<Int>` / `Map<String, Int>` 里
   存一个可空值,现在报成"这个载体的字面量被推翻了"——就是既有的定点机制,
   下一趟会把它重建成 Dyn 载体。此前是静默接受、运行时 raise。

### 数

生成 80 个"越界元素落到各种位置"的程序(present 与 absent 两种上界都跑):
改之前 13 条原生化里 **13 条分歧**;改之后 32 条原生化,**0 分歧**。
覆盖率门禁 65/65、61 程序 sweep、五个种子的生成式差分、
`cargo test --workspace --all-features`、以及 §48/§54 两份语料全绿。

语料补在 `examples/syntax/null_coalescing.lk`:四种落点各一条,
外加一条"算术上确实该报错"的对照。这一类此前**一条覆盖都没有**。

### 还没做的:报错文字

用 nil 做算术时两边都 raise,但话不一样:解释器说
`Add expected numbers or strings, got Nil and Int`,原生说 `runtime error`
(`lkrt_maybe_i64_unwrap`)。同一个程序 `try { xs[9] + 1 } catch e { e }`
在两个后端拿到两个字符串。

`lkrt_dyn_add` 那一处已经修好(它是这一族里唯一还写着 "runtime type error" 的,
`sub`/`mul`/`div`/`mod` 早就用 `binary_type_error` 指名操作数了)。
但真正走到的是 `read_scalar` 的断言,而 `read_scalar` 有 **94 个调用点**,
都不知道自己在为哪个运算符读值。要按 VM 的逐运算符措辞对上,得把上下文串下去 ——
单独一轮做,不在这一轮顺手改。

## §56 被 catch 到的那句话,两个后端终于一样(2026-08-19)

§55 结尾记了一条没做的:nil 参与算术时两边都 raise,但话不一样 ——
解释器说 `Add expected numbers or strings, got Nil and Int`,原生说 `runtime error`。
当时的判断是"`read_scalar` 有 94 个调用点,把上下文串下去代价太大"。

那个判断错在**串错了东西**。要串的不是上下文,是**那句话本身**,
而且只要串到真正知道它的那几个地方。

### 做法

- lkrt 多一个入口:`lkrt_rt_maybe_guard(present, message)`,`present == 0` 就 raise
  `message`。**一个**入口,不分类型 —— 它只看那一位,值由 `maybe.value` 自己取。
- `convert::read_scalar_saying(..., message)`:可空载体走 guard + `maybe.value`,
  其余类型原样交给 `read_scalar`。94 个调用点一个没动。
- `inst/scalar.rs` 的算术与有序比较两处,在读出**两侧的声明类型之后**构造那句话,
  作为常量 intern 进去。运算符和两个操作数的类型名在那里都是静态已知的,
  而这正是解释器那句话里的全部内容 —— 所以 lkrt 里**没有**一张消息表可以和执行器漂移。
- 两侧**都**可空是静态句子唯一给不对的情况(第二个操作数在不在只有运行时知道),
  那一种装箱交给 `dyn.*` 按值格式化。

措辞是**逐个探出来的**,不是从源码读的:5 个算术运算符 × 4 个有序比较 × 左右两侧,
共 18 条,逐条对着解释器跑。三种句式:`+` 和 `-` 各自说自己收什么,其余共用一句。

### 结果

18 条逐字节一致。`try { xs[9] + 1 } catch e { e }` 两边都是
`Add expected numbers or strings, got Nil and Int` —— 这是个**值**,不是诊断细节。

`examples/syntax/null_coalescing.lk` 里加了 9 条断言(含两侧都 nil 的那条),
所以这条一致性此后由差分门禁盯着。

### 顺带

- lkrt 里那句"这匹配 VM,VM 在 nil 参与算术时**halt**"是**假的** —— 解释器
  raise 的是一个可以被 catch 的值。注释已改,并指向新的 guard。
- **没有性能代价**,量过:同一 profile 的两个二进制,8000 万次
  `for x in xs { s = s + x }`,改前 0.17s,改后 0.17s。guard 是"看一位、冷调用",
  和原先的 unwrap 一样;值由纯 extract 取出。

### 顺手补完的落点

§55 那一族又扫了两轮(每种落点单独一个程序,VM 与严格原生逐字节比):
`out[0] = xs[越界]`(Dyn 列表的**下标赋值**,push 修了它没修)是第五条错答,已修;
`Map<Int,Int>` / `Map<Int,Float>` / `List<Int>` 的下标赋值补上了载体推翻检查。
现在两轮扫描 30 种落点,0 分歧,剩下的全是 NOLOWER(拒绝,安全):
列表字面量里的 nil、`insert`、`contains`、`join`、`Map<Int,_>` 的存。

## §57 nil 也要能被**构造**出来,不只是被存进去(2026-08-19)

§56 结尾列的五条 NOLOWER,逐条用"同一个落点、值不可空 / 值可空"两个程序对照,
结论一致:**五条全都是"不可空就降级,可空就拒绝"**,也就是同一个根因的第五、六、七个分支。

- **列表字面量**(`[xs[9], 1]`):Dyn 列表那条路有一张**元素类型白名单**,
  上面没有可空载体。这张表旁边就写着一条注释,记的是上一次同样的错误
  ——"漏一个元素类型在这里是错答,不是回退"(当时漏的是类型化 map)。
  这次漏的是四个可空载体。加上去,`join` 那条一起好了(它就是走字面量)。
- **`insert`**:那条 arm 的值参数类型不受限,类型化列表分支把载体**原样**递给
  `i64_insert`。拦住它的是 ABI 里的参数个数不匹配 —— 拦是拦住了,但报的不是问题本身。
  改成报"载体字面量被推翻",这既是更好的话,也是定点重建成 Dyn 列表的信号:
  于是这条从**拒绝变成了降级**。`push` 早就这么做了,`insert` 是同一个存,晚一个方法。

同一份 80 程序语料,原生化 **32 → 80**(全部),分歧仍然 0。

### 剩下的,以及为什么剩下

- `contains` / `count` / `index_of` 在**类型化**列表上,arm 要求精确类型,
  可空 needle 不匹配 → 拒绝。这一条其实有个更好的答案:`List<Int>` 里永远
  不可能有 nil,所以"缺席就是 false",不必把列表重建成 Dyn。留着。
- **`Map<Int, _>` 存非同类值根本没有载体**。`{"a": 1, "b": "s"}` 能降级(`MapStrDyn`),
  `{1: "a", 2: 3}` 不能 —— 载体集合里有 `MapStrDyn` 却没有 `MapI64Dyn`,
  这个不对称是任意的,而 `{1: "a"}` 是一句普通的程序。
  lkrt 那边 `StrDynMap = FxMap<String, LkDyn>` 是真的只认字符串键,
  所以补这一条要新增一个运行时 map 类型加一整套 ABI 入口 —— 是个单独的活。

## §58 空的 `catch` 让"body 每条路都离开"这个判断变成假的(2026-08-19)

把 `defer` 和控制流的组合逐个探了一遍(8 种形状,VM 与严格原生逐字节比),
出来一条错答。缩下来 `defer` 根本没参与:

```lk
fn f(n: Int) -> Int { try { if n > 0 { return 7; } } catch e { } return 5; }
f(0)   // 解释器 5,原生 Error: runtime type error
```

`f(1)`(走 return 那条)两边都对,错的是**没有** return 的那条。

MIR 里看得很清楚:检查块算出了结果码,然后 `br` 到 return 块 —— **一次都没测**。
于是 `dyn.as_i64` 去读那个还是 nil 的值 cell。

原因是 `always_leaves = region_handler == region_fallthrough`。这个条件真正检测的是
"编译器没有发出跳过 handler 的那条 `Jmp`",而**handler 是空的**时候也没有 ——
跳过空的东西没有东西可发。两件事被读成了一件。

改法是把这个特判整个删掉:**结果码永远测**。body 真的每条路都离开时,码 0 不会出现,
那条假边不可达 —— 合法,而且是死的。代价是一次比较,发生在一个刚从调用返回的位置上。

### 为什么生成式差分没抓到

它生成的每一个 `catch` 体里都有语句。已经补了一条"空 handler + 只有一条路 return"的形状
(两个上界都跑:return 的那条和不 return 的那条,后者才是错的),
语料也在 `examples/syntax/try_catch.lk` 里留了一份。

### 同一轮的其它探测

- **数值与浮点边界** 22 条(i64 最小值、回绕、除零、负数取模、NaN、-0、
  1e-320、大整数与浮点的字符串化、移位、按位取反):全部一致。
- **字符串与 Unicode** 24 条(多字节索引/切片/反转、emoji、`straße`.upper()、
  负下标、越界、空分隔符、`\u{a0}` 的 trim、Unicode 比较):全部一致。

这两族没有发现问题,记在这里是为了下次不用再探一遍。

## §59 最后一趟的可重试发现,第三种漏了(2026-08-20)

上面那条类型错误修好之后,`fn f(v) { … } f([]); f(5);` 还是不原生化,报
"empty list literal(s) at pc [4] were mis-guessed (retried as Dyn)" ——
一句**定点内部的话**漏到了用户面前,而且重试并没有发生。

`lower_bundled` 结尾那段注释已经把道理写清楚了:定点收敛 → `refine_signatures` 跑一次 →
最后一趟按细化后的签名降级,而**只有在最后一趟才出现**的发现没有下一趟可去,
所以那里专门收集了一批可重试的失败再走一遍。收集的名单上有
`DynLoopPhi` 和 `ParamCarrierContradicted`,**没有** `LiteralElemTypeContradicted`
(也没有 `PhiProvenance`)。

空的 `[]` 的元素类型是靠"往后找第一次 push"猜的;函数里根本没有 push 时没有任何证据,
猜的就是默认载体。而"这个参数是 Dyn"正是 `refine_signatures` 才定下来的,
于是矛盾**只在最后一趟可见**,而那里没人听。

两种都补进名单了。`f([]); f(5)` 现在原生化,和解释器一致。

顺带记一条**诊断本身的毛病**:`carrier_contradicted` 在句柄不是已知字面量时,
会把 `literal_carrier` 里**所有**同形状的 pc 都报出来 —— 所以 `f([1]); f(5)`
也报"empty list literal",尽管那里根本没有空字面量。这条没改,
它只是把话说得不准,不影响重试(重试的是同一批 pc)。

## §60 在装不下 nil 的容器里找 nil(2026-08-20)

§57 结尾列的第一条:`contains` / `count` / `index_of` 在**类型化**列表上遇到可空
needle 就拒绝降级。类型化的那些 arm 都精确匹配 needle 的类型,`Maybe<Int>` 一条也不匹配。

当时记的判断是对的:`List<Int>` 里永远不可能有 nil,所以"缺席就是没找到",
**不必**把列表重建成 Dyn 载体 —— 接收者没有问题,重建它会把一次查找变成一次分配。

做法是在方法分发的入口拦一次:把可空 needle 拆成 `present` 和 `value`,
拿 `value` 走原来的类型化 arm,再按方法的"没找到"是什么去 `select`:
`contains` 是 `false`,`count` 是 `0`,`index_of` 是 nil。
缺席那条路上 `value` 是没人写过的值,所以它的答案是被**丢掉**的,不是被相信的。

`Inst::Select` 本来就按分量选(Cranelift 没有聚合 select),所以 `index_of` 返回的
`Dyn` 载体和标量走同一条路,三个方法一套代码。

八种组合(`List<Int>` / `List<String>` / `List<Float>` / `Set` / Dyn 列表 × 三个方法)
逐个对着解释器跑,全部一致;语料进了 `examples/syntax/null_coalescing.lk`。

## §61 只读一个容器参数,却被判成写(2026-08-20)

`bare-metal-x86` 在这个分支上编不出来,两条独立的原因,都早于本轮:

### 一:`lkrt` 不带 `std` 编不过

`lkdyn.rs` 无条件写着 `crate::lkclosure::…`,而 `lkclosure` 是 `#[cfg(feature = "std")]` 的
(闭包值走 channel 的深拷贝模型,那个模型和线程在一起)。x86 内核链的是不带 std 的 `lkrt`,
于是整个 crate 编不过。四处引用(渲染一个闭包,以及 `map`/`filter`/`reduce` 的闭包回调)
连同它们在 `lib.rs` 的重导出一起加了 cfg —— 没有 `lkclosure` 就不可能存在闭包值,
这几处本来就不可达。

### 二:`drivers/text` 被判成"写过容器参数"

那个文件里就写着:"`font` is only ever read, which is what keeps this module bundlable"。
判定它的是 `module_may_mutate_a_parameter` —— 一个刻意过近似的污点分析。
过近似的代价按注释的说法是"白白不打包一个模块",但 `compile object:` **没有回退**,
所以代价其实是**构建失败**。

两处过近似:

- **通过寄存器的间接调用**把每个被污染的实参都判成"被留住了"。而 LK 的运算符是 desugar
  成调用的:`(bits >> shift) & 1` 就是 `__lk_shr` 和 `__lk_bit_and` 两次调用。于是
  "从容器参数里读出一个元素再移位"看起来和"交给一个不认识的函数"一模一样。
  现在:如果被调寄存器能被指名、而且指的是一个证明只读的内置(全部运算符 desugar,
  加上 `typeof` / `assert*` / `print*` / `panic` / `error`),就不判。
  指名要沿着 `Move` 传递 —— 被调者是被 `Move` 进调用窗口基址的 —— 而且全局槽位要按
  编译器的 fact 解析,指令里的 `bx` 是执行器自己都不信的占位符。
- **污点从不失效**。它只在传播的地方(`Move` 和容器读)被移除,所以一个曾经装过
  "从参数里读出的元素"的寄存器,在之后被普通算术覆盖之后**仍然是脏的** ——
  而字节码复用寄存器很凶。现在一批"把刚算出来的值写进 `a`"的操作码会清掉它。

两处都往**安全**的方向列举,不做推断:漏判一次改写是错答,多判一次只是拒绝。
守卫本身的测试(`clif_differential_test` 里"写过容器参数的模块不打包")仍然绿。

### 还没通的

x86 内核现在过了打包这一关,停在下一个:`print_file` 里一处方法在那个接收者类型上
还没有原生降级。那是另一条,没在这一轮做。


## §62 装箱 map 按 `String` 索引,而 nil 和 Bool 也是键(2026-08-20)

解释器的 map 键有四种:`nil`、Bool、Int、String。原生这边没有任何表示装得下前两种 ——
装箱载体 `StrDynMap = FxMap<String, LkDyn>`,类型化载体按 `String` 或 `i64`。

**读**这一侧已经补齐:`lkrt_dyn_get` 对任意键种做查找,miss 给 nil,非键种交给
`key_from_dyn` 按解释器的措辞 raise。**写**这一侧补不了:`m[nil] = 1` 解释器给
`{nil:1}`,原生没有地方放。

现状(2026-08-20 起):**不再是错答,是覆盖缺口**。键的静态类型不是 `Str` / `I64`
时,降级层一律拒绝,整程序回落 VM —— 正确,慢。下面记的那些复现,今天两端一致。

### 范围比"nil 和 Bool"大(2026-08-20 复测)

不止空字面量的猜测,也不止 nil/Bool。任何**跨种类的键**写进类型化 map 载体**曾经**是错答,
因为运行期拆箱 `dyn.as_key_str` / `dyn.as_key_i64` 拒绝其他种类,而解释器不拒绝 ——
LK 的 map 同时收 nil / Bool / Int / String,载体是原生这边的表示选择,程序没要求过。

    fn put(m: Any, k: Any) { m[k] = 1; return m; }
    println(put({"a": 1}, "b"));   // 两端 {"a":1,"b":1}
    println(put({"a": 1}, 7));     // VM {"a":1,7:1} / 原生 raise "runtime type error"

第二行的 map 是**显式字面量**,没有任何猜测参与。默认设置(允许回退)下同样错。

顺带:开着 hybrid 时错法不同 —— 桥接返回一个非字符串键的 map 会
`bridged return kind not yet marshalable: map with non-string key Int(7)`,
同样是解释器答得出来的程序在原生二进制里中止。

### 拒绝这条路的实测代价(修正)

此前这里写着"拒绝擦除的键会把 fuzz 原生化比例从 12/60 打到 10/60"。**那是错的**,
是被一个不稳的下限断言骗了:同一个种子在**未改动的树**上就是 10/60。按 300 例重测,
加不加这条拒绝都是 **76/300**,fuzz 代价为零(下限断言本身已改成只在大样本上生效)。

真实代价只有一处,而且已经消掉:`examples/syntax/closure_value.lk` 里
`try { m[f] = 1; }` 故意拿一个闭包当键,期待运行期按解释器的措辞拒绝 —— 那一条
**降级是对的**(两端都 raise),不该被守卫拦下。闭包这个事实活在
`ssa.closure_values` 里,是按函数的 SSA 状态,过不了 region 边界。

现在它跨得过去了:`SigInfer::try_body_closure_inputs` 记 `(body, 寄存器)`,
写在实参编排处、读在 body 的形参播种处 —— 和 `try_body_param_tys` 同一条路。
闭包句柄是 `Dyn`,**走的是两字载体那条分支**,不是单字那条;只补单字那条时
守卫照样拦,而且例子的报错一模一样。

### 已修的部分(2026-08-20)

键的静态类型是 `Ty::Dyn`、且不是已知闭包时,写入拒绝降级,整程序回落 VM。
实测:coverage 66/66 不变,fuzz 原生化 76/300 与基线**逐个相等**。
差分语料 `a_map_key_of_any_kind_answers_or_declines` 钉住"两端一致"。

**仍未修**:这些形状现在是回退,不是原生。要让它们原生化,还是需要下面这个表示改动。

### 为什么没有改表示

`StrDynMap` 换成 `FxMap<RtKey, LkDyn>` 是唯一的正解,顺带把"装箱 map 是字符串键"
这个特例连同各处的 `str_key` / `key_from_dyn` 转换层一起删掉。迭代序不是障碍:
`FxMap` 是 `IndexMap`,顺序按插入,与键类型无关。

障碍是**查找要分配**。`map.get(&str_key(text))` 对超过 7 字节的键会构造
`RtKey::String(String)` —— 而这正是结构体字段读取那条路径,它此前专门优化过
(`str_dyn_get_at` 按位置读,注释记着一次哈希查找实测 ~107ns 是整个循环的开销)。
结构体字段名超过 7 字节很常见。

绕开分配需要一个借用形式的键(`Equivalent<RtKey> for &str`),它的 `Hash` 必须与
`RtKey::ShortStr` / `RtKey::String` 逐位一致,包括 7 字节那道分界。那是一份会在
沉默中漂移的副本 —— 正是 [[lkrt-mirror-drift]] 那一类。

### 第四次撞上它:hybrid 桥接会中止(2026-08-20)

`lk-api` 的 `marshal_map` 把 VM 的 map 搬成原生装箱 map,遇到非字符串键
`hybrid_die`。于是一个**桥接函数返回混合键 map** 的程序,在默认设置下直接中止:

    fn put(m: Any, k: Any) { m[k] = 1; return m; }   // 这个函数降级不了 → 走桥
    println(put({"a": 1}, 7));
    → lk hybrid bridge: bridged return kind not yet marshalable: map with non-string key Int(7)

写入那一侧已经靠"降级层拒绝"修掉了(见上),桥接这一侧修不掉:它是运行期性质,
而桥的资格是编译期定的,返回值又是动态的 —— 不可能静态排除"会返回 map 的函数"。
这条是**目前仅剩的错答**,而且默认可达。

### 实际动手之后的清单(2026-08-20,已回滚)

把 `StrDynMap` 换成 `FxMap<RtKey, LkDyn>` 试了一次,`cargo build -p lkrt` 报 26 处。
比预估的"换个类型"大,因为**共享的辅助件按 `String` 写死**:

- `set_str_key<V>(map: &mut FxMap<String, V>, ...)` —— 同时服务 str_i64 / str_f64 /
  str_bool / str_dyn 四个载体,装箱那个要拆出自己的 setter。
- `map_iter_family!` 宏 —— 四次实例化共用,里面 `boxed_str_key(k: &String)`,
  装箱载体需要按键种装箱的版本。
- `boxed_map_keyed` / `str_dyn_map_mirrored` 的两段式镜像,以及 `chan.rs` 四处、
  `vm_mirror.rs` 两处。
- 显示要按键种渲染(`{nil:1,true:2}`),不能再直接打字符串。

**先量再改**:换类型之后 `map.get(text)` 变成 `map.get(&str_key(text))`,超过 7 字节
的键每次查找会构造 `RtKey::String(String)`。要不要做借用键(`Equivalent<RtKey> for
&str`,附 hash 一致性测试)**应该由性能门禁回答**,而不是预先假设 —— 结构体字段读取
走的是 `str_dyn_get_at` 的按位置路径,根本不查哈希,所以这条分配可能无关紧要。
先做朴素版本、跑 `bench/run_workload_bench.sh`,再决定要不要那份镜像。

规模与此前记下的 `MapI64Dyn` 载体相当。

## §63 原生这边换了表示的值,有四种办法被看出来(2026-08-20)

降级层有两处把值换成别的东西:**流物化成列表**(有限来源 + 纯 lambda),
**channel / task 表示成 `i64` 句柄**。两处的理由都是"看不出区别"。四种办法能看出来:

| 办法 | 解释器 | 换表示之后 |
| --- | --- | --- |
| `typeof(v)` | `Stream` / `Channel` | `List` / `Int` |
| `println(v)` | `<Stream>` / `<Channel>` | `[0,1,2]` / `1` |
| `v == 1` | `false` | **`true`** |
| trait 分派 | `impl … for Stream` | `impl … for List` |

第三行是最糟的:**一个 channel 等于整数 1**。

前三种已经拦住(`Ssa::disguised_values`):`typeof`、显示、以及"一边有标记另一边没有"
的比较,都拒绝降级而不是照表示回答。trait 分派同此。

### 逃逸只对流拦

装箱进容器、跨到类型化参数、被返回 —— 这三种都会把标记丢掉,把裸表示交给会照它
回答的代码。对流拦了(`Ssa::escape_is_visible`),实测代价为零。对句柄**没拦**:
channel 天天被传进函数、被塞进 `select` 的列表,拦下来当场让两个例子不再原生化
(`concurrency_demo.lk`、`select.lk`)。

**残留**:`println([c])` 打印的是 `[1]` 而不是 `[<Channel>]`。

### `stream.from_list` 标不了

`from_list` 和 `collect` 都是**值直通**:结果与入参共用同一个 SSA 值。所以标记
`from_list` 的结果等于同时标记了调用方的那个列表 —— `println(xs)` 会被拒;标记
`collect` 的结果等于标记了一个**确实是列表**的东西 —— `collected == [...]` 会被拒。
两个都试过,两个都当场把 `stream_demo.lk` 或它的比较打掉。

标记一个直通值需要它有自己的值可标,而 MIR 没有 copy 指令能造一个。
`stream.range` 构造新句柄,标了。

### 正解,以及句柄那一半已经做了(2026-08-20)

两处都是"表示不同"而不是"跟踪不到位"。channel / task 现在**带自己的 tag**
(`DYN_CHAN` / `DYN_TASK`)而不是裸 `i64` id,四种观察方式自然都对了,逃逸也不再丢
信息 —— 跟踪代码随之删掉。`println([c])` 现在打 `[<Channel>]`。

改动的清单是**上一次加 tag 时的足迹**:`rg DYN_CLOSURE` 列出五处必须学会新 tag 的
地方(`kind_name`、`dyn_eq_at`、`display_into`、排序 rank、channel 深拷贝),外加
trait 分派码。哪一处漏了都是沉默的错答,而这份清单是现成的。

顺带修好一处**解释器自相矛盾**:一个 channel 穿过 channel 之后,`==` 说它不是原来
那个,而往它 `send` 又会到达原来那个。堆对象比的是句柄,而 channel 的身份是 id。
现在按 id 比。

**流那一半也做了**(同日):`DYN_STREAM`,payload 是物化出来的 dyn-list 句柄。
四种观察方式全对,`stream.from_list(xs)` 也不再把 `xs` 一起标上 —— 因为**箱子本身
就是一个独立的值**,这正是"标记直通值"做不到的那件事。

`stream.collect` 取出里面的列表,`stream.map` / `filter` / `take` / `skip` / `chain`
取出、操作、再装回去;`chain` 的第二个参数也是流,一并拆。

代价与收益都量了:跟踪那套守卫此前让 trait-位置扫描里 7/35 回退,现在
**35/35 既一致又原生**。跟踪代码全部删除。

## §64 结构体实例和 map 共用一个载体,"不知道"必须当成"可能是结构体"(2026-08-21)

`NewObject` 编译成 `Map<str, Dyn>`,所以一个 `MapStrDyn` 字要么是 map,要么是
结构体实例,MIR 类型区分不了。解释器那边是两个不同的堆值,map 的**集合**操作
落在结构体上一律 raise。原生这边按载体作答:

| 操作 | 解释器 | 原生(修复前) |
| --- | --- | --- |
| `v.len()` | ``E `len()` has no answer for P`` | `ok 2` |
| `v.is_empty()` | `E P has no method 'is_empty'` | `ok false` |
| `v.keys()` | `E P has no method 'keys'` | `["ok ","p","q"]` |
| `"p" in v` | `E Contains haystack object is not searchable: "P"` | `ok true` |

类型检查器挡住了写明类型的接收者,所以这四条只在**擦除**位置出现:`fn f(v: Any)`,
或者两个调用点分别传结构体和 map 让参数事实归零的时候。§62 里的 `p.m["b"] = 2`
是同一类:两个后端一起答错,因为它们共用前端。

修复分两层。装箱那层(`Ty::Dyn` 接收者)由运行时判定:`dyn.len_of`、
`dyn.contains`、`dyn.map_pairs`/`keys`/`values`、`dyn.map_has`、`dyn.map_delete`
读 arena 类型标记,标记非零就按解释器的原话 raise。非 map 的 tag 一次比较就返回,
只有装箱 map 付一次查表。

有类型那层不能用运行时判定:`map_h.str_dyn_len`/`str_dyn_has` 是热路径,
每次调用加一次带锁查表不可接受。改成编译期证明,而且是**正向**的:
`ssa.struct_facts` 从 `HashMap<ValueId, String>` 变成
`HashMap<ValueId, StructFact>`,`StructFact` 只有 `Struct(name)` 和 `PlainMap`
两个成员,**没有 Unknown**——不在表里就是不知道,集合操作在不知道的地方拒绝下降。
方向选反了就是错答:如果"缺省 = 普通 map",漏记一处事实就多一个错答;现在漏记
一处只是多一次回退。

事实的传播点就是原来那张表的足迹:`seed_provenance` / `inherit_provenance` /
`verify_seeded_provenance` 三处(phi 按"每条非自环边都一致"合并,`Struct(P)` 与
`PlainMap` 不一致就归零),跨函数的 `param_structs` / `ret_structs`,跨 `try` 区域的
`try_body_struct_inputs`。`PlainMap` 目前在三个 map 字面量构造点写入。

把剩下的观察点按同一张表走了一遍——相等、显示、`typeof`、下标读写、`get`、
`values`、`clear`、`delete`、装进列表——只多出一处:`for k in v` 在原生这边
按 map 迭代,把字段当成 pair 发给循环,解释器那边是
`ToIter target object is not iterable: "P"`。两层各补一处:`ToIter` 的
`MapStrDyn` 分支要 `PlainMap` 证明,`dyn.to_iter` 读类型标记。

覆盖率没有掉:门禁 71/71,VM/原生扫描 73 一致 1 允许分歧,300 例模糊测试通过。
`a_map_that_is_one_still_lowers_its_collection_methods` 钉住反面——参数位置和
循环头上的普通 map 仍然全原生下降。

## §65 结构体的身份存在别的线程读不到的地方(2026-08-21)

`typeof(p)`、`println(p)`、trait 派发、字段声明检查,原生这边都靠两张表:
`id → 名字/字段` 的注册表(生成的入口序言写一次)和 `句柄 → id` 的标记表
(每次构造写一条)。两张都是 `thread_local!`。任务跑在别的线程上,两张都读不到:

| 程序 | 解释器 | 原生(修复前) |
| --- | --- | --- |
| 结构体经 channel 送进 task,在里面 `typeof` | `P` | `Map` |
| 同上,`"" + v` | `P{p:1,q:2}` | `{"p":1,"q":2}` |

三处改动:

1. **标记进句柄**。`StrDynMap` 从 `FxMap<String, LkDyn>` 的别名变成一个带
   `type_id` 字段的结构体(`Deref`/`DerefMut` 到原来的 map,所以调用点不动)。
   标记表整张删掉——顺带把 `typeof`、派发、声明检查上的一次哈希查表变成一次取字段。
2. **注册表改成进程级**。std 下 `Mutex<Option<…>>`,裸机维持 spin。所有调用者
   本来就是"把要的东西拷出闭包再 raise"(raise 会 longjmp 过 drop,守卫跨越它
   就永远不解锁),这条纪律现在是注释里写明的前提。
3. **过 channel 的深拷贝要带上 id**。`OwnedVal::Map` 只搬条目,收到的一端就是
   一张普通 map;加一个 `i64` 字段,`materialize` 写回去。

把 lkrt 里每一处 `is_map_tag` 逐条过了一遍(共 26 处),又挖出六条:`+` 合并、
`-` 去键(两种形状)、`.clear()`、`.get(k, default)`。前四条只要在 map 分支上加
"不是结构体实例",就自然掉进原有的报错——`kind_name` 早就会把结构体叫 `P`,所以
文案自动对上(`Add expected numbers or strings, got P and Map`)。后两条按接收者
拒绝。合法留下的只有字段读写(`dyn.field`/`dyn.index_set`)和相等(相等本来就比
类型标记)。

`.clear()` 那条还暴露出 §64 的守卫写窄了:它只在**已知是结构体**时拒绝,而两个
调用点分别传结构体和 map 的参数根本没有事实——正是错答所在。守卫改成
`MapStrDyn` 接收者必须**证明**是 map。改完 `examples/syntax/pattern_matching.lk`
掉了下降:`{ k: v, ..rest }` 产出的 `rest` 也是一个没标记的新 map,补上。同时
`str_dyn_without` 的拷贝把 `type_id` 清零——少一个字段的结构体不再是那个结构体。

`a_struct_keeps_its_name_across_a_task` 钉四条:过 channel、被闭包捕获、嵌套加
列表、从任务里送回来。

同一趟里 §64 的 `PlainMap` 证明缺了三个产出点——map 合并、`dyn.sub` 的 map 结果、
以及**所有** stdlib 行的 `Map<str, Dyn>` 返回值。三条 `PureCranelift` 差分用例
因此拒绝下降;覆盖率门禁和 VM/原生扫描都没看见(它们允许回退)。stdlib 那条写在
通用行下降的收尾处,而不是逐行写,这样新加一行不会漏。
