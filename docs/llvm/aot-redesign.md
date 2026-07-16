# AOT 后端重设计 RFC

> 状态:**已实现,legacy text 后端已退役**(核心设计 §2-§6 全部落地,MIR 管线为
> **唯一后端**;实现记录见 §9.5。§7 约定的 legacy 退役已完成:约 -4.8 万行,
> `use_mir_pipeline`/`allow_legacy_fallback`/`LK_AOT_MIR`/`LK_AOT_LEGACY` 开关一并移除;
> 剩余项均为 §1 划定的非目标——阶段 4 闭包/间接调用/可变全局/方法分派与模块 builtin)。目标是把当前 LLVM
> AOT 后端从"文本 IR 拼接 + 分析发射交织 + 逐 shape 手写"重构为"类型化中间表示(MIR)+
> 结构化 SSA 发射 + 单一真相 ABI + 句柄化运行时"。要求:**高性能、现代设计规范、
> 清晰项目结构、优雅**。
>
> **后续更新(codegen 后端已换 Cranelift):** 本 RFC 的 `lk-aot-codegen` 原为
> **MIR → LLVM 文本(IrBuilder)→ clang** 的字符串 IR 渲染器。此渲染器已被
> **Cranelift 后端(`aot/codegen/src/clif.rs`)** 取代——`MIR → Cranelift IR
> (typed FunctionBuilder + verifier)→ 原生 object →(clang 仅作链接驱动)链接
> lkrt`。字符串 IR 渲染器 `render_module`、`lk compile llvm` 命令、`compile_*_to_llvm`
> 流水线、`LlvmBackendOptions/OptLevel/--opt-level/--target-triple` 全部移除
> (约 -3.2 千行)。Cranelift 达与字符串 IR 路径 **100% 差分对等**(纯原生 + Tier 1
> hybrid,corpus/fuzz/examples 零回落);代价是 AOT 运行时约慢 ~17%(Cranelift
> `speed` vs clang `-O2`),换 typed-builder + verifier 正确性网与更快编译。
> **本 RFC 的 §2/§3.2 架构描述以下按字符串 IR 时期保留为历史,codegen 现读作 Cranelift。**
>
> 关联:能力边界与拒绝原因见 [`aot-gaps-and-lkrt.md`](./aot-gaps-and-lkrt.md);
> 现行 ABI 约束见 [`native-stdlib.md`](./native-stdlib.md)。

---

## 0. 现状问题(基于代码,非印象)

| 症结 | 证据 | 后果 |
| --- | --- | --- |
| **纯文本 IR 拼接** | `llvm/src/llvm/scalar/` 2.4 万行 `push_str(format!("%r{dst} = ..."))`,SSA 名靠 `ir_text.rs::next_tmp` 计数、label 靠字符串 | 无类型检查、无 SSA 校验、寄存器/label 记账易错;`select i1` 之类样板串污染整模块 |
| **分析与发射交织** | `diagnostics.rs` 判定可 lower,`scalar/`/`subfunction/` 里又散落 `bail!` | "能不能 lower"不是一个可测试的总函数,而是发射途中随时崩 |
| **全有或全无 + 脆** | `backend.rs:78` 任意未覆盖 shape → 整程序 `bail!` | 真实程序一个动态调用就整体掉不出 AOT |
| **N×M 逐 shape** | `dynamic_containers/` 每(布局×方法)一份;`intrinsics.rs`/`lib.rs`/`containers.rs` 三处手工双写签名 | 加一个 shape 成本线性叠加,签名漂移到链接/运行期才炸 |
| **定长容器** | emit 侧分配 `[4096 x T]`(`dynamic_containers.rs:50`),map 为 O(n) 线性探查 | 第 4097 元素越界(正确性悬崖),map 性能悬崖 |
| **裸算术** | `block_helpers.rs:1021` `sdiv i64`,无除零守卫 | 与 VM `bail!("divisor is zero")` 分歧(UB) |
| **全泄漏所有权** | `dup_cstr`/`strdup` 从不 free,`lkrt_string_free` 声明零调用 | 长驻程序漏内存 |
| **ABI 版本名义化** | `lkrt_abi_version()` 无人比对 | emit 与 `liblkrt.a` 漂移 = 静默 UB |

**根因**:缺少一层**类型化中间表示**。字节码直接被"边分析边拼字符串"翻成 LLVM 文本,
所有脆性都源于此。

---

## 1. 设计目标与非目标

### 目标
1. **可 lower 判定 = 一个总函数**:`fn lower(artifact) -> Result<Mir, Unsupported>`,
   构造成功即保证发射不会中途失败。发射(`Mir -> LLVM`)是 total。
2. **发射类型安全**:SSA 值、类型、基本块都是 Rust 类型,不是字符串。
3. **ABI 单一真相**:一处声明 → 同时生成 lkrt 导出 + codegen 声明 + 元数据,零手工双写。
4. **句柄化高性能运行时**:容器 = lkrt 拥有的可增长 typed handle(Vec/hashbrown),
   无 4096 上限、真哈希、可释放。
5. **优雅/现代**:`Result` + 错误枚举取代 `bail!(String)`;快照测试 MIR(稳定)取代
   IR 子串断言(脆);differential 测试(AOT vs VM)作为一等公民。

### 非目标
- 不改前端(tokenizer/parser/macro/resolve/typeck)与字节码格式。
- 不引入静默的 generic runtime-value 回退(文档 §5.3 铁律)。
- 不在本 RFC 内实现闭包/间接调用 ABI —— 但新结构要**为它们留好扩展点**(见 §3.1)。
- 不强制引入 `inkwell`(见 §3.2 权衡)。

---

## 2. 目标架构总览

```
 字节码 ModuleArtifact
        │
        ▼   lk-aot-lower  (总函数,失败即给出 Unsupported)
   ┌─────────────┐
   │  AOT-MIR    │  类型化、SSA、容器=handle、调用已解析、语义已对齐(除零守卫等)
   └─────────────┘
        │
        ▼   lk-aot-codegen (total: MIR -> 原生 object)
   ┌─────────────┐
   │ clif.rs     │  Cranelift FunctionBuilder(typed Value/Block/Type)+ verifier,
   │ (Cranelift) │  发射校验过的原生 relocatable object(历史上为 IrBuilder → .ll 文本)
   └─────────────┘
        │
        ▼   clang 仅作链接驱动(原字符串 IR 时期用 clang/opt 编译 .ll)
      native exe  ──链接──▶  liblkrt.a
                                 ▲
        lk-aot-abi (单一真相 schema) ──生成──┤ codegen 侧 declare
                                            └ lkrt 侧 #[no_mangle] wrapper + 注册
```

**关键分层**:`lower`(可能失败,产出 MIR)与 `codegen`(不失败,消费 MIR)彻底分离。
MIR 是稳定的、可快照、可 diff 的中间产物。

---

## 3. 核心设计

### 3.1 `lk-aot-mir`:类型化中间表示

一个不依赖 LLVM 的、SSA 形式的中间层。字节码的寄存器/常量池被 lower 成带类型的值。

```rust
// crate: lk-aot-mir
pub struct MirModule {
    pub abi_version: u32,
    pub functions: Vec<MirFunction>,
    pub entry: FuncId,
    pub globals: Vec<MirGlobal>,      // 字符串常量、格式串等
}

pub struct MirFunction {
    pub id: FuncId,
    pub params: Vec<(ValueId, Ty)>,   // 为将来 native fn ABI 预留(现阶段 entry 为空)
    pub blocks: Vec<Block>,
    pub entry_block: BlockId,
    pub ret: Ty,
}

pub struct Block { pub id: BlockId, pub params: Vec<(ValueId, Ty)>, pub insts: Vec<Inst>, pub term: Term }

/// 类型是**封闭枚举**——这就是可 lower 子集的形式化定义。
pub enum Ty {
    I64, F64, Bool, Str,               // 标量
    List(ElemTy),                      // handle
    Map(KeyTy, ElemTy),                // handle
    Nil,
    // 扩展点:Closure { env: Vec<Ty>, sig: SigId }, FnPtr(SigId) —— 见 §7 阶段 4
}

pub enum Inst {
    // 纯标量:codegen 直接映射 LLVM;除零已在 lower 期决定用 checked helper
    BinI64 { dst: ValueId, op: IntOp, lhs: ValueId, rhs: ValueId },  // op::Div/Mod 携带 checked 标记
    BinF64 { .. },
    // 容器:一律是对 ABI intrinsic 的调用,不再逐 shape 生成 IR
    Call { dst: Option<ValueId>, callee: AbiFn, args: Vec<ValueId> },
    // 显示/模板
    FmtI64 { dst: ValueId, src: ValueId },  // 内部用 lkrt_i64_decimal_len 等
    ...
}

pub enum Term { Ret(Option<ValueId>), Br(BlockId, Vec<ValueId>), CondBr { .. }, Abort(DiagId) }
```

要点:
- **`Ty` 是封闭枚举 = 可 lower 子集的定义**。lower 时遇到无法归到 `Ty` 的东西 → 返回
  `Unsupported`。发射时 `Ty` 已保证被覆盖(`match` 穷尽)。
- **容器统一为 `Call { callee: AbiFn, .. }`** —— 布局×方法的组合在 lower 期解析成一个
  `AbiFn`(见 §3.3),codegen 只发一条 `call`。彻底消灭 `dynamic_containers/` 的逐 shape。
- **语义对齐在 lower 期完成**:除零 → `IntOp::Div{checked:true}` → codegen 发
  `@lkrt_i64_div_checked`;present-bit、字符串所有权同理落到 `AbiFn` 选择上。
- **块参数(block args)替代裸 phi**:用 SSA 块参数,codegen 再降成 phi。消除手写 phi 的
  记账地狱(`emit_string_map_delete` 曾经的 `%label.dst.i` phi)。

### 3.2 `lk-aot-codegen`:结构化 SSA builder

发射不再 `push_str` 裸串,而是通过一个拥有类型/值/块的 builder;**只有这一层知道 LLVM**。

```rust
pub struct IrBuilder { /* 值→类型表、块表、SSA 名分配、字符串常量去重 */ }
impl IrBuilder {
    fn value(&mut self, ty: LlTy) -> Value;             // 返回类型化句柄,不是 %rN 字符串
    fn add(&mut self, a: Value, b: Value) -> Value;     // 校验 a.ty == b.ty
    fn call(&mut self, f: AbiFn, args: &[Value]) -> Option<Value>;  // 校验签名 arity/类型
    fn cond_br(&mut self, c: Value, t: BlockId, e: BlockId);
    fn finish(self) -> String;                          // 一次性输出**校验过**的 .ll 文本
}
```

**权衡:文本 builder vs `inkwell`**
- **推荐:自建文本 builder(保留 clang-on-.ll 管线)**。现管线 `native_executable.rs`
  用 clang 子进程编译 `.ll`,不链接 LLVM 库——构建轻、无版本地狱。自建 builder 保留这套,
  只在 crate 内加一层类型化 SSA 抽象 + 发射前 `debug_assert` 校验(类型匹配、块封闭、
  值有定义)。收益 90%,成本可控。
- **可选:`inkwell`**。得到真正的 verifier + 内建 opt pass + 无文本 UB,但链接 LLVM 库
  (版本 pin、构建重)。若未来 AOT 成为一等目标可切换;MIR 层不变,只换 codegen 后端。
  **MIR 的存在正是为了让 codegen 后端可替换。**

> **已落地(codegen 后端 = Cranelift):** 上述"文本 builder vs inkwell"的权衡已有第三条
> 出路并被采纳——**Cranelift**(`aot/codegen/src/clif.rs`)。它给了 inkwell 那样的真正
> **verifier + typed FunctionBuilder**(类型不匹配即编译期报错,而非文本 UB),却**不链接
> LLVM 库**(纯 Rust crate,无版本 pin、构建轻),正是本节推荐"文本 builder"想要的收益。
> 代价是优化不及 clang `-O2`(AOT 运行时约 ~17%),换更强正确性网 + 更快编译。字符串 IR
> 的 `IrBuilder`/`render_module` 已删除;`MIR → Cranelift IR → 原生 object → 链接 lkrt`
> 为唯一原生 codegen。上面 `IrBuilder` 代码块保留为历史设计。

### 3.3 `lk-aot-abi`:ABI 单一真相

一张声明式表,`build.rs` 或 proc-macro 生成三份产物,消除 `intrinsics.rs`/`lib.rs`/
`containers.rs` 的手工双写。

```rust
// lk-aot-abi/abi.ron (或 const 表)
abi_fn! {
    name: "lkrt_map_str_int_lookup",
    module: "map.str", effect: Pure,
    params: [Ptr, Ptr, Ptr, I64, StrPtr, I64, OutPtr(I64)],
    ret: I64,   // present-bit
}
```
生成:
1. **codegen 侧**:`AbiFn` 常量 + `declare` 文本(替代 `intrinsics.rs` 的手写数组)。
2. **lkrt 侧**:`#[no_mangle] extern "C"` wrapper 骨架(签名由表保证,body 引用泛型实现),
   替代 `lib.rs` 的 `pub use` 长列表 + 手对齐签名。
3. **ABI 版本**:表变更 → `abi_version` bump;生成的 `main` 序言 assert
   `lkrt_abi_version() == <编译期常量>`,不等即 abort(落实 `native-stdlib.md` §5.2)。

`lk-aot-abi` **零依赖 core/stdlib/LLVM**,可被 codegen 和 lkrt 同时依赖。

### 3.4 运行时:typed handle 容器 + 所有权

用 lkrt 拥有的可增长容器替代 emit 侧定长 `[4096 x T]`。

```rust
// lkrt/src/containers/ —— 内部真实数据结构
#[repr(transparent)] pub struct LkList(*mut ListRepr);   // opaque handle
enum ListRepr { I64(Vec<i64>), F64(Vec<f64>), Str(Vec<CString>) }  // 单态,无 4096 上限

#[no_mangle] pub extern "C" fn lkrt_list_i64_new() -> LkList;
#[no_mangle] pub extern "C" fn lkrt_list_i64_push(h: LkList, v: i64);   // 摊还 O(1)
#[no_mangle] pub extern "C" fn lkrt_list_i64_get(h: LkList, i: i64, out: *mut i64) -> i64;
// map: hashbrown::HashMap，真 O(1),复合 key 不再 prefix+number 线性探查
```

**收益**:消除 4096 上限(正确性)、map O(1)(性能)、真所有权(内存)。

**所有权模型**(分级):
- **默认 arena**:程序退出时统一释放(短命脚本,零 per-op free 成本)。
- **可选 scope/RAII**:MIR 带逃逸/生命周期信息时,codegen 在作用域末尾发 `drop`;
  长驻程序(server)不漏。这依赖 §3.1 MIR 能表达"值不再活"。
- `lkrt_string_free` 真正接入(现在零调用)。

**性能补充**:非逃逸小容器可由 MIR 逃逸分析改为栈/bump 分配;`Pure` intrinsic 标记
让 LLVM 放心做 CSE/hoist。

### 3.5 `Unsupported`:错误枚举 + 总函数

```rust
pub enum Unsupported {
    EntryHasParams(u32), EntryHasCaptures(u32),
    RuntimeGlobal(GlobalId),
    DynamicCallTarget { pc: usize, reg: u8 },
    OpcodeNotLowerable { pc: usize, op: Opcode },
    // ...
}
impl Unsupported { pub fn reason(&self) -> String { /* 面向用户的解释 */ } }
```
取代 `diagnostics.rs` 的 `format!` 散串:枚举可穷尽匹配、可测试、可生成"支持矩阵"文档,
且**同一枚举既驱动诊断也驱动 backend.md 的自动生成**。

---

## 4. 项目结构

现状 `llvm` 单 crate 4.8 万行,职责混杂。拆为职责单一的 crate:

```
crates(新增/重构):
  lk-aot-abi/        # ABI schema 单一真相;零依赖;生成 codegen decl + lkrt wrapper
  lk-aot-mir/        # 类型化 MIR + 类型定义;依赖 lk-core(仅读字节码类型)
  lk-aot-lower/      # ModuleArtifact -> Result<Mir, Unsupported>(总函数,含语义对齐)
  lk-aot-codegen/    # Mir -> 原生 object(Cranelift,clif.rs;原 IrBuilder → LLVM 文本);依赖 lk-aot-abi
  lk-aot/            # 编排:lower -> codegen -> clang 链接驱动(现 backend.rs + native_executable.rs)
  lkrt/              # 运行时静态库;依赖 lk-aot-abi(仅 schema);铁律:不依赖 core/stdlib
```

`cli` 的 `llvm` feature 改指 `lk-aot`。`llvm` 老 crate 逐步清空到上述 crate(见 §7)。

**模块层内(以 lk-aot-lower 为例)**:
```
lower/
  scalar.rs      # 标量 op -> Inst(除零→checked)
  containers.rs  # 容器 op -> Call{AbiFn}(取代 dynamic_containers/ 全部逐 shape)
  control.rs     # 块/分支 -> Block/Term(SSA 块参数)
  calls.rs       # 直接调用解析;间接/闭包在此返回 Unsupported(扩展点)
  facts.rs       # 类型事实(移自 core::vm::analysis / 现 scalar/facts)
```

---

## 5. 性能设计要点

1. **容器**:hashbrown map(O(1))、Vec list(摊还 O(1)、无 4096 墙);单态化保留(每
   元素类型一份 `Vec<T>`),但由 §3.3 泛型 + §3.3 handle 承载,零 IR 膨胀。
2. **`Pure` 语义**:ABI 表标注 `Pure` 的 intrinsic 允许 LLVM CSE/hoist/DCE。
3. **逃逸分析(MIR 层)**:非逃逸容器栈/bump 分配,省 malloc/free。
4. **除零 checked helper 内联友好**:标记 `#[inline]` + `Pure`(除 abort 分支),
   LLVM 可把已知非零除数的守卫优化掉。
5. **发射一次成型**:builder 内部用 `String` 预留容量 + 一次 `finish`,不反复重扫。
6. **保留 dist profile 与 perf 门禁**:重构**不得**触碰 core/VM 解释器(perf 门禁测的是
   VM LK/Lua 比);AOT 侧性能用 `bench RUN_AOT=1` + 与句柄化前基线对比。

---

## 6. 测试策略

| 层 | 方法 | 取代 |
| --- | --- | --- |
| MIR | 快照测试(insta):字节码 → MIR 文本快照,稳定可 review | —— |
| codegen | golden `.ll`(结构稳定)+ builder 单元校验(类型/arity) | 现 `tests/*.rs` 的 IR 子串断言(脆) |
| lkrt | 现有隔离单测(保留) + `LkList/LkMap` 增长/别名/所有权 | —— |
| 端到端 | **differential harness**:artifact → native 运行 vs VM 运行,逐项比对 | 现在缺(map 路径无端到端) |
| 可 lower | `Unsupported` 枚举穷尽性 + 每 reason 一个反例用例 | `diagnostics.rs` 字符串断言 |

differential harness 直接解决"emit 签名 == helper 签名 == 运行结果 == VM"这条现在断开的链。

---

## 7. 迁移路径(增量、可回退,不做大爆炸重写)

4.8 万行不能一次推翻。**MIR 作为绞刑架(strangler)插在中间**,逐块把发射从"旧文本路径"
切到"MIR→builder 路径",每步保持 `cargo test --workspace` 全绿。

- **阶段 0 — 地基(低风险)**
  - 建 `lk-aot-abi`,把现 `intrinsics.rs` 数组迁为 schema,生成 codegen decl;lkrt wrapper
    先手保留、逐步转生成。**先落地本 RFC 里最便宜的正确性项:除零 checked helper**
    (`lkrt_{i64,f64}_{div,mod}_checked`)+ ABI 版本 assert。
- **阶段 1 — MIR + builder 骨架**
  - 建 `lk-aot-mir`/`lk-aot-codegen`;先覆盖**标量直线函数**(现 `straightline_main.rs`),
    走 MIR→builder,与旧路径 golden 对拍一致后切换。
- **阶段 2 — 容器全量迁 MIR**
  - `dynamic_containers/` 全部改为 lower 期产 `Call{AbiFn}`;同时把运行时换成句柄化容器
    (§3.4),差分测试守正确性。删除逐 shape 发射代码(预计减 ~2 万行)。
- **阶段 3 — 控制流/块**
  - `scalar/blocks/` 迁 MIR 块参数;删手写 phi/label 记账。
- **阶段 4 — 能力扩展(独立项,建 MIR 后才低成本)**
  - 在 `Ty` 加 `Closure`/`FnPtr`,`lower/calls.rs` 实现 native fn ABI → 闭包 → 间接调用
    (对应 aot-gaps §2.1 四大硬限制)。MIR 已就位,新增能力是"加枚举分支 + 一段 lower",
    不再是"再写一堆文本 IR"。

每阶段可独立合并、独立回退;旧路径在被完全替换前保留为 fallback 对拍基准。

---

## 8. 现有文件 → 新结构映射

| 现在 | 去向 |
| --- | --- |
| `llvm/src/llvm/intrinsics.rs` | `lk-aot-abi`(schema)+ `lk-aot-codegen`(生成 decl) |
| `llvm/src/llvm/diagnostics.rs` | `lk-aot-lower`(`Unsupported` 枚举 + 总函数) |
| `llvm/src/llvm/scalar/facts.rs` | `lk-aot-lower/facts.rs` |
| `llvm/src/llvm/scalar/**`(2.4 万行文本发射) | `lk-aot-lower`(→MIR) + `lk-aot-codegen`(→LL);大幅缩减 |
| `llvm/src/llvm/dynamic_containers/**` | 删除;lower 期产 `Call{AbiFn}`,运行时进 `lkrt/containers/` |
| `llvm/src/llvm/straightline_*`、`subfunction/**`、`output/**` | 拆入 `lower` + `codegen` |
| `llvm/src/native_executable.rs` | `lk-aot`(编排,基本不变) |
| `lkrt/src/containers.rs`(定长 helper) | `lkrt/src/containers/`(句柄化 + 增长 + 所有权) |
| `llvm/src/llvm/tests/*`(IR 子串断言) | MIR 快照 + golden `.ll` + differential harness |

---

## 9.5 实现进度

- **阶段 0 — 地基:✅ 已落地(1613 workspace 测试全绿)。**
  - 新建 `lk-aot-abi`(`aot/abi/`,零依赖):`AbiType`/`AbiEffect`/`AbiFn` + `ABI_VERSION`
    + `ABI_FUNCTIONS` 表(迁自 `intrinsics.rs`)+ `find()`。`llvm/intrinsics.rs` 缩为薄适配
    (re-export + 保留 LLVM 特定的 `declarations()`/`llvm_type()`);`lkrt` 复用
    `lk_aot_abi::ABI_VERSION`(消除自带 `= 1`)。**单一真相已就位**。
  - 除零守卫:`lkrt/src/arith.rs` 的 `lkrt_{i64,f64}_{div,mod}_checked`(rhs==0 → abort,
    `i64::MIN/-1` 用 wrapping 避免 UB);emit 侧主标量 int/float 路径 + 混合浮点 + call-slot
    int 改调 helper。端到端验证:`x/0` 在 VM 报错 exit 1、native 确定性 abort exit 134
    (此前为 `sdiv i64 x,0` UB;float `/0` 此前静默给 inf)。callee_eval/straightline_main
    仍走"proven-nonzero 才发 sdiv"(安全,MIN/-1 残留留待 MIR 统一)。
  - ABI 版本 assert:`lkrt_abi_check(expected)`,`main` 入口无条件 `call`(不改 entry CFG,
    保住 `[x, %entry]` phi);链接的 stale `lkrt` → abort 报错。
- **阶段 1 — MIR + codegen 地基:✅ 已落地并端到端验证。**
  - `lk-aot-mir`(`aot/mir/`,仅依赖 abi):类型化 SSA 数据模型(`Ty`/`Const`/`Inst`/
    `Term`/`Block`/`MirFunction`/`MirModule`,SSA **block 参数**替代裸 phi;容器/host 操作
    统一为 `Inst::Call{AbiRef}`)+ `validate()`(单赋值 / 先定义后用 / 分支目标 / ABI 可解析)。
    `Ty` 是**封闭枚举 = 可 lower 子集的定义**。
  - `lk-aot-codegen`(`aot/codegen/`,依赖 abi+mir):**total** `render_module(&MirModule)
    -> String`;唯一知道 LLVM 语法的地方;ABI `declare` 渲染迁到此(按 RFC 归属);
    block 参数→phi(扫描前驱分支实参);Div/Mod→guarded helper;entry→`@main` 打印返回值 +
    `lkrt_abi_check`。
  - **端到端验证**:`aot/codegen/examples/demo.rs` 渲染 `20/4` MIR → `clang` 链接
    `liblkrt.a` → 运行输出 `5`,`@lkrt_i64_div_checked`/`lkrt_abi_check` 真实执行。
    mir 4 + codegen 2 单测(含 golden IR:guarded div、block-param phi)。
- **阶段 1b — `lk-aot-lower`(bytecode→MIR 桥):✅ 首个 strangler 切片已落地。**
  - `aot/lower`(`lk-aot-lower`,依赖 lk-core+mir):`lower(&ModuleArtifact) ->
    Result<MirModule, Unsupported>`。**总函数**:能力判定就是它——构造成功即保证 codegen 不崩,
    否则返回带 pc/opcode 的精确 `Unsupported`(枚举,可穷尽/可测)。
  - 切片范围(持续扩):无参无捕获单入口、**标量直线全覆盖** —— int/float/bool/nil 常量、
    int/float 算术(含立即数 AddIntI/MulIntI/ModIntI)、int 比较(→Bool)、Move、Return。
    寄存器带**类型追踪**((ValueId, Ty)),类型不匹配 → `Unsupported::TypeMismatch`;
    其余 opcode(控制流/调用/容器)→ `Unsupported`(调用方回退旧 text 后端)。
    CLI 端到端(`LK_AOT_MIR=1`)验证:`1.5+2.5`→4、`3<5`→true、`x*3+1`→31 均走新路径且 =VM。
  - **端到端(全链路在测)**:手写 `ModuleArtifact`(`20/4`)→ `lower` → `validate` →
    `render_module` → guarded-div LLVM(`lowers_straightline_integer_division`)。
  - **完整新 pipeline 骨架已打通**:`abi → mir → lower → codegen` 四 crate 就位,
    真实字节码可走全新路径产出正确 native 二进制。
- **阶段 1b(续)— 绞刑架 opt-in 切换:✅ 已接入。** `compile_module_artifact_to_llvm`
  入口:`LK_AOT_MIR=1` 且 `lower` 接受该 artifact 时走 `lower→codegen`,否则回退旧 text
  后端。默认关闭 → 251 llvm + 1622 workspace 测试零改动。验证:`LK_AOT_MIR=1 lk compile
  '20/4'` 产出 `ModuleID='lk_aot'` + `@lkrt_abi_check` 的新 IR,native 输出 `5`=VM;
  无 env 仍走旧后端(`lk_fib_iterative`)。
- **阶段 1b(续)— 控制流切片(acyclic + SSA 合并):✅ 已落地并端到端验证。** `lower` 支持
  `Test`/`BrTrue`/`BrFalse`/`Jmp` → MIR 块 + `CondBr`/`Br`(分支语义照抄旧后端 `test_targets`,
  零臆测)。**两遍 SSA 合并构造**:leader 升序=拓扑序(前向 CFG),Pass A 算每块 phi 参数
  (前驱分歧的 reg→block 参数,一致的继承支配定义),Pass B 回填分支实参。回边(loop)/
  非 Bool 条件/返回类型冲突 → 精确 `Unsupported`。端到端:`branch_demo` 的 `min`(if/else
  合并,含 `phi i64`)lower→clang→运行,`min(3,5)=3`/`min(5,3)=3`/`min(7,7)=7` 三方向全对。
- **阶段 1b(续)— 融合 compare-and-branch + 循环(Braun SSA):✅ 已落地并端到端验证。**
  融合 `TestXxxInt(I)`+`Jmp` → `FusedCmp` 终结符(消费 Jmp,`jump_when` 极性用交换目标实现)。
  SSA 构造改为 **Braun 按需构造**(`read/write_variable` + 未密封块 incomplete-phi + latch
  填充后密封回填),**统一处理 acyclic 合并与循环回边**(subsumes 之前的两遍法)。跳过 trivial-phi
  消除(正确非最简)。**端到端差分对拍 VM(`LK_AOT_MIR=1`,走新路径)**:`sum 1..100=5050`、
  `countdown=10`、`factorial=720`、**嵌套循环 `nested=25`** 全部 =VM。
- **阶段 1b(续)— 多函数直接调用(native function ABI):✅ I64 函数 + 递归已落地并差分验证。**
  lower 现下降**所有**函数为 `MirModule.functions`;`CallDirect`(寄存器窗口:`a`=dst、`b`=callee
  index、`c`=argc,实参在 `[a+1, a+1+argc)`)→ `Inst::CallFn`→codegen `call i64 @lk_fn_N`。
  ABI 切片:用户函数强制 `(i64,…)->i64`,参数/返回全程 `read_typed(I64)`/返回类型校验——不符
  即整模块**回退**(绝不误编译)。`LoadFunction`/`SetGlobal` 对直接调用为 no-op。**差分对拍 VM
  (`LK_AOT_MIR=1` 走新路径)**:`add=7`、`fact(6)=720`、`fib(10)=55`、`gcd(48,36)=12`、
  嵌套 `dbl(inc(5))=12` 全 =VM。**差分测试当场抓出并修复了一个 CallDirect 参数误读导致的段错误
  误编译**(印证"逐形状差分对拍"的必要性)。
- **阶段 2(最大一块)— 容器句柄化:🚧 地基已落地并差分验证。** 新增 `lkrt/src/lklist.rs` 的
  **growable `LkList<i64>` opaque handle**(`*mut Vec<i64>`,new/push/len/get;区别于旧的 caller
  预分配定长 `[4096 x T]`,无上限)+ ABI schema `list_h` 条目 + MIR `Ty::ListI64` + codegen。
  lower:常量 `List<i64>` 字面量(`LoadHeapConst`)→ 物化为 handle(new + 逐元素 push);`Len`
  → `lkrt_lklist_i64_len`。**差分对拍 VM(新路径)**:`[1,2,3,4].len()=4`、`[..5..].len()*2=10`。
  **⚠️ 关键正确性发现**:`GetList` 索引是 `Maybe<Int>`(负索引从末尾;越界→`Nil`,见
  `lklist` get 的 `present` 出参)——索引/set/迭代 + `LkMap`(hashbrown)+ f64/str 元素是后续片,
  须按 present-bit 建模并逐形状差分(含越界/负索引)。
  **GetList(常量在界内):✅** 可静态证明在界内的访问(const-物化列表已知长度 + 常量下标 ∈
  `[0,len)`)→ `lkrt_lklist_i64_at(handle, idx)`(干净 i64);lower 用 `const_int`/`list_len` 分析。
  差分:`xs[0]+xs[2]=40`、`xs[1]*xs[3]=525`、`len+xs[0]=4` =VM(NEW)。
  **GetList(动态下标)+ `Maybe<Int>` 模型:✅ 已落地并差分验证(正确性地雷已拆)。** 非"常量在界内"
  的下标 → `Inst::ListGetMaybe` → `lkrt_lklist_i64_get_pair(handle,index) -> {i64,i64}`(`#[repr(C)]`
  双 i64 **按值返回**,SysV rax:rdx = LLVM `{i64,i64}`,无 alloca/出参),VM 语义(负从末尾、越界→
  present=0)。`Ty::MaybeI64`(codegen `{i64,i64}`)。**唯一消费者=return**:extractvalue present 分支,
  present 打印元素,absent **只 `ret` 不打印**(VM 顶层 nil 返回打印空,已核实);Maybe 入算术/比较 →
  `TypeMismatch` **回退**(不做 eager-abort,不与 `return xs[越界]`→nil 分歧)。差分(全 =VM):
  `xs[2]=30`/`xs[1]=20`/越界 `xs[9]`→空/负 `xs[-1]`→30/常量越界 `xs[7]`→空 走 **NEW**;`xs[i]+5=25`、
  `if(xs[i]>15)` 正确回退 **OLD**。
  **循环建表:✅** 空 `[]`=空 ListI64 常量;死 `LoadString` no-op → `let xs=[]; while{xs.push(i)} len`
  全走 NEW(`build_loop=5`)。**list.push:✅**(`lkrt_lklist_{i64,f64}_push`,引用语义 + `list_len` 追踪)。
- **RFC 收官轮(全部落地,workspace 1684 测试全绿)**:
  - **§6 differential harness ✅ 一等公民**:`cli/tests/aot_differential_test.rs` —— 69 个语料用例
    (标量/控制流/函数/list/map/字符串)每例:VM 运行 vs MIR 管线 native 编译运行,stdout 与
    成功/失败逐项比对;`Path::New` 用例额外断言确实走了 MIR 管线(`ModuleID = 'lk_aot'`)。
  - **§6 MIR 快照 ✅**:`lk_aot_mir::render()`(稳定、无 LLVM 语法的行式文本)+
    `aot/lower/tests/mir_snapshots.rs`(真实源码 → 字节码 → MIR 文本 golden,6 个形状:
    直线除法 / if-else 合并 / 循环块参数 / 直接调用 / 列表物化+动态索引 / map 查找)。
  - **§3.3 单一真相闭环 ✅**:ABI 表重构为 `for_each_abi_fn!` **数据宏**(140 条),
    `ABI_FUNCTIONS` const 表与 lkrt 的 `abi_conformance_test` **从同一宏展开**——每个符号的
    存在性/`extern "C"`/arity 由 fn-pointer coercion 在**编译期**强制,参数/返回的寄存器类
    (i64/f64/ptr/void)由测试比对 schema;签名漂移不可能再活过 CI(StrPtr/Ptr 同 class:
    LLVM 不透明指针下调用约定相同,区分仅是文档)。
  - **§3.4 所有权模型 ✅**:默认 **arena** —— 所有运行时分配(字符串 + 容器句柄)在创建时注册
    (`arena_c_string`/`arena_handle` + 类型化 drop fn),codegen 在 entry 干净退出路径发
    `lkrt_cleanup()` 统一回收(打印之后);**`lkrt_string_free` 真正接入**——lower 对 concat 链
    中已知死亡的 display 临时串与中间累加串发 eager free(`to_display_str` 返回 fresh 标记)。
    长驻循环里逐次分配的中间串不再累积。
  - **§3.5 `Unsupported::reason()` ✅**:每变体一句面向用户的解释 + `Display`;双后端都拒时
    编译错误同时携带 legacy 诊断与 MIR 精确原因。
  - **Maybe<Str> ✅(元素类型矩阵补齐)**:`lkrt_lklist_str_get_pair -> {ptr,i64}` +
    `Ty::MaybeStr`/`ListGetMaybeStr`/`UnwrapMaybeStr`;`MaybePresent` 改携带 `maybe_ty`(三载体)。
    差分:动态索引 concat 循环 `"abc"`、越界→nil、负索引、`==nil` 分支全 =VM。
  - **翻默认 ✅(绞刑架主备易位,历史记录)**:MIR 管线**默认开**。当时曾提供
    `LK_AOT_MIR=0` / `LlvmBackendOptions::use_mir_pipeline=Some(false)` 退回 legacy 的
    过渡通道;**legacy 后端退役时这两个开关已一并删除,现在没有任何运行时回退**。34 个 llvm crate legacy-IR 结构断言测试 pin 到 legacy 选项(随删 legacy 一并退役);
    7 个 CLI 集成测试改写为 MIR 路径断言。**差分测试当场抓出并修复 legacy 后端一个真实分歧**:
    `return nil;` legacy native 打印 `nil`,VM 与 MIR 管线都打印空——改写后的 CLI 测试现在锁定
    正确(VM)行为。
- **RFC 状态:核心设计(§2-§6)全部落地;按 §1 非目标划界的剩余项**:
  1. 闭包/间接调用/可变全局(§7 阶段 4)= **本 RFC 明确的非目标**,扩展点已就位
     (`Ty` 封闭枚举加变体 + lower 加 arm 即可);`__lk_call_method` 动态方法分派
     (list `.sort()`/`.pop()` 等)同属此类,现回退 legacy。
  2. 删除 legacy text 后端(§7 "旧路径在被完全替换前保留为 fallback 对拍基准"):
     待 MIR 覆盖吸收 legacy 独有形状(方法分派/对象/try 等)后整体退役,连同其 34 个
     pinned 结构断言测试与 `dynamic_containers/`(预计 -2~4 万行)。
- **codegen 后端迁移到 Cranelift ✅(字符串 IR 渲染器退役)**:§3.2 预留的"codegen 后端可替换"
  兑现——`lk-aot-codegen` 的 `render_module`(MIR → LLVM 文本 → clang)由 **Cranelift**
  (`clif.rs`:`MIR → Cranelift IR(typed FunctionBuilder + verifier)→ 原生 object`)取代。
  - 覆盖顺序(strangler,`LK_AOT_CLIF` opt-in → 默认 → 唯一):标量/控制流/直接调用 → 字符串/
    全局/ABI 调用/PrintStr/entry-main → `Dyn`/`Maybe*` `{i64,i64}` 载体(`Slot` 双寄存器,
    x64 rax:rdx / AArch64 x0:x1 匹配按值结构体 ABI)→ 可变全局载体 → `Const::Nil`/`FnAddr` →
    `TraitDispatch` → `TryCall`(lkrt C `setjmp` trampoline,Cranelift 无法发 `returns_twice`)→
    `CallVm` hybrid 桥 → `Maybe<f64>` 载体(lkrt out-pointer shim,规避 `{double,i64}` 混合类
    聚合的跨平台返回寄存器差异)。`Inst` match 现已穷尽。
  - 差分对等:clif-only(`LK_AOT_NO_FALLBACK`)跑通全量 `aot_differential` + `aot_fuzz` +
    examples + hybrid,零回落零分歧;确定性例程 VM vs clif-native stdout+exit 43/43 逐字对齐。
  - 性能:bench(dist,workload suite)clif AOT/VM ≈0.336x vs 字符串 IR ≈0.288x —— 慢 ~17%
    (Cranelift `speed` vs clang `-O2`),仍 ~3x 快于 VM;换 typed-builder + verifier 正确性网 +
    更快编译(省 clang `-O2`)+ 纯 Rust codegen(不链接 LLVM 库)。
  - 删除:`render_module` + 全部 `render_*`、`lk compile llvm` 命令、`compile_*_to_llvm` 流水线、
    `LlvmModule/LlvmBackend/LlvmBackendOptions/OptLevel`、`compile_native_executable_from_llvm(_hybrid)`、
    `llvm/src/llvm/tests/*`、`--opt-level/--skip-opt/--target-triple` 参数(约 -3.2 千行)。
    `clang` 仅保留作链接驱动。

## 9. 一句话总结

**引入 `AOT-MIR` 这一层类型化中间表示,是解开当前所有脆性的总钥匙**:它把"能不能 lower"
变成可测试的总函数、把发射变成类型安全的 total 映射、把容器组合爆炸收敛成"lower 期选一个
`AbiFn`"、并为闭包/间接调用等能力扩展留出"加枚举分支"级别的低成本入口。ABI 单一真相消灭
双写漂移,句柄化运行时消灭 4096 墙与内存泄漏。迁移用绞刑架模式增量替换,全程 `cargo test`
绿、性能门禁不受影响。
