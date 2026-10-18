//! Untimed, independent diagnostics for the two fixed M2a query shapes.
//!
//! The extension's format-6 codec is deliberately private. This reader pins its
//! documented layout and the exact persisted plan, then compares decoded rows
//! against SQL projections of the live bases. It shares no incremental code.

use std::collections::BTreeMap;

use rusqlite::{types::ValueRef, Connection};
use serde::{Deserialize, Serialize};

type Result<T> = std::result::Result<T, String>;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) enum Datum {
    Null,
    Int(i64),
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct StateEntry {
    pub key: Vec<Datum>,
    pub value: Vec<Datum>,
    pub weight: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) struct RetainedSignature {
    pub arrangements: BTreeMap<String, Vec<StateEntry>>,
    /// Physical output entries, including their stored multiplicity.
    pub output: Vec<(Vec<Datum>, i64)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ArrangementMetrics {
    pub table: String,
    pub entries: u64,
    pub weight_total: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PendingMetrics {
    pub base_table: String,
    pub delta_rows: u64,
    pub replace_candidate_rows: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct GroupCount {
    pub group: Option<i64>,
    pub rows: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PageMetrics {
    pub page_size: u64,
    pub page_count: u64,
    pub freelist_count: u64,
    pub allocated_bytes: u64,
    pub live_page_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Snapshot {
    pub base_rows: BTreeMap<String, u64>,
    /// Number of logical joined rows (or events for the single-table shape).
    pub joined_rows: u64,
    pub live_output_groups: u64,
    pub group_histogram: Vec<GroupCount>,
    pub arrangements: Vec<ArrangementMetrics>,
    pub aggregate_state_rows: u64,
    pub output_rows: u64,
    pub output_weight_total: i64,
    pub pending: Vec<PendingMetrics>,
    pub stage_rows: u64,
    /// Retained space of the stage table: its row count at this checkpoint,
    /// including the single `apply` sentinel. Format 5 empties the stage at the
    /// start of the next apply, so after a refresh this is the last applied
    /// batch (equal to `stage_rows`), and it is 0 after bootstrap. It is
    /// reported beside the other state and is never a pending delta.
    pub retained_stage_rows: u64,
    /// Refresh retains its last already-applied staging batch. Bootstrap is
    /// empty, and an idle refresh leaves this histogram unchanged.
    pub stage_operation_counts: BTreeMap<String, u64>,
    pub pages: PageMetrics,
    // Retain the full decoded multiset for equality checks, without repeating
    // tens of thousands of join rows in every exported summary checkpoint.
    #[serde(skip)]
    retained: RetainedSignature,
}

impl Snapshot {
    /// Excludes page allocation, sequence numbers and progress bookkeeping.
    /// Compare this after idle refreshes and after a complete inverse trace.
    pub fn retained_signature(&self) -> &RetainedSignature {
        &self.retained
    }

    /// Drop decoded rows after validation and before storing a checkpoint.
    /// Callers must keep a full snapshot for any pending idle/signature check.
    pub fn into_summary(mut self) -> Self {
        self.retained = RetainedSignature::default();
        self
    }
}

fn sql<T>(value: rusqlite::Result<T>) -> Result<T> {
    value.map_err(|error| format!("M2a state diagnostics: {error}"))
}

fn quoted(name: &str) -> String {
    format!("\"main\".\"{}\"", name.replace('"', "\"\""))
}

fn count(conn: &Connection, table: &str) -> Result<u64> {
    let count: i64 = sql(conn.query_row(
        &format!("SELECT count(*) FROM {}", quoted(table)),
        [],
        |r| r.get(0),
    ))?;
    u64::try_from(count).map_err(|_| "M2a negative table row count".into())
}

/// Also used for same-checkpoint base-only and covering-recompute databases.
pub(crate) fn capture_pages(conn: &Connection) -> Result<PageMetrics> {
    let pragma = |name: &str| {
        let value = sql(conn.query_row(&format!("PRAGMA main.{name}"), [], |row| {
            row.get::<_, i64>(0)
        }))?;
        u64::try_from(value).map_err(|_| format!("M2a negative {name}"))
    };
    let page_size = pragma("page_size")?;
    let page_count = pragma("page_count")?;
    let freelist_count = pragma("freelist_count")?;
    let allocated_bytes = page_size
        .checked_mul(page_count)
        .ok_or("M2a allocated-byte overflow")?;
    let live_page_bytes = page_count
        .checked_sub(freelist_count)
        .and_then(|pages| page_size.checked_mul(pages))
        .ok_or("M2a invalid page/freelist counts")?;
    Ok(PageMetrics {
        page_size,
        page_count,
        freelist_count,
        allocated_bytes,
        live_page_bytes,
    })
}

fn decode(mut bytes: &[u8]) -> Result<Vec<Datum>> {
    let mut row = Vec::new();
    while let Some((&tag, tail)) = bytes.split_first() {
        bytes = tail;
        row.push(match tag {
            0 => Datum::Null,
            1 => {
                let (value, tail) = bytes
                    .split_first_chunk::<8>()
                    .ok_or("M2a truncated INTEGER in persisted state")?;
                bytes = tail;
                Datum::Int(i64::from_be_bytes(*value))
            }
            2 => {
                let (length, tail) = bytes
                    .split_first_chunk::<4>()
                    .ok_or("M2a truncated TEXT length in persisted state")?;
                let length = u32::from_be_bytes(*length) as usize;
                if tail.len() < length {
                    return Err("M2a truncated TEXT in persisted state".into());
                }
                let (value, tail) = tail.split_at(length);
                bytes = tail;
                Datum::Text(
                    std::str::from_utf8(value)
                        .map_err(|_| "M2a non-UTF8 TEXT in persisted state")?
                        .to_owned(),
                )
            }
            _ => return Err(format!("M2a unknown persisted state tag {tag}")),
        });
    }
    Ok(row)
}

fn read_values(row: &rusqlite::Row<'_>, columns: usize) -> rusqlite::Result<Vec<Datum>> {
    (0..columns)
        .map(|index| {
            Ok(match row.get_ref(index)? {
                ValueRef::Null => Datum::Null,
                ValueRef::Integer(value) => Datum::Int(value),
                ValueRef::Text(value) => Datum::Text(
                    std::str::from_utf8(value)
                        .map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                index,
                                rusqlite::types::Type::Text,
                                Box::new(error),
                            )
                        })?
                        .to_owned(),
                ),
                other => {
                    return Err(rusqlite::Error::InvalidColumnType(
                        index,
                        format!("M2a diagnostic column {index}"),
                        other.data_type(),
                    ))
                }
            })
        })
        .collect()
}

fn query_values(conn: &Connection, query: &str, columns: usize) -> Result<Vec<Vec<Datum>>> {
    let mut statement = sql(conn.prepare(query))?;
    let rows = sql(statement.query_map([], |row| read_values(row, columns)))?;
    let mut values = sql(rows.collect::<rusqlite::Result<Vec<_>>>())?;
    values.sort();
    Ok(values)
}

fn arrangement(conn: &Connection, table: &str) -> Result<Vec<StateEntry>> {
    let mut statement = sql(conn.prepare(&format!("SELECT key,val,w FROM {}", quoted(table))))?;
    let rows = sql(statement.query_map([], |row| {
        Ok((
            row.get::<_, Vec<u8>>(0)?,
            row.get::<_, Vec<u8>>(1)?,
            row.get::<_, i64>(2)?,
        ))
    }))?;
    let mut entries = Vec::new();
    for row in rows {
        let (key, value, weight) = sql(row)?;
        if weight <= 0 {
            return Err(format!(
                "M2a nonpositive retained weight {weight} in {table}"
            ));
        }
        entries.push(StateEntry {
            key: decode(&key)?,
            value: decode(&value)?,
            weight,
        });
    }
    entries.sort();
    Ok(entries)
}

fn expected_tables(join: bool, view: &str) -> Vec<String> {
    let mut names = vec![format!("__ivm_state_{view}_0_agg_groups")];
    if join {
        names.push(format!("__ivm_state_{view}_2_join_left"));
        names.push(format!("__ivm_state_{view}_2_join_right"));
    }
    names.sort();
    names
}

fn verify_layout(conn: &Connection, join: bool, view: &str) -> Result<Vec<String>> {
    let (format, plan): (i64, String) = sql(conn.query_row(
        "SELECT format,plan FROM main.__ivm_view WHERE name=?1",
        [view],
        |row| Ok((row.get(0)?, row.get(1)?)),
    ))?;
    let expected_plan = if join {
        "aggregate([0],[count,sum(1)],project([2,5],join(1,1,scan(3:lhs,[0,1,2]),scan(3:rhs,[0,1,2]))))"
    } else {
        "aggregate([0],[count,sum(1)],project([1,2],scan(6:events,[0,1,2])))"
    };
    if format != 6 || plan != expected_plan {
        return Err(format!(
            "M2a unsupported persisted diagnostic layout: format={format}, plan={plan}"
        ));
    }
    let prefix = format!("__ivm_state_{view}_");
    let mut statement = sql(conn.prepare(
        "SELECT name FROM main.sqlite_schema WHERE type='table' AND substr(name,1,length(?1))=?1 ORDER BY name",
    ))?;
    let names = sql(statement.query_map([prefix], |row| row.get::<_, String>(0)))?;
    let names = sql(names.collect::<rusqlite::Result<Vec<_>>>())?;
    if names != expected_tables(join, view) {
        return Err(format!("M2a operator state table mismatch: {names:?}"));
    }
    Ok(names)
}

fn live_aggregate_sql(join: bool) -> &'static str {
    if join {
        "SELECT lhs.g,count(*),sum(rhs.amount),count(rhs.amount) FROM main.lhs JOIN main.rhs ON lhs.k=rhs.k GROUP BY lhs.g"
    } else {
        "SELECT g,count(*),sum(amount),count(amount) FROM main.events GROUP BY g"
    }
}

fn checked_weight_total(entries: &[StateEntry]) -> Result<i64> {
    entries.iter().try_fold(0_i64, |total, entry| {
        total
            .checked_add(entry.weight)
            .ok_or_else(|| "M2a arrangement weight overflow".into())
    })
}

/// Capture before or after refresh. Pending deltas are allowed here: retained
/// state still describes the preceding refresh while histograms describe the
/// current bases. No refresh or database mutation occurs in this function.
pub(crate) fn capture_snapshot(conn: &Connection, join: bool, view: &str) -> Result<Snapshot> {
    let tables = verify_layout(conn, join, view)?;
    let bases: &[&str] = if join { &["lhs", "rhs"] } else { &["events"] };
    let base_rows = bases
        .iter()
        .map(|table| Ok(((*table).to_owned(), count(conn, table)?)))
        .collect::<Result<_>>()?;
    let mut retained = RetainedSignature::default();
    let mut arrangements = Vec::new();
    let mut aggregate_state_rows = 0;
    for table in tables {
        let entries = arrangement(conn, &table)?;
        if table.ends_with("_agg_groups") {
            aggregate_state_rows = entries.len() as u64;
        }
        arrangements.push(ArrangementMetrics {
            entries: entries.len() as u64,
            weight_total: checked_weight_total(&entries)?,
            table: table.clone(),
        });
        retained.arrangements.insert(table, entries);
    }
    let output_table = format!("__ivm_out_{view}");
    let mut statement = sql(conn.prepare(&format!("SELECT * FROM {}", quoted(&output_table))))?;
    if statement.column_count() != 4 {
        return Err("M2a unexpected output-table arity".into());
    }
    let output =
        sql(statement.query_map([], |row| Ok((read_values(row, 3)?, row.get::<_, i64>(3)?))))?;
    retained.output = sql(output.collect::<rusqlite::Result<Vec<_>>>())?;
    retained.output.sort();
    let output_weight_total = retained
        .output
        .iter()
        .try_fold(0_i64, |total, (_, weight)| {
            if *weight <= 0 {
                return Err("M2a nonpositive retained output weight".to_owned());
            }
            total
                .checked_add(*weight)
                .ok_or_else(|| "M2a output weight overflow".to_owned())
        })?;
    let aggregates = query_values(conn, live_aggregate_sql(join), 4)?;
    let joined_rows = aggregates.iter().try_fold(0_u64, |total, row| {
        let Datum::Int(rows) = row[1] else {
            return Err("M2a noninteger live row count".to_owned());
        };
        u64::try_from(rows)
            .ok()
            .and_then(|rows| total.checked_add(rows))
            .ok_or_else(|| "M2a logical row-count overflow".to_owned())
    })?;
    let histogram_table = if join { "lhs" } else { "events" };
    let histogram = query_values(
        conn,
        &format!(
            "SELECT g,count(*) FROM {} GROUP BY g",
            quoted(histogram_table)
        ),
        2,
    )?;
    let group_histogram = histogram
        .into_iter()
        .map(|row| {
            let group = match row[0] {
                Datum::Int(value) => Some(value),
                Datum::Null => None,
                _ => return Err("M2a noninteger group key".to_owned()),
            };
            let Datum::Int(rows) = row[1] else {
                return Err("M2a noninteger histogram count".to_owned());
            };
            Ok(GroupCount {
                group,
                rows: u64::try_from(rows).map_err(|_| "M2a negative histogram count")?,
            })
        })
        .collect::<Result<_>>()?;
    let pending = bases
        .iter()
        .map(|table| {
            Ok(PendingMetrics {
                base_table: (*table).to_owned(),
                delta_rows: count(conn, &format!("__ivm_delta_{table}"))?,
                replace_candidate_rows: count(conn, &format!("__ivm_pend_{table}"))?,
            })
        })
        .collect::<Result<_>>()?;
    let stage_table = format!("__ivm_stage_{view}");
    let stage_operations = query_values(
        conn,
        &format!(
            "SELECT op,count(*) FROM {} GROUP BY op",
            quoted(&stage_table)
        ),
        2,
    )?;
    let stage_operation_counts = stage_operations
        .into_iter()
        .map(|row| {
            let Datum::Text(operation) = &row[0] else {
                return Err("M2a nontext stage operation".to_owned());
            };
            let Datum::Int(rows) = row[1] else {
                return Err("M2a noninteger stage count".to_owned());
            };
            Ok((
                operation.clone(),
                u64::try_from(rows).map_err(|_| "M2a negative stage count")?,
            ))
        })
        .collect::<Result<_>>()?;
    let stage_rows = count(conn, &stage_table)?;
    Ok(Snapshot {
        base_rows,
        joined_rows,
        live_output_groups: aggregates.len() as u64,
        group_histogram,
        arrangements,
        aggregate_state_rows,
        output_rows: retained.output.len() as u64,
        output_weight_total,
        pending,
        stage_rows,
        retained_stage_rows: stage_rows,
        stage_operation_counts,
        pages: capture_pages(conn)?,
        retained,
    })
}

fn expected_arrangements(
    conn: &Connection,
    join: bool,
    view: &str,
) -> Result<BTreeMap<String, Vec<StateEntry>>> {
    let mut expected = BTreeMap::new();
    let aggregates = query_values(conn, live_aggregate_sql(join), 4)?;
    let mut groups = Vec::new();
    for row in aggregates {
        // GroupState's COUNT(*) accumulator has a zero pair; SUM carries
        // both sum and non-null count, retaining a positive-count zero sum.
        groups.push(StateEntry {
            key: vec![row[0].clone()],
            value: vec![
                row[1].clone(),
                Datum::Int(0),
                Datum::Int(0),
                if row[2] == Datum::Null {
                    Datum::Int(0)
                } else {
                    row[2].clone()
                },
                row[3].clone(),
            ],
            weight: 1,
        });
    }
    groups.sort();
    expected.insert(format!("__ivm_state_{view}_0_agg_groups"), groups);
    if join {
        for (table, columns, role) in [("lhs", "id,k,g", "left"), ("rhs", "id,k,amount", "right")] {
            let values = query_values(
                conn,
                &format!(
                    "SELECT {columns} FROM {} WHERE k IS NOT NULL",
                    quoted(table)
                ),
                3,
            )?;
            let mut entries: Vec<_> = values
                .into_iter()
                .map(|value| StateEntry {
                    key: vec![value[1].clone()],
                    value,
                    weight: 1,
                })
                .collect();
            entries.sort();
            expected.insert(format!("__ivm_state_{view}_2_join_{role}"), entries);
        }
    }
    Ok(expected)
}

/// Spec §5 (amended 2026-10-17): pending deltas and replacement candidates
/// must be empty. The stage table may hold exactly the last applied batch,
/// marked by one `apply` sentinel, because format 6 empties it at the start of
/// the next apply. Its rows are retained space, not pending work.
fn validate_consumed(snapshot: &Snapshot) -> Result<()> {
    if snapshot
        .pending
        .iter()
        .any(|pending| pending.delta_rows != 0 || pending.replace_candidate_rows != 0)
    {
        return Err(format!(
            "M2a refresh left pending deltas or replacement candidates: {:?}",
            snapshot.pending
        ));
    }
    if snapshot.stage_rows > 0
        && (snapshot.stage_operation_counts.get("apply") != Some(&1)
            || snapshot.stage_operation_counts.keys().any(|operation| {
                !matches!(
                    operation.as_str(),
                    "apply" | "state" | "out+" | "out-" | "progress"
                )
            }))
    {
        return Err(format!(
            "M2a staging batch is not already applied: {:?}",
            snapshot.stage_operation_counts
        ));
    }
    Ok(())
}

/// After a refresh, validate every retained entry and multiplicity, aggregate
/// accumulator, physical/virtual output and consumed deltas. Format 5 leaves
/// its last staging batch present, marked by one `apply` sentinel; the next
/// non-idle apply replaces it. Treat it as reported retained space, not as a
/// pending delta or a leak, and never delete it in the diagnostic runner.
pub(crate) fn validate_snapshot(conn: &Connection, join: bool, view: &str) -> Result<Snapshot> {
    let snapshot = capture_snapshot(conn, join, view)?;
    validate_consumed(&snapshot)?;
    let expected = expected_arrangements(conn, join, view)?;
    for (table, expected) in expected {
        let actual = &snapshot.retained.arrangements[&table];
        if *actual != expected {
            let first_difference = actual
                .iter()
                .zip(&expected)
                .position(|(a, e)| a != e)
                .unwrap_or(actual.len().min(expected.len()));
            return Err(format!(
                "M2a decoded arrangement mismatch in {table}: actual entries={}, expected entries={}, first differing entry={first_difference}, actual={:?}, expected={:?}",
                actual.len(), expected.len(), actual.get(first_difference), expected.get(first_difference)
            ));
        }
    }
    let expected_output: Vec<_> = query_values(conn, live_aggregate_sql(join), 4)?
        .into_iter()
        .map(|row| (row[..3].to_vec(), 1))
        .collect();
    if snapshot.retained.output != expected_output {
        return Err("M2a physical output multiset differs from SQLite oracle".into());
    }
    let virtual_output = query_values(conn, &format!("SELECT * FROM {}", quoted(view)), 3)?;
    let expected_output: Vec<_> = expected_output.into_iter().map(|(row, _)| row).collect();
    if virtual_output != expected_output {
        return Err("M2a virtual output multiset differs from SQLite oracle".into());
    }
    Ok(snapshot)
}

/// Validate a repeated refresh with no intervening writes. Page allocation is
/// allowed to retain a reusable high-water mark; logical state, staging row
/// counts and staging operation counts must remain exactly unchanged.
pub(crate) fn validate_idle(previous: &Snapshot, after: &Snapshot) -> Result<()> {
    validate_consumed(previous)?;
    validate_consumed(after)?;
    if previous.retained != after.retained {
        return Err("M2a idle refresh changed retained operator/output state".into());
    }
    if previous.stage_rows != after.stage_rows
        || previous.stage_operation_counts != after.stage_operation_counts
    {
        return Err("M2a idle refresh changed retained staging rows".into());
    }
    if previous.base_rows != after.base_rows
        || previous.joined_rows != after.joined_rows
        || previous.live_output_groups != after.live_output_groups
        || previous.group_histogram != after.group_histogram
    {
        return Err("M2a idle refresh changed live-base diagnostics".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ivmlite_test::open_with_extension;

    const VIEW: &str = "mv_m2a";

    fn fixture(join: bool) -> Connection {
        let conn = open_with_extension(None).expect("fresh debug extension required");
        if join {
            conn.execute_batch(
                "CREATE TABLE lhs(id INTEGER PRIMARY KEY,k INTEGER,g INTEGER) STRICT;
                 CREATE TABLE rhs(id INTEGER PRIMARY KEY,k INTEGER,amount INTEGER) STRICT;
                 INSERT INTO lhs VALUES (1,5,10),(2,5,10),(3,8,20),(4,NULL,30);
                 INSERT INTO rhs VALUES (1,5,7),(2,5,-7),(3,9,3),(4,NULL,4);
                 CREATE VIRTUAL TABLE mv_m2a USING ivm('SELECT lhs.g,COUNT(*),SUM(rhs.amount) FROM lhs JOIN rhs ON lhs.k=rhs.k GROUP BY lhs.g');",
            )
            .unwrap();
        } else {
            conn.execute_batch(
                "CREATE TABLE events(id INTEGER PRIMARY KEY,g INTEGER,amount INTEGER) STRICT;
                 INSERT INTO events VALUES (1,10,7),(2,10,-7),(3,20,NULL),(4,NULL,5);
                 CREATE VIRTUAL TABLE mv_m2a USING ivm('SELECT g,COUNT(*),SUM(amount) FROM events GROUP BY g');",
            )
            .unwrap();
        }
        conn
    }

    fn refresh(conn: &Connection) {
        conn.execute("INSERT INTO mv_m2a(mv_m2a) VALUES('refresh')", [])
            .unwrap();
    }

    #[test]
    fn m2a_state_single_zero_sum_null_and_idle_refresh() {
        let conn = fixture(false);
        let before = validate_snapshot(&conn, false, VIEW).unwrap();
        assert_eq!(before.base_rows["events"], 4);
        assert_eq!(before.joined_rows, 4);
        assert_eq!(before.aggregate_state_rows, 3);
        assert!(before
            .retained_signature()
            .output
            .contains(&(vec![Datum::Int(10), Datum::Int(2), Datum::Int(0)], 1)));
        conn.execute("UPDATE events SET amount=9 WHERE id=1", [])
            .unwrap();
        let pending = capture_snapshot(&conn, false, VIEW).unwrap();
        assert_eq!(pending.retained_signature(), before.retained_signature());
        assert_eq!(pending.pending[0].delta_rows, 2);
        assert!(validate_snapshot(&conn, false, VIEW).is_err());
        refresh(&conn);
        let after = validate_snapshot(&conn, false, VIEW).unwrap();
        refresh(&conn);
        let idle = validate_snapshot(&conn, false, VIEW).unwrap();
        assert_eq!(after.retained_signature(), idle.retained_signature());
        assert!(after.stage_rows > 0);
        assert_eq!(after.stage_operation_counts["apply"], 1);
        validate_idle(&after, &idle).unwrap();
    }

    #[test]
    fn m2a_state_join_full_rows_orphans_nulls_and_inverse() {
        let conn = fixture(true);
        let before = validate_snapshot(&conn, true, VIEW).unwrap();
        assert_eq!(before.joined_rows, 4);
        assert_eq!(before.aggregate_state_rows, 1);
        assert_eq!(before.arrangements[1].entries, 3);
        conn.execute_batch(
            "BEGIN;
             UPDATE lhs SET k=9,g=40 WHERE id=1;
             UPDATE rhs SET k=9,amount=13 WHERE id=1;
             COMMIT;",
        )
        .unwrap();
        refresh(&conn);
        validate_snapshot(&conn, true, VIEW).unwrap();
        conn.execute_batch(
            "BEGIN;
             UPDATE rhs SET k=5,amount=7 WHERE id=1;
             UPDATE lhs SET k=5,g=10 WHERE id=1;
             COMMIT;",
        )
        .unwrap();
        refresh(&conn);
        let restored = validate_snapshot(&conn, true, VIEW).unwrap();
        assert_eq!(before.retained_signature(), restored.retained_signature());
    }

    #[test]
    fn m2a_state_rejects_ghost_missing_and_wrong_weight_with_same_totals() {
        for mutation in [
            "UPDATE __ivm_state_mv_m2a_2_join_left SET val=x'01000000000000006301000000000000000501000000000000000a' WHERE key=x'010000000000000005' AND val=x'01000000000000000101000000000000000501000000000000000a'",
            "DELETE FROM __ivm_state_mv_m2a_2_join_right WHERE key=x'010000000000000009'",
            "UPDATE __ivm_state_mv_m2a_2_join_left SET w=2 WHERE key=x'010000000000000008'",
        ] {
            let conn = fixture(true);
            validate_snapshot(&conn, true, VIEW).unwrap();
            assert_eq!(conn.execute(mutation, []).unwrap(), 1);
            let error = validate_snapshot(&conn, true, VIEW).unwrap_err();
            assert!(error.contains("decoded arrangement mismatch"), "{error}");
        }
    }

    #[test]
    fn m2a_state_rejects_corrupt_bytes_and_output_duplicates() {
        let conn = fixture(true);
        conn.execute(
            "UPDATE __ivm_state_mv_m2a_2_join_left SET val=x'01' WHERE key=x'010000000000000008'",
            [],
        )
        .unwrap();
        assert!(capture_snapshot(&conn, true, VIEW)
            .unwrap_err()
            .contains("truncated INTEGER"));
        let conn = fixture(false);
        conn.execute_batch("INSERT INTO __ivm_out_mv_m2a SELECT * FROM __ivm_out_mv_m2a LIMIT 1;")
            .unwrap();
        assert!(validate_snapshot(&conn, false, VIEW)
            .unwrap_err()
            .contains("physical output multiset"));
    }

    #[test]
    fn m2a_state_requires_applied_staging_and_rejects_idle_growth() {
        let conn = fixture(false);
        conn.execute("UPDATE events SET amount=9 WHERE id=1", [])
            .unwrap();
        refresh(&conn);
        let before = validate_snapshot(&conn, false, VIEW).unwrap();
        conn.execute("INSERT INTO __ivm_stage_mv_m2a(op) VALUES('state')", [])
            .unwrap();
        let after = validate_snapshot(&conn, false, VIEW).unwrap();
        assert!(validate_idle(&before, &after)
            .unwrap_err()
            .contains("staging rows"));
        conn.execute(
            "UPDATE __ivm_stage_mv_m2a SET op='arm' WHERE op='apply'",
            [],
        )
        .unwrap();
        assert!(validate_snapshot(&conn, false, VIEW)
            .unwrap_err()
            .contains("not already applied"));
    }

    fn consumed_snapshot() -> Snapshot {
        let conn = fixture(false);
        validate_snapshot(&conn, false, VIEW).unwrap()
    }

    #[test]
    fn stage_holds_exactly_the_last_applied_batch_after_refresh() {
        let conn = fixture(true);
        let created = validate_snapshot(&conn, true, VIEW).unwrap();
        assert_eq!(created.retained_stage_rows, 0);
        conn.execute_batch(
            "INSERT INTO lhs VALUES (5,8,20);
             INSERT INTO rhs VALUES (5,8,11);
             UPDATE lhs SET g=21 WHERE id=3;",
        )
        .unwrap();
        refresh(&conn);
        let after = validate_snapshot(&conn, true, VIEW).unwrap();
        assert!(after
            .pending
            .iter()
            .all(|pending| pending.delta_rows == 0 && pending.replace_candidate_rows == 0));
        assert_eq!(after.stage_operation_counts["apply"], 1);
        assert!(after.retained_stage_rows > 0);
        assert_eq!(after.retained_stage_rows, after.stage_rows);
        refresh(&conn);
        let idle = validate_snapshot(&conn, true, VIEW).unwrap();
        assert_eq!(idle.retained_stage_rows, after.retained_stage_rows);
    }

    #[test]
    fn validate_consumed_rejects_a_stage_without_an_apply_sentinel() {
        let snapshot = Snapshot {
            stage_rows: 2,
            retained_stage_rows: 2,
            stage_operation_counts: BTreeMap::from([("state".to_owned(), 2)]),
            ..consumed_snapshot()
        };
        let error = validate_consumed(&snapshot).unwrap_err();
        assert!(error.contains("not already applied"), "{error}");
    }

    #[test]
    fn validate_consumed_rejects_pending_deltas() {
        let snapshot = Snapshot {
            pending: vec![PendingMetrics {
                base_table: "events".to_owned(),
                delta_rows: 1,
                replace_candidate_rows: 0,
            }],
            ..consumed_snapshot()
        };
        let error = validate_consumed(&snapshot).unwrap_err();
        assert!(error.contains("pending deltas"), "{error}");
    }

    fn staged(operations: &[(&str, u64)]) -> Snapshot {
        Snapshot {
            stage_rows: operations.iter().map(|&(_, rows)| rows).sum(),
            stage_operation_counts: operations
                .iter()
                .map(|&(operation, rows)| (operation.to_owned(), rows))
                .collect(),
            ..consumed_snapshot()
        }
    }

    #[test]
    fn validate_consumed_accepts_one_sentinel_with_known_operations_only() {
        validate_consumed(&staged(&[
            ("apply", 1),
            ("state", 2),
            ("out+", 1),
            ("out-", 1),
            ("progress", 1),
        ]))
        .unwrap();
        for operations in [
            &[("apply", 1), ("bogus", 2)][..],
            &[("apply", 2), ("state", 1)][..],
        ] {
            let error = validate_consumed(&staged(operations)).unwrap_err();
            assert!(
                error.contains("not already applied"),
                "{operations:?}: {error}"
            );
        }
    }

    #[test]
    fn validate_consumed_rejects_replacement_candidates() {
        let snapshot = Snapshot {
            pending: vec![PendingMetrics {
                base_table: "events".to_owned(),
                delta_rows: 0,
                replace_candidate_rows: 1,
            }],
            ..consumed_snapshot()
        };
        let error = validate_consumed(&snapshot).unwrap_err();
        assert!(error.contains("replacement candidates"), "{error}");
    }

    #[test]
    fn validate_idle_rejects_changed_retained_state_and_live_bases() {
        let before = consumed_snapshot();
        validate_idle(&before, &before.clone()).unwrap();
        let mut retained = before.clone();
        retained.retained.output.push((vec![Datum::Int(99)], 1));
        let error = validate_idle(&before, &retained).unwrap_err();
        assert!(error.contains("retained operator/output state"), "{error}");
        let live = [
            Snapshot {
                base_rows: BTreeMap::from([("events".to_owned(), 999)]),
                ..before.clone()
            },
            Snapshot {
                joined_rows: before.joined_rows + 1,
                ..before.clone()
            },
            Snapshot {
                live_output_groups: before.live_output_groups + 1,
                ..before.clone()
            },
            Snapshot {
                group_histogram: vec![GroupCount {
                    group: Some(1),
                    rows: 1,
                }],
                ..before.clone()
            },
        ];
        for after in live {
            let error = validate_idle(&before, &after).unwrap_err();
            assert!(error.contains("live-base diagnostics"), "{error}");
        }
    }

    #[test]
    fn m2a_state_rejects_zero_and_negative_retained_weights() {
        for weight in [0, -1] {
            let conn = fixture(true);
            conn.execute(
                &format!("UPDATE __ivm_state_mv_m2a_2_join_left SET w={weight} WHERE key=x'010000000000000008'"),
                [],
            )
            .unwrap();
            let error = capture_snapshot(&conn, true, VIEW).unwrap_err();
            assert!(
                error.contains("nonpositive retained weight"),
                "{weight}: {error}"
            );
        }
    }

    #[test]
    fn m2a_state_summary_drops_decoded_rows_and_preserves_every_exported_field() {
        let conn = fixture(true);
        let snapshot = validate_snapshot(&conn, true, VIEW).unwrap();
        assert!(!snapshot.retained_signature().arrangements.is_empty());
        assert!(!snapshot.retained_signature().output.is_empty());
        let exported = serde_json::to_value(&snapshot).unwrap();
        let summary = snapshot.into_summary();
        assert!(summary.retained_signature().arrangements.is_empty());
        assert!(summary.retained_signature().output.is_empty());
        assert_eq!(serde_json::to_value(summary).unwrap(), exported);
    }

    #[test]
    fn m2a_state_codec_is_strict_and_page_measurement_is_consistent() {
        assert_eq!(
            decode(&[0, 2, 0, 0, 0, 1, b'x']).unwrap(),
            vec![Datum::Null, Datum::Text("x".into())]
        );
        for bytes in [
            &[1, 0][..],
            &[2, 0, 0][..],
            &[2, 0, 0, 0, 1, 255][..],
            &[9][..],
        ] {
            assert!(decode(bytes).is_err());
        }
        let conn = Connection::open_in_memory().unwrap();
        let pages = capture_pages(&conn).unwrap();
        assert_eq!(pages.allocated_bytes, pages.page_size * pages.page_count);
        assert_eq!(
            pages.live_page_bytes,
            pages.page_size * (pages.page_count - pages.freelist_count)
        );
    }
}
