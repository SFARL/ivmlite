/// v0's value domain. It deliberately has no Real and no Blob: Real would keep
/// an incremental SUM from matching a full recomputation bit for bit (floating-
/// point addition is not associative), and Blob is not needed under v0's STRICT
/// table restriction. Both are scheduled for M4.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Value {
    Null,
    Int(i64),
    Text(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int_and_text_with_same_digits_are_distinct() {
        assert_ne!(Value::Int(1), Value::Text("1".to_string()));
    }

    #[test]
    #[allow(clippy::useless_vec)]
    fn null_orders_before_everything() {
        let mut vs = vec![Value::Text("a".into()), Value::Int(3), Value::Null];
        vs.sort();
        assert_eq!(vs[0], Value::Null);
    }
}
