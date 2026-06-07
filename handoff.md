# Handoff — LK VM 性能优化 (2026-06-06)

## 目标
`plan.md` 要求：geomean ≤ 1.10x (vs Lua 5.5.0)；用户更强目标 <0.9x

## 当前状态 (2026-06-06)

### 性能基准 (20 samples, RUN_AOT=0, release build)

| Workload | Ratio | | Workload | Ratio |
|----------|-------|-|----------|-------|
| binary_search | **0.808x** ✅ | | stock_max | 1.487x |
| order_score | 1.054x ✅ | | fraud_rule | 1.811x |
| cart_pricing | 1.391x | | gcd_batch | 1.847x |
| log_parse | 1.453x | | route_perm | 2.714x |
| string_key | 1.580x | | two_sum | 2.672x |
| prime_trial | 1.522x | | inventory | 2.877x |
| matrix_3x3 | 1.545x | | histogram | 2.997x |
| sliding_window | 1.756x | | | |
| **geomean** | **1.723x** | | |

### 优化历史

| 改动 | geomean | 差异 |
|------|---------|------|
| 基线 (2026-06-04) | 2.235x | — |
| vm-profile feature gate 消除 metrics | ~1.82x | -18.5% |
| Opcode 连续重编号 (0-63) | ~1.82x | 中性 |
| #[cold] 标记动态回退/慢路径 | ~1.73x | -5.3% |
| callable/named_call #[cold] | ~1.73x | 中性 |
| **总计** | **1.723x** | **-22.9%** |

### 本轮完成的优化

#### 1. vm-profile feature gate (commit: 3e55281)

**问题**：`collect_metrics = vm_runtime_metrics_enabled()` 是 AtomicBool::load(Relaxed)，默认 false 但编译器无法证明。导致每个 opcode 的 `if collect_metrics { record_xxx(); }` 和 `record_xxx_known_enabled()` 调用无法被消除。

**方案**：三路 cfg：
- `#[cfg(test)]`: 线程本地计数器
- `#[cfg(all(not(test), feature = "vm-profile"))]`: AtomicBool 门控 + atomic 计数器
- `#[cfg(all(not(test), not(feature = "vm-profile")))]`: compile-time no-op

**效果**：
- 默认 release build 中 `vm_runtime_metrics_enabled()` 返回 `false` 常量
- 所有 `if collect_metrics { ... }` 和 `record_*` 调用被 LLVM 死代码消除
- Geomean: 2.235x → ~1.82x

**文件**：
- `core/Cargo.toml` — 新增 `vm-profile = []` feature
- `cli/Cargo.toml` — 透传 `vm-profile` feature
- `core/src/vm/analysis.rs` — 977 行 → 重写 metrics 部分为三路 cfg

#### 2. Opcode 连续重编号 (commit: 3e55281)

**问题**：Opcode discriminant 不连续（0-63 带 gap），`from_bits()` 是 57-arm match。
**方案**：重编号为 0-63 连续，热路径 opcode 排前面。
**效果**：中性（LLVM 已经使用跳表 dispatch，连续编号主要简化验证路径）

#### 3. #[cold] 标记慢路径 (commit: 2047119, a617c4e)

**问题**：run_function_inner 是巨型函数，热路径和冷路径（错误处理、dynamic fallback）混在一起，LLVM 无法有效分离指令缓存布局。

**方案**：给以下方法加 `#[cold]`：
- `dynamic_add/sub/div/mod`, `number_compare`, `values_equal`
- `runtime_value_is_list/map/display_string/from_string`
- `relative_pc/relative_pc_from`
- `get_heap_index_slow_path` 和 string/list index 慢路径
- `object_key_from_register`, `lookup_map_handle` 等容器慢路径
- `load_function_value`, `make_closure_value`, `load_native_value`, `call_function_named`

**注意**：不能给热路径加 `#[cold]`。已撤回对 `call_function`、`call_direct_function`、`collect_pending_garbage` 的标记。

**效果**：Geomean 1.82x → 1.73x（-5.3%），尤其是 arithmetic-heavy workload 改善明显。

### vm-profile 使用方法

```bash
# 构建 profile 版本
cargo build --release -p lk-cli --features vm-profile

# 运行 benchmark 带 profile
RUN_AOT=0 PROFILE_WORKLOADS=1 RUNS=1 EXTRA_RUNS=0 bash bench/run_workload_bench.sh
```

注意：普通 release build 的 `PROFILE_WORKLOADS=1` 会打印全零 counters，因为 metrics 是 compile-time no-op。

### 根本瓶颈（更新）

1. **dispatch overhead = ~1.7x**：每条指令 ~6 个内存访问（取指→解码→match→栈读取→写入→pc递增）
2. **Map/list 操作额外 0.7-1.3x**：TypedMap.get_str Mixed 分支双查找、Arc<str> 分配
3. **collect_metrics 已消除** ✅
4. **Opcode dispatch 已使用跳表** ✅
5. **慢路径已从热循环分离** ✅

### vm-profile counters 要点

- Copy profile counters 全是 1，说明 clone 开销已被前序改动压低
- Container ops 集中在 map-heavy workload（histogram 742K, two_sum 400K）
- Typed branches 占所有 branch 的 ~100%（fused branch 优化有效）

## 关键文件

- `core/Cargo.toml` — `vm-profile` feature
- `core/src/vm/analysis.rs` — 三路 cfg metrics 系统
- `core/src/vm/ir.rs` — Opcode 连续重编号 (0-63)
- `core/src/vm/exec.rs` — dispatch loop (1318 行)
- `core/src/vm/exec/arithmetic.rs` — #[cold] 动态回退
- `core/src/vm/exec/container/index.rs` — #[cold] 慢路径
- `core/src/vm/exec/value_ops.rs` — #[cold] 类型检查慢路径

## 下一步方向

### P1: 继续拆分 dispatch loop 热路径

按 STATUS.md P1 方针：
1. 将 AddInt/SubInt/CmpLtInt/Test/Jmp 等热路径提取为 tiny `#[inline(always)]` helper
2. 对 dynamic fallback / 错误路径已有 `#[cold]`
3. 主循环保留 opcode dispatch 和最短路径
4. 每拆一组跑 benchmark 验证

### P2: Map/List 优化

- TypedMap.get_str Mixed 分支优化：避免双查找（先 ShortStr 后 Arc<str>）
- known_string_key 直接走 TypedMap 类型分支
- 消除热循环内 Arc<str> 分配
- Register write 消除：对立即消费的临时值（compare 结果、index 结果）做 SSA level 消除

### P3: Typed branch lowering

- typed compare + branch 直接 lowering
- 避免比较结果 register 写入
- 通用控制流优化（避免 benchmark-shaped fused opcode）

### P4: Template JIT

如果坚持 geomean < 0.9x 目标，需要编译热循环到原生代码：
- 复用 LLVM lowering 的 scalar block 能力
- 保留 VM 解释器作为 fallback 和 correctness oracle

## 约束

- 不能 force push
- git commit msg 应该类似 `feat:`/`fix:`/`docs:` 等开头
- 除了 llvm 部分不能使用 unsafe
- 单文件不能超过 1500 行
- 不需要保持向前兼容性
