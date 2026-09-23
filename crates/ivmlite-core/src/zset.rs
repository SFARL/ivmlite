use std::collections::btree_map::Entry;
use std::collections::BTreeMap;

use crate::Row;

/// A multiset with weights. Weights are i64: INSERT = +1, DELETE = -1.
///
/// A BTreeMap rather than a HashMap, so iteration order is deterministic: a
/// failing differential-test case must replay exactly from its seed.
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

    /// Add `weight` to `row`'s current weight. A row whose weight reaches zero is removed.
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
        assert_eq!(
            z.len(),
            0,
            "a row whose weight reaches zero must be removed, leaving no w=0 zombie row"
        );
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
        assert_eq!(
            a.len(),
            2,
            "row(1) should be removed once its weight reaches zero"
        );
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
        assert_eq!(
            seen,
            vec![1, 2, 3],
            "a BTreeMap guarantees the order; a HashMap does not"
        );
    }
}
