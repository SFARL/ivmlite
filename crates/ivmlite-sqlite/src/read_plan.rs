//! Private, ephemeral xBestIndex/xFilter protocol for candidate snapshots.

use rusqlite::types::Value as SqlValue;
use rusqlite::vtab::IndexConstraintOp;

const INVALID_PLAN: &str = "ivmlite internal error: invalid selective-read plan";
const FULL_SCAN_COST: f64 = 1_000_000.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Op {
    Eq,
    Gt,
    Ge,
    Lt,
    Le,
}

impl Op {
    pub fn from_constraint(op: IndexConstraintOp) -> Option<Self> {
        match op {
            IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_EQ => Some(Self::Eq),
            IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_GT => Some(Self::Gt),
            IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_GE => Some(Self::Ge),
            IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_LT => Some(Self::Lt),
            IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_LE => Some(Self::Le),
            _ => None,
        }
    }

    fn token(self) -> &'static str {
        match self {
            Self::Eq => "eq",
            Self::Gt => "gt",
            Self::Ge => "ge",
            Self::Lt => "lt",
            Self::Le => "le",
        }
    }

    fn decode(token: &str) -> Option<Self> {
        match token {
            "eq" => Some(Self::Eq),
            "gt" => Some(Self::Gt),
            "ge" => Some(Self::Ge),
            "lt" => Some(Self::Lt),
            "le" => Some(Self::Le),
            _ => None,
        }
    }

    fn sql(self) -> &'static str {
        match self {
            Self::Eq => "=",
            Self::Gt => ">",
            Self::Ge => ">=",
            Self::Lt => "<",
            Self::Le => "<=",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Term {
    pub column: usize,
    pub op: Op,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ReadPlan {
    pub terms: Vec<Term>,
}

impl ReadPlan {
    pub fn encode(&self) -> String {
        format!(
            "sr1:{}",
            self.terms
                .iter()
                .map(|term| format!("{}={}", term.column, term.op.token()))
                .collect::<Vec<_>>()
                .join(",")
        )
    }

    pub fn decode(idx: i32, text: Option<&str>, keys: usize, argc: usize) -> Result<Self, String> {
        if idx == 0 && text.is_none() && argc == 0 {
            return Ok(Self::default());
        }
        let invalid = || INVALID_PLAN.to_string();
        if idx != 1 {
            return Err(invalid());
        }
        let body = text
            .and_then(|text| text.strip_prefix("sr1:"))
            .filter(|body| !body.is_empty())
            .ok_or_else(invalid)?;
        let mut terms = Vec::new();
        for term in body.split(',') {
            let (column, op) = term.split_once('=').ok_or_else(invalid)?;
            if column.is_empty()
                || !column.bytes().all(|c| c.is_ascii_digit())
                || (column.len() > 1 && column.starts_with('0'))
            {
                return Err(invalid());
            }
            let column: usize = column.parse().map_err(|_| invalid())?;
            if column >= keys {
                return Err(invalid());
            }
            terms.push(Term {
                column,
                op: Op::decode(op).ok_or_else(invalid)?,
            });
        }
        if terms.len() != argc {
            return Err(invalid());
        }
        Ok(Self { terms })
    }

    /// Every term is a superset of the original comparison. SQLite keeps
    /// the original predicate as a residual (`omit=false`).
    /// Split one leading numeric EQ/LT/LE candidate into disjoint branches.
    /// UNION ALL lets SQLite choose a bounded index access for each branch;
    /// a combined OR may instead scan a wide output table. Splitting only
    /// one leading term bounds query growth and avoids doubling full scans
    /// for a non-leading constraint.
    pub fn predicates(&self, quoted_columns: &[String], args: &[SqlValue]) -> Vec<String> {
        let leading_numeric = |i: usize, term: &Term| {
            term.column == 0 && matches!(args[i], SqlValue::Integer(_) | SqlValue::Real(_))
        };
        let anchor = self
            .terms
            .iter()
            .enumerate()
            .position(|(i, term)| leading_numeric(i, term) && term.op == Op::Eq)
            .or_else(|| {
                self.terms.iter().enumerate().position(|(i, term)| {
                    leading_numeric(i, term) && matches!(term.op, Op::Lt | Op::Le)
                })
            });
        (0..if anchor.is_some() { 2 } else { 1 })
            .map(|branch| {
                self.terms
                    .iter()
                    .zip(args)
                    .enumerate()
                    .map(|(i, (term, arg))| {
                        let column = &quoted_columns[term.column];
                        let parameter = format!("?{}", i + 1);
                        if anchor == Some(i) {
                            numeric_branches(column, &parameter, term.op)[branch].clone()
                        } else {
                            term_predicate(column, &parameter, term.op, arg)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" AND ")
            })
            .collect()
    }

    pub fn estimate(&self) -> f64 {
        // The argument's storage class is unknown in xBestIndex. Some EQ
        // candidates can seek, but TEXT arguments and non-leading constraints
        // can scan. Do not promise a seek before xFilter sees the value.
        FULL_SCAN_COST
    }
}

fn term_predicate(column: &str, parameter: &str, op: Op, arg: &SqlValue) -> String {
    match arg {
        SqlValue::Null => format!("({parameter} IS NOT NULL AND 0)"),
        SqlValue::Blob(_) => format!("({column} COLLATE BINARY {} {parameter})", op.sql()),
        SqlValue::Integer(_) | SqlValue::Real(_) => numeric_comparisons(column, parameter, op),
        SqlValue::Text(_) => comparisons(column, parameter, op),
    }
}

/// For a numeric RHS, raw EQ/LT/LE cannot match TEXT, making these branches
/// disjoint. Retaining every row exactly once permits UNION ALL snapshots.
fn numeric_branches(column: &str, parameter: &str, op: Op) -> [String; 2] {
    let op = op.sql();
    [
        format!("({column} COLLATE BINARY {op} {parameter})"),
        format!(
            "({column} COLLATE BINARY >= '' AND \
              {column} COLLATE BINARY < x'' AND \
              +CAST({column} AS NUMERIC) COLLATE BINARY {op} {parameter})"
        ),
    ]
}

/// The declared key has BLOB affinity. SQLite comparisons with such a key
/// apply either NUMERIC affinity or no affinity; a TEXT RHS cannot apply TEXT
/// affinity. A numeric RHS is unchanged, leaving only possible TEXT-to-number
/// conversion of the key. For GT/GE, every TEXT key already passes raw, so
/// numeric conversion can only remove matches from that superset.
fn numeric_comparisons(column: &str, parameter: &str, op: Op) -> String {
    if matches!(op, Op::Gt | Op::Ge) {
        return format!("({column} COLLATE BINARY {} {parameter})", op.sql());
    }
    format!("({})", numeric_branches(column, parameter, op).join(" OR "))
}

/// A compound SELECT or foreign virtual table may expose an affinity which
/// does not agree with the RHS value's storage class. NUMERIC affinity can
/// convert either operand, both operands, or leave both unchanged. The key's
/// BLOB affinity rules out TEXT-affinity propagation for selected constraints.
/// Unary plus strips CAST's affinity, preventing an extra implicit coercion.
fn comparisons(column: &str, parameter: &str, op: Op) -> String {
    let op = op.sql();
    let compare = |left: &str, right: &str| format!("{left} COLLATE BINARY {op} {right}");
    let mut branches = vec![compare(column, parameter)];
    let left = format!("+CAST({column} AS NUMERIC)");
    let right = format!("+CAST({parameter} AS NUMERIC)");
    branches.push(compare(&left, parameter));
    branches.push(compare(column, &right));
    branches.push(compare(&left, &right));
    format!("({})", branches.join(" OR "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_roundtrip_and_empty_plan() {
        let plan = ReadPlan {
            terms: vec![
                Term {
                    column: 0,
                    op: Op::Eq,
                },
                Term {
                    column: 1,
                    op: Op::Ge,
                },
                Term {
                    column: 1,
                    op: Op::Lt,
                },
                Term {
                    column: 0,
                    op: Op::Gt,
                },
                Term {
                    column: 0,
                    op: Op::Le,
                },
            ],
        };
        assert_eq!(plan.encode(), "sr1:0=eq,1=ge,1=lt,0=gt,0=le");
        assert_eq!(
            ReadPlan::decode(1, Some(&plan.encode()), 2, 5).unwrap(),
            plan
        );
        assert_eq!(
            ReadPlan::decode(0, None, 0, 0).unwrap(),
            ReadPlan::default()
        );
    }

    #[test]
    fn descriptor_rejects_noncanonical_or_inconsistent_metadata() {
        for text in [
            "sr2:0=eq",
            "sr1:",
            "sr1:0=ne",
            "sr1:01=eq",
            "sr1:-1=eq",
            "sr1:+1=eq",
            "sr1:2=eq",
            "sr1:0=eq,",
            "sr1:,0=eq",
            "sr1:0=eq=eq",
            "sr1: 0=eq",
            "sr1:0=EQ",
            "sr1:184467440737095516160=eq",
        ] {
            assert_eq!(
                ReadPlan::decode(1, Some(text), 2, 1).unwrap_err(),
                INVALID_PLAN,
                "{text}"
            );
        }
        for (idx, text, keys, argc) in [
            (1, Some("sr1:0=eq"), 2, 0),
            (0, Some("sr1:0=eq"), 2, 0),
            (9, None, 2, 0),
            (1, None, 2, 0),
            (0, None, 2, 1),
            (1, Some("sr1:0=eq"), 0, 1),
        ] {
            assert_eq!(
                ReadPlan::decode(idx, text, keys, argc).unwrap_err(),
                INVALID_PLAN
            );
        }
    }

    #[test]
    fn only_five_comparison_operators_are_eligible() {
        for (input, expected) in [
            (IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_EQ, Some(Op::Eq)),
            (IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_GT, Some(Op::Gt)),
            (IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_GE, Some(Op::Ge)),
            (IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_LT, Some(Op::Lt)),
            (IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_LE, Some(Op::Le)),
            (IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_IS, None),
            (IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_ISNULL, None),
            (IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_NE, None),
            (IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_LIMIT, None),
        ] {
            assert_eq!(Op::from_constraint(input), expected);
        }
    }

    #[test]
    fn null_equality_retains_its_numbered_parameter() {
        let plan = ReadPlan {
            terms: vec![Term {
                column: 0,
                op: Op::Eq,
            }],
        };
        assert_eq!(
            plan.predicates(&["\"g\"".into()], &[SqlValue::Null]),
            vec!["(?1 IS NOT NULL AND 0)"]
        );
    }

    #[test]
    fn query_branches_split_only_one_leading_anchor_and_prefer_equality() {
        let columns = ["\"g\"".into(), "\"h\"".into()];
        let plan = ReadPlan {
            terms: vec![
                Term {
                    column: 0,
                    op: Op::Le,
                },
                Term {
                    column: 1,
                    op: Op::Eq,
                },
                Term {
                    column: 0,
                    op: Op::Eq,
                },
            ],
        };
        let branches = plan.predicates(
            &columns,
            &[
                SqlValue::Integer(50),
                SqlValue::Integer(1),
                SqlValue::Real(42.0),
            ],
        );
        assert_eq!(branches.len(), 2);
        for branch in &branches {
            assert!(branch.contains("?1") && branch.contains("?2") && branch.contains("?3"));
        }
        assert!(branches[0].ends_with("(\"g\" COLLATE BINARY = ?3)"));
        assert!(branches[1].ends_with("(\"g\" COLLATE BINARY >= '' AND \"g\" COLLATE BINARY < x'' AND +CAST(\"g\" AS NUMERIC) COLLATE BINARY = ?3)"));
        for op in [Op::Lt, Op::Le] {
            let plan = ReadPlan {
                terms: vec![Term { column: 0, op }],
            };
            let branches = plan.predicates(&columns, &[SqlValue::Integer(42)]);
            assert_eq!(branches, numeric_branches("\"g\"", "?1", op));
        }
        for (column, op, arg) in [
            (1, Op::Eq, SqlValue::Integer(42)),
            (0, Op::Gt, SqlValue::Integer(42)),
            (0, Op::Ge, SqlValue::Integer(42)),
            (0, Op::Eq, SqlValue::Text("42".into())),
        ] {
            let plan = ReadPlan {
                terms: vec![Term { column, op }],
            };
            assert_eq!(plan.predicates(&columns, &[arg]).len(), 1);
        }
        assert_eq!(ReadPlan::default().predicates(&columns, &[]), vec![""]);
    }

    #[test]
    fn estimates_do_not_assume_argument_class_or_compound_discounts() {
        for terms in [
            vec![],
            vec![Term {
                column: 0,
                op: Op::Eq,
            }],
            vec![Term {
                column: 0,
                op: Op::Gt,
            }],
            vec![Term {
                column: 1,
                op: Op::Eq,
            }],
            vec![
                Term {
                    column: 0,
                    op: Op::Eq,
                },
                Term {
                    column: 1,
                    op: Op::Eq,
                },
            ],
        ] {
            let cost = ReadPlan { terms }.estimate();
            assert_eq!(cost.to_bits(), FULL_SCAN_COST.to_bits());
        }
    }
}
