/// v0 的值域。刻意不含 Real 与 Blob：
/// Real 会让增量 SUM 与全量重算无法 bit-for-bit 相等（浮点加法不满足结合律），
/// Blob 在 v0 的 STRICT table 限制下用不到。两者均排在 M4。
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
