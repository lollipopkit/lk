use crate::val::Val;

#[inline]
pub(super) fn index_value(text: &str, index: i64) -> Option<Val> {
    if index < 0 {
        Some(Val::Nil)
    } else if text.is_ascii() {
        let index = index as usize;
        let bytes = text.as_bytes();
        Some(if index < bytes.len() {
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
