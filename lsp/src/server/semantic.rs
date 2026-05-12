use tower_lsp::lsp_types::{SemanticToken, SemanticTokensEdit};

pub(crate) fn common_prefix_suffix_delete_count(old: &[SemanticToken], new: &[SemanticToken]) -> (usize, usize, usize) {
    let mut cp = 0usize;
    let min_len = old.len().min(new.len());
    while cp < min_len && semantic_token_eq(&old[cp], &new[cp]) {
        cp += 1;
    }

    if cp == old.len() && old.len() == new.len() {
        return (cp, 0, 0);
    }

    let mut cs = 0usize;
    while cs < (old.len() - cp)
        && cs < (new.len() - cp)
        && semantic_token_eq(&old[old.len() - 1 - cs], &new[new.len() - 1 - cs])
    {
        cs += 1;
    }
    let delete_count = old.len().saturating_sub(cp + cs);
    (cp, cs, delete_count)
}

fn semantic_token_eq(a: &SemanticToken, b: &SemanticToken) -> bool {
    a.delta_line == b.delta_line
        && a.delta_start == b.delta_start
        && a.length == b.length
        && a.token_type == b.token_type
        && a.token_modifiers_bitset == b.token_modifiers_bitset
}

pub(crate) fn semantic_tokens_delta_edit(old: &[SemanticToken], new: &[SemanticToken]) -> Option<SemanticTokensEdit> {
    let (common_prefix, common_suffix, delete_tokens) = common_prefix_suffix_delete_count(old, new);
    let insert_tokens = new[common_prefix..(new.len() - common_suffix)].to_vec();
    if delete_tokens == 0 && insert_tokens.is_empty() {
        return None;
    }

    Some(SemanticTokensEdit {
        start: (common_prefix * 5) as u32,
        delete_count: (delete_tokens * 5) as u32,
        data: (!insert_tokens.is_empty()).then_some(insert_tokens),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(line: u32, start: u32) -> SemanticToken {
        SemanticToken {
            delta_line: line,
            delta_start: start,
            length: 1,
            token_type: 0,
            token_modifiers_bitset: 0,
        }
    }

    #[test]
    fn delta_edit_uses_flattened_array_offsets() {
        let old = vec![token(0, 0), token(0, 2), token(1, 0)];
        let new = vec![token(0, 0), token(0, 3), token(1, 0)];

        let edit = semantic_tokens_delta_edit(&old, &new).expect("one edit");

        assert_eq!(edit.start, 5);
        assert_eq!(edit.delete_count, 5);
        assert_eq!(edit.data.as_ref().map(Vec::len), Some(1));
    }

    #[test]
    fn delta_edit_can_insert_without_deleting() {
        let old = vec![token(0, 0)];
        let new = vec![token(0, 0), token(0, 2)];

        let edit = semantic_tokens_delta_edit(&old, &new).expect("insert edit");

        assert_eq!(edit.start, 5);
        assert_eq!(edit.delete_count, 0);
        assert_eq!(edit.data.as_ref().map(Vec::len), Some(1));
    }
}
