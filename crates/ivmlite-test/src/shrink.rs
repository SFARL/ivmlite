use std::collections::BTreeMap;

use ivmlite_core::Row;

use crate::{run, Engine, Op, Predicate, TestCase};

fn still_fails<E, F>(case: &TestCase, make: &F) -> bool
where
    E: Engine,
    F: Fn() -> E,
{
    let mut engine = make();
    run(&mut engine, case).is_err()
}

/// Sequence legality: every DELETE / UPDATE must hit a row that exists at that point **in its own table**.
///
/// This is why the shrinker is written in-house rather than using proptest: a
/// naive shrink deletes some INSERT and leaves a later DELETE of that row
/// dangling, producing an illegal sequence the engine was never obliged to
/// handle, which makes the "failure" meaningless (spec §9.3). It is the
/// in-house shrinker's one load-bearing property, and so it is `pub`: callers
/// (integration tests included) can re-assert legality on `shrink`'s output
/// directly instead of trusting that shrink uses it correctly internally.
///
/// With multiple tables this is easier to get wrong: deleting from table B with
/// a row from table A is an illegal sequence, and the engine was never obliged
/// to handle illegal input — "failing" on an illegal sequence means nothing,
/// which is the whole reason for an in-house shrinker over proptest (spec §9.3).
pub fn is_legal(initial: &BTreeMap<String, Vec<Row>>, ops: &[(String, Op)]) -> bool {
    let mut live: BTreeMap<String, Vec<Row>> = initial.clone();
    for (table, op) in ops {
        let Some(l) = live.get_mut(table) else {
            return false; // unknown table
        };
        match op {
            Op::Insert(r) => l.push(r.clone()),
            Op::Delete(r) => match l.iter().position(|x| x == r) {
                Some(i) => {
                    l.swap_remove(i);
                }
                None => return false,
            },
            Op::Update { old, new } => match l.iter().position(|x| x == old) {
                Some(i) => {
                    l.swap_remove(i);
                    l.push(new.clone());
                }
                None => return false,
            },
        }
    }
    true
}

/// Shrink a failing case to a minimum. The order follows spec §9.3: **shrink
/// the update sequence first, then the query, then the data**. Every step
/// requires the smaller case to **still be legal and still fail**.
pub fn shrink<E, F>(case: &TestCase, make: F) -> TestCase
where
    E: Engine,
    F: Fn() -> E,
{
    let mut best = case.clone();

    // Phase one: delete ranges of ops with delta-debugging's shrinking granularity.
    let mut granularity = best.ops.len().max(1);
    while granularity >= 1 {
        let mut improved = true;
        while improved {
            improved = false;
            let chunk = (best.ops.len() / granularity).max(1);
            let mut start = 0;
            while start < best.ops.len() {
                let end = (start + chunk).min(best.ops.len());
                let mut ops = best.ops.clone();
                ops.drain(start..end);

                if is_legal(&best.initial, &ops) {
                    let candidate = TestCase {
                        ops,
                        ..best.clone()
                    };
                    if still_fails(&candidate, &make) {
                        best = candidate;
                        improved = true;
                        continue; // do not advance start; keep trying at the same position
                    }
                }
                start = end;
            }
        }
        if granularity == 1 {
            break;
        }
        granularity /= 2;
    }

    // Phase two: shrink the query. Shrinking the query does not affect sequence
    // legality (legality concerns rows, not the query), so no is_legal gate is
    // needed here.
    loop {
        let mut improved = false;

        // Drop an aggregate, keeping at least one
        if best.query.aggs.len() > 1 {
            for i in 0..best.query.aggs.len() {
                let mut query = best.query.clone();
                query.aggs.remove(i);
                let candidate = TestCase {
                    query,
                    ..best.clone()
                };
                if still_fails(&candidate, &make) {
                    best = candidate;
                    improved = true;
                    break;
                }
            }
        }

        // Drop a group-by column, keeping at least one
        if !improved && best.query.group_by.len() > 1 {
            for i in 0..best.query.group_by.len() {
                let mut query = best.query.clone();
                query.group_by.remove(i);
                let candidate = TestCase {
                    query,
                    ..best.clone()
                };
                if still_fails(&candidate, &make) {
                    best = candidate;
                    improved = true;
                    break;
                }
            }
        }

        // Degrade the predicate to None
        if !improved && best.query.predicate != Predicate::None {
            let mut query = best.query.clone();
            query.predicate = Predicate::None;
            let candidate = TestCase {
                query,
                ..best.clone()
            };
            if still_fails(&candidate, &make) {
                best = candidate;
                improved = true;
            }
        }

        if !improved {
            break;
        }
    }

    // Phase three: delete initial rows one by one, table by table.
    let tables: Vec<String> = best.initial.keys().cloned().collect();
    for table in tables {
        let mut i = 0;
        loop {
            let len = best.initial.get(&table).map_or(0, Vec::len);
            if i >= len {
                break;
            }
            let mut initial = best.initial.clone();
            initial
                .get_mut(&table)
                .expect("the table must exist")
                .remove(i);
            if is_legal(&initial, &best.ops) {
                let candidate = TestCase {
                    initial,
                    ..best.clone()
                };
                if still_fails(&candidate, &make) {
                    best = candidate;
                    continue; // do not advance i
                }
            }
            i += 1;
        }
    }

    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use ivmlite_core::Value;

    fn row(n: i64) -> Row {
        Row::new(vec![Value::Int(n)])
    }

    fn initial(rows: Vec<Row>) -> BTreeMap<String, Vec<Row>> {
        BTreeMap::from([("t0".to_string(), rows)])
    }

    fn ops_on(table: &str, ops: Vec<Op>) -> Vec<(String, Op)> {
        ops.into_iter().map(|op| (table.to_string(), op)).collect()
    }

    /// I2: this is the one reason the shrinker is in-house rather than proptest
    /// (spec §9.3). It unit-tests `is_legal` itself directly, not only through
    /// indirect integration-test assertions.
    #[test]
    fn dangling_delete_is_illegal() {
        // Row 1 never existed (initial is empty), so deleting it must be illegal.
        assert!(!is_legal(
            &initial(vec![]),
            &ops_on("t0", vec![Op::Delete(row(1))])
        ));
    }

    #[test]
    fn dangling_update_is_illegal() {
        // old=row(1) is not in the live set, so the UPDATE must be illegal.
        assert!(!is_legal(
            &initial(vec![]),
            &ops_on(
                "t0",
                vec![Op::Update {
                    old: row(1),
                    new: row(2)
                }]
            )
        ));
    }

    #[test]
    fn legal_sequence_is_legal() {
        // insert 1 → delete 1 → insert 2 → update 2->3: every step hits a row that exists at that point.
        let ops = ops_on(
            "t0",
            vec![
                Op::Insert(row(1)),
                Op::Delete(row(1)),
                Op::Insert(row(2)),
                Op::Update {
                    old: row(2),
                    new: row(3),
                },
            ],
        );
        assert!(is_legal(&initial(vec![]), &ops));
    }

    #[test]
    fn delete_of_a_row_present_in_initial_is_legal() {
        assert!(is_legal(
            &initial(vec![row(1)]),
            &ops_on("t0", vec![Op::Delete(row(1))])
        ));
    }

    #[test]
    fn delete_after_insert_of_a_different_row_is_illegal() {
        // insert 1, then delete 2 — 2 never existed.
        assert!(!is_legal(
            &initial(vec![]),
            &ops_on("t0", vec![Op::Insert(row(1)), Op::Delete(row(2))])
        ));
    }

    #[test]
    fn deleting_a_row_that_exists_in_another_table_is_illegal() {
        // With one table this shape does not exist; with several it is the cell most easily got wrong.
        let initial =
            BTreeMap::from([("t0".to_string(), vec![row(1)]), ("t1".to_string(), vec![])]);
        let ops = vec![("t1".to_string(), Op::Delete(row(1)))];
        assert!(
            !is_legal(&initial, &ops),
            "t1 does not have this row, even though t0 does"
        );
    }
}
