# Handoff

目标:落地 `docs/llvm/aot-gaps-and-lkrt.md` —— 把单态动态容器/字符串/display 操作从
llvm 手写 IR 下沉到 `lkrt` typed ABI。**本轮:完成 string-key map + decimal_len 下沉
(batch 5),full-suite AOT skip 元凶(dynamic-map string GetIndex)已消除。**

## 全部已完成的下沉

- **list**:`list.{i64,f64,str}`(batch 1-3)。
- **map.i64**:`lkrt_map_i64_{int,f64,ptr}_{lookup,set}`(batch 4)。
- **map.str**(batch 5,本轮):`lkrt_map_str_{split_key,contains}` +
  `lkrt_map_str_{int,f64,ptr}_{lookup,set,delete}`。复合短字符串 key(prefix+int)。
  - Phase A:int/f64 set/get/split_key 从 `@lk_*`(手写 IR helper 池)drop-in 换成
    `@lkrt_map_str_*`,删除 `native_dynamic_container_helpers()` 里 6 个手写定义。
  - Phase B:补 `lkrt_map_str_ptr_{lookup,set}`,`string_maps.rs` 的 ptr map set/get
    从内联循环改调 helper,删 `emit_string_map_{set,get}_loop`。
  - Phase C:补 `lkrt_map_str_contains` + `lkrt_map_str_{int,f64,ptr}_delete`,
    `has`/`delete` 内联循环改调 helper,删 `emit_string_key_match`/`emit_string_map_copy_item`。
- **display**:`lkrt_i64_decimal_len` 替 `@lk_i64_decimal_len`。

## 验证

- `cargo test -p lkrt` 21、`cargo test -p lk-llvm` 251、`cargo test --workspace
  --all-features` **1609 passed / 0 failed**。
- `nm` 确认 `lkrt_map_str_*` + `lkrt_i64_decimal_len` 链接进 native 二进制;
  string-int-map set/get(`m.k=v`)端到端 `lk compile` 与 VM 一致。
- **测试断言修复**:旧 helper 池无条件注入每模块,3 个测试误断样板 IR
  (`select i1`/`strcmp`),已改断被测 lowering 自身产物。

## 关键坑(后续必读)

- **helper 池样板**:删手写 helper 前先查是否有测试断言其内部 IR(见 aot-gaps §8 末)。
- **in-place 别名**(`src==dst`):裸指针 + `ptr::copy`;delete 的 `dst_i<=i` 前向写
  才安全(见 progress.md 与 aot-gaps §7)。

## 剩余待下沉(收益低 / 受阻)

- `@lk_concat_i64_list`(bool-list concat 借 i64-slot)仍手写 IR。
- 两种 map 的 `iter`/`values`/`keys`(索引拷贝/snprintf 重建 key,非 ad-hoc 方法体)。
- 结构性硬限制(§2.1:闭包/间接调用/可变全局)—— 单独立项。
- 无法非折叠 native RUN 的形状:`map` 模块非 CLI 可达 + 常量折叠 + env.get_or 有
  **既有无关 bug**(`@lk_const_str_0` undefined)。已用结构断言 + 单测覆盖。

细节见 `progress.md`,规范见 aot-gaps §8 与 native-stdlib.md。

## 架构级重设计(新)

`docs/llvm/aot-redesign.md` —— 把后端从"文本 IR 拼接 + 分析发射交织 + 逐 shape 手写"
重构为 **类型化 MIR + 结构化 SSA 发射 + 单一真相 ABI schema + 句柄化运行时**。含目标架构、
crate 划分、性能/测试策略、绞刑架式增量迁移(阶段 0 先落地除零守卫 + ABI 版本 assert)、
现有文件→新结构映射。本 RFC 是"目标",本轮下沉是沿旧结构的"当前进展"。
