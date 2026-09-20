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
            .map_err(|e| EngineError(format!("非 UTF-8 文本: {e}"))),
        ValueRef::Real(_) => Err(EngineError("v0 不支持 REAL".into())),
        ValueRef::Blob(_) => Err(EngineError("v0 不支持 BLOB".into())),
    }
}

/// 权威判据：把全部基表状态灌进内存 SQLite，让 SQLite 自己执行原始 SQL。
///
/// 与本项目全部代码无关的独立实现——用 NaiveRecompute 判 NaiveRecompute
/// 是循环论证，这正是它单独存在的理由。
pub fn recompute_via_sqlite(
    db: &Database,
    query: &ViewQuery,
    bases: &BTreeMap<String, ZSet>,
) -> Result<ZSet, EngineError> {
    let conn = Connection::open_in_memory().map_err(|e| EngineError(e.to_string()))?;

    for schema in db.tables() {
        conn.execute_batch(&create_table_sql(schema))
            .map_err(|e| EngineError(e.to_string()))?;

        let base = bases
            .get(&schema.table)
            .ok_or_else(|| EngineError(format!("缺少基表 {} 的状态", schema.table)))?;

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
                    "基表 {} 的状态含负权重 {weight}，行 {row:?}",
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

    // 视图 SQL 目前仍按单表渲染；join 查询的渲染在引擎计划的 Phase 3 加入。
    let anchor = db
        .tables()
        .first()
        .ok_or_else(|| EngineError("database 为空".into()))?;
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
        }
    }

    fn single_base(table: &str, base: ZSet) -> BTreeMap<String, ZSet> {
        BTreeMap::from([(table.to_string(), base)])
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
        };
        let base = ZSet::from_rows([(row(Value::Text("a".into()), 1), 3)]);

        let db = Database::single(orders());
        let bases = single_base("orders", base);
        let got = recompute_via_sqlite(&db, &q, &bases).unwrap();
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

        let db = Database::single(orders());
        let bases = single_base("orders", base);
        assert!(
            recompute_via_sqlite(&db, &q, &bases).is_err(),
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
            "SQLite 的 SUM 在无非 NULL 输入时返回 NULL"
        );

        let mut e = NaiveRecompute::new();
        e.create_view(&nullable_amount, &q, &base).unwrap();
        assert_eq!(e.materialize().unwrap(), want);
    }

    #[test]
    fn builds_every_table_in_the_database() {
        // 两张表，查询只涉及 orders；customers 必须也被建表并校验其基表
        // 状态，即使查询从不读它——否则 join 查询（Phase 3）会在 oracle 侧
        // 静默少一张表。
        //
        // 用负权重当探针：customers 的负权重只有在它真的被遍历（建表 +
        // 校验基表状态）时才会被拒绝。断言不能只看查询结果——查询目前仍
        // 只按 anchor（db.tables()[0] == orders）渲染，customers 从不出现
        // 在 SQL 里，所以一个只建 tables()[0] 的实现会算出一模一样的
        // `got`，那样的断言测不出任何区别。
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
            err.0.contains("customers"),
            "customers 的负权重必须被拒绝——这证明该表确实被建出来并校验过：{}",
            err.0
        );
    }

    #[test]
    fn missing_base_state_for_a_declared_table_is_an_error() {
        let db = Database::new(vec![orders(), customers()]);
        let bases = BTreeMap::from([("orders".to_string(), ZSet::new())]);
        let err = recompute_via_sqlite(&db, &count_by_region(), &bases).unwrap_err();
        assert!(err.0.contains("customers"), "错误应指名缺哪张表：{}", err.0);
    }
}
