//! Reading every table back exactly as stored, for `marrow export --audit`. The table list comes
//! from SQLite's own catalogue rather than from the schema this crate creates, so a table or
//! column added later is dumped without anyone remembering to add it here.

use rusqlite::types::ValueRef;

use crate::{Error, Store};

/// One stored value, as SQLite holds it.
#[derive(Clone, Debug, PartialEq)]
pub enum Cell {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

/// A whole table: the statement that created it, its columns, and every row.
#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    pub name: String,
    pub sql: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Cell>>,
}

impl Store {
    /// Every table in the database, SQLite's internal ones included, with every row.
    pub fn audit(&self) -> Result<Vec<Table>, Error> {
        let mut statement = self.connection.prepare(
            "SELECT name, coalesce(sql, '') FROM sqlite_master WHERE type = 'table' ORDER BY name",
        )?;
        let catalogue = statement
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get(1)?)))?
            .collect::<Result<Vec<(String, String)>, _>>()?;
        let mut tables = Vec::with_capacity(catalogue.len());
        for (name, sql) in catalogue {
            let quoted = format!("\"{}\"", name.replace('"', "\"\""));
            let mut select = self
                .connection
                .prepare(&format!("SELECT * FROM {quoted}"))?;
            let columns: Vec<String> = select
                .column_names()
                .into_iter()
                .map(str::to_owned)
                .collect();
            let width = columns.len();
            let rows = select
                .query_map([], |row| {
                    (0..width)
                        .map(|index| {
                            Ok(match row.get_ref(index)? {
                                ValueRef::Null => Cell::Null,
                                ValueRef::Integer(value) => Cell::Integer(value),
                                ValueRef::Real(value) => Cell::Real(value),
                                ValueRef::Text(bytes) => {
                                    Cell::Text(String::from_utf8_lossy(bytes).into_owned())
                                }
                                ValueRef::Blob(bytes) => Cell::Blob(bytes.to_vec()),
                            })
                        })
                        .collect::<rusqlite::Result<Vec<Cell>>>()
                })?
                .collect::<Result<Vec<_>, _>>()?;
            tables.push(Table {
                name,
                sql,
                columns,
                rows,
            });
        }
        Ok(tables)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_table_is_dumped_with_its_rows() {
        let directory = std::env::temp_dir().join(format!("marrow-audit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let store = Store::open(&directory.join("db.sqlite")).unwrap();
        store
            .connection()
            .execute_batch("CREATE TABLE extra (a, b); INSERT INTO extra VALUES (x'0102', 1.5);")
            .unwrap();
        let tables = store.audit().unwrap();
        let names: Vec<&str> = tables.iter().map(|table| table.name.as_str()).collect();
        for expected in [
            "commits",
            "extra",
            "fates",
            "lines",
            "meta",
            "sessions",
            "snapshot_lines",
            "snapshots",
        ] {
            assert!(
                names.contains(&expected),
                "{expected} missing from {names:?}"
            );
        }
        let extra = tables.iter().find(|table| table.name == "extra").unwrap();
        assert_eq!(extra.columns, ["a", "b"]);
        assert_eq!(extra.rows, [vec![Cell::Blob(vec![1, 2]), Cell::Real(1.5)]]);
        let meta = tables.iter().find(|table| table.name == "meta").unwrap();
        assert!(meta.rows.contains(&vec![
            Cell::Text("schema_version".to_owned()),
            Cell::Text(crate::SCHEMA_VERSION.to_string()),
        ]));
        drop(store);
        let _ = std::fs::remove_dir_all(&directory);
    }
}
