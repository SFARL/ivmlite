//! A view's life cycle on one connection (Phase 3a spec §5): create with
//! bootstrap, reconnect, refresh, destroy. Everything here is ordinary SQL on
//! the connection the virtual table was called on; `vtab.rs` only adapts it to
//! SQLite's callbacks.

use std::rc::Rc;

use ivmlite_core::{ArrangementId, MemArrangement, Node, Plan, Row, Schema, Value, ZSet};
use ivmlite_sql::{compile, Catalog, CompiledView};
use rusqlite::types::ValueRef;
use rusqlite::{params, Connection, OptionalExtension};

use crate::catalog::{require_utf8, SqliteCatalog};
use crate::names::{
    apply_trigger, delta_table, has_reserved_prefix, literal, main_qualified, out_table, quote,
    stage_table, state_table, trigger, DELTA_SEQ, DELTA_W, DEPS, META, PREFIX, PROGRESS, VIEWS,
};
use crate::state::{BufferedArrangement, Pending};

/// The shadow-table layout's version, stored in `__ivm_meta` and with each view.
pub const FORMAT: i64 = 1;

/// The column of the view's output table that holds each row's weight.
const WEIGHT: &str = "__w";

/// Names SQLite reserves for a table's rowid (spec §7): a result column with
/// one of these would shadow the alias `__ivm_out_<view>`'s own rowid needs —
/// the apply trigger's `DELETE … WHERE rowid = …` and the cursor's
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

/// Whether the main schema holds an object of `kind` (`table`, `trigger`)
/// named exactly `name`.
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

/// A base table's column shape — its columns' names and types, in order, as
/// the catalog reports them — stored in `__ivm_dep` at create and compared on
/// every connect and refresh. A table dropped and recreated with other
/// column types can compile to the same plan, since the plan names columns
/// by position only.
fn shape(schema: &Schema) -> String {
    schema
        .columns
        .iter()
        .map(|c| format!("{} {}", quote(&c.name), sql_type(c)))
        .collect::<Vec<_>>()
        .join(", ")
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
             CREATE TABLE IF NOT EXISTS {}(view TEXT NOT NULL, tbl TEXT NOT NULL,
                 shape TEXT NOT NULL, PRIMARY KEY(view, tbl));
             CREATE TABLE IF NOT EXISTS {}(view TEXT NOT NULL, tbl TEXT NOT NULL,
                 applied_seq INTEGER NOT NULL, PRIMARY KEY(view, tbl));
             INSERT OR IGNORE INTO {meta}(key, value) VALUES ('format', {FORMAT});",
            main_qualified(VIEWS),
            main_qualified(DEPS),
            main_qualified(PROGRESS),
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

fn create_delta_table(conn: &Connection, schema: &Schema) -> Result<()> {
    let t = &schema.table;
    let delta = main_qualified(&delta_table(t));
    // Measured: SQLite rejects a schema-qualified table name on an INSERT
    // inside a trigger body ("qualified table names are not allowed on
    // INSERT, UPDATE, and DELETE statements within triggers"), so the bodies
    // below reference the delta table unqualified. This still resolves to
    // `main`, not a same-named TEMP table: a non-TEMP trigger's body resolves
    // an unqualified name in the schema the trigger itself lives in, and the
    // trigger's own name is qualified to `main` below.
    let delta_body = quote(&delta_table(t));
    let defs: Vec<String> = schema
        .columns
        .iter()
        .map(|c| format!("{} {}", quote(&c.name), sql_type(c)))
        .collect();
    let cols = column_list(schema, "");
    let new = column_list(schema, "NEW.");
    let old = column_list(schema, "OLD.");
    // The ON clause of CREATE TRIGGER cannot be schema-qualified (SQL forbids
    // it), so this stays an unqualified reference; the trigger's own name
    // below is qualified to `main` instead, which SQLite requires to bind to
    // an `ON` table in that same schema — never a same-named TEMP table.
    let base = quote(t);
    // Spec §8.1: an UPDATE is a retraction of OLD plus an insertion of NEW, so
    // the delta table already holds a Z-set. AUTOINCREMENT: once Phase 3b
    // deletes consumed deltas, a reused `seq` would fall below a watermark.
    // The delta table's own columns are `DELTA_SEQ`/`DELTA_W`
    // (`__ivm_seq`/`__ivm_w`), not `seq`/`w`, so a base column named `seq` or
    // `w` is not shadowed by them.
    exec(
        conn,
        &format!(
            "CREATE TABLE {delta}({DELTA_SEQ} INTEGER PRIMARY KEY AUTOINCREMENT, {DELTA_W} INTEGER NOT NULL, {defs});
             CREATE TRIGGER {ins} AFTER INSERT ON {base} BEGIN
                 INSERT INTO {delta_body}({DELTA_W}, {cols}) VALUES (1, {new});
             END;
             CREATE TRIGGER {del} AFTER DELETE ON {base} BEGIN
                 INSERT INTO {delta_body}({DELTA_W}, {cols}) VALUES (-1, {old});
             END;
             CREATE TRIGGER {upd} AFTER UPDATE ON {base} BEGIN
                 INSERT INTO {delta_body}({DELTA_W}, {cols}) VALUES (-1, {old});
                 INSERT INTO {delta_body}({DELTA_W}, {cols}) VALUES (1, {new});
             END;",
            defs = defs.join(", "),
            ins = main_qualified(&trigger(t, "ins")),
            del = main_qualified(&trigger(t, "del")),
            upd = main_qualified(&trigger(t, "upd")),
        ),
    )
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
    exec(
        conn,
        &format!(
            "CREATE TABLE {}({}, {WEIGHT} INTEGER NOT NULL)",
            main_qualified(&out_table(name)),
            defs.join(", ")
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

/// The stage table and the trigger that applies it (see `apply`). One stage
/// row is one change: `op` is `state` (a weight change of arrangement `arr`),
/// `out+` / `out-` (an output row, in `c0`, `c1`, …), or `progress` (table
/// `tbl` consumed through `seq`).
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
    let new_cols: Vec<String> = (0..n).map(|i| format!("NEW.c{i}")).collect();
    let same: Vec<String> = cols
        .iter()
        .enumerate()
        .map(|(i, c)| format!("{c} IS NEW.c{i}"))
        .collect();
    let same = same.join(" AND ");
    let mut body = Vec::new();
    for (i, id) in ids.iter().enumerate() {
        let t = quote(&state_table(name, *id));
        body.push(format!(
            "INSERT INTO {t}(key, val, w) SELECT NEW.key, NEW.val, NEW.w \
             WHERE NEW.op = 'state' AND NEW.arr = {i} \
             ON CONFLICT(key, val) DO UPDATE SET w = w + excluded.w;"
        ));
        body.push(format!(
            "DELETE FROM {t} WHERE NEW.op = 'state' AND NEW.arr = {i} \
             AND key = NEW.key AND val = NEW.val AND w = 0;"
        ));
    }
    body.push(format!(
        "SELECT RAISE(ABORT, 'ivmlite broken invariant: the view retracted a row its output table does not hold') \
         WHERE NEW.op = 'out-' AND NOT EXISTS (SELECT 1 FROM {out} WHERE {same});"
    ));
    body.push(format!(
        "DELETE FROM {out} WHERE NEW.op = 'out-' \
         AND rowid = (SELECT rowid FROM {out} WHERE {same} LIMIT 1);"
    ));
    body.push(format!(
        "INSERT INTO {out}({}, {WEIGHT}) SELECT {}, 1 WHERE NEW.op = 'out+';",
        cols.join(", "),
        new_cols.join(", ")
    ));
    body.push(format!(
        "UPDATE {progress} SET applied_seq = NEW.seq \
         WHERE NEW.op = 'progress' AND view = {} AND tbl = NEW.tbl;",
        literal(name)
    ));
    exec(
        conn,
        &format!(
            "CREATE TABLE {}(op TEXT NOT NULL, arr INTEGER, key BLOB, val BLOB, w INTEGER,
                 tbl TEXT, seq INTEGER, {}, armed INTEGER NOT NULL DEFAULT 0);
             CREATE TRIGGER {} AFTER UPDATE OF armed ON {stage}
                 WHEN OLD.armed = 0 AND NEW.armed = 1
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

/// Apply `changes` to the state tables, the output table and the watermarks
/// **in one statement**, so they change together or not at all.
///
/// A refresh runs inside `INSERT INTO v(v)`, where a `SAVEPOINT` is refused
/// and, inside an explicit transaction, a failed callback's own writes are not
/// rolled back (both measured, Phase 3a). One statement is atomic on its own:
/// the changes are first written to the stage table — harmless if that fails
/// part way, since the stage is emptied at the start of every apply — and then
/// a single `UPDATE … SET armed = 1` fires the apply trigger for every row.
/// If any row fails, SQLite rolls that whole statement back.
fn apply(conn: &Connection, name: &str, view: &CompiledView, changes: &Changes) -> Result<()> {
    let stage = main_qualified(&stage_table(name));
    exec(conn, &format!("DELETE FROM {stage}"))?;
    let stage_state =
        format!("INSERT INTO {stage}(op, arr, key, val, w) VALUES ('state', ?1, ?2, ?3, ?4)");
    for (i, pending) in changes.state.iter().enumerate() {
        for (key, vals) in pending.borrow().iter() {
            for (val, w) in vals {
                conn.prepare_cached(&stage_state)
                    .and_then(|mut s| {
                        s.execute(params![
                            i as i64,
                            crate::encode::encode(key),
                            crate::encode::encode(val),
                            w
                        ])
                    })
                    .map_err(sql_error)?;
            }
        }
    }
    let n = view.columns.len();
    let placeholders: Vec<String> = (0..n).map(|i| format!("?{}", i + 2)).collect();
    let cs: Vec<String> = (0..n).map(|i| format!("c{i}")).collect();
    let stage_out = format!(
        "INSERT INTO {stage}(op, {}) VALUES (?1, {})",
        cs.join(", "),
        placeholders.join(", ")
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
        let mut values = vec![rusqlite::types::Value::Text(op.to_string())];
        values.extend(row.0.iter().map(sql_value));
        conn.prepare_cached(&stage_out)
            .and_then(|mut s| s.execute(rusqlite::params_from_iter(values.iter())))
            .map_err(sql_error)?;
    }
    for (table, seq) in &changes.progress {
        conn.execute(
            &format!("INSERT INTO {stage}(op, tbl, seq) VALUES ('progress', ?1, ?2)"),
            params![table, seq],
        )
        .map_err(sql_error)?;
    }
    // The one statement that changes durable state, and the last one: a
    // failure after it would report an error for changes that stay applied.
    // The stage is left full and emptied by the next apply.
    exec(conn, &format!("UPDATE {stage} SET armed = 1"))
}

/// `CREATE VIRTUAL TABLE <name> USING ivm('<sql>')`, inside the statement's
/// own transaction: create every shadow object, then bootstrap (spec §7.3).
pub fn create(conn: &Rc<Connection>, name: &str, sql: &str) -> Result<CompiledView> {
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
        if table_exists(conn, &delta_table(&schema.table))? {
            return Err(format!(
                "table {} is already tracked by another ivmlite view; Phase 3a supports one view per base table",
                schema.table
            ));
        }
    }
    for schema in &schemas {
        create_delta_table(conn, schema)?;
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
    for (t, schema) in view.tables.iter().zip(&schemas) {
        conn.execute(
            &format!(
                "INSERT INTO {}(view, tbl, shape) VALUES (?1, ?2, ?3)",
                main_qualified(DEPS)
            ),
            params![name, t, shape(schema)],
        )
        .map_err(sql_error)?;
    }

    // Bootstrap. The delta tables and triggers were created above, in this
    // same transaction: every write from now on is captured, and no write so
    // far is in a delta table, so the snapshot read here is exactly the state
    // at watermark 0 (spec §7.3).
    let batches: Vec<(String, ZSet)> = schemas
        .iter()
        .map(|s| Ok((s.table.clone(), read_base(conn, s)?)))
        .collect::<Result<_>>()?;
    for schema in &schemas {
        conn.execute(
            &format!(
                "INSERT INTO {}(view, tbl, applied_seq) VALUES (?1, ?2, 0)",
                main_qualified(PROGRESS)
            ),
            params![name, schema.table],
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
    Ok(view)
}

/// A reopened view: the table declaration it was created with, and either the
/// compiled view or why it can no longer be maintained.
pub struct Reopened {
    pub declaration: String,
    pub view: std::result::Result<CompiledView, String>,
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
    let view = verify(conn, name, &sql, &plan, format).map_err(|why| broken(name, &why));
    Ok(Reopened { declaration, view })
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
) -> Result<CompiledView> {
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
    let mut needed: Vec<String> = arrangement_ids(&view.plan)
        .into_iter()
        .map(|id| state_table(name, id))
        .collect();
    needed.push(out_table(name));
    needed.push(stage_table(name));
    needed.extend(view.tables.iter().map(|t| delta_table(t)));
    for table in needed {
        if !table_exists(conn, &table)? {
            return Err(format!("its shadow table {table} is missing"));
        }
    }
    check_capture(conn, name, &view)?;
    Ok(view)
}

/// Checked on every connect and every refresh: each base table still has the
/// column shape recorded at create and its three capture triggers, and the
/// view still has its apply trigger, each on the table it was created on. `DROP TABLE t` drops `t`'s triggers but
/// not its delta table, so a recreated `t` would otherwise leave every later
/// write uncaptured; without the apply trigger, a refresh would apply
/// nothing. Either way the view would go stale with no error.
fn check_capture(conn: &Connection, name: &str, view: &CompiledView) -> Result<()> {
    for table in &view.tables {
        let recorded: Option<String> = conn
            .query_row(
                &format!(
                    "SELECT shape FROM {} WHERE view = ?1 AND tbl = ?2",
                    main_qualified(DEPS)
                ),
                params![name, table],
                |r| r.get(0),
            )
            .optional()
            .map_err(sql_error)?;
        let recorded =
            recorded.ok_or_else(|| format!("its dependency on table {table} is not recorded"))?;
        let now = shape(&base_schema(conn, table)?);
        if now != recorded {
            return Err(format!(
                "base table {table} changed shape since the view was created \
                 (was ({recorded}), now ({now}))"
            ));
        }
        for event in ["ins", "del", "upd"] {
            check_trigger(conn, &trigger(table, event), table)
                .map_err(|why| format!("its capture trigger {why}"))?;
        }
    }
    check_trigger(conn, &apply_trigger(name), &stage_table(name))
        .map_err(|why| format!("its apply trigger {why}"))
}

/// `INSERT INTO v(v) VALUES('refresh')`: bring the view up to date. State,
/// output and watermarks change together or not at all (see `apply`).
pub fn refresh(conn: &Rc<Connection>, name: &str, view: &CompiledView) -> Result<()> {
    check_capture(conn, name, view).map_err(|why| broken(name, &why))?;
    let mut batches = Vec::new();
    let mut progress = Vec::new();
    for table in &view.tables {
        let schema = base_schema(conn, table)?;
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
        let (delta, last) = read_deltas(conn, &schema, applied)?;
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

/// `DROP TABLE v`: triggers first, so the base tables stay writable (M-1
/// scenario 9), then every shadow table and the view's metadata. It uses only
/// what is recorded, not the compiled view, so a view that can no longer be
/// maintained can still be dropped.
pub fn destroy(conn: &Connection, name: &str) -> Result<()> {
    let tables: Vec<String> = conn
        .prepare(&format!(
            "SELECT tbl FROM {} WHERE view = ?1",
            main_qualified(DEPS)
        ))
        .and_then(|mut s| s.query_map([name], |r| r.get(0))?.collect())
        .map_err(sql_error)?;
    for t in &tables {
        for event in ["ins", "del", "upd"] {
            exec(
                conn,
                &format!(
                    "DROP TRIGGER IF EXISTS {}",
                    main_qualified(&trigger(t, event))
                ),
            )?;
        }
        exec(
            conn,
            &format!("DROP TABLE IF EXISTS {}", main_qualified(&delta_table(t))),
        )?;
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
    for table in [VIEWS, DEPS, PROGRESS] {
        let column = if table == VIEWS { "name" } else { "view" };
        conn.execute(
            &format!("DELETE FROM {} WHERE {column} = ?1", main_qualified(table)),
            [name],
        )
        .map_err(sql_error)?;
    }
    let left: i64 = conn
        .query_row(
            &format!("SELECT count(*) FROM {}", main_qualified(VIEWS)),
            [],
            |r| r.get(0),
        )
        .map_err(sql_error)?;
    if left == 0 {
        for table in [META, VIEWS, DEPS, PROGRESS] {
            exec(conn, &format!("DROP TABLE {}", main_qualified(table)))?;
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
}
