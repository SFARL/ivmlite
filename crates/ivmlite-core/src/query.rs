#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggFn {
    Count,
    Sum,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agg {
    pub func: AggFn,
    /// `None` for COUNT(*); for SUM it must be `Some`, pointing at an INTEGER column.
    pub column: Option<usize>,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Predicate {
    None,
    IntGt { column: usize, value: i64 },
    IsNotNull { column: usize },
}

/// A two-table inner equi-join (M1a Phase 3): `FROM <anchor> JOIN <right> ON
/// <anchor>.<left_column> = <right>.<right_column>`.
///
/// The left input is always the anchor table (`db.tables()[0]`). Every other
/// column index in the query — `group_by`, each `Agg::column`, the
/// `predicate` — refers to the **joined row**: the anchor's columns, then the
/// right table's.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Join {
    pub right: String,
    /// A column of the anchor table.
    pub left_column: usize,
    /// A column of the right table.
    pub right_column: usize,
}

/// The differential harness's query format.
///
/// This is **not** the product's IR: M1b's SQL front end lowers straight from
/// `sqlparser` to `Plan` without going through it. It only has to cover the
/// query shapes v0 supports, so a join is one optional field rather than a
/// tree; replacing it with a tree once the shapes multiply (multi-table joins,
/// filters below a join) changes only test-side code.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewQuery {
    /// v0 allows only bare columns as group-by keys, not expressions (spec §7.1).
    pub group_by: Vec<usize>,
    pub aggs: Vec<Agg>,
    pub predicate: Predicate,
    /// `None` for a single-table view. `serde(default)` keeps the frozen
    /// regression fixtures, written before joins existed, loadable.
    #[cfg_attr(feature = "serde", serde(default))]
    pub join: Option<Join>,
}

impl ViewQuery {
    pub fn output_arity(&self) -> usize {
        self.group_by.len() + self.aggs.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_arity_is_group_by_plus_aggs() {
        let q = ViewQuery {
            group_by: vec![0],
            aggs: vec![
                Agg {
                    func: AggFn::Sum,
                    column: Some(1),
                },
                Agg {
                    func: AggFn::Count,
                    column: None,
                },
            ],
            predicate: Predicate::None,
            join: None,
        };
        assert_eq!(q.output_arity(), 3);
    }
}
