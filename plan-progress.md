# LK VM 性能优化进度

## 目标

geomean ≤ 1.10x（vs Lua 5.5.0）；用户要求更强目标 <0.9x

## 当前状态 (2026-06-06)

### 最新 benchmark (20 samples, RUN_AOT=0, release build without vm-profile)

| Workload | Ratio | Confidence |
|----------|-------|------------|
| binary_search | **0.808x** | medium ✅ ahead |
| order_score | 1.054x | low ✅ close |
| cart_pricing | 1.391x | medium |
| log_parse | 1.453x | high |
| string_key | 1.580x | medium |
| prime_trial | 1.522x | low |
| stock_max | 1.487x | low |
| sliding_window | 1.756x | medium |
| fraud_rule | 1.811x | medium |
| gcd_batch | 1.847x | medium |
| matrix_3x3 | 1.545x | low |
| two_sum | 2.672x | medium |
| route_perm | 2.714x | low |
| inventory | 2.877x | medium |
| histogram | 2.997x | medium |
| **geomean** | **1.723x** | |

### 改进历史

| 改动 | geomean | 差异 |
|------|---------|------|
| 基线 (2026-06-04) | 2.235x | — |
| vm-profile feature gate 消除 metrics | ~1.82x | -18.5% |
| Opcode 重编号为连续值 | ~1.82x | 中性 |

### vm-profile counters (profile build, 1 sample)

| Workload | Opcodes | Branches | Containers | Map ops |
|----------|---------|----------|------------|---------|
| histogram | 9.08M | 864K | 742K | 0 |
| two_sum | 5.65M | 607K | 400K | 0 |
| inventory | 5.82M | 560K | 435K | 9K |
| route_perm | 2.22M | 388K | 152K | 0 |
| binary_search | 17.4M | 3.96M | 5 | 0 |

### 根本瓶颈分析

1. **dispatch overhead = ~2.0x** (纯循环微测) vs Lua computed goto
2. **Map/container 操作额外 0.7-1.3x**：Arc<str> 分配、heap 多层间接、dynamic type dispatch
3. **TypedMap.get_str Mixed 分支**双查找（先 ShortStr 再 Arc<str>）
4. **collect_metrics 开销已消除**：通过 vm-profile feature gate，默认 build 为 compile-time no-op

### 已完成的架构改变

#### vm-profile feature gate (completed)
- `lk-core/Cargo.toml`: 新增 `vm-profile = []` feature
- `lk-cli/Cargo.toml`: 透传 `vm-profile` feature
- `core/src/vm/analysis.rs`: 三路 cfg：
  - `#[cfg(test)]`: 线程本地计数器
  - `#[cfg(all(not(test), feature = "vm-profile"))]`: AtomicBool 门控 + atomic 计数器
  - `#[cfg(all(not(test), not(feature = "vm-profile")))]`: compile-time no-op（`false` 常量 + 空 record 函数）
- 效果：默认 release build 中 `vm_runtime_metrics_enabled()` 返回 `false` 常量，所有 `if collect_metrics { ... }` 和 `record_*` 调用被 LLVM 死代码消除

#### Opcode 连续重编号 (completed)
- 将 Opcode discriminant 从稀疏 (0-63 带 gap) 重编为连续 0-63
- 热路径 opcode 排前面：AddInt=1, SubInt=2, CmpLtInt=13, Test=17, Jmp=18
- `from_bits()` 从 57-arm match 改为连续 64-arm match，LLVM 可生成更好跳表
- 效果：中性（LLVM 已使用跳表 dispatch，连续编号主要简化验证路径）

## 下一步

要达到 ≤1.10x 目标，需要 ~40% 性能提升，增量优化不够。需要：

### P1: 拆分 dispatch loop 热路径
- 将 AddInt/SubInt/CmpLtInt/Test/Jmp 等热路径提取为 `#[inline(always)]` 小函数
- 对 dynamic fallback / 错误路径使用 `#[cold] #[inline(never)]`
- 每拆一组跑 benchmark 验证

### P2: Map/List 优化
- TypedMap.get_str Mixed 分支优化：避免双查找
- known_string_key 直接走 TypedMap 类型分支
- 消除热循环内 Arc<str> 分配

### P3: Template JIT / 热循环编译
- 编译热循环到原生代码（plan.md P2/P4）
- 复用 LLVM lowering 的 scalar block 能力
