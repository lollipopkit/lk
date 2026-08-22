//! The map a *program* sees.
//!
//! Distinct from [`FastHashMap`](super::fast_map::FastHashMap), which is the
//! compiler's and the runtime's own bookkeeping: those tables are asked
//! questions about keys and never iterated for the user, so hash order costs
//! nothing. A `Map` in the language is different — it is printed, iterated,
//! and compared — and hash order there is a property of *how the map was
//! built* rather than of what it holds:
//!
//! ```lk
//! let a = {"zebra": 1, "apple": 2, "mango": 3, "kiwi": 4};
//! let b = {};
//! b["zebra"] = 1; b["apple"] = 2; b["mango"] = 3; b["kiwi"] = 4;
//! // a == b, and the two printed their fields in different orders.
//! ```
//!
//! Insertion order fixes that, and it is what every scripting language a
//! reader is likely to come from does. It also removes a load-bearing
//! coincidence: the native runtime used to reproduce the interpreter's *hash
//! layout* to keep `for k in m` iterating the same way in both back ends, and
//! that argument rested on both linking the same `hashbrown`, the same rustc
//! deriving the same `Hash` discriminants, and one fixed seed. Appending to a
//! vector needs no such argument.
//!
//! `shift_remove`, not `swap_remove`: a delete keeps the surviving entries in
//! order, which is the whole point. It is `O(n)` in the tail, which is the
//! price of the guarantee.

pub type ValueMap<K, V> = indexmap::IndexMap<K, V, rustc_hash::FxBuildHasher>;

#[inline]
pub fn value_map_new<K, V>() -> ValueMap<K, V> {
    ValueMap::default()
}

#[inline]
pub fn value_map_with_capacity<K, V>(capacity: usize) -> ValueMap<K, V> {
    ValueMap::with_capacity_and_hasher(capacity, Default::default())
}

#[inline]
pub fn value_map_from_iter<K: Eq + core::hash::Hash, V, I: IntoIterator<Item = (K, V)>>(iter: I) -> ValueMap<K, V> {
    iter.into_iter().collect()
}
