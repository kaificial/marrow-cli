//! SQLite storage for captured lines, fates, sessions, commits, and the latest known state of
//! each tracked file. Only hashes, counts, and metadata are stored, never source text.

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

pub const SCHEMA_VERSION: i64 = 2;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("can't open the store at {path}: {message}")]
    Open { path: String, message: String },
    #[error(
        "the store was built with schema version {found}, and this build of marrow uses \
         version {expected}"
    )]
    Schema { found: i64, expected: i64 },
    #[error("store error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Agent,
    Human,
    AgentThenHumanRevised,
}

impl Origin {
    pub fn as_str(self) -> &'static str {
        match self {
            Origin::Agent => "agent",
            Origin::Human => "human",
            Origin::AgentThenHumanRevised => "agent_then_human_revised",
        }
    }
}

/// One tracked line of a file version, as hashes and counts.
#[derive(Clone, Debug, PartialEq)]
pub struct SnapshotLine {
    pub line_id: i64,
    pub line_number: u32,
    pub kind: String,
    pub role: String,
    pub content_tokens: u32,
    pub fingerprint: u64,
    pub tokens: Vec<u64>,
    pub unigrams: Vec<u64>,
    pub bigrams: Vec<u64>,
}

/// The last known state of one file: what the next write is compared against.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub language: String,
    pub normalizer_version: i64,
    pub observed_at: i64,
    /// Git's blob id for the contents this was built from, so reconciliation can tell whether
    /// the committed version is the one we already saw. Null for snapshots written before the
    /// content was known.
    pub content_id: Option<String>,
    pub lines: Vec<SnapshotLine>,
}

/// A line seen for the first time.
#[derive(Clone, Debug)]
pub struct BornLine<'a> {
    pub file_path_hash: &'a str,
    pub birth_commit: Option<&'a str>,
    pub birth_ts: i64,
    pub origin: Origin,
    pub session_id: Option<&'a str>,
    pub model: Option<&'a str>,
    pub syntactic_role: &'a str,
    pub token_count: u32,
}

/// Something that happened to a line at a point in time.
#[derive(Clone, Debug)]
pub struct FateRow<'a> {
    pub line_id: i64,
    pub observed_at: i64,
    /// The last time the line was known to be alive and unchanged. The change happened somewhere
    /// between then and `observed_at`, which is what interval-censored survival analysis needs.
    pub previous_seen_at: Option<i64>,
    pub state: &'a str,
    pub similarity_score: f64,
    pub deciding_layer: &'a str,
    pub matched_candidate_id: Option<i64>,
    pub line_number: Option<u32>,
}

pub struct Store {
    connection: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Store, Error> {
        let connection = Connection::open(path).map_err(|error| Error::Open {
            path: path.display().to_string(),
            message: error.to_string(),
        })?;
        // A hook fires once per agent write, and two writes can overlap, so wait for the lock
        // instead of failing the agent's write.
        connection.busy_timeout(std::time::Duration::from_secs(10))?;
        connection.pragma_update(None, "journal_mode", "wal")?;
        connection.pragma_update(None, "foreign_keys", "on")?;
        let mut store = Store { connection };
        match store.stored_version()? {
            Some(SCHEMA_VERSION) => {}
            Some(found) => {
                return Err(Error::Schema {
                    found,
                    expected: SCHEMA_VERSION,
                })
            }
            None => store.create_schema()?,
        }
        Ok(store)
    }

    /// The version an existing store was built with, or `None` for a store that has no tables yet.
    /// Read before creating anything, so an older store is reported rather than half-upgraded.
    fn stored_version(&self) -> Result<Option<i64>, Error> {
        // Any of marrow's tables means this store was built by some version of marrow. Asking
        // only about `meta` would let a store that predates it be half-upgraded in place.
        let tables: i64 = self.connection.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name IN
                 ('meta', 'sessions', 'commits', 'lines', 'fates', 'snapshots', 'snapshot_lines')",
            [],
            |row| row.get(0),
        )?;
        if tables == 0 {
            return Ok(None);
        }
        let version: Option<String> = self
            .connection
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        // A store with tables but no readable version is from before versions were stamped.
        Ok(Some(
            version.and_then(|value| value.parse().ok()).unwrap_or(0),
        ))
    }

    fn create_schema(&mut self) -> Result<(), Error> {
        self.connection.execute_batch(
            "BEGIN IMMEDIATE;
             CREATE TABLE IF NOT EXISTS meta (
                 key TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS sessions (
                 session_id TEXT PRIMARY KEY,
                 tool TEXT NOT NULL,
                 model TEXT,
                 started_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS commits (
                 sha TEXT PRIMARY KEY,
                 authored_at INTEGER NOT NULL,
                 is_merge INTEGER NOT NULL
             );
             -- last_seen_at and last_seen_commit are the censoring checkpoint: a line with no
             -- later fate row was still alive, unchanged, at that time and commit.
             CREATE TABLE IF NOT EXISTS lines (
                 line_id INTEGER PRIMARY KEY,
                 file_path_hash TEXT NOT NULL,
                 birth_commit TEXT REFERENCES commits(sha),
                 birth_ts INTEGER NOT NULL,
                 origin TEXT NOT NULL,
                 session_id TEXT REFERENCES sessions(session_id),
                 model TEXT,
                 syntactic_role TEXT NOT NULL,
                 token_count INTEGER NOT NULL,
                 last_seen_at INTEGER NOT NULL,
                 last_seen_commit TEXT REFERENCES commits(sha)
             );
             CREATE INDEX IF NOT EXISTS lines_by_file ON lines(file_path_hash);
             -- No key on (line_id, observed_at): two writes can land in the same second, and
             -- both are real history. Rows are ordered by rowid.
             CREATE TABLE IF NOT EXISTS fates (
                 line_id INTEGER NOT NULL REFERENCES lines(line_id),
                 observed_at INTEGER NOT NULL,
                 previous_seen_at INTEGER,
                 state TEXT NOT NULL,
                 similarity_score REAL NOT NULL,
                 deciding_layer TEXT NOT NULL,
                 matched_candidate_id INTEGER REFERENCES lines(line_id),
                 line_number INTEGER
             );
             CREATE INDEX IF NOT EXISTS fates_by_line ON fates(line_id, observed_at);
             CREATE TABLE IF NOT EXISTS snapshots (
                 file_path_hash TEXT PRIMARY KEY,
                 language TEXT NOT NULL,
                 normalizer_version INTEGER NOT NULL,
                 observed_at INTEGER NOT NULL,
                 content_id TEXT
             );
             CREATE TABLE IF NOT EXISTS snapshot_lines (
                 file_path_hash TEXT NOT NULL REFERENCES snapshots(file_path_hash) ON DELETE CASCADE,
                 line_number INTEGER NOT NULL,
                 line_id INTEGER NOT NULL REFERENCES lines(line_id),
                 kind TEXT NOT NULL,
                 syntactic_role TEXT NOT NULL,
                 content_tokens INTEGER NOT NULL,
                 fingerprint BLOB NOT NULL,
                 tokens BLOB NOT NULL,
                 unigrams BLOB NOT NULL,
                 bigrams BLOB NOT NULL,
                 PRIMARY KEY (file_path_hash, line_number)
             );
             COMMIT;",
        )?;
        self.connection.execute(
            "INSERT OR IGNORE INTO meta (key, value) VALUES ('schema_version', ?1)",
            params![SCHEMA_VERSION.to_string()],
        )?;
        Ok(())
    }

    pub fn schema_version(&self) -> Result<i64, Error> {
        let version: String = self.connection.query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )?;
        Ok(version.parse().unwrap_or(0))
    }

    /// Takes the write lock up front, so two overlapping writes queue instead of racing.
    pub fn begin(&mut self) -> Result<Recording<'_>, Error> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        Ok(Recording { transaction })
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }
}

pub struct Recording<'a> {
    transaction: rusqlite::Transaction<'a>,
}

impl Recording<'_> {
    pub fn ensure_session(
        &self,
        session_id: &str,
        tool: &str,
        model: Option<&str>,
        started_at: i64,
    ) -> Result<(), Error> {
        self.transaction.execute(
            "INSERT INTO sessions (session_id, tool, model, started_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(session_id) DO UPDATE SET model = COALESCE(excluded.model, model)",
            params![session_id, tool, model, started_at],
        )?;
        Ok(())
    }

    /// Every file the store has a last known state for.
    pub fn snapshot_files(&self) -> Result<Vec<String>, Error> {
        let mut statement = self
            .transaction
            .prepare("SELECT file_path_hash FROM snapshots ORDER BY file_path_hash")?;
        let hashes = statement
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(hashes)
    }

    pub fn snapshot(&self, file_path_hash: &str) -> Result<Option<Snapshot>, Error> {
        let header = self
            .transaction
            .query_row(
                "SELECT language, normalizer_version, observed_at, content_id FROM snapshots
                 WHERE file_path_hash = ?1",
                params![file_path_hash],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .optional()?;
        let Some((language, normalizer_version, observed_at, content_id)) = header else {
            return Ok(None);
        };
        let mut statement = self.transaction.prepare(
            "SELECT line_id, line_number, kind, syntactic_role, content_tokens, fingerprint,
                    tokens, unigrams, bigrams
             FROM snapshot_lines WHERE file_path_hash = ?1 ORDER BY line_number",
        )?;
        let lines = statement
            .query_map(params![file_path_hash], |row| {
                Ok(SnapshotLine {
                    line_id: row.get(0)?,
                    line_number: row.get(1)?,
                    kind: row.get(2)?,
                    role: row.get(3)?,
                    content_tokens: row.get(4)?,
                    fingerprint: decode_hashes(&row.get::<_, Vec<u8>>(5)?)
                        .first()
                        .copied()
                        .unwrap_or_default(),
                    tokens: decode_hashes(&row.get::<_, Vec<u8>>(6)?),
                    unigrams: decode_hashes(&row.get::<_, Vec<u8>>(7)?),
                    bigrams: decode_hashes(&row.get::<_, Vec<u8>>(8)?),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some(Snapshot {
            language,
            normalizer_version,
            observed_at,
            content_id,
            lines,
        }))
    }

    pub fn insert_line(&self, line: &BornLine<'_>) -> Result<i64, Error> {
        self.transaction.execute(
            "INSERT INTO lines (file_path_hash, birth_commit, birth_ts, origin, session_id, model,
                                syntactic_role, token_count, last_seen_at, last_seen_commit)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?3, ?2)",
            params![
                line.file_path_hash,
                line.birth_commit,
                line.birth_ts,
                line.origin.as_str(),
                line.session_id,
                line.model,
                line.syntactic_role,
                line.token_count
            ],
        )?;
        Ok(self.transaction.last_insert_rowid())
    }

    pub fn insert_fate(&self, fate: &FateRow<'_>) -> Result<(), Error> {
        self.transaction.execute(
            "INSERT INTO fates (line_id, observed_at, previous_seen_at, state, similarity_score,
                                deciding_layer, matched_candidate_id, line_number)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                fate.line_id,
                fate.observed_at,
                fate.previous_seen_at,
                fate.state,
                fate.similarity_score,
                fate.deciding_layer,
                fate.matched_candidate_id,
                fate.line_number
            ],
        )?;
        Ok(())
    }

    /// Moves the censoring checkpoint forward for lines that are still alive.
    pub fn mark_alive(
        &self,
        line_ids: &[i64],
        observed_at: i64,
        commit: Option<&str>,
    ) -> Result<(), Error> {
        // MAX, so a commit dated earlier than a write we already captured can't walk the
        // checkpoint backwards. The commit only comes along when the time does, so the two
        // halves always describe the same sighting.
        let mut update = self.transaction.prepare(
            "UPDATE lines SET
                 last_seen_commit = CASE
                     WHEN ?3 IS NOT NULL AND ?2 >= last_seen_at THEN ?3
                     ELSE last_seen_commit
                 END,
                 last_seen_at = MAX(last_seen_at, ?2)
             WHERE line_id = ?1",
        )?;
        for line_id in line_ids {
            update.execute(params![line_id, observed_at, commit])?;
        }
        Ok(())
    }

    /// Attaches the commit a line first appeared in. Lines captured from a write don't have one
    /// yet, and lines already anchored keep the commit they were anchored to.
    pub fn anchor_births(&self, line_ids: &[i64], commit: &str) -> Result<usize, Error> {
        let mut update = self.transaction.prepare(
            "UPDATE lines SET birth_commit = ?2 WHERE line_id = ?1 AND birth_commit IS NULL",
        )?;
        let mut anchored = 0;
        for line_id in line_ids {
            anchored += update.execute(params![line_id, commit])?;
        }
        Ok(anchored)
    }

    /// An agent-written line a person then changed becomes the third origin (PRD §6). A line that
    /// is already human-written, or already relabelled, keeps the origin it has.
    pub fn mark_revised(&self, line_id: i64) -> Result<bool, Error> {
        let changed = self.transaction.execute(
            "UPDATE lines SET origin = ?2 WHERE line_id = ?1 AND origin = ?3",
            params![
                line_id,
                Origin::AgentThenHumanRevised.as_str(),
                Origin::Agent.as_str()
            ],
        )?;
        Ok(changed > 0)
    }

    pub fn insert_commit(&self, sha: &str, authored_at: i64, is_merge: bool) -> Result<(), Error> {
        self.transaction.execute(
            "INSERT INTO commits (sha, authored_at, is_merge) VALUES (?1, ?2, ?3)
             ON CONFLICT(sha) DO NOTHING",
            params![sha, authored_at, i64::from(is_merge)],
        )?;
        Ok(())
    }

    /// Records that a file's last known state is still current, without rewriting its lines.
    pub fn touch_snapshot(&self, file_path_hash: &str, observed_at: i64) -> Result<(), Error> {
        self.transaction.execute(
            "UPDATE snapshots SET observed_at = MAX(observed_at, ?2) WHERE file_path_hash = ?1",
            params![file_path_hash, observed_at],
        )?;
        Ok(())
    }

    /// Forgets the last known state of a file. Its history in `lines` and `fates` stays.
    pub fn forget_snapshot(&self, file_path_hash: &str) -> Result<(), Error> {
        self.transaction.execute(
            "DELETE FROM snapshot_lines WHERE file_path_hash = ?1",
            params![file_path_hash],
        )?;
        self.transaction.execute(
            "DELETE FROM snapshots WHERE file_path_hash = ?1",
            params![file_path_hash],
        )?;
        Ok(())
    }

    pub fn replace_snapshot(&self, file_path_hash: &str, snapshot: &Snapshot) -> Result<(), Error> {
        self.transaction.execute(
            "INSERT INTO snapshots (file_path_hash, language, normalizer_version, observed_at,
                                    content_id)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(file_path_hash) DO UPDATE SET language = excluded.language,
                 normalizer_version = excluded.normalizer_version,
                 observed_at = excluded.observed_at,
                 content_id = excluded.content_id",
            params![
                file_path_hash,
                snapshot.language,
                snapshot.normalizer_version,
                snapshot.observed_at,
                snapshot.content_id
            ],
        )?;
        self.transaction.execute(
            "DELETE FROM snapshot_lines WHERE file_path_hash = ?1",
            params![file_path_hash],
        )?;
        let mut insert = self.transaction.prepare(
            "INSERT INTO snapshot_lines (file_path_hash, line_number, line_id, kind,
                                         syntactic_role, content_tokens, fingerprint, tokens,
                                         unigrams, bigrams)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        )?;
        for line in &snapshot.lines {
            insert.execute(params![
                file_path_hash,
                line.line_number,
                line.line_id,
                line.kind,
                line.role,
                line.content_tokens,
                encode_hashes(std::slice::from_ref(&line.fingerprint)),
                encode_hashes(&line.tokens),
                encode_hashes(&line.unigrams),
                encode_hashes(&line.bigrams),
            ])?;
        }
        Ok(())
    }

    pub fn commit(self) -> Result<(), Error> {
        self.transaction.commit()?;
        Ok(())
    }
}

fn encode_hashes(hashes: &[u64]) -> Vec<u8> {
    hashes.iter().flat_map(|hash| hash.to_le_bytes()).collect()
}

fn decode_hashes(bytes: &[u8]) -> Vec<u64> {
    bytes
        .chunks_exact(8)
        .map(|chunk| u64::from_le_bytes(chunk.try_into().expect("eight bytes")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{BornLine, FateRow, Origin, Snapshot, SnapshotLine, Store, SCHEMA_VERSION};

    fn line(line_id: i64, line_number: u32, fingerprint: u64) -> SnapshotLine {
        SnapshotLine {
            line_id,
            line_number,
            kind: "code".to_owned(),
            role: "expression_statement".to_owned(),
            content_tokens: 3,
            fingerprint,
            tokens: vec![1, 2, 3],
            unigrams: vec![1, 2, 3],
            bigrams: vec![4, 5, 6, 7],
        }
    }

    fn store() -> Store {
        let path = std::env::temp_dir().join(format!(
            "marrow-store-test-{}-{:?}.sqlite",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_file(&path);
        Store::open(&path).expect("opens")
    }

    #[test]
    fn a_new_store_has_the_current_schema_and_no_rows() {
        let store = store();
        assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
        let lines: i64 = store
            .connection()
            .query_row("SELECT count(*) FROM lines", [], |row| row.get(0))
            .unwrap();
        assert_eq!(lines, 0);
    }

    #[test]
    fn a_snapshot_comes_back_exactly_as_it_was_written() {
        let mut store = store();
        let recording = store.begin().unwrap();
        recording
            .ensure_session(
                "session-1",
                "claude_code",
                Some("claude-sonnet-5"),
                1_700_000_000,
            )
            .unwrap();
        let first = recording
            .insert_line(&BornLine {
                file_path_hash: "abc",
                birth_commit: None,
                birth_ts: 1_700_000_000,
                origin: Origin::Agent,
                session_id: Some("session-1"),
                model: Some("claude-sonnet-5"),
                syntactic_role: "function_item",
                token_count: 7,
            })
            .unwrap();
        let snapshot = Snapshot {
            language: "rust".to_owned(),
            normalizer_version: 1,
            observed_at: 1_700_000_000,
            content_id: Some("e69de29bb2d1d6434b8b29ae775ad8c2e48c5391".to_owned()),
            lines: vec![line(first, 1, 0xdead_beef)],
        };
        recording.replace_snapshot("abc", &snapshot).unwrap();
        recording.commit().unwrap();

        let recording = store.begin().unwrap();
        assert_eq!(recording.snapshot("abc").unwrap(), Some(snapshot));
        assert_eq!(recording.snapshot("missing").unwrap(), None);
    }

    #[test]
    fn replacing_a_snapshot_drops_the_old_lines_and_keeps_the_history() {
        let mut store = store();
        let recording = store.begin().unwrap();
        recording
            .ensure_session("session-1", "claude_code", None, 10)
            .unwrap();
        let first = recording
            .insert_line(&BornLine {
                file_path_hash: "abc",
                birth_commit: None,
                birth_ts: 10,
                origin: Origin::Agent,
                session_id: Some("session-1"),
                model: None,
                syntactic_role: "",
                token_count: 3,
            })
            .unwrap();
        recording
            .replace_snapshot(
                "abc",
                &Snapshot {
                    language: "python".to_owned(),
                    normalizer_version: 1,
                    observed_at: 10,
                    content_id: None,
                    lines: vec![line(first, 1, 1), line(first, 2, 2)],
                },
            )
            .unwrap();
        recording
            .insert_fate(&FateRow {
                line_id: first,
                observed_at: 20,
                previous_seen_at: Some(10),
                state: "edited",
                similarity_score: 0.75,
                deciding_layer: "within_hunk_alignment",
                matched_candidate_id: None,
                line_number: Some(1),
            })
            .unwrap();
        recording
            .replace_snapshot(
                "abc",
                &Snapshot {
                    language: "python".to_owned(),
                    normalizer_version: 1,
                    observed_at: 20,
                    content_id: None,
                    lines: vec![line(first, 1, 9)],
                },
            )
            .unwrap();
        recording.commit().unwrap();

        let recording = store.begin().unwrap();
        let snapshot = recording.snapshot("abc").unwrap().unwrap();
        assert_eq!(snapshot.observed_at, 20);
        assert_eq!(snapshot.lines.len(), 1);
        assert_eq!(snapshot.lines[0].fingerprint, 9);
        let fates: i64 = recording
            .transaction
            .query_row("SELECT count(*) FROM fates", [], |row| row.get(0))
            .unwrap();
        assert_eq!(fates, 1);
    }

    #[test]
    fn a_session_keeps_its_model_when_a_later_write_does_not_know_it() {
        let mut store = store();
        let recording = store.begin().unwrap();
        recording
            .ensure_session("session-1", "claude_code", Some("claude-opus-5"), 5)
            .unwrap();
        recording
            .ensure_session("session-1", "claude_code", None, 9)
            .unwrap();
        recording.commit().unwrap();
        let model: Option<String> = store
            .connection()
            .query_row(
                "SELECT model FROM sessions WHERE session_id = 'session-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(model.as_deref(), Some("claude-opus-5"));
    }
}
