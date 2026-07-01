# 实现进度:LLVM 容器操作下沉到 lkrt

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
