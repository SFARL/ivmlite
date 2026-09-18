use ivmlite_core::{Row, Value, ZSet};
use rusqlite::types::{ToSqlOutput, ValueRef};
use rusqlite::{Connection, ToSql};

use crate::{EngineError, Schema, ViewQuery};

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
            .map_err(|e| EngineError(format!("非 UTF-8 文本: {e}"))),
        ValueRef::Real(_) => Err(EngineError("v0 不支持 REAL".into())),
        ValueRef::Blob(_) => Err(EngineError("v0 不支持 BLOB".into())),
    }
}

/// 权威判据：把基表状态灌进内存 SQLite，让 SQLite 自己执行原始 SQL。
///
/// 这是一个与本项目全部代码无关的独立实现，因此可以用来判定
/// NaiveRecompute 与未来的真实引擎是否正确。
pub fn recompute_via_sqlite(
    schema: &Schema,
    query: &ViewQuery,
    base: &ZSet,
) -> Result<ZSet, EngineError> {
    let conn = Connection::open_in_memory().map_err(|e| EngineError(e.to_string()))?;
    conn.execute_batch(&schema.create_table_sql())
        .map_err(|e| EngineError(e.to_string()))?;

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

    {
        let mut stmt = conn
            .prepare(&insert_sql)
            .map_err(|e| EngineError(e.to_string()))?;
        for (row, weight) in base.iter() {
            if *weight < 0 {
                return Err(EngineError(format!(
                    "基表状态含负权重 {weight}，行 {row:?}"
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

    let sql = query.to_sql(schema);
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
    use crate::{Agg, AggFn, Column, ColumnType, Predicate, Schema, ViewQuery};

    fn schema() -> Schema {
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

    fn row(region: Value, amount: i64) -> Row {
        Row::new(vec![region, Value::Int(amount)])
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
        };
        let base = ZSet::from_rows([
            (row(Value::Text("a".into()), 10), 1),
            (row(Value::Text("a".into()), 5), 1),
            (row(Value::Text("b".into()), 3), 1),
        ]);

        let got = recompute_via_sqlite(&schema(), &q, &base).unwrap();
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
        };
        let base = ZSet::from_rows([(row(Value::Text("a".into()), 1), 3)]);

        let got = recompute_via_sqlite(&schema(), &q, &base).unwrap();
        assert_eq!(
            got.weight_of(&Row::new(vec![Value::Text("a".into()), Value::Int(3)])),
            1,
            "权重 3 应展开为 3 行，COUNT(*) 得 3"
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
        };
        let base = ZSet::from_rows([(row(Value::Text("a".into()), 1), -1)]);
        assert!(
            recompute_via_sqlite(&schema(), &q, &base).is_err(),
            "基表状态出现负权重说明上游已经错了，oracle 必须拒绝而非静默"
        );
    }

    /// 钉死 spec §6.1 的 NULL 语义契约，并让 NaiveRecompute 与 SQLite 对齐。
    /// Task 7 已经单独断言过 NaiveRecompute 的行为，这里补上 SQLite 的背书。
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
        };
        let base = ZSet::from_rows([(Row::new(vec![Value::Text("a".into()), Value::Null]), 2)]);

        let want = recompute_via_sqlite(&nullable_amount, &q, &base).unwrap();
        assert_eq!(
            want.weight_of(&Row::new(vec![
                Value::Text("a".into()),
                Value::Null,
                Value::Int(2)
            ])),
            1,
            "SQLite 的 SUM 在无非 NULL 输入时返回 NULL"
        );

        let mut e = NaiveRecompute::new();
        e.create_view(&nullable_amount, &q, &base).unwrap();
        assert_eq!(e.materialize().unwrap(), want);
    }
}
