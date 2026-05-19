use crate::val::Val;

#[inline]
pub(super) fn index_value(text: &str, index: i64) -> Option<Val> {
    let len = if text.is_ascii() {
        text.len()
    } else {
        text.chars().count()
    };
    let index = if index < 0 {
        len.checked_sub(index.unsigned_abs() as usize)?
    } else {
        index as usize
    };
    if text.is_ascii() {
        let bytes = text.as_bytes();
        Some(if index < len {
            Val::ascii_char_value(bytes[index])
        } else {
            Val::Nil
        })
    } else {
        Some(
            text.chars()
                .nth(index as usize)
                .map(|character| Val::from_str(&character.to_string()))
                .unwrap_or(Val::Nil),
        )
    }
}

#[inline]
pub(super) fn slice_range_value(text: &str, key: &[Val]) -> Option<Val> {
    let len = if text.is_ascii() {
        text.len()
    } else {
        text.chars().count()
    };
    let (start, end) = super::list_ops::range_key_bounds(key, len)?;
    if text.is_ascii() {
        return Some(Val::from_str(&text[start..end]));
    }
    Some(Val::from_str(
        &text
            .chars()
            .skip(start)
            .take(end.saturating_sub(start))
            .collect::<String>(),
    ))
}
