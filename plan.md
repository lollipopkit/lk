# LK VM 性能优化计划

目标：缩小与 Lua 5.5 的差距（当前几何均值 ~3.7x 落后）。所有优化聚焦架构/CPU/内存布局层面，参考 Lua 实现。

## 当前基线（RUNS=10）

几何均值比：**3.727x（LK/Lua）**，全部 workload 处于 behind 状态。

主要瓶颈：
- Val 枚举过大（24B），导致寄存器数组 cache density 低
- 每次函数调用需分配/回收独立的 `Vec<Val>`（reg_pool/reg_stack）
- 字符串等值比较走字节级比较，map key 查找慢
- 短字符串（常见 map key）仍走堆分配路径
- AotFunction 变体 padding 浪费 7 字节，拖大整体枚举

---

## Phase 1：Val 24B → 16B（#1 + #6）✅ 优先

**原因**：Val 是 VM 的基础数据单元，缩小 33% 直接改善所有 workload 的 i-cache/d-cache 命中率。

### 1a. Str(Arc<str>) → Str(ArcStr)

- `Arc<str>` 是 fat pointer（16B：ptr + len）；`arcstr::ArcStr` 是 thin pointer（8B）
- ArcStr 实现 `Deref<Target=str>`、`Borrow<str>`、`Hash`、`Eq`，API 几乎零变化
- Map key 从 `FastHashMap<Arc<str>, Val>` 改为 `FastHashMap<ArcStr, Val>`
  - HashMap 仍支持 `get("literal_str")` 查询（通过 `Borrow<str>` impl）
- 涉及文件：`core/src/val/`，`core/src/vm/`，`stdlib/src/`，`core/src/ast/`

### 1b. AotFunction(AotFunction) → AotFunction(Box<AotFunction>)

- `AotFunction { ptr: usize, arity: u8 }` 当前 16B（7B padding）
- 改为 `Box<AotFunction>` 后变体为 8B thin 指针
- AotFunction 仅在 LLVM 注册时创建，调用时读取，非热路径，额外一次 deref 无代价
- 涉及文件：`core/src/val/values/mod.rs`，`core/src/llvm/runtime.rs`

**预期效果**：sizeof(Val) = 24B → 16B，寄存器密度提升 33%，全部 workload 受益。

---

## Phase 2：统一扁平寄存器栈（#2）🔴 大重构

**原因**：当前每次函数调用通过 `VmNestedCallGuard` 做 `mem::take`/`Vec::push`，涉及多次堆分配和内存移动。Lua 使用单一连续栈，调用只是移动栈顶指针，零分配。

### 设计

```rust
pub struct Vm {
    // 单一扁平栈，所有帧共享
    stack: Vec<Val>,
    stack_top: usize,
    // 帧只记录元数据（不含寄存器数据）
    frames: Vec<CallFrameMeta>, // 增加 reg_base: usize 字段
    // 删除 reg_pool, reg_stack
    ...
}
```

- 调用新帧：`frame_base = stack_top; stack_top += n_regs; stack.resize(stack_top, Nil);`
- 返回：`stack_top = frame_base;`（仅更新指针，数据留在栈中待复用）
- 参数传递：caller 把 args 写到 `stack[frame_base..]`，无需复制
- RegisterWindowRef 改为 `Absolute(usize)`（绝对 stack 索引）

### 涉及改动

- `Vm` 结构体：删 `reg_pool`/`reg_stack`，加 `stack`/`stack_top`
- 删除 `VmNestedCallGuard`：调用帧通过 `stack_top` 分配/释放窗口
- `exec_inner`：帧 setup 改为栈分配
- `run_opcode_code`/`run_packed_code`：`regs: &mut Vec<Val>` → `regs: &mut [Val]`（frame 窗口）
- `RegisterWindowRef`：删 `StackIndex(usize)`，改为 `Base(usize)` 绝对偏移

**预期效果**：function-call-heavy workload（binary_search 3.4x、order_score_pipeline 4.6x）改善 20-30%。

---

## Phase 3：短字符串 Interning（#3）

**原因**：map key 查找走字节级哈希+比较；如果相同内容字符串指针相同，可退化为 O(1) 指针哈希。

### 设计

```rust
// core/src/val/values/intern.rs
static INTERN_TABLE: Lazy<DashMap<ArcStr, ArcStr>> = Lazy::new(DashMap::new);

pub fn intern(s: &str) -> ArcStr {
    // 仅对短字符串 intern（≤ 64 字节，覆盖大部分 map key）
    if s.len() > 64 { return ArcStr::from(s); }
    if let Some(entry) = INTERN_TABLE.get(s) { return entry.clone(); }
    let arc: ArcStr = ArcStr::from(s);
    INTERN_TABLE.insert(arc.clone(), arc.clone());
    arc
}
```

- `Val::str_intern(s: &str) -> Val` 构造经过 intern 的字符串 Val
- 编译器生成字符串常量时走 intern 路径（`consts` 数组里的字符串字面量）
- Map key 读取和写入走 intern 路径
- **相同内容字符串 → 相同 ArcStr 指针** → HashMap 仍按内容哈希，但 ArcStr 相等检测可以先做指针比较

**预期效果**：histogram_group_count（2.1x）、two_sum_map（1.25x）、string_key_hash（2.3x）明显改善。

---

## Phase 4：SSO 内联小字符串（#4）

**原因**：常见 map key（如 `"k_1"`、`"a,b"`、`"sum"`）≤ 7 字节，可以完全内联在 Val 里，零堆分配、零引用计数。

### 设计

```rust
/// 最多 7 字节的内联字符串（Copy，零分配）
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ShortStr {
    len: u8,
    data: [u8; 7],
}

impl ShortStr {
    pub fn new(s: &str) -> Option<Self> { // None if s.len() > 7
        if s.len() > 7 { return None; }
        let mut data = [0u8; 7];
        data[..s.len()].copy_from_slice(s.as_bytes());
        Some(Self { len: s.len() as u8, data })
    }
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.data[..self.len as usize]).unwrap()
    }
}

pub enum Val {
    ShortStr(ShortStr),  // 8B，Copy，≤7 char inline
    Str(ArcStr),         // 8B，thin Arc，任意长度
    Int(i64),
    ...
}
```

- 添加统一访问宏/方法：`val.as_str() -> Option<&str>`（ShortStr 和 Str 都返回 &str）
- `Val::from_str(s: &str) -> Val`：≤7 字节用 ShortStr，更长用 intern() → Str
- 更新所有 `match Val::Str(s)` 模式，添加 `| Val::ShortStr(s) => s.as_str()` 分支

**预期效果**：string/map workload 中短字符串创建从堆分配变为栈操作，histogram（2.1x）、two_sum（1.25x）、string_key_hash（2.3x）进一步改善。

---

## 实施状态

| 优化 | 状态 | 关键文件 |
|------|------|---------|
| #1+6: Val 24B→16B (ArcStr + Box<AotFunction>) | ✅ 完成 | `core/src/val/`, `core/src/llvm/` |
| #2: 扁平寄存器栈 | ✅ 完成 | `core/src/vm/vm/` |
| #3: 短字符串 interning | ✅ 完成 | `core/src/val/values/intern.rs` |
| #4: SSO ShortStr 变体 | ✅ 完成 | `core/src/val/values/mod.rs` |

---

## ShortStr 迁移进度（跨 crate 适配）

Phase 4 引入 `Val::ShortStr` 后，需将所有 `match Val::Str(s)` 模式更新为同时处理 ShortStr。

**通用修复策略**：使用 `val.as_str()` 替代 `Val::Str(s) => &**s`，使用 `val.as_str().map(ArcStr::from)` 替代 `Val::Str(s) => s.clone()`。

| 文件 | 状态 | 说明 |
|------|------|------|
| `core/src/val/values/mod.rs` | ✅ | `Val::from_str`、`Val::as_str` 实现 |
| `core/src/vm/context.rs` | ✅ | `core_call_method_builtin` 处理 ShortStr |
| `core/src/vm/lkb.rs` | ✅ | `encode_val` 处理 ShortStr |
| `core/src/**` | ✅ | 所有 core 文件修复完毕 |
| `stdlib/src/string.rs` | ✅ | 20+ 处 `Val::Str` 模式全部改为 `as_str()` |
| `stdlib/src/list.rs` | ✅ | `join()` 修复 |
| `stdlib/src/map.rs` | ✅ | `has/get/set/delete/insert/remove` 等修复 |
| `stdlib/src/os.rs` | ✅ | `env.get/set/unset`、`dir.list`、`exec` 修复 |
| `cargo test -p lk-core` | ✅ | 561 passed, 0 failed |
| `cargo test -p lk-cli --test lkb_cli_test` | ✅ | 8 passed, 0 failed |
| `cargo test --workspace` | ✅ | 全 workspace 测试通过 |

---

## 后续方向（未在本计划中）

- **NaN-boxing**：将所有 Val 压入 8 字节（需要 unsafe，LLVM 部分可行）
- **Arc → Rc**（单线程 VM 路径，减少原子操作）
- **Instruction fusion**：更多 peephole 优化（AddImmJmp 等已有基础）
- **JIT**：基于 LLVM 的运行时 JIT 编译热函数
