//! Character-oriented string operations.
//!
//! One implementation, used by both spellings of every string operation: the
//! method form (`s.substring(…)`, dispatched in the VM) and the module form
//! (`string.substring(s, …)`, exported by the standard library). They used to
//! be written twice, and had drifted — `"héllo wörld".len()` answered 11 while
//! `string.len(…)` answered 13.
//!
//! **Positions are characters, not bytes.** LK's strings are UTF-8 and its
//! `len` counts characters, so every position that meets a length has to count
//! the same thing; `s.substring(0, s.len())` is the shape that decides it.
//! Byte-oriented work belongs to `bytes`.
//!
//! Slicing by byte offset is also what made these operations *panic* rather
//! than fail: `&s[2..5]` on `"héllo"` lands inside `é`, and inside the VM a
//! panic is not an error the program can see — it takes the process down.

/// The number of characters in `text`.
///
/// The ASCII fast path matters: this is on the hot path for every `s.len()`,
/// and for ASCII the byte length already *is* the character count.
pub fn char_len(text: &str) -> usize {
    if text.is_ascii() {
        text.len()
    } else {
        text.chars().count()
    }
}

/// The byte offset of character `index`, or the end of the string when `index`
/// is past the last character.
fn byte_offset(text: &str, index: usize) -> usize {
    text.char_indices()
        .nth(index)
        .map(|(offset, _)| offset)
        .unwrap_or(text.len())
}

/// `length` characters of `text` starting at character `start`.
///
/// Out of range is an empty result rather than an error, matching what both
/// implementations did with byte offsets before.
pub fn substring(text: &str, start: usize, length: usize) -> &str {
    let start_offset = byte_offset(text, start);
    let end_offset = byte_offset(text, start.saturating_add(length));
    // `saturating_add` because `usize` is 32-bit on the bare-metal targets,
    // where two large `Int` arguments overflow it; wrapping would produce
    // `end < start`.
    if end_offset <= start_offset {
        return "";
    }
    &text[start_offset..end_offset]
}

/// The character index at which `needle` first occurs.
///
/// `None` for "not found" — the method form used to answer `-1`, which is a
/// valid index and so goes wrong quietly when it is used as one.
pub fn find_char_index(text: &str, needle: &str) -> Option<usize> {
    let byte = text.find(needle)?;
    Some(text[..byte].chars().count())
}

/// Like [`find_char_index`], starting the search at character `start`.
pub fn find_char_index_from(text: &str, needle: &str, start: usize) -> Option<usize> {
    let start_offset = byte_offset(text, start);
    let byte = text[start_offset..].find(needle)? + start_offset;
    Some(text[..byte].chars().count())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_characters_not_bytes() {
        assert_eq!(char_len("héllo wörld"), 11);
        assert_eq!(char_len(""), 0);
        assert_eq!(char_len("abc"), 3);
    }

    #[test]
    fn substring_takes_character_positions() {
        // The case that used to panic: byte 2 is inside `é`.
        assert_eq!(substring("héllo", 2, 3), "llo");
        assert_eq!(substring("héllo", 0, 2), "hé");
        assert_eq!(substring("abc", 1, 1), "b");
    }

    #[test]
    fn substring_out_of_range_is_empty_not_a_panic() {
        assert_eq!(substring("abc", 5, 2), "");
        assert_eq!(substring("abc", 1, 0), "");
        assert_eq!(substring("abc", 0, 99), "abc");
        assert_eq!(substring("héllo", 3, usize::MAX), "lo");
    }

    #[test]
    fn substring_composes_with_len() {
        // The shape that forces positions and lengths to count the same thing.
        let text = "héllo wörld";
        assert_eq!(substring(text, 0, char_len(text)), text);
    }

    #[test]
    fn find_answers_in_characters() {
        // Byte offset would be 3 — `é` is two bytes.
        assert_eq!(find_char_index("héllo", "llo"), Some(2));
        assert_eq!(find_char_index("héllo", "h"), Some(0));
        assert_eq!(find_char_index("héllo", "zz"), None);
    }

    #[test]
    fn find_composes_with_substring() {
        let text = "héllo wörld";
        let at = find_char_index(text, "wörld").expect("present");
        assert_eq!(substring(text, at, 5), "wörld");
    }

    #[test]
    fn find_from_skips_earlier_matches() {
        assert_eq!(find_char_index_from("ababa", "a", 1), Some(2));
        assert_eq!(find_char_index_from("héllo", "l", 3), Some(3));
        assert_eq!(find_char_index_from("abc", "a", 9), None);
    }
}
