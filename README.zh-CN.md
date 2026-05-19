中文 | [English](README.md)

<div align="center">
    <h2>LK</h2>
    <h5>使用 Rust 编写的，类似 Rust 的脚本语言</h5>
</div>

## 简介

### 示例（语句）

更多语言细节： [LANG_zh.md](LANG_zh.md)

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
use lk_core::{expr::Expr, vm::VmContext, val::Val};

// 解析表达式
let expr_src = "data.req.user.name in 'foobar' && data.files.0.published == true";
let expr = Expr::try_from(expr_src)?;

// 在 VmContext 中提供变量（词法环境）
let mut ctx = VmContext::new();
let data_val: Val = serde_json::json!({
    "req": { "user": { "name": "foo" } },
    "files": [ { "name": "file1", "published": true } ]
}).into();
ctx.set("data", data_val);

// 求值
let result = expr.eval_with_ctx(&mut ctx)?; // Val::Bool(true)
assert_eq!(result, Val::Bool(true));
```

#### CLI

- 进入 REPL：`lk`
- 执行脚本/字节码：`lk FILE`（自动检测 `.lk` 源码或 `.lkb` 字节码）
- 仅做静态类型检查：`lk check FILE`（输出编译期诊断信息）
- 编译为字节码：`lk compile [FILE]` → `FILE.lkb`（省略 `FILE` 时使用当前目录的 `main.lk`、package 的 `src/main.lk`，或单一 workspace app 入口）
- 编译为 LLVM IR：`lk compile llvm [FILE]`（详见 [docs/llvm/backend.md](docs/llvm/backend.md)）
- 编译为 ELF 可执行文件：`lk compile exe [FILE]`（需安装 LLVM 工具链与系统链接器，详见 [docs/llvm/backend.md](docs/llvm/backend.md)）
- 创建包并管理依赖：`lk init`、`lk pkg add`、`lk pkg fetch`、`lk pkg tree`（详见 [docs/packages.md](docs/packages.md)）

注意：命令行参数路径必须为经净化的相对路径。

#### VS Code

VS Code 支持已合并为 `vsc-ext/lsp` 下的单个扩展，包含 `.lk` 语言注册、TextMate 高亮、代码片段和 LK LSP 客户端。使用 `make debug-lsp-ext` 启动本地 Extension Development Host，或使用 `make vsix` 构建 VSIX。

## 许可证

```plaintext
Apache-2.0 lollipopkit
```
