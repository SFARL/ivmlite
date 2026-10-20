//! M2c spec §4 "Encoding of `val`" (amendment 1): MIN/MAX state values,
//! encoded so that `memcmp` order is spec §2's cross-class order.
//!
//! - A numeric is `0x01`, then 8 bytes of the order-preserving `f64` (an
//!   `Int` rounded to nearest; `-0.0` as `+0.0`), then 8 bytes of the
//!   order-preserving `i64` tie (an `Int`'s own value; a `Real` truncated
//!   toward zero and saturated; `-0.0`'s is 0), then one entry byte: `0x00`
//!   Int, `0x01` Real, `0x02` Real `-0.0`.
//! - TEXT is `0x02` and BLOB `0x03`, then the bytes with each `0x00` written
//!   `0x00 0xFF`, then `0x00 0x00`.
//!
//! Why this orders exactly: rounding is monotone, so the `f64` prefix orders
//! any two numerics whose prefixes differ. With equal prefixes the truncated
//! tie orders an `Int` against a `Real` exactly (a prefix that large is
//! integral, so the Real's tie is its value), and equal ties fall to the
//! entry byte, which puts `Int` first (spec §4 "Ties"). The escape keeps
//! `memcmp` order equal to the raw bytes' order, prefixes included.

use crate::{StateError, Value};

const NUMERIC: u8 = 0x01;
const TEXT: u8 = 0x02;
const BLOB: u8 = 0x03;
const INT_ENTRY: u8 = 0x00;
const REAL_ENTRY: u8 = 0x01;
const NEGATIVE_ZERO_ENTRY: u8 = 0x02;

/// The bytes after a numeric's class byte: prefix, tie and entry.
const NUMERIC_BYTES: usize = 17;

/// The order-preserving form of `value`; `None` for NULL, which MIN and MAX
/// skip.
pub fn sort_key(value: &Value) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    match value {
        Value::Null => return None,
        Value::Int(i) => numeric(&mut out, *i as f64, *i, INT_ENTRY),
        Value::Real(x) if *x == 0.0 && x.is_sign_negative() => {
            numeric(&mut out, 0.0, 0, NEGATIVE_ZERO_ENTRY)
        }
        // `as` truncates toward zero and saturates, infinities included.
        Value::Real(x) => numeric(&mut out, *x, *x as i64, REAL_ENTRY),
        Value::Text(s) => escaped(&mut out, TEXT, s.as_bytes()),
        Value::Blob(b) => escaped(&mut out, BLOB, b),
    }
    Some(out)
}

fn numeric(out: &mut Vec<u8>, approx: f64, tie: i64, entry: u8) {
    out.push(NUMERIC);
    let bits = approx.to_bits();
    let ordered = if bits >> 63 == 1 {
        !bits
    } else {
        bits | (1 << 63)
    };
    out.extend_from_slice(&ordered.to_be_bytes());
    out.extend_from_slice(&((tie as u64) ^ (1 << 63)).to_be_bytes());
    out.push(entry);
}

fn escaped(out: &mut Vec<u8>, class: u8, payload: &[u8]) {
    out.push(class);
    for &b in payload {
        out.push(b);
        if b == 0 {
            out.push(0xff);
        }
    }
    out.extend_from_slice(&[0, 0]);
}

/// The inverse of `sort_key`.
///
/// # Errors
/// If `key` is not a key `sort_key` produces: an unknown class or entry, a
/// wrong length, an unterminated TEXT or BLOB, TEXT that is not UTF-8, a NaN,
/// or a numeric that is not in its canonical encoding.
pub fn from_sort_key(key: &[u8]) -> Result<Value, StateError> {
    let corrupt = |what: &str| StateError(format!("corrupted MIN/MAX state: {what}"));
    let (&class, rest) = key.split_first().ok_or_else(|| corrupt("an empty value"))?;
    match class {
        NUMERIC => {
            if rest.len() != NUMERIC_BYTES {
                return Err(corrupt("a numeric value is not 17 bytes"));
            }
            let ordered = u64::from_be_bytes(rest[..8].try_into().expect("8 bytes"));
            let approx = f64::from_bits(if ordered >> 63 == 1 {
                ordered ^ (1 << 63)
            } else {
                !ordered
            });
            let tie =
                (u64::from_be_bytes(rest[8..16].try_into().expect("8 bytes")) ^ (1 << 63)) as i64;
            let value = match rest[16] {
                INT_ENTRY => Value::Int(tie),
                REAL_ENTRY if approx.is_nan() => return Err(corrupt("REAL is NaN")),
                REAL_ENTRY => Value::Real(approx),
                NEGATIVE_ZERO_ENTRY => Value::Real(-0.0),
                other => return Err(corrupt(&format!("unknown numeric entry {other:#04x}"))),
            };
            if sort_key(&value).as_deref() != Some(key) {
                return Err(corrupt("a numeric value is not in its canonical encoding"));
            }
            Ok(value)
        }
        TEXT | BLOB => {
            let mut payload = Vec::new();
            let mut i = 0;
            loop {
                match (rest.get(i), rest.get(i + 1)) {
                    (Some(0), Some(0)) if i + 2 == rest.len() => break,
                    (Some(0), Some(0xff)) => {
                        payload.push(0);
                        i += 2;
                    }
                    (Some(0), _) | (None, _) => {
                        return Err(corrupt("a TEXT or BLOB value is not terminated"))
                    }
                    (Some(&b), _) => {
                        payload.push(b);
                        i += 1;
                    }
                }
            }
            if class == TEXT {
                String::from_utf8(payload)
                    .map(Value::Text)
                    .map_err(|_| corrupt("TEXT is not UTF-8"))
            } else {
                Ok(Value::Blob(payload))
            }
        }
        other => Err(corrupt(&format!("unknown class {other:#04x}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::SplitMix64;
    use std::cmp::Ordering;

    fn domain() -> Vec<Value> {
        let two53 = 9_007_199_254_740_992_i64;
        let mut v = vec![
            Value::Int(0),
            Value::Int(1),
            Value::Int(-1),
            Value::Int(i64::MAX),
            Value::Int(i64::MIN),
            Value::Int(i64::MAX - 1),
            Value::Int(two53),
            Value::Int(two53 + 1),
            Value::Int(-two53 - 1),
            Value::Int(1 << 60),
            Value::Int((1 << 60) + 1),
            Value::Real(0.0),
            Value::Real(-0.0),
            Value::Real(1.0),
            Value::Real(-1.0),
            Value::Real(0.5),
            Value::Real(-0.5),
            Value::Real(two53 as f64),
            Value::Real((1i64 << 60) as f64),
            Value::Real(9_223_372_036_854_775_808.0),
            Value::Real(-9_223_372_036_854_775_808.0),
            Value::Real(1e300),
            Value::Real(-1e300),
            Value::Real(f64::INFINITY),
            Value::Real(f64::NEG_INFINITY),
            Value::Real(5e-324),
            Value::Real(-5e-324),
            Value::Text(String::new()),
            Value::Text("a".into()),
            Value::Text("a\0".into()),
            Value::Text("ab".into()),
            Value::Text("\u{100}".into()),
            Value::Blob(vec![]),
            Value::Blob(vec![0]),
            Value::Blob(vec![0, 0]),
            Value::Blob(vec![0, 1]),
            Value::Blob(vec![1]),
            Value::Blob(vec![0xff]),
        ];
        let mut rng = SplitMix64(11);
        while v.len() < 400 {
            v.push(match rng.below(4) {
                0 => Value::Int((rng.next_u64() as i64) >> rng.below(64)),
                1 => {
                    let x = f64::from_bits(rng.next_u64());
                    if x.is_nan() {
                        continue;
                    }
                    Value::Real(x)
                }
                2 => Value::Text(
                    (0..rng.below(4))
                        .map(|_| ['\0', 'a', 'b'][rng.below(3) as usize])
                        .collect(),
                ),
                _ => Value::Blob(
                    (0..rng.below(4))
                        .map(|_| [0u8, 1, 0xff][rng.below(3) as usize])
                        .collect(),
                ),
            });
        }
        v
    }

    /// Where a numeric entry sorts among the entries numerically equal to it
    /// (M2c spec §4 "Ties", amendment 1): `Int`, then `Real`, then `-0.0`.
    fn tie_rank(v: &Value) -> u8 {
        match v {
            Value::Int(_) => 0,
            Value::Real(x) if *x == 0.0 && x.is_sign_negative() => 2,
            _ => 1,
        }
    }

    #[test]
    fn sort_keys_order_like_sql_cmp() {
        // M2c spec §4: memcmp order equals §2's order; numerically equal
        // entries stay distinct and order by their tie rank.
        let d = domain();
        for a in &d {
            for b in &d {
                let (ka, kb) = (sort_key(a).unwrap(), sort_key(b).unwrap());
                match a.sql_cmp(b) {
                    Ordering::Less => assert!(ka < kb, "{a:?} < {b:?}"),
                    Ordering::Greater => assert!(ka > kb, "{a:?} > {b:?}"),
                    Ordering::Equal if a == b => assert_eq!(ka, kb, "{a:?}"),
                    Ordering::Equal => {
                        assert_eq!(ka.cmp(&kb), tie_rank(a).cmp(&tie_rank(b)), "{a:?} vs {b:?}")
                    }
                }
            }
        }
    }

    #[test]
    fn sort_keys_round_trip() {
        for a in domain() {
            let back = from_sort_key(&sort_key(&a).unwrap()).unwrap();
            // Identity is exact: a `Real` only bit for bit.
            assert_eq!(back, a);
        }
        assert_eq!(sort_key(&Value::Null), None);
    }

    #[test]
    fn min_prefers_the_int_in_a_numeric_tie() {
        let k = |v: Value| sort_key(&v).unwrap();
        assert!(k(Value::Int(1)) < k(Value::Real(1.0)));
        assert!(k(Value::Int(0)) < k(Value::Real(0.0)));
        assert!(k(Value::Real(0.0)) < k(Value::Real(-0.0)));
        assert!(k(Value::Int(i64::MAX)) < k(Value::Real(9_223_372_036_854_775_808.0)));
        assert!(k(Value::Real((1i64 << 60) as f64)) < k(Value::Int((1 << 60) + 1)));
    }

    #[test]
    fn corrupt_keys_are_errors_not_panics() {
        for bad in [
            vec![],
            vec![0x01, 0],
            vec![0x02, b'a'],
            vec![0x02, 0, 7],
            vec![0x09],
            vec![0x02, 0xff, 0, 0],
        ] {
            let e = from_sort_key(&bad).expect_err("corrupt");
            assert!(e.0.contains("corrupted MIN/MAX state"), "{bad:?}: {e}");
        }
        let mut nan = sort_key(&Value::Real(1.0)).unwrap();
        nan[1..9].copy_from_slice(&(f64::NAN.to_bits() | (1 << 63)).to_be_bytes());
        assert!(from_sort_key(&nan).is_err());
    }
}
