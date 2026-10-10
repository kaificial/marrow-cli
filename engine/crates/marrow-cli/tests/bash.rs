//! Agent edits made through the Bash tool, end to end: the hook before the command notes the
//! files, the hook after it records what the command changed as the agent's.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use marrow_store::Store;

const HELPER: &str = "def normalize_name(raw):\n    trimmed = raw.strip()\n    lowered = trimmed.lower()\n    return lowered.replace(\" \", \"_\")\n";
const ADDED: &str = "\n\ndef shout(text):\n    louder = text.upper()\n    return louder + \"!\"\n";

fn repo(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("marrow-bash-{name}"));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a scratch directory");
    git(&directory, &["init", "-q", "--template="]);
    git(&directory, &["config", "user.name", "Test Person"]);
    git(&directory, &["config", "user.email", "person@example.com"]);
    git(&directory, &["config", "commit.gpgsign", "false"]);
    std::fs::write(directory.join("names.py"), HELPER).unwrap();
    git(&directory, &["add", "."]);
    git(&directory, &["commit", "-q", "-m", "a person's helper"]);
    assert!(marrow(&directory, &["backfill"], "").status.success());
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

fn marrow(directory: &Path, args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_marrow"))
        .args(args)
        .current_dir(directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("runs marrow");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    child.wait_with_output().expect("marrow finishes")
}

/// Claude Code's payload for a Bash tool call, sent to the hook the way Claude Code sends it.
fn hook(directory: &Path, event: &str, command: &str) -> Output {
    let payload = serde_json::json!({
        "session_id": "session-9",
        "cwd": directory.display().to_string(),
        "hook_event_name": event,
        "tool_name": "Bash",
        "tool_use_id": "toolu_1",
        "tool_input": {"command": command},
    });
    let output = marrow(directory, &["record", "--hook"], &payload.to_string());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

/// Runs `change` between the two hooks, as if it were the Bash command.
fn bash(directory: &Path, command: &str, change: impl FnOnce()) -> String {
    hook(directory, "PreToolUse", command);
    change();
    String::from_utf8_lossy(&hook(directory, "PostToolUse", command).stdout).into_owned()
}

fn agent_lines(directory: &Path) -> i64 {
    let store = Store::open(&directory.join(".marrow").join("db.sqlite")).expect("opens the store");
    store
        .connection()
        .query_row(
            "SELECT count(*) FROM lines WHERE session_id = 'session-9'",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

fn append(path: &Path, text: &str) {
    let mut contents = std::fs::read_to_string(path).unwrap();
    contents.push_str(text);
    std::fs::write(path, contents).unwrap();
}

#[test]
fn lines_a_bash_command_adds_are_the_agents() {
    let directory = repo("edit");
    let said = bash(&directory, "python tools/add_shout.py", || {
        append(&directory.join("names.py"), ADDED)
    });
    assert!(said.contains("the command changed 1 file"), "{said}");
    assert_eq!(agent_lines(&directory), 3, "the three lines of shout");
    git(&directory, &["commit", "-qam", "commit the agent's work"]);
    assert!(marrow(&directory, &["reconcile"], "").status.success());
    assert_eq!(
        agent_lines(&directory),
        3,
        "still the agent's after the commit"
    );
}

#[test]
fn a_file_a_bash_command_creates_is_the_agents_from_its_first_line() {
    let directory = repo("create");
    bash(&directory, "cat > shout.py", || {
        std::fs::write(directory.join("shout.py"), ADDED.trim_start()).unwrap()
    });
    assert_eq!(agent_lines(&directory), 3);
}

#[test]
fn a_persons_edit_before_the_command_stays_theirs() {
    let directory = repo("person-first");
    std::fs::write(
        directory.join("mine.py"),
        "def mine(x):\n    return x * 2\n",
    )
    .unwrap();
    bash(&directory, "sed -i '$a ...' names.py", || {
        append(&directory.join("names.py"), ADDED)
    });
    assert_eq!(
        agent_lines(&directory),
        3,
        "only names.py changed during the command"
    );
}

#[test]
fn restoring_files_from_git_is_not_a_write() {
    let directory = repo("restore");
    append(&directory.join("names.py"), ADDED);
    bash(&directory, "git checkout -- names.py", || {
        git(&directory, &["checkout", "--", "names.py"])
    });
    assert_eq!(agent_lines(&directory), 0);
}

#[test]
fn a_command_that_moves_head_is_left_to_reconciliation() {
    let directory = repo("head");
    bash(&directory, "./make-commit.sh", || {
        append(&directory.join("names.py"), ADDED);
        git(&directory, &["commit", "-qam", "made by a script"]);
    });
    assert_eq!(agent_lines(&directory), 0);
}

#[test]
fn ignored_files_are_never_credited() {
    let directory = repo("ignored");
    std::fs::write(directory.join(".gitignore"), "generated/\n").unwrap();
    bash(&directory, "python codegen.py", || {
        std::fs::create_dir_all(directory.join("generated")).unwrap();
        std::fs::write(
            directory.join("generated").join("out.py"),
            ADDED.trim_start(),
        )
        .unwrap();
    });
    assert_eq!(agent_lines(&directory), 0);
}

#[test]
fn without_the_note_from_before_nothing_is_guessed() {
    let directory = repo("no-note");
    append(&directory.join("names.py"), ADDED);
    hook(&directory, "PostToolUse", "sed -i ... names.py");
    assert_eq!(agent_lines(&directory), 0);
}
