//! Renders core's plan IR as SQL that SQLite can execute.
//!
//! This layer deliberately lives in `ivmlite-test`, not `ivmlite-core`: its only
//! reason to exist is driving the oracle (having SQLite compute the answer
//! itself as the authoritative judge). Core owns the IR; the test crate owns
//! "how to express that IR as SQL".

use ivmlite_core::{AggFn, CmpOp, ColumnType, Database, Predicate, Schema, Value, ViewQuery};

fn sql_type(ty: ColumnType) -> &'static str {
    match ty {
        ColumnType::Integer => "INTEGER",
        ColumnType::Text => "TEXT",
    }
}

fn sql_op(op: CmpOp) -> &'static str {
    match op {
        CmpOp::Gt => ">",
        CmpOp::Ge => ">=",
        CmpOp::Lt => "<",
        CmpOp::Le => "<=",
        CmpOp::Eq => "=",
        CmpOp::Ne => "!=",
    }
}

/// A literal as SQL. A TEXT literal is single-quoted, with each `'` doubled.
///
/// # Panics
/// On `Value::Null`, which `lower` rejects as a comparison literal.
fn sql_literal(value: &Value) -> String {
    match value {
        Value::Int(n) => n.to_string(),
        Value::Text(s) => format!("'{}'", s.replace('\'', "''")),
        Value::Null => panic!("a comparison literal is never NULL; lower rejects it"),
    }
}

/// Generate a STRICT CREATE TABLE statement (spec §7.1: STRICT pins column
/// types; otherwise type affinity lets a column store a different type in each
/// row, splitting one logical group key in two).
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

/// Render a ViewQuery as SQL against `db`.
///
/// Column indices refer to the query's row: the anchor table's (`db.tables()[0]`)
/// columns, then — for a join — the right table's. Every column is qualified
/// as `"table"."column"`, because the harness's tables share column names.
///
/// # Panics
/// If the query joins a table `db` does not declare. Only the oracle and the
/// harness's diff message call this, on queries `create_view` has already
/// accepted or that the enumerator produced.
pub fn view_query_to_sql(query: &ViewQuery, db: &Database) -> String {
    let anchor = &db.tables()[0];
    let right = query.join.as_ref().map(|j| {
        db.get(&j.right).unwrap_or_else(|| {
            panic!(
                "the query joins {}, which the database does not declare",
                j.right
            )
        })
    });
    let mut columns: Vec<(&str, &str)> = anchor
        .columns
        .iter()
        .map(|c| (anchor.table.as_str(), c.name.as_str()))
        .collect();
    if let Some(r) = right {
        columns.extend(
            r.columns
                .iter()
                .map(|c| (r.table.as_str(), c.name.as_str())),
        );
    }
    let name = |i: usize| format!("\"{}\".\"{}\"", columns[i].0, columns[i].1);

    let mut select: Vec<String> = query.group_by.iter().map(|i| name(*i)).collect();
    for agg in &query.aggs {
        select.push(match (agg.func, agg.column) {
            (AggFn::Count, _) => "COUNT(*)".to_string(),
            (AggFn::Sum, Some(i)) => format!("SUM({})", name(i)),
            (AggFn::Sum, None) => panic!("SUM must name a column"),
        });
    }

    let from = match (&query.join, right) {
        (Some(j), Some(r)) => format!(
            "\"{}\" JOIN \"{}\" ON {} = {}",
            anchor.table,
            r.table,
            name(j.left_column),
            name(anchor.arity() + j.right_column)
        ),
        _ => format!("\"{}\"", anchor.table),
    };

    let where_clause = match &query.predicate {
        Predicate::None => String::new(),
        Predicate::Compare { column, op, value } => {
            format!(
                " WHERE {} {} {}",
                name(*column),
                sql_op(*op),
                sql_literal(value)
            )
        }
        Predicate::IsNull { column } => format!(" WHERE {} IS NULL", name(*column)),
        Predicate::IsNotNull { column } => format!(" WHERE {} IS NOT NULL", name(*column)),
    };

    let group = query
        .group_by
        .iter()
        .map(|i| name(*i))
        .collect::<Vec<_>>()
        .join(", ");

    format!(
        "SELECT {} FROM {}{} GROUP BY {}",
        select.join(", "),
        from,
        where_clause,
        group
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{join_on_k, kv};
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
        assert!(
            sql.contains("STRICT"),
            "spec §7.1 requires a STRICT table: {sql}"
        );
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
            join: None,
        };
        assert_eq!(
            view_query_to_sql(&q, &Database::single(orders())),
            "SELECT \"orders\".\"region\", SUM(\"orders\".\"amount\"), COUNT(*) FROM \"orders\" GROUP BY \"orders\".\"region\""
        );
    }

    #[test]
    fn to_sql_renders_a_join() {
        let db = Database::new(vec![kv("t0"), kv("t1")]);
        let q = join_on_k();
        assert_eq!(
            view_query_to_sql(&q, &db),
            "SELECT \"t0\".\"k\", COUNT(*), SUM(\"t1\".\"v\") FROM \"t0\" JOIN \"t1\" \
             ON \"t0\".\"k\" = \"t1\".\"k\" GROUP BY \"t0\".\"k\""
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
            predicate: Predicate::Compare {
                column: 1,
                op: CmpOp::Gt,
                value: Value::Int(3),
            },
            join: None,
        };
        assert!(view_query_to_sql(&q, &Database::single(orders()))
            .contains("WHERE \"orders\".\"amount\" > 3"));
    }

    #[test]
    fn to_sql_renders_every_operator_text_literals_and_is_null() {
        let render = |predicate: Predicate| {
            view_query_to_sql(
                &ViewQuery {
                    group_by: vec![0],
                    aggs: vec![Agg {
                        func: AggFn::Count,
                        column: None,
                    }],
                    predicate,
                    join: None,
                },
                &Database::single(orders()),
            )
        };
        let expected = [
            (CmpOp::Gt, ">"),
            (CmpOp::Ge, ">="),
            (CmpOp::Lt, "<"),
            (CmpOp::Le, "<="),
            (CmpOp::Eq, "="),
            (CmpOp::Ne, "!="),
        ];
        for (op, symbol) in expected {
            let sql = render(Predicate::Compare {
                column: 1,
                op,
                value: Value::Int(-3),
            });
            assert!(
                sql.contains(&format!("WHERE \"orders\".\"amount\" {symbol} -3 ")),
                "{sql}"
            );
        }
        let sql = render(Predicate::Compare {
            column: 0,
            op: CmpOp::Eq,
            value: Value::Text("it's".into()),
        });
        assert!(
            sql.contains("WHERE \"orders\".\"region\" = 'it''s' "),
            "{sql}"
        );
        let sql = render(Predicate::IsNull { column: 0 });
        assert!(
            sql.contains("WHERE \"orders\".\"region\" IS NULL "),
            "{sql}"
        );
    }

    /// Item 13 (a deferred minor, from the same source as I4): `IsNotNull`
    /// used to have no unit-test coverage at all — deleting the whole loop in
    /// `enumerate` that generates it left the suite green, precisely because
    /// nobody asserted its output even at the `to_sql` level.
    #[test]
    fn to_sql_renders_is_not_null_predicate() {
        let q = ViewQuery {
            group_by: vec![0],
            aggs: vec![Agg {
                func: AggFn::Count,
                column: None,
            }],
            predicate: Predicate::IsNotNull { column: 0 },
            join: None,
        };
        assert!(
            view_query_to_sql(&q, &Database::single(orders()))
                .contains("WHERE \"orders\".\"region\" IS NOT NULL"),
            "{}",
            view_query_to_sql(&q, &Database::single(orders()))
        );
    }
}
