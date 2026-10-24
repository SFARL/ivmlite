//! Resolved scalar projections. Evaluation belongs to the host SQL engine.
use crate::{Row, StateError, Value};

pub const MAX_EXPRESSION_DEPTH: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Plus,
    Minus,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScalarExpr {
    Column(usize),
    Literal(Value),
    Unary {
        op: UnaryOp,
        expr: Box<ScalarExpr>,
    },
    Binary {
        op: BinaryOp,
        left: Box<ScalarExpr>,
        right: Box<ScalarExpr>,
    },
}
impl ScalarExpr {
    pub fn validate(&self, arity: usize) -> Result<(), StateError> {
        fn walk(e: &ScalarExpr, arity: usize, depth: usize) -> Result<(), StateError> {
            if depth > MAX_EXPRESSION_DEPTH {
                return Err(StateError(
                    "scalar expression exceeds depth limit 64".into(),
                ));
            }
            match e {
                ScalarExpr::Column(c) if *c >= arity => Err(StateError(format!(
                    "scalar column index {c} exceeds input arity {arity}"
                ))),
                ScalarExpr::Column(_) | ScalarExpr::Literal(_) => Ok(()),
                ScalarExpr::Unary { expr, .. } => walk(expr, arity, depth + 1),
                ScalarExpr::Binary { left, right, .. } => {
                    walk(left, arity, depth + 1)?;
                    walk(right, arity, depth + 1)
                }
            }
        }
        walk(self, arity, 0)
    }
    pub fn canonical(&self) -> String {
        match self {
            Self::Column(c) => format!("column({c})"),
            Self::Literal(v) => match v {
                Value::Null => "literal(null)".into(),
                Value::Int(n) => format!("literal(int:{n})"),
                Value::Real(x) => format!("literal(real:{:016x})", x.to_bits()),
                Value::Text(t) => format!("literal(text:{}:{t})", t.len()),
                Value::Blob(b) => format!(
                    "literal(blob:{}:{})",
                    b.len(),
                    b.iter().map(|x| format!("{x:02x}")).collect::<String>()
                ),
            },
            Self::Unary { op, expr } => {
                let e = expr.canonical();
                format!("unary({op:?},{}:{e})", e.len())
            }
            Self::Binary { op, left, right } => {
                let l = left.canonical();
                let r = right.canonical();
                format!("binary({op:?},{}:{l},{}:{r})", l.len(), r.len())
            }
        }
    }
}
/// Evaluate one entire projection together, preserving the host's scalar semantics.
pub trait ProjectionEvaluator {
    fn evaluate(&mut self, expressions: &[ScalarExpr], input: &Row) -> Result<Row, StateError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{fresh_mem_arrangement, Agg, AggFn, Node, Plan, ZSet};
    struct Columns;
    impl ProjectionEvaluator for Columns {
        fn evaluate(&mut self, exprs: &[ScalarExpr], row: &Row) -> Result<Row, StateError> {
            Ok(Row::new(
                exprs
                    .iter()
                    .map(|e| match e {
                        ScalarExpr::Column(c) => row.get(*c).clone(),
                        ScalarExpr::Literal(v) => v.clone(),
                        _ => panic!("test uses columns and literals"),
                    })
                    .collect(),
            ))
        }
    }
    #[test]
    fn typed_literals_operand_order_and_boundaries_have_distinct_encodings() {
        let expressions = [
            ScalarExpr::Literal(Value::Int(1)),
            ScalarExpr::Literal(Value::Real(1.0)),
            ScalarExpr::Literal(Value::Text("a,b)".into())),
            ScalarExpr::Literal(Value::Text("a".into())),
            ScalarExpr::Literal(Value::Blob(b"a".to_vec())),
        ];
        let mut texts = expressions
            .iter()
            .map(ScalarExpr::canonical)
            .collect::<Vec<_>>();
        texts.sort();
        texts.dedup();
        assert_eq!(texts.len(), expressions.len());
        let binary = |left, right| ScalarExpr::Binary {
            op: BinaryOp::Subtract,
            left: Box::new(left),
            right: Box::new(right),
        };
        assert_ne!(
            binary(expressions[0].clone(), expressions[1].clone()).canonical(),
            binary(expressions[1].clone(), expressions[0].clone()).canonical()
        );
    }
    #[test]
    fn weighted_projection_collisions_consolidate() {
        let plan = Plan::Map {
            input: Box::new(Plan::Scan {
                table: "t".into(),
                columns: vec![0, 1],
            }),
            expressions: vec![ScalarExpr::Column(0)],
        };
        let mut node = Node::build(&plan, &mut fresh_mem_arrangement).unwrap();
        let input = ZSet::from_rows([
            (Row::new(vec![Value::Int(1), Value::Int(10)]), 4),
            (Row::new(vec![Value::Int(1), Value::Int(20)]), -2),
        ]);
        assert_eq!(
            node.delta_with_evaluator("t", &input, &mut Columns)
                .unwrap(),
            ZSet::from_rows([(Row::new(vec![Value::Int(1)]), 2)])
        );
    }
    #[test]
    fn legacy_refusal_precedes_stateful_child_mutation() {
        let plan = Plan::Map {
            input: Box::new(Plan::Aggregate {
                input: Box::new(Plan::Scan {
                    table: "t".into(),
                    columns: vec![0],
                }),
                group_by: vec![0],
                aggs: vec![Agg {
                    func: AggFn::Count,
                    column: None,
                }],
            }),
            expressions: vec![ScalarExpr::Column(0), ScalarExpr::Column(1)],
        };
        let mut node = Node::build(&plan, &mut fresh_mem_arrangement).unwrap();
        let input = ZSet::from_rows([(Row::new(vec![Value::Int(1)]), 1)]);
        assert!(node
            .delta("t", &input)
            .unwrap_err()
            .0
            .contains("explicit projection evaluator"));
        assert_eq!(
            node.delta_with_evaluator("t", &input, &mut Columns)
                .unwrap(),
            ZSet::from_rows([(Row::new(vec![Value::Int(1), Value::Int(1)]), 1)])
        );
    }
    #[test]
    fn invalid_projection_is_rejected_before_provider_is_called() {
        let plan = Plan::Map {
            input: Box::new(Plan::Scan {
                table: "t".into(),
                columns: vec![0],
            }),
            expressions: vec![ScalarExpr::Column(1)],
        };
        assert!(Node::build(&plan, &mut |_| panic!(
            "invalid plan must not reach provider"
        ))
        .unwrap_err()
        .0
        .contains("arity"));
        let mut expr = ScalarExpr::Column(0);
        for _ in 0..66 {
            expr = ScalarExpr::Unary {
                op: UnaryOp::Plus,
                expr: Box::new(expr),
            };
        }
        assert!(expr.validate(1).unwrap_err().0.contains("depth"));
    }
}

#[cfg(test)]
mod preflight_join_test {
    use super::*;
    use crate::{fresh_mem_arrangement, Agg, AggFn, Node, Plan, ZSet};
    struct Copy;
    impl ProjectionEvaluator for Copy {
        fn evaluate(&mut self, _: &[ScalarExpr], input: &Row) -> Result<Row, StateError> {
            Ok(input.clone())
        }
    }
    #[test]
    fn legacy_refusal_checks_later_join_child_before_mutating_earlier_aggregate() {
        let scan = |table: &str| Plan::Scan {
            table: table.into(),
            columns: vec![0],
        };
        let plan = Plan::Join {
            left: Box::new(Plan::Aggregate {
                input: Box::new(scan("l")),
                group_by: vec![0],
                aggs: vec![Agg {
                    func: AggFn::Count,
                    column: None,
                }],
            }),
            right: Box::new(Plan::Map {
                input: Box::new(scan("r")),
                expressions: vec![ScalarExpr::Column(0)],
            }),
            left_key: 0,
            right_key: 0,
        };
        let mut node = Node::build(&plan, &mut fresh_mem_arrangement).unwrap();
        let input = ZSet::from_rows([(Row::new(vec![Value::Int(1)]), 1)]);
        assert!(node.delta("l", &input).is_err());
        node.delta_with_evaluator("r", &input, &mut Copy).unwrap();
        assert_eq!(
            node.delta_with_evaluator("l", &input, &mut Copy).unwrap(),
            ZSet::from_rows([(
                Row::new(vec![Value::Int(1), Value::Int(1), Value::Int(1)]),
                1
            )])
        );
    }
}
