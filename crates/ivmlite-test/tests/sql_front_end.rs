//! M1b Phase 2b: the SQL front end against the harness's query space.

use ivmlite_core::{lower_query, ColumnType};
use ivmlite_sql::compile;
use ivmlite_test::{
    create_table_sql, enumerate, enumerate_join, gen_database,
    gen_database_with_swapped_right_table, view_query_to_sql, Database, ViewQuery,
};

fn every_query(db: &Database) -> Vec<ViewQuery> {
    let mut qs = enumerate(&db.tables()[0]);
    qs.extend(enumerate_join(&db.tables()[0], &db.tables()[1]));
    qs
}

fn databases() -> [Database; 2] {
    [gen_database(2), gen_database_with_swapped_right_table()]
}

/// The SQL a `ViewQuery` renders to compiles to the very `Plan` `lower_query`
/// builds from the `ViewQuery` itself — so every query the differential
/// harness verifies through `lower_query` is verified through SQL too.
#[test]
fn every_enumerated_query_compiles_to_the_plan_lower_query_builds() {
    for db in databases() {
        let queries = every_query(&db);
        assert_eq!(queries.len(), 153 + 900);
        for q in queries {
            let sql = view_query_to_sql(&q, &db);
            let compiled = compile(&sql, &db).unwrap_or_else(|e| panic!("{sql}: {e}"));
            assert_eq!(
                compiled.plan,
                lower_query(&q, &db).expect("an enumerated query lowers"),
                "{sql}"
            );
            let mut tables = vec![db.tables()[0].table.clone()];
            if q.join.is_some() {
                tables.push(db.tables()[1].table.clone());
            }
            assert_eq!(compiled.tables, tables, "{sql}");
        }
    }
}

/// Unaliased result columns get the names SQLite gives them (measured with
/// SQLite 3.53 through rusqlite): a bare column its declared name, an
/// aggregate its text exactly as written.
#[test]
fn result_column_names_match_sqlite() {
    let db = gen_database(2);
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    for t in db.tables() {
        conn.execute(&create_table_sql(t), []).unwrap();
    }
    let mut sqls: Vec<String> = every_query(&db)
        .iter()
        .map(|q| view_query_to_sql(q, &db))
        .collect();
    sqls.extend(
        [
            "SELECT K, count( * ), Sum(V) FROM T0 GROUP BY k",
            "SELECT t0.k AS region, sum(t0.v) total FROM t0 JOIN t1 ON t0.k = t1.k GROUP BY t0.k",
            "SELECT a.k, SUM ( b.\"v\" ) FROM t0 a JOIN t1 AS b ON b.k = a.k GROUP BY a.k",
            "SELECT\n  k,\n  COUNT(*)\nFROM t0\nGROUP BY k",
            // Final review Critical 1: a comment inside an aggregate call must
            // not unbalance the (now token-based) scanner, whatever quote or
            // bracket character it contains.
            "SELECT k, COUNT(/*(*/*) FROM t0 GROUP BY k",
            "SELECT k, COUNT(/*'*/*) FROM t0 GROUP BY k",
            "SELECT k, COUNT(* -- (\n) FROM t0 GROUP BY k",
            "SELECT k, SUM(v /* \" */) FROM t0 GROUP BY k",
            // Final review Important 2: shapes the differential harness never
            // renders, where SQLite's naming and the old `call_text` disagreed.
            "SELECT k, (COUNT(*)) FROM t0 GROUP BY k",
            "SELECT k, ( SUM(v) ) FROM t0 GROUP BY k",
            "SELECT k, COUNT(/* ) */ *) FROM t0 GROUP BY k",
            "SELECT k, COUNT(*) -- )\nFROM t0 GROUP BY k",
            "SELECT k, COUNT(*) -- hi\r\nFROM t0 GROUP BY k",
            "SELECT k, COUNT(*) -- hi  \nFROM t0 GROUP BY k",
            // Final review Important 2: a comment before an item is not part
            // of its name (measured against SQLite 3.53).
            "SELECT k, /*c*/ COUNT(*) FROM t0 GROUP BY k",
        ]
        .map(String::from),
    );
    for sql in sqls {
        let expected: Vec<String> = conn
            .prepare(&sql)
            .unwrap_or_else(|e| panic!("{sql}: {e}"))
            .column_names()
            .into_iter()
            .map(String::from)
            .collect();
        let compiled = compile(&sql, &db).unwrap_or_else(|e| panic!("{sql}: {e}"));
        let names: Vec<String> = compiled.columns.iter().map(|c| c.name.clone()).collect();
        assert_eq!(names, expected, "{sql}");
    }
}

/// Final review recommendation: an output column's type and nullability
/// (Ruling 4) for a join query, not just its plan — a group key's are its
/// table column's, `COUNT(*)` is a non-null INTEGER, `SUM` is a nullable
/// INTEGER (SUM over only NULLs is NULL, spec §6.1). `gen_database`'s
/// columns are both nullable (`k TEXT`, `v INTEGER`), so the group key here
/// is expected nullable too — it is not `COUNT(*)`'s non-null INTEGER that
/// would pass by accident.
#[test]
fn join_output_column_types_and_nullability_match_ruling_4() {
    let db = gen_database(2);
    let sql = "SELECT t1.k, SUM(t0.v), COUNT(*) FROM t0 JOIN t1 ON t0.k = t1.k GROUP BY t1.k";
    let compiled = compile(sql, &db).unwrap_or_else(|e| panic!("{sql}: {e}"));
    let types: Vec<(ColumnType, bool)> = compiled
        .columns
        .iter()
        .map(|c| (c.ty, c.nullable))
        .collect();
    assert_eq!(
        types,
        vec![
            (ColumnType::Text, true),     // t1.k: the group key's own column
            (ColumnType::Integer, true),  // SUM(t0.v): nullable
            (ColumnType::Integer, false), // COUNT(*): never NULL
        ],
        "{sql}"
    );
}
