use std::collections::BTreeSet;

use ivmlite_core::{Value, ZSet};

use crate::ViewQuery;

/// Properties that can be checked without an oracle (the first layer of
/// spec §9.1). They are very cheap, so they are checked after every batch of
/// deltas rather than only at the end.
pub fn check_invariants(state: &ZSet, query: &ViewQuery) -> Result<(), String> {
    let key_arity = query.group_by.len();
    let mut seen: BTreeSet<Vec<Value>> = BTreeSet::new();

    for (row, weight) in state.iter() {
        if *weight < 0 {
            return Err(format!(
                "negative weight {weight} in the final state, row {row:?}"
            ));
        }
        if *weight != 1 {
            return Err(format!(
                "each group of an aggregate view should be exactly one row with weight 1; got weight {weight}, row {row:?}"
            ));
        }
        if row.len() != query.output_arity() {
            return Err(format!(
                "output row width {} does not match the view's {}, row {row:?}",
                row.len(),
                query.output_arity()
            ));
        }
        let key: Vec<Value> = (0..key_arity).map(|i| row.get(i).clone()).collect();
        if !seen.insert(key.clone()) {
            // Item 20 (a deferred minor): include row:?, like the other three
            // error paths. A duplicate group key is M1's most likely failure
            // mode, and leaving the row out would force someone to dig through a
            // ZSet dump at exactly the moment it matters most.
            return Err(format!(
                "group key {key:?} appears more than once in the output, row {row:?}"
            ));
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
        assert!(err.contains("negative weight"), "got: {err}");
    }

    #[test]
    fn rejects_duplicate_group_keys() {
        // The same group key "a" appears with two different aggregate results.
        let z = ZSet::from_rows([(out("a", 1), 1), (out("a", 2), 1)]);
        let err = check_invariants(&z, &q()).unwrap_err();
        assert!(err.contains("group key"), "got: {err}");
        // Item 20: the message must name the specific row that collided, not
        // just the key. The rows iterate in order, so ("a", 1) claims the key
        // and ("a", 2) is the one that collides. Asserting on that row's own
        // Debug text pins the intent; the substring check this replaced was a
        // single character meaning "row", which almost any message satisfied.
        assert!(err.contains(&format!("{:?}", out("a", 2))), "got: {err}");
    }

    #[test]
    fn rejects_weight_greater_than_one_for_aggregate_views() {
        let z = ZSet::from_rows([(out("a", 1), 2)]);
        let err = check_invariants(&z, &q()).unwrap_err();
        // "weight" alone would also match the negative-weight path, so match the phrase unique to this one.
        assert!(err.contains("got weight 2"), "got: {err}");
    }

    #[test]
    fn rejects_wrong_row_width() {
        // Missing the count column; row has only 1 column instead of expected 2
        let z = ZSet::from_rows([(Row::new(vec![Value::Text("a".into())]), 1)]);
        let err = check_invariants(&z, &q()).unwrap_err();
        assert!(err.contains("width"), "got: {err}");
    }
}
