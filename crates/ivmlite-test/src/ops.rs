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
    /// UPDATE 拆成 retract + insert——写进 delta 的内容本身已经是 Z-set（spec §8.1）。
    pub fn to_delta(&self) -> Vec<(Row, i64)> {
        match self {
            Op::Insert(r) => vec![(r.clone(), 1)],
            Op::Delete(r) => vec![(r.clone(), -1)],
            Op::Update { old, new } => vec![(old.clone(), -1), (new.clone(), 1)],
        }
    }
}

/// 生成有偏的、带表标签的多表更新序列。
///
/// spec §9.2：纯随机生成器在 IVM 测试里几乎抓不到 bug——随机 DELETE 很少
/// 命中真实存在的行。这里为 `db` 里的每张表各自维护一份 live 行集合，
/// DELETE / UPDATE 一律从对应表的 live 集合里采样，于是"删掉刚插入的行"
/// 和"把一个 group 删空再填回来"会自然高频发生——且不会出现用一张表的行
/// 去删另一张表这种非法序列。
///
/// 每一步先均匀选表、再选操作：选表必须是均匀分布，否则 join 算子两侧
/// `ΔR⋈S` 与 `R⋈ΔS` 的覆盖会失衡。live 集合按 `db.tables()` 的下标存成
/// `Vec`（m3 更正：不是为了迭代顺序确定——`live` 在本函数里只按 `t_idx`
/// 索引，从不整体迭代，把它换成 `HashMap<usize, Vec<Row>>` 一样能保证同一
/// 个 `t_idx` 每次取到同一张表。真正保证"选表结果与 seed 确定绑定"的是
/// `db.tables()` 本身：它返回的 `Vec<Schema>` 保留插入顺序，这条由
/// `ivmlite-core` 的 `table_order_is_preserved`（`database.rs`）守护，
/// 不是这里的 `Vec` 选择（spec §9.4）。
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

        // live 为空时只能插入。
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

        // 重放序列，验证每个 DELETE / UPDATE 命中的行当时确实存在。
        let mut live: Vec<Row> = initial.clone();
        let mut hits = 0usize;
        for (table, op) in &ops {
            assert_eq!(table, &schema.table, "单表用例不应出现别的表名");
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

    /// item 12（deferred minor）：`initial` 为空时 live 集合从空开始，且
    /// 每次 insert 只会往 live 里加，不会自然变空——真正会命中"live 为空"
    /// 守卫的只有第一次迭代。删掉那条守卫（`gen_ops` 里的
    /// `if live.is_empty() { 0 } else { ... }`）本该在这里 panic，但用一个
    /// 空 `initial` 加大 `count` 只测得到第一步，之后 live 已非空——所以
    /// 这条测试断言的是"第一步在 live 为空时必须是 Insert 且不 panic"。
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
            "live 集合为空时第一步必须是 Insert，实得 {:?}",
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
                "spec §9.2 第 4 条：差分 schema 固定每表 2 列——加宽到 3 列会让穷举规模涨约 8 倍，\
                 这是「穷举优于随机」成立的前提，不是魔数"
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
            assert!(db.get(&table).is_some(), "未知表 {table}");
        }
    }

    #[test]
    fn every_table_receives_some_ops() {
        // 每张表至少拿到均匀选表下期望份额的一半——若生成器把选表概率往某张
        // 表偏斜，join 的 ΔR⋈S 与 R⋈ΔS 两条路径的覆盖就会失衡。这个下界不
        // 证明选表就是均匀的，只保证偏得太狠会被抓到：300 次操作、2 张表时
        // 均匀选表下任何一张跌破这条线的概率约 2.4e-19，而 90/10 的偏斜下
        // 少数表期望只有 30，会可靠地跌破 75 这条线。
        let mut rng = StdRng::seed_from_u64(3);
        let db = gen_database(2);
        let domain = Domain::default();
        let initial = gen_initial(&mut rng, &db, &domain, 20);
        let count = 300;
        let ops = gen_ops(&mut rng, &db, &domain, &initial, count);
        let floor = count / db.len() / 2; // 均匀份额的一半
        for t in db.tables() {
            let n = ops.iter().filter(|(tbl, _)| *tbl == t.table).count();
            assert!(
                n > floor,
                "表 {} 只收到 {n} 个操作（下界 {floor}），两侧 delta 路径覆盖不均",
                t.table
            );
        }
    }

    #[test]
    fn deletes_target_rows_that_exist_in_their_own_table() {
        // 有偏采样必须按表各自维护 live 集合——用一张表的行去删另一张表是非法序列。
        let mut rng = StdRng::seed_from_u64(4);
        let db = gen_database(2);
        let domain = Domain::default();
        let initial = gen_initial(&mut rng, &db, &domain, 30);
        let mut live: BTreeMap<String, Vec<Row>> = initial.clone();
        let mut hits = 0usize;
        let ops = gen_ops(&mut rng, &db, &domain, &initial, 300);
        for (table, op) in &ops {
            let l = live.get_mut(table).expect("表必须存在");
            match op {
                Op::Insert(r) => l.push(r.clone()),
                Op::Delete(r) => {
                    let pos = l
                        .iter()
                        .position(|x| x == r)
                        .expect("DELETE 必须命中本表存在的行");
                    l.swap_remove(pos);
                    hits += 1;
                }
                Op::Update { old, new } => {
                    let pos = l
                        .iter()
                        .position(|x| x == old)
                        .expect("UPDATE 必须命中本表存在的行");
                    l.swap_remove(pos);
                    l.push(new.clone());
                    hits += 1;
                }
            }
        }
        assert!(
            hits > ops.len() / 10,
            "有偏采样产出的删改过少：{hits}/{}",
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
