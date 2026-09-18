use ivmlite_core::Row;
use rand::rngs::StdRng;
use rand::RngExt;

use crate::{gen_row, Domain, Schema};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    Insert(Row),
    Delete(Row),
    Update { old: Row, new: Row },
}

impl Op {
    /// UPDATE 拆成 retract + insert——写进 delta 的内容本身已经是 Z-set（spec §8.1）。
    pub fn to_delta(&self) -> Vec<(Row, i64)> {
        match self {
            Op::Insert(r) => vec![(r.clone(), 1)],
            Op::Delete(r) => vec![(r.clone(), -1)],
            Op::Update { old, new } => vec![(old.clone(), -1), (new.clone(), 1)],
        }
    }
}

/// 生成有偏的更新序列。
///
/// spec §9.2：纯随机生成器在 IVM 测试里几乎抓不到 bug——随机 DELETE 很少
/// 命中真实存在的行。这里维护一份 live 行集合，DELETE / UPDATE 一律从中采样，
/// 于是"删掉刚插入的行"和"把一个 group 删空再填回来"会自然高频发生。
pub fn gen_ops(
    rng: &mut StdRng,
    schema: &Schema,
    domain: &Domain,
    initial: &[Row],
    count: usize,
) -> Vec<Op> {
    let mut live: Vec<Row> = initial.to_vec();
    let mut ops = Vec::with_capacity(count);

    for _ in 0..count {
        // live 为空时只能插入。
        let choice = if live.is_empty() {
            0
        } else {
            rng.random_range(0..10)
        };
        match choice {
            0..=3 => {
                let r = gen_row(rng, schema, domain);
                live.push(r.clone());
                ops.push(Op::Insert(r));
            }
            4..=6 => {
                let idx = rng.random_range(0..live.len());
                let r = live.swap_remove(idx);
                ops.push(Op::Delete(r));
            }
            _ => {
                let idx = rng.random_range(0..live.len());
                let old = live.swap_remove(idx);
                let new = gen_row(rng, schema, domain);
                live.push(new.clone());
                ops.push(Op::Update { old, new });
            }
        }
    }
    ops
}

#[cfg(test)]
mod tests {
    use super::{gen_ops, Op};
    use crate::{Column, ColumnType, Domain, Schema};
    use ivmlite_core::{Row, Value};
    use rand::SeedableRng;

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
        let initial = crate::gen_rows(&mut rng, &schema, &domain, 40);
        let ops = gen_ops(&mut rng, &schema, &domain, &initial, 300);

        // 重放序列，验证每个 DELETE / UPDATE 命中的行当时确实存在。
        let mut live: Vec<Row> = initial.clone();
        let mut hits = 0usize;
        for op in &ops {
            match op {
                Op::Insert(r) => live.push(r.clone()),
                Op::Delete(r) => {
                    let pos = live.iter().position(|x| x == r);
                    assert!(pos.is_some(), "DELETE 必须命中存在的行");
                    live.remove(pos.unwrap());
                    hits += 1;
                }
                Op::Update { old, new } => {
                    let pos = live.iter().position(|x| x == old);
                    assert!(pos.is_some(), "UPDATE 必须命中存在的行");
                    live.remove(pos.unwrap());
                    live.push(new.clone());
                    hits += 1;
                }
            }
        }
        assert!(
            hits > ops.len() / 10,
            "有偏采样必须产生足量的删改，否则测不到 retraction；实得 {hits}/{}",
            ops.len()
        );
    }

    #[test]
    fn sequence_is_reproducible_from_seed() {
        let schema = orders();
        let domain = Domain::default();
        let make = || {
            let mut rng = rand::rngs::StdRng::seed_from_u64(99);
            let initial = crate::gen_rows(&mut rng, &schema, &domain, 10);
            gen_ops(&mut rng, &schema, &domain, &initial, 50)
        };
        assert_eq!(make(), make());
    }
}
