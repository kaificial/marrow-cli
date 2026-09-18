use tree_sitter::Node;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Language {
    Rust,
    Python,
    TypeScript,
    Tsx,
}

impl Language {
    pub fn from_path(path: &str) -> Option<Language> {
        let file_name = path.rsplit('/').next().unwrap_or(path);
        let (_, extension) = file_name.rsplit_once('.')?;
        match extension {
            "rs" => Some(Language::Rust),
            "py" => Some(Language::Python),
            "ts" | "mts" | "cts" => Some(Language::TypeScript),
            "tsx" => Some(Language::Tsx),
            _ => None,
        }
    }

    pub(crate) fn grammar(self) -> tree_sitter::Language {
        match self {
            Language::Rust => tree_sitter_rust::LANGUAGE.into(),
            Language::Python => tree_sitter_python::LANGUAGE.into(),
            Language::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Language::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        }
    }

    pub(crate) fn is_comment(self, node: Node) -> bool {
        match self {
            Language::Rust => matches!(node.kind(), "line_comment" | "block_comment"),
            _ => node.kind() == "comment",
        }
    }

    /// Nodes whose own text, and any text between their children, is split into word tokens.
    pub(crate) fn is_string_text(self, node: Node) -> bool {
        if !node.is_named() {
            return false;
        }
        match self {
            Language::Rust => matches!(
                node.kind(),
                "string_literal" | "raw_string_literal" | "string_content"
            ),
            Language::Python => matches!(node.kind(), "string" | "string_content"),
            Language::TypeScript | Language::Tsx => {
                matches!(
                    node.kind(),
                    "string" | "template_string" | "string_fragment"
                )
            }
        }
    }

    pub(crate) fn comment_openers(self) -> &'static [&'static str] {
        match self {
            Language::Rust => &["//!", "///", "//", "/*!", "/**", "/*"],
            Language::Python => &["#"],
            Language::TypeScript | Language::Tsx => &["//", "/**", "/*"],
        }
    }

    pub(crate) fn is_one_element_tuple(self, node: Node) -> bool {
        let tuple_kinds: &[&str] = match self {
            Language::Rust => &["tuple_expression", "tuple_type", "tuple_pattern"],
            Language::Python => &["tuple", "tuple_pattern"],
            Language::TypeScript | Language::Tsx => &[],
        };
        if !tuple_kinds.contains(&node.kind()) {
            return false;
        }
        let mut cursor = node.walk();
        let elements = node
            .named_children(&mut cursor)
            .filter(|child| !self.is_comment(*child))
            .count();
        elements == 1
    }
}

#[cfg(test)]
mod tests {
    use super::Language;

    #[test]
    fn language_comes_from_the_extension() {
        assert_eq!(Language::from_path("src/main.rs"), Some(Language::Rust));
        assert_eq!(Language::from_path("pkg/mod.py"), Some(Language::Python));
        assert_eq!(Language::from_path("a/b.mts"), Some(Language::TypeScript));
        assert_eq!(Language::from_path("view.tsx"), Some(Language::Tsx));
        assert_eq!(Language::from_path("README.md"), None);
        assert_eq!(Language::from_path("dir.rs/Makefile"), None);
        assert_eq!(Language::from_path("script.js"), None);
    }
}
