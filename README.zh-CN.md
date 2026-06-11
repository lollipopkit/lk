中文 | [English](README.md)

<div align="center">
    <h2>LK</h2>
    <h5>使用 Rust 编写的轻量 / 高效 / 现代语言</h5>
</div>

## 特性

- 类 Rust 语法，支持一等 named parameters
- Rust 形态的 `macro_rules!` 声明式宏，支持函数式调用、显式宏导出/re-export、文件/package 导入、标准 `macros` 导入、item attributes、内置 `#[derive(Debug|Show)]`、隔离进程外部 derive/attribute/function-like provider、dependency-aware proc macro 缓存失效、LSP macro-origin hover/symbols、同文件/导入宏与 generated item goto-definition，以及逐 token macro origin/source-map 检查；宏生态路线图见 [docs/macros.md](docs/macros.md)
- VM 解释器和 LLVM 编译器后端，支持跨平台原生编译和浏览器 WASM
- 内置标准库/各类语法糖
- 包管理器和 REPL，支持 VS Code LSP 扩展

## 示例

细节： [lang.lollipopkit.com](https://lang.lollipopkit.com)

## 安装

安装 GitHub 最新 release：

```bash
curl -fsSL https://raw.githubusercontent.com/lollipopkit/lk/main/scripts/install.sh | sh
```

安装指定 release：

```bash
curl -fsSL https://raw.githubusercontent.com/lollipopkit/lk/main/scripts/install.sh | LK_VERSION=v0.1.3 sh
```

### 示例文件

```
examples/
├── syntax/          # 语言特性演示
│   ├── closure.lk        # 闭包与高阶函数
│   ├── match.lk          # match 表达式与模式
│   ├── pattern_matching.lk # if-let、while-let、解构
│   ├── ...               # 更多
├── stdlib/           # 标准库演示
│   ├── list_ops.lk        # 列表方法 (map, filter, reduce)
│   ├── stream_demo.lk     # 惰性流管道
│   ├── ...               # 更多
├── general/          # 综合示例
│   ├── sort_search.lk    # 插入排序、搜索算法
│   ├── config_parser.lk  # JSON/YAML/TOML 配置加载
│   ├── ...
└── _references/      # 跨语言参考（Dart、Lua、C）
```

运行示例：`lk examples/syntax/closure.lk`

## 用法

### 集成（库）

```rust
use lk_core::{syntax::{parse_program_source, ParseOptions}, vm::VmContext};

// 通过 bytecode VM 解析并执行。
let source = r#"
let data = {
    "req": { "user": { "name": "foo" } },
    "files": [ { "name": "file1", "published": true } ],
};
return data.req.user.name in "foobar" && data.files.0.published == true;
"#;
let program = parse_program_source(source, ParseOptions::default())?;
let mut ctx = VmContext::new();
let result = program.execute_with_ctx(&mut ctx)?;

assert_eq!(result.display_first_return(), "true");
```

### CLI

- 进入 REPL：`lk`
- 执行源码或模块产物：`lk FILE`（支持 `.lk` 和 `.lkm`）
- 仅做静态类型检查：`lk check FILE`（输出编译期诊断信息）
- 编译为 native 可执行文件：`lk compile [FILE]`（省略 `FILE` 时使用当前目录的 `main.lk`、package 的 `src/main.lk`，或单一 workspace app 入口；不支持的 LLVM native lowering 形状会失败）
- 编译为 bytecode 模块产物：`lk compile bytecode [FILE]` → `FILE.lkm`
- 编译为 LLVM IR：`lk compile llvm [FILE]`（详见 [docs/llvm/backend.md](docs/llvm/backend.md)）
- 创建包、管理依赖、发布 registry manifest、管理签名 keyring，并运行本地签名 registry：`lk pkg init`、`lk pkg add`、`lk pkg fetch`、`lk pkg check`、`lk pkg publish`、`lk pkg key`、`lk pkg serve`、`lk pkg tree`（详见 [docs/packages.md](docs/packages.md)）

注意：命令行参数路径必须为经净化的相对路径。

### 编辑器支持

编辑器集成统一放在 `ecosystem/` 下。

- VS Code 支持已合并为 `ecosystem/vsc-ext/lsp` 下的单个扩展，包含 `.lk` 语言注册、TextMate 高亮、代码片段，以及带智能补全的 LK LSP 客户端；补全覆盖 stdlib 模块、导入别名、本地符号、named arguments、重复出现的字符串参数值和常见 receiver 方法。使用 `make debug-lsp-ext` 启动本地 Extension Development Host，或使用 `make vsix` 构建 VSIX。
- Zed 支持位于 `ecosystem/zed-ext`，使用 `ecosystem/tree-sitter-lk` 提供 Tree-sitter 高亮，并启动 `lk-lsp` 提供 diagnostics、completion、hover、goto definition、document symbols、semantic tokens 和 inlay hints。使用 `make zed-ext-check` 验证扩展 crate。

## 许可证

```plaintext
Apache-2.0 lollipopkit
```

## 致谢

- 设计灵感部分来自于大学时期看到的手写lua VM/Compiler教程
- OpenAI OSS赠送的六个月ChatGPT Pro
