//! M2c spec §6: what one differential check accepts.

use std::collections::BTreeMap;

use ivmlite_core::{AggFn, Row, Value, ZSet};

use crate::exact_reference::UNIT_ROUNDOFF;
use crate::naive::Evaluated;
use crate::ViewQuery;

/// Judge the engine's state `got` against the independent `reference`, and
/// the reference against SQLite's own result `sqlite` (`None` when SQLite
/// stopped on a prefix `integer overflow`, M2c spec §6, amendment 6).
///
/// - `got` must equal the reference exactly: every value by identity, a
///   REAL bit for bit.
/// - The reference and SQLite must have the same groups by canonical key,
///   which compares the key columns: an `Int` and a `Real` of equal value
///   are one key (M2c spec §2), anything else is not.
/// - Each aggregate must be identical, or, for MIN/MAX, a numeric tie
///   (equal by `sql_cmp`), or, for a finite REAL SUM/TOTAL/AVG, within KBN's
///   bound `4ε·Σ|xᵢ|` (AVG: `4ε·Σ|xᵢ|/n + 2ε·|sqlite|`; amendment 7).
///   For explicit intermediate REAL overflow, SQLite ±Inf or NULL may match
///   a finite REAL reference only when Σ|xᵢ| ≥ MAX/4 (spec §6 amendment).
pub fn compare_with_oracles(
    got: &ZSet,
    reference: &Evaluated,
    sqlite: Option<&ZSet>,
    query: &ViewQuery,
) -> Result<(), String> {
    if got != &reference.rows {
        return Err(format!(
            "the engine disagrees with the exact reference\n  engine:    {got:?}\n  reference: {:?}",
            reference.rows
        ));
    }
    let Some(sqlite) = sqlite else {
        return Ok(());
    };
    let keys = query.group_by.len();
    let ours = by_group(&reference.rows, keys)?;
    let theirs = by_group(sqlite, keys)?;
    if !ours.keys().eq(theirs.keys()) {
        return Err(format!(
            "the reference's groups differ from SQLite's\n  reference: {:?}\n  SQLite:    {:?}",
            ours.values().collect::<Vec<_>>(),
            theirs.values().collect::<Vec<_>>()
        ));
    }
    for (key, row) in &ours {
        for column in 0..keys {
            if matches!(theirs[key].get(column), Value::Real(_))
                && !matches!(row.get(column), Value::Real(_))
            {
                return Err(format!("group {key:?}, column {column}: SQLite's REAL group key requires a REAL reference key"));
            }
        }
        for (i, agg) in query.aggs.iter().enumerate() {
            agree(
                agg.func,
                row.get(keys + i),
                theirs[key].get(keys + i),
                reference.magnitudes.get(&(key.clone(), i)).copied(),
            )
            .map_err(|why| format!("group {key:?}, aggregate {i} ({:?}): {why}", agg.func))?;
        }
    }
    Ok(())
}

/// Rows by canonical group key (M2c spec §2: SQLite outputs the key of an
/// arbitrary row of the group, so its key may be another row's class).
fn by_group(z: &ZSet, keys: usize) -> Result<BTreeMap<Row, Row>, String> {
    let mut out = BTreeMap::new();
    for (row, &w) in z.iter() {
        if w != 1 {
            return Err(format!("row {row:?} has weight {w}"));
        }
        let key = Row::new(row.0[..keys].iter().map(Value::canonical).collect());
        if out.insert(key.clone(), row.clone()).is_some() {
            return Err(format!("two rows share the group {key:?}"));
        }
    }
    Ok(out)
}

/// Whether the reference's aggregate value `ours` and SQLite's agree.
fn agree(
    func: AggFn,
    ours: &Value,
    sqlite: &Value,
    magnitude: Option<(f64, i64)>,
) -> Result<(), String> {
    if ours == sqlite {
        return Ok(());
    }
    match (func, ours, sqlite) {
        // M2c spec §4 "Ties": SQLite's pick among equal numerics follows scan
        // order. Values of two classes never compare equal.
        (AggFn::Min | AggFn::Max, a, b) if a.sql_cmp(b).is_eq() => Ok(()),
        // M2c spec §6, intermediate REAL overflow amendment: SQLite's
        // running KBN sum can produce ±Inf or NULL over finite inputs even
        // when the exact result is finite. Only near the finite range limit
        // is that difference exempt from the KBN comparison. Explicit
        // overflow cases are separate named tests: they cannot use evaluate,
        // whose fsum partials must remain finite.
        (AggFn::Sum | AggFn::Total | AggFn::Avg, Value::Real(a), b)
            if a.is_finite()
                && (matches!(b, Value::Real(x) if x.is_infinite()) || matches!(b, Value::Null))
                && magnitude.is_some_and(|(abs, _)| abs >= f64::MAX / 4.0) =>
        {
            Ok(())
        }
        (AggFn::Sum | AggFn::Total | AggFn::Avg, Value::Real(a), Value::Real(b))
            if a.is_finite() && b.is_finite() =>
        {
            let (abs, n) = magnitude.ok_or("no magnitude recorded for this aggregate")?;
            let bound = match func {
                AggFn::Avg => 4.0 * UNIT_ROUNDOFF * abs / n as f64 + 2.0 * UNIT_ROUNDOFF * b.abs(),
                _ => 4.0 * UNIT_ROUNDOFF * abs,
            };
            if (a - b).abs() <= bound {
                Ok(())
            } else {
                Err(format!(
                    "{a:?} and SQLite's {b:?} differ by more than KBN's bound {bound:e}; \
                     investigate, do not widen it"
                ))
            }
        }
        _ => Err(format!("{ours:?} against SQLite's {sqlite:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Agg, AggFn, Predicate, ViewQuery};

    fn q() -> ViewQuery {
        ViewQuery {
            group_by: vec![0],
            aggs: vec![
                Agg {
                    func: AggFn::Sum,
                    column: Some(1),
                },
                Agg {
                    func: AggFn::Min,
                    column: Some(1),
                },
            ],
            predicate: Predicate::None,
            join: None,
        }
    }

    fn z(rows: Vec<Vec<Value>>) -> ZSet {
        ZSet::from_rows(rows.into_iter().map(|r| (Row::new(r), 1)))
    }

    fn reference(sum: f64) -> Evaluated {
        Evaluated {
            rows: z(vec![vec![
                Value::Real(1.0),
                Value::Real(sum),
                Value::Int(1),
            ]]),
            magnitudes: BTreeMap::from([((Row::new(vec![Value::Int(1)]), 0), (0.6, 3))]),
        }
    }

    #[test]
    fn a_one_ulp_difference_from_the_reference_is_a_failure() {
        let r = reference(0.6);
        let got = z(vec![vec![
            Value::Real(1.0),
            Value::Real(0.6f64.next_up()),
            Value::Int(1),
        ]]);
        assert!(compare_with_oracles(&got, &r, None, &q()).is_err());
        assert!(compare_with_oracles(&r.rows, &r, None, &q()).is_ok());
    }

    #[test]
    fn sqlite_may_differ_within_the_kbn_bound_and_not_beyond() {
        let r = reference(0.6);
        // SQLite's key is INTEGER and its MIN a REAL of equal value: one group, a tie.
        let near = z(vec![vec![
            Value::Int(1),
            Value::Real(0.6f64.next_up()),
            Value::Real(1.0),
        ]]);
        assert!(compare_with_oracles(&r.rows, &r, Some(&near), &q()).is_ok());
        let far = z(vec![vec![
            Value::Int(1),
            Value::Real(0.6 + 1e-10),
            Value::Int(1),
        ]]);
        let e = compare_with_oracles(&r.rows, &r, Some(&far), &q()).unwrap_err();
        assert!(e.contains("do not widen it"), "{e}");
    }

    #[test]
    fn a_class_difference_outside_ties_is_a_failure() {
        let r = reference(0.6);
        let text_min = z(vec![vec![
            Value::Int(1),
            Value::Real(0.6),
            Value::Text("1".into()),
        ]]);
        assert!(compare_with_oracles(&r.rows, &r, Some(&text_min), &q()).is_err());
        let int_sum = z(vec![vec![Value::Int(1), Value::Int(1), Value::Int(1)]]);
        assert!(compare_with_oracles(&r.rows, &r, Some(&int_sum), &q()).is_err());
    }

    #[test]
    fn a_group_key_of_another_class_is_a_failure() {
        // M2c spec §2: only an Int and a Real of equal value are one key.
        let r = reference(0.6);
        let text_key = z(vec![vec![
            Value::Text("1".into()),
            Value::Real(0.6),
            Value::Int(1),
        ]]);
        let e = compare_with_oracles(&r.rows, &r, Some(&text_key), &q()).unwrap_err();
        assert!(e.contains("groups differ"), "{e}");
        let other_value = z(vec![vec![Value::Int(2), Value::Real(0.6), Value::Int(1)]]);
        assert!(compare_with_oracles(&r.rows, &r, Some(&other_value), &q()).is_err());
    }

    #[test]
    fn the_avg_bound_divides_the_magnitude_by_the_count() {
        let query = ViewQuery {
            group_by: vec![0],
            aggs: vec![Agg {
                func: AggFn::Avg,
                column: Some(1),
            }],
            predicate: Predicate::None,
            join: None,
        };
        // Σ|x| = 3000 over n = 1000 terms, mean 1: the bound is
        // 4ε·3 + 2ε·1 = 14ε, about 1.55e-15.
        let r = Evaluated {
            rows: z(vec![vec![Value::Int(1), Value::Real(1.0)]]),
            magnitudes: BTreeMap::from([((Row::new(vec![Value::Int(1)]), 0), (3000.0, 1000))]),
        };
        let at = |x: f64| z(vec![vec![Value::Int(1), Value::Real(x)]]);
        assert!(compare_with_oracles(&r.rows, &r, Some(&at(1.0 + 1.4e-15)), &query).is_ok());
        assert!(compare_with_oracles(&r.rows, &r, Some(&at(1.0 + 1e-13)), &query).is_err());
    }

    #[test]
    fn a_sqlite_real_group_key_requires_a_real_reference_key() {
        let mut r = reference(0.6);
        r.rows = z(vec![vec![Value::Int(1), Value::Real(0.6), Value::Int(1)]]);
        let real_key = z(vec![vec![
            Value::Real(1.0),
            Value::Real(0.6),
            Value::Int(1),
        ]]);
        assert!(compare_with_oracles(&r.rows, &r, Some(&real_key), &q()).is_err());
    }

    #[test]
    fn sqlite_intermediate_real_overflow_requires_a_large_magnitude() {
        for func in [AggFn::Sum, AggFn::Total, AggFn::Avg] {
            let mut query = q();
            query.aggs[0].func = func;
            let mut r = reference(0.6);
            for magnitude in [f64::MAX / 4.0, f64::MAX, f64::INFINITY] {
                r.magnitudes
                    .insert((Row::new(vec![Value::Int(1)]), 0), (magnitude, 3));
                for value in [
                    Value::Real(f64::INFINITY),
                    Value::Real(f64::NEG_INFINITY),
                    Value::Null,
                ] {
                    let sqlite = z(vec![vec![Value::Int(1), value, Value::Int(1)]]);
                    assert!(
                        compare_with_oracles(&r.rows, &r, Some(&sqlite), &query).is_ok(),
                        "{func:?}, magnitude {magnitude}"
                    );
                }
            }
            for magnitude in [0.6, (f64::MAX / 4.0).next_down()] {
                r.magnitudes
                    .insert((Row::new(vec![Value::Int(1)]), 0), (magnitude, 3));
                for value in [
                    Value::Real(f64::INFINITY),
                    Value::Real(f64::NEG_INFINITY),
                    Value::Null,
                ] {
                    let sqlite = z(vec![vec![Value::Int(1), value, Value::Int(1)]]);
                    assert!(compare_with_oracles(&r.rows, &r, Some(&sqlite), &query).is_err());
                }
            }
        }
    }

    #[test]
    fn sqlite_infinity_cannot_match_a_finite_exact_aggregate() {
        // Spec §6 gives no infinity exemption from the KBN comparison.
        for func in [AggFn::Sum, AggFn::Total, AggFn::Avg] {
            let mut query = q();
            query.aggs[0].func = func;
            let r = reference(0.6);
            for infinity in [f64::INFINITY, f64::NEG_INFINITY] {
                let inf = z(vec![vec![
                    Value::Int(1),
                    Value::Real(infinity),
                    Value::Int(1),
                ]]);
                assert!(
                    compare_with_oracles(&r.rows, &r, Some(&inf), &query).is_err(),
                    "{func:?}: {infinity}"
                );
                let inf_ref = reference(infinity);
                assert!(compare_with_oracles(&inf_ref.rows, &inf_ref, Some(&inf), &query).is_ok());
                assert!(
                    compare_with_oracles(&inf_ref.rows, &inf_ref, Some(&r.rows), &query).is_err()
                );
                let opposite = reference(-infinity);
                assert!(compare_with_oracles(
                    &inf_ref.rows,
                    &inf_ref,
                    Some(&opposite.rows),
                    &query
                )
                .is_err());
            }
        }
        let r = reference(0.6);
        // An infinite reference is never excused.
        let inf_ref = Evaluated {
            rows: z(vec![vec![
                Value::Real(1.0),
                Value::Real(f64::INFINITY),
                Value::Int(1),
            ]]),
            magnitudes: r.magnitudes.clone(),
        };
        let finite = z(vec![vec![Value::Int(1), Value::Real(0.6), Value::Int(1)]]);
        assert!(compare_with_oracles(&inf_ref.rows, &inf_ref, Some(&finite), &q()).is_err());
        // Nor is a SQLite infinity on MIN.
        let inf_min = z(vec![vec![
            Value::Int(1),
            Value::Real(0.6),
            Value::Real(f64::INFINITY),
        ]]);
        assert!(compare_with_oracles(&r.rows, &r, Some(&inf_min), &q()).is_err());
    }
}
