//! `marrow init` end to end: what it creates, what it leaves alone, and that running it twice
//! changes nothing.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

fn repo(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("marrow-init-{name}"));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a scratch directory");
    git(&directory, &["init", "-q", "--template=", "-b", "main"]);
    git(&directory, &["config", "user.name", "Test Person"]);
    git(&directory, &["config", "user.email", "person@example.com"]);
    git(&directory, &["config", "commit.gpgsign", "false"]);
    std::fs::write(
        directory.join("app.py"),
        "def add(a, b):\n    return a + b\n",
    )
    .unwrap();
    git(&directory, &["add", "."]);
    git(&directory, &["commit", "-q", "-m", "start"]);
    directory
}

fn git(directory: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(directory)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .expect("runs git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Runs marrow with the home directory pointed at the repository, so a hook in the real user's
/// own Claude Code settings can't make init think the work is already done.
fn marrow(directory: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_marrow"))
        .args(args)
        .current_dir(directory)
        .env("HOME", directory)
        .env("USERPROFILE", directory)
        .output()
        .expect("runs marrow")
}

fn settings(directory: &Path) -> Value {
    let text = std::fs::read_to_string(directory.join(".claude").join("settings.local.json"))
        .expect("init wrote the settings");
    serde_json::from_str(&text).expect("valid JSON")
}

fn marrow_hooks(settings: &Value) -> usize {
    settings["hooks"]["PostToolUse"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter(|entry| {
                    entry["matcher"] == "Edit|Write"
                        && entry["hooks"][0]["command"]
                            .as_str()
                            .is_some_and(|command| command.ends_with("record --hook"))
                })
                .count()
        })
        .unwrap_or(0)
}

#[test]
fn init_sets_up_the_store_and_both_hooks() {
    let directory = repo("fresh");
    let output = marrow(&directory, &["init", "--no-backfill"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(directory.join(".marrow").join("db.sqlite").exists());
    assert_eq!(marrow_hooks(&settings(&directory)), 1);
    let post_commit =
        std::fs::read_to_string(directory.join(".git").join("hooks").join("post-commit"))
            .expect("init installed the post-commit hook");
    assert!(post_commit.contains("reconcile"));
    let exclude =
        std::fs::read_to_string(directory.join(".git").join("info").join("exclude")).unwrap();
    assert!(exclude.lines().any(|line| line == ".marrow/"));
    assert!(exclude
        .lines()
        .any(|line| line == ".claude/settings.local.json"));

    let doctor = String::from_utf8_lossy(&marrow(&directory, &["doctor"]).stdout).into_owned();
    assert!(doctor.contains("ok    write hook"), "{doctor}");
    assert!(doctor.contains("ok    commit hook"), "{doctor}");
}

#[test]
fn running_init_twice_registers_the_hook_once() {
    let directory = repo("twice");
    assert!(marrow(&directory, &["init", "--no-backfill"])
        .status
        .success());
    let again = marrow(&directory, &["init", "--no-backfill"]);
    assert!(again.status.success());
    assert!(String::from_utf8_lossy(&again.stdout).contains("already"));
    assert_eq!(marrow_hooks(&settings(&directory)), 1);
}

#[test]
fn init_keeps_what_the_settings_file_already_holds() {
    let directory = repo("existing");
    std::fs::create_dir_all(directory.join(".claude")).unwrap();
    std::fs::write(
        directory.join(".claude").join("settings.local.json"),
        r#"{"permissions": {"allow": ["Bash(ls)"]}, "hooks": {"PostToolUse": [
            {"matcher": "Write", "hooks": [{"type": "command", "command": "echo hi"}]}
        ]}}"#,
    )
    .unwrap();
    assert!(marrow(&directory, &["init", "--no-backfill"])
        .status
        .success());
    let settings = settings(&directory);
    assert_eq!(settings["permissions"]["allow"][0], "Bash(ls)");
    assert_eq!(
        settings["hooks"]["PostToolUse"][0]["hooks"][0]["command"],
        "echo hi"
    );
    assert_eq!(marrow_hooks(&settings), 1);
}

#[test]
fn a_settings_file_that_isnt_json_is_left_alone() {
    let directory = repo("broken");
    std::fs::create_dir_all(directory.join(".claude")).unwrap();
    let path = directory.join(".claude").join("settings.local.json");
    std::fs::write(&path, "{ not json").unwrap();
    let output = marrow(&directory, &["init", "--no-backfill"]);
    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("left it alone"), "{stdout}");
    assert!(stdout.contains("record --hook"), "{stdout}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
    // The other steps still ran.
    assert!(directory
        .join(".git")
        .join("hooks")
        .join("post-commit")
        .exists());
}

#[test]
fn init_can_record_the_history_so_far() {
    let directory = repo("backfill");
    let output = marrow(&directory, &["init", "--backfill"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("recorded 1 commit "), "{stdout}");
    assert!(stdout.contains("in total: 1 commit ("), "{stdout}");
}

#[test]
fn a_hook_whose_binary_has_moved_is_reported_and_replaced() {
    let directory = repo("moved");
    std::fs::create_dir_all(directory.join(".claude")).unwrap();
    std::fs::write(
        directory.join(".claude").join("settings.local.json"),
        r#"{"hooks": {"PostToolUse": [{"matcher": "Edit|Write", "hooks": [
            {"type": "command", "command": "\"/no/such/place/marrow\" record --hook"}
        ]}]}}"#,
    )
    .unwrap();
    let doctor = String::from_utf8_lossy(&marrow(&directory, &["doctor"]).stdout).into_owned();
    assert!(doctor.contains("warn  write hook"), "{doctor}");
    assert!(doctor.contains("doesn't exist"), "{doctor}");

    assert!(marrow(&directory, &["init", "--no-backfill"])
        .status
        .success());
    let settings = settings(&directory);
    assert_eq!(marrow_hooks(&settings), 1);
    assert!(!settings.to_string().contains("no/such/place"));
    let doctor = String::from_utf8_lossy(&marrow(&directory, &["doctor"]).stdout).into_owned();
    assert!(doctor.contains("ok    write hook"), "{doctor}");
}
