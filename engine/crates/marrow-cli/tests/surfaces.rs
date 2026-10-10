//! `marrow explain`, `report` and `status` end to end, on real histories whose every step the
//! tests know.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use marrow_store::Store;

const HELPER: &str = "def normalize_name(raw):\n    trimmed = raw.strip()\n    lowered = trimmed.lower()\n    return lowered.replace(\" \", \"_\")\n";
const REPORT: &str =
    "def report(rows):\n    total = len(rows)\n    print(total)\n    return total\n";

fn repo(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("marrow-surfaces-{name}"));
    if directory.exists() {
        remove(&directory);
    }
    std::fs::create_dir_all(&directory).expect("a scratch directory");
    git(&directory, &["init", "-q", "--template=", "-b", "main"]);
    git(&directory, &["config", "user.name", "Test Person"]);
    git(&directory, &["config", "user.email", "person@example.com"]);
    git(&directory, &["config", "commit.gpgsign", "false"]);
    directory
}

fn remove(directory: &Path) {
    fn walk(directory: &Path, found: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(directory).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, found);
            }
            found.push(path);
        }
    }
    let mut found = Vec::new();
    walk(directory, &mut found);
    for entry in found {
        if let Ok(metadata) = std::fs::metadata(&entry) {
            let mut permissions = metadata.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            permissions.set_readonly(false);
            let _ = std::fs::set_permissions(&entry, permissions);
        }
    }
    let _ = std::fs::remove_dir_all(directory);
}

fn git(directory: &Path, args: &[&str]) -> String {
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
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn commit(directory: &Path, message: &str) -> String {
    git(directory, &["add", "-A"]);
    git(directory, &["commit", "-q", "-m", message]);
    git(directory, &["rev-parse", "HEAD"])
}

fn write(directory: &Path, name: &str, contents: &str) -> PathBuf {
    let path = directory.join(name);
    std::fs::write(&path, contents).expect("writes the file");
    path
}

fn marrow(directory: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_marrow"))
        .args(args)
        .current_dir(directory)
        .output()
        .expect("runs marrow")
}

fn succeeds(directory: &Path, args: &[&str]) -> String {
    let output = marrow(directory, args);
    assert!(
        output.status.success(),
        "marrow {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn agent_writes(directory: &Path, name: &str, contents: &str) {
    let file = write(directory, name, contents);
    succeeds(
        directory,
        &[
            "record",
            "--session",
            "session-7",
            "--model",
            "claude-opus-5",
            "--file",
            &file.to_string_lossy(),
        ],
    );
}

fn short(sha: &str) -> &str {
    &sha[..8]
}

#[test]
fn explain_shows_where_a_line_was_born_what_changed_it_and_where_it_is() {
    let directory = repo("explain");
    write(&directory, "inventory.py", HELPER);
    let born = commit(&directory, "add the helper");
    write(
        &directory,
        "inventory.py",
        &HELPER.replace("raw.strip()", "raw.rstrip()"),
    );
    let edited = commit(&directory, "tighten the trim");
    write(
        &directory,
        "inventory.py",
        &format!(
            "import os\n{}",
            HELPER.replace("raw.strip()", "raw.rstrip()")
        ),
    );
    commit(&directory, "an import above it");
    succeeds(&directory, &["backfill"]);

    let explained = succeeds(&directory, &["explain", "inventory.py:3"]);
    for expected in [
        "trimmed = raw.rstrip()",
        &format!("commit {} \"add the helper\"", short(&born)),
        "at inventory.py:2",
        "written by a person",
        &format!("commit {}: edited", short(&edited)),
        "(within_hunk_alignment)",
        "now at inventory.py:2",
        "alive at inventory.py:3",
    ] {
        assert!(
            explained.contains(expected),
            "missing {expected:?} in:\n{explained}"
        );
    }
}

#[test]
fn explain_follows_a_line_through_changes_not_recorded_yet() {
    let directory = repo("explain-disk");
    write(&directory, "inventory.py", HELPER);
    commit(&directory, "add the helper");
    succeeds(&directory, &["backfill"]);
    write(
        &directory,
        "inventory.py",
        &format!("import os\nimport re\n{HELPER}"),
    );

    let explained = succeeds(&directory, &["explain", "inventory.py:5"]);
    assert!(
        explained.contains("lowered = trimmed.lower()"),
        "{explained}"
    );
    assert!(explained.contains("at inventory.py:3"), "{explained}");
    assert!(
        explained.contains("on disk since then: unchanged, though lines around it have moved"),
        "{explained}"
    );

    let new = marrow(&directory, &["explain", "inventory.py:1"]);
    assert_eq!(new.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&new.stderr).contains("new since marrow last saw the file"),
        "{}",
        String::from_utf8_lossy(&new.stderr)
    );
}

/// A human file, and an agent file of which a person deletes one line after it's committed.
fn mixed_history(name: &str) -> PathBuf {
    let directory = repo(name);
    write(&directory, "inventory.py", HELPER);
    commit(&directory, "a person's helper");
    succeeds(&directory, &["backfill"]);
    agent_writes(&directory, "report.py", REPORT);
    commit(&directory, "the agent's report");
    succeeds(&directory, &["reconcile"]);
    write(
        &directory,
        "report.py",
        &REPORT.replace("    print(total)\n", ""),
    );
    commit(&directory, "a person drops the print");
    succeeds(&directory, &["reconcile"]);
    directory
}

#[test]
fn report_compares_agent_and_human_lines() {
    let directory = mixed_history("report");
    let report = succeeds(&directory, &["report", "--resamples", "20"]);
    let row = |name: &str| {
        report
            .lines()
            .find(|line| line.trim_start().starts_with(name))
            .unwrap_or_else(|| panic!("no {name} row in:\n{report}"))
            .split_whitespace()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        row("agent")[1..3],
        ["4", "1"],
        "four agent lines, one deleted"
    );
    assert_eq!(
        row("human")[1..3],
        ["4", "0"],
        "four human lines, none deleted"
    );
    assert!(report.contains("Turnbull"), "{report}");
    assert!(report.contains("Death: deleted, or rewritten"), "{report}");
    assert!(
        report.contains("No curve is drawn yet"),
        "four lines each is too few to draw: {report}"
    );
    assert!(!report.contains("-0.0"), "no negative ages: {report}");
    assert!(!report.contains("Left out of the curves"), "{report}");
}

#[test]
fn status_summarises_the_latest_session() {
    let directory = mixed_history("status");
    let status = succeeds(&directory, &["status"]);
    for expected in [
        "Session session-7 (claude-opus-5)",
        "written                     4 lines",
        "still there                 3 (75%)",
        "reached a commit            4 (100%)",
        "deleted                     1 (25%)",
        "agent lines",
        "human lines",
    ] {
        assert!(
            status.contains(expected),
            "missing {expected:?} in:\n{status}"
        );
    }
}

#[test]
fn edits_after_the_first_commit_are_scored_against_the_committed_version() {
    // The agent writes one version, rewrites the line, and commits the second. A person then
    // tweaks it. That tweak must be measured against what was committed, not against the
    // version the agent first typed, or the rewritten death would count the agent's own
    // pre-commit churn against its code.
    let directory = repo("anchored");
    write(&directory, "notes.py", "def notes():\n    return []\n");
    commit(&directory, "start");
    succeeds(&directory, &["backfill"]);
    agent_writes(
        &directory,
        "total.py",
        "def total(rows):\n    return sum(rows)\n",
    );
    agent_writes(
        &directory,
        "total.py",
        "def total(rows):\n    return sum(row.amount for row in rows if row.amount)\n",
    );
    commit(&directory, "the agent's second version");
    succeeds(&directory, &["reconcile"]);
    write(
        &directory,
        "total.py",
        "def total(rows):\n    return sum(row.amount for row in rows if row.amount > 0)\n",
    );
    let tweaked = commit(&directory, "a person's tweak");
    succeeds(&directory, &["reconcile"]);

    let store = Store::open(&directory.join(".marrow").join("db.sqlite")).expect("opens");
    let fates = store.fates().expect("reads");
    let tweak = fates
        .iter()
        .find(|fate| fate.observed_commit.as_deref() == Some(tweaked.as_str()))
        .expect("the tweak was recorded");
    let original = tweak.birth_similarity.expect("scored against the original");
    assert!(
        (original - tweak.similarity_score).abs() < 1e-12,
        "the committed version is both the version before and the original: {original} vs {}",
        tweak.similarity_score
    );
}

#[test]
fn a_rewrite_threshold_outside_zero_to_one_is_a_usage_error() {
    let directory = repo("threshold");
    let output = marrow(&directory, &["report", "--rewrite-threshold", "1.5"]);
    assert_eq!(output.status.code(), Some(2));
}

/// A write through Claude Code's hook payload, naming the tool that made it.
fn hook_writes(directory: &Path, name: &str, contents: &str, tool: &str) -> String {
    use std::io::Write;
    let file = write(directory, name, contents);
    let payload = serde_json::json!({
        "session_id": "session-9",
        "tool_name": tool,
        "tool_input": {"file_path": file.to_string_lossy()},
    })
    .to_string();
    let mut child = Command::new(env!("CARGO_BIN_EXE_marrow"))
        .args(["record", "--hook"])
        .current_dir(directory)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("runs marrow record");
    child
        .stdin
        .as_mut()
        .expect("a stdin")
        .write_all(payload.as_bytes())
        .expect("writes the payload");
    let output = child.wait_with_output().expect("waits");
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn store(directory: &Path) -> Store {
    Store::open(&directory.join(".marrow").join("db.sqlite")).expect("opens the store")
}

#[test]
fn a_line_revised_later_stays_in_the_group_it_entered_with() {
    let directory = mixed_history("entered");
    // A person revises one of the agent's surviving lines, long after it was committed.
    write(
        &directory,
        "report.py",
        &REPORT
            .replace("    print(total)\n", "")
            .replace("len(rows)", "len(list(rows))"),
    );
    commit(&directory, "a person tweaks the count");
    succeeds(&directory, &["reconcile"]);
    let report = succeeds(&directory, &["report", "--resamples", "0"]);
    let agent = report
        .lines()
        .find(|line| line.trim_start().starts_with("agent "))
        .expect("an agent row");
    assert_eq!(
        agent.split_whitespace().nth(1),
        Some("4"),
        "all four lines stay the agent's: {report}"
    );
    assert!(
        !report.contains("revised by a person"),
        "nobody revised a line on its way into its first commit: {report}"
    );
    assert!(
        report.contains("so these curves compare the code that survived"),
        "{report}"
    );
}

#[test]
fn an_agent_edit_to_a_file_marrow_never_saw_credits_only_what_it_added() {
    let directory = repo("unseen-committed");
    write(&directory, "inventory.py", HELPER);
    commit(&directory, "a person's helper, before marrow was installed");
    let said = hook_writes(
        &directory,
        "inventory.py",
        &format!("{HELPER}\n\ndef shout(name):\n    return name.upper()\n"),
        "Edit",
    );
    assert!(
        said.contains("2 lines born") && said.contains("4 already in the file"),
        "{said}"
    );
    let lines = store(&directory).lines().expect("reads");
    let agent = lines
        .iter()
        .filter(|line| line.session_id.is_some())
        .count();
    let unseen = lines.iter().filter(|line| !line.birth_observed).count();
    assert_eq!((agent, unseen), (2, 4));

    // A file git doesn't have, edited by the agent before marrow ever saw it: nothing in it can
    // be shown to be the agent's.
    let directory = repo("unseen-untracked");
    write(&directory, "notes.py", "def notes():\n    return []\n");
    commit(&directory, "start");
    let said = hook_writes(
        &directory,
        "draft.py",
        "def draft():\n    return 1\n",
        "Edit",
    );
    assert!(said.contains("0 lines born"), "{said}");
}

#[test]
fn a_branch_marrow_never_walked_isnt_born_at_an_unrelated_commit() {
    let directory = repo("legacy-branch");
    write(&directory, "inventory.py", HELPER);
    commit(&directory, "main");
    git(&directory, &["checkout", "-q", "-b", "legacy"]);
    write(&directory, "legacy.py", REPORT);
    commit(&directory, "old code on a branch");
    git(&directory, &["checkout", "-q", "main"]);
    succeeds(&directory, &["backfill"]);
    git(&directory, &["checkout", "-q", "legacy"]);
    write(&directory, "fresh.py", "def fresh():\n    return 0\n");
    commit(&directory, "something new on the old branch");
    succeeds(&directory, &["reconcile"]);
    let lines = store(&directory).lines().expect("reads");
    let unseen = lines.iter().filter(|line| !line.birth_observed).count();
    let seen = lines.iter().filter(|line| line.birth_observed).count();
    assert_eq!(unseen, 4, "the branch's old file was already there");
    assert_eq!(
        seen, 6,
        "the helper, and the commit's own new file, were seen being born"
    );
}

#[test]
fn a_commit_dated_in_the_future_doesnt_hold_back_the_ones_after_it() {
    fn commit_on(directory: &Path, message: &str, date: &str) {
        git(directory, &["add", "-A"]);
        let output = Command::new("git")
            .args(["commit", "-q", "-m", message])
            .current_dir(directory)
            .env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date)
            .output()
            .expect("runs git");
        assert!(output.status.success());
    }
    let directory = repo("future");
    write(&directory, "one.py", "def one():\n    return 1\n");
    commit_on(&directory, "january", "2026-01-10T12:00:00+0000");
    write(&directory, "two.py", "def two():\n    return 2\n");
    commit_on(
        &directory,
        "a machine set to 2031",
        "2031-06-01T12:00:00+0000",
    );
    write(&directory, "three.py", "def three():\n    return 3\n");
    commit_on(&directory, "february", "2026-02-10T12:00:00+0000");
    write(&directory, "four.py", "def four():\n    return 4\n");
    commit_on(&directory, "march", "2026-03-10T12:00:00+0000");
    succeeds(&directory, &["backfill"]);
    let births: Vec<i64> = store(&directory)
        .lines()
        .expect("reads")
        .iter()
        .map(|line| line.birth_ts)
        .collect();
    let (february, march) = (1_770_724_800, 1_773_144_000);
    assert_eq!(births[6..], [march, march], "march is still march");
    assert_eq!(
        births[2..6],
        [february, february, february, february],
        "the 2031 commit waits for the next one, rather than everything waiting for it"
    );
}

#[test]
fn anchoring_through_a_rename_takes_the_committed_version() {
    let directory = repo("renamed-anchor");
    write(&directory, "a.py", "def a():\n    return 1\n");
    commit(&directory, "a person's file");
    succeeds(&directory, &["backfill"]);
    agent_writes(
        &directory,
        "a.py",
        "def a():\n    return 1\n\ndef total(rows):\n    return sum(rows)\n",
    );
    // A person renames the file and tweaks the agent's line in the same commit.
    git(&directory, &["mv", "a.py", "b.py"]);
    write(
        &directory,
        "b.py",
        "def a():\n    return 1\n\ndef total(rows):\n    return sum(rows) or 0\n",
    );
    commit(&directory, "rename and tweak");
    succeeds(&directory, &["reconcile"]);
    write(
        &directory,
        "b.py",
        "def a():\n    return 1\n\ndef total(rows):\n    return sum(rows) or 0.0\n",
    );
    let later = commit(&directory, "another tweak");
    succeeds(&directory, &["reconcile"]);
    let fates = store(&directory).fates().expect("reads");
    let tweak = fates
        .iter()
        .find(|fate| fate.observed_commit.as_deref() == Some(later.as_str()))
        .expect("the second tweak was recorded");
    assert!(
        (tweak.birth_similarity.expect("scored") - tweak.similarity_score).abs() < 1e-12,
        "the original is the version committed in the rename, not the agent's first draft"
    );
}

#[test]
fn explain_says_nothing_about_disk_when_the_line_is_just_where_it_was() {
    let directory = repo("explain-bytes");
    write(&directory, "inventory.py", HELPER);
    commit(&directory, "add the helper");
    succeeds(&directory, &["backfill"]);
    // Different bytes, the same lines: trailing spaces on another line.
    write(
        &directory,
        "inventory.py",
        &HELPER.replace("lower()\n", "lower()   \n"),
    );
    let explained = succeeds(&directory, &["explain", "inventory.py:2"]);
    assert!(!explained.contains("on disk since then"), "{explained}");
}
