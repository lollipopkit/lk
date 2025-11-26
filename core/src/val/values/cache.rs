use std::sync::{Arc, Weak};

use dashmap::{DashMap, mapref::entry::Entry};
use once_cell::sync::Lazy;

use crate::util::fast_map::{FastHashSet, fast_hash_set_with_capacity};

use super::Val;

const LIST_CACHE_MIN_LEN: usize = 64;

struct HomogeneousListCache<T> {
    list: Weak<[Val]>,
    set: FastHashSet<T>,
}

static LIST_INT_CACHE: Lazy<DashMap<usize, HomogeneousListCache<i64>>> = Lazy::new(DashMap::new);
static LIST_STR_CACHE: Lazy<DashMap<usize, HomogeneousListCache<Arc<str>>>> = Lazy::new(DashMap::new);
static LIST_BOOL_CACHE: Lazy<DashMap<usize, HomogeneousListCache<bool>>> = Lazy::new(DashMap::new);

fn cleanup_cache<T>(map: &DashMap<usize, HomogeneousListCache<T>>) {
    map.retain(|_, entry| entry.list.upgrade().is_some());
}

fn list_cache_key(list: &Arc<[Val]>) -> usize {
    Arc::as_ptr(list) as *const () as usize
}

fn cached_list_contains_int(list: &Arc<[Val]>, needle: i64) -> Option<bool> {
    if list.len() < LIST_CACHE_MIN_LEN {
        return None;
    }

    let key = list_cache_key(list);

    if let Some(entry) = LIST_INT_CACHE.get(&key) {
        let entry = entry.value();
        if entry.list.upgrade().is_some() {
            return Some(entry.set.contains(&needle));
        }
    }

    let mut set = fast_hash_set_with_capacity(list.len());
    for item in list.iter() {
        match item {
            Val::Int(v) => {
                set.insert(*v);
            }
            _ => return None,
        }
    }

    let contains = set.contains(&needle);

    cleanup_cache(&LIST_INT_CACHE);
    Some(match LIST_INT_CACHE.entry(key) {
        Entry::Vacant(slot) => {
            slot.insert(HomogeneousListCache {
                list: Arc::downgrade(list),
                set,
            });
            contains
        }
        Entry::Occupied(mut occupied) => {
            if occupied.get().list.upgrade().is_none() {
                occupied.insert(HomogeneousListCache {
                    list: Arc::downgrade(list),
                    set,
                });
                contains
            } else {
                occupied.get().set.contains(&needle)
            }
        }
    })
}

fn cached_list_contains_str(list: &Arc<[Val]>, needle: &Arc<str>) -> Option<bool> {
    if list.len() < LIST_CACHE_MIN_LEN {
        return None;
    }

    let key = list_cache_key(list);

    if let Some(entry) = LIST_STR_CACHE.get(&key) {
        let entry = entry.value();
        if entry.list.upgrade().is_some() {
            return Some(entry.set.contains(needle));
        }
    }

    let mut set = fast_hash_set_with_capacity(list.len());
    for item in list.iter() {
        match item {
            Val::Str(s) => {
                set.insert(s.clone());
            }
            _ => return None,
        }
    }

    let contains = set.contains(needle);

    cleanup_cache(&LIST_STR_CACHE);
    Some(match LIST_STR_CACHE.entry(key) {
        Entry::Vacant(slot) => {
            slot.insert(HomogeneousListCache {
                list: Arc::downgrade(list),
                set,
            });
            contains
        }
        Entry::Occupied(mut occupied) => {
            if occupied.get().list.upgrade().is_none() {
                occupied.insert(HomogeneousListCache {
                    list: Arc::downgrade(list),
                    set,
                });
                contains
            } else {
                occupied.get().set.contains(needle)
            }
        }
    })
}

fn cached_list_contains_bool(list: &Arc<[Val]>, needle: bool) -> Option<bool> {
    if list.len() < LIST_CACHE_MIN_LEN {
        return None;
    }

    let key = list_cache_key(list);

    if let Some(entry) = LIST_BOOL_CACHE.get(&key) {
        let entry = entry.value();
        if entry.list.upgrade().is_some() {
            return Some(entry.set.contains(&needle));
        }
    }

    let mut set = fast_hash_set_with_capacity(list.len());
    for item in list.iter() {
        match item {
            Val::Bool(b) => {
                set.insert(*b);
            }
            _ => return None,
        }
    }

    let contains = set.contains(&needle);

    cleanup_cache(&LIST_BOOL_CACHE);
    Some(match LIST_BOOL_CACHE.entry(key) {
        Entry::Vacant(slot) => {
            slot.insert(HomogeneousListCache {
                list: Arc::downgrade(list),
                set,
            });
            contains
        }
        Entry::Occupied(mut occupied) => {
            if occupied.get().list.upgrade().is_none() {
                occupied.insert(HomogeneousListCache {
                    list: Arc::downgrade(list),
                    set,
                });
                contains
            } else {
                occupied.get().set.contains(&needle)
            }
        }
    })
}

pub(super) fn cached_list_contains(list: &Arc<[Val]>, needle: &Val) -> Option<bool> {
    match needle {
        Val::Int(i) => cached_list_contains_int(list, *i),
        Val::Str(s) => cached_list_contains_str(list, s),
        Val::Bool(b) => cached_list_contains_bool(list, *b),
        _ => None,
    }
}

#[cfg(test)]
mod cache_tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn reuses_cached_int_membership() {
        LIST_INT_CACHE.clear();
        let list: Arc<[Val]> = (0..128).map(Val::Int).collect::<Vec<_>>().into();
        let needle = Val::Int(64);

        assert!(Val::list_contains(&list, &needle));
        assert_eq!(LIST_INT_CACHE.len(), 1);
        assert!(Val::list_contains(&list, &needle));
        assert_eq!(LIST_INT_CACHE.len(), 1);
    }

    #[test]
    fn cached_membership_scales_for_large_lists() {
        use std::time::{Duration, Instant};

        LIST_INT_CACHE.clear();
        let list: Arc<[Val]> = (0..100_000).map(Val::Int).collect::<Vec<_>>().into();
        let needle = Val::Int(99_999);

        assert!(Val::list_contains(&list, &needle));

        let start = Instant::now();
        for _ in 0..1_000 {
            assert!(Val::list_contains(&list, &needle));
        }
        assert!(start.elapsed() < Duration::from_millis(50));
    }
}
