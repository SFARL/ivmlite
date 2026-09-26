//! The canonical byte encoding of a `Row` in a state table's `key` and `val`
//! columns (Phase 3a spec §4): per value a tag byte, then its payload —
//! `0x00` NULL; `0x01` INTEGER, 8 bytes big-endian; `0x02` TEXT, a 4-byte
//! big-endian length, then the UTF-8 bytes. The same row always encodes to the
//! same bytes, so `PRIMARY KEY(key, val)` identifies a row.

use ivmlite_core::{Row, StateError, Value};

const NULL: u8 = 0x00;
const INT: u8 = 0x01;
const TEXT: u8 = 0x02;

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
        ] {
            let e = decode(&bad).expect_err("corrupt state must be reported");
            assert!(e.0.contains("corrupted state row encoding"), "{e}");
        }
    }
}
