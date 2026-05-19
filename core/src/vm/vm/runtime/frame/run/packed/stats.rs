#[cfg(debug_assertions)]
use std::{
    collections::BTreeMap,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};

#[cfg(debug_assertions)]
use crate::vm::bc32;

#[cfg(debug_assertions)]
static PACKED_HOT_HITS: AtomicUsize = AtomicUsize::new(0);
#[cfg(debug_assertions)]
static PACKED_HOT_SENTINEL_SKIPS: AtomicUsize = AtomicUsize::new(0);
#[cfg(debug_assertions)]
static PACKED_HOT_BUILD_ATTEMPTS: AtomicUsize = AtomicUsize::new(0);
#[cfg(debug_assertions)]
static PACKED_HOT_BUILD_SUCCESSES: AtomicUsize = AtomicUsize::new(0);
#[cfg(debug_assertions)]
static PACKED_HOT_SENTINEL_TAGS: OnceLock<Mutex<BTreeMap<u16, usize>>> = OnceLock::new();

#[cfg(debug_assertions)]
pub(super) struct PackedHotStatsGuard {
    dump: bool,
}

#[cfg(not(debug_assertions))]
#[allow(dead_code)]
pub(super) struct PackedHotStatsGuard;

#[cfg(debug_assertions)]
impl PackedHotStatsGuard {
    pub(super) fn new() -> Self {
        let dump = std::env::var("LK_DUMP_PACKED_STATS")
            .ok()
            .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE"))
            .unwrap_or(false);
        Self { dump }
    }
}

#[cfg(not(debug_assertions))]
impl PackedHotStatsGuard {
    #[allow(dead_code)]
    pub(super) fn new() -> Self {
        Self
    }
}

#[cfg(debug_assertions)]
impl Drop for PackedHotStatsGuard {
    fn drop(&mut self) {
        if self.dump {
            let hits = PACKED_HOT_HITS.swap(0, Ordering::Relaxed);
            let sentinel_skips = PACKED_HOT_SENTINEL_SKIPS.swap(0, Ordering::Relaxed);
            let attempts = PACKED_HOT_BUILD_ATTEMPTS.swap(0, Ordering::Relaxed);
            let successes = PACKED_HOT_BUILD_SUCCESSES.swap(0, Ordering::Relaxed);
            eprintln!(
                "[packed-hot-cache] hits={} sentinel_skips={} build_successes={} build_attempts={}",
                hits, sentinel_skips, successes, attempts
            );
            if let Some(map) = PACKED_HOT_SENTINEL_TAGS.get() {
                let mut guard = map.lock().unwrap();
                if !guard.is_empty() {
                    eprintln!("[packed-hot-cache] sentinel breakdown:");
                    for (key, count) in guard.iter() {
                        let label = sentinel_label(*key);
                        eprintln!("{} => {}", label, count);
                    }
                    guard.clear();
                }
            }
        }
    }
}

#[inline]
pub(super) fn record_hot_hit() {
    #[cfg(debug_assertions)]
    PACKED_HOT_HITS.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub(super) fn record_sentinel_skip(_word: u32) {
    #[cfg(debug_assertions)]
    {
        PACKED_HOT_SENTINEL_SKIPS.fetch_add(1, Ordering::Relaxed);
        let word = _word;
        let raw_tag = bc32::tag_of(word);
        let key = if raw_tag == bc32::TAG_EXT {
            0x100 | (((word >> 16) & 0xFF) as u16)
        } else {
            raw_tag as u16
        };
        let map = PACKED_HOT_SENTINEL_TAGS.get_or_init(|| Mutex::new(BTreeMap::new()));
        let mut guard = map.lock().unwrap();
        *guard.entry(key).or_insert(0) += 1;
    }
}

#[cfg(debug_assertions)]
fn sentinel_label(key: u16) -> String {
    if (key & 0x100) != 0 {
        let ext_op = (key & 0xFF) as u8;
        return format!("  <Ext:{}:{}>", ext_op, ext_op_name(ext_op));
    }
    match bc32::decode_tag_byte(key as u8) {
        bc32::DecodedTag::Regular { tag, .. } => format!("  {:?}", tag),
        bc32::DecodedTag::RegExt => "  <RegExt>".to_string(),
        bc32::DecodedTag::Ext => format!("  <ExtTag:{}>", key),
    }
}

#[cfg(debug_assertions)]
fn ext_op_name(ext_op: u8) -> &'static str {
    match ext_op {
        bc32::EXT_OP_FLOOR => "Floor",
        bc32::EXT_OP_STARTS_WITH_K => "StartsWithK",
        bc32::EXT_OP_CONTAINS_K => "ContainsK",
        bc32::EXT_OP_TO_ITER => "ToIter",
        bc32::EXT_OP_MAP_HAS_K => "MapHasK",
        bc32::EXT_OP_ADD_INT => "AddInt",
        bc32::EXT_OP_ADD_FLOAT => "AddFloat",
        bc32::EXT_OP_SUB_INT => "SubInt",
        bc32::EXT_OP_SUB_FLOAT => "SubFloat",
        bc32::EXT_OP_MUL_INT => "MulInt",
        bc32::EXT_OP_MUL_FLOAT => "MulFloat",
        bc32::EXT_OP_DIV_FLOAT => "DivFloat",
        bc32::EXT_OP_MOD_INT => "ModInt",
        bc32::EXT_OP_MOD_FLOAT => "ModFloat",
        bc32::EXT_OP_LIST_LEN => "ListLen",
        bc32::EXT_OP_MAP_LEN => "MapLen",
        bc32::EXT_OP_STR_LEN => "StrLen",
        bc32::EXT_OP_LIST_INDEX_I => "ListIndexI",
        bc32::EXT_OP_STR_INDEX_I => "StrIndexI",
        bc32::EXT_OP_MAP_GET_INTERNED => "MapGetInterned",
        bc32::EXT_OP_MAP_SET_INTERNED => "MapSetInterned",
        bc32::EXT_OP_MAP_GET_DYNAMIC => "MapGetDynamic",
        bc32::EXT_OP_STR_CONCAT_KNOWN_CAP => "StrConcatKnownCap",
        bc32::EXT_OP_LIST_SET_I => "ListSetI",
        bc32::EXT_OP_CALL_NATIVE_FAST => "CallNativeFast",
        bc32::EXT_OP_CMP_I => "CmpI",
        bc32::EXT_OP_CALL_CLOSURE_EXACT => "CallClosureExact",
        bc32::EXT_OP_CALL_EXACT => "CallExact",
        bc32::EXT_OP_CALL_NAMED_FALLBACK => "CallNamedFallback",
        _ => "Unknown",
    }
}

#[inline]
pub(super) fn record_build_attempt() {
    #[cfg(debug_assertions)]
    PACKED_HOT_BUILD_ATTEMPTS.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub(super) fn record_build_success() {
    #[cfg(debug_assertions)]
    PACKED_HOT_BUILD_SUCCESSES.fetch_add(1, Ordering::Relaxed);
}
