# AOT Lowering Gaps & lkrt Sinking Plan

> 设计笔记 / 决策记录。面向维护者,记录当前 LLVM AOT 后端的覆盖形态、长尾缺口的
> 根因,以及把单态容器/字符串/display 操作下沉到 `lkrt` 的路径。规范性 ABI 约束
> 见 [`native-stdlib.md`](./native-stdlib.md),已支持形状清单见 [`backend.md`](./backend.md)。
>
> **架构级重设计**(类型化 MIR + 结构化发射 + 单一真相 ABI + 句柄化运行时)见
> [`aot-redesign.md`](./aot-redesign.md):本文记录"当前实现的下沉进展",重设计文档记录
> "目标架构与迁移路径"。

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
- **禁止**对可能重叠的范围用 `copy_from_slice` / `copy_nonoverlapping`
  (会触发 `ptr::copy_nonoverlapping` 前置条件 panic);
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
