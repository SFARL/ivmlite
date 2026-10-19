use std::collections::BTreeMap;

use ivmlite_core::{Database, Row, Value};
use rand::rngs::StdRng;
use rand::RngExt;

use crate::{Column, ColumnType, Schema};

/// The generator's value-domain configuration.
///
/// `distinct` is deliberately small: if a column had a million distinct
/// values, every group would have one row, and "the same group inserted into
/// and deleted from repeatedly" would never be tested — which is exactly where
/// retraction and zombie-row bugs come from (spec §9.2).
#[derive(Debug, Clone)]
pub struct Domain {
    pub distinct: usize,
    pub null_rate: f64,
}

impl Default for Domain {
    fn default() -> Self {
        Domain {
            distinct: 8,
            null_rate: 0.2,
        }
    }
}

/// The values a column of one affinity draws (M2c spec §6). Every value is
/// a fixed point of that affinity: SQLite stores it unchanged, with its
/// storage class and bits, so the in-memory engines see exactly what SQLite
/// stores and nothing is normalized at run time
/// (`every_pool_value_is_stored_unchanged` checks this against SQLite).
///
/// Every REAL is finite, not subnormal and below 2^64 in magnitude, so the
/// reference's `fsum` and `exact_mean` stay exact; convertible TEXT has at
/// most 15 significant digits, or is exactly representable (`2^63`), so
/// every conforming text-to-number conversion agrees.
#[derive(Debug, Clone, PartialEq)]
pub struct Pool {
    /// The common values, drawn uniformly from the first `Domain::distinct`.
    pub narrow: Vec<Value>,
    /// Edge values (2^53 neighbours, 0.1, BLOB in a typed column, odd
    /// TEXT), drawn with probability 1/16.
    pub edge: Vec<Value>,
    /// Integers next to ±2^63, drawn with probability 1/64, so a SUM can
    /// overflow `i64` but few cases end early on it (M2c spec §6, amendment 6).
    pub big: Vec<Value>,
}

/// One in `BIG_ODDS` non-NULL values comes from `Pool::big`.
const BIG_ODDS: u32 = 64;
/// One in `EDGE_ODDS` of the rest comes from `Pool::edge`.
const EDGE_ODDS: u32 = 16;
/// 2^53: above it, not every integer is a REAL.
const TWO_POW_53: i64 = 1 << 53;

fn ints(range: std::ops::RangeInclusive<i64>) -> impl Iterator<Item = Value> {
    range.map(Value::Int)
}

fn text(s: &str) -> Value {
    Value::Text(s.into())
}

/// The pool of a column of affinity `ty`.
pub fn value_pool(ty: ColumnType) -> Pool {
    let near_i64_bounds = vec![Value::Int(i64::MAX - 1), Value::Int(i64::MIN + 1)];
    match ty {
        ColumnType::Integer | ColumnType::Numeric => Pool {
            narrow: ints(0..=5).chain([Value::Real(2.5), text("v1")]).collect(),
            edge: vec![
                Value::Int(TWO_POW_53 + 1),
                Value::Int(-TWO_POW_53),
                Value::Real(0.1),
                Value::Blob(vec![1]),
            ],
            big: near_i64_bounds,
        },
        ColumnType::Real => Pool {
            narrow: [0.0, 1.0, 2.0, 3.0, 4.0, 2.5, 0.1]
                .into_iter()
                .map(Value::Real)
                .chain([text("v1")])
                .collect(),
            edge: vec![
                Value::Real(TWO_POW_53 as f64),
                Value::Real(1e15 + 0.5),
                Value::Real(-0.25),
                Value::Blob(vec![1]),
            ],
            big: vec![],
        },
        ColumnType::Text => Pool {
            narrow: (0..=5)
                .map(|n| Value::Text(format!("v{n}")))
                .chain([text("12"), text("1.5")])
                .collect(),
            edge: vec![
                text(" 7 "),
                text("1e2"),
                text("9223372036854775808"),
                Value::Blob(vec![1]),
            ],
            big: vec![],
        },
        ColumnType::Blob => Pool {
            narrow: ints(0..=3)
                .chain([Value::Real(1.0), Value::Real(-0.0), Value::Real(2.5)])
                .chain([text("3"), text("1.5"), text("v1")])
                .chain([Value::Blob(vec![0]), Value::Blob(b"12".to_vec())])
                .collect(),
            edge: vec![Value::Int(TWO_POW_53 + 1), Value::Real(0.1), text("12abc")],
            big: near_i64_bounds,
        },
    }
}

/// One value of `pool`: `big` with probability 1/64 (when it has values),
/// else `edge` with probability 1/16, else one of the first
/// `domain.distinct` values of `narrow`, uniformly.
fn draw(rng: &mut StdRng, pool: &Pool, domain: &Domain) -> Value {
    let pick =
        |rng: &mut StdRng, values: &[Value]| values[rng.random_range(0..values.len())].clone();
    if !pool.big.is_empty() && rng.random_range(0..BIG_ODDS) == 0 {
        return pick(rng, &pool.big);
    }
    if rng.random_range(0..EDGE_ODDS) == 0 {
        return pick(rng, &pool.edge);
    }
    // Item 10 (a deferred minor): `distinct == 0` would leave an empty slice
    // and panic `random_range`. `.max(1)` is spelled the same way as
    // `ivmlite-workload`'s row generation (`group_cardinality.max(1)` /
    // `amount_max.max(1)`), so the one rule has one spelling across the
    // repository.
    let narrow = &pool.narrow[..domain.distinct.max(1).min(pool.narrow.len())];
    pick(rng, narrow)
}

pub fn gen_row(rng: &mut StdRng, schema: &Schema, domain: &Domain) -> Row {
    let values = schema
        .columns
        .iter()
        .map(|col| {
            if col.nullable && rng.random_bool(domain.null_rate) {
                return Value::Null;
            }
            draw(rng, &value_pool(col.ty), domain)
        })
        .collect();
    Row::new(values)
}

pub fn gen_rows(rng: &mut StdRng, schema: &Schema, domain: &Domain, count: usize) -> Vec<Row> {
    (0..count).map(|_| gen_row(rng, schema, domain)).collect()
}

fn column(name: &str, ty: ColumnType) -> Column {
    Column {
        name: name.into(),
        ty,
        nullable: true,
    }
}

/// Generate the table structure for a differential test case:
/// `t{i}(k TEXT, v INTEGER, x)`, the last column with no declared type
/// (BLOB affinity).
///
/// Each table has exactly 3 columns, and the column count is **not** a
/// tunable parameter: the enumeration's size follows from it (parent spec
/// §9.2 item 4, amended by M2c: 510 single-table queries and 1,197 join
/// queries per database, crossed as M2c spec §6 records).
/// Every column is nullable so that spec §6.1's "`SUM` over no non-NULL
/// input returns NULL" path is really reached by random testing — which is
/// why M0's integration tests made `amount` nullable.
pub fn gen_database(table_count: usize) -> Database {
    let tables = (0..table_count)
        .map(|i| Schema {
            table: format!("t{i}"),
            columns: vec![
                column("k", ColumnType::Text),
                column("v", ColumnType::Integer),
                column("x", ColumnType::Blob),
            ],
        })
        .collect();
    Database::new(tables)
}

/// `gen_database(2)` with the right table's columns reversed:
/// `t0(k TEXT, v INTEGER, x)` and `t1(x, v INTEGER, k TEXT)`.
///
/// In `gen_database`'s tables every column sits at the same position in both
/// tables, so every same-affinity key pair `enumerate_join` produces joins
/// column `i` to column `i`, and reading one side's key index for the other
/// side goes unnoticed. Over this database the key pairs are `(0, 2)`,
/// `(1, 1)` and `(2, 0)`.
pub fn gen_database_with_swapped_right_table() -> Database {
    let mut tables = gen_database(2).tables().to_vec();
    tables[1].columns.reverse();
    Database::new(tables)
}

/// The REAL and NUMERIC affinities (M2c spec §6):
/// `t0(r REAL, n NUMERIC, k TEXT)` and `t1(n NUMERIC, k TEXT, r REAL)`, so
/// its join key pairs are `(0, 2)` REAL, `(1, 0)` NUMERIC and `(2, 1)` TEXT,
/// all at different positions.
pub fn gen_real_numeric_database() -> Database {
    Database::new(vec![
        Schema {
            table: "t0".into(),
            columns: vec![
                column("r", ColumnType::Real),
                column("n", ColumnType::Numeric),
                column("k", ColumnType::Text),
            ],
        },
        Schema {
            table: "t1".into(),
            columns: vec![
                column("n", ColumnType::Numeric),
                column("k", ColumnType::Text),
                column("r", ColumnType::Real),
            ],
        },
    ])
}

/// Generate a batch of initial rows for each table the `Database` declares,
/// keyed by table name in a `BTreeMap` — a `BTreeMap` rather than a `HashMap`,
/// because the iteration order must be deterministic (spec §9.4).
pub fn gen_initial(
    rng: &mut StdRng,
    db: &Database,
    domain: &Domain,
    rows_per_table: usize,
) -> BTreeMap<String, Vec<Row>> {
    db.tables()
        .iter()
        .map(|s| (s.table.clone(), gen_rows(rng, s, domain, rows_per_table)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Column, ColumnType, Schema};
    use rand::SeedableRng;
    use std::collections::HashSet;

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
    fn domain_is_narrow_by_default() {
        let d = Domain::default();
        assert_eq!(
            d.distinct, 8,
            "a narrow value domain is the precondition for catching retraction bugs (spec §9.2)"
        );
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(
                d.null_rate, 0.2,
                "the NULL rate must be 0.2, which is what makes NULL frequent in the tests"
            );
        }
    }

    #[test]
    fn generated_values_come_from_their_columns_pool() {
        // M2c spec §6: a column draws only its affinity's pool, and only the
        // first `distinct` narrow values, so groups still repeat.
        let mut rng = rand::rngs::StdRng::seed_from_u64(1);
        let domain = Domain {
            distinct: 3,
            null_rate: 0.2,
        };
        for db in [gen_database(1), gen_real_numeric_database()] {
            let schema = &db.tables()[0];
            for row in gen_rows(&mut rng, schema, &domain, 2000) {
                for (v, col) in row.0.iter().zip(&schema.columns) {
                    let pool = value_pool(col.ty);
                    let allowed = *v == Value::Null
                        || pool.narrow[..3].contains(v)
                        || pool.edge.contains(v)
                        || pool.big.contains(v);
                    assert!(allowed, "{:?} drew {v:?}", col.ty);
                }
            }
        }
        let distinct: HashSet<Value> = gen_rows(&mut rng, &orders(), &domain, 2000)
            .into_iter()
            .map(|r| r.get(0).clone())
            .collect();
        let text = value_pool(ColumnType::Text);
        assert_eq!(
            distinct.len(),
            3 + text.edge.len() + 1,
            "every allowed TEXT value appears, NULL included: {distinct:?}"
        );
    }

    #[test]
    fn every_pool_value_is_stored_unchanged() {
        // M2c spec §6, against SQLite itself: a pool value inserted into a
        // column of its affinity reads back with the same class and bits.
        let c = rusqlite::Connection::open_in_memory().unwrap();
        for ty in [
            ColumnType::Integer,
            ColumnType::Real,
            ColumnType::Numeric,
            ColumnType::Text,
            ColumnType::Blob,
        ] {
            c.execute_batch(&format!(
                "DROP TABLE IF EXISTS p; CREATE TABLE p(c {})",
                ty_sql(ty)
            ))
            .unwrap();
            let pool = value_pool(ty);
            for v in pool.narrow.iter().chain(&pool.edge).chain(&pool.big) {
                c.execute("DELETE FROM p", []).unwrap();
                c.execute("INSERT INTO p VALUES (?1)", [crate::oracle::to_sql(v)])
                    .unwrap();
                let back: rusqlite::types::Value =
                    c.query_row("SELECT c FROM p", [], |r| r.get(0)).unwrap();
                assert_eq!(crate::test_support::from_sql(back), *v, "{ty:?} {v:?}");
            }
        }
    }

    fn ty_sql(ty: ColumnType) -> &'static str {
        if ty == ColumnType::Blob {
            ""
        } else {
            ty.name()
        }
    }

    #[test]
    fn the_generated_databases_declare_all_five_types() {
        let mut seen: Vec<ColumnType> = [gen_database(2), gen_real_numeric_database()]
            .iter()
            .flat_map(|db| {
                db.tables()
                    .iter()
                    .flat_map(|t| t.columns.iter().map(|c| c.ty))
                    .collect::<Vec<_>>()
            })
            .collect();
        seen.sort_by_key(|t| t.name());
        seen.dedup();
        assert_eq!(seen.len(), 5, "{seen:?}");
        assert!(gen_database(3)
            .tables()
            .iter()
            .all(|t| t.columns.len() == 3 && t.columns.iter().all(|c| c.nullable)));
    }

    #[test]
    fn rare_values_are_drawn_but_rarely() {
        let schema = gen_database(1).tables()[0].clone();
        let mut rng = rand::rngs::StdRng::seed_from_u64(5);
        let rows = gen_rows(&mut rng, &schema, &Domain::default(), 6400);
        let big = rows
            .iter()
            .filter(|r| matches!(r.get(1), Value::Int(n) if n.unsigned_abs() > 1 << 62))
            .count();
        assert!(
            (40..=160).contains(&big),
            "about 1/64 of 6400 * 0.8 non-NULL values, got {big}"
        );
    }

    #[test]
    fn nullable_columns_actually_produce_nulls() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(2);
        let rows = gen_rows(&mut rng, &orders(), &Domain::default(), 500);
        let nulls = rows.iter().filter(|r| r.get(0) == &Value::Null).count();
        assert!(
            nulls > 0,
            "NULL forms its own group under GROUP BY, a classic bug site, so it must appear often"
        );
    }

    #[test]
    fn non_nullable_columns_never_produce_nulls() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(3);
        let rows = gen_rows(&mut rng, &orders(), &Domain::default(), 500);
        assert!(rows.iter().all(|r| r.get(1) != &Value::Null));
    }

    /// Item 10 (a deferred minor): `distinct == 0` must not panic, rather than
    /// handing the empty range `0..0` to `random_range`.
    #[test]
    fn zero_distinct_domain_does_not_panic() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(1);
        let domain = Domain {
            distinct: 0,
            null_rate: 0.0,
        };
        let rows = gen_rows(&mut rng, &orders(), &domain, 10);
        assert_eq!(rows.len(), 10);
    }

    #[test]
    fn same_seed_yields_same_rows() {
        let schema = orders();
        let a = gen_rows(
            &mut rand::rngs::StdRng::seed_from_u64(7),
            &schema,
            &Domain::default(),
            20,
        );
        let b = gen_rows(
            &mut rand::rngs::StdRng::seed_from_u64(7),
            &schema,
            &Domain::default(),
            20,
        );
        assert_eq!(a, b);
    }
}
