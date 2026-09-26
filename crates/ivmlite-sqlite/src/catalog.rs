//! The SQLite implementation of `ivmlite_sql::Catalog` (spec §4.3, §7.1).

use ivmlite_core::{Column, ColumnType, Schema};
use ivmlite_sql::{Catalog, CatalogError};
use rusqlite::{Connection, OptionalExtension};

use crate::names::{literal, PREFIX};

/// Reads a table's shape from `pragma_table_list`, `pragma_table_info` and its
/// `CREATE` statement, and refuses what v0 cannot represent.
pub struct SqliteCatalog<'a> {
    pub conn: &'a Connection,
}

impl Catalog for SqliteCatalog<'_> {
    fn table(&self, name: &str) -> Result<Option<Schema>, CatalogError> {
        let err = |e: rusqlite::Error| CatalogError(format!("reading the catalog: {e}"));
        // pragma_table_list matches names case-insensitively, as SQLite does.
        let found: Option<(String, String, bool)> = self
            .conn
            .query_row(
                "SELECT name, type, strict FROM pragma_table_list WHERE schema = 'main' AND name = ?1",
                [name],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(err)?;
        let Some((declared, kind, strict)) = found else {
            return Ok(None);
        };
        let refuse = |why: &str| Err(CatalogError(format!("table {declared}: {why}")));
        if declared.len() >= PREFIX.len() && declared[..PREFIX.len()].eq_ignore_ascii_case(PREFIX) {
            return refuse("is one of ivmlite's own shadow tables");
        }
        if kind != "table" {
            return refuse(&format!("is a {kind}, not an ordinary table"));
        }
        if !strict {
            return refuse(
                "is not STRICT; v0 needs STRICT tables so a column's values have one type (spec §7.1)",
            );
        }
        let sql: String = self
            .conn
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = ?1",
                [&declared],
                |r| r.get(0),
            )
            .map_err(err)?;
        if sql.to_ascii_uppercase().contains("COLLATE") {
            return refuse(
                "declares a COLLATE clause; v0 supports only the BINARY collation (spec §7.1)",
            );
        }
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT name, type, \"notnull\", pk FROM pragma_table_info({})",
                literal(&declared)
            ))
            .map_err(err)?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, bool>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            })
            .map_err(err)?;
        let mut columns = Vec::new();
        for row in rows {
            let (column, ty, not_null, pk) = row.map_err(err)?;
            let ty = match ty.to_ascii_uppercase().as_str() {
                "INTEGER" | "INT" => ColumnType::Integer,
                "TEXT" => ColumnType::Text,
                other => {
                    return refuse(&format!(
                    "column {column} has type {other}; v0 supports INTEGER and TEXT columns only"
                ))
                }
            };
            // An INTEGER PRIMARY KEY is the rowid and can never be NULL.
            let rowid = pk == 1 && ty == ColumnType::Integer;
            columns.push(Column {
                name: column,
                ty,
                nullable: !not_null && !rowid,
            });
        }
        Ok(Some(Schema {
            table: declared,
            columns,
        }))
    }
}

/// Fails unless the database is UTF-8: TEXT compares by UTF-8 byte order in
/// the engine, which is SQLite's BINARY collation only for UTF-8 (Phase 3a
/// spec §5; measured, `'Ā' > 'a'` differs between UTF-8 and UTF-16LE).
pub fn require_utf8(conn: &Connection) -> Result<(), String> {
    let encoding: String = conn
        .query_row("PRAGMA encoding", [], |r| r.get(0))
        .map_err(|e| format!("reading the database encoding: {e}"))?;
    if encoding == "UTF-8" {
        Ok(())
    } else {
        Err(format!(
            "the database encoding is {encoding}; ivmlite v0 supports UTF-8 databases only"
        ))
    }
}
