//! The editor grammars keep their own copy of the language's type names.
//!
//! Three copies, historically, and they disagreed: machine ints (`u8`, `usize`,
//! …) were in the language for a month while neither grammar highlighted them,
//! and completion's receiver table listed a `Str` that the language has never
//! had. Nobody did anything wrong — a copy nobody can check is a copy that
//! drifts.
//!
//! So the copies are checked here, against `lk_values`, which is the one that
//! decides. A name added to the language and not to a grammar fails this; a
//! name in a grammar that the language does not have fails it too.

#[cfg(test)]
mod tests {
    use lk_core::val::{IntKind, CONTAINER_TYPE_NAMES, PRIMITIVE_TYPES};
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    /// Names the tree-sitter grammar legitimately does not list as builtins.
    ///
    /// Each of these takes a type parameter (`Set<T>`), so it needs a rule of
    /// its own beside `list_type`/`map_type` rather than a bare keyword in
    /// `primitive_type` — putting it there would make `Set<Int>` fail to parse.
    /// Until someone writes those rules they fall through to `type_identifier`,
    /// which still highlights as a type, just not as a builtin one.
    ///
    /// `List` and `Map` are absent from this list because they *do* have rules.
    const TREE_SITTER_NOT_YET_BUILTIN: &[&str] = &["Set", "Tuple", "Task", "Channel", "Box", "Boxed"];

    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .map(Path::to_path_buf)
            .expect("lsp crate has a parent directory")
    }

    /// Every type name the language spells, builtin and machine-int alike.
    fn language_type_names() -> BTreeSet<String> {
        PRIMITIVE_TYPES
            .iter()
            .map(|(name, _)| (*name).to_string())
            .chain(CONTAINER_TYPE_NAMES.iter().map(|name| (*name).to_string()))
            .chain(IntKind::ALL.iter().map(|kind| kind.name().to_string()))
            .collect()
    }

    /// The quoted words inside `primitive_type: $ => choice(...)`.
    fn tree_sitter_primitive_names(grammar: &str) -> BTreeSet<String> {
        let start = grammar
            .find("primitive_type: $ => choice(")
            .expect("grammar.js declares primitive_type");
        let rest = &grammar[start..];
        let end = rest.find(')').expect("primitive_type choice is closed");
        rest[..end]
            .split('\'')
            .skip(1)
            .step_by(2)
            .map(ToString::to_string)
            .collect()
    }

    /// The alternatives of every `support.type.primitive.lk` match pattern.
    fn textmate_type_names(grammar: &str) -> BTreeSet<String> {
        let mut names = BTreeSet::new();
        for section in grammar.split("support.type.primitive.lk").skip(1) {
            let Some(open) = section.find("(") else { continue };
            let Some(close) = section[open..].find(")") else {
                continue;
            };
            for name in section[open + 1..open + close].split('|') {
                let name = name.trim();
                if !name.is_empty() {
                    names.insert(name.to_string());
                }
            }
        }
        assert!(!names.is_empty(), "found no type patterns in the TextMate grammar");
        names
    }

    #[test]
    fn type_name_lists_agree() {
        let root = repo_root();
        let grammar_js = std::fs::read_to_string(root.join("ecosystem/tree-sitter-lk/grammar.js"))
            .expect("read tree-sitter grammar");
        let tm_language = std::fs::read_to_string(root.join("ecosystem/vsc-ext/lsp/syntaxes/lk.tmLanguage.json"))
            .expect("read TextMate grammar");

        let language = language_type_names();

        // TextMate is pure highlighting — it can and should carry every name.
        let textmate = textmate_type_names(&tm_language);
        assert_eq!(
            language.difference(&textmate).collect::<Vec<_>>(),
            Vec::<&String>::new(),
            "the TextMate grammar does not highlight these language types"
        );
        assert_eq!(
            textmate.difference(&language).collect::<Vec<_>>(),
            Vec::<&String>::new(),
            "the TextMate grammar highlights these as types, but the language has no such type"
        );

        // tree-sitter drives parsing, so a parameterised name cannot be a bare
        // keyword here; those are listed as not-yet-builtin with a rationale.
        let expected_tree_sitter: BTreeSet<String> = language
            .iter()
            .filter(|name| !TREE_SITTER_NOT_YET_BUILTIN.contains(&name.as_str()))
            // `List<T>`/`Map<K, V>` have rules of their own.
            .filter(|name| name.as_str() != "List" && name.as_str() != "Map")
            .cloned()
            .collect();
        let tree_sitter = tree_sitter_primitive_names(&grammar_js);
        assert_eq!(
            expected_tree_sitter.difference(&tree_sitter).collect::<Vec<_>>(),
            Vec::<&String>::new(),
            "grammar.js does not know these language types — add them to `primitive_type` \
             and re-run `tree-sitter generate`, or list them in TREE_SITTER_NOT_YET_BUILTIN"
        );
        assert_eq!(
            tree_sitter.difference(&expected_tree_sitter).collect::<Vec<_>>(),
            Vec::<&String>::new(),
            "grammar.js lists these as builtin types, but the language has no such type"
        );
    }

    #[test]
    fn the_generated_parser_is_not_stale() {
        // `src/parser.c` is a committed build product. A `primitive_type` name
        // present in the grammar but absent from the generated parser means
        // somebody edited grammar.js without re-running `tree-sitter generate`,
        // and the editor is still parsing with the old rules.
        let root = repo_root();
        let grammar_js = std::fs::read_to_string(root.join("ecosystem/tree-sitter-lk/grammar.js"))
            .expect("read tree-sitter grammar");
        let generated = std::fs::read_to_string(root.join("ecosystem/tree-sitter-lk/src/grammar.json"))
            .expect("read generated grammar.json");

        for name in tree_sitter_primitive_names(&grammar_js) {
            assert!(
                generated.contains(&format!("\"value\": \"{name}\"")),
                "`{name}` is in grammar.js but not in the generated parser — run `tree-sitter generate`"
            );
        }
    }
}
