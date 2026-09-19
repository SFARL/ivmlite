use std::collections::BTreeSet;

use ivmlite_core::{Value, ZSet};

use crate::ViewQuery;

/// 不需要 oracle 就能检查的性质（spec §9.1 第一层）。
/// 跑得极快，因此在每一批 delta 之后都检查，而不是只在最后检查。
pub fn check_invariants(state: &ZSet, query: &ViewQuery) -> Result<(), String> {
    let key_arity = query.group_by.len();
    let mut seen: BTreeSet<Vec<Value>> = BTreeSet::new();

    for (row, weight) in state.iter() {
        if *weight < 0 {
            return Err(format!("最终状态出现负权重 {weight}，行 {row:?}"));
        }
        if *weight != 1 {
            return Err(format!(
                "聚合视图的每个 group 应恰好一行、权重为 1，实得权重 {weight}，行 {row:?}"
            ));
        }
        if row.len() != query.output_arity() {
            return Err(format!(
                "输出行宽度 {} 与视图的 {} 不符，行 {row:?}",
                row.len(),
                query.output_arity()
            ));
        }
        let key: Vec<Value> = (0..key_arity).map(|i| row.get(i).clone()).collect();
        if !seen.insert(key.clone()) {
            // item 20（deferred minor）：补上 row:?，与另外三条错误路径一致。
            // 重复 group key 是 M1 最可能的失败模式，缺这个信息等于在最需要
            // 的时刻逼人去翻 ZSet dump。
            return Err(format!("group key {key:?} 在输出中出现多次，行 {row:?}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use ivmlite_core::{Row, Value, ZSet};

    use super::check_invariants;
    use crate::{Agg, AggFn, Predicate, ViewQuery};

    fn q() -> ViewQuery {
        ViewQuery {
            group_by: vec![0],
            aggs: vec![Agg {
                func: AggFn::Count,
                column: None,
            }],
            predicate: Predicate::None,
        }
    }

    fn out(region: &str, count: i64) -> Row {
        Row::new(vec![Value::Text(region.into()), Value::Int(count)])
    }

    #[test]
    fn accepts_a_well_formed_state() {
        let z = ZSet::from_rows([(out("a", 1), 1), (out("b", 2), 1)]);
        assert!(check_invariants(&z, &q()).is_ok());
    }

    #[test]
    fn rejects_negative_weights() {
        let z = ZSet::from_rows([(out("a", 1), -1)]);
        let err = check_invariants(&z, &q()).unwrap_err();
        assert!(err.contains("负权重"), "实得: {err}");
    }

    #[test]
    fn rejects_duplicate_group_keys() {
        // 同一个 group key "a" 出现了两行不同的聚合结果
        let z = ZSet::from_rows([(out("a", 1), 1), (out("a", 2), 1)]);
        let err = check_invariants(&z, &q()).unwrap_err();
        assert!(err.contains("group key"), "实得: {err}");
        // item 20：错误信息必须带上具体是哪一行撞上了重复 key，不能只报 key。
        assert!(err.contains("行"), "实得: {err}");
    }

    #[test]
    fn rejects_weight_greater_than_one_for_aggregate_views() {
        let z = ZSet::from_rows([(out("a", 1), 2)]);
        let err = check_invariants(&z, &q()).unwrap_err();
        assert!(err.contains("权重"), "实得: {err}");
    }

    #[test]
    fn rejects_wrong_row_width() {
        // Missing the count column; row has only 1 column instead of expected 2
        let z = ZSet::from_rows([(Row::new(vec![Value::Text("a".into())]), 1)]);
        let err = check_invariants(&z, &q()).unwrap_err();
        assert!(err.contains("宽度"), "实得: {err}");
    }
}
