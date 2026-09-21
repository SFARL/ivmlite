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

/// 序列的合法性：每个 DELETE / UPDATE 必须命中**它自己那张表**当时存在的行。
///
/// 这是自研 shrinker 而非直接用 proptest 的原因——朴素的缩小会删掉某个
/// INSERT，让后续针对该行的 DELETE 悬空，产出一个引擎本就不该处理的非法
/// 序列，于是"失败"变得毫无意义（spec §9.3）。这是自研 shrinker 唯一的
/// load-bearing 性质，因此是 `pub`：调用方（包括集成测试）可以直接对
/// `shrink` 的产出重新断言合法性，而不是只信任 shrink 内部没有用错它。
///
/// 多表化之后这条更容易出错：用 A 表的行去删 B 表是非法序列，而引擎从来
/// 没有义务处理非法输入——在非法序列上「失败」毫无意义，这正是自研 shrinker
/// 而不用 proptest 的全部理由（spec §9.3）。
pub fn is_legal(initial: &BTreeMap<String, Vec<Row>>, ops: &[(String, Op)]) -> bool {
    let mut live: BTreeMap<String, Vec<Row>> = initial.clone();
    for (table, op) in ops {
        let Some(l) = live.get_mut(table) else {
            return false; // 未知表
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

/// 把失败用例缩到最小。顺序遵循 spec §9.3：**先缩更新序列，再缩 query，
/// 最后缩数据**。每一步都要求缩小后的用例**仍然合法且仍然失败**。
pub fn shrink<E, F>(case: &TestCase, make: F) -> TestCase
where
    E: Engine,
    F: Fn() -> E,
{
    let mut best = case.clone();

    // 阶段一：按 delta-debugging 的粒度递减删除 op 区间。
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
                        continue; // 不推进 start，同一位置继续尝试
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

    // 阶段二：缩小 query。缩 query 不影响序列合法性(合法性只关乎行，不关乎查询)，
    // 所以这里不需要 is_legal 门禁。
    loop {
        let mut improved = false;

        // 去掉一个聚合，至少保留一个
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

        // 去掉一个 group-by 列，至少保留一个
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

        // 谓词退化成 None
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

    // 阶段三：逐张表、逐条删除初始行。
    let tables: Vec<String> = best.initial.keys().cloned().collect();
    for table in tables {
        let mut i = 0;
        loop {
            let len = best.initial.get(&table).map_or(0, Vec::len);
            if i >= len {
                break;
            }
            let mut initial = best.initial.clone();
            initial.get_mut(&table).expect("表必须存在").remove(i);
            if is_legal(&initial, &best.ops) {
                let candidate = TestCase {
                    initial,
                    ..best.clone()
                };
                if still_fails(&candidate, &make) {
                    best = candidate;
                    continue; // 不推进 i
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

    /// I2：这是自研 shrinker 而非直接用 proptest 的唯一理由（spec §9.3）。
    /// 直接单元测试 `is_legal` 本身，而不是只通过间接的集成测试断言。
    #[test]
    fn dangling_delete_is_illegal() {
        // 行 1 从未存在过（initial 为空），删它必须判非法。
        assert!(!is_legal(
            &initial(vec![]),
            &ops_on("t0", vec![Op::Delete(row(1))])
        ));
    }

    #[test]
    fn dangling_update_is_illegal() {
        // old=row(1) 不在 live 集合里，UPDATE 必须判非法。
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
        // insert 1 → delete 1 → insert 2 → update 2->3：每一步都命中当时存在的行。
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
        // insert 1，然后删 2——2 从未存在过。
        assert!(!is_legal(
            &initial(vec![]),
            &ops_on("t0", vec![Op::Insert(row(1)), Op::Delete(row(2))])
        ));
    }

    #[test]
    fn deleting_a_row_that_exists_in_another_table_is_illegal() {
        // 单表时这个形态根本不存在；多表化后它是最容易被写错的一格。
        let initial =
            BTreeMap::from([("t0".to_string(), vec![row(1)]), ("t1".to_string(), vec![])]);
        let ops = vec![("t1".to_string(), Op::Delete(row(1)))];
        assert!(!is_legal(&initial, &ops), "t1 里没有这一行，即便 t0 里有");
    }
}
