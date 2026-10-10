//! `marrow uninstall`: stop watching the repository in the current directory. It takes out the
//! hooks `marrow init` put in, leaves anything a person wrote alone, and keeps `.marrow/` unless
//! asked to delete it. A tool that watches what you write should be as easy to remove as to add.

use std::fmt::Write;
use std::path::Path;

use serde_json::Value;

use crate::capture::repository_root;
use crate::reconcile::{HOOKS, INSTALLED};

pub struct Removal {
    pub text: String,
    /// False when something is left that has to be removed by hand.
    pub complete: bool,
}

pub fn uninstall(purge: bool) -> Result<Removal, String> {
    let cwd = std::env::current_dir().map_err(|error| format!("can't find the cwd: {error}"))?;
    let root =
        repository_root(&cwd.canonicalize().unwrap_or(cwd)).ok_or("not inside a git repository")?;
    let mut removal = Removal {
        text: String::new(),
        complete: true,
    };

    let local = root.join(".claude").join("settings.local.json");
    match remove_marrow_hooks(&local) {
        Ok(0) => removal.say("ok  ", "no marrow hooks in .claude/settings.local.json"),
        Ok(count) => removal.say(
            "ok  ",
            &format!("removed {count} marrow hook(s) from .claude/settings.local.json"),
        ),
        Err(message) => removal.warn(&message),
    }
    // The shared settings file belongs to the project, so marrow only points out its hooks.
    let shared = root.join(".claude").join("settings.json");
    if std::fs::read_to_string(&shared)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .is_some_and(|settings| count_marrow_hooks(&settings) > 0)
    {
        removal.warn(
            ".claude/settings.json runs marrow too. It is shared with the project, so remove\n\
             those hooks by hand",
        );
    }

    for name in HOOKS {
        let hook = root.join(".git").join("hooks").join(name);
        let Ok(script) = std::fs::read_to_string(&hook) else {
            continue;
        };
        if script.contains(INSTALLED) {
            std::fs::remove_file(&hook)
                .map_err(|error| format!("can't remove .git/hooks/{name}: {error}"))?;
            removal.say("ok  ", &format!("removed .git/hooks/{name}"));
        } else if script.contains("marrow") {
            removal.warn(&format!(
                ".git/hooks/{name} is yours and also runs marrow. Remove that line by hand"
            ));
        }
    }

    let data = root.join(".marrow");
    if purge {
        if data.exists() {
            std::fs::remove_dir_all(&data)
                .map_err(|error| format!("can't delete .marrow/: {error}"))?;
            removal.say("ok  ", "deleted .marrow/ and everything marrow recorded");
        }
    } else if data.exists() {
        removal.say(
            "ok  ",
            "kept .marrow/, which holds what marrow recorded. Delete it with: marrow uninstall \
             --purge",
        );
    }
    Ok(removal)
}

impl Removal {
    fn say(&mut self, mark: &str, message: &str) {
        let mut lines = message.lines();
        let _ = writeln!(self.text, "{mark}  {}", lines.next().unwrap_or_default());
        for line in lines {
            let _ = writeln!(self.text, "      {line}");
        }
    }

    fn warn(&mut self, message: &str) {
        self.complete = false;
        self.say("warn", message);
    }
}

/// Takes every hook command that runs marrow out of a settings file, and any entry left empty
/// by that, keeping everything else as it was. Returns how many commands it took out.
fn remove_marrow_hooks(path: &Path) -> Result<usize, String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Ok(0);
    };
    let Ok(mut settings) = serde_json::from_str::<Value>(&text) else {
        return Err(
            ".claude/settings.local.json isn't valid JSON, so marrow left it alone. Remove its\n\
             marrow hooks by hand"
                .to_owned(),
        );
    };
    let removed = count_marrow_hooks(&settings);
    if removed == 0 {
        return Ok(0);
    }
    if let Some(events) = settings.get_mut("hooks").and_then(Value::as_object_mut) {
        for entries in events.values_mut().filter_map(Value::as_array_mut) {
            for entry in entries.iter_mut() {
                if let Some(hooks) = entry.get_mut("hooks").and_then(Value::as_array_mut) {
                    hooks.retain(|hook| !runs_marrow(hook));
                }
            }
            entries.retain(|entry| {
                entry
                    .get("hooks")
                    .and_then(Value::as_array)
                    .is_none_or(|hooks| !hooks.is_empty())
            });
        }
        events.retain(|_, entries| entries.as_array().is_none_or(|list| !list.is_empty()));
    }
    let text = serde_json::to_string_pretty(&settings).expect("a JSON value always serializes");
    std::fs::write(path, format!("{text}\n"))
        .map_err(|error| format!("can't write .claude/settings.local.json: {error}"))?;
    Ok(removed)
}

fn count_marrow_hooks(settings: &Value) -> usize {
    settings
        .get("hooks")
        .and_then(Value::as_object)
        .map(|events| {
            events
                .values()
                .filter_map(Value::as_array)
                .flatten()
                .filter_map(|entry| entry.get("hooks")?.as_array())
                .flatten()
                .filter(|hook| runs_marrow(hook))
                .count()
        })
        .unwrap_or(0)
}

/// A hook that runs marrow's own commands, by any route: its binary, a wrapper script, PATH.
fn runs_marrow(hook: &Value) -> bool {
    hook.get("command")
        .and_then(Value::as_str)
        .is_some_and(|command| command.contains("marrow") && command.contains("record"))
}
