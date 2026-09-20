use std::collections::btree_map::Entry;
use std::collections::BTreeMap;

use crate::Row;

/// 带权重的多重集。权重为 i64：INSERT = +1，DELETE = -1。
///
/// 用 BTreeMap 而非 HashMap，是为了让迭代顺序确定——差分测试的失败用例
/// 必须能凭 seed 精确重放。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ZSet {
    inner: BTreeMap<Row, i64>,
}

impl ZSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_rows(rows: impl IntoIterator<Item = (Row, i64)>) -> Self {
        let mut z = Self::new();
        for (row, weight) in rows {
            z.update(row, weight);
        }
        z
    }

    /// 把 `weight` 加到 `row` 现有的权重上。归零的行会被移除。
    pub fn update(&mut self, row: Row, weight: i64) {
        if weight == 0 {
            return;
        }
        match self.inner.entry(row) {
            Entry::Occupied(mut slot) => {
                let combined = *slot.get() + weight;
                if combined == 0 {
                    slot.remove();
                } else {
                    *slot.get_mut() = combined;
                }
            }
            Entry::Vacant(slot) => {
                slot.insert(weight);
            }
        }
    }

    pub fn merge(&mut self, other: &ZSet) {
        for (row, weight) in other.iter() {
            self.update(row.clone(), *weight);
        }
    }

    pub fn weight_of(&self, row: &Row) -> i64 {
        self.inner.get(row).copied().unwrap_or(0)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Row, &i64)> {
        self.inner.iter()
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Value;

    fn row(n: i64) -> Row {
        Row::new(vec![Value::Int(n)])
    }

    #[test]
    fn weights_accumulate() {
        let mut z = ZSet::new();
        z.update(row(1), 1);
        z.update(row(1), 2);
        assert_eq!(z.weight_of(&row(1)), 3);
    }

    #[test]
    fn zero_weight_rows_are_removed_not_kept() {
        let mut z = ZSet::new();
        z.update(row(1), 1);
        z.update(row(1), -1);
        assert_eq!(z.weight_of(&row(1)), 0);
        assert_eq!(z.len(), 0, "权重归零的行必须删除，不得留 w=0 的僵尸行");
        assert!(z.is_empty());
    }

    #[test]
    fn update_with_zero_weight_is_a_noop() {
        let mut z = ZSet::new();
        z.update(row(1), 0);
        assert_eq!(z.len(), 0);
    }

    #[test]
    fn negative_weights_are_representable() {
        let mut z = ZSet::new();
        z.update(row(1), -2);
        assert_eq!(z.weight_of(&row(1)), -2);
    }

    #[test]
    fn merge_is_pointwise_addition() {
        let mut a = ZSet::from_rows([(row(1), 1), (row(2), 5)]);
        let b = ZSet::from_rows([(row(1), -1), (row(3), 2)]);
        a.merge(&b);
        assert_eq!(a.weight_of(&row(1)), 0);
        assert_eq!(a.weight_of(&row(2)), 5);
        assert_eq!(a.weight_of(&row(3)), 2);
        assert_eq!(a.len(), 2, "row(1) 归零后应被移除");
    }

    #[test]
    fn iteration_order_is_deterministic() {
        let a = ZSet::from_rows([(row(3), 1), (row(1), 1), (row(2), 1)]);
        let seen: Vec<i64> = a
            .iter()
            .map(|(r, _)| match r.get(0) {
                Value::Int(n) => *n,
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(seen, vec![1, 2, 3], "BTreeMap 保证顺序，HashMap 不保证");
    }
}
