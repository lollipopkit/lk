use arcstr::ArcStr;
use dashmap::DashMap;
use once_cell::sync::Lazy;

const INTERN_MAX_LEN: usize = 64;

static INTERN_TABLE: Lazy<DashMap<ArcStr, ArcStr>> = Lazy::new(DashMap::new);

#[inline]
pub(super) fn intern(s: &str) -> ArcStr {
    if s.len() > INTERN_MAX_LEN {
        return ArcStr::from(s);
    }
    if let Some(entry) = INTERN_TABLE.get(s) {
        return entry.clone();
    }
    let arc = ArcStr::from(s);
    match INTERN_TABLE.entry(arc.clone()) {
        dashmap::mapref::entry::Entry::Occupied(entry) => entry.get().clone(),
        dashmap::mapref::entry::Entry::Vacant(entry) => {
            entry.insert(arc.clone());
            arc
        }
    }
}
