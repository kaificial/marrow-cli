//! `marrow reconcile` end to end: a real git repository, real commits made by hand, and the
//! stored result.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use marrow_store::Store;

const HELPER: &str = "def normalize_name(raw):\n    trimmed = raw.strip()\n    lowered = trimmed.lower()\n    return lowered.replace(\" \", \"_\")\n";
const CALLER: &str = "def run(rows):\n    return [normalize_name(row) for row in rows]\n";

/// A scratch repository with no global configuration, hooks, or templates in play.
fn repo(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("marrow-reconcile-{name}"));
    if directory.exists() {
        remove(&directory);
    }
    std::fs::create_dir_all(&directory).expect("a scratch directory");
    git(&directory, &["init", "-q", "--template="]);
    git(&directory, &["config", "user.name", "Test Person"]);
    git(&directory, &["config", "user.email", "person@example.com"]);
    git(&directory, &["config", "commit.gpgsign", "false"]);
    directory
}

/// Windows keeps `.git` objects read-only, so a plain remove_dir_all trips over them.
fn remove(directory: &Path) {
    for entry in walkdir(directory) {
        if let Ok(metadata) = std::fs::metadata(&entry) {
            let mut permissions = metadata.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            permissions.set_readonly(false);
            let _ = std::fs::set_permissions(&entry, permissions);
        }
    }
    let _ = std::fs::remove_dir_all(directory);
}

fn walkdir(directory: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(directory) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(walkdir(&path));
        }
        found.push(path);
    }
    found
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
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// A commit dated well into the future, so any checkpoint that moves is unmistakable.
fn hand_commit_in_2030(directory: &Path, message: &str) -> (String, i64) {
    git(directory, &["add", "-A"]);
    let output = Command::new("git")
        .args(["commit", "-q", "-m", message])
        .current_dir(directory)
        .env_remove("GIT_DIR")
        .env("GIT_AUTHOR_DATE", "2030-01-01T00:00:00+0000")
        .env("GIT_COMMITTER_DATE", "2030-01-01T00:00:00+0000")
        .output()
        .expect("runs git");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    (git(directory, &["rev-parse", "HEAD"]), 1_893_456_000)
}

/// A commit made by hand, the way a person would: no agent, no hook.
fn hand_commit(directory: &Path, message: &str) -> String {
    git(directory, &["add", "-A"]);
    git(
        directory,
        &[
            "-c",
            "advice.addEmptyPathspec=false",
            "commit",
            "-q",
            "-m",
            message,
        ],
    );
    git(directory, &["rev-parse", "HEAD"])
}

fn write(directory: &Path, name: &str, contents: &str) -> PathBuf {
    let path = directory.join(name);
    std::fs::write(&path, contents).expect("writes the file");
    path
}

fn marrow(args: &[&str], directory: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_marrow"))
        .args(args)
        .current_dir(directory)
        .output()
        .expect("runs marrow")
}

fn record(directory: &Path, file: &Path) {
    let output = marrow(
        &[
            "record",
            "--session",
            "session-agent",
            "--model",
            "claude-opus-5",
            "--file",
            &file.to_string_lossy(),
        ],
        directory,
    );
    assert!(
        output.status.success(),
        "record failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn reconciled(directory: &Path) -> String {
    let output = marrow(&["reconcile"], directory);
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    assert!(output.status.success(), "reconcile failed: {stderr}");
    assert!(stderr.is_empty(), "unexpected warning: {stderr}");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

struct Line {
    origin: String,
    birth_commit: Option<String>,
    last_seen_commit: Option<String>,
    last_seen_at: i64,
}

struct Stored {
    lines: Vec<Line>,
    fates: Vec<(String, String, Option<i64>, i64)>,
    commits: Vec<(String, i64)>,
    snapshots: i64,
    snapshot_lines: i64,
}

fn stored(directory: &Path) -> Stored {
    let store = Store::open(&directory.join(".marrow").join("db.sqlite")).expect("opens the store");
    let connection = store.connection();
    let count = |sql: &str| -> i64 { connection.query_row(sql, [], |row| row.get(0)).unwrap() };
    let mut lines = connection
        .prepare("SELECT origin, birth_commit, last_seen_commit, last_seen_at FROM lines ORDER BY line_id")
        .unwrap();
    let lines = lines
        .query_map([], |row| {
            Ok(Line {
                origin: row.get(0)?,
                birth_commit: row.get(1)?,
                last_seen_commit: row.get(2)?,
                last_seen_at: row.get(3)?,
            })
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let mut fates = connection
        .prepare(
            "SELECT state, deciding_layer, previous_seen_at, observed_at FROM fates
             ORDER BY rowid",
        )
        .unwrap();
    let fates = fates
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let mut commits = connection
        .prepare("SELECT sha, is_merge FROM commits ORDER BY sha")
        .unwrap();
    let commits = commits
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    Stored {
        lines,
        fates,
        commits,
        snapshots: count("SELECT count(*) FROM snapshots"),
        snapshot_lines: count("SELECT count(*) FROM snapshot_lines"),
    }
}

#[test]
fn a_commit_made_by_hand_is_reconciled_and_labelled_human() {
    let directory = repo("by-hand");
    write(&directory, "inventory.py", HELPER);
    let sha = hand_commit(&directory, "add the helper");
    assert_eq!(
        reconciled(&directory),
        format!(
            "marrow reconcile: {} — 4 born, 0 edited, 0 moved, 0 dead, 4 alive",
            &sha[..8]
        )
    );
    let stored = stored(&directory);
    assert_eq!(stored.lines.len(), 4);
    assert!(
        stored.lines.iter().all(|line| line.origin == "human"
            && line.birth_commit.as_deref() == Some(sha.as_str())
            && line.last_seen_commit.as_deref() == Some(sha.as_str())),
        "every line is human-written and checkpointed at the commit"
    );
    assert!(stored.fates.is_empty(), "a first sighting has no fates");
    assert_eq!(stored.commits, [(sha, 0)]);
    assert_eq!((stored.snapshots, stored.snapshot_lines), (1, 4));
}

#[test]
fn marrow_installed_mid_history_reconciles_the_tree_it_finds() {
    let directory = repo("mid-history");
    write(&directory, "inventory.py", HELPER);
    hand_commit(&directory, "first");
    write(&directory, "caller.py", CALLER);
    let second = hand_commit(&directory, "second");
    reconciled(&directory);
    let stored = stored(&directory);
    assert_eq!(
        stored.lines.len(),
        6,
        "both files are picked up, even though the first commit was never seen"
    );
    assert!(stored
        .lines
        .iter()
        .all(|line| line.birth_commit.as_deref() == Some(second.as_str())));
    assert!(
        stored.fates.is_empty(),
        "with nothing to compare against, nothing changed: {:?}",
        stored.fates
    );
    assert_eq!(stored.snapshots, 2);
}

#[test]
fn a_captured_line_keeps_its_agent_origin_and_gains_a_birth_commit() {
    let directory = repo("agent-origin");
    let file = write(&directory, "inventory.py", HELPER);
    record(&directory, &file);
    let sha = hand_commit(&directory, "commit the agent's work");
    let report = reconciled(&directory);
    assert!(report.contains("0 born"), "{report}");
    assert!(report.contains("4 alive"), "{report}");
    let stored = stored(&directory);
    assert_eq!(stored.lines.len(), 4, "no line was born twice");
    assert!(stored.lines.iter().all(|line| line.origin == "agent"));
    assert!(
        stored
            .lines
            .iter()
            .all(|line| line.birth_commit.as_deref() == Some(sha.as_str())),
        "the commit the hook couldn't know about is attached now"
    );
    assert!(
        stored.fates.is_empty(),
        "nothing changed: {:?}",
        stored.fates
    );
}

#[test]
fn an_agent_line_a_person_edits_becomes_the_third_origin() {
    let directory = repo("revised");
    let file = write(&directory, "inventory.py", HELPER);
    record(&directory, &file);
    write(
        &directory,
        "inventory.py",
        &HELPER.replace("raw.strip()", "raw.rstrip()"),
    );
    hand_commit(&directory, "tighten the trim by hand");
    reconciled(&directory);
    let stored = stored(&directory);
    let origins: Vec<&str> = stored
        .lines
        .iter()
        .map(|line| line.origin.as_str())
        .collect();
    assert_eq!(
        origins
            .iter()
            .filter(|origin| **origin == "agent_then_human_revised")
            .count(),
        1,
        "{origins:?}"
    );
    assert_eq!(stored.fates.len(), 1, "{:?}", stored.fates);
    let (state, layer, previous_seen_at, observed_at) = stored.fates[0].clone();
    assert_eq!(
        (state.as_str(), layer.as_str()),
        ("edited", "within_hunk_alignment")
    );
    assert!(
        previous_seen_at.is_some_and(|previous| previous <= observed_at),
        "the change is placed in an interval, not a moment"
    );
}

#[test]
fn a_file_deleted_by_hand_has_its_lines_recorded_dead() {
    let directory = repo("deleted");
    let file = write(&directory, "inventory.py", HELPER);
    record(&directory, &file);
    write(&directory, "caller.py", CALLER);
    hand_commit(&directory, "both files");
    reconciled(&directory);
    std::fs::remove_file(&file).expect("removes the file");
    hand_commit(&directory, "drop the helper");
    reconciled(&directory);
    let stored = stored(&directory);
    let dead: Vec<_> = stored
        .fates
        .iter()
        .filter(|fate| fate.0 == "dead")
        .collect();
    assert_eq!(dead.len(), 4, "{:?}", stored.fates);
    assert!(dead.iter().all(|fate| fate.1 == "no_match"));
    assert_eq!(
        stored.snapshots, 1,
        "the deleted file's last known state is forgotten"
    );
    assert_eq!(stored.commits.len(), 2);
}

#[test]
fn a_file_renamed_by_hand_keeps_its_lines() {
    let directory = repo("renamed");
    let file = write(&directory, "inventory.py", HELPER);
    record(&directory, &file);
    hand_commit(&directory, "add the helper");
    reconciled(&directory);
    git(&directory, &["mv", "inventory.py", "names.py"]);
    let sha = hand_commit(&directory, "rename it");
    reconciled(&directory);
    let stored = stored(&directory);
    assert_eq!(stored.lines.len(), 4, "nothing was born or died");
    assert!(
        stored.fates.is_empty(),
        "a rename with no edits leaves every line verbatim: {:?}",
        stored.fates
    );
    assert!(stored
        .lines
        .iter()
        .all(|line| line.last_seen_commit.as_deref() == Some(sha.as_str())));
    assert_eq!(stored.snapshots, 1, "the state moved to the new path");
}

#[test]
fn a_file_written_but_never_committed_keeps_its_lines_alive() {
    let directory = repo("uncommitted");
    let scratch = write(&directory, "scratch.py", HELPER);
    record(&directory, &scratch);
    write(&directory, ".gitignore", "scratch.py\n");
    hand_commit(&directory, "ignore the scratch file");
    reconciled(&directory);
    let stored = stored(&directory);
    assert!(
        stored.fates.is_empty(),
        "an uncommitted file isn't a deleted one: {:?}",
        stored.fates
    );
    assert_eq!(stored.lines.len(), 4);
    assert!(
        stored.lines.iter().all(|line| line.birth_commit.is_none()),
        "a line that has never been committed has no birth commit"
    );
    assert_eq!(stored.snapshots, 1, "its last known state is kept");
}

#[test]
fn reconciling_the_same_commit_again_changes_nothing() {
    let directory = repo("idempotent");
    let file = write(&directory, "inventory.py", HELPER);
    record(&directory, &file);
    write(
        &directory,
        "inventory.py",
        &HELPER.replace("    lowered = trimmed.lower()\n", ""),
    );
    hand_commit(&directory, "drop a line by hand");
    let first = reconciled(&directory);
    let before = stored(&directory);
    let again = reconciled(&directory);
    let after = stored(&directory);
    assert!(first.contains("1 dead"), "{first}");
    assert!(
        again.contains("0 dead"),
        "the second run has nothing left to report: {again}"
    );
    assert_eq!(
        before.lines.len(),
        after.lines.len(),
        "no line is born twice"
    );
    assert_eq!(
        before.fates.len(),
        after.fates.len(),
        "no fate is recorded twice"
    );
    assert_eq!(before.snapshot_lines, after.snapshot_lines);
    assert!(
        after.lines.iter().all(|line| line.last_seen_at > 0),
        "every surviving line has a checkpoint"
    );
}

#[test]
fn a_branch_switch_does_not_kill_the_lines_it_hides() {
    let directory = repo("branches");
    write(&directory, "base.py", HELPER);
    hand_commit(&directory, "base");
    reconciled(&directory);
    git(&directory, &["checkout", "-q", "-b", "feature"]);
    write(&directory, "feature.py", CALLER);
    hand_commit(&directory, "a feature on a branch");
    reconciled(&directory);

    // Checking out main takes feature.py away without anything dying.
    git(&directory, &["checkout", "-q", "-"]);
    write(&directory, "notes.py", "def notes():\n    return []\n");
    hand_commit(&directory, "something else on the first branch");
    reconciled(&directory);
    let switched = stored(&directory);
    assert!(
        switched.fates.is_empty(),
        "a branch switch isn't a deletion: {:?}",
        switched.fates
    );
    assert_eq!(switched.lines.len(), 8);

    git(
        &directory,
        &["merge", "--no-ff", "-q", "-m", "merge", "feature"],
    );
    reconciled(&directory);
    let merged = stored(&directory);
    assert_eq!(
        merged.lines.len(),
        8,
        "the merged-in lines are the ones we already had, not new ones"
    );
    assert!(merged.fates.is_empty(), "{:?}", merged.fates);
    assert!(
        merged.commits.iter().any(|(_, is_merge)| *is_merge == 1),
        "the merge is recorded as one: {:?}",
        merged.commits
    );
}

#[test]
fn an_edit_left_out_of_a_commit_keeps_the_working_tree_as_the_truth() {
    let directory = repo("partial");
    let file = write(&directory, "inventory.py", HELPER);
    record(&directory, &file);
    hand_commit(&directory, "the agent's helper");
    reconciled(&directory);

    // The agent edits the file again, and a person commits something else without staging it.
    write(
        &directory,
        "inventory.py",
        &HELPER.replace("raw.strip()", "raw.rstrip()"),
    );
    record(&directory, &file);
    write(&directory, "caller.py", CALLER);
    git(&directory, &["add", "caller.py"]);
    git(&directory, &["commit", "-q", "-m", "only the caller"]);
    reconciled(&directory);

    let stored = stored(&directory);
    assert_eq!(
        stored.fates.len(),
        1,
        "only the hook's edit, not a second one from the commit: {:?}",
        stored.fates
    );
    assert_eq!(stored.fates[0].0, "edited");
    assert!(
        stored.lines.iter().all(|line| line.last_seen_at > 0),
        "the uncommitted lines are still alive"
    );
    assert_eq!(
        stored.lines.len(),
        6,
        "the agent's lines are not born again as a person's: {:?}",
        stored
            .lines
            .iter()
            .map(|line| line.origin.as_str())
            .collect::<Vec<_>>()
    );
    assert_eq!(stored.snapshot_lines, 6, "both files are still tracked");
}

#[test]
fn work_that_vanished_before_any_commit_stops_being_confirmed_alive() {
    let directory = repo("vanished");
    let scratch = write(&directory, "scratch.py", HELPER);
    record(&directory, &scratch);
    write(&directory, ".gitignore", "scratch.py\n");
    hand_commit(&directory, "ignore the scratch file");
    reconciled(&directory);
    let before = stored(&directory);
    let checkpoints: Vec<i64> = before.lines.iter().map(|line| line.last_seen_at).collect();

    // The file goes away without git ever having it. Nothing can be said about those lines.
    std::fs::remove_file(&scratch).expect("removes the file");
    write(&directory, "caller.py", CALLER);
    let (_, in_2030) = hand_commit_in_2030(&directory, "something else entirely");
    reconciled(&directory);

    let after = stored(&directory);
    assert_eq!(
        after.lines[..4]
            .iter()
            .map(|line| line.last_seen_at)
            .collect::<Vec<_>>(),
        checkpoints[..4],
        "lines nobody can find are not confirmed alive again"
    );
    assert!(
        after.fates.is_empty(),
        "and they aren't called dead either, on no evidence: {:?}",
        after.fates
    );
    assert!(
        after.lines[4..]
            .iter()
            .all(|line| line.last_seen_at == in_2030),
        "while the committed lines do get the commit's own date"
    );
}

#[test]
fn a_repository_that_rewrites_line_endings_is_still_reconciled() {
    let directory = repo("line-endings");
    git(&directory, &["config", "core.autocrlf", "true"]);
    let file = write(&directory, "inventory.py", &HELPER.replace('\n', "\r\n"));
    record(&directory, &file);
    // git stores the blob with the carriage returns stripped, so the bytes marrow captured can
    // never match the committed bytes. The comparison has to work on tokens instead.
    let sha = hand_commit(&directory, "the agent's helper, with windows line endings");
    reconciled(&directory);
    let stored = stored(&directory);
    assert_eq!(stored.lines.len(), 4, "nothing was born a second time");
    assert!(
        stored.lines.iter().all(
            |line| line.origin == "agent" && line.birth_commit.as_deref() == Some(sha.as_str())
        ),
        "the agent keeps the credit, and the birth commit is attached"
    );
    assert!(
        stored.fates.is_empty(),
        "line endings are not an edit: {:?}",
        stored.fates
    );
}

#[test]
fn install_writes_the_hook_and_leaves_an_existing_one_alone() {
    let directory = repo("install");
    let output = marrow(&["reconcile", "--install"], &directory);
    assert!(output.status.success());
    let hook = directory.join(".git").join("hooks").join("post-commit");
    let script = std::fs::read_to_string(&hook).expect("the hook is written");
    assert!(script.contains("reconcile --repo"), "{script}");
    assert!(script.starts_with("#!/bin/sh"), "{script}");

    let again = marrow(&["reconcile", "--install"], &directory);
    assert!(String::from_utf8_lossy(&again.stdout).contains("already runs marrow"));

    std::fs::write(&hook, "#!/bin/sh\necho mine\n").unwrap();
    let refused = marrow(&["reconcile", "--install"], &directory);
    assert_eq!(refused.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(stderr.contains("already exists"), "{stderr}");
    assert_eq!(
        std::fs::read_to_string(&hook).unwrap(),
        "#!/bin/sh\necho mine\n",
        "someone else's hook is left as it is"
    );
}

#[test]
fn the_installed_hook_runs_on_a_real_commit() {
    let directory = repo("hooked");
    assert!(marrow(&["reconcile", "--install"], &directory)
        .status
        .success());
    write(&directory, "inventory.py", HELPER);
    let sha = hand_commit(&directory, "a commit that reconciles itself");
    let stored = stored(&directory);
    assert_eq!(stored.lines.len(), 4, "the hook ran without being asked");
    assert!(stored
        .lines
        .iter()
        .all(|line| line.origin == "human" && line.birth_commit.as_deref() == Some(sha.as_str())));
}

#[test]
fn a_repository_with_no_commits_is_skipped() {
    let directory = repo("unborn");
    let output = marrow(&["reconcile"], &directory);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("no commits yet"));
}
