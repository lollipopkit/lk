# LLVM Runtime Linking Plan

## 背景

LLVM 后端已经能够将大部分 VM 字节码指令翻译成 LLVM IR，并在 IR 中调用一组 `lkr_rt_*` 的 runtime helper（例如 `lkr_rt_intern_string`、`lkr_rt_build_list`、`lkr_rt_call` 等）。当我们使用 `llc` + `clang` 将生成的 `.ll` 链接成可执行文件时，这些符号缺失导致链接失败。

要实现端到端的 AOT 流程，需要为这些 helper 提供可链接的实现，并定义清晰的运行时数据约定，以便与现有的 `VmContext` 与 `Val` 类型互通。

## 目标

1. **建立运行时代码库**：新建 `core/src/llvm/runtime.rs`（或独立 crate），导出所有 `lkr_rt_*` 符号。
2. **定义值编码协议**：设计 LLVM 后端与 runtime 之间的 64-bit 值布局（立即数、指针句柄、tag）。
3. **实现 helper**：以 `#[no_mangle] extern "C"` 暴露的函数为入口，内部桥接到现有解释器实现：
   - 字符串常量、全局读写、集合构造（list/map）、`Len`/`Index`、`Call` 等操作。
   - 函数调用 helper 需要能够调用闭包、Rust 原生函数等。
4. **集成构建**：调整 `Cargo.toml` 导出 `cdylib` 或 `staticlib`，让 CLI 在 `compile exe` 时自动链接该库。
5. **提供最小 `main` stub**：在链接路径中包含一个 `main`，负责初始化 runtime（例如构建 `VmContext`、安装 stdlib），然后调用 `lkr_entry`。
6. **验证**：
   - 添加新的集成测试，生成 `.ll`、`llc` 为 `.o`、`clang` 链接，与 runtime 库静态链接后运行产物。
   - CI 增加 step，确保 LLVM AOT 流程可用。

## 值编码草案

| Tag | 含义 | Bits | 说明 |
| --- | ---- | ---- | ---- |
| `0b000` | 未定义/保留 | 61 bits | --- |
| `0b001` | Int immed | 61 bits | `value << 3 | TAG_INT` |
| `0b010` | Bool | 1 bit | `0` -> false, `1` -> true |
| `0b011` | Nil | - | 固定常量 |
| `0b100` | Pointer handle | 61 bits | 指向堆对象表（字符串、列表、map 等） |
| `0b101` | Function handle | 61 bits | 指向闭包或 Rust 函数 |
| `0b110` | Iterator handle | 61 bits | 等 |

> TODO: 最终方案需考虑 GC/生命周期。初版可以借助 `VmContext` 的 `Val` 转换并在 runtime 内部暂存于 `Arc<Val>` 表。

## Helper 实现要点

- **字符串 (`intern_string`)**：构造 `Val::Str`，返回 handle；支持重复利用。
- **全局 (`load_global`/`define_global`)**：通过全局 `VmContext` 字典读写。
- **集合 (`build_list`/`build_map`)**：根据传入 array 构造 `Val::List` / `Val::Map`。
- **索引 (`access`/`index`/`len`/`to_iter`)**：直接复用现有 VM 逻辑，处理错误 -> 返回 Nil。
- **函数调用 (`call`)**：解析函数 handle -> `Val::Closure` / `Val::RustFunction`，执行并返回结果。

## 工程步骤

1. `docs/llvm/linker.md`：记录设计与约定（本文）。
2. `core/src/llvm/runtime.rs`：实现 helper，定义 value encoding。
3. `core/src/lib.rs`：导出 runtime 模块。
4. `Cargo.toml`：添加 `crate-type = ["rlib", "staticlib"]`；CLI 构建时链接。
5. CLI：在 `compile exe` 时追加 runtime staticlib。
6. 添加 `tests/llvm_link.rs` 集成测试，执行 end-to-end 构建。

## 未决问题

- GC/生命周期：需要确保 runtime handle 持续有效，可能需要 VM 级引用计数表。
- 错误处理：helper 抛错时，如何向 LLVM caller 报错（panic / status code / trap）。
- 性能：初版依赖 `VmContext`，后续可优化。

## 下一步

- 评估现有 `Val` 的 64-bit 编码需求；确定 int/bool/nil/pointer 区分方式。
- 搭建最小 runtime shell，先实现 `intern_string` / `define_global` / `load_global`，验证链接错误消失。
- 逐步补齐其他 helper，直到示例 `import.lkr` 可直接运行。

