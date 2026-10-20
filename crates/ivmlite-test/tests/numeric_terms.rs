//! M2c spec §4: `numeric_term` classifies a value exactly as SQLite's `sum`
//! does, on the SQLite the tests link (M2c spec §4, "How a value counts").

use ivmlite_core::{numeric_term, parse_numeric_text, NumericTerm, Value};
use rusqlite::types::Value as Sql;
use rusqlite::Connection;

const CORPUS: &[&str] = &[
    "0",
    "12",
    " 12 ",
    "\t12\n",
    "+7",
    "-0",
    "007",
    "12abc",
    "abc",
    "",
    " ",
    "1.0",
    "1.",
    ".5",
    "-.5",
    "1e2",
    "1E+2",
    "1e-2",
    "1e",
    "1e+",
    " 0x10",
    "inf",
    "nan",
    "1e400",
    "-1e400",
    "9223372036854775807",
    "9223372036854775808",
    "-9223372036854775808",
    "-9223372036854775809",
    "123456789012345",
    "0.1",
    "2.5",
    "-2.5e-3",
    "1 2",
    "١٢",
    "1e-400",
    "4.9e-324",
    "1.7976931348623157e308",
    "0.1234567890123456789",
    "123456789012345678901234567890",
    "1e5 x",
    "1x5",
    "- 1",
    "+-1",
    "1e+-2",
    ".",
    "-.",
    ".e5",
    "00012.50",
    "-0.0",
    "-0e0",
    "-1e-400",
    "1\0",
    "1\0x",
    "\x001",
    "12 \0",
    "1.5\0x",
];

fn sqlite_sum(c: &Connection, v: &Sql) -> Sql {
    c.query_row("SELECT sum(?1)", [v], |r| r.get(0)).unwrap()
}

/// The byte length SQLite sees in the bound text, embedded NULs included:
/// `length()` of a TEXT stops at the first NUL, that of a BLOB does not.
fn bound_length(c: &Connection, s: &str) -> usize {
    let length: i64 = c
        .query_row("SELECT length(CAST(?1 AS BLOB))", [s], |r| r.get(0))
        .unwrap();
    usize::try_from(length).unwrap()
}

/// `CAST(?1 AS REAL)`: the same `sqlite3AtoF` reading as `sum`, but without
/// `sum`'s starting value of `+0.0`, so the sign of a zero survives.
fn sqlite_cast_real(c: &Connection, v: &Sql) -> f64 {
    c.query_row("SELECT CAST(?1 AS REAL)", [v], |r| r.get(0))
        .unwrap()
}

#[test]
fn text_and_blob_terms_match_sqlite_sum() {
    let c = Connection::open_in_memory().unwrap();
    for &s in CORPUS {
        assert_eq!(bound_length(&c, s), s.len(), "SQLite received {s:?} whole");
        for (value, sql) in [
            (Value::Text(s.into()), Sql::Text(s.into())),
            (
                Value::Blob(s.as_bytes().to_vec()),
                Sql::Blob(s.as_bytes().to_vec()),
            ),
        ] {
            let want = sqlite_sum(&c, &sql);
            let got = numeric_term(&value).expect("non-NULL");
            match (got, &want) {
                (NumericTerm::Integer(n), Sql::Integer(m)) => assert_eq!(n, *m, "{value:?}"),
                (NumericTerm::Approx(x), Sql::Real(y)) => {
                    // `sum` starts from +0.0, so a lone -0.0 term sums to +0.0.
                    assert_eq!((x + 0.0).to_bits(), y.to_bits(), "{value:?}");
                    assert_eq!(
                        x.to_bits(),
                        sqlite_cast_real(&c, &sql).to_bits(),
                        "{value:?}"
                    );
                }
                other => panic!("{value:?}: ivmlite {:?}, SQLite {want:?}", other.0),
            }
        }
    }
}

/// M2c spec §5: a TEXT stored into a REAL-affinity column converts exactly
/// when `parse_numeric_text` says it is well formed, and to the same number.
/// The sign of a zero is not compared: SQLite stores `-0.0` in a REAL column
/// as `+0.0` (the `+ 0.0` below), while `CAST` in the test above pins the
/// sign `numeric_term` reads. Everything else, including a text that ends at
/// a NUL, must agree bit for bit.
#[test]
fn parse_numeric_text_matches_a_real_affinity_column() {
    let c = Connection::open_in_memory().unwrap();
    c.execute("CREATE TABLE t(x REAL)", []).unwrap();
    for &s in CORPUS {
        c.execute("DELETE FROM t", []).unwrap();
        c.execute("INSERT INTO t VALUES (?1)", [s]).unwrap();
        let stored: Sql = c.query_row("SELECT x FROM t", [], |r| r.get(0)).unwrap();
        match (parse_numeric_text(s), &stored) {
            (Some(NumericTerm::Integer(n)), Sql::Real(y)) => {
                assert_eq!((n as f64).to_bits(), y.to_bits(), "{s:?}");
            }
            (Some(NumericTerm::Approx(x)), Sql::Real(y)) => {
                assert_eq!((x + 0.0).to_bits(), y.to_bits(), "{s:?}");
            }
            (None, Sql::Text(t)) => assert_eq!(t, s),
            other => panic!("{s:?}: ivmlite {:?}, SQLite {stored:?}", other.0),
        }
    }
}
