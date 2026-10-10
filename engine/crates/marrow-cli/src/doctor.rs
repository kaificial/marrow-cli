//! `marrow doctor`: is marrow actually watching this repository, and can this build of it be
//! trusted (PRD §7.1)? Four checks: the Claude Code write hook, the post-commit hook, how much of
//! the history the store holds, and the mutation-corpus result for this exact build.

use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use marrow_core::history::Repository;
use marrow_store::{Store, CAPTURE_STARTED};
use serde_json::Value;

use crate::backfill::short;
use crate::capture::{date, repository_root, BACKFILL_RUNNING, BACKFILL_STATE};

/// The meta key a backfill leaves naming the last commit it recorded.
const BACKFILL_LAST_COMMIT: &str = "backfill.last_commit";
const BACKFILL_SINCE: &str = "backfill.since";

/// What doctor found: the report, and whether anything needs attention.
pub struct Checkup {
    pub text: String,
    pub healthy: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    Ok,
    Warn,
}

struct Report {
    text: String,
    healthy: bool,
}

impl Report {
    fn check(&mut self, verdict: Verdict, name: &str, detail: &str) {
        let mark = match verdict {
            Verdict::Ok => "ok  ",
            Verdict::Warn => {
                self.healthy = false;
                "warn"
            }
        };
        let mut lines = detail.lines();
        let _ = writeln!(
            self.text,
            "{mark}  {name:<17}{}",
            lines.next().unwrap_or_default()
        );
        for line in lines {
            let _ = writeln!(self.text, "      {:<17}{line}", "");
        }
    }
}

pub fn doctor() -> Result<Checkup, String> {
    let cwd = std::env::current_dir().map_err(|error| format!("can't find the cwd: {error}"))?;
    let root =
        repository_root(&cwd.canonicalize().unwrap_or(cwd)).ok_or("not inside a git repository")?;
    let mut report = Report {
        text: String::new(),
        healthy: true,
    };

    let (verdict, detail) = write_hook(&root);
    report.check(verdict, "write hook", &detail);
    let (verdict, detail) = commit_hook(&root);
    report.check(verdict, "commit hook", &detail);

    let database = root.join(".marrow").join("db.sqlite");
    if database.exists() {
        match Store::open(&database) {
            Ok(store) => {
                let (verdict, detail) = history(&root, &store)?;
                report.check(verdict, "history", &detail);
            }
            Err(error) => report.check(Verdict::Warn, "store", &error.to_string()),
        }
    } else {
        report.check(
            Verdict::Warn,
            "store",
            "no .marrow/db.sqlite yet. Run marrow init to create it",
        );
    }

    let executable = std::env::current_exe()
        .map_err(|error| format!("can't find the marrow executable: {error}"))?;
    let (verdict, detail) = corpus(&executable);
    report.check(verdict, "mutation corpus", &detail);

    Ok(Checkup {
        text: report.text,
        healthy: report.healthy,
    })
}

/// Claude Code reads hooks from the project's shared and local settings and from the user's own.
/// The hook counts if a `PostToolUse` entry whose matcher covers both Edit and Write runs a
/// command that mentions marrow.
pub(crate) fn write_hook(root: &Path) -> (Verdict, String) {
    let mut places = vec![
        root.join(".claude").join("settings.json"),
        root.join(".claude").join("settings.local.json"),
    ];
    if let Some(home) = home_directory() {
        places.push(home.join(".claude").join("settings.json"));
    }
    let mut partial = None;
    let mut stale = None;
    for place in &places {
        let Ok(text) = std::fs::read_to_string(place) else {
            continue;
        };
        let Ok(settings) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        match registered(&settings) {
            Coverage::Full => match missing_binary(&settings) {
                None => return (Verdict::Ok, format!("registered in {}", shown(root, place))),
                Some(binary) => stale = Some((place, binary)),
            },
            Coverage::Partial => partial = Some(place),
            Coverage::None => {}
        }
    }
    if let Some((place, binary)) = stale {
        return (
            Verdict::Warn,
            format!(
                "{} runs {binary}, which doesn't exist, so agent writes aren't being\nrecorded. \
                 Run marrow init again with the marrow you use now",
                shown(root, place)
            ),
        );
    }
    match partial {
        Some(place) => (
            Verdict::Warn,
            format!(
                "{} runs marrow on PostToolUse, but its matcher doesn't cover both Edit and \
                 Write,\nso some agent writes won't be recorded",
                shown(root, place)
            ),
        ),
        None => (
            Verdict::Warn,
            "no PostToolUse hook runs marrow, so agent writes aren't being recorded.\nRun marrow \
             init, or add one to .claude/settings.json that runs: marrow record --hook"
                .to_owned(),
        ),
    }
}

enum Coverage {
    Full,
    Partial,
    None,
}

fn registered(settings: &Value) -> Coverage {
    let Some(entries) = settings
        .pointer("/hooks/PostToolUse")
        .and_then(Value::as_array)
    else {
        return Coverage::None;
    };
    let mut edit = false;
    let mut write = false;
    for entry in entries {
        let runs_marrow = entry
            .get("hooks")
            .and_then(Value::as_array)
            .is_some_and(|hooks| {
                hooks.iter().any(|hook| {
                    hook.get("command")
                        .and_then(Value::as_str)
                        .is_some_and(|command| command.contains("marrow"))
                })
            });
        if !runs_marrow {
            continue;
        }
        let matcher = entry.get("matcher").and_then(Value::as_str).unwrap_or("");
        edit |= covers(matcher, "Edit");
        write |= covers(matcher, "Write");
    }
    match (edit, write) {
        (true, true) => Coverage::Full,
        (false, false) => Coverage::None,
        _ => Coverage::Partial,
    }
}

/// A hook `marrow init` wrote names the binary by its full path, in quotes. If that file is gone
/// (the binary was moved, or rebuilt elsewhere) the hook fails on every write. Commands that
/// find marrow some other way, through `PATH` or a script, aren't checked.
fn missing_binary(settings: &Value) -> Option<String> {
    let entries = settings.pointer("/hooks/PostToolUse")?.as_array()?;
    entries
        .iter()
        .filter_map(|entry| entry.get("hooks")?.as_array())
        .flatten()
        .filter_map(|hook| hook.get("command")?.as_str())
        .filter_map(quoted_marrow_binary)
        .find(|binary| !Path::new(binary).exists())
}

/// The quoted path at the start of `command`, when it names a marrow binary outright.
pub(crate) fn quoted_marrow_binary(command: &str) -> Option<String> {
    let rest = command.trim_start().strip_prefix('"')?;
    let path = &rest[..rest.find('"')?];
    let name = Path::new(path).file_stem()?.to_str()?;
    (name == "marrow" && !path.contains('$')).then(|| path.to_owned())
}

/// Claude Code matchers are tool names, `|`-separated alternatives of them, or empty or `*` for
/// every tool.
fn covers(matcher: &str, tool: &str) -> bool {
    let matcher = matcher.trim();
    matcher.is_empty()
        || matcher == "*"
        || matcher
            .split('|')
            .any(|alternative| alternative.trim() == tool)
}

fn commit_hook(root: &Path) -> (Verdict, String) {
    let hook = root.join(".git").join("hooks").join("post-commit");
    match std::fs::read_to_string(&hook) {
        Ok(script) if script.contains("marrow") && script.contains("reconcile") => {
            (Verdict::Ok, "post-commit runs marrow reconcile".to_owned())
        }
        Ok(_) => (
            Verdict::Warn,
            "a post-commit hook exists but doesn't run marrow reconcile, so commits aren't\n\
             checkpointed. Add a line running marrow reconcile to it"
                .to_owned(),
        ),
        Err(_) => (
            Verdict::Warn,
            "no post-commit hook, so commits aren't checkpointed. Run: marrow init".to_owned(),
        ),
    }
}

/// How much of the history the store holds: whether a backfill ran and finished, and whether the
/// commit checked out now has been recorded.
fn history(root: &Path, store: &Store) -> Result<(Verdict, String), String> {
    let meta = |key| store.meta(key).map_err(|error| format!("{error}"));
    let mut verdict = Verdict::Ok;
    let mut lines = Vec::new();
    match meta(BACKFILL_STATE)?.as_deref() {
        Some(BACKFILL_RUNNING) => {
            verdict = Verdict::Warn;
            lines.push(
                "a backfill was interrupted. Run marrow backfill to finish it; capture waits \
                 until it does"
                    .to_owned(),
            );
        }
        Some(_) => {
            let through = meta(BACKFILL_LAST_COMMIT)?
                .map(|sha| format!(" through {}", short(&sha).to_owned()))
                .unwrap_or_default();
            let window = meta(BACKFILL_SINCE)?
                .map(|since| format!(", limited to {since}"))
                .unwrap_or_else(|| ", whole first-parent history".to_owned());
            lines.push(format!("backfill complete{through}{window}"));
        }
        None => match meta(CAPTURE_STARTED)? {
            Some(started) => {
                let when = started
                    .parse::<i64>()
                    .map(date)
                    .unwrap_or_else(|_| started.clone());
                lines.push(format!(
                    "no backfill: history before capture started ({when}) isn't recorded, and \
                     can't be\nadded now (docs/decisions/0009-backfill.md)"
                ));
            }
            None => {
                verdict = Verdict::Warn;
                lines.push(
                    "no backfill and no capture yet. Run marrow backfill to record the \
                     history so far"
                        .to_owned(),
                );
            }
        },
    }
    if let Ok(repository) = Repository::open(root) {
        if let Ok(head) = repository.head() {
            let recorded = store
                .commit(&head.sha)
                .map_err(|error| format!("{error}"))?
                .is_some();
            if recorded {
                lines.push(format!("HEAD ({}) is recorded", short(&head.sha)));
            } else {
                verdict = Verdict::Warn;
                lines.push(format!(
                    "HEAD ({}) isn't recorded. Run marrow reconcile, and check the commit hook",
                    short(&head.sha)
                ));
            }
        }
    }
    Ok((verdict, lines.join("\n")))
}

/// `just eval` leaves its result beside the binary it graded (eval/runner/run.py). The binary's
/// size and modification time say whether that result is for this build or an older one.
fn corpus(executable: &Path) -> (Verdict, String) {
    let not_graded = |why: &str| {
        (
            Verdict::Warn,
            format!("{why}. Run just eval in the marrow repository to grade it"),
        )
    };
    let record_path = PathBuf::from(format!("{}.eval.json", executable.display()));
    let Ok(text) = std::fs::read_to_string(&record_path) else {
        return not_graded("this build hasn't been graded");
    };
    let Ok(record) = serde_json::from_str::<Value>(&text) else {
        return not_graded("the grading record beside this build can't be read");
    };
    let Ok(metadata) = std::fs::metadata(executable) else {
        return not_graded("can't read this build's file details");
    };
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos());
    let recorded_size = record.get("binary_size").and_then(Value::as_u64);
    let recorded_modified = record
        .get("binary_mtime_ns")
        .and_then(Value::as_u64)
        .map(u128::from);
    if recorded_size != Some(metadata.len()) || recorded_modified != modified {
        return not_graded("the last grading was of an earlier build");
    }
    let number = |key: &str| record.get(key).and_then(Value::as_u64).unwrap_or(0);
    let fixtures = number("fixtures");
    let passed = number("fixtures_passed");
    let unscored = number("fixtures_unscored");
    let decisions = number("fate_decisions");
    let correct = number("fate_decisions_correct");
    let graded = record
        .get("graded_at")
        .and_then(Value::as_i64)
        .map(date)
        .unwrap_or_else(|| "an unknown date".to_owned());
    let detail = format!(
        "{passed} of {fixtures} fixtures pass, {correct} of {decisions} fate decisions right \
         (graded {graded})"
    );
    if passed == fixtures && unscored == 0 {
        (Verdict::Ok, detail)
    } else {
        (Verdict::Warn, detail)
    }
}

fn home_directory() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn shown(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn matchers_cover_named_tools_alternatives_and_everything() {
        assert!(covers("Edit|Write", "Edit"));
        assert!(covers("Edit | Write", "Write"));
        assert!(covers("", "Write"));
        assert!(covers("*", "Edit"));
        assert!(!covers("Edit", "Write"));
        assert!(!covers("MultiEdit", "Edit"));
    }

    #[test]
    fn a_hook_counts_only_if_it_runs_marrow_on_both_tools() {
        let entry = |matcher: &str, command: &str| {
            json!({"hooks": {"PostToolUse": [
                {"matcher": matcher, "hooks": [{"type": "command", "command": command}]}
            ]}})
        };
        assert!(matches!(
            registered(&entry("Edit|Write", "marrow record --hook")),
            Coverage::Full
        ));
        assert!(matches!(
            registered(&entry("Edit", "marrow record --hook")),
            Coverage::Partial
        ));
        assert!(matches!(
            registered(&entry("Edit|Write", "cargo fmt")),
            Coverage::None
        ));
        assert!(matches!(registered(&json!({})), Coverage::None));
    }

    #[test]
    fn only_a_quoted_marrow_binary_is_checked_for() {
        assert_eq!(
            quoted_marrow_binary("\"C:/tools/marrow.exe\" record --hook").as_deref(),
            Some("C:/tools/marrow.exe")
        );
        assert_eq!(quoted_marrow_binary("marrow record --hook"), None);
        assert_eq!(
            quoted_marrow_binary("python \"$CLAUDE_PROJECT_DIR/.claude/hooks/marrow_record.py\""),
            None
        );
        let gone = json!({"hooks": {"PostToolUse": [{"matcher": "Edit|Write", "hooks": [
            {"type": "command", "command": "\"/no/such/place/marrow\" record --hook"}
        ]}]}});
        assert_eq!(
            missing_binary(&gone).as_deref(),
            Some("/no/such/place/marrow")
        );
    }
}
