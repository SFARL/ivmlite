use std::cmp::Ordering;
use std::hash::{Hash, Hasher};

/// The value domain (M2c spec §2): SQLite's five storage classes.
///
/// Equality, hashing and `Ord` are **row identity**, which Z-set
/// consolidation uses: exact by storage class, and a `Real` equals another
/// only bit for bit, so `Int(1)`, `Real(1.0)`, `Text("1")`, `Real(0.0)` and
/// `Real(-0.0)` are five different rows. `Ord` is a total order consistent
/// with that equality and is **not** SQL's order: use `sql_cmp` for
/// comparisons and MIN/MAX, and `canonical` for grouping and join keys.
///
/// A `Real` is never NaN: SQLite stores NULL instead, and every decoder
/// refuses a NaN as corrupted state.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone)]
pub enum Value {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

/// 2^63 as an `f64`: the first value above the `i64` range.
const TWO_POW_63: f64 = 9_223_372_036_854_775_808.0;

impl Value {
    /// The storage class's position in the identity order.
    fn rank(&self) -> u8 {
        match self {
            Value::Null => 0,
            Value::Int(_) => 1,
            Value::Real(_) => 2,
            Value::Text(_) => 3,
            Value::Blob(_) => 4,
        }
    }

    /// M2c spec §2: the group-key form. An integral `Real` within the `i64`
    /// range becomes `Int`, and `-0.0` becomes `Int(0)`; every other value is
    /// unchanged. Canonical values then compare by exact equality, which is
    /// SQLite's GROUP BY equality.
    pub fn canonical(&self) -> Value {
        match self {
            Value::Real(x) if x.fract() == 0.0 && (-TWO_POW_63..TWO_POW_63).contains(x) => {
                Value::Int(*x as i64)
            }
            other => other.clone(),
        }
    }

    /// M2c spec §2: SQLite's cross-class order, NULL < numeric < TEXT < BLOB.
    /// Numerics compare by exact value, `Int` against `Real` included; TEXT
    /// and BLOB compare with `memcmp` (BINARY). Two NULLs compare equal here;
    /// a predicate treats NULL as UNKNOWN before it ever calls this.
    pub fn sql_cmp(&self, other: &Value) -> Ordering {
        let class = |v: &Value| match v {
            Value::Null => 0,
            Value::Int(_) | Value::Real(_) => 1,
            Value::Text(_) => 2,
            Value::Blob(_) => 3,
        };
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => a.cmp(b),
            (Value::Real(a), Value::Real(b)) => a.partial_cmp(b).expect("a Real is never NaN"),
            (Value::Int(a), Value::Real(b)) => cmp_int_real(*a, *b),
            (Value::Real(a), Value::Int(b)) => cmp_int_real(*b, *a).reverse(),
            (Value::Text(a), Value::Text(b)) => a.as_bytes().cmp(b.as_bytes()),
            (Value::Blob(a), Value::Blob(b)) => a.cmp(b),
            _ => class(self).cmp(&class(other)),
        }
    }
}

/// `i` against `r`, exactly: no rounding of `i` to `f64`.
fn cmp_int_real(i: i64, r: f64) -> Ordering {
    if r >= TWO_POW_63 {
        return Ordering::Less;
    }
    if r < -TWO_POW_63 {
        return Ordering::Greater;
    }
    let whole = r.trunc();
    match i.cmp(&(whole as i64)) {
        // Same integer part: the fraction decides. `r - whole` is exact.
        Ordering::Equal => 0.0f64
            .partial_cmp(&(r - whole))
            .expect("a Real is never NaN"),
        other => other,
    }
}

impl Ord for Value {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => a.cmp(b),
            // `total_cmp` is equality exactly when the bits are equal.
            (Value::Real(a), Value::Real(b)) => a.total_cmp(b),
            (Value::Text(a), Value::Text(b)) => a.cmp(b),
            (Value::Blob(a), Value::Blob(b)) => a.cmp(b),
            _ => self.rank().cmp(&other.rank()),
        }
    }
}

impl PartialOrd for Value {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Value {}

impl Hash for Value {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.rank().hash(state);
        match self {
            Value::Null => {}
            Value::Int(n) => n.hash(state),
            Value::Real(x) => x.to_bits().hash(state),
            Value::Text(s) => s.hash(state),
            Value::Blob(b) => b.hash(state),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;
    use std::collections::BTreeSet;

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

    #[test]
    fn int_real_and_text_with_same_digits_are_three_rows() {
        // Spec §2: row identity is exact by storage class.
        let set: BTreeSet<Value> = [Value::Int(1), Value::Real(1.0), Value::Text("1".into())]
            .into_iter()
            .collect();
        assert_eq!(set.len(), 3);
    }

    #[test]
    fn real_identity_is_bitwise() {
        // Spec §2: two Reals are equal only bit for bit, so 0.0 and -0.0 are two rows.
        assert_ne!(Value::Real(0.0), Value::Real(-0.0));
        assert_eq!(Value::Real(0.1 + 0.2), Value::Real(0.1 + 0.2));
    }

    #[test]
    fn canonical_maps_integral_reals_and_negative_zero_to_int() {
        assert_eq!(Value::Real(1.0).canonical(), Value::Int(1));
        assert_eq!(Value::Real(-0.0).canonical(), Value::Int(0));
        assert_eq!(
            Value::Real(-9_223_372_036_854_775_808.0).canonical(),
            Value::Int(i64::MIN)
        );
        // Outside the i64 range, non-integral, infinite: unchanged.
        assert_eq!(
            Value::Real(9_223_372_036_854_775_808.0).canonical(),
            Value::Real(9_223_372_036_854_775_808.0)
        );
        assert_eq!(Value::Real(1.5).canonical(), Value::Real(1.5));
        assert_eq!(
            Value::Real(f64::INFINITY).canonical(),
            Value::Real(f64::INFINITY)
        );
        // TEXT never becomes a number: '1' differs from 1 (spec §2).
        assert_eq!(Value::Text("1".into()).canonical(), Value::Text("1".into()));
    }

    #[test]
    fn sql_order_is_null_then_numeric_then_text_then_blob() {
        let ordered = [
            Value::Null,
            Value::Real(f64::NEG_INFINITY),
            Value::Int(-5),
            Value::Real(2.5),
            Value::Int(3),
            Value::Text("".into()),
            Value::Text("a".into()),
            Value::Blob(vec![]),
            Value::Blob(vec![0]),
        ];
        for (i, a) in ordered.iter().enumerate() {
            for (j, b) in ordered.iter().enumerate() {
                assert_eq!(a.sql_cmp(b), i.cmp(&j), "{a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn int_and_real_compare_exactly_near_2_pow_53_and_2_pow_63() {
        let two53 = 9_007_199_254_740_992_i64;
        assert_eq!(
            Value::Int(two53 + 1).sql_cmp(&Value::Real(two53 as f64)),
            Ordering::Greater
        );
        assert_eq!(
            Value::Int(two53).sql_cmp(&Value::Real(two53 as f64)),
            Ordering::Equal
        );
        assert_eq!(
            Value::Int(i64::MAX).sql_cmp(&Value::Real(9_223_372_036_854_775_808.0)),
            Ordering::Less
        );
        assert_eq!(
            Value::Int(i64::MIN).sql_cmp(&Value::Real(-9_223_372_036_854_775_808.0)),
            Ordering::Equal
        );
        assert_eq!(Value::Int(1).sql_cmp(&Value::Real(1.5)), Ordering::Less);
        assert_eq!(
            Value::Int(-1).sql_cmp(&Value::Real(-1.5)),
            Ordering::Greater
        );
        assert_eq!(
            Value::Real(0.0).sql_cmp(&Value::Real(-0.0)),
            Ordering::Equal
        );
    }

    #[test]
    fn text_and_blob_compare_by_bytes() {
        // BINARY collation: 'Ā' (0xC4 0x80) sorts after 'b'.
        assert_eq!(
            Value::Text("Ā".into()).sql_cmp(&Value::Text("b".into())),
            Ordering::Greater
        );
        assert_eq!(
            Value::Blob(vec![1, 2]).sql_cmp(&Value::Blob(vec![1, 2, 0])),
            Ordering::Less
        );
    }
}
