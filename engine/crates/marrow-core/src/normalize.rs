use tree_sitter::{Node, Parser};
use xxhash_rust::xxh3::xxh3_64;

use crate::constants::PARSE_TIMEOUT_MICROS;
use crate::language::Language;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Code,
    Comment,
}

#[derive(Clone, Debug)]
pub struct Line {
    pub number: u32,
    pub kind: LineKind,
    pub role: &'static str,
    pub content_tokens: u32,
    pub tokens: Vec<u64>,
    pub fingerprint: u64,
    pub unigrams: Vec<u64>,
    pub bigrams: Vec<u64>,
}

/// Bumped whenever tokenizing or the canonical rules change. Fingerprints from different
/// versions are never compared (docs/specs/genealogy.md, "Normalizer version").
pub const NORMALIZER_VERSION: i64 = 1;

const LINE_START: u64 = 0x6d61_7272_6f77_0001;
const LINE_END: u64 = 0x6d61_7272_6f77_0002;

/// Returns None when tree-sitter gives up; some inputs make the grammars loop until the timeout.
pub fn normalize(source: &str, language: Language) -> Option<Vec<Line>> {
    tokenize(source, language).map(Tokenizer::into_lines)
}

fn tokenize(source: &str, language: Language) -> Option<Tokenizer<'_>> {
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let mut parser = Parser::new();
    parser
        .set_language(&language.grammar())
        .expect("grammar matches the tree-sitter version");
    parser.set_timeout_micros(PARSE_TIMEOUT_MICROS);
    let tree = parser.parse(source, None)?;
    let mut tokenizer = Tokenizer::new(source, language);
    tokenizer.walk(tree.root_node());
    Some(tokenizer)
}

struct Piece<'a> {
    line: usize,
    text: &'a str,
    comment: bool,
    canonical: Canonical,
}

/// How a token is compared, after removing formatter-only differences (spec: Canonical tokens).
#[derive(Clone, Debug, PartialEq)]
enum Canonical {
    Same,
    Text(&'static str),
    Lowercase(String),
    Dropped,
    Comma { tuple: bool },
}

struct Frame {
    end: usize,
    gap_start: usize,
    words: bool,
}

struct Tokenizer<'a> {
    source: &'a str,
    language: Language,
    line_starts: Vec<usize>,
    pieces: Vec<Piece<'a>>,
    roles: Vec<Option<(usize, &'static str)>>,
}

impl<'a> Tokenizer<'a> {
    fn new(source: &'a str, language: Language) -> Self {
        let mut line_starts = vec![0];
        line_starts.extend(source.match_indices('\n').map(|(index, _)| index + 1));
        let roles = vec![None; line_starts.len()];
        Tokenizer {
            source,
            language,
            line_starts,
            pieces: Vec::new(),
            roles,
        }
    }

    fn line_of(&self, byte: usize) -> usize {
        match self.line_starts.binary_search(&byte) {
            Ok(index) => index,
            Err(index) => index - 1,
        }
    }

    fn walk(&mut self, root: Node<'_>) {
        self.gap(0, root.start_byte(), false);
        let mut cursor = root.walk();
        let mut frames: Vec<Frame> = Vec::new();
        'nodes: loop {
            let node = cursor.node();
            if let Some(frame) = frames.last_mut() {
                let gap_start = frame.gap_start;
                let words = frame.words;
                frame.gap_start = node.end_byte();
                self.gap(gap_start, node.start_byte(), words);
            }
            if self.visit(node) && cursor.goto_first_child() {
                frames.push(Frame {
                    end: node.end_byte(),
                    gap_start: node.start_byte(),
                    words: self.language.is_string_text(node),
                });
                continue;
            }
            loop {
                if cursor.goto_next_sibling() {
                    continue 'nodes;
                }
                if !cursor.goto_parent() {
                    break 'nodes;
                }
                if let Some(frame) = frames.pop() {
                    self.gap(frame.gap_start, frame.end, frame.words);
                }
            }
        }
        self.gap(root.end_byte(), self.source.len(), false);
    }

    fn visit(&mut self, node: Node<'_>) -> bool {
        if node.start_byte() == node.end_byte() {
            return false;
        }
        self.record_role(node);
        if self.language.is_comment(node) {
            self.comment(node);
            return false;
        }
        if node.child_count() == 0 {
            if self.language.is_string_text(node) {
                self.words(node.start_byte(), node.end_byte(), false);
            } else {
                let text = &self.source[node.start_byte()..node.end_byte()];
                match canonical_leaf(self.language, node, text) {
                    Canonical::Same => self.atom(node.start_byte(), node.end_byte()),
                    canonical => {
                        let line = self.line_of(node.start_byte());
                        self.pieces.push(Piece {
                            line,
                            text,
                            comment: false,
                            canonical,
                        });
                    }
                }
            }
            return false;
        }
        true
    }

    fn record_role(&mut self, node: Node<'_>) {
        if !node.is_named() || node.parent().is_none() {
            return;
        }
        let line = self.line_of(node.start_byte());
        let span = node.end_byte() - node.start_byte();
        if self.roles[line].is_none_or(|(best, _)| span > best) {
            self.roles[line] = Some((span, node.kind()));
        }
    }

    fn gap(&mut self, start: usize, end: usize, words: bool) {
        if start >= end {
            return;
        }
        if words {
            self.words(start, end, false);
        } else {
            self.atom(start, end);
        }
    }

    fn atom(&mut self, start: usize, end: usize) {
        let text = &self.source[start..end];
        if text.trim().is_empty() {
            return;
        }
        if text.contains(char::is_whitespace) {
            self.words(start, end, false);
        } else {
            self.push(start, text, false);
        }
    }

    fn words(&mut self, start: usize, end: usize, comment: bool) {
        let text = &self.source[start..end];
        let mut run_start: Option<usize> = None;
        for (offset, character) in text.char_indices() {
            if character.is_alphanumeric() || character == '_' {
                run_start.get_or_insert(offset);
                continue;
            }
            if let Some(run) = run_start.take() {
                self.push(start + run, &text[run..offset], comment);
            }
            if !character.is_whitespace() {
                let width = character.len_utf8();
                self.push(start + offset, &text[offset..offset + width], comment);
            }
        }
        if let Some(run) = run_start {
            self.push(start + run, &text[run..], comment);
        }
    }

    fn comment(&mut self, node: Node<'_>) {
        let (start, end) = (node.start_byte(), node.end_byte());
        let text = &self.source[start..end];
        let terminated = text.ends_with("*/");
        let opener = self
            .language
            .comment_openers()
            .iter()
            .copied()
            .filter(|opener| text.starts_with(opener))
            .filter(|opener| {
                !opener.starts_with("/*") || !terminated || opener.len() + 2 <= text.len()
            })
            .max_by_key(|opener| opener.len());
        let mut body_start = start;
        if let Some(opener) = opener {
            self.push(start, &text[..opener.len()], true);
            body_start += opener.len();
        }
        let closes = opener.is_some_and(|opener| opener.starts_with("/*")) && terminated;
        let body_end = if closes { end - 2 } else { end };
        self.words(body_start, body_end, true);
        if closes {
            self.push(body_end, &text[text.len() - 2..], true);
        }
    }

    fn push(&mut self, byte: usize, text: &'a str, comment: bool) {
        let line = self.line_of(byte);
        self.pieces.push(Piece {
            line,
            text,
            comment,
            canonical: Canonical::Same,
        });
    }

    /// A comma directly before `)`, `]`, or `}` is dropped, unless a one-element tuple needs it
    /// or it follows another comma or an opening bracket.
    fn resolve_trailing_commas(&mut self) {
        let code: Vec<usize> = (0..self.pieces.len())
            .filter(|&index| !self.pieces[index].comment)
            .collect();
        for (position, &index) in code.iter().enumerate() {
            let Canonical::Comma { tuple } = self.pieces[index].canonical else {
                continue;
            };
            let before = position
                .checked_sub(1)
                .map(|previous| self.pieces[code[previous]].text);
            let after = code.get(position + 1).map(|&next| self.pieces[next].text);
            let trailing = matches!(after, Some(")" | "]" | "}"))
                && !matches!(before, Some("," | "(" | "[" | "{"));
            self.pieces[index].canonical = if trailing && !tuple {
                Canonical::Dropped
            } else {
                Canonical::Same
            };
        }
    }

    fn into_lines(mut self) -> Vec<Line> {
        self.resolve_trailing_commas();
        let mut code: Vec<Vec<&Piece>> = vec![Vec::new(); self.line_starts.len()];
        let mut comments: Vec<Vec<&Piece>> = vec![Vec::new(); self.line_starts.len()];
        for piece in &self.pieces {
            if piece.comment {
                comments[piece.line].push(piece);
            } else {
                code[piece.line].push(piece);
            }
        }
        let mut lines = Vec::new();
        for (index, (code, comments)) in code.iter().zip(&comments).enumerate() {
            let (kind, pieces) = if has_tokens(code) {
                (LineKind::Code, code)
            } else if has_tokens(comments) {
                (LineKind::Comment, comments)
            } else {
                continue;
            };
            lines.push(build_line(index + 1, kind, pieces, self.roles[index]));
        }
        lines
    }
}

fn canonical_leaf(language: Language, node: Node<'_>, text: &str) -> Canonical {
    let parent_kind = node.parent().map_or("", |parent| parent.kind());
    if node.kind() == "escape_sequence" && language != Language::Rust {
        return match text {
            "\\'" => Canonical::Text("'"),
            "\\\"" => Canonical::Text("\""),
            _ => Canonical::Same,
        };
    }
    match language {
        Language::Rust | Language::Python if !node.is_named() && text == "," => Canonical::Comma {
            tuple: node
                .parent()
                .is_some_and(|parent| language.is_one_element_tuple(parent)),
        },
        Language::Python if matches!(node.kind(), "string_start" | "string_end") => {
            Canonical::Lowercase(text.to_ascii_lowercase().replace('\'', "\""))
        }
        Language::TypeScript | Language::Tsx if !node.is_named() => match text {
            "'" if parent_kind == "string" => Canonical::Text("\""),
            ";" if !in_for_header(node) => Canonical::Dropped,
            "," if matches!(parent_kind, "interface_body" | "object_type") => Canonical::Dropped,
            "," => Canonical::Comma { tuple: false },
            _ => Canonical::Same,
        },
        _ => Canonical::Same,
    }
}

fn in_for_header(node: Node<'_>) -> bool {
    let mut ancestor = node.parent();
    while let Some(current) = ancestor {
        if current.kind() == "for_statement" {
            return current
                .child_by_field_name("body")
                .is_some_and(|body| node.start_byte() < body.start_byte());
        }
        ancestor = current.parent();
    }
    false
}

fn has_tokens(pieces: &[&Piece]) -> bool {
    pieces.iter().any(|piece| canonical_text(piece).is_some())
}

fn canonical_text<'p>(piece: &'p Piece<'_>) -> Option<&'p str> {
    match &piece.canonical {
        Canonical::Same | Canonical::Comma { .. } => Some(piece.text),
        Canonical::Text(text) => Some(text),
        Canonical::Lowercase(text) => Some(text),
        Canonical::Dropped => None,
    }
}

fn build_line(
    number: usize,
    kind: LineKind,
    pieces: &[&Piece],
    role: Option<(usize, &'static str)>,
) -> Line {
    let tokens: Vec<u64> = pieces
        .iter()
        .filter_map(|piece| canonical_text(piece))
        .map(|text| xxh3_64(text.as_bytes()))
        .collect();
    let content_tokens = pieces.iter().filter(|piece| is_content(piece.text)).count() as u32;
    let mut fingerprint_bytes = Vec::with_capacity(1 + tokens.len() * 8);
    fingerprint_bytes.push(match kind {
        LineKind::Code => 0,
        LineKind::Comment => 1,
    });
    for token in &tokens {
        fingerprint_bytes.extend_from_slice(&token.to_le_bytes());
    }
    let mut unigrams = tokens.clone();
    unigrams.sort_unstable();
    unigrams.dedup();
    let mut sequence = Vec::with_capacity(tokens.len() + 2);
    sequence.push(LINE_START);
    sequence.extend_from_slice(&tokens);
    sequence.push(LINE_END);
    let mut bigrams: Vec<u64> = sequence
        .windows(2)
        .map(|pair| {
            let mut bytes = [0u8; 16];
            bytes[..8].copy_from_slice(&pair[0].to_le_bytes());
            bytes[8..].copy_from_slice(&pair[1].to_le_bytes());
            xxh3_64(&bytes)
        })
        .collect();
    bigrams.sort_unstable();
    Line {
        number: number as u32,
        kind,
        role: role.map_or("", |(_, kind)| kind),
        content_tokens,
        fingerprint: xxh3_64(&fingerprint_bytes),
        tokens,
        unigrams,
        bigrams,
    }
}

fn is_content(text: &str) -> bool {
    text.chars()
        .any(|character| character.is_alphanumeric() || character == '_')
}

#[cfg(test)]
mod tests {
    use super::{canonical_text, tokenize, Line, LineKind};
    use crate::language::Language;

    fn normalize(source: &str, language: Language) -> Vec<Line> {
        super::normalize(source, language).expect("parses")
    }

    const RUST: &str = "//! Inner doc.\n/// Outer doc.\nfn f(x: (u8,)) -> String {\n    /* block\n       more */\n    let s = \"a \\\"b\\\" c\";\n    let r = r#\"raw text\"#;\n    let c = 'x';\n    g(a, b,); // trailing\n    format!(\"{}\", s)\n}\n";
    const PYTHON: &str = "# comment\ndef f(x):\n    s = 'it\\'s'\n    d = \"\"\"doc\n    more\"\"\"\n    v = f\"a {x} b\"\n    return s  # trailing\n";
    const TYPESCRIPT: &str = "// c\ninterface I { a: string, b: number; }\nfor (let i = 0; i < n; i++) { f(i); }\nconst t = `a ${s} b`;\n/* block\n * more */\nlet u = [1, 2,];\n";

    fn token_texts(source: &str, language: Language) -> Vec<(usize, String, bool)> {
        tokenize(source, language)
            .expect("parses")
            .pieces
            .iter()
            .map(|piece| (piece.line + 1, piece.text.to_string(), piece.comment))
            .collect()
    }

    #[test]
    fn tokens_cover_every_non_whitespace_character() {
        for (source, language) in [
            (RUST, Language::Rust),
            (PYTHON, Language::Python),
            (TYPESCRIPT, Language::TypeScript),
            ("fn broken( {\n  let x = ;\n", Language::Rust),
        ] {
            let joined: String = token_texts(source, language)
                .into_iter()
                .map(|(_, text, _)| text)
                .collect();
            let expected: String = source.chars().filter(|c| !c.is_whitespace()).collect();
            assert_eq!(joined, expected, "{language:?}");
        }
    }

    #[test]
    fn whitespace_never_changes_a_fingerprint() {
        let tight = normalize("fn f(x:u8)->u8{x+1}\n", Language::Rust);
        let loose = normalize("  fn  f( x : u8 ) -> u8 { x + 1 }\r\n", Language::Rust);
        assert_eq!(tight.len(), 1);
        assert_eq!(tight[0].fingerprint, loose[0].fingerprint);
    }

    #[test]
    fn comment_lines_are_tracked_and_trailing_comments_are_ignored() {
        let before = normalize(
            "x = 1  # first\n# Parse the header row.\n",
            Language::Python,
        );
        let after = normalize(
            "x = 1  # second\n# Parse the first row.\n",
            Language::Python,
        );
        assert_eq!(before[0].kind, LineKind::Code);
        assert_eq!(before[0].fingerprint, after[0].fingerprint);
        assert_eq!(before[1].kind, LineKind::Comment);
        assert_ne!(before[1].fingerprint, after[1].fingerprint);
        assert_eq!(before[1].content_tokens, 4);
    }

    #[test]
    fn comments_and_strings_split_into_words() {
        let source = "// Parsed inventory row.\nfn f() { let s = \"invalid item\"; }\n";
        let texts: Vec<String> = token_texts(source, Language::Rust)
            .into_iter()
            .map(|(_, text, _)| text)
            .collect();
        assert_eq!(
            texts,
            [
                "//",
                "Parsed",
                "inventory",
                "row",
                ".",
                "fn",
                "f",
                "(",
                ")",
                "{",
                "let",
                "s",
                "=",
                "\"",
                "invalid",
                "item",
                "\"",
                ";",
                "}"
            ]
        );
    }

    #[test]
    fn multi_line_comments_and_strings_count_on_each_line() {
        let lines = normalize(
            "/* one\n   two */\nconst s = `a\nb`;\n",
            Language::TypeScript,
        );
        let numbers: Vec<u32> = lines.iter().map(|line| line.number).collect();
        assert_eq!(numbers, [1, 2, 3, 4]);
        assert_eq!(lines[0].kind, LineKind::Comment);
        assert_eq!(lines[1].kind, LineKind::Comment);
        assert_eq!(lines[3].kind, LineKind::Code);
    }

    #[test]
    fn blank_lines_are_not_tracked() {
        let lines = normalize("a = 1\n\n   \nb = 2\n", Language::Python);
        let numbers: Vec<u32> = lines.iter().map(|line| line.number).collect();
        assert_eq!(numbers, [1, 4]);
    }

    fn canonical(source: &str, language: Language) -> Vec<Vec<String>> {
        let mut tokenizer = tokenize(source, language).expect("parses");
        tokenizer.resolve_trailing_commas();
        let mut lines: Vec<Vec<String>> = vec![Vec::new(); tokenizer.line_starts.len()];
        for piece in &tokenizer.pieces {
            if let Some(text) = canonical_text(piece) {
                lines[piece.line].push(text.to_owned());
            }
        }
        lines.retain(|line| !line.is_empty());
        lines
    }

    fn same_fingerprints(a: &str, b: &str, language: Language) -> bool {
        let fingerprints = |source| -> Vec<u64> {
            normalize(source, language)
                .iter()
                .map(|line| line.fingerprint)
                .collect()
        };
        fingerprints(a) == fingerprints(b)
    }

    #[test]
    fn quote_style_is_ignored_in_python_and_typescript_but_not_rust() {
        assert!(same_fingerprints(
            "s = 'it\\'s'\n",
            "s = \"it's\"\n",
            Language::Python
        ));
        assert!(same_fingerprints(
            "s = R'a'\n",
            "s = r\"a\"\n",
            Language::Python
        ));
        assert!(same_fingerprints(
            "d = '''doc'''\n",
            "d = \"\"\"doc\"\"\"\n",
            Language::Python
        ));
        assert!(same_fingerprints(
            "const s = 'x';\n",
            "const s = \"x\"\n",
            Language::TypeScript
        ));
        assert!(!same_fingerprints(
            "const s = `x`;\n",
            "const s = \"x\";\n",
            Language::TypeScript
        ));
        assert!(!same_fingerprints(
            "fn f() { g('a'); }\n",
            "fn f() { g(\"a\"); }\n",
            Language::Rust
        ));
    }

    #[test]
    fn trailing_commas_are_ignored_unless_the_syntax_needs_them() {
        assert!(same_fingerprints(
            "fn f() { g(a, b,); }\n",
            "fn f() { g(a, b); }\n",
            Language::Rust
        ));
        assert!(same_fingerprints(
            "g(\n    a,\n)\n",
            "g(\n    a\n)\n",
            Language::Python
        ));
        assert!(same_fingerprints(
            "let u = [1, 2,];\n",
            "let u = [1, 2]\n",
            Language::TypeScript
        ));
        assert!(!same_fingerprints(
            "t = (x,)\n",
            "t = (x)\n",
            Language::Python
        ));
        assert!(!same_fingerprints(
            "fn f() -> (u8,) { (1,) }\n",
            "fn f() -> (u8) { (1) }\n",
            Language::Rust
        ));
        assert_eq!(
            canonical("let u = [1,,];\n", Language::TypeScript),
            [vec!["let", "u", "=", "[", "1", ",", ",", "]"]]
        );
    }

    #[test]
    fn typescript_semicolons_are_dropped_outside_for_headers_and_rust_keeps_them() {
        assert_eq!(
            canonical(
                "interface I { a: string, b: number; }\nclass K { x = 1; }\nfor (let i = 0; i < n; i++) { f(i); }\n",
                Language::TypeScript,
            ),
            [
                vec!["interface", "I", "{", "a", ":", "string", "b", ":", "number", "}"],
                vec!["class", "K", "{", "x", "=", "1", "}"],
                vec!["for", "(", "let", "i", "=", "0", ";", "i", "<", "n", ";", "i", "++", ")", "{", "f", "(", "i", ")", "}"],
            ]
        );
        assert!(!same_fingerprints(
            "fn f() { g(); }\n",
            "fn f() { g() }\n",
            Language::Rust
        ));
    }

    #[test]
    fn a_byte_order_mark_is_ignored() {
        let with_mark = normalize("\u{feff}fn main() {}\n", Language::Rust);
        let without = normalize("fn main() {}\n", Language::Rust);
        assert_eq!(with_mark[0].fingerprint, without[0].fingerprint);
    }

    #[test]
    fn lines_left_without_tokens_are_not_tracked() {
        let numbers =
            |lines: Vec<Line>| -> Vec<u32> { lines.iter().map(|line| line.number).collect() };
        assert_eq!(
            numbers(normalize("let a = 1\n;\n", Language::TypeScript)),
            [1]
        );
        assert_eq!(
            numbers(normalize("g(\n    a\n    ,\n)\n", Language::Python)),
            [1, 2, 4]
        );
        let with_comment = normalize("let a = 1\n; // done\n", Language::TypeScript);
        assert_eq!(with_comment.len(), 2);
        assert_eq!(with_comment[1].kind, LineKind::Comment);
    }

    #[test]
    fn content_tokens_ignore_punctuation() {
        let lines = normalize("}\nuse std::io::Read;\n", Language::Rust);
        assert_eq!(lines[0].content_tokens, 0);
        assert_eq!(lines[1].content_tokens, 4);
    }
}
