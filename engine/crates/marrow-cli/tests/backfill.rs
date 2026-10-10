//! `marrow backfill` end to end: real git histories, walked from the repository's own folder.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use marrow_store::Store;

const HELPER: &str = "def normalize_name(raw):\n    trimmed = raw.strip()\n    lowered = trimmed.lower()\n    return lowered.replace(\" \", \"_\")\n";
const CALLER: &str = "def run(rows):\n    return [normalize_name(row) for row in rows]\n";
const REPORT: &str =
    "def report(rows):\n    total = len(rows)\n    print(total)\n    return total\n";

fn repo(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("marrow-backfill-{name}"));
    if directory.exists() {
        remove(&directory);
    }
    std::fs::create_dir_all(&directory).expect("a scratch directory");
    git(&directory, &["init", "-q", "--template=", "-b", "main"]);
    configure(&directory);
    directory
}

fn configure(directory: &Path) {
    git(directory, &["config", "user.name", "Test Person"]);
    git(directory, &["config", "user.email", "person@example.com"]);
    git(directory, &["config", "commit.gpgsign", "false"]);
}

/// Windows keeps `.git` objects read-only, so a plain remove_dir_all trips over them.
fn remove(directory: &Path) {
    for entry in walk(directory) {
        if let Ok(metadata) = std::fs::metadata(&entry) {
            let mut permissions = metadata.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            permissions.set_readonly(false);
            let _ = std::fs::set_permissions(&entry, permissions);
        }
    }
    let _ = std::fs::remove_dir_all(directory);
}

fn walk(directory: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(directory) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(walk(&path));
        }
        found.push(path);
    }
    found
}

fn git_with(directory: &Path, args: &[&str], date: Option<&str>) -> String {
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(directory)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE");
    if let Some(date) = date {
        command
            .env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date);
    }
    let output = command.output().expect("runs git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn git(directory: &Path, args: &[&str]) -> String {
    git_with(directory, args, None)
}

fn commit(directory: &Path, message: &str) -> String {
    commit_on(directory, message, None)
}

fn commit_on(directory: &Path, message: &str, date: Option<&str>) -> String {
    git(directory, &["add", "-A"]);
    git_with(directory, &["commit", "-q", "-m", message], date);
    git(directory, &["rev-parse", "HEAD"])
}

fn write(directory: &Path, name: &str, contents: &str) -> PathBuf {
    let path = directory.join(name);
    std::fs::write(&path, contents).expect("writes the file");
    path
}

fn marrow(directory: &Path, args: &[&str]) -> Output {
    marrow_with(directory, args, None)
}

fn marrow_with(directory: &Path, args: &[&str], crash_at: Option<usize>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_marrow"));
    command.args(args).current_dir(directory);
    match crash_at {
        Some(index) => command.env("MARROW_TEST_CRASH_BACKFILL_AT", index.to_string()),
        None => command.env_remove("MARROW_TEST_CRASH_BACKFILL_AT"),
    };
    command.output().expect("runs marrow")
}

fn backfilled(directory: &Path, args: &[&str]) -> String {
    let mut all = vec!["backfill"];
    all.extend_from_slice(args);
    let output = marrow(directory, &all);
    assert!(
        output.status.success(),
        "backfill failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn query<T: rusqlite_value::FromRow>(directory: &Path, sql: &str) -> Vec<T> {
    let store = Store::open(&directory.join(".marrow").join("db.sqlite")).expect("opens the store");
    let mut statement = store.connection().prepare(sql).expect("valid SQL");
    statement
        .query_map([], |row| T::from_row(row))
        .expect("runs")
        .collect::<Result<Vec<_>, _>>()
        .expect("reads")
}

/// Rows read into plain tuples, without the tests depending on rusqlite by name.
mod rusqlite_value {
    pub trait FromRow: Sized {
        fn from_row(row: &marrow_store::Row<'_>) -> marrow_store::SqlResult<Self>;
    }

    impl FromRow for i64 {
        fn from_row(row: &marrow_store::Row<'_>) -> marrow_store::SqlResult<Self> {
            row.get(0)
        }
    }

    impl FromRow for String {
        fn from_row(row: &marrow_store::Row<'_>) -> marrow_store::SqlResult<Self> {
            row.get(0)
        }
    }

    impl FromRow for Option<String> {
        fn from_row(row: &marrow_store::Row<'_>) -> marrow_store::SqlResult<Self> {
            row.get(0)
        }
    }

    impl FromRow for (String, i64) {
        fn from_row(row: &marrow_store::Row<'_>) -> marrow_store::SqlResult<Self> {
            Ok((row.get(0)?, row.get(1)?))
        }
    }

    impl FromRow for (String, String) {
        fn from_row(row: &marrow_store::Row<'_>) -> marrow_store::SqlResult<Self> {
            Ok((row.get(0)?, row.get(1)?))
        }
    }

    impl FromRow for (String, i64, i64) {
        fn from_row(row: &marrow_store::Row<'_>) -> marrow_store::SqlResult<Self> {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        }
    }
}

/// Every row of every table, as text, so two stores can be compared exactly.
fn dump(directory: &Path) -> Vec<String> {
    let mut rows = Vec::new();
    for table in [
        "meta",
        "sessions",
        "commits",
        "lines",
        "fates",
        "snapshots",
        "snapshot_lines",
    ] {
        let columns: Vec<String> = query(
            directory,
            &format!("SELECT name FROM pragma_table_info('{table}')"),
        );
        let row = columns
            .iter()
            .map(|column| format!("quote({column})"))
            .collect::<Vec<_>>()
            .join(" || '|' || ");
        let mut table_rows: Vec<String> = query(
            directory,
            &format!("SELECT '{table}|' || {row} FROM {table} ORDER BY rowid"),
        );
        rows.append(&mut table_rows);
    }
    rows
}

fn count(directory: &Path, sql: &str) -> i64 {
    query::<i64>(directory, sql)[0]
}

#[test]
fn every_first_parent_commit_is_recorded_and_every_line_is_human() {
    let directory = repo("walk");
    write(&directory, "inventory.py", HELPER);
    let first = commit(&directory, "add the helper");
    write(
        &directory,
        "inventory.py",
        &HELPER.replace("raw.strip()", "raw.rstrip()"),
    );
    let second = commit(&directory, "tighten the trim");
    write(
        &directory,
        "inventory.py",
        &HELPER
            .replace("raw.strip()", "raw.rstrip()")
            .replace("    lowered = trimmed.lower()\n", ""),
    );
    write(&directory, "report.py", REPORT);
    let third = commit(&directory, "drop a line and add a report");

    let output = backfilled(&directory, &[]);
    assert!(
        output.starts_with("marrow backfill: recorded 3 commits through"),
        "{output}"
    );
    assert_eq!(
        query::<(String, i64)>(
            &directory,
"SELECT l.birth_commit, count(*) FROM lines l JOIN commits c ON c.sha = l.birth_commit GROUP BY l.birth_commit ORDER BY c.rowid"
        )
        .into_iter()
        .map(|(sha, lines)| (sha == first, sha == third, lines))
        .collect::<Vec<_>>(),
        [(true, false, 4), (false, true, 4)],
        "four lines born with the helper, four with the report, none with the edit"
    );
    assert_eq!(
        query::<(String, String)>(
            &directory,
            "SELECT observed_commit, state FROM fates ORDER BY rowid"
        ),
        [
            (second.clone(), "edited".to_owned()),
            (third.clone(), "dead".to_owned())
        ]
    );
    assert_eq!(
        query::<String>(&directory, "SELECT DISTINCT origin FROM lines"),
        ["human"]
    );
    assert_eq!(
        count(
            &directory,
            "SELECT count(*) FROM lines WHERE birth_observed = 0"
        ),
        0,
        "a walk from the root sees every birth"
    );
    assert_eq!(
        query::<Option<String>>(
            &directory,
            "SELECT DISTINCT last_seen_commit FROM lines
             WHERE line_id IN (SELECT line_id FROM snapshot_lines)"
        ),
        [Some(third.clone())],
        "every line still alive is checkpointed at the last commit"
    );
    assert_eq!(
        query::<Option<String>>(
            &directory,
            "SELECT l.last_seen_commit FROM lines l JOIN fates f USING (line_id)
             WHERE f.state = 'dead'"
        ),
        [Some(second)],
        "a dead line was last seen at the commit before it died"
    );
    assert_eq!(
        query::<String>(&directory, "SELECT DISTINCT observed_by FROM commits"),
        ["backfill"]
    );
}

#[test]
fn a_merged_branch_arrives_at_the_merge_commit() {
    let directory = repo("merge");
    write(&directory, "inventory.py", HELPER);
    commit(&directory, "base");
    git(&directory, &["checkout", "-q", "-b", "feature"]);
    write(&directory, "caller.py", CALLER);
    let branch_commit = commit(&directory, "a feature, written on a branch");
    git(&directory, &["checkout", "-q", "main"]);
    write(&directory, "report.py", REPORT);
    commit(&directory, "something else on main");
    git(
        &directory,
        &[
            "merge",
            "--no-ff",
            "-q",
            "-m",
            "merge the feature",
            "feature",
        ],
    );
    let merge = git(&directory, &["rev-parse", "HEAD"]);

    backfilled(&directory, &[]);
    assert_eq!(
        count(&directory, "SELECT count(*) FROM commits"),
        3,
        "only the first-parent chain is walked"
    );
    assert_eq!(
        count(
            &directory,
            &format!("SELECT count(*) FROM commits WHERE sha = '{branch_commit}'")
        ),
        0,
        "the branch's own commits are never visited"
    );
    assert_eq!(
        query::<(String, i64, i64)>(
            &directory,
            "SELECT c.sha, c.is_merge, count(l.line_id) FROM commits c
             LEFT JOIN lines l ON l.birth_commit = c.sha
             WHERE c.is_merge = 1 GROUP BY c.sha"
        ),
        [(merge, 1, 2)],
        "the branch's two lines are born at the merge, and the merge is flagged"
    );
}

#[test]
fn a_claude_trailer_is_stored_beside_the_origin_and_never_changes_it() {
    let directory = repo("trailer");
    write(&directory, "inventory.py", HELPER);
    commit(&directory, "written by hand");
    write(&directory, "report.py", REPORT);
    git(&directory, &["add", "-A"]);
    git(
        &directory,
        &[
            "commit",
            "-q",
            "-m",
            "add a report",
            "-m",
            "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>",
        ],
    );
    let output = backfilled(&directory, &[]);
    assert!(
        output.contains("1 commit carries Claude Code's co-author trailer"),
        "{output}"
    );
    assert_eq!(
        query::<Option<String>>(
            &directory,
            "SELECT agent_trailer FROM commits ORDER BY committed_at, rowid"
        ),
        [
            None,
            Some("Claude Opus 5 <noreply@anthropic.com>".to_owned())
        ]
    );
    assert_eq!(
        query::<String>(&directory, "SELECT DISTINCT origin FROM lines"),
        ["human"],
        "the trailer is a flag, not a relabel"
    );
    assert_eq!(
        count(
            &directory,
            "SELECT count(*) FROM lines l JOIN commits c ON c.sha = l.birth_commit
             WHERE c.agent_trailer IS NOT NULL"
        ),
        4,
        "but the report's lines can be found through it"
    );
}

#[test]
fn since_a_count_starts_from_lines_that_were_already_there() {
    let directory = repo("since-count");
    write(&directory, "inventory.py", HELPER);
    let first = commit(&directory, "one");
    write(&directory, "caller.py", CALLER);
    let baseline = commit(&directory, "two");
    write(&directory, "report.py", REPORT);
    commit(&directory, "three");
    write(
        &directory,
        "report.py",
        &REPORT.replace("print(total)", "print(total, rows)"),
    );
    commit(&directory, "four");

    let output = backfilled(&directory, &["--since", "2"]);
    assert!(
        output.starts_with("marrow backfill: recorded 3 commits"),
        "the last two, and the commit they start from: {output}"
    );
    assert_eq!(
        count(
            &directory,
            &format!("SELECT count(*) FROM commits WHERE sha = '{first}'")
        ),
        0
    );
    assert_eq!(
        query::<(String, i64)>(
            &directory,
            "SELECT birth_commit, birth_observed FROM lines GROUP BY 1, 2 ORDER BY 2"
        )
        .into_iter()
        .map(|(sha, observed)| (sha == baseline, observed))
        .collect::<Vec<_>>(),
        [(true, 0), (false, 1)],
        "lines already there at the baseline were born at a time nobody saw; the report's were seen"
    );
    assert_eq!(
        count(
            &directory,
            "SELECT count(*) FROM lines WHERE birth_observed = 0"
        ),
        6
    );
}

#[test]
fn since_a_date_starts_at_the_first_commit_made_on_or_after_it() {
    let directory = repo("since-date");
    // December is the root. Without it January would be the root, and a root's lines really
    // were born in it, so it would never be a baseline.
    write(&directory, "notes.py", "def notes():\n    return []\n");
    commit_on(&directory, "december", Some("2025-12-10T12:00:00+0000"));
    write(&directory, "inventory.py", HELPER);
    commit_on(&directory, "january", Some("2026-01-10T12:00:00+0000"));
    write(&directory, "caller.py", CALLER);
    let february = commit_on(&directory, "february", Some("2026-02-10T12:00:00+0000"));
    write(&directory, "report.py", REPORT);
    commit_on(&directory, "march", Some("2026-03-10T12:00:00+0000"));

    let output = backfilled(&directory, &["--since", "2026-02-01"]);
    assert!(
        output.starts_with("marrow backfill: recorded 3 commits"),
        "january as the baseline, then february and march: {output}"
    );
    assert_eq!(
        query::<(String, i64)>(
            &directory,
            "SELECT birth_commit, birth_observed FROM lines GROUP BY 1, 2 ORDER BY birth_ts"
        )
        .iter()
        .map(|(sha, observed)| (sha == &february, *observed))
        .collect::<Vec<_>>(),
        [(false, 0), (true, 1), (false, 1)]
    );
    let nothing = backfilled(
        &repo_with_one_commit("since-future"),
        &["--since", "2099-01-01"],
    );
    assert_eq!(nothing, "marrow backfill: no commits since 2099-01-01");
}

fn repo_with_one_commit(name: &str) -> PathBuf {
    let directory = repo(name);
    write(&directory, "inventory.py", HELPER);
    commit(&directory, "one");
    directory
}

/// A small history with some of everything: births, edits, a death, and a file moved away.
fn busy_history(name: &str) -> PathBuf {
    let directory = repo(name);
    write(&directory, "inventory.py", HELPER);
    commit(&directory, "one");
    write(&directory, "caller.py", CALLER);
    commit(&directory, "two");
    write(
        &directory,
        "inventory.py",
        &HELPER.replace("raw.strip()", "raw.rstrip()"),
    );
    commit(&directory, "three");
    write(&directory, "report.py", REPORT);
    commit(&directory, "four");
    git(&directory, &["mv", "caller.py", "entry.py"]);
    commit(&directory, "five");
    write(
        &directory,
        "report.py",
        &REPORT.replace("    print(total)\n", ""),
    );
    commit(&directory, "six");
    directory
}

#[test]
fn an_interrupted_backfill_resumes_to_exactly_the_same_result() {
    if !cfg!(debug_assertions) {
        return; // The crash switch only exists in debug builds.
    }
    let interrupted = busy_history("interrupted");
    let uninterrupted = std::env::temp_dir().join("marrow-backfill-uninterrupted");
    if uninterrupted.exists() {
        remove(&uninterrupted);
    }
    git(
        &std::env::temp_dir(),
        &[
            "clone",
            "-q",
            &interrupted.to_string_lossy(),
            &uninterrupted.to_string_lossy(),
        ],
    );

    // Killed partway through recording the fourth commit.
    let killed = marrow_with(&interrupted, &["backfill"], Some(3));
    assert_eq!(killed.status.code(), Some(101));
    assert_eq!(
        count(&interrupted, "SELECT count(*) FROM commits"),
        3,
        "the commit it was killed in the middle of left nothing behind"
    );

    // Capture waits rather than writing into a half-finished walk.
    let file = interrupted.join("report.py");
    let waited = marrow(
        &interrupted,
        &[
            "record",
            "--session",
            "session-1",
            "--file",
            &file.to_string_lossy(),
        ],
    );
    assert!(waited.status.success());
    assert!(
        String::from_utf8_lossy(&waited.stderr).contains("hasn't finished"),
        "{}",
        String::from_utf8_lossy(&waited.stderr)
    );

    let resumed = backfilled(&interrupted, &[]);
    assert!(
        resumed.starts_with("marrow backfill: resumed and recorded 3 commits"),
        "{resumed}"
    );
    backfilled(&uninterrupted, &[]);
    assert_eq!(
        dump(&interrupted),
        dump(&uninterrupted),
        "stopping and resuming changes nothing, down to the line ids"
    );
    assert_eq!(
        backfilled(&interrupted, &[])
            .lines()
            .next()
            .unwrap_or_default(),
        format!(
            "marrow backfill: already recorded through {}",
            &git(&interrupted, &["rev-parse", "HEAD"])[..8]
        )
    );
}

#[test]
fn a_backfill_carries_on_to_commits_made_after_it() {
    let directory = busy_history("extended");
    backfilled(&directory, &[]);
    let before = count(&directory, "SELECT count(*) FROM lines");
    write(
        &directory,
        "caller2.py",
        CALLER.replace("run", "run_all").as_str(),
    );
    commit(&directory, "seven");
    let output = backfilled(&directory, &[]);
    assert!(
        output.starts_with("marrow backfill: resumed and recorded 1 commit through"),
        "{output}"
    );
    assert_eq!(
        count(&directory, "SELECT count(*) FROM lines"),
        before + 2,
        "only the new commit's lines are added"
    );
}

#[test]
fn a_rewritten_history_is_refused_rather_than_patched() {
    if !cfg!(debug_assertions) {
        return;
    }
    let directory = busy_history("rewritten");
    let killed = marrow_with(&directory, &["backfill"], Some(2));
    assert_eq!(killed.status.code(), Some(101));
    let first = git(&directory, &["rev-list", "--max-parents=0", "HEAD"]);
    git(&directory, &["reset", "-q", "--hard", &first]);
    write(&directory, "other.py", CALLER);
    commit(&directory, "a different second commit");
    let refused = marrow(&directory, &["backfill"]);
    assert_eq!(refused.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("no longer in this branch's"));
}

#[test]
fn resuming_needs_the_same_window_it_started_with() {
    if !cfg!(debug_assertions) {
        return;
    }
    let directory = busy_history("window");
    let killed = marrow_with(&directory, &["backfill", "--since", "3"], Some(1));
    assert_eq!(killed.status.code(), Some(101));
    let refused = marrow(&directory, &["backfill", "--since", "4"]);
    assert_eq!(refused.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("with --since 3"));
    let resumed = backfilled(&directory, &["--since", "3"]);
    assert!(resumed.contains("resumed"), "{resumed}");
}

#[test]
fn a_backfill_has_to_come_before_capture() {
    let directory = repo_with_one_commit("after-capture");
    let file = write(&directory, "caller.py", CALLER);
    let recorded = marrow(
        &directory,
        &[
            "record",
            "--session",
            "session-1",
            "--file",
            &file.to_string_lossy(),
        ],
    );
    assert!(recorded.status.success());
    let refused = marrow(&directory, &["backfill"]);
    assert_eq!(refused.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("already holds live capture"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
}

#[test]
fn capture_that_only_edits_still_counts_as_capture() {
    // An agent write that changes a line adds no line of its own, and throwing the change away
    // afterwards leaves no file to notice. The store still holds that write, so a backfill run
    // afterwards would resume from a state no commit ever had.
    let directory = repo_with_one_commit("edit-only-capture");
    backfilled(&directory, &[]);
    let file = write(
        &directory,
        "inventory.py",
        &HELPER.replace("raw.strip()", "raw.rstrip()"),
    );
    let recorded = marrow(
        &directory,
        &[
            "record",
            "--session",
            "session-1",
            "--file",
            &file.to_string_lossy(),
        ],
    );
    assert!(
        String::from_utf8_lossy(&recorded.stdout).contains("0 lines born, 1 line changed"),
        "{}",
        String::from_utf8_lossy(&recorded.stdout)
    );
    write(&directory, "inventory.py", HELPER);
    write(&directory, "report.py", REPORT);
    commit(
        &directory,
        "a commit after the agent's write was thrown away",
    );
    let refused = marrow(&directory, &["backfill"]);
    assert_eq!(refused.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("already holds live capture"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
}

#[test]
fn a_window_left_by_a_run_killed_before_its_first_commit_is_forgotten() {
    if !cfg!(debug_assertions) {
        return;
    }
    let directory = busy_history("stale-window");
    let killed = marrow_with(&directory, &["backfill", "--since", "3"], Some(0));
    assert_eq!(killed.status.code(), Some(101));
    let output = backfilled(&directory, &[]);
    assert!(
        output.starts_with("marrow backfill: recorded 6 commits"),
        "nothing had been recorded, so this is a fresh walk of everything: {output}"
    );
    assert_eq!(
        count(
            &directory,
            "SELECT count(*) FROM meta WHERE key = 'backfill.since'"
        ),
        0,
        "and the old window doesn't linger to confuse a later resume"
    );
}

#[test]
fn a_shallow_clone_starts_from_lines_that_were_already_there() {
    let full = busy_history("shallow-source");
    let shallow = std::env::temp_dir().join("marrow-backfill-shallow");
    if shallow.exists() {
        remove(&shallow);
    }
    let source = full.to_string_lossy().replace('\\', "/");
    let url = if source.starts_with('/') {
        format!("file://{source}")
    } else {
        format!("file:///{source}")
    };
    git(
        &std::env::temp_dir(),
        &[
            "clone",
            "-q",
            "--depth",
            "2",
            &url,
            &shallow.to_string_lossy(),
        ],
    );
    let output = backfilled(&shallow, &[]);
    assert!(
        output.starts_with("marrow backfill: recorded 2 commits"),
        "{output}"
    );
    let oldest = git(&shallow, &["rev-list", "--max-parents=0", "HEAD"]);
    assert_eq!(
        query::<(String, i64)>(
            &shallow,
            "SELECT birth_commit, birth_observed FROM lines GROUP BY 1, 2 ORDER BY 2"
        )
        .into_iter()
        .map(|(sha, observed)| (sha == oldest, observed))
        .collect::<Vec<_>>(),
        [(true, 0)],
        "the clone's oldest commit isn't the repository's first, so its lines' births weren't seen"
    );
}

#[test]
fn the_walk_s_clock_never_runs_backwards() {
    let directory = repo("clock");
    write(&directory, "inventory.py", HELPER);
    commit_on(&directory, "first", Some("2026-01-10T12:00:00+0000"));
    write(
        &directory,
        "inventory.py",
        &HELPER.replace("raw.strip()", "raw.rstrip()"),
    );
    write(&directory, "report.py", REPORT);
    // Committed on a machine whose clock was five days slow.
    commit_on(&directory, "second", Some("2026-01-05T12:00:00+0000"));
    write(
        &directory,
        "report.py",
        &REPORT.replace("    print(total)\n", ""),
    );
    commit_on(&directory, "third", Some("2026-01-20T12:00:00+0000"));
    backfilled(&directory, &[]);
    assert_eq!(
        count(
            &directory,
            "SELECT count(*) FROM fates f JOIN lines l USING (line_id)
             WHERE f.observed_at < l.birth_ts OR f.previous_seen_at > f.observed_at"
        ),
        0,
        "no change is dated before the line it happened to, or before the sighting it follows"
    );
    assert_eq!(
        count(
            &directory,
            "SELECT count(*) FROM lines WHERE last_seen_at < birth_ts"
        ),
        0
    );
    assert_eq!(
        query::<i64>(
            &directory,
            "SELECT committed_at FROM commits ORDER BY rowid"
        )[1],
        1_767_614_400,
        "the commit itself keeps the date it really has"
    );
}

#[test]
fn reconciliation_carries_on_from_a_finished_backfill() {
    let directory = busy_history("handover");
    backfilled(&directory, &[]);
    let lines = count(&directory, "SELECT count(*) FROM lines");
    write(
        &directory,
        "inventory.py",
        &HELPER
            .replace("raw.strip()", "raw.rstrip()")
            .replace("lowered.replace", "lowered.strip().replace"),
    );
    commit(&directory, "a hand edit after the backfill");
    let reconciled = marrow(&directory, &["reconcile"]);
    assert!(
        reconciled.status.success(),
        "{}",
        String::from_utf8_lossy(&reconciled.stderr)
    );
    let report = String::from_utf8_lossy(&reconciled.stdout);
    assert!(report.contains("0 born, 1 edited"), "{report}");
    assert_eq!(
        count(&directory, "SELECT count(*) FROM lines"),
        lines,
        "nothing the backfill recorded is born a second time"
    );
}

#[test]
fn a_since_that_is_neither_a_count_nor_a_date_is_a_usage_error() {
    let directory = repo_with_one_commit("bad-since");
    for bad in ["last-week", "0", "2026-02-30"] {
        let output = marrow(&directory, &["backfill", "--since", bad]);
        assert_eq!(output.status.code(), Some(2), "{bad}");
    }
    assert!(
        !directory.join(".marrow").join("db.sqlite").exists(),
        "a usage error touches nothing"
    );
}
