中文 | [English](README.md)

<div align="center">
    <h2>LKR</h2>
    <h5>使用 Rust 编写的，类似 Rust 的脚本语言</h5>
</div>

## 简介

### 示例（语句）

更多语言细节： [LANG_zh.md](LANG_zh.md)

## 特性

### 用法

#### 集成（库）

```rust
use lkr_core::{expr::Expr, vm::VmContext, val::Val};

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

- 进入 REPL：`lkr`
- 执行脚本/字节码：`lkr FILE`（自动检测 `.lkr` 源码或 `.lkrb` 字节码）
- 仅做静态类型检查：`lkr check FILE`（输出编译期诊断信息）
- 编译为字节码：`lkr compile FILE` → `FILE.lkrb`
- 编译为 LLVM IR：`lkr compile llvm FILE`（详见 [docs/llvm/backend.md](docs/llvm/backend.md)）
- 编译为 ELF 可执行文件：`lkr compile exe FILE`（需安装 LLVM 工具链与系统链接器，详见 [docs/llvm/backend.md](docs/llvm/backend.md)）

注意：命令行参数路径必须为经净化的相对路径。

## 许可证

```plaintext
Apache-2.0 lollipopkit
```
