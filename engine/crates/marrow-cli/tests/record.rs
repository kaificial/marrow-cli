//! `marrow record` end to end: a scratch repo, real writes, and the stored result.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use marrow_store::Store;

const HELPER: &str = "def normalize_name(raw):\n    trimmed = raw.strip()\n    lowered = trimmed.lower()\n    return lowered.replace(\" \", \"_\")\n";
const CALLER: &str = "def run(rows):\n    return [normalize_name(row) for row in rows]\n";
/// Bigger than the helper, so the diff keeps this still and the helper is what moved.
const BODY: &str = "def run(rows):\n    cleaned = []\n    for row in rows:\n        cleaned.append(normalize_name(row))\n    cleaned.sort()\n    return cleaned\n";

fn scratch(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("marrow-record-{name}"));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(directory.join(".git").join("info")).expect("a scratch repo");
    directory
}

fn write(directory: &Path, name: &str, contents: &str) -> PathBuf {
    let path = directory.join(name);
    std::fs::write(&path, contents).expect("writes the file");
    path
}

fn record(file: &Path, session: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_marrow"))
        .args(["record", "--session", session, "--file"])
        .arg(file)
        .output()
        .expect("runs marrow record")
}

fn recorded(file: &Path, session: &str) -> String {
    let output = record(file, session);
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    assert!(output.status.success(), "record failed: {stderr}");
    assert!(stderr.is_empty(), "unexpected warning: {stderr}");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

struct Stored {
    lines: i64,
    sessions: i64,
    snapshot_lines: i64,
    fates: Vec<(String, f64, String, Option<u32>)>,
    origins: Vec<String>,
}

fn stored(directory: &Path) -> Stored {
    let store = Store::open(&directory.join(".marrow").join("db.sqlite")).expect("opens the store");
    let connection = store.connection();
    let count = |sql: &str| -> i64 { connection.query_row(sql, [], |row| row.get(0)).unwrap() };
    let mut statement = connection
        .prepare(
            "SELECT state, similarity_score, deciding_layer, line_number FROM fates
             ORDER BY observed_at, rowid",
        )
        .unwrap();
    let fates = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, f64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<u32>>(3)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let mut origins = connection
        .prepare("SELECT DISTINCT origin FROM lines")
        .unwrap();
    let origins = origins
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    Stored {
        lines: count("SELECT count(*) FROM lines"),
        sessions: count("SELECT count(*) FROM sessions"),
        snapshot_lines: count("SELECT count(*) FROM snapshot_lines"),
        fates,
        origins,
    }
}

#[test]
fn a_first_write_gives_every_line_a_birth_and_no_fates() {
    let directory = scratch("first-write");
    let file = write(&directory, "inventory.py", HELPER);
    assert_eq!(
        recorded(&file, "session-1"),
        "marrow record: 4 lines born, 0 lines changed"
    );
    let stored = stored(&directory);
    assert_eq!(
        (stored.lines, stored.snapshot_lines, stored.sessions),
        (4, 4, 1)
    );
    assert!(stored.fates.is_empty());
    assert_eq!(stored.origins, ["agent"]);
    assert!(
        std::fs::read_to_string(directory.join(".git").join("info").join("exclude"))
            .unwrap()
            .contains(".marrow/"),
        "the store excludes itself from git"
    );
}

#[test]
fn an_edited_line_is_recorded_with_its_score_and_layer() {
    let directory = scratch("edited");
    let file = write(&directory, "inventory.py", HELPER);
    recorded(&file, "session-1");
    write(
        &directory,
        "inventory.py",
        &HELPER.replace("raw.strip()", "raw.rstrip()"),
    );
    assert_eq!(
        recorded(&file, "session-1"),
        "marrow record: 0 lines born, 1 line changed"
    );
    let stored = stored(&directory);
    assert_eq!(stored.lines, 4, "an edited line keeps its identity");
    let (state, score, layer, line_number) = stored.fates[0].clone();
    assert_eq!(
        (state.as_str(), layer.as_str(), line_number),
        ("edited", "within_hunk_alignment", Some(2))
    );
    assert!((0.5..1.0).contains(&score), "{score}");
}

#[test]
fn a_deleted_line_is_recorded_as_dead() {
    let directory = scratch("deleted");
    let file = write(&directory, "inventory.py", HELPER);
    recorded(&file, "session-1");
    write(
        &directory,
        "inventory.py",
        &HELPER.replace("    lowered = trimmed.lower()\n", ""),
    );
    recorded(&file, "session-1");
    let stored = stored(&directory);
    let dead: Vec<_> = stored
        .fates
        .iter()
        .filter(|fate| fate.0 == "dead")
        .collect();
    assert_eq!(dead.len(), 1, "{:?}", stored.fates);
    assert_eq!(dead[0].2, "no_match");
    assert_eq!(stored.snapshot_lines, 3);
}

#[test]
fn a_block_moved_inside_the_file_is_recorded_as_moved() {
    let directory = scratch("moved");
    let file = write(&directory, "inventory.py", &format!("{HELPER}\n{BODY}"));
    recorded(&file, "session-1");
    write(&directory, "inventory.py", &format!("{BODY}\n{HELPER}"));
    recorded(&file, "session-1");
    let stored = stored(&directory);
    let moved: Vec<_> = stored
        .fates
        .iter()
        .filter(|fate| fate.0 == "moved")
        .collect();
    assert_eq!(moved.len(), 4, "the whole helper moved: {:?}", stored.fates);
    assert!(moved
        .iter()
        .all(|fate| fate.2 == "intra_file_move" && fate.1 == 1.0));
    assert_eq!(stored.lines, 10, "nothing was born or died");
}

#[test]
fn writes_in_quick_succession_each_compare_against_the_one_before() {
    let directory = scratch("succession");
    let file = write(&directory, "inventory.py", HELPER);
    recorded(&file, "session-1");
    write(
        &directory,
        "inventory.py",
        &HELPER.replace("raw.strip()", "raw.rstrip()"),
    );
    recorded(&file, "session-1");
    write(
        &directory,
        "inventory.py",
        &HELPER.replace("raw.strip()", "raw.lstrip()"),
    );
    recorded(&file, "session-1");
    let stored = stored(&directory);
    assert_eq!(
        stored
            .fates
            .iter()
            .map(|fate| fate.0.as_str())
            .collect::<Vec<_>>(),
        ["edited", "edited"],
        "each write is compared against the previous one, so the same line is edited twice"
    );
    assert_eq!(stored.lines, 4);
}

#[test]
fn two_writes_at_the_same_time_both_get_recorded() {
    let directory = scratch("concurrent");
    let first = write(&directory, "first.py", HELPER);
    let second = write(&directory, "second.py", CALLER);
    let mut children: Vec<_> = [&first, &second]
        .into_iter()
        .map(|file| {
            Command::new(env!("CARGO_BIN_EXE_marrow"))
                .args(["record", "--session", "session-1", "--file"])
                .arg(file)
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .expect("starts marrow record")
        })
        .collect();
    for child in &mut children {
        let output = child.wait_with_output_ref();
        assert!(output.0, "a concurrent write failed: {}", output.1);
    }
    let stored = stored(&directory);
    assert_eq!(stored.lines, 6);
    assert_eq!(stored.snapshot_lines, 6);
}

trait WaitOutput {
    fn wait_with_output_ref(&mut self) -> (bool, String);
}

impl WaitOutput for std::process::Child {
    fn wait_with_output_ref(&mut self) -> (bool, String) {
        let status = self.wait().expect("waits");
        let mut stderr = String::new();
        if let Some(pipe) = self.stderr.as_mut() {
            use std::io::Read;
            let _ = pipe.read_to_string(&mut stderr);
        }
        (status.success(), stderr)
    }
}

#[test]
fn the_hook_payload_carries_the_session_and_file() {
    let directory = scratch("hook");
    let file = write(&directory, "inventory.py", HELPER);
    let payload = serde_json::json!({
        "session_id": "session-from-hook",
        "hook_event_name": "PostToolUse",
        "tool_name": "Write",
        "tool_input": {"file_path": file.to_string_lossy()},
    })
    .to_string();
    let mut child = Command::new(env!("CARGO_BIN_EXE_marrow"))
        .args(["record", "--hook"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("starts marrow record");
    {
        use std::io::Write;
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(payload.as_bytes())
            .unwrap();
    }
    let output = child.wait_with_output().expect("waits");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let store = Store::open(&directory.join(".marrow").join("db.sqlite")).unwrap();
    let session: String = store
        .connection()
        .query_row("SELECT session_id FROM sessions", [], |row| row.get(0))
        .unwrap();
    assert_eq!(session, "session-from-hook");
}

#[test]
fn files_marrow_cannot_read_are_skipped_without_failing_the_write() {
    let directory = scratch("skipped");
    for (name, contents) in [
        ("README.md", b"# notes\n".to_vec()),
        ("logo.py", vec![0xff, 0xfe, 0x00]),
    ] {
        let path = directory.join(name);
        std::fs::write(&path, contents).unwrap();
        let output = record(&path, "session-1");
        assert!(output.status.success(), "{name} failed the write");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("skipped"), "{name}: {stderr}");
    }
    assert!(
        !directory.join(".marrow").join("db.sqlite").exists(),
        "a skipped file leaves no store behind"
    );
}

#[test]
fn a_write_outside_a_repository_is_skipped() {
    let directory = std::env::temp_dir().join("marrow-record-no-repo");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    let file = write(&directory, "inventory.py", HELPER);
    let output = record(&file, "session-1");
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not inside a git repository"));
}

#[test]
fn record_without_a_session_or_file_is_a_usage_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_marrow"))
        .args(["record"])
        .output()
        .expect("runs");
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("needs --session and --file"));
}

/// The parser only gives up after PARSE_TIMEOUT_MICROS, so this waits ten seconds.
#[test]
#[ignore]
fn a_file_the_parser_gives_up_on_is_skipped() {
    let directory = scratch("unparsed");
    let path = directory.join("hang.ts");
    std::fs::write(
        &path,
        [
            0x24, 0x5c, 0x27, 0x27, 0x27, 0x2a, 0x5b, 0x7d, 0x3d, 0x72, 0x22, 0x66, 0x22, 0x66,
            0x22, 0x29,
        ],
    )
    .unwrap();
    let output = record(&path, "session-1");
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("gave up parsing"));
}
