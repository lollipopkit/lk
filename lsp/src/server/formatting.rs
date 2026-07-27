use lk_core::fmt::{format_source, FormatOptions};

/// Format a document with the shared `lk fmt` engine, so an editor save and a
/// `lk fmt --check` run in CI can never disagree.
///
/// The editor's `tab_size` / `insert_spaces` are deliberately *not* consulted,
/// which is what makes that sentence true. They are a per-user preference, and
/// honouring them here would mean a file formatted on save fails `lk fmt
/// --check` in CI for no reason its author could see — the answer to "how is
/// this file indented" has to be a property of the language, not of who saved
/// it last.
///
/// A buffer that does not tokenize — the normal state mid-edit — is returned
/// unchanged, which the handler turns into an empty edit list.
pub(crate) fn format_lk(input: &str) -> String {
    format_source(input, FormatOptions::default()).unwrap_or_else(|_| input.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same answer `lk fmt` gives, whatever the editor is set to — a file
    /// formatted on save has to pass `lk fmt --check`.
    #[test]
    fn formats_the_way_the_cli_does() {
        let src = "fn main() {\nlet x = 1;\n}\n";
        assert_eq!(format_lk(src), "fn main() {\n    let x = 1;\n}\n");
    }

    #[test]
    fn leaves_unparsable_buffers_alone() {
        let mid_edit = "fn main() {\nlet s = \"oops\n";
        assert_eq!(format_lk(mid_edit), mid_edit);
    }
}
