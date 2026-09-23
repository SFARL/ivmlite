use std::collections::BTreeMap;

use ivmlite_core::{Database, Row};
use rand::rngs::StdRng;
use rand::RngExt;

use crate::{gen_row, Domain};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Op {
    Insert(Row),
    Delete(Row),
    Update { old: Row, new: Row },
}

impl Op {
    /// UPDATE splits into a retract plus an insert — what is written to the delta is itself already a Z-set (spec §8.1).
    pub fn to_delta(&self) -> Vec<(Row, i64)> {
        match self {
            Op::Insert(r) => vec![(r.clone(), 1)],
            Op::Delete(r) => vec![(r.clone(), -1)],
            Op::Update { old, new } => vec![(old.clone(), -1), (new.clone(), 1)],
        }
    }
}

/// Generate a biased, table-tagged, multi-table update sequence.
///
/// Spec §9.2: a purely random generator catches almost no bugs in IVM testing —
/// a random DELETE rarely hits a row that actually exists. Here each table in
/// `db` keeps its own set of live rows, and every DELETE / UPDATE samples from
/// its own table's live set, so "delete a row just inserted" and "empty a group
/// and fill it back up" happen naturally and often — and the illegal sequence
/// of deleting from one table with another table's row cannot occur.
///
/// Each step picks a table uniformly, then an operation. Table choice must be
/// uniform, or the join operator's two paths `ΔR⋈S` and `R⋈ΔS` get uneven
/// coverage. The live sets are stored as a `Vec` indexed like `db.tables()` (m3
/// correction: not for a deterministic iteration order — `live` is only ever
/// indexed by `t_idx` in this function and never iterated as a whole, so a
/// `HashMap<usize, Vec<Row>>` would equally make the same `t_idx` pick the same
/// table every time. What binds the choice of table to the seed is
/// `db.tables()` itself: the `Vec<Schema>` it returns keeps insertion order,
/// guarded by `table_order_is_preserved` in `ivmlite-core`'s `database.rs`, not
/// by the `Vec` chosen here; spec §9.4).
pub fn gen_ops(
    rng: &mut StdRng,
    db: &Database,
    domain: &Domain,
    initial: &BTreeMap<String, Vec<Row>>,
    count: usize,
) -> Vec<(String, Op)> {
    let tables = db.tables();
    let mut live: Vec<Vec<Row>> = tables
        .iter()
        .map(|s| initial.get(&s.table).cloned().unwrap_or_default())
        .collect();

    let mut ops = Vec::with_capacity(count);

    for _ in 0..count {
        let t_idx = rng.random_range(0..tables.len());
        let schema = &tables[t_idx];
        let table_live = &mut live[t_idx];

        // With an empty live set, only inserts are possible.
        let choice = if table_live.is_empty() {
            0
        } else {
            rng.random_range(0..10)
        };
        match choice {
            0..=3 => {
                let r = gen_row(rng, schema, domain);
                table_live.push(r.clone());
                ops.push((schema.table.clone(), Op::Insert(r)));
            }
            4..=6 => {
                let idx = rng.random_range(0..table_live.len());
                let r = table_live.swap_remove(idx);
                ops.push((schema.table.clone(), Op::Delete(r)));
            }
            _ => {
                let idx = rng.random_range(0..table_live.len());
                let old = table_live.swap_remove(idx);
                let new = gen_row(rng, schema, domain);
                table_live.push(new.clone());
                ops.push((schema.table.clone(), Op::Update { old, new }));
            }
        }
    }
    ops
}

#[cfg(test)]
mod tests {
    use super::{gen_ops, Op};
    use crate::test_support::{as_initial, single_table_db};
    use crate::{gen_database, gen_initial, Column, ColumnType, Domain, Schema};
    use ivmlite_core::{Row, Value};
    use rand::rngs::StdRng;
    use rand::SeedableRng;
    use std::collections::BTreeMap;

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

    fn row(n: i64) -> Row {
        Row::new(vec![Value::Text(format!("v{n}")), Value::Int(n)])
    }

    #[test]
    fn update_becomes_retract_plus_insert() {
        let op = Op::Update {
            old: row(1),
            new: row(2),
        };
        assert_eq!(op.to_delta(), vec![(row(1), -1), (row(2), 1)]);
    }

    #[test]
    fn insert_and_delete_map_to_plus_and_minus_one() {
        assert_eq!(Op::Insert(row(1)).to_delta(), vec![(row(1), 1)]);
        assert_eq!(Op::Delete(row(1)).to_delta(), vec![(row(1), -1)]);
    }

    #[test]
    fn deletes_target_rows_that_actually_exist() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(11);
        let schema = orders();
        let domain = Domain::default();
        let db = single_table_db(&schema);
        let initial = crate::gen_rows(&mut rng, &schema, &domain, 40);
        let initial_map = as_initial(&schema, initial.clone());
        let ops = gen_ops(&mut rng, &db, &domain, &initial_map, 300);

        // Replay the sequence, checking every DELETE / UPDATE hits a row that exists at that point.
        let mut live: Vec<Row> = initial.clone();
        let mut hits = 0usize;
        for (table, op) in &ops {
            assert_eq!(
                table, &schema.table,
                "a single-table case should not mention another table"
            );
            match op {
                Op::Insert(r) => live.push(r.clone()),
                Op::Delete(r) => {
                    let pos = live.iter().position(|x| x == r);
                    assert!(pos.is_some(), "a DELETE must hit a row that exists");
                    live.remove(pos.unwrap());
                    hits += 1;
                }
                Op::Update { old, new } => {
                    let pos = live.iter().position(|x| x == old);
                    assert!(pos.is_some(), "an UPDATE must hit a row that exists");
                    live.remove(pos.unwrap());
                    live.push(new.clone());
                    hits += 1;
                }
            }
        }
        assert!(
            hits > ops.len() / 10,
            "biased sampling must produce enough deletes and updates, or retraction goes untested; got {hits}/{}",
            ops.len()
        );
    }

    /// Item 12 (a deferred minor): with an empty `initial` the live set starts
    /// empty, and each insert only adds to it, so it does not become empty on
    /// its own — only the first iteration really hits the "live is empty" guard.
    /// Deleting that guard (`if live.is_empty() { 0 } else { ... }` in
    /// `gen_ops`) should panic here, but an empty `initial` with a larger
    /// `count` exercises only the first step, after which live is non-empty — so
    /// this test asserts "with an empty live set, the first step must be an
    /// Insert, without panicking".
    #[test]
    fn empty_live_set_only_ever_produces_an_insert_first() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(123);
        let schema = orders();
        let domain = Domain::default();
        let db = single_table_db(&schema);
        let ops = gen_ops(&mut rng, &db, &domain, &BTreeMap::new(), 20);
        assert_eq!(ops.len(), 20);
        assert!(
            matches!(ops[0].1, Op::Insert(_)),
            "with an empty live set the first step must be an Insert, got {:?}",
            ops[0]
        );
    }

    #[test]
    fn sequence_is_reproducible_from_seed() {
        let schema = orders();
        let domain = Domain::default();
        let db = single_table_db(&schema);
        let make = || {
            let mut rng = rand::rngs::StdRng::seed_from_u64(99);
            let initial = crate::gen_rows(&mut rng, &schema, &domain, 10);
            let initial_map = as_initial(&schema, initial);
            gen_ops(&mut rng, &db, &domain, &initial_map, 50)
        };
        assert_eq!(make(), make());
    }

    #[test]
    fn generated_database_tables_have_exactly_two_columns() {
        let db = gen_database(2);
        assert_eq!(db.len(), 2);
        for t in db.tables() {
            assert_eq!(
                t.arity(),
                2,
                "spec §9.2 item 4: the differential schema is fixed at 2 columns per table — \
                 widening to 3 grows the enumeration about 8x; it is the precondition for \
                 \"enumeration beats randomness\", not a magic number"
            );
        }
    }

    #[test]
    fn ops_are_tagged_with_a_table_that_exists() {
        let mut rng = StdRng::seed_from_u64(2);
        let db = gen_database(2);
        let domain = Domain::default();
        let initial = gen_initial(&mut rng, &db, &domain, 20);
        for (table, _) in gen_ops(&mut rng, &db, &domain, &initial, 200) {
            assert!(db.get(&table).is_some(), "unknown table {table}");
        }
    }

    #[test]
    fn every_table_receives_some_ops() {
        // Every table gets at least half its expected share under uniform
        // choice — if the generator skewed table choice toward one table, join's
        // two paths ΔR⋈S and R⋈ΔS would get uneven coverage. This lower bound
        // does not prove the choice is uniform; it only guarantees a large skew
        // is caught: with 300 operations and 2 tables, the chance of either
        // falling below the line under uniform choice is about 2.4e-19, while
        // under a 90/10 skew the minority table expects only 30 and reliably
        // falls below 75.
        let mut rng = StdRng::seed_from_u64(3);
        let db = gen_database(2);
        let domain = Domain::default();
        let initial = gen_initial(&mut rng, &db, &domain, 20);
        let count = 300;
        let ops = gen_ops(&mut rng, &db, &domain, &initial, count);
        let floor = count / db.len() / 2; // half the uniform share
        for t in db.tables() {
            let n = ops.iter().filter(|(tbl, _)| *tbl == t.table).count();
            assert!(
                n > floor,
                "table {} received only {n} operations (lower bound {floor}), leaving the two delta paths unevenly covered",
                t.table
            );
        }
    }

    #[test]
    fn deletes_target_rows_that_exist_in_their_own_table() {
        // Biased sampling must keep a live set per table — deleting from one table with another's row is an illegal sequence.
        let mut rng = StdRng::seed_from_u64(4);
        let db = gen_database(2);
        let domain = Domain::default();
        let initial = gen_initial(&mut rng, &db, &domain, 30);
        let mut live: BTreeMap<String, Vec<Row>> = initial.clone();
        let mut hits = 0usize;
        let ops = gen_ops(&mut rng, &db, &domain, &initial, 300);
        for (table, op) in &ops {
            let l = live.get_mut(table).expect("the table must exist");
            match op {
                Op::Insert(r) => l.push(r.clone()),
                Op::Delete(r) => {
                    let pos = l
                        .iter()
                        .position(|x| x == r)
                        .expect("a DELETE must hit a row that exists in its own table");
                    l.swap_remove(pos);
                    hits += 1;
                }
                Op::Update { old, new } => {
                    let pos = l
                        .iter()
                        .position(|x| x == old)
                        .expect("an UPDATE must hit a row that exists in its own table");
                    l.swap_remove(pos);
                    l.push(new.clone());
                    hits += 1;
                }
            }
        }
        assert!(
            hits > ops.len() / 10,
            "biased sampling produced too few deletes and updates: {hits}/{}",
            ops.len()
        );
    }

    #[test]
    fn same_seed_yields_the_same_multi_table_sequence() {
        let make = || {
            let mut rng = StdRng::seed_from_u64(99);
            let db = gen_database(2);
            let domain = Domain::default();
            let initial = gen_initial(&mut rng, &db, &domain, 10);
            gen_ops(&mut rng, &db, &domain, &initial, 50)
        };
        assert_eq!(make(), make());
    }
}
