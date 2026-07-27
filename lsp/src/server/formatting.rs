use lk_core::fmt::{format_source, FormatOptions};
use tower_lsp::lsp_types::FormattingOptions;

/// Format a document with the shared `lk fmt` engine, so an editor save and a
/// `lk fmt --check` run in CI can never disagree.
///
/// A buffer that does not tokenize — the normal state mid-edit — is returned
/// unchanged, which the handler turns into an empty edit list.
pub(crate) fn format_lk(input: &str, options: &FormattingOptions) -> String {
    let opts = FormatOptions {
        indent_width: options.tab_size.clamp(1, 16) as usize,
        use_tabs: !options.insert_spaces,
    };
    format_source(input, opts).unwrap_or_else(|_| input.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(tab_size: u32, insert_spaces: bool) -> FormattingOptions {
        FormattingOptions {
            tab_size,
            insert_spaces,
            ..Default::default()
        }
    }

    #[test]
    fn formats_with_editor_options() {
        let src = "fn main() {\nlet x = 1;\n}\n";
        assert_eq!(format_lk(src, &options(2, true)), "fn main() {\n  let x = 1;\n}\n");
        assert_eq!(format_lk(src, &options(4, false)), "fn main() {\n\tlet x = 1;\n}\n");
    }

    #[test]
    fn leaves_unparsable_buffers_alone() {
        let mid_edit = "fn main() {\nlet s = \"oops\n";
        assert_eq!(format_lk(mid_edit, &options(4, true)), mid_edit);
    }
}
