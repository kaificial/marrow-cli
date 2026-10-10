//! `marrow init` (PRD §7.1): start watching the repository in the current directory. It creates
//! the store, registers the Claude Code write hook, and installs the post-commit and post-merge
//! hooks. Running it again is harmless: each step that is already done is left as it is.

use std::fmt::Write;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::capture::{keep_out_of_git, open_store, repository_root};
use crate::doctor::{self, Verdict};
use crate::reconcile;

/// Where init registers the write hook. The command names this machine's marrow binary by its
/// full path, so it belongs in the personal settings file rather than the shared one.
const SETTINGS: &str = ".claude/settings.local.json";

pub struct Setup {
    pub text: String,
    /// False when a step couldn't be finished and needs doing by hand.
    pub complete: bool,
}

pub fn init() -> Result<Setup, String> {
    let cwd = std::env::current_dir().map_err(|error| format!("can't find the cwd: {error}"))?;
    let root =
        repository_root(&cwd.canonicalize().unwrap_or(cwd)).ok_or("not inside a git repository")?;
    let mut setup = Setup {
        text: String::new(),
        complete: true,
    };

    let existed = root.join(".marrow").join("db.sqlite").exists();
    open_store(&root)?;
    setup.step(Ok(if existed {
        ".marrow/ already exists".to_owned()
    } else {
        "created .marrow/, kept out of git through .git/info/exclude".to_owned()
    }));

    let executable = std::env::current_exe()
        .map_err(|error| format!("can't find the marrow executable: {error}"))?;
    let repointed = repoint_hooks(&root, &executable);
    let write_hook = match doctor::write_hook(&root) {
        (Verdict::Ok, detail) => Ok(format!("write hook {detail}")),
        (Verdict::Warn, _) => add_hooks(&root, &executable, "write hook", WRITE_HOOKS),
    };
    setup.step(write_hook);
    let bash_hook = match doctor::bash_hook(&root) {
        (Verdict::Ok, detail) => Ok(format!("bash hook {detail}")),
        (Verdict::Warn, _) => add_hooks(&root, &executable, "bash hook", BASH_HOOKS),
    };
    setup.step(bash_hook);
    if let Some(outcome) = repointed {
        setup.step(outcome);
    }

    setup.step(reconcile::install(Some(&root)).map(|message| format!("commit hook {message}")));
    if doctor::in_build_directory(&executable) {
        setup.note(&format!(
            "the hooks run {}, a build that cargo clean deletes.\n\
             For hooks that last, install marrow (cargo install --path engine/crates/marrow-cli)\n\
             and run marrow init again with the installed one",
            hook_path(&executable)
        ));
    }
    Ok(setup)
}

impl Setup {
    fn step(&mut self, outcome: Result<String, String>) {
        let (mark, message) = match outcome {
            Ok(message) => ("ok  ", message),
            Err(message) => {
                self.complete = false;
                ("warn", message)
            }
        };
        let mut lines = message.lines();
        let _ = writeln!(self.text, "{mark}  {}", lines.next().unwrap_or_default());
        for line in lines {
            let _ = writeln!(self.text, "      {line}");
        }
    }

    /// Something worth knowing that doesn't stop marrow working.
    fn note(&mut self, message: &str) {
        let mut lines = message.lines();
        let _ = writeln!(self.text, "note  {}", lines.next().unwrap_or_default());
        for line in lines {
            let _ = writeln!(self.text, "      {line}");
        }
    }
}

/// The binary's path in forward slashes, so the shell Claude Code runs hooks in reads it the
/// same way on every platform.
fn hook_path(executable: &Path) -> String {
    let path = executable.display().to_string().replace('\\', "/");
    path.strip_prefix("//?/").unwrap_or(&path).to_owned()
}

fn hook_command(executable: &Path) -> String {
    format!("\"{}\" record --hook", hook_path(executable))
}

/// The write hook: after every Edit or Write.
const WRITE_HOOKS: &[(&str, &str)] = &[("PostToolUse", "Edit|Write")];
/// Around every Bash command, to see what it changed (docs/decisions/0015-bash-writes.md).
const BASH_HOOKS: &[(&str, &str)] = &[("PreToolUse", "Bash"), ("PostToolUse", "Bash")];

/// Points the hook entries an earlier init wrote at the binary running now, so installing marrow
/// somewhere new and running init again moves the hooks with it. Entries a person wrote are left
/// alone. None when there was nothing of init's to change.
fn repoint_hooks(root: &Path, executable: &Path) -> Option<Result<String, String>> {
    let path = root.join(".claude").join("settings.local.json");
    let mut settings: Value = serde_json::from_str(&std::fs::read_to_string(&path).ok()?).ok()?;
    let wanted = hook_command(executable);
    let mut changed = false;
    for event in ["PreToolUse", "PostToolUse"] {
        let Some(entries) = settings
            .pointer_mut(&format!("/hooks/{event}"))
            .and_then(Value::as_array_mut)
        else {
            continue;
        };
        for entry in entries {
            let Some(hooks) = entry.get_mut("hooks").and_then(Value::as_array_mut) else {
                continue;
            };
            for hook in hooks {
                let Some(command) = hook.get("command").and_then(Value::as_str) else {
                    continue;
                };
                let ours = doctor::quoted_marrow_binary(command)
                    .is_some_and(|binary| command == hook_command(Path::new(&binary)));
                if ours && command != wanted {
                    hook["command"] = Value::String(wanted.clone());
                    changed = true;
                }
            }
        }
    }
    if !changed {
        return None;
    }
    let text = serde_json::to_string_pretty(&settings).expect("a JSON value always serializes");
    Some(
        std::fs::write(&path, format!("{text}\n"))
            .map(|()| format!("hooks now run {}", hook_path(executable)))
            .map_err(|error| format!("can't write {SETTINGS}: {error}")),
    )
}

/// Adds hook entries to the personal settings file, keeping whatever else is in it. A file that
/// isn't a JSON object is left alone rather than overwritten.
fn add_hooks(
    root: &Path,
    executable: &Path,
    label: &str,
    wanted: &[(&str, &str)],
) -> Result<String, String> {
    let command = hook_command(executable);
    let path: PathBuf = root.join(".claude").join("settings.local.json");
    let by_hand = || {
        let places: Vec<String> = wanted
            .iter()
            .map(|(event, matcher)| format!("{event} for {matcher}"))
            .collect();
        format!(
            "Add a hook ({}) to {SETTINGS} that runs:\n{command}",
            places.join(", ")
        )
    };
    let mut settings = match std::fs::read_to_string(&path) {
        Ok(text) if text.trim().is_empty() => Value::Object(Map::new()),
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(value) if value.is_object() => value,
            _ => {
                return Err(format!(
                    "{SETTINGS} isn't a JSON object, so marrow left it alone.\n{}",
                    by_hand()
                ))
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Value::Object(Map::new()),
        Err(error) => return Err(format!("can't read {SETTINGS}: {error}\n{}", by_hand())),
    };
    let hooks = settings
        .as_object_mut()
        .expect("checked above")
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(hooks) = hooks.as_object_mut() else {
        return Err(format!(
            "the \"hooks\" in {SETTINGS} isn't an object, so marrow left it alone.\n{}",
            by_hand()
        ));
    };
    for (event, matcher) in wanted {
        let entries = hooks
            .entry(*event)
            .or_insert_with(|| Value::Array(Vec::new()));
        let Some(entries) = entries.as_array_mut() else {
            return Err(format!(
                "{event} in {SETTINGS} isn't a list, so marrow left it alone.\n{}",
                by_hand()
            ));
        };
        // An earlier init's entry whose binary has since moved would fail on every call, so it
        // is replaced rather than left beside the new one.
        entries.retain(|existing| !runs_missing_marrow(existing));
        entries.push(json!({
            "matcher": matcher,
            "hooks": [{"type": "command", "command": command}],
        }));
    }

    let directory = root.join(".claude");
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("can't create .claude/: {error}\n{}", by_hand()))?;
    let text = serde_json::to_string_pretty(&settings).expect("a JSON value always serializes");
    std::fs::write(&path, format!("{text}\n"))
        .map_err(|error| format!("can't write {SETTINGS}: {error}\n{}", by_hand()))?;
    // It names a path on this machine, so it is never worth committing.
    keep_out_of_git(root, SETTINGS);
    Ok(format!("{label} registered in {SETTINGS}"))
}

fn runs_missing_marrow(entry: &Value) -> bool {
    entry
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|hooks| {
            hooks.iter().any(|hook| {
                hook.get("command")
                    .and_then(Value::as_str)
                    .and_then(doctor::quoted_marrow_binary)
                    .is_some_and(|binary| !Path::new(&binary).exists())
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hook_command_quotes_a_forward_slash_path() {
        assert_eq!(
            hook_command(Path::new(r"\\?\C:\tools\marrow.exe")),
            "\"C:/tools/marrow.exe\" record --hook"
        );
        assert_eq!(
            hook_command(Path::new("/usr/local/bin/marrow")),
            "\"/usr/local/bin/marrow\" record --hook"
        );
    }
}
