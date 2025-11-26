use tower_lsp::lsp_types::SemanticToken;

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
