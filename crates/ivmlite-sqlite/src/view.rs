//! A view's life cycle on one connection (Phase 3a spec §5): create with
//! bootstrap, reconnect, refresh, destroy. Everything here is ordinary SQL on
//! the connection the virtual table was called on; `vtab.rs` only adapts it to
//! SQLite's callbacks.

use std::rc::Rc;

use ivmlite_core::{ArrangementId, MemArrangement, Node, Plan, Row, Schema, Value, ZSet};
use ivmlite_sql::{compile, Catalog, CompiledView};
use rusqlite::types::ValueRef;
use rusqlite::{params, Connection, OptionalExtension};

use crate::catalog::{require_utf8, CaptureInfo, SqliteCatalog};
use crate::names::{
    apply_trigger, delta_table, has_reserved_prefix, literal, main_qualified, out_index, out_table,
    pend_table, quote, stage_table, state_table, trigger, CAPTURE_EVENTS, DELTA_SEQ, DELTA_W, DEPS,
    META, PREFIX, PROBE, PROBE_STEP, PROGRESS, TRACKED, VIEWS,
};
use crate::state::{BufferedArrangement, Pending};

/// The shadow-table layout's version, stored in `__ivm_meta` and with each view.
///
/// Format 2 (Phase 3b spec §3) moves a base table's shape from `__ivm_dep`
/// into the new `__ivm_tracked` table, one row per table rather than one per
/// (view, table) pair: several views can now share a table's capture, and the
/// shape is a property of the table's triggers, not of any one view that
/// reads them.
///
/// Format 3 (Phase 4 spec §4) adds the output table's index, `out_index`: a
/// database built in format 2 has no such index, and no release ever wrote
/// format 2, so it is refused rather than migrated.
///
/// Format 4 (Phase 5 spec §4) drops the stage table's `armed` column: its
/// apply trigger fires once, when the arming `UPDATE` turns the stage's
/// sentinel row into `apply`, and applies every staged row set-based. A format-3 database's trigger is the
/// per-row one, so it is refused; since this is an alpha, it is not migrated.
pub const FORMAT: i64 = 4;

/// The column of the view's output table that holds each row's weight.
const WEIGHT: &str = "__w";

/// Names SQLite reserves for a table's rowid (spec §7): a result column with
/// one of these would shadow the alias `__ivm_out_<view>`'s own rowid needs —
/// the apply trigger's `DELETE … WHERE rowid IN …` and the cursor's
/// `SELECT rowid, …` both rely on `rowid` naming the real row id, not a
/// same-named result column (reproduced: `SELECT k AS rowid, SUM(x) FROM t
/// GROUP BY k` left the output empty after an UPDATE).
const ROWID_ALIASES: [&str; 3] = ["rowid", "oid", "_rowid_"];

type Result<T> = std::result::Result<T, String>;

fn sql_error(e: rusqlite::Error) -> String {
    e.to_string()
}

fn exec(conn: &Connection, sql: &str) -> Result<()> {
    conn.execute_batch(sql).map_err(sql_error)
}

/// Compile `sql` against the database's catalog, refusing a non-UTF-8 database.
fn compile_view(conn: &Connection, sql: &str) -> Result<CompiledView> {
    require_utf8(conn)?;
    compile(sql, &SqliteCatalog { conn }).map_err(|e| e.0)
}

/// Every arrangement the plan's operators ask for, in build order.
fn arrangement_ids(plan: &Plan) -> Vec<ArrangementId> {
    let mut ids = Vec::new();
    Node::build(plan, &mut |id| {
        ids.push(id);
        Ok(Box::new(MemArrangement::new()))
    })
    .expect("an in-memory provider cannot fail");
    ids
}

/// Whether the main schema holds an object of `kind` (`table`, `index`,
/// `trigger`) named exactly `name`.
fn object_exists(conn: &Connection, kind: &str, name: &str) -> Result<bool> {
    conn.query_row(
        "SELECT 1 FROM \"main\".sqlite_schema WHERE type = ?1 AND name = ?2",
        [kind, name],
        |_| Ok(()),
    )
    .optional()
    .map(|found| found.is_some())
    .map_err(sql_error)
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    object_exists(conn, "table", name)
}

/// `trigger` must exist and be on `table`. Its name alone proves nothing:
/// `ALTER TABLE t RENAME TO u` carries `t`'s triggers to `u` under their
/// old names.
fn check_trigger(conn: &Connection, trigger: &str, table: &str) -> Result<()> {
    let on: Option<String> = conn
        .query_row(
            "SELECT tbl_name FROM \"main\".sqlite_schema WHERE type = 'trigger' AND name = ?1",
            [trigger],
            |r| r.get(0),
        )
        .optional()
        .map_err(sql_error)?;
    match on {
        None => Err(format!("{trigger} is missing")),
        Some(on) if !on.eq_ignore_ascii_case(table) => {
            Err(format!("{trigger} is on table {on}, not {table}"))
        }
        Some(_) => Ok(()),
    }
}

/// What a view's last passing catalog check saw (Phase 5 spec §3): the
/// schema cookie, and its base tables' schemas in `view.tables` order.
pub struct Checked {
    pub schema_version: i64,
    pub schemas: Vec<Schema>,
}

/// `main`'s schema cookie (Phase 5 spec §3), which SQLite increments on every
/// schema change any connection makes to the database. Reading it, unlike
/// setting it, expires no prepared statement.
pub fn schema_version(conn: &Connection) -> Result<i64> {
    conn.query_row("PRAGMA \"main\".schema_version", [], |r| r.get(0))
        .map_err(sql_error)
}

fn base_schema(conn: &Connection, table: &str) -> Result<Schema> {
    SqliteCatalog { conn }
        .table(table)
        .map_err(|e| e.0)?
        .ok_or_else(|| format!("no such table: {table}"))
}

fn column_list(schema: &Schema, prefix: &str) -> String {
    schema
        .columns
        .iter()
        .map(|c| format!("{prefix}{}", quote(&c.name)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn sql_type(column: &ivmlite_core::Column) -> &'static str {
    match column.ty {
        ivmlite_core::ColumnType::Integer => "INTEGER",
        ivmlite_core::ColumnType::Text => "TEXT",
    }
}

/// Everything the capture triggers depend on (Phase 3b spec §3): the
/// columns' names and types in order, whether the table is WITHOUT ROWID,
/// and every unique index — its key columns with collation, `NOT NULL` and
/// default, and its partial and expression flags — in a canonical order,
/// so an index's name never matters. Stored in `__ivm_tracked` when the
/// table is first tracked and compared on every later create over it, every
/// connect and every refresh that runs the catalog checks (Phase 5 spec §3:
/// those after a schema change). A table dropped and recreated with other
/// column types can compile to the same plan, since the plan names columns
/// by position only.
fn shape(schema: &Schema, capture: &CaptureInfo) -> String {
    let columns: Vec<String> = schema
        .columns
        .iter()
        .map(|c| format!("{} {}", quote(&c.name), sql_type(c)))
        .collect();
    let mut keys: Vec<String> = capture
        .unique_keys
        .iter()
        .map(|k| {
            let cols: Vec<String> = k
                .columns
                .iter()
                .map(|c| {
                    let mut text = format!("{} {}", quote(&c.name), c.collation);
                    if c.not_null {
                        text.push_str(" NOT NULL");
                    }
                    if let Some(d) = &c.default {
                        text.push_str(&format!(" DEFAULT {d}"));
                    }
                    text
                })
                .collect();
            format!(
                "({}){}{}",
                cols.join(", "),
                if k.partial { " partial" } else { "" },
                if k.expression { " expression" } else { "" }
            )
        })
        .collect();
    keys.sort();
    format!(
        "{}; without rowid: {}; unique: [{}]",
        columns.join(", "),
        capture.without_rowid,
        keys.join("; ")
    )
}

/// The `CREATE TABLE` statement SQLite is given for the virtual table: the
/// output columns, then a hidden column named after the view — the command
/// channel of `INSERT INTO v(v) VALUES('refresh')` (the FTS5 idiom, spec §8.3).
pub fn declaration(name: &str, view: &CompiledView) -> Result<String> {
    if view
        .columns
        .iter()
        .any(|c| c.name.eq_ignore_ascii_case(name))
    {
        return Err(format!(
            "a result column is named {name}, like the view; the view's name is its command column"
        ));
    }
    if let Some(c) = view.columns.iter().find(|c| {
        ROWID_ALIASES
            .iter()
            .any(|reserved| c.name.eq_ignore_ascii_case(reserved))
    }) {
        return Err(format!(
            "a result column is named {}, which SQLite reserves for the rowid",
            c.name
        ));
    }
    let columns: Vec<String> = view
        .columns
        .iter()
        .map(|c| format!("{} {}", quote(&c.name), sql_type(c)))
        .collect();
    Ok(format!(
        "CREATE TABLE x({}, {} HIDDEN)",
        columns.join(", "),
        quote(name)
    ))
}

fn create_global_tables(conn: &Connection) -> Result<()> {
    let meta = main_qualified(META);
    exec(
        conn,
        &format!(
            "CREATE TABLE IF NOT EXISTS {meta}(key TEXT PRIMARY KEY, value);
             CREATE TABLE IF NOT EXISTS {}(name TEXT PRIMARY KEY, sql TEXT NOT NULL,
                 plan TEXT NOT NULL, declaration TEXT NOT NULL, format INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS {}(tbl TEXT PRIMARY KEY, shape TEXT NOT NULL,
                 broken TEXT);
             CREATE TABLE IF NOT EXISTS {}(view TEXT NOT NULL, tbl TEXT NOT NULL,
                 PRIMARY KEY(view, tbl));
             CREATE TABLE IF NOT EXISTS {}(view TEXT NOT NULL, tbl TEXT NOT NULL,
                 applied_seq INTEGER NOT NULL, PRIMARY KEY(view, tbl));
             INSERT OR IGNORE INTO {meta}(key, value) VALUES ('format', {FORMAT});",
            main_qualified(VIEWS),
            main_qualified(TRACKED),
            main_qualified(DEPS),
            main_qualified(PROGRESS),
        ),
    )?;
    // The recursive-triggers probe (spec §6.2): `PROBE_STEP` re-inserts into
    // `PROBE` while `n < 2`. With `recursive_triggers` OFF it does not fire
    // for its own insert, so inserting a 0 leaves 2 rows; with it ON, 3. As
    // in `track`, the `ON` table and the body's table stay unqualified and
    // the trigger's own name binds it to `main`.
    let probe_body = quote(PROBE);
    exec(
        conn,
        &format!(
            "CREATE TABLE IF NOT EXISTS {}(n INTEGER NOT NULL);
             CREATE TRIGGER IF NOT EXISTS {} AFTER INSERT ON {probe_body} WHEN NEW.n < 2
             BEGIN INSERT INTO {probe_body}(n) VALUES (NEW.n + 1); END;",
            main_qualified(PROBE),
            main_qualified(PROBE_STEP),
        ),
    )?;
    let format: i64 = conn
        .query_row(
            &format!("SELECT value FROM {meta} WHERE key = 'format'"),
            [],
            |r| r.get(0),
        )
        .map_err(sql_error)?;
    if format != FORMAT {
        return Err(format!(
            "the database's ivmlite state has format {format}; this extension reads format {FORMAT}"
        ));
    }
    Ok(())
}

/// How the capture triggers name a row of `t` (spec §6.2): its rowid, or
/// for a WITHOUT ROWID table its primary-key columns.
fn identity(capture: &CaptureInfo) -> Option<Vec<String>> {
    capture.without_rowid.then(|| {
        capture
            .unique_keys
            .iter()
            .find(|k| k.primary)
            .expect("a WITHOUT ROWID table has a primary key")
            .columns
            .iter()
            .map(|c| quote(&c.name))
            .collect()
    })
}

/// `a = b AND …` over the identity: `rowid`, or the primary-key columns.
fn same_row(pk: &Option<Vec<String>>, left: &str, right: &str) -> String {
    match pk {
        None => format!("{left}rowid = {right}rowid"),
        Some(cols) => cols
            .iter()
            .map(|c| format!("{left}{c} = {right}{c}"))
            .collect::<Vec<_>>()
            .join(" AND "),
    }
}

/// The alias every generated subquery gives the base table, and the one the
/// confirmation gives the pend table. The base table's own name cannot be
/// used: a table named `p`, `old` or `new` (in any case) would capture
/// `p.`, `OLD.` or `NEW.` references meant for the pend table or the
/// trigger's pseudo-rows, and REPLACE deletions would silently go
/// uncaptured (task review, measured). The catalog refuses base tables with
/// the reserved `__ivm_` prefix, so these aliases cannot collide.
const BASE_ALIAS: &str = "__ivm_b";
const PEND_ALIAS: &str = "__ivm_p";

/// The existing rows a new row could replace (spec §6.2), one `UNION`
/// branch per unique index plus the rowid, each able to use its own index.
/// `exclude_old`: in BEFORE UPDATE, never the row being updated.
fn candidates(schema: &Schema, capture: &CaptureInfo, exclude_old: bool) -> String {
    let base = quote(&schema.table);
    let b = format!("{BASE_ALIAS}.");
    let cols = column_list(schema, &b);
    let pk = identity(capture);
    let rid = if pk.is_some() {
        "NULL".to_string()
    } else {
        format!("{b}rowid")
    };
    let mut branches = Vec::new();
    if pk.is_none() {
        branches.push(format!("{b}rowid = NEW.rowid"));
    }
    for key in &capture.unique_keys {
        let terms: Vec<String> = key
            .columns
            .iter()
            .map(|c| {
                let q = quote(&c.name);
                // REPLACE substitutes a NOT NULL key's default for a NULL
                // (spec §2). `d` is a literal: the catalog refuses any other
                // default on such a key (spec §6.1), so it is safe to splice.
                match (&c.default, c.not_null) {
                    (Some(d), true) if !d.eq_ignore_ascii_case("NULL") => {
                        format!("{b}{q} = COALESCE(NEW.{q}, {d})")
                    }
                    _ => format!("{b}{q} = NEW.{q}"),
                }
            })
            .collect();
        branches.push(terms.join(" AND "));
    }
    let exclude = match (&pk, exclude_old) {
        (_, false) => String::new(),
        (None, true) => format!(" AND {b}rowid <> OLD.rowid"),
        (Some(_), true) => format!(" AND NOT ({})", same_row(&pk, &b, "OLD.")),
    };
    branches
        .iter()
        .map(|branch| {
            format!("SELECT {rid}, {cols} FROM {base} AS {BASE_ALIAS} WHERE ({branch}){exclude}")
        })
        .collect::<Vec<_>>()
        .join(" UNION ")
}

/// The `sqlite_schema` rows that concern `table`, as a `WHERE` condition:
/// the table itself, its indexes and the triggers on it. The latch scans
/// exactly these, once (Phase 4 spec §5).
fn rows_of(table: &str) -> String {
    format!("tbl_name = {} COLLATE NOCASE", literal(table))
}

/// Among `rows_of(table)`, `table`'s own row.
fn is_table_row(table: &str) -> String {
    format!(
        "type = 'table' AND name = {} COLLATE NOCASE",
        literal(table)
    )
}

/// Among `rows_of(table)`, an explicit unique index. SQLite stores every
/// such statement with the prefix normalized to `CREATE UNIQUE INDEX `
/// (measured: `create  unique index if not exists` and a leading comment
/// are both stored that way), so the `LIKE` needs no more than that prefix.
/// Autoindexes have no `sql` and are left out: they change only when the
/// table is rebuilt, which drops the triggers too.
const IS_UNIQUE_INDEX: &str = "type = 'index' AND sql LIKE 'CREATE UNIQUE INDEX%'";

/// `table`'s own row of `main`'s `sqlite_schema`, as a `FROM … WHERE`
/// clause, built from the predicates the latch counts with.
fn table_row(table: &str) -> String {
    format!(
        "FROM \"main\".sqlite_schema WHERE {} AND {}",
        rows_of(table),
        is_table_row(table)
    )
}

/// The rows of `table`'s explicit unique indexes in `main`'s
/// `sqlite_schema`, as a `FROM … WHERE` clause, built from the predicates
/// the latch counts with.
fn unique_index_rows(table: &str) -> String {
    format!(
        "FROM \"main\".sqlite_schema WHERE {} AND {IS_UNIQUE_INDEX}",
        rows_of(table)
    )
}

/// What `table`'s capture triggers were generated for (spec §6.2): its own
/// `CREATE TABLE` statement and its explicit unique indexes' statements, as
/// `sqlite_schema` held them when `table` was first tracked.
struct Fingerprint {
    table_sql: String,
    unique_indexes: Vec<String>,
}

impl Fingerprint {
    /// Read with the same `rows_of`, `is_table_row` and `IS_UNIQUE_INDEX`
    /// predicates the latch counts with, qualified to `main`.
    fn read(conn: &Connection, table: &str) -> Result<Fingerprint> {
        let table_sql = conn
            .query_row(&format!("SELECT sql {}", table_row(table)), [], |r| {
                r.get(0)
            })
            .map_err(sql_error)?;
        let unique_indexes = conn
            .prepare(&format!("SELECT sql {}", unique_index_rows(table)))
            .and_then(|mut s| s.query_map([], |r| r.get(0))?.collect())
            .map_err(sql_error)?;
        Ok(Fingerprint {
            table_sql,
            unique_indexes,
        })
    }

    /// A condition, for a capture trigger's body, that holds when `table`'s
    /// capture differs from this fingerprint (spec §6.2, Rulings 17 and 18).
    /// Each part compares without any order, so the trigger needs no
    /// aggregate `ORDER BY` (SQLite 3.44), which would make the database
    /// unreadable to an older SQLite, with or without the extension:
    ///
    /// - the table's own text covers every in-place change to its columns:
    ///   SQLite 3.53 rewrites it for `ALTER COLUMN ... SET/DROP NOT NULL`
    ///   (measured: `k INTEGER DEFAULT 5` becomes `k INTEGER DEFAULT 5 NOT
    ///   NULL` and back);
    /// - all five capture triggers sit on the table named `table`: the other
    ///   parts look `table` up by name, so while it is renamed away and a
    ///   decoy with its exact text stands in its place, the triggers sit on
    ///   the renamed table and are missing here. Trigger names are unique in
    ///   a schema, so no decoy can carry them;
    /// - the unique indexes are as many as recorded, and as many of them
    ///   have a recorded text. Index names are unique in a schema and every
    ///   text contains its name, so together these compare the sets. With
    ///   none recorded, the count alone covers it, so the text part is left
    ///   out.
    ///
    /// Every part concerns a row whose `tbl_name` is `table`, so they are
    /// one aggregate over a single scan of `sqlite_schema` (Phase 4 spec
    /// §5), not one subquery each: the latch runs for every written row.
    /// Each part counts with `count(CASE WHEN … THEN 1 END)`, which is 0 on
    /// empty input, never with `sum(…)`, which is NULL there: while `table`
    /// is renamed away the scan finds no row at all, and a NULL condition
    /// would silently not latch. `FILTER` and window functions stay out for
    /// the same old-SQLite reason as `ORDER BY`. The table part counts only
    /// the rows with the recorded text: a schema cannot hold two tables of
    /// the same name (in any case), so exactly one such row means `table`'s
    /// row exists and is unchanged. The `sqlite_schema` read is
    /// unqualified: a trigger in `main` reads `main`'s (measured, with an
    /// attached database holding a same-named table and unique index).
    fn changed(&self, table: &str) -> String {
        let table_row = is_table_row(table);
        let names: Vec<String> = CAPTURE_EVENTS
            .iter()
            .map(|event| literal(&trigger(table, event)))
            .collect();
        let recorded = self.unique_indexes.len();
        let mut parts = vec![
            format!(
                "count(CASE WHEN {table_row} AND sql IS {} THEN 1 END) <> 1",
                literal(&self.table_sql)
            ),
            format!(
                "count(CASE WHEN type = 'trigger' AND name IN ({}) THEN 1 END) <> {}",
                names.join(", "),
                CAPTURE_EVENTS.len()
            ),
            format!("count(CASE WHEN {IS_UNIQUE_INDEX} THEN 1 END) <> {recorded}"),
        ];
        if recorded > 0 {
            let texts: Vec<String> = self.unique_indexes.iter().map(|s| literal(s)).collect();
            parts.push(format!(
                "count(CASE WHEN {IS_UNIQUE_INDEX} AND sql IN ({}) THEN 1 END) <> {recorded}",
                texts.join(", ")
            ));
        }
        format!(
            "(SELECT {} FROM sqlite_schema WHERE {})",
            parts.join(" OR "),
            rows_of(table)
        )
    }
}

/// What `__ivm_tracked.broken` says once a capture trigger of `table` has
/// run against a definition, unique indexes or capture triggers other than
/// the ones it was generated with.
fn capture_changed(table: &str) -> String {
    format!(
        "the definition, unique indexes or capture triggers of {table} changed \
         after its capture was generated"
    )
}

/// The `CREATE TRIGGER` statements for every one of `CAPTURE_EVENTS`
/// (Phase 3a §8.1, Phase 3b §6.2). `fingerprint` is what the table held
/// when it was tracked.
fn capture_triggers(schema: &Schema, capture: &CaptureInfo, fingerprint: &Fingerprint) -> String {
    let t = &schema.table;
    // Measured: SQLite rejects a schema-qualified table name on an INSERT
    // inside a trigger body ("qualified table names are not allowed on
    // INSERT, UPDATE, and DELETE statements within triggers"), so the bodies
    // below reference the delta, pend and probe tables unqualified. This
    // still resolves to `main`, not a same-named TEMP table: a non-TEMP
    // trigger's body resolves an unqualified name in the schema the trigger
    // itself lives in, and the trigger's own name is qualified to `main`
    // below.
    let delta = quote(&delta_table(t));
    let pend = quote(&pend_table(t));
    let probe = quote(PROBE);
    // The ON clause of CREATE TRIGGER cannot be schema-qualified (SQL forbids
    // it), so this stays an unqualified reference; the trigger's own name
    // below is qualified to `main` instead, which SQLite requires to bind to
    // an `ON` table in that same schema — never a same-named TEMP table.
    let base = quote(t);
    let cols = column_list(schema, "");
    let new = column_list(schema, "NEW.");
    let old = column_list(schema, "OLD.");
    let p = format!("{PEND_ALIAS}.");
    let b = format!("{BASE_ALIAS}.");
    let p_cols = column_list(schema, &p);
    let pk = identity(capture);
    // Record the candidates, but only when the probe shows recursive
    // triggers OFF: with them ON, SQLite's own DELETE trigger captures every
    // row REPLACE removes, as in Phase 3a.
    //
    // Before anything else, latch a change to `t`'s definition, unique
    // indexes or capture triggers (spec §6.3): the candidate lookup below
    // was generated from the columns and indexes that existed when `t` was
    // tracked, so under any other definition or index set a REPLACE may
    // remove a row it never looks up (a NOT NULL set on a nullable unique
    // key with a default, for one), and the triggers must sit on the table
    // that was looked up. The shape check alone cannot see a change that
    // was made and undone between two refreshes; this sees every write, and
    // `broken` is never cleared.
    let latch = format!(
        "UPDATE {} SET broken = {} WHERE tbl = {} AND broken IS NULL AND ({});",
        quote(TRACKED),
        literal(&capture_changed(t)),
        literal(t),
        fingerprint.changed(t),
    );
    let fill = |exclude_old: bool| {
        let c = candidates(schema, capture, exclude_old);
        format!(
            "{latch}
                 DELETE FROM {pend};
                 INSERT INTO {probe}(n) SELECT 0 WHERE EXISTS ({c});
                 INSERT INTO {pend}(__ivm_rid, {cols}) SELECT * FROM ({c}) WHERE (SELECT count(*) FROM {probe}) = 2;
                 DELETE FROM {probe};"
        )
    };
    // A candidate was removed when it is gone from `t`, or when the new row
    // now holds its rowid (a rowid REPLACE, even with identical values).
    let gone = match &pk {
        None => format!(
            "{p}__ivm_rid = NEW.rowid OR NOT EXISTS \
             (SELECT 1 FROM {base} AS {BASE_ALIAS} WHERE {b}rowid = {p}__ivm_rid)"
        ),
        Some(_) => format!(
            "({}) OR NOT EXISTS (SELECT 1 FROM {base} AS {BASE_ALIAS} WHERE {})",
            same_row(&pk, &p, "NEW."),
            same_row(&pk, &b, &p)
        ),
    };
    let confirm = format!(
        "INSERT INTO {delta}({DELTA_W}, {cols}) SELECT -1, {p_cols} FROM {pend} AS {PEND_ALIAS} WHERE {gone};
                 DELETE FROM {pend};"
    );
    // A deleted row is never also counted as a confirmed candidate.
    let forget = match &pk {
        None => format!("DELETE FROM {pend} WHERE __ivm_rid = OLD.rowid;"),
        Some(_) => format!("DELETE FROM {pend} WHERE {};", same_row(&pk, "", "OLD.")),
    };
    let plus_new = format!("INSERT INTO {delta}({DELTA_W}, {cols}) VALUES (1, {new});");
    let minus_old = format!("INSERT INTO {delta}({DELTA_W}, {cols}) VALUES (-1, {old});");
    let mut sql = String::new();
    for event in CAPTURE_EVENTS {
        let (when, body) = match event {
            // Spec §6.2: record what this INSERT could replace.
            "preins" => ("BEFORE INSERT", fill(false)),
            // Spec §6.2: record what this UPDATE could replace, never the
            // row being updated itself.
            "preupd" => ("BEFORE UPDATE", fill(true)),
            // Spec §6.2: confirm the removed candidates, then the new row.
            "ins" => (
                "AFTER INSERT",
                format!("{confirm}\n                 {plus_new}"),
            ),
            // Spec §6.2 and §8.1: confirm the removed candidates, then the
            // update as a retraction of OLD plus an insertion of NEW.
            "upd" => (
                "AFTER UPDATE",
                format!("{confirm}\n                 {minus_old}\n                 {plus_new}"),
            ),
            // Spec §6.2: forget the row's candidate, then retract it.
            "del" => (
                "AFTER DELETE",
                format!("{forget}\n                 {minus_old}"),
            ),
            other => unreachable!("CAPTURE_EVENTS has no trigger body for {other}"),
        };
        sql.push_str(&format!(
            "\n             CREATE TRIGGER {} {when} ON {base} BEGIN\n                 {body}\n             END;",
            main_qualified(&trigger(t, event)),
        ));
    }
    sql
}

/// Start capturing `table`'s writes (Phase 3a §8.1, shared since Phase 3b
/// §4): its delta and pend tables, its `CAPTURE_EVENTS` triggers, and the
/// `__ivm_tracked` row that every later view of it, and every connect and
/// checking refresh, checks its capture against.
fn track(conn: &Connection, schema: &Schema) -> Result<()> {
    let t = &schema.table;
    let capture = SqliteCatalog { conn }.capture(t)?;
    let defs: Vec<String> = schema
        .columns
        .iter()
        .map(|c| format!("{} {}", quote(&c.name), sql_type(c)))
        .collect();
    let defs = defs.join(", ");
    // AUTOINCREMENT: once Phase 3b's GC deletes consumed deltas, a reused
    // `seq` would fall below a watermark. The delta table's own columns are
    // `DELTA_SEQ`/`DELTA_W` (`__ivm_seq`/`__ivm_w`), not `seq`/`w`, so a base
    // column named `seq` or `w` is not shadowed by them.
    let mut sql = format!(
        "CREATE TABLE {}({DELTA_SEQ} INTEGER PRIMARY KEY AUTOINCREMENT, \
         {DELTA_W} INTEGER NOT NULL, {defs});
         CREATE TABLE {}(__ivm_rid INTEGER, {defs});",
        main_qualified(&delta_table(t)),
        main_qualified(&pend_table(t)),
    );
    let fingerprint = Fingerprint::read(conn, t)?;
    sql.push_str(&capture_triggers(schema, &capture, &fingerprint));
    exec(conn, &sql)?;
    conn.execute(
        &format!(
            "INSERT INTO {}(tbl, shape) VALUES (?1, ?2)",
            main_qualified(TRACKED)
        ),
        params![t, shape(schema, &capture)],
    )
    .map_err(sql_error)?;
    Ok(())
}

/// Whether `table` already has a delta table and capture triggers, shared
/// with whatever view or views created them.
fn is_tracked(conn: &Connection, table: &str) -> Result<bool> {
    conn.query_row(
        &format!("SELECT 1 FROM {} WHERE tbl = ?1", main_qualified(TRACKED)),
        [table],
        |_| Ok(()),
    )
    .optional()
    .map(|found| found.is_some())
    .map_err(sql_error)
}

/// The views that read `table`, by name.
fn readers(conn: &Connection, table: &str) -> Result<Vec<String>> {
    conn.prepare(&format!(
        "SELECT view FROM {} WHERE tbl = ?1 ORDER BY view",
        main_qualified(DEPS)
    ))
    .and_then(|mut s| s.query_map([table], |r| r.get(0))?.collect())
    .map_err(sql_error)
}

/// The highest sequence number `table`'s delta table has ever handed out
/// (spec §4). Not `MAX(__ivm_seq)`: GC can empty the delta table, and
/// AUTOINCREMENT never reuses a number it handed out.
fn high_watermark(conn: &Connection, table: &str) -> Result<i64> {
    conn.query_row(
        "SELECT seq FROM \"main\".sqlite_sequence WHERE name = ?1",
        [delta_table(table)],
        |r| r.get(0),
    )
    .optional()
    .map(|seq| seq.unwrap_or(0))
    .map_err(sql_error)
}

/// Delete `table`'s delta rows that every reader has consumed (spec §5).
/// Used when a view is dropped while others still read `table`: the dropped
/// view may have been the slowest.
fn collect_garbage(conn: &Connection, table: &str) -> Result<()> {
    conn.execute(
        &format!(
            "DELETE FROM {} WHERE {DELTA_SEQ} <= (SELECT MIN(applied_seq) FROM {} WHERE tbl = ?1)",
            main_qualified(&delta_table(table)),
            main_qualified(PROGRESS)
        ),
        [table],
    )
    .map(|_| ())
    .map_err(sql_error)
}

/// `table` is tracked and no capture trigger has latched a change to its
/// definition, unique indexes or capture triggers; returns the shape its
/// triggers were generated from. The latch is set by a data write, never by
/// a schema change, so a refresh reads it even when it skips every other
/// check (Phase 5 spec §3).
fn check_latch(conn: &Connection, table: &str) -> Result<String> {
    let recorded: Option<(String, Option<String>)> = conn
        .query_row(
            &format!(
                "SELECT shape, broken FROM {} WHERE tbl = ?1",
                main_qualified(TRACKED)
            ),
            [table],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(sql_error)?;
    let (recorded, latched) = recorded.ok_or_else(|| format!("table {table} is not tracked"))?;
    match latched {
        Some(why) => Err(why),
        None => Ok(recorded),
    }
}

/// `table`'s capture as every view of it relies on: `check_latch`, the shape
/// its triggers were generated from, and every capture trigger on `table`
/// itself. Returns `table`'s schema as read for the shape.
fn check_table_capture(conn: &Connection, table: &str) -> Result<Schema> {
    let recorded = check_latch(conn, table)?;
    let capture = SqliteCatalog { conn }.capture(table)?;
    let schema = base_schema(conn, table)?;
    let now = shape(&schema, &capture);
    if now != recorded {
        return Err(format!(
            "base table {table} changed shape since it was first tracked \
             (was ({recorded}), now ({now}))"
        ));
    }
    for event in CAPTURE_EVENTS {
        check_trigger(conn, &trigger(table, event), table)
            .map_err(|why| format!("its capture trigger {why}"))?;
    }
    check_trigger(conn, PROBE_STEP, PROBE)
        .map_err(|why| format!("its recursive-triggers probe {why}"))?;
    // The delta table too, not only at connect (`verify`): a refresh on a
    // connection that is already connected would otherwise fail with a bare
    // "no such table" rather than as a broken view.
    for shadow in [delta_table(table), pend_table(table)] {
        if !table_exists(conn, &shadow)? {
            return Err(format!("its shadow table {shadow} is missing"));
        }
    }
    Ok(schema)
}

/// Stop capturing `table`: triggers first, so it stays writable.
fn untrack(conn: &Connection, table: &str) -> Result<()> {
    for event in CAPTURE_EVENTS {
        exec(
            conn,
            &format!(
                "DROP TRIGGER IF EXISTS {}",
                main_qualified(&trigger(table, event))
            ),
        )?;
    }
    for shadow in [delta_table(table), pend_table(table)] {
        exec(
            conn,
            &format!("DROP TABLE IF EXISTS {}", main_qualified(&shadow)),
        )?;
    }
    // A view whose `__ivm_tracked` the user dropped is broken, and must
    // still be droppable (Phase 3a §5).
    if table_exists(conn, TRACKED)? {
        conn.execute(
            &format!("DELETE FROM {} WHERE tbl = ?1", main_qualified(TRACKED)),
            [table],
        )
        .map_err(sql_error)?;
    }
    Ok(())
}

fn create_out_table(conn: &Connection, name: &str, view: &CompiledView) -> Result<()> {
    if view
        .columns
        .iter()
        .any(|c| c.name.eq_ignore_ascii_case(WEIGHT))
    {
        return Err(format!(
            "a result column is named {WEIGHT}, which ivmlite reserves"
        ));
    }
    let defs: Vec<String> = view
        .columns
        .iter()
        .map(|c| format!("{} {}", quote(&c.name), sql_type(c)))
        .collect();
    let cols: Vec<String> = view.columns.iter().map(|c| quote(&c.name)).collect();
    // Non-unique (spec §4): v0's root aggregate makes output rows distinct,
    // but the apply trigger's semantics are per-copy — one `out-` removes
    // one row (see `one_retraction_removes_one_copy_of_a_duplicated_output_row`).
    // A unique index would refuse the duplicate that white-box test creates,
    // or worse, silently collapse it. Dropped with the output table itself:
    // SQLite drops a table's indexes when the table is dropped.
    exec(
        conn,
        &format!(
            "CREATE TABLE {}({}, {WEIGHT} INTEGER NOT NULL);
             CREATE INDEX {} ON {}({});",
            main_qualified(&out_table(name)),
            defs.join(", "),
            main_qualified(&out_index(name)),
            quote(&out_table(name)),
            cols.join(", "),
        ),
    )
}

fn value_of(v: ValueRef<'_>) -> std::result::Result<Value, String> {
    match v {
        ValueRef::Null => Ok(Value::Null),
        ValueRef::Integer(n) => Ok(Value::Int(n)),
        ValueRef::Text(t) => std::str::from_utf8(t)
            .map(|s| Value::Text(s.to_string()))
            .map_err(|_| "a TEXT value is not UTF-8".to_string()),
        other => Err(format!(
            "a {:?} value in a column v0 declares INTEGER or TEXT",
            other.data_type()
        )),
    }
}

fn read_row(
    r: &rusqlite::Row<'_>,
    from: usize,
    n: usize,
) -> rusqlite::Result<std::result::Result<Row, String>> {
    let mut values = Vec::with_capacity(n);
    for i in from..from + n {
        match value_of(r.get_ref(i)?) {
            Ok(v) => values.push(v),
            Err(e) => return Ok(Err(e)),
        }
    }
    Ok(Ok(Row::new(values)))
}

/// Every row of a base table, as the first batch of deltas.
fn read_base(conn: &Connection, schema: &Schema) -> Result<ZSet> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {} FROM {}",
            column_list(schema, ""),
            main_qualified(&schema.table)
        ))
        .map_err(sql_error)?;
    let n = schema.columns.len();
    let rows = stmt
        .query_map([], |r| read_row(r, 0, n))
        .map_err(sql_error)?;
    let mut z = ZSet::new();
    for row in rows {
        z.update(row.map_err(sql_error)??, 1);
    }
    Ok(z)
}

/// The deltas of `table` after `after`, consolidated, and the highest `seq` read.
fn read_deltas(conn: &Connection, schema: &Schema, after: i64) -> Result<(ZSet, i64)> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {DELTA_SEQ}, {DELTA_W}, {} FROM {} WHERE {DELTA_SEQ} > ?1 ORDER BY {DELTA_SEQ}",
            column_list(schema, ""),
            main_qualified(&delta_table(&schema.table))
        ))
        .map_err(sql_error)?;
    let n = schema.columns.len();
    let rows = stmt
        .query_map([after], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, read_row(r, 2, n)?))
        })
        .map_err(sql_error)?;
    let mut z = ZSet::new();
    let mut last = after;
    for row in rows {
        let (seq, w, row) = row.map_err(sql_error)?;
        z.update(row?, w);
        last = seq;
    }
    Ok((z, last))
}

/// The lookup a retraction (`out-`) uses to find the one output row it
/// removes (Phase 4 spec §4): `SELECT rowid FROM <out> WHERE <c0> IS
/// <operand(0)> AND <c1> IS <operand(1)> AND …`. `cols` must be every output
/// column, quoted, in the output table's own order — the same order
/// `create_out_table` indexed them in — so SQLite can plan this as a SEARCH
/// on `out_index`, not a SCAN. `operand` gives each column's `IS`
/// comparison's right-hand side; the only caller is `create_stage`, which
/// passes `__ivm_s.cN`, the stage row under its reserved alias (Phase 5
/// spec §4), and uses the result, unchanged, in both the RAISE's existence
/// check and the retracting `DELETE`.
///
/// This crate cannot open a live `Connection` to check the resulting plan
/// itself — it builds `rusqlite` with only the `loadable_extension`
/// feature, never linked to a real SQLite (see its own Cargo.toml). Below,
/// `the_retraction_lookup_names_every_column_by_position` pins this
/// function's exact generated text; it does not check the plan.
/// `ivmlite-test`'s `the_retraction_lookup_searches_the_output_index`
/// reads the *stored apply trigger's own SQL* back out of `sqlite_schema`
/// (not a copy of this function's text) and checks that its plan is a
/// SEARCH covering every output column.
fn retraction_lookup(out: &str, cols: &[String], operand: impl Fn(usize) -> String) -> String {
    let terms: Vec<String> = cols
        .iter()
        .enumerate()
        .map(|(i, c)| format!("{c} IS {}", operand(i)))
        .collect();
    format!("SELECT rowid FROM {out} WHERE {}", terms.join(" AND "))
}

/// The stage table and the trigger that applies it (see `apply`). One stage
/// row is one change: `op` is `state` (a weight change of arrangement `arr`),
/// `out+` / `out-` (an output row, in `c0`, `c1`, …), `progress` (table
/// `tbl` consumed through `seq`), or the sentinel: staged first as `arm`, and
/// updated to `apply` by the arming statement, which fires the trigger
/// (Phase 5 spec §4, amendment 2026-10-08).
fn create_stage(
    conn: &Connection,
    name: &str,
    view: &CompiledView,
    ids: &[ArrangementId],
) -> Result<()> {
    // The trigger body names every table unqualified: SQLite rejects a
    // schema-qualified name on INSERT/UPDATE/DELETE inside a trigger, and a
    // trigger in `main` resolves its body's names in `main` (see
    // `create_delta_table`). Every statement outside the body is qualified.
    let stage = quote(&stage_table(name));
    let out = quote(&out_table(name));
    let progress = quote(PROGRESS);
    let n = view.columns.len();
    let defs: Vec<String> = view
        .columns
        .iter()
        .enumerate()
        .map(|(i, c)| format!("c{i} {}", sql_type(c)))
        .collect();
    let cols: Vec<String> = view.columns.iter().map(|c| quote(&c.name)).collect();
    // Phase 5 spec §4: the trigger fires once, when the arming `UPDATE` turns
    // the sentinel into `apply`, and each statement of its body applies every
    // staged row of one kind at once. It fires on `UPDATE OF op`, not on
    // INSERT, so no staging insert evaluates it (amendment 2026-10-08: an
    // INSERT trigger's `WHEN` cost each staging insert about as much as the
    // set-based body saved). The body filters on `op`, so the sentinel is
    // inert in it.
    // Wherever a subquery over the output table reads the stage's `cN`, the
    // stage carries the reserved alias `__ivm_s`: an unqualified `c0` there
    // would resolve to an output column named `c0` first.
    let s = "__ivm_s";
    let lookup = retraction_lookup(&out, &cols, |i| format!("{s}.c{i}"));
    let mut body = Vec::new();
    // The order is the row trigger's (Phase 3a §5): state, then output, then
    // watermarks, then GC, which reads the watermarks just written.
    for (i, id) in ids.iter().enumerate() {
        let t = quote(&state_table(name, *id));
        // One upsert per arrangement adds every staged weight change. SQLite
        // documents that an upsert over a SELECT needs a WHERE clause (even
        // `WHERE true`), or ON CONFLICT can parse as a join's ON; the filter
        // is that clause, and the trailing `AND true` (as spec §4 writes it)
        // changes nothing. A pending map holds each (key, val) once, so no
        // row is upserted twice.
        body.push(format!(
            "INSERT INTO {t}(key, val, w) SELECT key, val, w FROM {stage} \
             WHERE op = 'state' AND arr = {i} AND true \
             ON CONFLICT(key, val) DO UPDATE SET w = w + excluded.w;"
        ));
        // Then every staged key whose weight reached 0 is removed, after
        // the whole upsert: the final weight is the same either way.
        body.push(format!(
            "DELETE FROM {t} WHERE w = 0 AND (key, val) IN \
             (SELECT key, val FROM {stage} WHERE op = 'state' AND arr = {i});"
        ));
    }
    // Every retraction is checked before any is applied, so one with no
    // output row aborts the whole statement.
    body.push(format!(
        "SELECT RAISE(ABORT, 'ivmlite broken invariant: the view retracted a row its output table does not hold') \
         WHERE EXISTS (SELECT 1 FROM {stage} AS {s} WHERE {s}.op = 'out-' AND NOT EXISTS ({lookup}));"
    ));
    // Each `out-` row picks one output rowid. v0's output Z-set is
    // consolidated, so each output row is staged at most once, with weight
    // ±1: no two `out-` rows are identical, and each removes exactly one
    // copy. Two identical ones would pick the same rowid and remove one copy
    // between them; v0 cannot stage them (Phase 5 spec §4).
    body.push(format!(
        "DELETE FROM {out} WHERE rowid IN \
         (SELECT ({lookup} LIMIT 1) FROM {stage} AS {s} WHERE {s}.op = 'out-');"
    ));
    // After the retractions, so no `out-` can match a row inserted here;
    // the consolidated Z-set never stages one row as both `out+` and `out-`.
    body.push(format!(
        "INSERT INTO {out}({}, {WEIGHT}) SELECT {}, 1 FROM {stage} WHERE op = 'out+';",
        cols.join(", "),
        (0..n)
            .map(|i| format!("c{i}"))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    // Every staged watermark of this view at once; a view reads each base
    // table once (a self-join is refused), so each `tbl` is staged once.
    body.push(format!(
        "UPDATE {progress} SET applied_seq = \
         (SELECT seq FROM {stage} WHERE op = 'progress' AND tbl = {progress}.tbl) \
         WHERE view = {} AND tbl IN (SELECT tbl FROM {stage} WHERE op = 'progress');",
        literal(name)
    ));
    // Phase 3b spec §5: once this view's watermark for `t` moves, delete
    // every delta row of `t` that every reader has consumed — only when this
    // apply stages `t`'s watermark, inside the one arming statement.
    for t in &view.tables {
        body.push(format!(
            "DELETE FROM {delta} WHERE {DELTA_SEQ} <= (SELECT MIN(applied_seq) FROM {progress} WHERE tbl = {lit}) \
             AND EXISTS (SELECT 1 FROM {stage} WHERE op = 'progress' AND tbl = {lit});",
            delta = quote(&delta_table(t)),
            lit = literal(t),
        ));
    }
    exec(
        conn,
        &format!(
            "CREATE TABLE {}(op TEXT NOT NULL, arr INTEGER, key BLOB, val BLOB, w INTEGER,
                 tbl TEXT, seq INTEGER, {});
             CREATE TRIGGER {} AFTER UPDATE OF op ON {stage}
                 WHEN NEW.op = 'apply'
             BEGIN
                 {}
             END;",
            main_qualified(&stage_table(name)),
            defs.join(", "),
            main_qualified(&apply_trigger(name)),
            body.join("\n                 ")
        ),
    )
}

/// What one refresh (or the bootstrap) changes.
struct Changes {
    /// Each arrangement's pending weight changes, indexed like `ids`.
    state: Vec<Pending>,
    output: ZSet,
    /// `(table, seq)`: the table's deltas through `seq` are consumed.
    progress: Vec<(String, i64)>,
}

/// Push one batch per table through the view's operator tree. Its
/// arrangements read the state tables and buffer their writes.
fn compute(
    conn: &Rc<Connection>,
    name: &str,
    plan: &Plan,
    ids: &[ArrangementId],
    batches: &[(String, ZSet)],
) -> Result<(Vec<Pending>, ZSet)> {
    let pending: Vec<Pending> = ids.iter().map(|_| Pending::default()).collect();
    let mut tree = Node::build(plan, &mut |id| {
        let i = ids
            .iter()
            .position(|x| *x == id)
            .expect("the ids were collected from this same plan");
        Ok(Box::new(BufferedArrangement::new(
            conn.clone(),
            &state_table(name, id),
            pending[i].clone(),
        )))
    })
    .map_err(|e| e.0)?;
    let mut out = ZSet::new();
    for (table, delta) in batches {
        if !delta.is_empty() {
            out.merge(&tree.delta(table, delta).map_err(|e| e.0)?);
        }
    }
    Ok((pending, out))
}

fn sql_value(v: &Value) -> rusqlite::types::Value {
    match v {
        Value::Null => rusqlite::types::Value::Null,
        Value::Int(n) => rusqlite::types::Value::Integer(*n),
        Value::Text(s) => rusqlite::types::Value::Text(s.clone()),
    }
}

/// How many rows one staging `INSERT` carries (Phase 5 spec §6, amendments
/// 2026-10-08), unless `MAX_PARAMS` allows fewer.
const STAGE_CHUNK: usize = 64;

/// The most parameters one staging `INSERT` binds: `SQLITE_MAX_VARIABLE_NUMBER`
/// defaults to 999 before SQLite 3.32 (32766 since), and the extension may
/// run on an older SQLite.
const MAX_PARAMS: usize = 999;

/// The rows one staging `INSERT` carries for rows of `width` parameters:
/// `STAGE_CHUNK`, or fewer when that many would bind more than `MAX_PARAMS`.
/// The width is the view's column count plus one for an output row, so it is
/// derived here rather than assumed. A row wider than `MAX_PARAMS` still goes
/// one row per statement, as before chunking; only an SQLite whose limit is
/// higher (any default build since 3.32) accepts it.
fn rows_per_statement(width: usize) -> usize {
    (MAX_PARAMS / width).clamp(1, STAGE_CHUNK)
}

/// Stages rows of one shape with multi-row `INSERT … VALUES (…), (…), …`
/// statements, `chunk` rows at a time, then one remainder statement for the
/// rows left over (Phase 5 spec §6, amendments 2026-10-08). Each shape is a
/// `prepare_cached` statement. Rows are inserted in the order they are pushed,
/// each with exactly the values it would have had as a one-row insert.
struct Stager<'c> {
    conn: &'c Connection,
    /// `INSERT INTO <stage>(<columns>) VALUES `.
    head: String,
    /// One row's `(…)`; its anonymous `?` parameters are the row's width.
    row: String,
    width: usize,
    chunk: usize,
    /// The full chunk's SQL, built at the first full chunk.
    full: Option<String>,
    values: Vec<rusqlite::types::Value>,
}

impl<'c> Stager<'c> {
    fn new(conn: &'c Connection, head: String, row: String) -> Self {
        let width = row.matches('?').count();
        let chunk = rows_per_statement(width);
        Stager {
            conn,
            head,
            row,
            width,
            chunk,
            full: None,
            values: Vec::with_capacity(chunk * width),
        }
    }

    fn sql(&self, rows: usize) -> String {
        let mut sql = self.head.clone();
        for i in 0..rows {
            if i > 0 {
                sql.push_str(", ");
            }
            sql.push_str(&self.row);
        }
        sql
    }

    fn push(&mut self, row: impl IntoIterator<Item = rusqlite::types::Value>) -> Result<()> {
        let before = self.values.len();
        self.values.extend(row);
        debug_assert_eq!(self.values.len() - before, self.width);
        if self.values.len() == self.chunk * self.width {
            if self.full.is_none() {
                self.full = Some(self.sql(self.chunk));
            }
            let sql = self.full.as_deref().expect("filled just above");
            execute_staging(self.conn, sql, &mut self.values)?;
        }
        Ok(())
    }

    /// Stage the rows left over, fewer than one chunk, in one statement.
    fn finish(mut self) -> Result<()> {
        if self.values.is_empty() {
            return Ok(());
        }
        let sql = self.sql(self.values.len() / self.width);
        execute_staging(self.conn, &sql, &mut self.values)
    }
}

fn execute_staging(
    conn: &Connection,
    sql: &str,
    values: &mut Vec<rusqlite::types::Value>,
) -> Result<()> {
    conn.prepare_cached(sql)
        .and_then(|mut s| s.execute(rusqlite::params_from_iter(values.drain(..))))
        .map(|_| ())
        .map_err(sql_error)
}

/// Apply `changes` to the state tables, the output table and the watermarks
/// **in one statement**, so they change together or not at all.
///
/// A refresh runs inside `INSERT INTO v(v)`, where a `SAVEPOINT` is refused
/// and, inside an explicit transaction, a failed callback's own writes are not
/// rolled back (both measured, Phase 3a). One statement is atomic on its own:
/// the changes are first written to the stage table — harmless if that fails
/// part way, since the stage is emptied at the start of every apply — and then
/// a single `UPDATE` of the sentinel row, staged first as `arm`, to `apply`
/// fires the apply trigger once, whose set-based statements apply every
/// staged row (Phase 5 spec §4 and its amendment 2026-10-08). If any of them
/// fails, SQLite rolls that whole statement back. The stage keeps its rows,
/// the sentinel included, until the next apply empties it — except after the
/// bootstrap's own apply, which `create` empties itself at the end (spec §5),
/// so no view's stage is ever left full once `create` returns.
fn apply(conn: &Connection, name: &str, view: &CompiledView, changes: &Changes) -> Result<()> {
    use rusqlite::types::Value as SqlValue;
    let stage = main_qualified(&stage_table(name));
    exec(conn, &format!("DELETE FROM {stage}"))?;
    // The sentinel the arming statement turns into `apply` (Phase 5 spec §4,
    // amendment 2026-10-08), found again by its rowid, not by a scan. It
    // stays a one-row insert of its own, so `last_insert_rowid` is its rowid.
    exec(conn, &format!("INSERT INTO {stage}(op) VALUES ('arm')"))?;
    let sentinel = conn.last_insert_rowid();
    let mut state = Stager::new(
        conn,
        format!("INSERT INTO {stage}(op, arr, key, val, w) VALUES "),
        "('state', ?, ?, ?, ?)".to_string(),
    );
    for (i, pending) in changes.state.iter().enumerate() {
        for (key, vals) in pending.borrow().iter() {
            for (val, w) in vals {
                state.push([
                    SqlValue::Integer(i as i64),
                    SqlValue::Blob(crate::encode::encode(key)),
                    SqlValue::Blob(crate::encode::encode(val)),
                    SqlValue::Integer(*w),
                ])?;
            }
        }
    }
    state.finish()?;
    let n = view.columns.len();
    let cs: Vec<String> = (0..n).map(|i| format!("c{i}")).collect();
    let mut output = Stager::new(
        conn,
        format!("INSERT INTO {stage}(op, {}) VALUES ", cs.join(", ")),
        format!("({})", vec!["?"; n + 1].join(", ")),
    );
    for (row, &w) in changes.output.iter() {
        let op = match w {
            1 => "out+",
            -1 => "out-",
            w => {
                return Err(format!(
                    "broken invariant: output row {row:?} has weight {w}; a v0 view's rows have weight 1"
                ))
            }
        };
        output.push(
            std::iter::once(SqlValue::Text(op.to_string())).chain(row.0.iter().map(sql_value)),
        )?;
    }
    output.finish()?;
    for (table, seq) in &changes.progress {
        conn.execute(
            &format!("INSERT INTO {stage}(op, tbl, seq) VALUES ('progress', ?1, ?2)"),
            params![table, seq],
        )
        .map_err(sql_error)?;
    }
    // The one statement that changes durable state, and the last one: a
    // failure after it would report an error for changes that stay applied
    // (Phase 3a §5). The stage is left full here; a refresh's apply is
    // emptied by the next apply, while the bootstrap's is emptied right
    // after by `create`'s own cleanup DELETE (spec §5). Cached like the
    // staging inserts, though the cache lasts one callback: `vtab.rs` wraps
    // the handle in a new `Connection` for each.
    conn.prepare_cached(&format!("UPDATE {stage} SET op = 'apply' WHERE rowid = ?1"))
        .and_then(|mut s| s.execute(params![sentinel]))
        .map(|_| ())
        .map_err(sql_error)
}

/// `CREATE VIRTUAL TABLE <name> USING ivm('<sql>')`, inside the statement's
/// own transaction: create every shadow object, then bootstrap (spec §7.3).
/// Returns the view and what its catalog checks would now see (Phase 5 spec
/// §3): everything they inspect was just created or checked here.
pub fn create(conn: &Rc<Connection>, name: &str, sql: &str) -> Result<(CompiledView, Checked)> {
    if has_reserved_prefix(name) {
        return Err(format!(
            "the view name {name} starts with {PREFIX}, which ivmlite reserves for its own shadow tables"
        ));
    }
    let view = compile_view(conn, sql)?;
    // Computed first: a name clash fails before anything is created.
    let declared = declaration(name, &view)?;
    create_global_tables(conn)?;
    let schemas: Vec<Schema> = view
        .tables
        .iter()
        .map(|t| base_schema(conn, t))
        .collect::<Result<_>>()?;
    for schema in &schemas {
        let t = &schema.table;
        if is_tracked(conn, t)? {
            check_table_capture(conn, t).map_err(|why| {
                format!(
                    "table {t} is tracked but its capture is broken: {why}; drop the views \
                     that read it ({}) and create this view again",
                    readers(conn, t).unwrap_or_default().join(", ")
                )
            })?;
        } else {
            track(conn, schema)?;
        }
    }
    let ids = arrangement_ids(&view.plan);
    for id in &ids {
        exec(
            conn,
            &format!(
                "CREATE TABLE {}(key BLOB NOT NULL, val BLOB NOT NULL, w INTEGER NOT NULL,
                     PRIMARY KEY(key, val)) WITHOUT ROWID",
                main_qualified(&state_table(name, *id))
            ),
        )?;
    }
    create_out_table(conn, name, &view)?;
    create_stage(conn, name, &view, &ids)?;
    conn.execute(
        &format!(
            "INSERT INTO {}(name, sql, plan, declaration, format) VALUES (?1, ?2, ?3, ?4, ?5)",
            main_qualified(VIEWS)
        ),
        params![name, sql, view.plan.canonical(), declared, FORMAT],
    )
    .map_err(sql_error)?;
    for t in &view.tables {
        conn.execute(
            &format!(
                "INSERT INTO {}(view, tbl) VALUES (?1, ?2)",
                main_qualified(DEPS)
            ),
            params![name, t],
        )
        .map_err(sql_error)?;
    }

    // Bootstrap. `CREATE VIRTUAL TABLE` runs this whole function inside one
    // write transaction, so no other connection can write between the
    // watermark read below and the base-table scan here: together they are
    // exactly the state as of that watermark (spec §4), whether the table was
    // just tracked above (watermark 0) or was already tracked with deltas
    // some other view has not yet consumed (a positive watermark — replaying
    // those older rows would double-count them, since this scan already
    // includes them).
    let batches: Vec<(String, ZSet)> = schemas
        .iter()
        .map(|s| Ok((s.table.clone(), read_base(conn, s)?)))
        .collect::<Result<_>>()?;
    for schema in &schemas {
        conn.execute(
            &format!(
                "INSERT INTO {}(view, tbl, applied_seq) VALUES (?1, ?2, ?3)",
                main_qualified(PROGRESS)
            ),
            params![name, schema.table, high_watermark(conn, &schema.table)?],
        )
        .map_err(sql_error)?;
    }
    let (state, output) = compute(conn, name, &view.plan, &ids, &batches)?;
    apply(
        conn,
        name,
        &view,
        &Changes {
            state,
            output,
            progress: Vec::new(),
        },
    )?;
    // Spec §5: unlike a refresh, a failing CREATE VIRTUAL TABLE is rolled back
    // as a whole — it writes sqlite_schema — so emptying the stage after the
    // bootstrap's apply cannot leave an applied-but-reported-failed state
    // (Phase 3a §5). "Nothing after the apply can fail a refresh" protects a
    // *refresh* inside an explicit transaction, where a callback's writes are
    // not undone; a failing `CREATE VIRTUAL TABLE` is rolled back as a whole,
    // in autocommit and in an explicit transaction alike (Phase 3a §5,
    // measured), so this DELETE is safe to fail.
    exec(
        conn,
        &format!("DELETE FROM {}", main_qualified(&stage_table(name))),
    )?;
    // Read last, once this function's own DDL has moved the cookie. The
    // write transaction `CREATE VIRTUAL TABLE` runs in keeps every other
    // connection from moving it in between.
    let checked = Checked {
        schema_version: schema_version(conn)?,
        schemas,
    };
    Ok((view, checked))
}

/// A reopened view: the table declaration it was created with, either the
/// compiled view or why it can no longer be maintained, and, when it can, what
/// its passing catalog checks saw (Phase 5 spec §3).
pub struct Reopened {
    pub declaration: String,
    pub view: std::result::Result<CompiledView, String>,
    pub checked: Option<Checked>,
}

/// Reopen an existing view. Its stored SQL must compile to the same plan and
/// every shadow table it relies on must exist; if not, the view still opens —
/// with the stored declaration — so it can be dropped, and every read and
/// refresh reports why it is broken.
pub fn connect(conn: &Connection, name: &str) -> Result<Reopened> {
    let stored: Option<(String, String, String, i64)> = conn
        .query_row(
            &format!(
                "SELECT sql, plan, declaration, format FROM {} WHERE name = ?1",
                main_qualified(VIEWS)
            ),
            [name],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()
        .map_err(sql_error)?;
    let Some((sql, plan, declaration, format)) = stored else {
        return Err(format!("ivmlite has no record of the view {name}"));
    };
    let (view, checked) = match verify(conn, name, &sql, &plan, format) {
        Ok((view, checked)) => (Ok(view), Some(checked)),
        Err(why) => (Err(broken(name, &why)), None),
    };
    Ok(Reopened {
        declaration,
        view,
        checked,
    })
}

/// The error every read and refresh of a broken view reports (spec §5).
fn broken(name: &str, why: &str) -> String {
    format!("view {name} cannot be maintained: {why}; drop and recreate it")
}

fn verify(
    conn: &Connection,
    name: &str,
    sql: &str,
    plan: &str,
    format: i64,
) -> Result<(CompiledView, Checked)> {
    // Read before the checks (Phase 5 spec §3): a connect need not run inside
    // a transaction, so another connection may change the schema while they
    // run, and the cookie read first is then older than the one the next
    // refresh reads, which runs every check again.
    let version = schema_version(conn)?;
    if format != FORMAT {
        return Err(format!(
            "it was stored in format {format}, and this extension reads format {FORMAT}"
        ));
    }
    let view = compile_view(conn, sql)?;
    let now = view.plan.canonical();
    if now != plan {
        return Err(format!(
            "its SQL now compiles to a different plan than the one its state was built with \
             (stored {plan}, now {now})"
        ));
    }
    let mut needed: Vec<(String, &str)> = arrangement_ids(&view.plan)
        .into_iter()
        .map(|id| (state_table(name, id), "table"))
        .collect();
    needed.push((out_table(name), "table"));
    // Phase 4 spec §4: the output index too, so a view whose index the user
    // dropped is broken rather than silently falling back to a full scan.
    needed.push((out_index(name), "index"));
    needed.push((stage_table(name), "table"));
    needed.extend(view.tables.iter().map(|t| (delta_table(t), "table")));
    for (object, kind) in needed {
        if !object_exists(conn, kind, &object)? {
            return Err(format!("its shadow {kind} {object} is missing"));
        }
    }
    let schemas = check_capture(conn, name, &view)?;
    Ok((
        view,
        Checked {
            schema_version: version,
            schemas,
        },
    ))
}

/// The output table's index must exist too (Phase 4 spec §4), checked by
/// every refresh that runs the catalog checks (Phase 5 spec §3) —
/// `verify`'s own `needed` list above already checks it at every connect,
/// but `refresh` runs on the same connection a view was created on and never
/// calls `verify`. Without this, a user who drops
/// `__ivm_outidx_<view>` on the connection that already holds the view
/// would see a refresh silently fall back to a full table scan instead of a
/// broken view (Phase 4 spec §4: the index is what keeps a retraction a
/// SEARCH rather than a SCAN).
fn check_output_index(conn: &Connection, name: &str) -> Result<()> {
    let index = out_index(name);
    if !object_exists(conn, "index", &index)? {
        return Err(format!("its shadow index {index} is missing"));
    }
    Ok(())
}

/// Checked on every connect, and by every refresh after a schema change
/// (Phase 5 spec §3): each base table the view
/// reads is still tracked with its recorded shape and its capture triggers
/// (now `__ivm_tracked`'s concern, shared across every view of the table —
/// Phase 3b spec §3), and the view still has its apply trigger, each on the
/// table it was created on. `DROP TABLE t` drops `t`'s triggers but not its
/// delta table, so a recreated `t` would otherwise leave every later write
/// uncaptured; without the apply trigger, a refresh would apply nothing.
/// Either way the view would go stale with no error. Returns the base
/// tables' schemas, in `view.tables` order.
fn check_capture(conn: &Connection, name: &str, view: &CompiledView) -> Result<Vec<Schema>> {
    let mut schemas = Vec::with_capacity(view.tables.len());
    for table in &view.tables {
        let recorded: Option<i64> = conn
            .query_row(
                &format!(
                    "SELECT 1 FROM {} WHERE view = ?1 AND tbl = ?2",
                    main_qualified(DEPS)
                ),
                params![name, table],
                |r| r.get(0),
            )
            .optional()
            .map_err(sql_error)?;
        if recorded.is_none() {
            return Err(format!("its dependency on table {table} is not recorded"));
        }
        schemas.push(check_table_capture(conn, table)?);
    }
    check_trigger(conn, &apply_trigger(name), &stage_table(name))
        .map_err(|why| format!("its apply trigger {why}"))?;
    Ok(schemas)
}

/// The base tables' schemas a refresh reads deltas with, after the catalog
/// checks (Phase 5 spec §3). While `main`'s schema cookie still has the value
/// `checked` passed at, no object those checks inspect can have changed, so
/// only each table's latch is read: a data write sets it, and moves no
/// cookie. Otherwise every check runs, and `checked` holds the new cookie
/// and schemas only once they pass.
fn checked_schemas<'a>(
    conn: &Connection,
    name: &str,
    view: &CompiledView,
    checked: &'a mut Option<Checked>,
) -> Result<&'a [Schema]> {
    let version = schema_version(conn)?;
    // The hit arm returns nothing borrowed from `checked`, so the miss arm
    // may assign to it; the borrow is taken once both arms are done.
    match checked {
        Some(c) if c.schema_version == version => {
            for table in &view.tables {
                check_latch(conn, table).map_err(|why| broken(name, &why))?;
            }
        }
        _ => {
            *checked = None;
            let schemas = check_capture(conn, name, view).map_err(|why| broken(name, &why))?;
            check_output_index(conn, name).map_err(|why| broken(name, &why))?;
            *checked = Some(Checked {
                schema_version: version,
                schemas,
            });
        }
    }
    Ok(&checked
        .as_ref()
        .expect("a hit found the cache filled, and a miss filled it")
        .schemas)
}

/// `INSERT INTO v(v) VALUES('refresh')`: bring the view up to date. State,
/// output and watermarks change together or not at all (see `apply`).
/// `checked` is the view's per-connection cache of its last passing catalog
/// check (Phase 5 spec §3), updated here.
pub fn refresh(
    conn: &Rc<Connection>,
    name: &str,
    view: &CompiledView,
    checked: &mut Option<Checked>,
) -> Result<()> {
    let schemas = checked_schemas(conn, name, view, checked)?;
    let mut batches = Vec::new();
    let mut progress = Vec::new();
    for (table, schema) in view.tables.iter().zip(schemas) {
        let applied: i64 = conn
            .query_row(
                &format!(
                    "SELECT applied_seq FROM {} WHERE view = ?1 AND tbl = ?2",
                    main_qualified(PROGRESS)
                ),
                params![name, table],
                |r| r.get(0),
            )
            .map_err(sql_error)?;
        let (delta, last) = read_deltas(conn, schema, applied)?;
        batches.push((table.clone(), delta));
        if last > applied {
            progress.push((table.clone(), last));
        }
    }
    let ids = arrangement_ids(&view.plan);
    let (state, output) = compute(conn, name, &view.plan, &ids, &batches)?;
    apply(
        conn,
        name,
        view,
        &Changes {
            state,
            output,
            progress,
        },
    )
}

/// Whether `table` is one of `view`'s state tables: `__ivm_state_<view>_`
/// followed by exactly `<node>_<role>`. The suffix is checked exactly, so a
/// view named `v` never claims the tables of a view named `v_1`.
fn is_state_table_of(table: &str, view: &str) -> bool {
    let Some(rest) = table.strip_prefix(&format!("__ivm_state_{view}_")) else {
        return false;
    };
    let Some((node, role)) = rest.split_once('_') else {
        return false;
    };
    !node.is_empty()
        && node.bytes().all(|b| b.is_ascii_digit())
        && ["join_left", "join_right", "agg_groups"].contains(&role)
}

/// `DROP TABLE v`: the view's own shadow tables and metadata first, then —
/// for each base table it read — its capture too, but only once no other
/// view still reads it (Phase 3b spec §4; Task 2 adds the "readers remain"
/// branch's GC). It uses only what is recorded, not the compiled view, so a
/// view that can no longer be maintained can still be dropped.
pub fn destroy(conn: &Connection, name: &str) -> Result<()> {
    let tables: Vec<String> = conn
        .prepare(&format!(
            "SELECT tbl FROM {} WHERE view = ?1",
            main_qualified(DEPS)
        ))
        .and_then(|mut s| s.query_map([name], |r| r.get(0))?.collect())
        .map_err(sql_error)?;
    for table in [VIEWS, DEPS, PROGRESS] {
        let column = if table == VIEWS { "name" } else { "view" };
        conn.execute(
            &format!("DELETE FROM {} WHERE {column} = ?1", main_qualified(table)),
            [name],
        )
        .map_err(sql_error)?;
    }
    let state: Vec<String> = conn
        .prepare("SELECT name FROM \"main\".sqlite_schema WHERE type = 'table'")
        .and_then(|mut s| {
            s.query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(sql_error)?
        .into_iter()
        .filter(|t| is_state_table_of(t, name))
        .collect();
    for t in state {
        exec(conn, &format!("DROP TABLE {}", main_qualified(&t)))?;
    }
    exec(
        conn,
        &format!("DROP TABLE IF EXISTS {}", main_qualified(&out_table(name))),
    )?;
    exec(
        conn,
        &format!(
            "DROP TABLE IF EXISTS {}",
            main_qualified(&stage_table(name))
        ),
    )?;
    // This view's own `__ivm_dep` row was already deleted above, so `readers`
    // now reports only the views, if any, still reading `t` — never this one.
    // A delta table the user dropped leaves nothing to collect, and the
    // views that still read it are broken and must stay droppable.
    for t in &tables {
        if readers(conn, t)?.is_empty() {
            untrack(conn, t)?;
        } else if table_exists(conn, &delta_table(t))? {
            collect_garbage(conn, t)?;
        }
    }
    let left: i64 = conn
        .query_row(
            &format!("SELECT count(*) FROM {}", main_qualified(VIEWS)),
            [],
            |r| r.get(0),
        )
        .map_err(sql_error)?;
    if left == 0 {
        // `PROBE`'s trigger goes with it.
        // IF EXISTS: a view whose global table the user dropped (the probe,
        // say) is broken, and must still be droppable.
        for table in [META, VIEWS, TRACKED, DEPS, PROGRESS, PROBE] {
            exec(
                conn,
                &format!("DROP TABLE IF EXISTS {}", main_qualified(table)),
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ivmlite_core::{Column, ColumnType};

    /// A minimal `CompiledView` over one base table `t`, with the given
    /// output columns — enough to exercise `declaration`'s checks without a
    /// real SQL compile.
    fn view_with_columns(columns: Vec<&str>) -> CompiledView {
        CompiledView {
            plan: Plan::Scan {
                table: "t".into(),
                columns: vec![],
            },
            columns: columns
                .into_iter()
                .map(|name| Column {
                    name: name.into(),
                    ty: ColumnType::Integer,
                    nullable: false,
                })
                .collect(),
            tables: vec!["t".into()],
        }
    }

    #[test]
    fn declaration_rejects_a_rowid_alias_result_column_case_insensitively() {
        for reserved in ["rowid", "RowId", "OID", "_ROWID_"] {
            let view = view_with_columns(vec!["k", reserved]);
            let err = declaration("v", &view).expect_err(reserved);
            assert!(
                err.contains(reserved)
                    || err
                        .to_ascii_lowercase()
                        .contains(&reserved.to_ascii_lowercase()),
                "{reserved}: {err}"
            );
        }
    }

    #[test]
    fn declaration_accepts_an_ordinary_result_column_set() {
        let view = view_with_columns(vec!["k", "total"]);
        assert!(declaration("v", &view).is_ok());
    }

    #[test]
    fn a_views_state_tables_are_recognized_exactly() {
        assert!(is_state_table_of("__ivm_state_v_0_agg_groups", "v"));
        assert!(is_state_table_of("__ivm_state_v_12_join_right", "v"));
        // View `v_1`'s tables are not view `v`'s, and the other way round.
        assert!(!is_state_table_of("__ivm_state_v_1_0_agg_groups", "v"));
        assert!(!is_state_table_of("__ivm_state_v_0_agg_groups", "v_1"));
        assert!(is_state_table_of("__ivm_state_v_1_0_agg_groups", "v_1"));
        assert!(!is_state_table_of("__ivm_state_v_x_agg_groups", "v"));
        assert!(!is_state_table_of("__ivm_state_v__agg_groups", "v"));
        assert!(!is_state_table_of("__ivm_out_v", "v"));
    }

    #[test]
    fn a_staging_insert_binds_at_most_999_parameters() {
        assert_eq!(rows_per_statement(4), 64, "a state row");
        assert_eq!(rows_per_statement(15), 64, "64 rows of 15 bind 960");
        assert_eq!(rows_per_statement(16), 62, "64 rows of 16 would bind 1,024");
        assert_eq!(rows_per_statement(999), 1);
        assert_eq!(
            rows_per_statement(1000),
            1,
            "one row per statement, as before"
        );
    }

    // This crate builds `rusqlite` with only the `loadable_extension`
    // feature (see this crate's Cargo.toml): it is never linked to a real
    // SQLite, only loaded into one at runtime, so `Connection::open_in_memory`
    // panics with "SQLite API not initialized" here (confirmed: that is
    // exactly what happens if this test opens one). `EXPLAIN QUERY PLAN`
    // therefore cannot be run from this crate's own tests; this test only
    // pins `retraction_lookup`'s exact generated text. It does not, on its
    // own, prove anything about the real trigger's plan: `ivmlite-test`'s
    // `the_retraction_lookup_searches_the_output_index`, in
    // `extension_lifecycle.rs`, does that separately, by reading the apply
    // trigger's own stored SQL back out of `sqlite_schema` — not a copy of
    // this function's text — and running `EXPLAIN QUERY PLAN` on it.
    #[test]
    fn the_retraction_lookup_names_every_column_by_position() {
        let cols = vec![quote("k"), quote("s")];
        let lookup = retraction_lookup(&quote("__ivm_out_v"), &cols, |i| format!("?{}", i + 1));
        assert_eq!(
            lookup,
            "SELECT rowid FROM \"__ivm_out_v\" WHERE \"k\" IS ?1 AND \"s\" IS ?2"
        );
    }
}
