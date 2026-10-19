//! REAL aggregates depend on the final multiset, including after retractions.

mod common;

use common::{create, refresh};
use ivmlite_test::{exact_mean, fsum, open_with_extension};
use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::{RngExt, SeedableRng};
use rusqlite::Connection;

const QUERY: &str = "SELECT k, SUM(x), TOTAL(x), AVG(x) FROM t GROUP BY k";
const REINSERT_CHUNK: usize = 7;

fn dyadics(rng: &mut StdRng, count: usize) -> Vec<f64> {
    (0..count)
        .map(|_| {
            let mantissa = rng.random_range(-(1i64 << 52)..(1i64 << 52));
            let exponent = rng.random_range(-80..40);
            mantissa as f64 * 2f64.powi(exponent)
        })
        .collect()
}

fn view_over(values: &[f64]) -> Connection {
    let c = open_with_extension(None).unwrap();
    c.execute_batch("CREATE TABLE t(k TEXT, x REAL)").unwrap();
    insert(&c, values);
    create(&c, "v", QUERY).unwrap();
    c
}

fn insert(c: &Connection, values: &[f64]) {
    let mut stmt = c.prepare("INSERT INTO t VALUES ('a', ?1)").unwrap();
    for value in values {
        stmt.execute([value]).unwrap();
    }
}

fn aggregate_bits(c: &Connection) -> [u64; 3] {
    c.query_row(
        "SELECT \"SUM(x)\", \"TOTAL(x)\", \"AVG(x)\" FROM v",
        [],
        |r| {
            Ok([
                r.get::<_, f64>(0)?.to_bits(),
                r.get::<_, f64>(1)?.to_bits(),
                r.get::<_, f64>(2)?.to_bits(),
            ])
        },
    )
    .unwrap()
}

fn assert_reference(c: &Connection, values: &[f64], seed: u64, stage: &str) {
    // Check the base independently as well: deleting by SQL equality must
    // remove exactly one occurrence of the value replayed by its bits.
    let mut expected: Vec<u64> = values.iter().map(|v| v.to_bits()).collect();
    expected.sort_unstable();
    let mut actual: Vec<u64> = c
        .prepare("SELECT x FROM t")
        .unwrap()
        .query_map([], |r| Ok(r.get::<_, f64>(0)?.to_bits()))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    actual.sort_unstable();
    assert_eq!(actual, expected, "base multiset, seed {seed}, {stage}");

    let sum = fsum(values).to_bits();
    let mean = exact_mean(values, i64::try_from(values.len()).unwrap()).to_bits();
    assert_eq!(
        aggregate_bits(c),
        [sum, sum, mean],
        "exact SUM/TOTAL/AVG, seed {seed}, {stage}"
    );
}

fn exercise(mut values: Vec<f64>, mut removed: Vec<f64>, rng: &mut StdRng, seed: u64) {
    let c = view_over(&[]);
    insert(&c, &values);
    refresh(&c, "v").unwrap();
    assert_reference(&c, &values, seed, "initial insertion");

    removed.shuffle(rng);
    for &value in &removed {
        assert_eq!(
            c.execute(
                "DELETE FROM t WHERE rowid = (SELECT rowid FROM t WHERE x = ?1 LIMIT 1)",
                [value],
            )
            .unwrap(),
            1,
            "one deletion, seed {seed}"
        );
        let index = values
            .iter()
            .position(|v| v.to_bits() == value.to_bits())
            .expect("the independent multiset contains the deleted value");
        values.remove(index);
    }
    refresh(&c, "v").unwrap();
    assert_reference(&c, &values, seed, "retraction");

    removed.retain(|_| rng.random_bool(0.5));
    removed.shuffle(rng);
    for chunk in removed.chunks(REINSERT_CHUNK) {
        insert(&c, chunk);
        values.extend_from_slice(chunk);
        refresh(&c, "v").unwrap();
        assert_reference(&c, &values, seed, "chunked reinsertion");
    }

    let mut fresh_order = values.clone();
    fresh_order.shuffle(rng);
    let fresh = view_over(&fresh_order);
    assert_reference(&fresh, &values, seed, "fresh shuffled insertion");
    assert_eq!(
        aggregate_bits(&c),
        aggregate_bits(&fresh),
        "incremental and fresh final multisets, seed {seed}"
    );
}

#[test]
fn real_aggregates_are_independent_of_insert_delete_order() {
    for seed in 0..10 {
        let mut rng = StdRng::seed_from_u64(seed);
        let values = dyadics(&mut rng, 120);
        let mut removed = values.clone();
        removed.shuffle(&mut rng);
        removed.truncate(50);
        exercise(values, removed, &mut rng, seed);
    }
}

#[test]
fn real_aggregates_are_order_independent_across_extreme_magnitudes() {
    for seed in 0..10 {
        let mut rng = StdRng::seed_from_u64(seed);
        let mut values = dyadics(&mut rng, 80);
        values.extend([1e300, -1e300, 2.5e299, 1e-300, -3e-310, 5e-324]);
        let mut by_magnitude = values.clone();
        by_magnitude.sort_by(|a, b| a.abs().total_cmp(&b.abs()));
        let mut removed = by_magnitude[..4].to_vec();
        removed.extend_from_slice(&by_magnitude[by_magnitude.len() - 4..]);
        exercise(values, removed, &mut rng, seed);
    }
}
