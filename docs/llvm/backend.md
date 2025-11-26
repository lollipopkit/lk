# LLVM 后端

## 实现清单

完整路线图如下，`[x]` 表示已完成的项目：

- [x] 复用现有 VM Lowering，提供 `compile_program_to_llvm` / `compile_function_to_llvm` 入口。
- [x] 将基础算术、比较、局部寄存器读写与 `ret` 指令翻译为 LLVM IR。
- [x] 支持短路逻辑与空合并相关指令（`JmpFalseSet` / `JmpTrueSet`、`NullishPick`、`JmpIfNil` / `JmpIfNotNil`）。
- [x] 接入 `opt` 可选优化流程，CLI 支持 `--opt-level` 与 `--skip-opt`。
- [x] CLI 集成：`lkr compile llvm FILE` 输出 `.ll`，可选保留 `.unopt.ll`。
- [x] CLI 增加 `lkr compile exe FILE`，串联 `llc` 与系统链接器生成 ELF 可执行文件。
- [x] 新增单元测试覆盖上述指令翻译路径（`core/src/llvm/tests.rs`）。
- [ ] 扩展字节码指令映射（集合构造、全局/捕获访问、函数调用等）。
- [ ] 引入运行时 Helper 或内联策略以处理字符串、列表、映射等复杂类型。
- [ ] 追加 LLVM Bitcode/Object 文件生成与链接流程。
- [ ] 构建性能回归基准，验证 LLVM 后端相较 VM 的收益。
- [ ] 将 LLVM 生成与验证纳入 CI / 发布流水线。

## 架构概览

1. **复用字节码编译结果**：通过既有的 `compile_program` 获取 `vm::Function`，避免重复 AST 降级逻辑。
2. **块级翻译**：`lkr_core::llvm::LlvmBackend` 逐基本块遍历 VM 指令，输出文本 LLVM IR（统一使用 `i64`、不透明指针），保持原始控制流结构。
3. **可选优化**：后端可调用 `opt`（来源为 `llvm-tools-preview` 组件或 `LKR_LLVM_OPT` 环境变量指定路径）生成优化版 IR，与原始 IR 并存。

```
Program (AST) ──compile_program──▶ vm::Function ──LLVM translator──▶ module.ll
                                                                             │
                                                                             └─opt（可选）──▶ module.ll / module.unopt.ll
```

## 支持范围

- 常量类型：当前支持 `Int`、`Float`、`Bool`、`Str`、`Nil`，统一编码为 `i64`（为避免与普通整数冲突，`Nil` / `false` / `true` 分别使用接近 `i64::MIN` 的哨兵值 `-9223372036854775808`、`-9223372036854775807`、`-9223372036854775806`，浮点/字符串常量以位转换写入堆栈；为了让编码保持单射，编译期会拒绝与哨兵值相等的整数常量）。其他常量类型会报错提示。详见下文“值表示策略对比”章节对各方案的详细权衡。
- 核心指令：`LoadK`、`Move`、`LoadLocal`、`StoreLocal`、算术（`Add` / `Sub` / `Mul` / `Div` / `Mod` 以及特化指令 `AddInt` / `SubInt` / `MulInt` / `ModInt` / `AddFloat` / `SubFloat` / `MulFloat` / `DivFloat` / `ModFloat`）、比较（`Cmp*`）、布尔/字符串/全局工具（`ToBool` / `Not` / `ToStr` / `LoadGlobal` / `DefineGlobal` / `Len` / `ToIter`）、集合构造（`BuildList` / `BuildMap`）、成员/切片操作（`In`、`ListSlice`）、索引访问（`Access` / `AccessK` / `Index` / `IndexK`）、函数调用（`Call`）、短路/空合并控制（`JmpFalseSet`、`JmpTrueSet`、`NullishPick`、`JmpIfNil`、`JmpIfNotNil`）、无条件/条件跳转（`Jmp`、`JmpFalse`）、`Ret`。
- 控制流：保留原有编译器生成的结构化分支（如 `if` / `else`），每个基本块以 `br` 或 `ret` 终止，满足 LLVM 校验器需求。

运行时接口：当 IR 需要字符串常量、全局读写、集合构造或 `ToStr` / `Call` 等操作时，会自动声明 `declare i64 @lkr_rt_intern_string(ptr, i64)`、`declare i64 @lkr_rt_to_string(i64)`、`declare i64 @lkr_rt_load_global(i64)`、`declare void @lkr_rt_define_global(i64, i64)`、`declare i64 @lkr_rt_build_list(ptr, i64)`、`declare i64 @lkr_rt_call(i64, ptr, i64, i64)`、`declare i64 @lkr_rt_in(i64, i64)`、`declare i64 @lkr_rt_list_slice(i64, i64)` 等 helper，并在入口块中生成对应的调用。自 2025-10 起，main stub 还会注入 `lkr_rt_begin_session` / `lkr_rt_register_search_path` / `lkr_rt_register_bundled_module` / `lkr_rt_register_imports` / `lkr_rt_apply_imports`，用于在原生可执行文件中重放模块导入、预注册打包的 LKRB 模块以及初始化标准库。

这些符号由 `core/src/llvm/runtime.rs` 提供：模块内部维护一个 `VmContext`、字符串 interner 以及句柄表，将 LLVM 传入的 64-bit 编码值翻译回 `Val` 并重用既有 VM 语义（列表/字典构造、`ToIter` 等）。`lkr compile exe` 会在编译阶段遍历 AST，借助 `ModuleBundler` 提前编译文件类导入，序列化导入语句，并将二进制 LKRB 片段作为 LLVM 全局常量嵌入；运行时由上述 helper 逐个注册，从而在没有 VM 的情况下也能复现模块解析流程和 `execute_imports` 语义。

为了满足链接需求，CLI 会自动构建并链接 `liblkr_core.a` 与 `liblkr_stdlib.a`：前者导出所有 `lkr_rt_*` helper，后者提供 `lkr_stdlib_register_*` 桥接函数以注入内置模块/全局。需要本地 `cargo`、LLVM 工具链（`llc` 等）以及静态链接器均可用。

发布二进制版 CLI 时，务必把上述静态库一起打包（例如 `bin/lkr`、`lib/liblkr_core.a`、`lib/liblkr_stdlib.a`）。运行时会首先尝试从 `LKR_RUNTIME_LIB_DIR` 指定的目录、或可执行文件旁的 `lib/`、`lib/<target-triple>/` 及其 `release` / `debug` 子目录复用这些库，只有在找不到时才回退到 `cargo build`。这样即便目标机器上没有完整的 Rust 工具链，也能执行 `lkr compile exe`。

### 值表示策略对比

#### 方案 A：纯 `i64` + 哨兵值（现状）
- **优点**：
  - 与既有 VM 求值模型一致，LLVM 后端可以复用栈布局与运行时 ABI。
  - 所有指令与 helper 仅需处理单一寄存器宽度，汇编与链接流程最简单。
- **缺点**：
  - 需要保留哨兵常量，整数域被迫排除几个值，扩展更多类型会继续占用特殊编码。
  - 对读取方而言类型信息隐式存在，调试与错误诊断不直观。
  - LLVM 难以根据具体类型做优化（如针对 `i1`/`double` 的专用指令）。
- **适用场景**：原型阶段或追求最小改动的发布迭代。

#### 方案 B：NaN-boxing / 指针打标
- **优点**：
  - 仍保持 64-bit 宽度，整数/指针无需牺牲取值范围；可以快速用位运算判断类型。
  - LLVM IR 中可继续沿用 `i64` 或 `double`，不必重写过多栈操作。
- **缺点**：
  - 依赖 IEEE-754 `double` 的 NaN 表示，跨不同架构或日后扩展到 32-bit 平台需额外验证。
  - 语义上仍是“隐式类型”，只是换成了更灵活的哨兵形式，调试体验改善有限。
  - 指针需要保证低位空闲（对非 8 字节对齐的对象不适用），运行时需维护更多约束。
- **适用场景**：追求性能同时愿意承担平台假设的场合。

#### 方案 C：显式 `Value` 结构（如 `struct { i8 tag; i64 payload; }`）
- **优点**：
  - 类型信息显式存储，避免哨兵冲突，扩展新类型只需新增枚举 tag。
  - LLVM 可通过 tag 做 switch，优化机会更明确，调试输出也更友好。
  - 运行时与 VM 的互操作可通过 `#[repr(C)]` 结构统一，语义清晰。
- **缺点**：
  - 栈/寄存器操作需处理结构体（`alloca` + `load`/`store`），对性能有额外负担。
  - 现有 helper、列表/字典构造逻辑都需同步适配新的布局。
  - 需要重写常量加载、算术指令等 lowering，代码 churn 较大。
- **适用场景**：准备投入一次性重构、换取更高语义清晰度和可扩展性的版本。

#### 方案 D：LLVM 层面强类型化
- **优点**：
  - 直接使用 `i1`、`i64`、`double`、`ptr` 等原生类型，LLVM 能以最优形式优化算术与分支。
  - 运行时 ABI 可针对每种类型定制，避免多余装箱。
  - 语言后续增加代数数据类型、记录等，都可以沿用 LLVM 原语组合。
- **缺点**：
  - 需要重写求值栈与寄存器分配，算术/集合操作必须显式做类型收敛与转换。
  - VM ↔ LLVM 的互操作复杂度显著上升，调试工具、运行时 helper 均需重新设计。
  - 目前的 `lkr_rt_*` API 基于统一字宽，需要引入新的 FFI 层做拆装箱。
- **适用场景**：作为长期目标，在 LLVM 后端成为主路径且弱化 VM 依赖时。

#### 推荐方案：显式 `Value` 结构
- 在保证类型安全、调试友好的前提下，不依赖浮点格式假设，也不再牺牲整数域。
- 扩展路径清晰，可在 `tag` 中预留空间以支持后续的集合、函数闭包等复合值。
- 可与 VM 共存：先在 LLVM 后端落地 `Value` 结构，通过适配层将其映射回 VM 的 `Val`，再逐步推广到 VM 本身，形成统一的中间表示。
- 后续工作指引：
  1. 在 `core/src/llvm/runtime.rs` 定义 `#[repr(C)] struct Value { tag: u8, payload: u64 }`，实现与现有 `Val` 之间的转换。
  2. 更新常量加载与算术/比较 lowering，通过 tag 判断路径，在类型不匹配时调用运行时错误分支。
  3. 调整所有 `lkr_rt_*` helper 的签名与实现，使其接受/返回新的 `Value` 结构，并在链接文档中同步 ABI 说明。
  4. 增补单元测试与端到端用例，覆盖 tag 判别、跨语言 FFI 等关键路径。

在该方案完全实施之前，可继续沿用方案 A，并在诊断信息中提示哨兵冲突风险，为迁移提供过渡期提示。

### 当前限制

- 尚未覆盖闭包、捕获、具名参数调用、原生函数等动态特性，遇到此类指令会返回 `unsupported opcode`。
- 不支持多返回值与具名参数调用。
- 目前仅输出文本 IR，对象文件/可执行文件的生成将放在后续里程碑。

## 命令行使用

```
lkr compile llvm path/to/file.lkr [--opt-level {O0|O1|O2|O3}] [--skip-opt] [--target-triple TRIPLE]
lkr compile exe path/to/file.lkr [--opt-level {O0|O1|O2|O3}] [--skip-opt] [--target-triple TRIPLE] [--output PATH]
```

- `llvm`：生成 `file.ll`。若启用优化，优化后的 IR 写入 `file.ll`，未优化版本写入 `file.unopt.ll`。
- `--opt-level`：设置 `opt` 的优化级别，默认 `O2`。
- `--skip-opt`：跳过 `opt` 流程，适合尚未安装 LLVM 工具链的环境。
- `--target-triple`：覆盖模块的 target triple，同时传递给 `llc` / 链接器。
- `--output`：仅对 `compile exe` 生效，指定最终 ELF 输出路径（默认 `<源文件名>.elf`）。

工具查找顺序：

| 功能 | 环境变量优先级 | 备用来源 |
| ---- | -------------- | -------- |
| `opt` | `LKR_LLVM_OPT` | `llvm-tools-preview` / 系统 `PATH` |
| `llc` | `LKR_LLVM_LLC` | `llvm-tools-preview` / 系统 `PATH` |
| 链接器 | `LKR_CC` → `CC` → `cc` | 系统 `PATH` |

`compile exe` 会先调用 LLVM 后端生成 `.ll`（同样在需要时保留 `.unopt.ll`），然后执行：

1. `llc -filetype=obj` → 生成目标文件 `<name>.o`（尊重 `--target-triple`）。
2. `cc` / `clang` / `LKR_CC` → 链接生成 ELF，可通过 `--output` 改写目标路径。

生成的 IR 会自动附加一个最小化的 `main` Stub（调用 `@lkr_entry` 并忽略返回值），方便直接链接成可执行文件。

示例：

```
$ lkr compile examples/arith.lkr --emit llvm --opt-level O1
Emitted LLVM IR to examples/arith.ll (optimised, opt-level O1)
Preserved unoptimised IR at examples/arith.unopt.ll

$ lkr compile exe examples/arith.lkr --target-triple x86_64-unknown-linux-gnu
Emitted ELF executable to examples/arith.elf (opt-level O2, LLVM IR at examples/arith.ll)
```

## 作为库调用

```rust
use lkr_core::{
    llvm::{compile_function_to_llvm, LlvmBackendOptions, OptLevel},
    vm::compile_program,
};

let program = parser.parse_program(...)?;
let func = compile_program(&program);
let artifact = compile_function_to_llvm(
    &func,
    "lkr_entry",
    LlvmBackendOptions {
        module_name: "example".into(),
        opt_level: OptLevel::O3,
        ..Default::default()
    },
)?;
println!("{}", artifact.module.ir);
```

当 `artifact.optimised_ir` 为 `Some` 时，表示已成功调用 `opt` 并返回优化后的文本 IR。

## 测试与验证

- 单元测试：`cargo test -p lkr-core llvm::tests`，覆盖算术、分支、短路逻辑、空合并与判空跳转。
- CLI 冒烟：`cargo run -p lkr-cli -- compile llvm path/to/file.lkr --skip-opt`，在缺少 LLVM 工具链的环境中验证端到端流程。
- 翻译器会校验跳转目标，遇到无效控制流会直接报错并中止生成，避免生成非法 CFG。

## 后续工作

- 扩展更多指令映射（列表/字典构造、全局绑定、闭包捕获、函数调用等）。
- 引入逃逸分析驱动的调用约定，与工作流 B 协同优化。
- 在本地和 CI 中加入性能基准，比较 VM 与 LLVM 后端执行表现。
- 将 LLVM 生成与验证纳入 CI / 发布流水线。

### 缺失特性规划

1. **运行时符号实现**
   - [x] 在 `core/src/llvm/` 下新增 `runtime.rs`，实现 `lkr_rt_in`、`lkr_rt_list_slice` 等 helper，并明确 64-bit 值编码（立即数、句柄、标记位）。
   - [x] 以 `#[no_mangle] extern "C"` 暴露接口，复用 `VmContext` 与 stdlib 逻辑，确保 AOT 链接阶段符号可解析。
2. **链接链路完善**
   - [ ] 在 `Cargo.toml` 启用 `staticlib` / `cdylib` 输出，`lkr compile exe` 自动链接 runtime。
   - [ ] 在 `docs/llvm/linker.md` 同步记录最终链接命令、依赖工具与环境变量约定。
3. **剩余指令覆盖**
   - [ ] 支持闭包捕获与构造（`LoadCapture`、`MakeClosure`）。
   - [ ] 支持具名参数调用（`CallNamed`）与多返回值下的寄存器写回策略。
   - [ ] 处理任务、通道、迭代器等高阶运行时值的编码、判等与 helper。
4. **验证与基准**
   - [ ] 添加端到端 AOT 测试（生成 `.ll` → `.o` → 可执行文件并运行），并集成到 CI。
   - [ ] 构建性能基准，对比 LLVM 后端与 VM 路径的吞吐与延迟。
