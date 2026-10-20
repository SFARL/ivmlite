//! The canonical byte encoding of a `Row` in a state table's `key` and `val`
//! columns (Phase 3a spec §4; REAL and BLOB from M2c spec §3): per value a tag
//! byte, then its payload — `0x00` NULL; `0x01` INTEGER, 8 bytes big-endian;
//! `0x02` TEXT, a 4-byte big-endian length, then the UTF-8 bytes; `0x03` REAL,
//! the 8 bytes of `f64::to_bits`, big-endian (so `-0.0` keeps its sign bit);
//! `0x04` BLOB, a 4-byte big-endian length, then the bytes. The same row
//! always encodes to the same bytes, so `PRIMARY KEY(key, val)` identifies a
//! row.

use ivmlite_core::{Row, StateError, Value};

const NULL: u8 = 0x00;
const INT: u8 = 0x01;
const TEXT: u8 = 0x02;
const REAL: u8 = 0x03;
const BLOB: u8 = 0x04;

pub fn encode(row: &Row) -> Vec<u8> {
    let mut out = Vec::new();
    for value in &row.0 {
        match value {
            Value::Null => out.push(NULL),
            Value::Int(n) => {
                out.push(INT);
                out.extend_from_slice(&n.to_be_bytes());
            }
            Value::Text(s) => {
                out.push(TEXT);
                let len = u32::try_from(s.len()).expect("a TEXT value SQLite stored fits in 4 GiB");
                out.extend_from_slice(&len.to_be_bytes());
                out.extend_from_slice(s.as_bytes());
            }
            Value::Real(x) => {
                out.push(REAL);
                out.extend_from_slice(&x.to_bits().to_be_bytes());
            }
            Value::Blob(b) => {
                out.push(BLOB);
                let len = u32::try_from(b.len()).expect("a BLOB value SQLite stored fits in 4 GiB");
                out.extend_from_slice(&len.to_be_bytes());
                out.extend_from_slice(b);
            }
        }
    }
    out
}

pub fn decode(bytes: &[u8]) -> Result<Row, StateError> {
    let corrupt = |what: &str| StateError(format!("corrupted state row encoding: {what}"));
    let mut values = Vec::new();
    let mut rest = bytes;
    while let Some((&tag, tail)) = rest.split_first() {
        rest = tail;
        match tag {
            NULL => values.push(Value::Null),
            INT => {
                let (n, tail) = rest
                    .split_first_chunk::<8>()
                    .ok_or_else(|| corrupt("truncated INTEGER"))?;
                values.push(Value::Int(i64::from_be_bytes(*n)));
                rest = tail;
            }
            TEXT => {
                let (len, tail) = rest
                    .split_first_chunk::<4>()
                    .ok_or_else(|| corrupt("truncated TEXT length"))?;
                let len = u32::from_be_bytes(*len) as usize;
                if tail.len() < len {
                    return Err(corrupt("truncated TEXT"));
                }
                let (text, tail) = tail.split_at(len);
                let text = std::str::from_utf8(text).map_err(|_| corrupt("TEXT is not UTF-8"))?;
                values.push(Value::Text(text.to_string()));
                rest = tail;
            }
            REAL => {
                let (bits, tail) = rest
                    .split_first_chunk::<8>()
                    .ok_or_else(|| corrupt("truncated REAL"))?;
                let x = f64::from_bits(u64::from_be_bytes(*bits));
                if x.is_nan() {
                    return Err(corrupt("REAL is NaN"));
                }
                values.push(Value::Real(x));
                rest = tail;
            }
            BLOB => {
                let (len, tail) = rest
                    .split_first_chunk::<4>()
                    .ok_or_else(|| corrupt("truncated BLOB length"))?;
                let len = u32::from_be_bytes(*len) as usize;
                if tail.len() < len {
                    return Err(corrupt("truncated BLOB"));
                }
                let (blob, tail) = tail.split_at(len);
                values.push(Value::Blob(blob.to_vec()));
                rest = tail;
            }
            other => return Err(corrupt(&format!("unknown tag {other:#04x}"))),
        }
    }
    Ok(Row::new(values))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> Row {
        Row::new(vec![
            Value::Null,
            Value::Int(-1),
            Value::Int(i64::MAX),
            Value::Text(String::new()),
            Value::Text("Ā é".into()),
            Value::Real(-0.0),
            Value::Real(f64::MAX),
            Value::Blob(vec![]),
            Value::Blob(vec![0, 0xff]),
        ])
    }

    #[test]
    fn a_row_round_trips() {
        assert_eq!(decode(&encode(&row())).unwrap(), row());
    }

    #[test]
    fn the_encoding_is_the_documented_layout() {
        // Pinned byte for byte: state tables written by one build must be
        // readable by the next, and PRIMARY KEY(key, val) relies on the same
        // row always encoding to the same bytes (spec §7).
        let bytes = encode(&Row::new(vec![
            Value::Null,
            Value::Int(1),
            Value::Text("ab".into()),
        ]));
        assert_eq!(
            bytes,
            vec![0x00, 0x01, 0, 0, 0, 0, 0, 0, 0, 1, 0x02, 0, 0, 0, 2, b'a', b'b']
        );
    }

    #[test]
    fn corrupt_bytes_are_an_error_not_a_panic() {
        for bad in [
            vec![0x01, 0, 0],             // truncated INTEGER
            vec![0x02, 0, 0, 0, 9, b'a'], // TEXT shorter than its length
            vec![0x02, 0, 0, 0, 1, 0xff], // TEXT that is not UTF-8
            vec![0x07],                   // unknown tag
            vec![0x03, 0, 0],             // truncated REAL
            vec![0x04, 0, 0, 0, 5, 1],    // BLOB shorter than its length
        ] {
            let e = decode(&bad).expect_err("corrupt state must be reported");
            assert!(e.0.contains("corrupted state row encoding"), "{e}");
        }
    }

    #[test]
    fn real_and_blob_have_the_documented_layout() {
        let bytes = encode(&Row::new(vec![Value::Real(1.0), Value::Blob(vec![7])]));
        assert_eq!(
            bytes,
            vec![0x03, 0x3f, 0xf0, 0, 0, 0, 0, 0, 0, 0x04, 0, 0, 0, 1, 7]
        );
    }

    #[test]
    fn a_nan_real_is_corrupted_state() {
        let mut bytes = vec![0x03];
        bytes.extend_from_slice(&f64::NAN.to_bits().to_be_bytes());
        let e = decode(&bytes).expect_err("spec §2: SQLite never stores a NaN");
        assert!(
            e.0.contains("corrupted state row encoding: REAL is NaN"),
            "{e}"
        );
    }

    #[test]
    fn negative_zero_round_trips_bit_for_bit() {
        let back = decode(&encode(&Row::new(vec![Value::Real(-0.0)]))).unwrap();
        match back.get(0) {
            Value::Real(x) => assert_eq!(x.to_bits(), (-0.0f64).to_bits()),
            other => panic!("{other:?}"),
        }
    }
}
