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

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewQuery {
    /// v0 allows only bare columns as group-by keys, not expressions (spec §7.1).
    pub group_by: Vec<usize>,
    pub aggs: Vec<Agg>,
    pub predicate: Predicate,
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
        };
        assert_eq!(q.output_arity(), 3);
    }
}
