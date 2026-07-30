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

`spawn`、`try` 区域、以及被擦除的闭包环境这三处还不解析 `CellParam`,标了
TODO —— 它们拒绝,于是程序回落,而不是丢掉写回。

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

