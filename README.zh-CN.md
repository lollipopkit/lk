中文 | [English](README.md)

<div align="center">
    <h2>LK</h2>
    <h5>使用 Rust 编写的，类似 Rust 的脚本语言</h5>
</div>

## 简介

### 示例（语句）

更多语言细节： [lang.lollipopkit.com](https://lang.lollipopkit.com)

### 示例文件

```
examples/
├── syntax/          # 语言特性演示
│   ├── closure.lk        # 闭包与高阶函数
│   ├── match.lk          # match 表达式与模式
│   ├── pattern_matching.lk # if-let、while-let、解构
│   ├── operators.lk       # 算术、比较、逻辑、??
│   ├── ...               # 更多
├── stdlib/           # 标准库演示
│   ├── list_ops.lk        # 列表方法 (map, filter, reduce)
│   ├── json_demo.lk       # JSON 解析与处理
│   ├── stream_demo.lk     # 惰性流管道
│   ├── ...               # 更多
├── general/          # 综合示例
│   ├── sort_search.lk    # 插入排序、搜索算法
│   ├── word_count.lk     # 文本处理与词频统计
│   ├── config_parser.lk  # JSON/YAML/TOML 配置加载
│   ├── ...
└── _references/      # 跨语言参考（Dart、Lua、C）
```

运行示例：`lk examples/syntax/closure.lk`

## 特性

### 用法

#### 集成（库）

```rust
use lk_core::{stmt::stmt_parser::StmtParser, token::Tokenizer, vm::VmContext};

// 通过 Instr32 VM 解析并执行。
let source = r#"
let data = {
    "req": { "user": { "name": "foo" } },
    "files": [ { "name": "file1", "published": true } ],
};
return data.req.user.name in "foobar" && data.files.0.published == true;
"#;
let tokens = Tokenizer::tokenize(source)?;
let program = StmtParser::new(&tokens).parse_program()?;
let mut ctx = VmContext::new();
let result = program.execute32_with_ctx(&mut ctx)?;

assert_eq!(result.display_first_return(), "true");
```

#### CLI

- 进入 REPL：`lk`
- 执行源码或 Instr32 模块产物：`lk FILE`（支持 `.lk` 和 `.lkm`）
- 仅做静态类型检查：`lk check FILE`（输出编译期诊断信息）
- 编译为可执行 Instr32 模块产物：`lk compile [FILE]` → `FILE.lkm`（省略 `FILE` 时使用当前目录的 `main.lk`、package 的 `src/main.lk`，或单一 workspace app 入口）
- 编译为 LLVM IR：`lk compile llvm [FILE]`（详见 [docs/llvm/backend.md](docs/llvm/backend.md)）
- 编译为 native 可执行文件：`lk compile exe [FILE]`（仅支持可 LLVM native lowering 的形状；不支持的形状会失败，详见 [docs/llvm/backend.md](docs/llvm/backend.md)）
- 创建包并管理依赖：`lk init`、`lk pkg add`、`lk pkg fetch`、`lk pkg tree`（详见 [docs/packages.md](docs/packages.md)）

注意：命令行参数路径必须为经净化的相对路径。

#### VS Code

VS Code 支持已合并为 `vsc-ext/lsp` 下的单个扩展，包含 `.lk` 语言注册、TextMate 高亮、代码片段和 LK LSP 客户端。使用 `make debug-lsp-ext` 启动本地 Extension Development Host，或使用 `make vsix` 构建 VSIX。

## 许可证

```plaintext
Apache-2.0 lollipopkit
```
