//! `marrow export --audit`: everything marrow keeps about a repository, printed so a person can
//! check for themselves that none of it is source code (PRD §7.1, §8).
//!
//! The output is one JSON document. It lists every file in `.marrow/`, then every table in the
//! database, SQLite's own included, with its schema and every row. Nothing is summarised or
//! skipped. Blobs are printed losslessly: the store's blobs are arrays of 64-bit hashes, so one
//! whose length is a multiple of eight is printed as those hashes in hex, and any other blob as
//! plain hex bytes.

use std::fmt::Write;

use marrow_store::{Cell, Store};
use serde_json::{json, Value};

use crate::capture::repository_root;

pub fn export_audit() -> Result<String, String> {
    let cwd = std::env::current_dir().map_err(|error| format!("can't find the cwd: {error}"))?;
    let root =
        repository_root(&cwd.canonicalize().unwrap_or(cwd)).ok_or("not inside a git repository")?;
    let marrow = root.join(".marrow");
    let database = marrow.join("db.sqlite");
    if !database.exists() {
        return Err("marrow has no records for this repository yet".to_owned());
    }

    let mut files = Vec::new();
    let entries = std::fs::read_dir(&marrow)
        .map_err(|error| format!("can't list {}: {error}", marrow.display()))?;
    for entry in entries.flatten() {
        let size = entry.metadata().map(|metadata| metadata.len()).ok();
        files.push(json!({
            "name": entry.file_name().to_string_lossy(),
            "bytes": size,
        }));
    }
    files.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));

    let store = Store::open(&database).map_err(|error| format!("{error}"))?;
    let tables = store.audit().map_err(|error| format!("{error}"))?;

    // Written by hand around serde_json so each row lands on its own line: the dump is meant to
    // be read, and searched with grep, as much as parsed.
    let mut out = String::new();
    let _ = writeln!(out, "{{");
    let _ = writeln!(
        out,
        "  \"marrow_version\": {},",
        json!(env!("CARGO_PKG_VERSION"))
    );
    let _ = writeln!(
        out,
        "  \"note\": {},",
        json!(
            "Every file in .marrow/ and every row of every table in .marrow/db.sqlite. \
             Blobs whose length is a multiple of 8 are printed as their 64-bit little-endian \
             values in hex; other blobs as hex bytes."
        )
    );
    let _ = writeln!(out, "  \"files\": {},", Value::Array(files));
    let _ = writeln!(out, "  \"tables\": [");
    for (index, table) in tables.iter().enumerate() {
        let _ = writeln!(out, "    {{");
        let _ = writeln!(out, "      \"name\": {},", json!(table.name));
        let _ = writeln!(out, "      \"schema\": {},", json!(table.sql));
        let _ = writeln!(out, "      \"columns\": {},", json!(table.columns));
        let _ = writeln!(out, "      \"row_count\": {},", table.rows.len());
        if table.rows.is_empty() {
            let _ = writeln!(out, "      \"rows\": []");
        } else {
            let _ = writeln!(out, "      \"rows\": [");
            for (row_index, row) in table.rows.iter().enumerate() {
                let cells: Vec<Value> = row.iter().map(cell).collect();
                let comma = if row_index + 1 < table.rows.len() {
                    ","
                } else {
                    ""
                };
                let _ = writeln!(out, "        {}{comma}", Value::Array(cells));
            }
            let _ = writeln!(out, "      ]");
        }
        let comma = if index + 1 < tables.len() { "," } else { "" };
        let _ = writeln!(out, "    }}{comma}");
    }
    let _ = writeln!(out, "  ]");
    let _ = writeln!(out, "}}");
    Ok(out)
}

fn cell(value: &Cell) -> Value {
    match value {
        Cell::Null => Value::Null,
        Cell::Integer(number) => json!(number),
        Cell::Real(number) => json!(number),
        Cell::Text(text) => json!(text),
        Cell::Blob(bytes) if !bytes.is_empty() && bytes.len() % 8 == 0 => json!({
            "u64": bytes
                .as_chunks::<8>()
                .0
                .iter()
                .map(|chunk| {
                    let value = u64::from_le_bytes(*chunk);
                    format!("{value:016x}")
                })
                .collect::<Vec<_>>()
        }),
        Cell::Blob(bytes) => json!({
            "hex": bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>()
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blobs_of_hashes_print_as_hashes_and_the_rest_as_bytes() {
        let hash = 0x0123_4567_89ab_cdefu64;
        assert_eq!(
            cell(&Cell::Blob(hash.to_le_bytes().to_vec())),
            json!({"u64": ["0123456789abcdef"]})
        );
        assert_eq!(cell(&Cell::Blob(vec![0xde, 0xad])), json!({"hex": "dead"}));
        assert_eq!(cell(&Cell::Blob(Vec::new())), json!({"hex": ""}));
        assert_eq!(cell(&Cell::Null), Value::Null);
    }
}
