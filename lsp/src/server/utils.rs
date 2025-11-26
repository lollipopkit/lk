use std::hash::{Hash, Hasher};

use twox_hash::XxHash64;

pub(crate) fn compute_content_hash(content: &str) -> u64 {
    let mut hasher = XxHash64::default();
    content.hash(&mut hasher);
    hasher.finish()
}
