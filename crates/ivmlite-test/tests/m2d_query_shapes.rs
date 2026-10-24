//! M2d: typed SQLite scalar semantics and physical row bags.
mod common;
use common::{count, create, refresh, rows, TempFile};
use ivmlite_test::open_with_extension;
use rusqlite::{params, types::Value, Connection};

/// Compare full storage-class identities, including each REAL's exact bits.
fn assert_matches_oracle(c: &Connection, name: &str, sql: &str) {
    fn exact(c: &Connection, sql: &str) -> Vec<ivmlite_core::Row> {
        let mut stmt = c.prepare(sql).unwrap();
        let n = stmt.column_count();
        let mut rows = stmt
            .query_map([], |r| {
                (0..n)
                    .map(|i| {
                        r.get::<_, Value>(i).map(|value| match value {
                            Value::Null => ivmlite_core::Value::Null,
                            Value::Integer(n) => ivmlite_core::Value::Int(n),
                            Value::Real(x) => ivmlite_core::Value::Real(x),
                            Value::Text(t) => ivmlite_core::Value::Text(t),
                            Value::Blob(b) => ivmlite_core::Value::Blob(b),
                        })
                    })
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map(ivmlite_core::Row::new)
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        rows.sort();
        rows
    }
    assert_eq!(
        exact(c, &format!("SELECT * FROM {name}")),
        exact(c, sql),
        "{name}: {sql}"
    );
}

#[test]
fn four_shapes_bootstrap_and_refresh() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch("CREATE TABLE traffic(user_id INTEGER, up_bytes INTEGER, down_bytes INTEGER, active INTEGER, deleted_at TEXT); INSERT INTO traffic VALUES (1,2,3,1,NULL),(1,4,5,1,NULL),(2,6,7,1,NULL),(2,0,1,0,NULL);").unwrap();
    let sqls=[
        "SELECT user_id, SUM(up_bytes + down_bytes) AS bytes FROM traffic WHERE active = 1 AND deleted_at IS NULL GROUP BY user_id",
        "SELECT user_id, up_bytes * 0.5 AS converted FROM traffic WHERE up_bytes > 0 AND user_id IS NOT NULL",
        "SELECT SUM(up_bytes) + 1 AS adjusted, user_id FROM traffic GROUP BY user_id",
        "SELECT COUNT(*) AS n FROM traffic GROUP BY user_id",
    ];
    for (i, sql) in sqls.iter().enumerate() {
        let name = format!("v{i}");
        create(&c, &name, sql).unwrap();
        assert_matches_oracle(&c, &name, sql);
    }
    c.execute_batch("UPDATE traffic SET active=0 WHERE up_bytes=2; DELETE FROM traffic WHERE user_id=2; INSERT INTO traffic VALUES (3,1,1,1,NULL),(3,2,2,1,NULL);").unwrap();
    for (i, sql) in sqls.iter().enumerate() {
        let name = format!("v{i}");
        refresh(&c, &name).unwrap();
        assert_matches_oracle(&c, &name, sql);
    }
}

#[test]
fn scalar_projection_matches_host_sqlite_typed_rows() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY, a BLOB, b BLOB);")
        .unwrap();
    let values = [
        Value::Null,
        Value::Integer(0),
        Value::Integer(1),
        Value::Integer(-1),
        Value::Integer(i64::MAX),
        Value::Integer(i64::MIN),
        Value::Integer(9_007_199_254_740_993),
        Value::Real(1.0),
        Value::Real(0.5),
        Value::Real(-0.0),
        Value::Text("9007199254740993".into()),
        Value::Text("1e999".into()),
        Value::Real(f64::INFINITY),
        Value::Real(f64::NEG_INFINITY),
        Value::Text("12.5tail".into()),
        Value::Text("abc".into()),
        Value::Text("2\0tail".into()),
        Value::Blob(b"12\0tail".to_vec()),
        Value::Blob(vec![0, 255]),
    ];
    let mut id = 0;
    for a in &values {
        for b in &values {
            id += 1;
            c.execute("INSERT INTO t VALUES (?1,?2,?3)", params![id, a, b])
                .unwrap();
        }
    }
    let sql="SELECT id, a AS raw, +a AS pos, -a AS neg, a+b AS addv, a-b AS subv, a*b AS mulv, a/b AS divv, a%b AS remv, (a+b)*2 AS parens, -9223372036854775808 AS minv FROM t";
    create(&c, "v", sql).unwrap();
    assert_matches_oracle(&c, "v", sql);
    c.execute_batch("DELETE FROM t WHERE id%3=0; UPDATE t SET a=b,b=a WHERE id%5=0;")
        .unwrap();
    refresh(&c, "v").unwrap();
    assert_matches_oracle(&c, "v", sql);
}

#[test]
fn duplicate_projection_retracts_exact_copies_for_both_trigger_settings() {
    for recursive in [0, 1] {
        let c = open_with_extension(None).unwrap();
        c.execute_batch(&format!("PRAGMA recursive_triggers={recursive}; CREATE TABLE t(id INTEGER PRIMARY KEY,v BLOB); INSERT INTO t(v) VALUES (1),(1),(1),(1.0),(1.0),(NULL),(NULL),(X'00'),(X'00');")).unwrap();
        let sql = "SELECT v AS x FROM t";
        create(&c, "v", sql).unwrap();
        assert_matches_oracle(&c, "v", sql);
        let stable = rows(&c, "SELECT rowid FROM __ivm_out_v WHERE typeof(x)='real'");
        c.execute_batch("DELETE FROM t WHERE id IN (1,2,6,8);")
            .unwrap();
        refresh(&c, "v").unwrap();
        assert_matches_oracle(&c, "v", sql);
        assert_eq!(
            stable,
            rows(&c, "SELECT rowid FROM __ivm_out_v WHERE typeof(x)='real'")
        );
        assert_eq!(
            count(&c, "SELECT count(*) FROM __ivm_out_v WHERE __w!=1"),
            0
        );
        c.execute_batch("DELETE FROM t WHERE id=4;").unwrap();
        refresh(&c, "v").unwrap();
        assert_matches_oracle(&c, "v", sql);
        c.execute_batch("DELETE FROM t;").unwrap();
        refresh(&c, "v").unwrap();
        assert_matches_oracle(&c, "v", sql);
    }
}

#[test]
fn aggregate_arguments_all_functions_join_and_hidden_group_collisions() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch("CREATE TABLE l(k INTEGER,a BLOB,b BLOB); CREATE TABLE r(k INTEGER,x INTEGER,enabled NUMERIC); INSERT INTO l VALUES (1,2,3),(1,4,5),(2,2,3),(3,NULL,2); INSERT INTO r VALUES (1,10,1),(1,10,1),(2,10,1),(3,10,1);").unwrap();
    let sql="SELECT COUNT(l.a+l.b) AS c, SUM(l.a+l.b) AS s, TOTAL(l.a+l.b) AS t, AVG(l.a+l.b) AS a, MIN(l.a+l.b) AS mi, MAX(l.a+l.b) AS ma, SUM(l.a+l.b)+r.k AS adjusted, r.k AS again, r.k AS repeated FROM l JOIN r ON l.k=r.k WHERE l.a IS NOT NULL AND r.enabled='1' GROUP BY r.k";
    create(&c, "mv", sql).unwrap();
    assert_matches_oracle(&c, "mv", sql);
    let collision = "SELECT COUNT(*)/10 AS bucket FROM l JOIN r ON l.k=r.k GROUP BY r.k";
    create(&c, "collisions", collision).unwrap();
    assert_matches_oracle(&c, "collisions", collision);
    assert_eq!(
        count(&c, "SELECT count(*) FROM collisions WHERE bucket=0"),
        3
    );
    c.execute_batch("UPDATE l SET a=6,b=2 WHERE k=1; DELETE FROM r WHERE k=2; INSERT INTO r VALUES (1,20,1); UPDATE r SET enabled=0 WHERE x=20;").unwrap();
    refresh(&c, "mv").unwrap();
    refresh(&c, "collisions").unwrap();
    assert_matches_oracle(&c, "mv", sql);
    assert_matches_oracle(&c, "collisions", collision);
    c.execute_batch("DELETE FROM l WHERE k=1;").unwrap();
    refresh(&c, "mv").unwrap();
    refresh(&c, "collisions").unwrap();
    assert_matches_oracle(&c, "mv", sql);
    assert_matches_oracle(&c, "collisions", collision);
}

#[test]
fn and_filters_validate_and_convert_every_leaf_on_both_join_sides() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch("CREATE TABLE l(k INTEGER,n NUMERIC,s TEXT,v BLOB); CREATE TABLE r(k INTEGER,realv REAL,flag INTEGER); INSERT INTO l VALUES (1,2,'2',2),(1,NULL,NULL,NULL),(2,2,'02',X'32'),(3,2,'2',2),(4,2,'2',2); INSERT INTO r VALUES (1,2,1),(2,2,1),(3,NULL,1),(4,2,NULL);").unwrap();
    let conditions = [
        "l.n='2' AND l.s=2 AND r.realv='2.0' AND r.flag IS NOT NULL",
        "(l.n IS NULL AND l.s IS NULL) AND (r.flag=1 AND r.k>0)",
        "l.n=2 AND l.n!=2",
        "l.n=2 AND (r.realv>0 AND 2>=r.realv)",
        "l.v=2 AND r.realv=2 AND l.s='2'",
    ];
    for (i, condition) in conditions.iter().enumerate() {
        let sql =
            format!("SELECT l.k AS keyv,l.v AS val FROM l JOIN r ON l.k=r.k WHERE {condition}");
        let name = format!("mv{i}");
        create(&c, &name, &sql).unwrap();
        assert_matches_oracle(&c, &name, &sql);
    }
    c.execute_batch("UPDATE l SET n=2,s='2' WHERE n IS NULL; UPDATE r SET realv=2 WHERE realv IS NULL; UPDATE r SET flag=1 WHERE flag IS NULL;").unwrap();
    for (i, condition) in conditions.iter().enumerate() {
        let sql =
            format!("SELECT l.k AS keyv,l.v AS val FROM l JOIN r ON l.k=r.k WHERE {condition}");
        let name = format!("mv{i}");
        refresh(&c, &name).unwrap();
        assert_matches_oracle(&c, &name, &sql);
    }
    for bad in [
        "l.n=2 AND l.s=2.5",
        "l.n=2 AND r.flag=NULL",
        "l.n=2 AND nope=1",
        "l.n+1>0 AND r.flag=1",
        "l.n=2 OR r.flag=1",
    ] {
        let before = common::objects(&c);
        let err = create(
            &c,
            "bad",
            &format!("SELECT l.k FROM l JOIN r ON l.k=r.k WHERE {bad}"),
        )
        .unwrap_err();
        assert!(!err.to_string().is_empty());
        assert_eq!(before, common::objects(&c));
    }
}

#[test]
fn joined_fanout_bags_and_wildcards_follow_both_side_changes() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch("CREATE TABLE l(id INTEGER PRIMARY KEY,k BLOB,p INTEGER); CREATE TABLE r(rid INTEGER PRIMARY KEY,k BLOB,q INTEGER); INSERT INTO l VALUES (1,1,2),(2,1,2),(3,2,3); INSERT INTO r VALUES (10,1,4),(11,1,4),(12,2,5);").unwrap();
    let sql = "SELECT l.p+r.q AS total FROM l JOIN r ON l.k=r.k";
    let star = "SELECT l.*,r.rid,r.q FROM l JOIN r ON l.k=r.k";
    create(&c, "mv", sql).unwrap();
    create(&c, "stars", star).unwrap();
    assert_matches_oracle(&c, "mv", sql);
    assert_matches_oracle(&c, "stars", star);
    assert_eq!(count(&c, "SELECT count(*) FROM mv WHERE total=6"), 4);
    c.execute_batch("DELETE FROM l WHERE id=1; DELETE FROM r WHERE rid=10; INSERT INTO l VALUES (4,1,2),(5,1,2); INSERT INTO r VALUES (13,1,4),(14,1,4);").unwrap();
    refresh(&c, "mv").unwrap();
    refresh(&c, "stars").unwrap();
    assert_matches_oracle(&c, "mv", sql);
    assert_matches_oracle(&c, "stars", star);
    c.execute_batch("DELETE FROM l WHERE k=1; DELETE FROM r WHERE k=1;")
        .unwrap();
    refresh(&c, "mv").unwrap();
    refresh(&c, "stars").unwrap();
    assert_matches_oracle(&c, "mv", sql);
    assert_matches_oracle(&c, "stars", star);
    let simple = "SELECT * FROM l";
    create(&c, "allcols", simple).unwrap();
    assert_matches_oracle(&c, "allcols", simple);
}

fn maintenance_state(c: &Connection, name: &str) -> Vec<Vec<Vec<Value>>> {
    let mut data = common::durable_state(c, name);
    let deltas = rows(
        c,
        "SELECT name FROM sqlite_schema WHERE type='table' AND name LIKE '__ivm_delta_%'",
    );
    for row in deltas {
        if let Value::Text(table) = &row[0] {
            data.push(rows(c, &format!("SELECT * FROM \"{table}\"")));
        }
    }
    data
}

#[test]
fn count_underflow_rolls_back_state_output_progress_gc_and_retries() {
    for recursive in [0, 1] {
        let c = open_with_extension(None).unwrap();
        c.execute_batch(&format!("PRAGMA recursive_triggers={recursive}; CREATE TABLE t(k INTEGER,a INTEGER,b INTEGER); INSERT INTO t VALUES (1,2,3),(2,2,3); ")).unwrap();
        let sql = "SELECT SUM(a+b) AS total FROM t GROUP BY k";
        create(&c, "mv", sql).unwrap();
        c.execute_batch("CREATE TEMP TABLE saved AS SELECT * FROM __ivm_out_mv; DELETE FROM __ivm_out_mv WHERE rowid=(SELECT min(rowid) FROM __ivm_out_mv); DELETE FROM t;").unwrap();
        let before = maintenance_state(&c, "mv");
        let err = refresh(&c, "mv").unwrap_err();
        assert!(
            err.to_string().contains("retraction exceeds stored copies"),
            "{err}"
        );
        assert_eq!(before, maintenance_state(&c, "mv"));
        assert_eq!(
            count(&c, "SELECT count(*) FROM __ivm_stage_mv WHERE op='bag-'"),
            0,
            "the final statement rolls op updates back too"
        );
        c.execute_batch("DELETE FROM __ivm_out_mv; INSERT INTO __ivm_out_mv SELECT * FROM saved;")
            .unwrap();
        refresh(&c, "mv").unwrap();
        assert_matches_oracle(&c, "mv", sql);
        assert_eq!(count(&c, "SELECT count(*) FROM __ivm_delta_t"), 0);
    }
}

#[test]
fn bag_lifecycle_rollback_savepoint_pending_reopen_and_shared_capture() {
    let file = TempFile::new("m2d-lifecycle");
    let sql = "SELECT v+0 AS result FROM t WHERE active=1 AND v IS NOT NULL";
    {
        let c = open_with_extension(Some(file.path())).unwrap();
        c.execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY,v BLOB,active INTEGER); INSERT INTO t VALUES (1,2,1),(2,2,1),(3,3,0);").unwrap();
        create(&c, "mv", sql).unwrap();
        create(
            &c,
            "sibling",
            "SELECT active,COUNT(*) AS n FROM t GROUP BY active",
        )
        .unwrap();
        let stable = rows(&c, "SELECT rowid,* FROM __ivm_out_mv");
        c.execute_batch("BEGIN; DELETE FROM t WHERE id=1;").unwrap();
        refresh(&c, "mv").unwrap();
        assert_matches_oracle(&c, "mv", sql);
        c.execute_batch("ROLLBACK").unwrap();
        assert_eq!(stable, rows(&c, "SELECT rowid,* FROM __ivm_out_mv"));
        c.execute_batch("BEGIN; SAVEPOINT a; UPDATE t SET v=7 WHERE id=1;")
            .unwrap();
        refresh(&c, "mv").unwrap();
        assert_matches_oracle(&c, "mv", sql);
        c.execute_batch("ROLLBACK TO a; RELEASE a; COMMIT;")
            .unwrap();
        assert_eq!(stable, rows(&c, "SELECT rowid,* FROM __ivm_out_mv"));
        c.execute_batch("DELETE FROM t WHERE id=1; UPDATE t SET active=1 WHERE id=3; INSERT INTO t VALUES (4,2,1),(5,2,1); INSERT INTO t VALUES (6,8,1); DELETE FROM t WHERE id=6;").unwrap();
        assert_eq!(
            count(&c, "SELECT count(*) FROM mv"),
            2,
            "refresh is explicit"
        );
    }
    {
        let c = open_with_extension(Some(file.path())).unwrap();
        refresh(&c, "mv").unwrap();
        assert_matches_oracle(&c, "mv", sql);
        assert!(
            count(&c, "SELECT count(*) FROM __ivm_delta_t") > 0,
            "sibling still needs capture"
        );
        refresh(&c, "sibling").unwrap();
        assert_eq!(count(&c, "SELECT count(*) FROM __ivm_delta_t"), 0);
        c.execute_batch("DROP TABLE mv;").unwrap();
        assert_eq!(count(&c,"SELECT count(*) FROM sqlite_schema WHERE name IN ('__ivm_bagidx_mv','__ivm_bagretract_mv')"),0);
        create(&c, "mv", sql).unwrap();
        assert_matches_oracle(&c, "mv", sql);
    }
}

#[test]
fn missing_and_forged_bag_objects_fail_on_refresh_and_reopen() {
    for (tag,corrupt,expected) in [
        ("missing-index","DROP INDEX __ivm_bagidx_mv","bag index"),
        ("wrong-index-owner","DROP INDEX __ivm_bagidx_mv; CREATE INDEX __ivm_bagidx_mv ON other(x)","on table"),
        ("wrong-index-column","DROP INDEX __ivm_bagidx_mv; CREATE INDEX __ivm_bagidx_mv ON __ivm_out_mv(result)","typed identity"),
        ("missing-trigger","DROP TRIGGER __ivm_bagretract_mv","bag trigger"),
        ("wrong-trigger-owner","DROP TRIGGER __ivm_bagretract_mv; CREATE TRIGGER __ivm_bagretract_mv AFTER UPDATE ON other BEGIN SELECT 1; END","on table"),
        ("wrong-trigger-body","DROP TRIGGER __ivm_bagretract_mv; CREATE TRIGGER __ivm_bagretract_mv AFTER UPDATE OF op ON __ivm_stage_mv WHEN NEW.op='bag-' BEGIN SELECT 1; END","unexpected content"),
    ] {
        let file=TempFile::new(&format!("m2d-{tag}"));
        {
            let c=open_with_extension(Some(file.path())).unwrap();c.execute_batch("CREATE TABLE t(x INTEGER); CREATE TABLE other(x INTEGER); INSERT INTO t VALUES (1),(1);").unwrap();create(&c,"mv","SELECT x AS result FROM t").unwrap();c.execute_batch(corrupt).unwrap();c.execute_batch("DELETE FROM t;").unwrap();
            let before=maintenance_state(&c,"mv");let err=refresh(&c,"mv").unwrap_err();assert!(err.to_string().contains(expected),"{tag}: {err}");assert_eq!(before,maintenance_state(&c,"mv"));
        }
        let c=open_with_extension(Some(file.path())).unwrap();let err=c.execute_batch("SELECT * FROM mv").unwrap_err();assert!(err.to_string().contains(expected),"{tag}: {err}");c.execute_batch("DROP TABLE mv").unwrap();assert_eq!(count(&c,"SELECT count(*) FROM sqlite_schema WHERE name IN ('__ivm_bagidx_mv','__ivm_bagretract_mv')"),0);
    }
}

#[test]
fn format6_is_refused_without_mutation_and_can_be_dropped_and_rebuilt() {
    let file = TempFile::new("m2d-format6");
    {
        let c = open_with_extension(Some(file.path())).unwrap();
        c.execute_batch("CREATE TABLE t(k INTEGER,v INTEGER); INSERT INTO t VALUES (1,2)")
            .unwrap();
        create(&c, "mv", "SELECT k,SUM(v) FROM t GROUP BY k").unwrap();
        c.execute_batch("UPDATE __ivm_meta SET value=6 WHERE key='format'; UPDATE __ivm_view SET format=6; DROP INDEX __ivm_progress_tbl;").unwrap();
        let objects = common::objects(&c);
        let data = maintenance_state(&c, "mv");
        let err = create(&c, "another", "SELECT k FROM t").unwrap_err();
        assert!(err.to_string().contains("format 6") && err.to_string().contains("reads format 7"));
        assert_eq!(objects, common::objects(&c));
        assert_eq!(data, maintenance_state(&c, "mv"));
    }
    let c = open_with_extension(Some(file.path())).unwrap();
    let err = c.execute_batch("SELECT * FROM mv").unwrap_err();
    assert!(err.to_string().contains("format 6"));
    c.execute_batch("DROP TABLE mv").unwrap();
    create(&c, "mv", "SELECT k,v FROM t").unwrap();
    assert_matches_oracle(&c, "mv", "SELECT k,v FROM t");
}

#[test]
fn unsafe_scalar_forms_and_internal_aliases_are_rejected_without_objects() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch("CREATE TABLE t(k INTEGER,a INTEGER,b INTEGER)")
        .unwrap();
    for sql in [
        "SELECT SUM(a+b) FROM t",
        "SELECT k FROM t GROUP BY k",
        "SELECT k,SUM(SUM(a)) FROM t GROUP BY k",
        "SELECT k,CASE WHEN a=1 THEN 2 END FROM t",
        "SELECT abs(a) FROM t",
        "SELECT CAST(a AS TEXT) FROM t",
        "SELECT a||b FROM t",
        "SELECT a COLLATE BINARY FROM t",
        "SELECT ?1 FROM t",
        "SELECT a AS __IVM_identity FROM t",
        "SELECT a AS rowid FROM t",
        "SELECT a AS __w FROM t",
        "SELECT a AS x,b AS x FROM t",
        "SELECT a+1 FROM t WHERE a+1>2",
        "SELECT k,COUNT(*) FROM t GROUP BY k+1",
        "SELECT k,COUNT(*) FROM t WHERE a=1 AND b=NULL GROUP BY k",
    ] {
        let before = common::objects(&c);
        create(&c, "mv", sql).unwrap_err();
        assert_eq!(before, common::objects(&c), "{sql}");
    }
}

#[test]
fn bag_and_postaggregate_roots_use_full_cursor_snapshots_with_residual_sql() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch("CREATE TABLE t(k BLOB,v INTEGER); INSERT INTO t VALUES (1,2),(1.5,2),('1',2),(NULL,2),(X'31',2);").unwrap();
    let row = "SELECT k AS keyv,v AS val FROM t";
    let grouped = "SELECT SUM(v) AS val,k AS keyv FROM t GROUP BY k";
    for (name, sql) in [("bag", row), ("post", grouped)] {
        create(&c, name, sql).unwrap();
        let plan=rows(&c,&format!("EXPLAIN QUERY PLAN SELECT * FROM {name} WHERE keyv=1 AND val>0 ORDER BY keyv LIMIT 2"));
        assert!(
            format!("{plan:?}").contains("VIRTUAL TABLE INDEX 0:"),
            "{plan:?}"
        );
        c.execute_batch(&format!("CREATE TEMP TABLE expected AS {sql}"))
            .unwrap();
        for tail in [
            "WHERE keyv=1",
            "WHERE keyv='1'",
            "WHERE keyv IS NULL",
            "WHERE keyv>0 ORDER BY val LIMIT 2",
        ] {
            assert_eq!(
                rows(
                    &c,
                    &format!("SELECT typeof(keyv),keyv,val FROM {name} {tail}")
                ),
                rows(
                    &c,
                    &format!("SELECT typeof(keyv),keyv,val FROM expected {tail}")
                )
            );
        }
        c.execute_batch("DROP TABLE expected").unwrap();
    }
    let old = rows(&c, "SELECT rowid,keyv,val FROM bag");
    let mut live_stmt = c.prepare("SELECT rowid,keyv,val FROM bag").unwrap();
    let mut live = live_stmt.query([]).unwrap();
    let mut seen = Vec::new();
    let r = live.next().unwrap().unwrap();
    seen.push(
        (0..3)
            .map(|i| r.get::<_, Value>(i).unwrap())
            .collect::<Vec<_>>(),
    );
    c.execute_batch("DELETE FROM t; INSERT INTO t VALUES (2,8),(2,8)")
        .unwrap();
    refresh(&c, "bag").unwrap();
    while let Some(r) = live.next().unwrap() {
        seen.push((0..3).map(|i| r.get::<_, Value>(i).unwrap()).collect());
    }
    seen.sort_by_key(|r| format!("{r:?}"));
    assert_eq!(seen, old);
    assert_matches_oracle(&c, "bag", row);
}

#[test]
fn aggregate_expression_overflow_null_and_infinity_preserve_the_aggregate_contract() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch("CREATE TABLE t(k INTEGER,a BLOB,b BLOB); INSERT INTO t VALUES (1,9223372036854775807,1),(2,-9223372036854775808,-1),(3,1,0),(3,NULL,1),(4,1e999,1),(5,-1e999,1),(6,1e999,-1e999);").unwrap();
    let sql="SELECT k,SUM(a+b) AS s,TOTAL(a+b) AS t,AVG(a+b) AS av,COUNT(a/b) AS n,MIN(a%b) AS mi,MAX(-a) AS ma FROM t GROUP BY k";
    create(&c, "mv", sql).unwrap();
    assert_matches_oracle(&c, "mv", sql);
    c.execute_batch("UPDATE t SET a=2,b=0 WHERE k IN (1,2,4,5); DELETE FROM t WHERE k=6")
        .unwrap();
    refresh(&c, "mv").unwrap();
    assert_matches_oracle(&c, "mv", sql);
}

#[test]
fn literal_projections_preserve_types_and_multiple_identical_copies() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch("CREATE TABLE t(x INTEGER); INSERT INTO t VALUES (1),(2),(3)")
        .unwrap();
    let sql="SELECT 'a''b' AS textv,X'00ff' AS blobv,NULL AS nullv,1 AS intv,1.0 AS realv,+9223372036854775808 AS bigv,-(-9223372036854775808) AS negv FROM t";
    create(&c, "mv", sql).unwrap();
    assert_matches_oracle(&c, "mv", sql);
    assert_eq!(count(&c, "SELECT count(*) FROM mv"), 3);
    c.execute_batch("DELETE FROM t WHERE x<3").unwrap();
    refresh(&c, "mv").unwrap();
    assert_matches_oracle(&c, "mv", sql);
}

#[test]
fn expression_and_conjunction_depth_limits_are_explicit() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch("CREATE TABLE t(x INTEGER)").unwrap();
    let nested = format!("SELECT {}x{} AS val FROM t", "(".repeat(65), ")".repeat(65));
    let conjunction = format!("SELECT x FROM t WHERE {}", vec!["x=1"; 66].join(" AND "));
    for sql in [nested, conjunction] {
        let err = create(&c, "mv", &sql).unwrap_err();
        assert!(
            err.to_string().contains("depth limit 64")
                || err.to_string().contains("recursion limit exceeded"),
            "{err}"
        );
    }
}

#[test]
fn scalar_evaluation_failure_preserves_all_durable_state_and_staging() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch("CREATE TABLE t(a INTEGER,b INTEGER,c INTEGER,d INTEGER,e INTEGER); INSERT INTO t VALUES (1,2,3,4,5)").unwrap();
    let sql = "SELECT a+b AS result FROM t";
    create(&c, "mv", sql).unwrap();
    c.execute_batch("UPDATE t SET a=3").unwrap();
    let before = maintenance_state(&c, "mv");
    let stage = rows(&c, "SELECT * FROM __ivm_stage_mv");
    // The scalar SELECT binds the entire input layout, while delta reads and
    // catalog checks bind at most two variables on this single-table plan.
    let old = unsafe {
        rusqlite::ffi::sqlite3_limit(c.handle(), rusqlite::ffi::SQLITE_LIMIT_VARIABLE_NUMBER, 4)
    };
    let err = refresh(&c, "mv").unwrap_err();
    assert!(err.to_string().contains("variable number"), "{err}");
    assert_eq!(before, maintenance_state(&c, "mv"));
    assert_eq!(stage, rows(&c, "SELECT * FROM __ivm_stage_mv"));
    unsafe {
        rusqlite::ffi::sqlite3_limit(c.handle(), rusqlite::ffi::SQLITE_LIMIT_VARIABLE_NUMBER, old);
    }
    refresh(&c, "mv").unwrap();
    assert_matches_oracle(&c, "mv", sql);
}

#[test]
fn minimum_negative_weight_is_refused_before_negation_or_apply() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch("CREATE TABLE t(x INTEGER); INSERT INTO t VALUES (1)")
        .unwrap();
    create(&c, "mv", "SELECT x FROM t").unwrap();
    c.execute_batch("DELETE FROM t; UPDATE __ivm_delta_t SET __ivm_w=-9223372036854775808")
        .unwrap();
    let before = maintenance_state(&c, "mv");
    let err = refresh(&c, "mv").unwrap_err();
    assert!(err.to_string().contains("i64::MIN"), "{err}");
    assert_eq!(before, maintenance_state(&c, "mv"));
}

#[test]
fn bag_object_checks_ignore_temp_output_and_catalog_decoys() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch("CREATE TABLE t(x INTEGER); INSERT INTO t VALUES (1),(1)")
        .unwrap();
    let sql = "SELECT x AS result FROM t";
    create(&c, "mv", sql).unwrap();
    c.execute_batch("CREATE TEMP TABLE __ivm_out_mv(decoy); INSERT INTO temp.__ivm_out_mv VALUES ('untouched'); CREATE TEMP VIEW pragma_index_list AS SELECT 1 AS decoy; CREATE TEMP VIEW pragma_index_info AS SELECT 1 AS decoy; INSERT INTO t VALUES (2)").unwrap();
    refresh(&c, "mv").unwrap();
    assert_matches_oracle(&c, "mv", sql);
    assert_eq!(
        rows(&c, "SELECT * FROM temp.__ivm_out_mv"),
        vec![vec![Value::Text("untouched".into())]]
    );
    // Exercise catalog checking through a second view creation as well.
    create(&c, "sibling", "SELECT x+0 AS result FROM t").unwrap();
    assert_matches_oracle(&c, "sibling", "SELECT x+0 AS result FROM t");
}

#[test]
fn review_grouped_keys_must_be_one_actual_live_tuple() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch("CREATE TABLE t(a BLOB,b BLOB); INSERT INTO t VALUES (1.0,1),(1,1.0)")
        .unwrap();
    let sql = "SELECT a/2+b/2 AS x,COUNT(*) AS n FROM t GROUP BY a,b";
    create(&c, "mv", sql).unwrap();
    assert_matches_oracle(&c, "mv", sql);
}

#[test]
fn review_adversarial_real_literals_match_host_bits() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch("CREATE TABLE t(x INTEGER); INSERT INTO t VALUES (1)")
        .unwrap();
    for (i, number) in [
        "1.23456789012345678901234567890123456789",
        "2.2250738585072012e-308",
        "4.9406564584124654e-324",
        "2.4703282292062327e-324",
        "2.4703282292062328e-324",
        "1.7976931348623157e308",
        "2.2250738585072011e-308",
        "0.1000000000000000055511151231257827021181583404541015625",
        "0.1000000000000000055511151231257827021181583404541015626",
        "9007199254740993.0",
        "1.00000000000000011102230246251565404236316680908203125",
        "1.00000000000000011102230246251565404236316680908203126",
    ]
    .iter()
    .enumerate()
    {
        let sql = format!("SELECT {number} AS result FROM t");
        let name = format!("lit{i}");
        create(&c, &name, &sql).unwrap();
        assert_matches_oracle(&c, &name, &sql);
    }
}

#[test]
fn coherent_group_keys_follow_bulk_counts_and_representative_changes() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY,a BLOB,b BLOB,v INTEGER); INSERT INTO t VALUES (1,1.0,1,10),(2,1,1.0,10),(3,1,1.0,10)").unwrap();
    let sql = "SELECT a AS ka,b AS kb,a/2 AS x,SUM(v) AS s,COUNT(*) AS n FROM t GROUP BY a,b";
    create(&c, "mv", sql).unwrap();
    assert_eq!(
        rows(&c, "SELECT typeof(ka),typeof(kb),x,s,n FROM mv"),
        vec![vec![
            Value::Text("integer".into()),
            Value::Text("real".into()),
            Value::Integer(0),
            Value::Integer(30),
            Value::Integer(3)
        ]]
    );
    c.execute_batch("DELETE FROM t WHERE id=2").unwrap();
    refresh(&c, "mv").unwrap();
    assert_eq!(
        rows(&c, "SELECT typeof(ka),typeof(kb),x,s,n FROM mv"),
        vec![vec![
            Value::Text("integer".into()),
            Value::Text("real".into()),
            Value::Integer(0),
            Value::Integer(20),
            Value::Integer(2)
        ]]
    );
    // Aggregate values/count remain identical while the selected full tuple changes.
    c.execute_batch("DELETE FROM t WHERE id=3; INSERT INTO t VALUES (4,1.0,1,10)")
        .unwrap();
    refresh(&c, "mv").unwrap();
    assert_eq!(
        rows(&c, "SELECT typeof(ka),typeof(kb),x,s,n FROM mv"),
        vec![vec![
            Value::Text("real".into()),
            Value::Text("integer".into()),
            Value::Real(0.5),
            Value::Integer(20),
            Value::Integer(2)
        ]]
    );
    assert_matches_oracle(&c, "mv", sql);
    c.execute_batch("DELETE FROM t WHERE id=1").unwrap();
    refresh(&c, "mv").unwrap();
    assert_matches_oracle(&c, "mv", sql);
    c.execute_batch("DELETE FROM t; INSERT INTO t VALUES (5,1,1.0,10),(6,1,1.0,10),(7,1.0,1,10)")
        .unwrap();
    refresh(&c, "mv").unwrap();
    assert_eq!(
        rows(&c, "SELECT typeof(ka),typeof(kb),x,s,n FROM mv"),
        vec![vec![
            Value::Text("integer".into()),
            Value::Text("real".into()),
            Value::Integer(0),
            Value::Integer(30),
            Value::Integer(3)
        ]]
    );
    c.execute_batch("DELETE FROM t").unwrap();
    refresh(&c, "mv").unwrap();
    assert_matches_oracle(&c, "mv", sql);
    c.execute_batch("DROP TABLE mv").unwrap();
    assert_eq!(
        count(
            &c,
            "SELECT count(*) FROM sqlite_schema WHERE name LIKE '__ivm_state_mv_%'"
        ),
        0
    );
}

#[test]
fn coherent_tuples_survive_join_fanout_rollback_and_pending_reopen() {
    let file = TempFile::new("m2d-coherent-join");
    let sql = "SELECT l.a/2+l.b/2 AS x,COUNT(*) AS n FROM l JOIN r ON l.k=r.k GROUP BY l.a,l.b";
    {
        let c = open_with_extension(Some(file.path())).unwrap();
        c.execute_batch("CREATE TABLE l(k INTEGER,a BLOB,b BLOB); CREATE TABLE r(k INTEGER); INSERT INTO l VALUES (1,1.0,1),(1,1,1.0); INSERT INTO r VALUES (1),(1),(1)").unwrap();
        create(&c, "mv", sql).unwrap();
        assert_matches_oracle(&c, "mv", sql);
        let before = maintenance_state(&c, "mv");
        c.execute_batch("BEGIN; SAVEPOINT a; DELETE FROM l WHERE typeof(a)='integer'; DELETE FROM r WHERE rowid IN (1,2)").unwrap();
        refresh(&c, "mv").unwrap();
        assert_matches_oracle(&c, "mv", sql);
        c.execute_batch("ROLLBACK TO a; RELEASE a; ROLLBACK")
            .unwrap();
        assert_eq!(before, maintenance_state(&c, "mv"));
        c.execute_batch("DELETE FROM l WHERE typeof(a)='integer'; INSERT INTO r VALUES (1)")
            .unwrap();
    }
    let c = open_with_extension(Some(file.path())).unwrap();
    refresh(&c, "mv").unwrap();
    assert_matches_oracle(&c, "mv", sql);
    c.execute_batch("DELETE FROM l").unwrap();
    refresh(&c, "mv").unwrap();
    assert_matches_oracle(&c, "mv", sql);
}

#[test]
fn malformed_or_missing_coherent_tuple_state_fails_without_durable_apply() {
    for corruption in ["UPDATE __ivm_state_mv_1_agg_representatives SET w=-1","UPDATE __ivm_state_mv_1_agg_representatives SET w=99","DELETE FROM __ivm_state_mv_1_agg_representatives","UPDATE __ivm_state_mv_1_agg_representatives SET val=x'010000000000000001' WHERE val=(SELECT val FROM __ivm_state_mv_1_agg_representatives LIMIT 1)","UPDATE __ivm_state_mv_1_agg_representatives SET val=x'010000000000000009010000000000000001' WHERE val=(SELECT val FROM __ivm_state_mv_1_agg_representatives LIMIT 1)"] {
        let c=open_with_extension(None).unwrap();c.execute_batch("CREATE TABLE t(a BLOB,b BLOB,v INTEGER); INSERT INTO t VALUES (1.0,1,10),(1,1.0,20)").unwrap();let sql="SELECT a/2+b/2 AS x,SUM(v) AS s FROM t GROUP BY a,b";create(&c,"mv",sql).unwrap();
        c.execute_batch("CREATE TEMP TABLE saved_tuples AS SELECT * FROM __ivm_state_mv_1_agg_representatives; UPDATE t SET v=v+1").unwrap();c.execute_batch(corruption).unwrap();let before=maintenance_state(&c,"mv");let error=refresh(&c,"mv").unwrap_err();assert!(error.to_string().contains("representative"),"{corruption}: {error}");assert_eq!(before,maintenance_state(&c,"mv"));
        c.execute_batch("DELETE FROM __ivm_state_mv_1_agg_representatives; INSERT INTO __ivm_state_mv_1_agg_representatives SELECT * FROM saved_tuples").unwrap();refresh(&c,"mv").unwrap();assert_matches_oracle(&c,"mv",sql);
    }
}

#[test]
fn coherent_representative_is_independent_of_single_min_max_extremal_row() {
    let c = open_with_extension(None).unwrap();
    c.execute_batch("CREATE TABLE t(k BLOB,v INTEGER); INSERT INTO t VALUES (1,10),(1.0,20)")
        .unwrap();
    let sql = "SELECT k/2 AS x,MAX(v) AS m FROM t GROUP BY k";
    create(&c, "mv", sql).unwrap();
    // The documented representative is the smallest complete typed tuple,
    // independent of SQLite's special bare-key correlation with one MAX.
    assert_eq!(
        rows(&c, "SELECT * FROM mv"),
        vec![vec![Value::Integer(0), Value::Integer(20)]]
    );
    c.execute_batch("DELETE FROM t WHERE typeof(k)='integer'")
        .unwrap();
    refresh(&c, "mv").unwrap();
    assert_matches_oracle(&c, "mv", sql);
}
