use tower_lsp::lsp_types::FormattingOptions;

pub(crate) fn format_lkr(input: &str, options: &FormattingOptions) -> String {
    let mut out = String::with_capacity(input.len() + 16);
    let use_spaces = options.insert_spaces;
    let tab_size = options.tab_size.clamp(1, 8) as usize;
    let mut indent = 0isize;

    for raw_line in input.lines() {
        let line = raw_line.trim();
        let leading_closers = line
            .chars()
            .take_while(|c| c.is_whitespace() || *c == '}' || *c == ')' || *c == ']')
            .filter(|c| *c == '}' || *c == ')' || *c == ']')
            .count();
        if leading_closers > 0 && indent > 0 {
            indent -= leading_closers as isize;
            if indent < 0 {
                indent = 0;
            }
        }

        if use_spaces {
            for _ in 0..(indent.max(0) as usize * tab_size) {
                out.push(' ');
            }
        } else {
            for _ in 0..indent.max(0) {
                out.push('\t');
            }
        }
        out.push_str(line);
        out.push('\n');

        let mut delta = 0isize;
        for ch in line.chars() {
            match ch {
                '{' | '(' | '[' => delta += 1,
                '}' | ')' | ']' => delta -= 1,
                _ => {}
            }
        }
        indent += delta;
        if indent < 0 {
            indent = 0;
        }
    }

    out
}
