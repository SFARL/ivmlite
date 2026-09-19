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

/// 序列的合法性：每个 DELETE / UPDATE 必须命中当时存在的行。
///
/// 这是自研 shrinker 而非直接用 proptest 的原因——朴素的缩小会删掉某个
/// INSERT，让后续针对该行的 DELETE 悬空，产出一个引擎本就不该处理的非法
/// 序列，于是"失败"变得毫无意义（spec §9.3）。
fn is_legal(initial: &[Row], ops: &[Op]) -> bool {
    let mut live: Vec<Row> = initial.to_vec();
    for op in ops {
        match op {
            Op::Insert(r) => live.push(r.clone()),
            Op::Delete(r) => match live.iter().position(|x| x == r) {
                Some(i) => {
                    live.swap_remove(i);
                }
                None => return false,
            },
            Op::Update { old, new } => match live.iter().position(|x| x == old) {
                Some(i) => {
                    live.swap_remove(i);
                    live.push(new.clone());
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

    // 阶段三：逐条删除初始行。
    let mut i = 0;
    while i < best.initial.len() {
        let mut initial = best.initial.clone();
        initial.remove(i);
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

    best
}
