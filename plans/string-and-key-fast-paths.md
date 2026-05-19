# String and Key Fast Paths Plan

## Reference Implementations

- QuickJS atoms: `references/quickjs/quickjs.h`, `references/quickjs/quickjs-atom.h`
- CPython string interning docs: `references/cpython/InternalDocs/string_interning.md`
- Luau string/table key ops: `references/luau/VM/src/lvmexecute.cpp`,
  `references/luau/Compiler/src/Compiler.cpp`
- Rhai SmartString: `references/rhai/src/lib.rs`, `references/rhai/src/engine.rs`
- LK current intern table: `core/src/val/values/intern.rs`

## Borrow

- Treat frequently repeated property/map keys as internable identities.
- Cache string hash or key identity in the lookup path when it avoids repeated
  content hashing/comparison.
- Use compact strings for short temporary keys.
- Compile known string-key access into a distinct operation.

## Do Not Borrow

- Do not make all strings globally immortal.
- Do not turn every string into an atom; long one-off strings should remain
  ordinary strings.
- Do not make map keys pointer-equality-only; content equality must remain
  correct.

## LK Landing

- Keep the current `ShortStr` and length-limited intern table.
- Add compiler/runtime distinction between:
  - literal string key
  - template-generated string key
  - dynamic arbitrary string key
- Add fast path for `Map` lookup by interned key:
  - if key is interned/literal, use cached hash or cached entry
  - otherwise fall back to normal content lookup
- Add a string builder path for template strings that pre-computes segment count
  and capacity when possible.
- Review `map.get`, `map.set`, `histogram_group_count`, `two_sum_map`, and
  `log_parse_filter` against this plan.

## Acceptance

- Bench focus: `two_sum_map`, `histogram_group_count`, `string_key_hash`,
  `log_parse_filter`, `fraud_rule_scoring`.
- Add tests for key equality across `ShortStr`, interned `Str`, and long `Str`.
- Verify long unique strings do not permanently fill the intern table.

