use crate::Value;

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

/// A comparison operator from spec §6.1's whitelist. The literal is always the
/// right operand: `Compare { op: Gt, .. }` is `column > literal`.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Gt,
    Ge,
    Lt,
    Le,
    Eq,
    Ne,
}

impl CmpOp {
    pub const ALL: [CmpOp; 6] = [
        CmpOp::Gt,
        CmpOp::Ge,
        CmpOp::Lt,
        CmpOp::Le,
        CmpOp::Eq,
        CmpOp::Ne,
    ];

    /// Whether `column <op> literal` holds, given how the column's value
    /// orders against the literal.
    pub(crate) fn holds(self, ord: std::cmp::Ordering) -> bool {
        use std::cmp::Ordering::{Equal, Greater, Less};
        match self {
            CmpOp::Gt => ord == Greater,
            CmpOp::Ge => ord != Less,
            CmpOp::Lt => ord == Less,
            CmpOp::Le => ord != Greater,
            CmpOp::Eq => ord == Equal,
            CmpOp::Ne => ord != Equal,
        }
    }
}

/// A view's filter: spec §6.1's whitelist, at most one per view (M1b Phase 2a,
/// Ruling 1 — no `AND`).
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Predicate {
    None,
    /// `column <op> value`. `lower` requires `value` to have the column's
    /// declared type — `Int` for INTEGER, `Text` for TEXT — and rejects NULL.
    Compare {
        column: usize,
        op: CmpOp,
        value: Value,
    },
    IsNull {
        column: usize,
    },
    IsNotNull {
        column: usize,
    },
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
    /// `None` for a single-table view. The frozen regression fixtures, written
    /// before joins existed, carry no `join` key; a missing key deserializes to
    /// `None` anyway, through serde's missing-field handling for `Option`, and
    /// `serde(default)` states that intent explicitly.
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
