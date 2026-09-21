//! 把 core 的 plan IR 渲染成 SQLite 能执行的 SQL。
//!
//! 这一层刻意留在 `ivmlite-test` 而不进 `ivmlite-core`：它存在的唯一理由是
//! 驱动 oracle（让 SQLite 自己算一遍当作权威判据）。core 拥有 IR，test 拥有
//! 「如何把这个 IR 表达成 SQL」。

use ivmlite_core::{AggFn, ColumnType, Predicate, Schema, ViewQuery};

fn sql_type(ty: ColumnType) -> &'static str {
    match ty {
        ColumnType::Integer => "INTEGER",
        ColumnType::Text => "TEXT",
    }
}

/// 生成 STRICT 建表语句（spec §7.1：STRICT 把列类型钉死，否则 type affinity
/// 会让一列逐行存不同类型，把一个逻辑上的 group key 拆成两个）。
pub fn create_table_sql(schema: &Schema) -> String {
    let cols: Vec<String> = schema
        .columns
        .iter()
        .map(|c| {
            let null = if c.nullable { "" } else { " NOT NULL" };
            format!("\"{}\" {}{}", c.name, sql_type(c.ty), null)
        })
        .collect();
    format!(
        "CREATE TABLE \"{}\" ({}) STRICT",
        schema.table,
        cols.join(", ")
    )
}

/// 把 ViewQuery 渲染成 SQL。列下标按 `schema` 解析。
pub fn view_query_to_sql(query: &ViewQuery, schema: &Schema) -> String {
    let name = |i: usize| format!("\"{}\"", schema.columns[i].name);

    let mut select: Vec<String> = query.group_by.iter().map(|i| name(*i)).collect();
    for agg in &query.aggs {
        select.push(match (agg.func, agg.column) {
            (AggFn::Count, _) => "COUNT(*)".to_string(),
            (AggFn::Sum, Some(i)) => format!("SUM({})", name(i)),
            (AggFn::Sum, None) => panic!("SUM 必须指定列"),
        });
    }

    let where_clause = match &query.predicate {
        Predicate::None => String::new(),
        Predicate::IntGt { column, value } => format!(" WHERE {} > {}", name(*column), value),
        Predicate::IsNotNull { column } => format!(" WHERE {} IS NOT NULL", name(*column)),
    };

    let group = query
        .group_by
        .iter()
        .map(|i| name(*i))
        .collect::<Vec<_>>()
        .join(", ");

    format!(
        "SELECT {} FROM \"{}\"{} GROUP BY {}",
        select.join(", "),
        schema.table,
        where_clause,
        group
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ivmlite_core::{Agg, Column};

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

    #[test]
    fn create_table_sql_is_strict() {
        let sql = create_table_sql(&orders());
        assert!(sql.contains("STRICT"), "spec §7.1 要求 STRICT table：{sql}");
        assert!(sql.contains("\"region\" TEXT"));
        assert!(sql.contains("\"amount\" INTEGER NOT NULL"));
    }

    #[test]
    fn to_sql_renders_group_by_and_aggs() {
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
        assert_eq!(
            view_query_to_sql(&q, &orders()),
            "SELECT \"region\", SUM(\"amount\"), COUNT(*) FROM \"orders\" GROUP BY \"region\""
        );
    }

    #[test]
    fn to_sql_renders_predicate() {
        let q = ViewQuery {
            group_by: vec![0],
            aggs: vec![Agg {
                func: AggFn::Count,
                column: None,
            }],
            predicate: Predicate::IntGt {
                column: 1,
                value: 3,
            },
        };
        assert!(view_query_to_sql(&q, &orders()).contains("WHERE \"amount\" > 3"));
    }

    /// item 13（deferred minor，与 I4 同源）：`IsNotNull` 此前完全没有单元
    /// 测试覆盖——`enumerate` 里生成它的整段循环删掉之后 67 个测试照样全绿,
    /// 正是因为连 `to_sql` 这一层都没人断言过它的输出。
    #[test]
    fn to_sql_renders_is_not_null_predicate() {
        let q = ViewQuery {
            group_by: vec![0],
            aggs: vec![Agg {
                func: AggFn::Count,
                column: None,
            }],
            predicate: Predicate::IsNotNull { column: 0 },
        };
        assert!(
            view_query_to_sql(&q, &orders()).contains("WHERE \"region\" IS NOT NULL"),
            "{}",
            view_query_to_sql(&q, &orders())
        );
    }
}
