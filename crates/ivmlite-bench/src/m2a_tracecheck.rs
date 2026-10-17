//! Untimed proof that SQL bases contain the exported trace prefix exactly.
//!
//! A view and its SQLite oracle can agree after the executor skips an UPDATE:
//! both then read the same incorrect bases. This independent map replay checks
//! full rows (including primary keys), not checksums or the executor's SQL.
use std::collections::BTreeMap;

use ivmlite_workload::m2a::{Fixture, Mutation, Row, Table};
use rusqlite::Connection;

type Result<T> = std::result::Result<T, String>;

pub(crate) struct TraceReplay {
    tables: BTreeMap<Table, BTreeMap<i64, Row>>,
}

impl TraceReplay {
    pub(crate) fn new(fixture: &Fixture) -> Result<Self> {
        fixture
            .validate()
            .map_err(|error| format!("M2a trace-prefix fixture: {error}"))?;
        let mut tables = BTreeMap::new();
        for (&table, rows) in &fixture.tables {
            let mut expected = BTreeMap::new();
            for row in rows {
                validate_row(table, row)?;
                if expected.insert(row.id, row.clone()).is_some() {
                    return Err(format!(
                        "M2a trace-prefix duplicate initial {} PK {}",
                        table.name(),
                        row.id
                    ));
                }
            }
            tables.insert(table, expected);
        }
        Ok(Self { tables })
    }

    /// Advance one transaction without executing any SQL. Validate the entire
    /// patch before committing it so an invalid before-image cannot leave the
    /// replay model partially advanced. Inverse batches use the same method.
    pub(crate) fn apply(&mut self, operations: &[Mutation]) -> Result<()> {
        let mut patch: BTreeMap<(Table, i64), Option<Row>> = BTreeMap::new();
        for (index, operation) in operations.iter().enumerate() {
            let table = self.tables.get(&operation.table).ok_or_else(|| {
                format!(
                    "M2a trace-prefix operation {index} names absent table {}",
                    operation.table.name()
                )
            })?;
            let row = operation
                .before
                .as_ref()
                .or(operation.after.as_ref())
                .ok_or_else(|| format!("M2a trace-prefix operation {index} is empty"))?;
            for candidate in [&operation.before, &operation.after].into_iter().flatten() {
                validate_row(operation.table, candidate)?;
                if candidate.id != row.id {
                    return Err(format!(
                        "M2a trace-prefix operation {index} changes its primary key"
                    ));
                }
            }
            if operation.before == operation.after {
                return Err(format!(
                    "M2a trace-prefix operation {index} is a no-op UPDATE"
                ));
            }
            let actual = match patch.get(&(operation.table, row.id)) {
                Some(patched) => patched.as_ref(),
                None => table.get(&row.id),
            };
            if actual != operation.before.as_ref() {
                return Err(format!(
                    "M2a trace-prefix operation {index} {} PK {} before-image mismatch: expected live={actual:?}, trace={:?}",
                    operation.table.name(),
                    row.id,
                    operation.before
                ));
            }
            patch.insert((operation.table, row.id), operation.after.clone());
        }
        for ((table, id), after) in patch {
            let rows = self.tables.get_mut(&table).expect("validated table");
            if let Some(after) = after {
                rows.insert(id, after);
            } else {
                rows.remove(&id);
            }
        }
        Ok(())
    }

    /// Compare every SQL base row against the independently replayed prefix.
    /// Stream in primary-key order so verification does not retain another full
    /// fixture. Run outside timing at bootstrap, warm-up and final checkpoints,
    /// and after each transaction in the separate correctness replay.
    pub(crate) fn verify(&self, connection: &Connection) -> Result<()> {
        for (&table, expected) in &self.tables {
            let query = match table {
                Table::Events => "SELECT id,g,amount FROM main.events ORDER BY id",
                Table::Lhs => "SELECT id,k,g FROM main.lhs ORDER BY id",
                Table::Rhs => "SELECT id,k,amount FROM main.rhs ORDER BY id",
            };
            let mut statement = connection
                .prepare(query)
                .map_err(|error| format!("M2a trace-prefix {} query: {error}", table.name()))?;
            let mut actual = statement
                .query([])
                .map_err(|error| format!("M2a trace-prefix {} query: {error}", table.name()))?;
            let mut expected = expected.values();
            let mut index = 0;
            loop {
                let sql_row = actual
                    .next()
                    .map_err(|error| format!("M2a trace-prefix {} row: {error}", table.name()))?;
                let wanted = expected.next();
                let got = sql_row
                    .map(|sql_row| {
                        let id = sql_row.get::<_, i64>(0)?;
                        let first = sql_row.get::<_, i64>(1)?;
                        let second = sql_row.get::<_, i64>(2)?;
                        Ok::<_, rusqlite::Error>(match table {
                            Table::Events => Row {
                                id,
                                k: None,
                                g: Some(first),
                                amount: Some(second),
                            },
                            Table::Lhs => Row {
                                id,
                                k: Some(first),
                                g: Some(second),
                                amount: None,
                            },
                            Table::Rhs => Row {
                                id,
                                k: Some(first),
                                g: None,
                                amount: Some(second),
                            },
                        })
                    })
                    .transpose()
                    .map_err(|error| {
                        format!("M2a trace-prefix {} row {index}: {error}", table.name())
                    })?;
                if got.as_ref() != wanted {
                    return Err(format!(
                        "M2a trace-prefix {} row {index} mismatch: actual={got:?}, expected={wanted:?}",
                        table.name()
                    ));
                }
                if got.is_none() {
                    break;
                }
                index += 1;
            }
        }
        Ok(())
    }
}

fn validate_row(table: Table, row: &Row) -> Result<()> {
    let valid = match table {
        Table::Events => row.k.is_none() && row.g.is_some() && row.amount.is_some(),
        Table::Lhs => row.k.is_some() && row.g.is_some() && row.amount.is_none(),
        Table::Rhs => row.k.is_some() && row.g.is_none() && row.amount.is_some(),
    };
    if !valid {
        return Err(format!(
            "M2a trace-prefix {} PK {} does not match its non-null STRICT schema",
            table.name(),
            row.id
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ivmlite_test::demo_bench::check_same_multiset;
    use ivmlite_test::open_with_extension;
    use ivmlite_workload::m2a::{M2aConfig, Profile, Shape};
    use rusqlite::params;
    use std::path::Path;

    fn fixture(join: bool) -> Fixture {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../workloads/m2a.toml");
        let mut config = M2aConfig::load(&path).unwrap();
        config.single_rows = 800;
        config.single_groups = 20;
        config.join_rows = 800;
        config.join_groups = 20;
        config.batch_sizes = vec![10];
        config.fanouts = vec![8];
        let shape = if join { Shape::Join } else { Shape::Single };
        let cell = config
            .cells()
            .unwrap()
            .into_iter()
            .find(|cell| cell.shape == shape && cell.profile == Profile::ZipfHot)
            .unwrap();
        config.generate(&cell, 42).unwrap()
    }

    fn database(fixture: &Fixture, extension: bool) -> Connection {
        let connection = if extension {
            open_with_extension(None).unwrap()
        } else {
            Connection::open_in_memory().unwrap()
        };
        connection.execute_batch(fixture.cell.schema_ddl()).unwrap();
        for (&table, rows) in &fixture.tables {
            for row in rows {
                let (sql, a, b) = match table {
                    Table::Events => (
                        "INSERT INTO events(id,g,amount) VALUES(?1,?2,?3)",
                        row.g,
                        row.amount,
                    ),
                    Table::Lhs => ("INSERT INTO lhs(id,k,g) VALUES(?1,?2,?3)", row.k, row.g),
                    Table::Rhs => (
                        "INSERT INTO rhs(id,k,amount) VALUES(?1,?2,?3)",
                        row.k,
                        row.amount,
                    ),
                };
                connection.execute(sql, params![row.id, a, b]).unwrap();
            }
        }
        connection
    }

    /// Independent test executor, deliberately unrelated to Prepared::apply.
    fn execute(connection: &Connection, operation: &Mutation) {
        match (&operation.before, &operation.after) {
            (Some(before), None) => {
                connection
                    .execute(
                        &format!("DELETE FROM {} WHERE id=?1", operation.table.name()),
                        [before.id],
                    )
                    .unwrap();
            }
            (before, Some(after)) => {
                let (columns, a, b) = match operation.table {
                    Table::Events => ("g,amount", after.g, after.amount),
                    Table::Lhs => ("k,g", after.k, after.g),
                    Table::Rhs => ("k,amount", after.k, after.amount),
                };
                if before.is_some() {
                    let (first, second) = columns.split_once(',').unwrap();
                    connection
                        .execute(
                            &format!(
                                "UPDATE {} SET {first}=?2,{second}=?3 WHERE id=?1",
                                operation.table.name()
                            ),
                            params![after.id, a, b],
                        )
                        .unwrap();
                } else {
                    connection
                        .execute(
                            &format!(
                                "INSERT INTO {}(id,{columns}) VALUES(?1,?2,?3)",
                                operation.table.name()
                            ),
                            params![after.id, a, b],
                        )
                        .unwrap();
                }
            }
            _ => panic!("empty test mutation"),
        }
    }

    #[test]
    fn m2a_trace_prefix_tracks_every_batch_and_inverse_for_both_shapes() {
        for join in [false, true] {
            let fixture = fixture(join);
            let connection = database(&fixture, false);
            let mut replay = TraceReplay::new(&fixture).unwrap();
            replay.verify(&connection).unwrap();
            for batch in fixture
                .batches
                .iter()
                .chain(fixture.inverse_batches().iter())
            {
                replay.apply(batch).unwrap();
                for operation in batch {
                    execute(&connection, operation);
                }
                replay.verify(&connection).unwrap();
            }
            TraceReplay::new(&fixture)
                .unwrap()
                .verify(&connection)
                .unwrap();
        }
    }

    #[test]
    fn m2a_trace_prefix_rejects_missing_sql_updates_when_view_and_oracle_agree() {
        for join in [false, true] {
            let fixture = fixture(join);
            let connection = database(&fixture, true);
            connection
                .execute_batch(&format!(
                    "CREATE VIRTUAL TABLE mv_tracecheck USING ivm('{}')",
                    fixture.cell.view_sql()
                ))
                .unwrap();
            let mut replay = TraceReplay::new(&fixture).unwrap();
            replay.apply(&fixture.batches[0][..4]).unwrap();
            for operation in &fixture.batches[0][..4] {
                execute(&connection, operation);
            }
            let operation = &fixture.batches[0][4]; // Actual non-noop amount UPDATE.
            replay.apply(std::slice::from_ref(operation)).unwrap();
            // A broken executor omits the UPDATE; refresh and the same-DB
            // SQLite oracle both still return the unchanged result.
            connection
                .execute(
                    "INSERT INTO mv_tracecheck(mv_tracecheck) VALUES('refresh')",
                    [],
                )
                .unwrap();
            check_same_multiset(
                &connection,
                "SELECT * FROM mv_tracecheck",
                fixture.cell.view_sql(),
            )
            .unwrap();
            let error = replay.verify(&connection).unwrap_err();
            assert!(error.contains("trace-prefix") && error.contains("mismatch"));
        }
    }

    #[test]
    fn m2a_trace_prefix_rejects_wrong_sql_update_and_missing_delete() {
        let fixture = fixture(false);
        let connection = database(&fixture, false);
        let mut replay = TraceReplay::new(&fixture).unwrap();
        replay.apply(&fixture.batches[0][..4]).unwrap();
        for operation in &fixture.batches[0][..4] {
            execute(&connection, operation);
        }
        let update = &fixture.batches[0][4];
        replay.apply(std::slice::from_ref(update)).unwrap();
        let mut wrong = update.clone();
        wrong.after.as_mut().unwrap().amount = update.before.as_ref().unwrap().amount;
        execute(&connection, &wrong);
        assert!(replay.verify(&connection).is_err());
        execute(&connection, update);
        replay.verify(&connection).unwrap();
        let fresh = database(&fixture, false);
        let mut fresh_replay = TraceReplay::new(&fixture).unwrap();
        fresh_replay.apply(&fixture.batches[0][..1]).unwrap();
        assert!(fresh_replay.verify(&fresh).is_err());
    }

    #[test]
    fn m2a_trace_prefix_checks_before_images_and_keeps_rejected_batches_atomic() {
        let fixture = fixture(false);
        let connection = database(&fixture, false);
        let mut replay = TraceReplay::new(&fixture).unwrap();
        let mut batch = fixture.batches[0].clone();
        batch[4].before.as_mut().unwrap().amount = Some(999);
        let error = replay.apply(&batch).unwrap_err();
        assert!(error.contains("before-image mismatch"));
        replay.verify(&connection).unwrap(); // Earlier valid DELETE/INSERTs were not committed.
        let mut missing = fixture.batches[0][0].clone();
        missing.before.as_mut().unwrap().id = i64::MAX;
        assert!(replay.apply(&[missing]).is_err());
        let mut invalid_schema = fixture.batches[0][4].clone();
        invalid_schema.after.as_mut().unwrap().g = None;
        assert!(replay.apply(&[invalid_schema]).is_err());
        replay.verify(&connection).unwrap();
    }

    #[test]
    fn m2a_trace_prefix_rejects_missing_extra_and_corrupted_bootstrap_rows() {
        let fixture = fixture(false);
        let connection = database(&fixture, false);
        let replay = TraceReplay::new(&fixture).unwrap();
        connection
            .execute("UPDATE events SET g=g+1 WHERE id=0", [])
            .unwrap();
        assert!(replay.verify(&connection).is_err());
        let original = &fixture.tables[&Table::Events][0];
        connection
            .execute("UPDATE events SET g=?1 WHERE id=0", [original.g])
            .unwrap();
        replay.verify(&connection).unwrap();
        connection
            .execute("INSERT INTO events VALUES(999999,1,1)", [])
            .unwrap();
        assert!(replay.verify(&connection).is_err());
        connection
            .execute("DELETE FROM events WHERE id=999999", [])
            .unwrap();
        connection
            .execute("DELETE FROM events WHERE id=0", [])
            .unwrap();
        assert!(replay.verify(&connection).is_err());
    }
}
