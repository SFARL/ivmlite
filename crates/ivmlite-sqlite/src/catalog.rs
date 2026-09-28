//! The SQLite implementation of `ivmlite_sql::Catalog` (spec §4.3, §7.1).

use ivmlite_core::{Column, ColumnType, Schema};
use ivmlite_sql::{Catalog, CatalogError};
use rusqlite::{params, Connection, OptionalExtension};

use crate::names::{has_reserved_prefix, literal, PREFIX};

/// One key column of a unique index, as `pragma_index_xinfo` and
/// `pragma_table_xinfo` report it.
pub struct KeyColumn {
    pub name: String,
    pub collation: String,
    pub not_null: bool,
    /// `pragma_table_xinfo.dflt_value`: the default's SQL text, if any.
    pub default: Option<String>,
    /// A generated column (`pragma_table_xinfo.hidden` 2 or 3), which
    /// `pragma_table_info` does not list at all.
    pub generated: bool,
}

/// A unique index: every `pragma_index_list` row with `unique = 1` (origin
/// `pk`, `u` or `c`).
pub struct UniqueKey {
    pub index: String,
    /// `origin = 'pk'`: for a `WITHOUT ROWID` table, this key's columns are
    /// the primary key, which the capture triggers name its rows by in place
    /// of a rowid (Phase 3b spec §6.2).
    pub primary: bool,
    pub partial: bool,
    /// Some key is an expression (`pragma_index_xinfo.cid = -2`).
    pub expression: bool,
    /// The key columns in index order; an expression key is omitted.
    pub columns: Vec<KeyColumn>,
}

/// What the capture triggers depend on beyond the columns (spec §3, §6).
pub struct CaptureInfo {
    pub without_rowid: bool,
    pub unique_keys: Vec<UniqueKey>,
}

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
        if has_reserved_prefix(&declared) {
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
                "SELECT sql FROM \"main\".sqlite_schema WHERE type = 'table' AND name = ?1",
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
        // Reserved names are checked over every column, generated ones
        // included: `pragma_table_info` omits a generated column, but inside
        // the capture triggers its name still shadows the rowid (or a shadow
        // column) — a generated `rowid` made a plain INSERT retract an
        // unrelated row (external review of bc0c891, reproduced).
        const ROWID_ALIASES: [&str; 3] = ["rowid", "oid", "_rowid_"];
        let every_column: Vec<String> = self
            .conn
            .prepare(&format!(
                "SELECT name FROM pragma_table_xinfo({}, 'main')",
                literal(&declared)
            ))
            .and_then(|mut s| s.query_map([], |r| r.get(0))?.collect())
            .map_err(err)?;
        for name in &every_column {
            if ROWID_ALIASES.iter().any(|a| name.eq_ignore_ascii_case(a)) {
                return refuse(&format!(
                    "column {name} is named like the rowid, which ivmlite's capture triggers address rows by (Phase 3b spec §6.1)"
                ));
            }
            if has_reserved_prefix(name) {
                return refuse(&format!(
                    "column {name} starts with {PREFIX}, which ivmlite reserves for its own shadow columns (e.g. the delta table's)"
                ));
            }
        }
        let mut stmt = self
            .conn
            .prepare(&format!(
                // The schema argument: without it, pragma_table_info reads a
                // same-named TEMP table instead (measured).
                "SELECT name, type, \"notnull\", pk FROM pragma_table_info({}, 'main')",
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
        let capture = self.capture(&declared).map_err(CatalogError)?;
        for key in &capture.unique_keys {
            let index = &key.index;
            if key.partial {
                return refuse(&format!("unique index {index} is partial; ivmlite v0 needs full unique indexes to capture REPLACE (Phase 3b spec §6.1)"));
            }
            if key.expression {
                return refuse(&format!("unique index {index} has an expression key; ivmlite cannot look up what REPLACE would remove (Phase 3b spec §6.1)"));
            }
            for column in &key.columns {
                if column.generated {
                    return refuse(&format!(
                        "unique index {index} has the generated column {} as a key; ivmlite does not support generated columns in unique keys",
                        column.name
                    ));
                }
                if !column.collation.eq_ignore_ascii_case("BINARY") {
                    return refuse(&format!(
                        "unique index {index} uses collation {} on column {}; v0 supports only BINARY (Phase 3b spec §6.1)",
                        column.collation, column.name
                    ));
                }
                if let (true, Some(default)) = (column.not_null, &column.default) {
                    if !is_literal_default(default) {
                        return refuse(&format!(
                            "column {} is NOT NULL in unique index {index} with default {default}, which is not a literal; \
                             REPLACE substitutes the default, and ivmlite cannot re-evaluate it (Phase 3b spec §6.1)",
                            column.name
                        ));
                    }
                }
            }
        }
        Ok(Some(Schema {
            table: declared,
            columns,
        }))
    }
}

impl SqliteCatalog<'_> {
    /// `table`'s capture metadata (Phase 3b spec §3, §6.1): whether it is
    /// `WITHOUT ROWID`, and every unique index with its key columns'
    /// collation, `NOT NULL` and default, and its partial/expression flags.
    pub fn capture(&self, table: &str) -> Result<CaptureInfo, String> {
        let err = |e: rusqlite::Error| format!("reading the catalog: {e}");
        let without_rowid: bool = self
            .conn
            .query_row(
                // COLLATE NOCASE, as in `table`: SQLite matches table names
                // case-insensitively.
                "SELECT wr FROM pragma_table_list \
                 WHERE schema = 'main' AND name = ?1 COLLATE NOCASE",
                [table],
                |r| r.get(0),
            )
            .map_err(err)?;
        let mut index_stmt = self
            .conn
            .prepare(
                "SELECT name, origin, partial FROM pragma_index_list(?1, 'main') \
                 WHERE \"unique\" = 1 ORDER BY name",
            )
            .map_err(err)?;
        let index_rows: Vec<(String, String, bool)> = index_stmt
            .query_map([table], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map_err(err)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(err)?;
        let mut unique_keys = Vec::with_capacity(index_rows.len());
        for (index, origin, partial) in index_rows {
            let mut xinfo_stmt = self
                .conn
                .prepare(
                    "SELECT cid, name, coll FROM pragma_index_xinfo(?1, 'main') \
                     WHERE key = 1 ORDER BY seqno",
                )
                .map_err(err)?;
            let xinfo: Vec<(i64, Option<String>, String)> = xinfo_stmt
                .query_map([&index], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .map_err(err)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(err)?;
            let mut expression = false;
            let mut columns = Vec::new();
            for (cid, name, collation) in xinfo {
                if cid == -2 {
                    expression = true;
                    continue;
                }
                let name = name.ok_or_else(|| {
                    format!("index {index}: a key column has no name (cid {cid})")
                })?;
                // pragma_table_xinfo, not pragma_table_info: the latter
                // leaves generated columns out, and a unique index over one
                // then failed with "Query returned no rows" (final review).
                let (not_null, default, hidden): (bool, Option<String>, i64) = self
                    .conn
                    .query_row(
                        "SELECT \"notnull\", dflt_value, hidden FROM pragma_table_xinfo(?1, 'main') \
                         WHERE name = ?2",
                        params![table, name],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .map_err(err)?;
                columns.push(KeyColumn {
                    name,
                    collation,
                    not_null,
                    default,
                    generated: matches!(hidden, 2 | 3),
                });
            }
            unique_keys.push(UniqueKey {
                index,
                primary: origin == "pk",
                partial,
                expression,
                columns,
            });
        }
        Ok(CaptureInfo {
            without_rowid,
            unique_keys,
        })
    }
}

/// A literal default, as `pragma_table_info.dflt_value` reports it (spec
/// §6.1): `NULL` in any case, an optionally signed decimal integer, a
/// hexadecimal integer (`0x…`), or a single-quoted string whose embedded
/// quotes are doubled. `DEFAULT (5)` is reported as `5` and is accepted;
/// anything else (`CURRENT_TIMESTAMP`, `random()`, an expression) is not,
/// since REPLACE substitutes the default for a NULL and ivmlite must be able
/// to re-evaluate it deterministically in the candidate lookup.
pub fn is_literal_default(text: &str) -> bool {
    if text.eq_ignore_ascii_case("NULL") {
        return true;
    }
    if let Some(inner) = text.strip_prefix('\'').and_then(|t| t.strip_suffix('\'')) {
        // Every quote inside must be a doubled one.
        return !inner.replace("''", "").contains('\'');
    }
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    if let Some(hex) = unsigned
        .strip_prefix("0x")
        .or_else(|| unsigned.strip_prefix("0X"))
    {
        return !hex.is_empty() && hex.bytes().all(|b| b.is_ascii_hexdigit());
    }
    !unsigned.is_empty() && unsigned.bytes().all(|b| b.is_ascii_digit())
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
    fn literal_defaults_are_numbers_strings_and_null() {
        for yes in [
            "NULL", "null", "5", "-5", "+3", "0x10", "-0X1f", "'d'", "'x''y'", "''",
        ] {
            assert!(is_literal_default(yes), "{yes}");
        }
        for no in [
            "CURRENT_TIMESTAMP",
            "random()",
            "5.0",
            "1e3",
            "'a' || 'b'",
            "'unterminated",
            "x",
            "0x",
            "-",
            "(5)",
        ] {
            assert!(!is_literal_default(no), "{no}");
        }
    }
}
