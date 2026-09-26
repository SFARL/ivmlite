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
        // pragma_table_list's `name` column compares case-sensitively by
        // default (measured: `FROM orders` failed to find a table declared
        // `Orders`, although SQLite itself accepts it); COLLATE NOCASE makes
        // this match names the way SQLite does. The row's own `name` column
        // still carries the declared spelling, which `declared` below keeps.
        let found: Option<(String, String, bool)> = self
            .conn
            .query_row(
                "SELECT name, type, strict FROM pragma_table_list \
                 WHERE schema = 'main' AND name = ?1 COLLATE NOCASE",
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
        let ddl = tokens(&sql);
        if declares_collate(&ddl) {
            return refuse(
                "declares a COLLATE clause; v0 supports only the BINARY collation (spec §7.1)",
            );
        }
        if declares_on_conflict_replace(&ddl) {
            return refuse(
                "declares ON CONFLICT REPLACE; SQLite fires no DELETE trigger for a row that \
                 REPLACE removes unless the writing connection has PRAGMA recursive_triggers ON, \
                 so ivmlite v0 would miss the removal and the view would silently diverge \
                 (Phase 3a spec §5)",
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
            if column.len() >= PREFIX.len() && column[..PREFIX.len()].eq_ignore_ascii_case(PREFIX) {
                return refuse(&format!(
                    "column {column} starts with {PREFIX}, which ivmlite reserves for its own shadow columns (e.g. the delta table's)"
                ));
            }
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

/// The tokens of a `CREATE TABLE` statement, in order: each keyword or bare
/// identifier upper-cased, each punctuation character on its own, and each
/// quoted string or identifier (`'…'`, `"…"`, `` `…` ``, `[…]`) as one empty
/// token, so a word inside quotes is never mistaken for a keyword. Whitespace
/// and comments (`-- …`, `/* … */`) only separate tokens.
///
/// The DDL checks below scan the whole statement, not a single column's
/// definition — the conservative scope of spec §7.1 — but match keywords as
/// whole tokens, so a column named `collateral` is not a COLLATE clause.
fn tokens(sql: &str) -> Vec<String> {
    let chars: Vec<char> = sql.chars().collect();
    let at = |i: usize| chars.get(i).copied();
    // SQLite's identifier characters: ASCII letters, digits, `_`, `$`, and
    // every non-ASCII character.
    let is_word = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '$' || !c.is_ascii();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(c) = at(i) {
        if c.is_ascii_whitespace() {
            i += 1;
        } else if c == '-' && at(i + 1) == Some('-') {
            while at(i).is_some_and(|c| c != '\n') {
                i += 1;
            }
        } else if c == '/' && at(i + 1) == Some('*') {
            i += 2;
            while at(i).is_some() && !(at(i) == Some('*') && at(i + 1) == Some('/')) {
                i += 1;
            }
            i += 2;
        } else if matches!(c, '\'' | '"' | '`' | '[') {
            let close = if c == '[' { ']' } else { c };
            i += 1;
            while let Some(q) = at(i) {
                i += 1;
                if q == close {
                    // A doubled quote is an escaped quote, not the end;
                    // `[…]` has no escape.
                    if close != ']' && at(i) == Some(close) {
                        i += 1;
                    } else {
                        break;
                    }
                }
            }
            out.push(String::new());
        } else if is_word(c) {
            let start = i;
            while at(i).is_some_and(is_word) {
                i += 1;
            }
            out.push(
                chars[start..i]
                    .iter()
                    .collect::<String>()
                    .to_ascii_uppercase(),
            );
        } else {
            out.push(c.to_string());
            i += 1;
        }
    }
    out
}

fn declares_collate(ddl: &[String]) -> bool {
    ddl.iter().any(|t| t == "COLLATE")
}

/// A column or table constraint's `ON CONFLICT REPLACE` (final review,
/// Critical 1): with it, every plain INSERT or UPDATE that conflicts silently
/// removes the old row.
fn declares_on_conflict_replace(ddl: &[String]) -> bool {
    ddl.windows(3)
        .any(|w| w[0] == "ON" && w[1] == "CONFLICT" && w[2] == "REPLACE")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collate_is_matched_as_a_token_not_a_substring() {
        assert!(declares_collate(&tokens(
            "CREATE TABLE t(k TEXT collate nocase)"
        )));
        assert!(declares_collate(&tokens(
            "CREATE TABLE t(k TEXT/**/COLLATE\tbinary)"
        )));
        assert!(!declares_collate(&tokens(
            "CREATE TABLE t(collateral TEXT, x_collate INTEGER)"
        )));
        // Inside quotes or a comment, COLLATE is not a clause.
        assert!(!declares_collate(&tokens(
            "CREATE TABLE t(\"collate\" TEXT DEFAULT 'COLLATE') -- COLLATE"
        )));
    }

    #[test]
    fn on_conflict_replace_is_matched_across_whitespace_and_comments() {
        assert!(declares_on_conflict_replace(&tokens(
            "CREATE TABLE t(k TEXT UNIQUE ON CONFLICT REPLACE)"
        )));
        assert!(declares_on_conflict_replace(&tokens(
            "CREATE TABLE t(k TEXT, UNIQUE(k) on /* x */ conflict -- y\n replace)"
        )));
        assert!(!declares_on_conflict_replace(&tokens(
            "CREATE TABLE t(k TEXT UNIQUE ON CONFLICT IGNORE, \"on conflict replace\" TEXT)"
        )));
        assert!(!declares_on_conflict_replace(&tokens(
            "CREATE TABLE t(k TEXT DEFAULT 'on conflict replace', [on conflict replace] TEXT)"
        )));
    }
}
