# AOT 后端重设计 RFC

> 状态:提案 / 设计记录。目标是把当前 LLVM AOT 后端从"文本 IR 拼接 + 分析发射交织 +
> 逐 shape 手写"重构为"类型化中间表示(MIR)+ 结构化 SSA 发射 + 单一真相 ABI +
> 句柄化运行时"。要求:**高性能、现代设计规范、清晰项目结构、优雅**。
>
> 关联:能力边界与拒绝原因见 [`aot-gaps-and-lkrt.md`](./aot-gaps-and-lkrt.md);
> 现行 ABI 约束见 [`native-stdlib.md`](./native-stdlib.md);已支持形状见 [`backend.md`](./backend.md)。

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
        ▼   lk-aot-codegen (total: MIR -> LLVM)
   ┌─────────────┐
   │ IrBuilder   │  结构化 SSA builder(拥有 Value/BlockId/Type),emit 校验过的 .ll 文本
   └─────────────┘
        │
        ▼   clang/opt 子进程(不变)
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
  lk-aot-codegen/    # Mir -> LLVM 文本(IrBuilder);唯一知道 LLVM 的地方;依赖 lk-aot-abi
  lk-aot/            # 编排:lower -> codegen -> clang 链接(现 backend.rs + native_executable.rs)
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

## 9. 一句话总结

**引入 `AOT-MIR` 这一层类型化中间表示,是解开当前所有脆性的总钥匙**:它把"能不能 lower"
变成可测试的总函数、把发射变成类型安全的 total 映射、把容器组合爆炸收敛成"lower 期选一个
`AbiFn`"、并为闭包/间接调用等能力扩展留出"加枚举分支"级别的低成本入口。ABI 单一真相消灭
双写漂移,句柄化运行时消灭 4096 墙与内存泄漏。迁移用绞刑架模式增量替换,全程 `cargo test`
绿、性能门禁不受影响。
