use std::collections::BTreeMap;

use ivmlite_core::{Database, Row, Value, ZSet};
use rusqlite::types::{ToSqlOutput, ValueRef};
use rusqlite::{Connection, ToSql};

use crate::{create_table_sql, view_query_to_sql, EngineError, ViewQuery};

struct Bound<'a>(&'a Value);

impl ToSql for Bound<'_> {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(match self.0 {
            Value::Null => ToSqlOutput::Owned(rusqlite::types::Value::Null),
            Value::Int(n) => ToSqlOutput::Owned(rusqlite::types::Value::Integer(*n)),
            Value::Text(s) => ToSqlOutput::Owned(rusqlite::types::Value::Text(s.clone())),
        })
    }
}

fn from_sqlite(v: ValueRef<'_>) -> Result<Value, EngineError> {
    match v {
        ValueRef::Null => Ok(Value::Null),
        ValueRef::Integer(n) => Ok(Value::Int(n)),
        ValueRef::Text(bytes) => std::str::from_utf8(bytes)
            .map(|s| Value::Text(s.to_string()))
            .map_err(|e| EngineError(format!("text is not valid UTF-8: {e}"))),
        ValueRef::Real(_) => Err(EngineError("v0 does not support REAL".into())),
        ValueRef::Blob(_) => Err(EngineError("v0 does not support BLOB".into())),
    }
}

/// The authoritative judge: load every base-table state into an in-memory
/// SQLite and let SQLite execute the original SQL itself.
///
/// An implementation independent of all of this project's code — judging
/// NaiveRecompute with NaiveRecompute would be circular, which is exactly why
/// this exists separately.
pub fn recompute_via_sqlite(
    db: &Database,
    query: &ViewQuery,
    bases: &BTreeMap<String, ZSet>,
) -> Result<ZSet, EngineError> {
    let conn = Connection::open_in_memory().map_err(|e| EngineError(e.to_string()))?;

    for schema in db.tables() {
        conn.execute_batch(&create_table_sql(schema))
            .map_err(|e| EngineError(e.to_string()))?;

        let base = bases.get(&schema.table).ok_or_else(|| {
            EngineError(format!("missing the base state for table {}", schema.table))
        })?;

        let placeholders = vec!["?"; schema.arity()].join(", ");
        let insert_sql = format!(
            "INSERT INTO \"{}\" ({}) VALUES ({})",
            schema.table,
            schema
                .column_names()
                .iter()
                .map(|n| format!("\"{n}\""))
                .collect::<Vec<_>>()
                .join(", "),
            placeholders
        );

        let mut stmt = conn
            .prepare(&insert_sql)
            .map_err(|e| EngineError(e.to_string()))?;
        for (row, weight) in base.iter() {
            if *weight < 0 {
                return Err(EngineError(format!(
                    "the base state of table {} has a negative weight {weight}, row {row:?}",
                    schema.table
                )));
            }
            let bound: Vec<Bound> = row.0.iter().map(Bound).collect();
            let params: Vec<&dyn ToSql> = bound.iter().map(|b| b as &dyn ToSql).collect();
            for _ in 0..*weight {
                stmt.execute(params.as_slice())
                    .map_err(|e| EngineError(e.to_string()))?;
            }
        }
    }

    // View SQL is still rendered for a single table; rendering join queries arrives in the engine plan's Phase 3.
    let anchor = db
        .tables()
        .first()
        .ok_or_else(|| EngineError("database has no tables".into()))?;
    let sql = view_query_to_sql(query, anchor);

    let mut stmt = conn.prepare(&sql).map_err(|e| EngineError(e.to_string()))?;
    let arity = query.output_arity();

    let mut out = ZSet::new();
    let mut rows = stmt.query([]).map_err(|e| EngineError(e.to_string()))?;
    while let Some(r) = rows.next().map_err(|e| EngineError(e.to_string()))? {
        let mut values = Vec::with_capacity(arity);
        for i in 0..arity {
            let raw = r.get_ref(i).map_err(|e| EngineError(e.to_string()))?;
            values.push(from_sqlite(raw)?);
        }
        out.update(Row::new(values), 1);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::single_base;
    use crate::{Agg, AggFn, Column, ColumnType, Predicate, Schema, ViewQuery};

    fn orders() -> Schema {
        Schema {
            table: "orders".into(),
            columns: vec![
                Column {
                    name: "region".into(),
                    ty: ColumnType::Text,
                    nullable: true,
                },
                Column {
                    name: "amount".into(),
                    ty: ColumnType::Integer,
                    nullable: false,
                },
            ],
        }
    }

    fn customers() -> Schema {
        Schema {
            table: "customers".into(),
            columns: vec![Column {
                name: "name".into(),
                ty: ColumnType::Text,
                nullable: false,
            }],
        }
    }

    fn row(region: Value, amount: i64) -> Row {
        Row::new(vec![region, Value::Int(amount)])
    }

    fn cust_row(name: &str) -> Row {
        Row::new(vec![Value::Text(name.into())])
    }

    fn count_by_region() -> ViewQuery {
        ViewQuery {
            group_by: vec![0],
            aggs: vec![Agg {
                func: AggFn::Count,
                column: None,
            }],
            predicate: Predicate::None,
            join: None,
        }
    }

    #[test]
    fn matches_hand_computed_aggregate() {
        let q = ViewQuery {
            group_by: vec![0],
            aggs: vec![
                Agg {
                    func: AggFn::Sum,
                    column: Some(1),
                },
                Agg {
                    func: AggFn::Count,
                    column: None,
                },
            ],
            predicate: Predicate::None,
            join: None,
        };
        let base = ZSet::from_rows([
            (row(Value::Text("a".into()), 10), 1),
            (row(Value::Text("a".into()), 5), 1),
            (row(Value::Text("b".into()), 3), 1),
        ]);

        let db = Database::single(orders());
        let bases = single_base("orders", base);
        let got = recompute_via_sqlite(&db, &q, &bases).unwrap();
        assert_eq!(
            got.weight_of(&Row::new(vec![
                Value::Text("a".into()),
                Value::Int(15),
                Value::Int(2)
            ])),
            1
        );
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn expands_rows_by_weight() {
        let q = ViewQuery {
            group_by: vec![0],
            aggs: vec![Agg {
                func: AggFn::Count,
                column: None,
            }],
            predicate: Predicate::None,
            join: None,
        };
        let base = ZSet::from_rows([(row(Value::Text("a".into()), 1), 3)]);

        let db = Database::single(orders());
        let bases = single_base("orders", base);
        let got = recompute_via_sqlite(&db, &q, &bases).unwrap();
        assert_eq!(
            got.weight_of(&Row::new(vec![Value::Text("a".into()), Value::Int(3)])),
            1,
            "a weight of 3 should expand to 3 rows, giving COUNT(*) = 3"
        );
    }

    #[test]
    fn rejects_negative_weights_in_base_state() {
        let q = ViewQuery {
            group_by: vec![0],
            aggs: vec![Agg {
                func: AggFn::Count,
                column: None,
            }],
            predicate: Predicate::None,
            join: None,
        };
        let base = ZSet::from_rows([(row(Value::Text("a".into()), 1), -1)]);

        let db = Database::single(orders());
        let bases = single_base("orders", base);
        assert!(
            recompute_via_sqlite(&db, &q, &bases).is_err(),
            "a negative weight in the base state means something upstream is already wrong; the oracle must reject it, not accept it silently"
        );
    }

    /// Pins spec §6.1's NULL-semantics contract and lines NaiveRecompute up with
    /// SQLite. Task 7 already asserted NaiveRecompute's behaviour on its own;
    /// this adds SQLite's confirmation.
    #[test]
    fn sum_over_all_null_matches_naive_recompute() {
        use crate::{Engine, NaiveRecompute};

        let nullable_amount = Schema {
            table: "orders".into(),
            columns: vec![
                Column {
                    name: "region".into(),
                    ty: ColumnType::Text,
                    nullable: false,
                },
                Column {
                    name: "amount".into(),
                    ty: ColumnType::Integer,
                    nullable: true,
                },
            ],
        };
        let q = ViewQuery {
            group_by: vec![0],
            aggs: vec![
                Agg {
                    func: AggFn::Sum,
                    column: Some(1),
                },
                Agg {
                    func: AggFn::Count,
                    column: None,
                },
            ],
            predicate: Predicate::None,
            join: None,
        };
        let base = ZSet::from_rows([(Row::new(vec![Value::Text("a".into()), Value::Null]), 2)]);

        let db = Database::single(nullable_amount.clone());
        let bases = single_base("orders", base.clone());
        let want = recompute_via_sqlite(&db, &q, &bases).unwrap();
        assert_eq!(
            want.weight_of(&Row::new(vec![
                Value::Text("a".into()),
                Value::Null,
                Value::Int(2)
            ])),
            1,
            "SQLite's SUM returns NULL when there is no non-NULL input"
        );

        let mut e = NaiveRecompute::new();
        e.create_view(&db, &q, &bases).unwrap();
        assert_eq!(e.materialize().unwrap(), want);
    }

    #[test]
    fn builds_every_table_in_the_database() {
        // Two tables, and the query touches only orders; customers' base state
        // must be walked too, even though the query never reads it — otherwise
        // join queries (Phase 3) would silently be one table short on the
        // oracle side. The assertion cannot look only at the query result: the
        // query is still rendered for the anchor alone (db.tables()[0] ==
        // orders), customers never appears in the SQL, and an implementation
        // that creates only tables()[0] would compute exactly the same `got`,
        // so such an assertion would tell nothing apart.
        //
        // A negative weight is the probe: customers' negative weight is rejected
        // only if "the table loop reaches customers and really checks the
        // weights of its base state". Note this proves only that the loop body
        // reached the weight check for customers, not that CREATE TABLE ran on
        // its own — if an implementation skipped CREATE TABLE but still did the
        // base lookup and walk for customers, the insert would fail first with
        // "no such table: customers", a message that also contains the
        // substring "customers" and would let an assertion checking only for
        // "customers" pass by mistake. So the assertion also requires the
        // phrase unique to the negative-weight path ("has a negative weight"),
        // telling "the negative-weight check really ran for customers" apart
        // from "the name customers turned up in some unrelated SQL error".
        let db = Database::new(vec![orders(), customers()]);
        let bases = BTreeMap::from([
            (
                "orders".to_string(),
                ZSet::from_rows([(row(Value::Text("a".into()), 10), 1)]),
            ),
            (
                "customers".to_string(),
                ZSet::from_rows([(cust_row("a"), -1)]),
            ),
        ]);
        let q = count_by_region();
        let err = recompute_via_sqlite(&db, &q, &bases).unwrap_err();
        assert!(
            err.0.contains("customers") && err.0.contains("has a negative weight"),
            "it must be customers' negative-weight check that rejected this — an \
             unrelated SQL error that merely mentions customers (such as the \
             \"no such table\" a skipped CREATE TABLE causes) must not satisfy \
             this assertion: {}",
            err.0
        );
    }

    #[test]
    fn missing_base_state_for_a_declared_table_is_an_error() {
        let db = Database::new(vec![orders(), customers()]);
        let bases = BTreeMap::from([("orders".to_string(), ZSet::new())]);
        let err = recompute_via_sqlite(&db, &count_by_region(), &bases).unwrap_err();
        assert!(
            err.0.contains("customers"),
            "the error should name the missing table: {}",
            err.0
        );
    }
}
