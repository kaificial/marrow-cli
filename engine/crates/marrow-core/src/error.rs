use crate::constants::PARSE_TIMEOUT_MICROS;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("can't open a git repository at {path}: {message}")]
    Open { path: String, message: String },
    #[error("git error: {0}")]
    Git(String),
    #[error(
        "tree-sitter gave up parsing {path} at commit {commit} after {} seconds",
        PARSE_TIMEOUT_MICROS / 1_000_000
    )]
    ParserGaveUp { commit: String, path: String },
    #[error("internal error: {0}")]
    Internal(String),
}
