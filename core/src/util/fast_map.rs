pub type FastHashMap<K, V> = rustc_hash::FxHashMap<K, V>;

pub type FastHashSet<K> = rustc_hash::FxHashSet<K>;

#[inline]
pub fn fast_hash_map_new<K, V>() -> FastHashMap<K, V> {
    rustc_hash::FxHashMap::default()
}

#[inline]
pub fn fast_hash_map_with_capacity<K, V>(capacity: usize) -> FastHashMap<K, V> {
    rustc_hash::FxHashMap::with_capacity_and_hasher(capacity, Default::default())
}

#[inline]
pub fn fast_hash_set_new<K>() -> FastHashSet<K> {
    rustc_hash::FxHashSet::default()
}

#[inline]
pub fn fast_hash_set_with_capacity<K>(capacity: usize) -> FastHashSet<K> {
    rustc_hash::FxHashSet::with_capacity_and_hasher(capacity, Default::default())
}
