# List and Map Specialization Plan

## Reference Implementations

- CPython specialized subscription opcodes:
  `references/cpython/Include/internal/pycore_opcode_metadata.h`
- Luau table access bytecode:
  `references/luau/Compiler/src/Compiler.cpp`,
  `references/luau/VM/src/lvmexecute.cpp`
- Ruby benchmark categories:
  `references/ruby/benchmark/array_*`, `references/ruby/benchmark/hash_*`
- LK current list/map ops:
  `core/src/vm/vm/runtime/frame/run/opcode.rs`, `stdlib/src/list.rs`,
  `stdlib/src/map.rs`

## Borrow

- CPython's lesson is specific typed access: list-int, string-int, dict-like
  operations should become distinct hot paths.
- Luau's constant string and numeric table access opcodes are worth mirroring as
  separate LK operations.
- Ruby's benchmark taxonomy is useful for deciding which access patterns to test.

## Do Not Borrow

- Do not merge `List` and `Map` into one table type.
- Do not introduce Lua's array/hash split as the sole container model.
- Do not optimize only object-like property access while ignoring LK list-heavy
  workloads.

## LK Landing

- Add list specializations:
  - `IndexListInt`
  - `ListPushUnique`
  - `ListLen`
  - optional rolling-window friendly push/index sequence fusion
- Add map specializations:
  - `MapGetInternedStr`
  - `MapSetInternedStr`
  - `MapHasInternedStr`
  - `MapGetConstKey`
- Use copy-on-write rules consistently:
  - mutate in place when the backing storage is unique
  - clone only when shared
- Avoid caching stale list/map element values unless cache invalidation is exact.
  Prefer caching shape/key/index metadata over cached `Val` clones.

## Acceptance

- Bench focus: `sliding_window_sum`, `two_sum_map`, `histogram_group_count`,
  `inventory_reorder`, `fraud_rule_scoring`.
- Tests must cover mutation after cached access.
- Check that list/map public stdlib behavior remains unchanged.

