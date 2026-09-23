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

pub fn gen_row(rng: &mut StdRng, schema: &Schema, domain: &Domain) -> Row {
    let values = schema
        .columns
        .iter()
        .map(|col| {
            if col.nullable && rng.random_bool(domain.null_rate) {
                return Value::Null;
            }
            // Item 10 (a deferred minor): `distinct == 0` would make
            // `0..domain.distinct` an empty range and panic `random_range`.
            // `.max(1)` is spelled the same way as `ivmlite-workload`'s row
            // generation (`group_cardinality.max(1)` / `amount_max.max(1)`),
            // so the one rule has one spelling across the repository.
            let n = rng.random_range(0..domain.distinct.max(1)) as i64;
            match col.ty {
                ColumnType::Integer => Value::Int(n),
                ColumnType::Text => Value::Text(format!("v{n}")),
            }
        })
        .collect();
    Row::new(values)
}

pub fn gen_rows(rng: &mut StdRng, schema: &Schema, domain: &Domain, count: usize) -> Vec<Row> {
    (0..count).map(|_| gen_row(rng, schema, domain)).collect()
}

/// Generate the table structure for a differential test case.
///
/// Each table has exactly 2 columns (both nullable: one TEXT, one INTEGER),
/// and the column count is **not** a tunable parameter: spec §9.2 item 4 makes it
/// a precondition for "enumeration beats randomness", and widening to 3 columns
/// was measured to grow the enumeration from about 554 to about 4209. Both
/// columns are nullable so that spec §6.1's "`SUM` over no non-NULL input
/// returns NULL" path is really reached by random testing — which is why M0's
/// integration tests made `amount` nullable.
pub fn gen_database(table_count: usize) -> Database {
    let tables = (0..table_count)
        .map(|i| Schema {
            table: format!("t{i}"),
            columns: vec![
                Column {
                    name: "k".into(),
                    ty: ColumnType::Text,
                    nullable: true,
                },
                Column {
                    name: "v".into(),
                    ty: ColumnType::Integer,
                    nullable: true,
                },
            ],
        })
        .collect();
    Database::new(tables)
}

/// `gen_database(2)` with the right table's two columns swapped:
/// `t0(k TEXT, v INTEGER)` and `t1(v INTEGER, k TEXT)`.
///
/// In `gen_database`'s tables every column sits at the same position in both
/// tables, so every same-typed key pair `enumerate_join` produces joins column
/// `i` to column `i`, and reading one side's key index for the other side goes
/// unnoticed. Over this database the key pairs are `(0, 1)` and `(1, 0)`.
/// Still 2 columns per table (spec §9.2 item 4).
pub fn gen_database_with_swapped_right_table() -> Database {
    let mut tables = gen_database(2).tables().to_vec();
    tables[1].columns.reverse();
    Database::new(tables)
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

    /// Spec §6.1: SQLite's integer SUM raises an error on overflow, and whether
    /// it does depends on scan order, so incremental and full recomputation
    /// diverge in the overflow region. The generator must make overflow
    /// unreachable.
    #[test]
    fn domain_cannot_overflow_integer_sum() {
        let d = Domain::default();
        let worst_case_sum = (d.distinct as i128) * 1_000_000;
        assert!(
            worst_case_sum < (1i128 << 62),
            "even with a million rows all in one group, the sum must stay far below 2^62"
        );
    }

    #[test]
    fn generated_values_stay_within_the_domain() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(1);
        let schema = orders();
        let domain = Domain::default();
        let rows = gen_rows(&mut rng, &schema, &domain, 500);

        let distinct_regions: HashSet<&Value> = rows.iter().map(|r| r.get(0)).collect();
        assert!(
            distinct_regions.len() <= domain.distinct + 1,
            "the number of distinct values must be bounded by the domain (+1 for NULL), got {}",
            distinct_regions.len()
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
