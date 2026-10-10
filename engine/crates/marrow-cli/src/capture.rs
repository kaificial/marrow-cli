//! Pieces `marrow record` and `marrow reconcile` both need: where the repository is, how a
//! stored line maps to an engine line, and where the store lives.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use marrow_core::language::Language;
use marrow_core::normalize::{Line, LineKind};
use marrow_core::select::tracked_language;
use marrow_store::{SnapshotLine, Store};

pub const TOOL: &str = "claude_code";

pub const BACKFILL_UNFINISHED: &str =
    "a backfill was started and hasn't finished; run marrow backfill to finish it first";

/// Capture waits for an unfinished backfill: both write the same last-known file states, and a
/// write landing in the middle of a backfill would leave it resuming from a state it didn't make.
pub fn backfill_unfinished(recording: &marrow_store::Recording<'_>) -> Result<bool, String> {
    Ok(recording
        .meta(BACKFILL_STATE)
        .map_err(|error| format!("{error}"))?
        .is_some_and(|state| state == BACKFILL_RUNNING))
}

pub const BACKFILL_STATE: &str = "backfill.state";
pub const BACKFILL_RUNNING: &str = "running";

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
    // PRD §7.4: the store is local and never committed.
    keep_out_of_git(root, ".marrow/");
    Store::open(&marrow.join("db.sqlite")).map_err(|error| format!("{error}"))
}

/// Adds `pattern` to `.git/info/exclude`, which ignores it in this clone only, without touching
/// the repository's own `.gitignore`.
pub fn keep_out_of_git(root: &Path, pattern: &str) {
    let info = root.join(".git").join("info");
    if std::fs::create_dir_all(&info).is_err() {
        return;
    }
    let exclude = info.join("exclude");
    let current = std::fs::read_to_string(&exclude).unwrap_or_default();
    if current.lines().any(|line| line.trim() == pattern) {
        return;
    }
    let separator = if current.is_empty() || current.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    let _ = std::fs::write(&exclude, format!("{current}{separator}{pattern}\n"));
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

/// Tracked files the working tree has, so a file that was written but never committed isn't
/// mistaken for a deleted one.
pub fn working_tree_files(root: &Path) -> Vec<(String, PathBuf)> {
    let mut paths = Vec::new();
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(relative) = relative_path(root, &path) else {
                continue;
            };
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                // No `.git` holds source, wherever it is, and anything marrow excludes by path
                // is excluded wholesale, so neither is worth walking into.
                if entry.file_name() == *".git"
                    || tracked_language(&format!("{relative}/probe.rs")).is_none()
                {
                    continue;
                }
                directories.push(path);
            } else if tracked_language(&relative).is_some() {
                paths.push((relative, path));
            }
        }
    }
    paths
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or_default()
}
