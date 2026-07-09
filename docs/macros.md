# LK 宏系统

LK 提供 Rust 形态的宏系统:声明宏(`macro_rules!`)+ 隔离的外部过程宏
provider(derive / attribute / function-like),带卫生(hygiene)、宏导入/
再导出、`#[cfg(...)]` 条件编译,以及 token 级 origin/source-map(LSP 的
宏感知 hover / goto-definition 依赖它)。实现在 `core/src/macro_system/`;
本文是使用者视角的参考。

## 声明宏:`macro_rules!`

```lk
macro_rules! unless {
    ($condition:expr $body:block) => { if (!($condition)) $body };
}

unless!(x == 9 {
    println("not nine");
});
```

- **调用形式**:`name!(...)`、`name![...]`、`name!{...}` 三种定界符等价。
  注意与后缀解包 `!` 的消歧:`!` 紧跟 `(`/`[`/`{` 解析为宏调用,解包后再调
  用需加括号(`(x!)(...)`,见 `docs/semantics.md` 语法边界)。
- **多规则**:一个宏可含多条 `(matcher) => { template }` 规则,按声明序
  依次尝试,首个匹配的规则展开。

### Fragment 说明符

元变量写作 `$name:kind`,支持 10 种 fragment
(`core/src/macro_system.rs` `FragmentKind`):

| kind | 匹配 |
|------|------|
| `expr` | 表达式 |
| `stmt` | 语句 |
| `block` | `{ ... }` 块 |
| `item` | 顶层项(fn/struct/…) |
| `ident` | 标识符 |
| `literal` | 字面量 |
| `tt` | 单棵 token 树 |
| `pat` | 模式 |
| `ty` | 类型 |
| `path` | 路径 |

fragment 之后允许跟随的 token 受 follow-set 约束
(`core/src/macro_system/follow.rs`),非法组合在宏**定义**时报错,不会
等到调用点。

### 重复

- `$( ... )*` — 零次或多次;`$( ... )+` — 至少一次;`$( ... )?` — 零或一次
  (不允许分隔符)。
- `*`/`+` 可带单 token 分隔符(定界符除外):`$($value:expr),*`。
- 模板中元变量的重复深度必须与 matcher 一致
  (`core/src/macro_system/validation.rs` 在定义时校验)。

### 卫生(hygiene)

宏体内引入的绑定不会捕获/污染调用点同名变量(`core/src/macro_system/
hygiene.rs`);展开产物中的控制流、参数名、语义名各有针对性的保护面
(见 `hygiene_tests/`)。`$crate` 锚在定义时解析为定义方的绝对包名
(`runtime_anchor.rs`),跨包展开不会错绑。

## 宏导入与导出

宏是编译期实体,用普通 `use` 语法导入,但在宏展开阶段消费:

```lk
use { vec, assert_eq, matches } from macros;   // 内建 macros 模块
use { my_macro } from "helpers/macros";         // 文件导入(相对路径)
use { pkg_macro } from some_package;            // 包导入(Lk.toml 依赖)
use * as m from macros;                          // 命名空间导入:m::vec![1, 2]
```

- 定义处需 `export macro_rules! name { ... }` 才可被导入;
  `pub use { name } from "path";`(可 `as` 改名)做再导出。
- **内建 `macros` 模块**(`core/src/macro_system/imports.rs`
  `BUILTIN_MACRO_SOURCE`)提供 8 个宏:`vec!`、`assert!`、`assert_eq!`、
  `assert_ne!`、`matches!`、`panic!`、`todo!`、`unreachable!`。

## 属性与条件编译

- `#[cfg(true)]` / `#[cfg(false)]` / `#[cfg(feature = "...")]` 在宏展开阶段
  裁剪项;feature 由 `lk macro expand --feature NAME`(可重复)或对应
  构建配置开启。
- **内建 derive**:`#[derive(Debug)]`(等价 `#[derive(Show)]`)为 struct
  生成 `__LKShow` trait 的 `show` 方法——`"${value}"` 插值与 `println`
  会自动调用它(`Point { x: 1 }` → `"Point { x: 1 }"`)。

## 外部过程宏 provider

derive / attribute / function-like 三类 provider 是**隔离子进程**,通过版本
锁定的 JSON stdin/stdout 协议通信,在 `Lk.toml` 声明
(完整说明见 [docs/packages.md](packages.md)):

```toml
[macros]
trusted_dependencies = ["helper_macros"]   # 信任模型:显式 opt-in

[macros.derive.MakeAnswer]
command = "./tools/derive-make-answer"     # 相对 manifest 目录,或 PATH 命令
args = ["--json"]
timeout_ms = 5000
max_output_bytes = 1048576

[macros.attribute.route]
command = "lk-route-macro"

[macros.function_like.sql]
command = "lk-sql-macro"
```

- derive 在被注解 struct 之后追加生成项;attribute 可变换/替换/删除单个
  被注解项;function-like 展开 `name!(...)` 为 token 流。
- provider 响应可携带依赖元数据(`path` + `digest`),编译器/LSP 据此做
  **依赖感知的缓存失效**:依赖文件变化 → 重新展开;原生可执行产物写
  `.proc-macro-deps.json` sidecar。

## 检查展开:`lk macro expand`

```bash
lk macro expand FILE.lk              # 打印 token 级 + AST 级展开结果
lk macro expand FILE.lk --trace      # 每步展开轨迹(宏名/调用位置/产物规模)
lk macro expand FILE.lk --deps       # 过程宏依赖元数据(JSON)
lk macro expand FILE.lk --origins    # token 级宏 origin 栈(source-map)
lk macro expand FILE.lk --feature X  # 开启 cfg feature(可重复)
```

## 示例与相关文档

- 可运行示例:`examples/syntax/macros.lk`(内建宏导入、`macro_rules!`、
  `#[derive(Debug)]` 整对象插值、`#[cfg]` 函数选择)。
- provider 协议/信任模型细节:[docs/packages.md](packages.md);
  错误文本与展开语义边界:[docs/semantics.md](semantics.md)。
