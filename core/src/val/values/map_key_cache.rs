use std::cell::Cell;
use std::hash::BuildHasher;

use arcstr::ArcStr;
use hashbrown::hash_map::RawEntryMut;
use rustc_hash::FxBuildHasher;

use crate::util::fast_map::FastHashMap;

use super::Val;

const CACHED_HASH_MIN_LEN: usize = 64;

#[derive(Clone, Copy, Default)]
struct LastKeyHash {
    ptr: usize,
    len: usize,
    first: u8,
    last: u8,
    hash: u64,
}

thread_local! {
    static LAST_KEY_HASH: Cell<LastKeyHash> = const { Cell::new(LastKeyHash {
        ptr: 0,
        len: 0,
        first: 0,
        last: 0,
        hash: 0,
    }) };
}

#[inline]
fn hash_str(key: &str) -> u64 {
    FxBuildHasher.hash_one(key)
}

#[inline]
fn cached_hash_str(key: &str) -> u64 {
    if key.len() < CACHED_HASH_MIN_LEN {
        return hash_str(key);
    }
    let bytes = key.as_bytes();
    let ptr = key.as_ptr() as usize;
    let len = bytes.len();
    let first = bytes[0];
    let last = bytes[len - 1];
    LAST_KEY_HASH.with(|slot| {
        let cached = slot.get();
        if cached.ptr == ptr && cached.len == len && cached.first == first && cached.last == last {
            cached.hash
        } else {
            let hash = hash_str(key);
            slot.set(LastKeyHash {
                ptr,
                len,
                first,
                last,
                hash,
            });
            hash
        }
    })
}

#[inline]
fn fresh_cached_hash_str(key: &str) -> u64 {
    let hash = hash_str(key);
    if key.len() >= CACHED_HASH_MIN_LEN {
        let bytes = key.as_bytes();
        let len = bytes.len();
        LAST_KEY_HASH.with(|slot| {
            slot.set(LastKeyHash {
                ptr: key.as_ptr() as usize,
                len,
                first: bytes[0],
                last: bytes[len - 1],
                hash,
            });
        });
    }
    hash
}

#[inline]
pub(super) fn cache_fresh_str_hash(key: &str) {
    if key.len() >= CACHED_HASH_MIN_LEN {
        fresh_cached_hash_str(key);
    }
}

impl Val {
    #[inline]
    pub fn map_get_str<'a>(map: &'a FastHashMap<ArcStr, Val>, key: &str) -> Option<&'a Val> {
        if key.len() >= CACHED_HASH_MIN_LEN {
            let hash = cached_hash_str(key);
            if let Some((_, value)) = map.raw_entry().from_hash(hash, |candidate| candidate.as_str() == key) {
                return Some(value);
            }
        }
        map.get(key)
    }

    #[inline]
    pub fn map_contains_str(map: &FastHashMap<ArcStr, Val>, key: &str) -> bool {
        if key.len() >= CACHED_HASH_MIN_LEN {
            let hash = cached_hash_str(key);
            if map
                .raw_entry()
                .from_hash(hash, |candidate| candidate.as_str() == key)
                .is_some()
            {
                return true;
            }
        }
        map.contains_key(key)
    }

    #[inline]
    pub fn map_insert_arcstr(map: &mut FastHashMap<ArcStr, Val>, key: ArcStr, value: Val) -> Option<Val> {
        if key.len() < CACHED_HASH_MIN_LEN {
            return map.insert(key, value);
        }

        let hash = fresh_cached_hash_str(key.as_str());
        match map
            .raw_entry_mut()
            .from_hash(hash, |candidate| candidate.as_str() == key.as_str())
        {
            RawEntryMut::Occupied(mut entry) => Some(entry.insert(value)),
            RawEntryMut::Vacant(entry) => {
                entry.insert_hashed_nocheck(hash, key, value);
                None
            }
        }
    }

    #[inline]
    pub fn map_remove_str(map: &mut FastHashMap<ArcStr, Val>, key: &str) -> Option<Val> {
        if key.len() >= CACHED_HASH_MIN_LEN {
            let hash = cached_hash_str(key);
            if let RawEntryMut::Occupied(entry) = map
                .raw_entry_mut()
                .from_hash(hash, |candidate| candidate.as_str() == key)
            {
                return Some(entry.remove());
            }
        }
        map.remove(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::fast_map::fast_hash_map_with_capacity;

    #[test]
    fn cached_long_key_lookup_matches_hashmap_lookup() {
        let long_key = ArcStr::from("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789--long-key");
        let mut map = fast_hash_map_with_capacity(1);
        Val::map_insert_arcstr(&mut map, long_key.clone(), Val::Int(7));

        assert_eq!(Val::map_get_str(&map, long_key.as_str()), Some(&Val::Int(7)));
        assert!(Val::map_contains_str(&map, long_key.as_str()));
        assert_eq!(Val::map_get_str(&map, "missing-key"), None);
    }

    #[test]
    fn cached_long_key_insert_replaces_existing_entry() {
        let long_key = ArcStr::from("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789--replace-key");
        let mut map = fast_hash_map_with_capacity(1);

        assert_eq!(Val::map_insert_arcstr(&mut map, long_key.clone(), Val::Int(1)), None);
        assert_eq!(
            Val::map_insert_arcstr(&mut map, long_key.clone(), Val::Int(2)),
            Some(Val::Int(1))
        );

        assert_eq!(Val::map_get_str(&map, long_key.as_str()), Some(&Val::Int(2)));
        assert_eq!(map.get(long_key.as_str()), Some(&Val::Int(2)));
    }

    #[test]
    fn cached_long_key_remove_matches_hashmap_remove() {
        let long_key = ArcStr::from("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789--remove-key");
        let mut map = fast_hash_map_with_capacity(1);
        Val::map_insert_arcstr(&mut map, long_key.clone(), Val::Int(9));

        assert_eq!(Val::map_remove_str(&mut map, long_key.as_str()), Some(Val::Int(9)));
        assert_eq!(Val::map_get_str(&map, long_key.as_str()), None);
        assert_eq!(Val::map_remove_str(&mut map, long_key.as_str()), None);
    }

    #[test]
    fn concatenated_long_key_seeds_lookup_cache() {
        let key = Val::concat_strings(
            "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ",
            "0123456789--concat-key",
        );
        let Val::Str(key) = key else {
            panic!("expected long concat to produce ArcStr");
        };
        let mut map = fast_hash_map_with_capacity(1);
        Val::map_insert_arcstr(&mut map, key.clone(), Val::Int(11));

        assert_eq!(Val::map_get_str(&map, key.as_str()), Some(&Val::Int(11)));
        assert!(Val::map_contains_str(&map, key.as_str()));
    }
}
