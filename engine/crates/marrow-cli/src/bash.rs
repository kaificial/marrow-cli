//! Agent edits made through Claude Code's Bash tool (docs/decisions/0015-bash-writes.md).
//!
//! No Edit or Write hook fires for `sed -i`, `cat > file`, a script or `git apply`, so without
//! this those lines would reach a commit looking like a person's. Before a Bash command runs,
//! marrow notes the size and modification time of every file it tracks; afterwards it records
//! each file the command changed as an agent write. A command that moves HEAD or restores files
//! from git (a checkout, a stash, a reset) changes files without writing them, and is left to
//! reconciliation.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use marrow_core::history::Repository;
use marrow_core::path_hash;
use serde_json::{json, Value};

use crate::capture::{repository_root, working_tree_files};
use crate::record::{self, Capture};

/// Git subcommands that change files by restoring them rather than by writing new content.
const RESTORING: [&str; 11] = [
    "checkout",
    "switch",
    "restore",
    "reset",
    "stash",
    "merge",
    "rebase",
    "pull",
    "cherry-pick",
    "revert",
    "am",
];

/// A note left by a command whose PostToolUse never came (an interrupt) is cleared after this.
const STALE: Duration = Duration::from_secs(24 * 3_600);

pub struct Payload<'a> {
    pub session: &'a str,
    pub cwd: &'a Path,
    pub key: &'a str,
    pub command: &'a str,
}

/// What a Bash command did to the files marrow tracks.
pub struct Outcome {
    pub files: usize,
    pub born: usize,
    pub changed: usize,
}

/// PreToolUse: note every tracked file's size and modification time, and HEAD.
pub fn before(payload: &Payload<'_>) -> Result<(), String> {
    let Some(root) = repository_root(payload.cwd) else {
        return Ok(());
    };
    let directory = notes(&root);
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("can't create {}: {error}", directory.display()))?;
    clear_stale(&directory);
    let files: serde_json::Map<String, Value> = stamps(&root)
        .into_iter()
        .map(|(path, stamp)| (path_hash(&path), json!(stamp)))
        .collect();
    let note = json!({"head": head(&root), "files": files});
    std::fs::write(note_path(&root, payload.key), note.to_string())
        .map_err(|error| format!("can't write the note for this command: {error}"))
}

/// PostToolUse: record each tracked file the command changed as an agent write.
pub fn after(payload: &Payload<'_>) -> Result<Option<Outcome>, String> {
    let Some(root) = repository_root(payload.cwd) else {
        return Ok(None);
    };
    let path = note_path(&root, payload.key);
    // Without a note from before the command there's no telling what it changed, so nothing is
    // guessed at.
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(None);
    };
    let _ = std::fs::remove_file(&path);
    let note: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if note["head"] != json!(head(&root)) || restores_from_git(payload.command) {
        return Ok(None);
    }
    let mut outcome = Outcome {
        files: 0,
        born: 0,
        changed: 0,
    };
    let touched: Vec<(String, bool)> = stamps(&root)
        .into_iter()
        .filter_map(|(path, stamp)| {
            let earlier = &note["files"][path_hash(&path)];
            (*earlier != json!(stamp)).then(|| (path, earlier.is_null()))
        })
        .collect();
    if touched.is_empty() {
        return Ok(Some(outcome));
    }
    let ignored = ignored(&root, touched.iter().map(|(path, _)| path.as_str()))?;
    for (path, created) in touched {
        if ignored.contains(&path) {
            continue;
        }
        // A file the command created is the agent's from its first line; one it changed was
        // partly there before.
        let tool = if created { "Write" } else { "Bash" };
        match record::capture(payload.session, &root.join(&path), None, Some(tool))? {
            Capture::Recorded { born, changed, .. } => {
                outcome.files += 1;
                outcome.born += born;
                outcome.changed += changed;
            }
            Capture::Skipped(_) => {}
        }
    }
    Ok(Some(outcome))
}

/// Size and modification time, in nanoseconds, of every file in the working tree marrow tracks,
/// by repository-relative path. It runs before and after every Bash command, so it stays in this
/// process: starting git for it would cost more than most commands take.
fn stamps(root: &Path) -> HashMap<String, (u64, u64)> {
    working_tree_files(root)
        .into_iter()
        .filter_map(|(relative, path)| {
            let metadata = std::fs::metadata(path).ok()?;
            let modified = metadata
                .modified()
                .ok()?
                .duration_since(UNIX_EPOCH)
                .ok()?
                .as_nanos() as u64;
            Some((relative, (metadata.len(), modified)))
        })
        .collect()
}

/// Which of `paths` git ignores, so build output a command writes is never credited to anyone.
/// Asked only about files a command changed, which most commands don't.
fn ignored<'a>(root: &Path, paths: impl Iterator<Item = &'a str>) -> Result<Vec<String>, String> {
    let mut child = Command::new("git")
        .args(["check-ignore", "-z", "--stdin"])
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("can't run git check-ignore: {error}"))?;
    let list: Vec<u8> = paths
        .flat_map(|path| [path.as_bytes(), b"\0"].concat())
        .collect();
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(&list)
            .map_err(|error| format!("can't ask git check-ignore: {error}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("git check-ignore failed: {error}"))?;
    // It exits 1 when nothing is ignored; anything above that is a real failure, and then
    // nothing is recorded rather than something guessed.
    if output.status.code().is_none_or(|code| code > 1) {
        return Err(format!(
            "git check-ignore failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output
        .stdout
        .split(|&byte| byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| String::from_utf8_lossy(path).into_owned())
        .collect())
}

/// The commit HEAD points at, or None before the first commit. It is read straight from `.git`
/// where that is a plain directory, which is far quicker than opening the repository, and runs
/// twice per Bash command.
fn head(root: &Path) -> Option<String> {
    let git = root.join(".git");
    if git.is_dir() {
        let text = std::fs::read_to_string(git.join("HEAD")).ok()?;
        let text = text.trim();
        let Some(reference) = text.strip_prefix("ref: ") else {
            return Some(text.to_owned());
        };
        if let Ok(sha) = std::fs::read_to_string(git.join(reference)) {
            return Some(sha.trim().to_owned());
        }
        let packed = std::fs::read_to_string(git.join("packed-refs")).unwrap_or_default();
        return packed.lines().find_map(|line| {
            let (sha, name) = line.split_once(' ')?;
            (name == reference).then(|| sha.to_owned())
        });
    }
    // A worktree or submodule keeps its git directory elsewhere.
    let repository = Repository::open(root).ok()?;
    if !repository.has_commits().ok()? {
        return None;
    }
    Some(repository.head().ok()?.sha)
}

/// Whether the command runs a git subcommand that restores files rather than writing them.
pub fn restores_from_git(command: &str) -> bool {
    let words: Vec<&str> = command
        .split(|c: char| c.is_whitespace() || ";&|()".contains(c))
        .filter(|word| !word.is_empty())
        .collect();
    let mut index = 0;
    while index < words.len() {
        let word = words[index];
        index += 1;
        if word != "git" && !word.ends_with("/git") && !word.ends_with("git.exe") {
            continue;
        }
        while index < words.len() && words[index].starts_with('-') {
            // -C and -c take the next word as their value.
            if matches!(words[index], "-C" | "-c") {
                index += 1;
            }
            index += 1;
        }
        if words.get(index).is_some_and(|sub| RESTORING.contains(sub)) {
            return true;
        }
    }
    false
}

fn notes(root: &Path) -> PathBuf {
    root.join(".marrow").join("bash")
}

/// One note per tool call, so commands Claude Code runs side by side don't share one.
fn note_path(root: &Path, key: &str) -> PathBuf {
    let name: String = key
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    notes(root).join(format!("{name}.json"))
}

fn clear_stale(directory: &Path) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > STALE);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::restores_from_git;

    #[test]
    fn git_commands_that_restore_files_are_told_apart_from_ones_that_write() {
        for command in [
            "git checkout main",
            "git stash && git stash pop",
            "cd app && git -C . restore src/x.py",
            "git -c core.autocrlf=false reset --hard HEAD~1",
            "git pull --rebase",
        ] {
            assert!(restores_from_git(command), "{command}");
        }
        for command in [
            "git apply fix.patch",
            "git status",
            "sed -i 's/a/b/' src/x.py",
            "echo checkout > notes.txt",
            "python tools/generate.py",
        ] {
            assert!(!restores_from_git(command), "{command}");
        }
    }
}
