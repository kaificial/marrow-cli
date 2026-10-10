//! Reading the store back: the records `explain`, `report` and `status` are built from.

use std::collections::HashSet;

use rusqlite::{params, OptionalExtension, Row};

use crate::{read_snapshot, Error, Snapshot, Store};

/// A line as the store knows it.
#[derive(Clone, Debug, PartialEq)]
pub struct LineRecord {
    pub line_id: i64,
    /// The file it was born in.
    pub file_path_hash: String,
    pub birth_commit: Option<String>,
    pub birth_ts: i64,
    pub birth_line_number: u32,
    pub birth_observed: bool,
    pub origin: String,
    pub session_id: Option<String>,
    pub model: Option<String>,
    pub syntactic_role: String,
    pub last_seen_at: i64,
    pub last_seen_commit: Option<String>,
    /// When the line's first commit appeared on the branch, if it has one.
    pub entered_at: Option<i64>,
}

/// One thing that happened to a line.
#[derive(Clone, Debug, PartialEq)]
pub struct FateRecord {
    pub line_id: i64,
    pub observed_at: i64,
    pub observed_commit: Option<String>,
    pub previous_seen_at: Option<i64>,
    pub state: String,
    pub similarity_score: f64,
    pub deciding_layer: String,
    pub matched_candidate_id: Option<i64>,
    pub line_number: Option<u32>,
    pub file_path_hash: Option<String>,
    pub birth_similarity: Option<f64>,
    pub before_first_commit: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CommitRecord {
    pub sha: String,
    pub authored_at: i64,
    pub committed_at: i64,
    pub is_merge: bool,
    pub observed_by: String,
    pub agent_trailer: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SessionRecord {
    pub session_id: String,
    pub tool: String,
    pub model: Option<String>,
    pub started_at: i64,
}

const LINE_COLUMNS: &str = "l.line_id, l.file_path_hash, l.birth_commit, l.birth_ts,
    l.birth_line_number, l.birth_observed, l.origin, l.session_id, l.model, l.syntactic_role,
    l.last_seen_at, l.last_seen_commit, c.committed_at";

const FATE_COLUMNS: &str = "line_id, observed_at, observed_commit, previous_seen_at, state,
    similarity_score, deciding_layer, matched_candidate_id, line_number, file_path_hash,
    birth_similarity, before_first_commit";

fn line(row: &Row<'_>) -> rusqlite::Result<LineRecord> {
    Ok(LineRecord {
        line_id: row.get(0)?,
        file_path_hash: row.get(1)?,
        birth_commit: row.get(2)?,
        birth_ts: row.get(3)?,
        birth_line_number: row.get(4)?,
        birth_observed: row.get::<_, i64>(5)? != 0,
        origin: row.get(6)?,
        session_id: row.get(7)?,
        model: row.get(8)?,
        syntactic_role: row.get(9)?,
        last_seen_at: row.get(10)?,
        last_seen_commit: row.get(11)?,
        entered_at: row.get(12)?,
    })
}

fn fate(row: &Row<'_>) -> rusqlite::Result<FateRecord> {
    Ok(FateRecord {
        line_id: row.get(0)?,
        observed_at: row.get(1)?,
        observed_commit: row.get(2)?,
        previous_seen_at: row.get(3)?,
        state: row.get(4)?,
        similarity_score: row.get(5)?,
        deciding_layer: row.get(6)?,
        matched_candidate_id: row.get(7)?,
        line_number: row.get(8)?,
        file_path_hash: row.get(9)?,
        birth_similarity: row.get(10)?,
        before_first_commit: row.get::<_, i64>(11)? != 0,
    })
}

impl Store {
    /// Every line, in the order it was born.
    pub fn lines(&self) -> Result<Vec<LineRecord>, Error> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT {LINE_COLUMNS} FROM lines l LEFT JOIN commits c ON c.sha = l.birth_commit
             ORDER BY l.line_id"
        ))?;
        let lines = statement
            .query_map([], line)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(lines)
    }

    pub fn line(&self, line_id: i64) -> Result<Option<LineRecord>, Error> {
        Ok(self
            .connection
            .query_row(
                &format!(
                    "SELECT {LINE_COLUMNS} FROM lines l
                     LEFT JOIN commits c ON c.sha = l.birth_commit WHERE l.line_id = ?1"
                ),
                params![line_id],
                line,
            )
            .optional()?)
    }

    /// Every fate, grouped by line and in the order each line's happened.
    pub fn fates(&self) -> Result<Vec<FateRecord>, Error> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT {FATE_COLUMNS} FROM fates ORDER BY line_id, rowid"
        ))?;
        let fates = statement
            .query_map([], fate)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(fates)
    }

    pub fn fates_of(&self, line_id: i64) -> Result<Vec<FateRecord>, Error> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT {FATE_COLUMNS} FROM fates WHERE line_id = ?1 ORDER BY rowid"
        ))?;
        let fates = statement
            .query_map(params![line_id], fate)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(fates)
    }

    /// Where a line is now, if some file's last known state still holds it.
    pub fn position(&self, line_id: i64) -> Result<Option<(String, u32)>, Error> {
        Ok(self
            .connection
            .query_row(
                "SELECT file_path_hash, line_number FROM snapshot_lines WHERE line_id = ?1",
                params![line_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?)
    }

    /// Lines some file's last known state still holds.
    pub fn alive(&self) -> Result<HashSet<i64>, Error> {
        let mut statement = self
            .connection
            .prepare("SELECT line_id FROM snapshot_lines")?;
        let alive = statement
            .query_map([], |row| row.get(0))?
            .collect::<Result<HashSet<_>, _>>()?;
        Ok(alive)
    }

    pub fn commit(&self, sha: &str) -> Result<Option<CommitRecord>, Error> {
        Ok(self
            .connection
            .query_row(
                "SELECT sha, authored_at, committed_at, is_merge, observed_by, agent_trailer
                 FROM commits WHERE sha = ?1",
                params![sha],
                |row| {
                    Ok(CommitRecord {
                        sha: row.get(0)?,
                        authored_at: row.get(1)?,
                        committed_at: row.get(2)?,
                        is_merge: row.get::<_, i64>(3)? != 0,
                        observed_by: row.get(4)?,
                        agent_trailer: row.get(5)?,
                    })
                },
            )
            .optional()?)
    }

    /// Every agent session, the most recent first.
    pub fn sessions(&self) -> Result<Vec<SessionRecord>, Error> {
        let mut statement = self.connection.prepare(
            "SELECT session_id, tool, model, started_at FROM sessions
             ORDER BY started_at DESC, rowid DESC",
        )?;
        let sessions = statement
            .query_map([], |row| {
                Ok(SessionRecord {
                    session_id: row.get(0)?,
                    tool: row.get(1)?,
                    model: row.get(2)?,
                    started_at: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(sessions)
    }

    /// A file's last known state, outside any recording.
    pub fn snapshot(&self, file_path_hash: &str) -> Result<Option<Snapshot>, Error> {
        read_snapshot(&self.connection, file_path_hash)
    }

    /// One `meta` value, outside any recording.
    pub fn meta(&self, key: &str) -> Result<Option<String>, Error> {
        Ok(self
            .connection
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()?)
    }
}
