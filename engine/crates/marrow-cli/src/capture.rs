//! Pieces `marrow record` and `marrow reconcile` both need: where the repository is, how a
//! stored line maps to an engine line, and where the store lives.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use marrow_core::language::Language;
use marrow_core::normalize::{Line, LineKind};
use marrow_store::{SnapshotLine, Store};

pub const TOOL: &str = "claude_code";

/// The nearest directory at or above `from` holding a `.git`.
pub fn repository_root(from: &Path) -> Option<PathBuf> {
    from.ancestors()
        .find(|directory| directory.join(".git").exists())
        .map(Path::to_path_buf)
}

/// A repo-relative path with forward slashes, which is how git and the store both name files.
pub fn relative_path(root: &Path, file: &Path) -> Option<String> {
    let relative = file.strip_prefix(root).ok()?;
    Some(
        relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
    )
}

/// Opens the store, creating `.marrow/` and keeping it out of git.
pub fn open_store(root: &Path) -> Result<Store, String> {
    let marrow = root.join(".marrow");
    std::fs::create_dir_all(&marrow)
        .map_err(|error| format!("can't create {}: {error}", marrow.display()))?;
    keep_out_of_git(root);
    Store::open(&marrow.join("db.sqlite")).map_err(|error| format!("{error}"))
}

/// PRD §7.4: the store is local and never committed.
fn keep_out_of_git(root: &Path) {
    let info = root.join(".git").join("info");
    if std::fs::create_dir_all(&info).is_err() {
        return;
    }
    let exclude = info.join("exclude");
    let current = std::fs::read_to_string(&exclude).unwrap_or_default();
    if current.lines().any(|line| line.trim() == ".marrow/") {
        return;
    }
    let separator = if current.is_empty() || current.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    let _ = std::fs::write(&exclude, format!("{current}{separator}.marrow/\n"));
}

pub fn language_name(language: Language) -> String {
    format!("{language:?}").to_lowercase()
}

pub fn stored_line(line_id: i64, line: &Line) -> SnapshotLine {
    SnapshotLine {
        line_id,
        line_number: line.number,
        kind: match line.kind {
            LineKind::Code => "code",
            LineKind::Comment => "comment",
        }
        .to_owned(),
        role: line.role.to_owned(),
        content_tokens: line.content_tokens,
        fingerprint: line.fingerprint,
        tokens: line.tokens.clone(),
        unigrams: line.unigrams.clone(),
        bigrams: line.bigrams.clone(),
    }
}

/// The stored role is kept for reports; matching never reads it, so it isn't restored here.
pub fn engine_line(stored: &SnapshotLine) -> Line {
    Line {
        number: stored.line_number,
        kind: if stored.kind == "comment" {
            LineKind::Comment
        } else {
            LineKind::Code
        },
        role: "",
        content_tokens: stored.content_tokens,
        tokens: stored.tokens.clone(),
        fingerprint: stored.fingerprint,
        unigrams: stored.unigrams.clone(),
        bigrams: stored.bigrams.clone(),
    }
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or_default()
}
