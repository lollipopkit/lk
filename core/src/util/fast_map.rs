pub type FastHashMap<K, V> = hashbrown::HashMap<K, V, rustc_hash::FxBuildHasher>;

pub type FastHashSet<K> = hashbrown::HashSet<K, rustc_hash::FxBuildHasher>;

#[inline]
pub fn fast_hash_map_new<K, V>() -> FastHashMap<K, V> {
    hashbrown::HashMap::default()
}

#[inline]
pub fn fast_hash_map_with_capacity<K, V>(capacity: usize) -> FastHashMap<K, V> {
    hashbrown::HashMap::with_capacity_and_hasher(capacity, Default::default())
}

#[inline]
pub fn fast_hash_set_new<K>() -> FastHashSet<K> {
    hashbrown::HashSet::default()
}

#[inline]
pub fn fast_hash_set_with_capacity<K>(capacity: usize) -> FastHashSet<K> {
    hashbrown::HashSet::with_capacity_and_hasher(capacity, Default::default())
}
