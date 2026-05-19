pub(super) fn range_iteration_count(start: i64, limit: i64, step: i64, inclusive: bool) -> i64 {
    if step > 0 {
        if inclusive {
            if start > limit { 0 } else { ((limit - start) / step) + 1 }
        } else if start >= limit {
            0
        } else {
            ((limit - start - 1) / step) + 1
        }
    } else {
        let stride = -step;
        if inclusive {
            if start < limit {
                0
            } else {
                ((start - limit) / stride) + 1
            }
        } else if start <= limit {
            0
        } else {
            ((start - limit - 1) / stride) + 1
        }
    }
}
