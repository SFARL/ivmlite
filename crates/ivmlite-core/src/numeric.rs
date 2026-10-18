//! M2c spec §4 "How a value counts": SQLite's `sqlite3_value_numeric_type`
//! for TEXT (numeric affinity without changing what is stored) and
//! `sqlite3_value_double` for TEXT and BLOB, after the grammar of SQLite's
//! `sqlite3AtoF`. The digits are converted by Rust's correctly rounded `f64`
//! parser; `tests/numeric_terms.rs` in `ivmlite-test` pins the grammar
//! against the SQLite the tests link.

use crate::Value;

/// How SUM/TOTAL/AVG count one non-NULL value (M2c spec §4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NumericTerm {
    Integer(i64),
    Approx(f64),
}

/// What `sqlite3AtoF` reads from a byte string.
struct Scan {
    /// The consumed prefix's value (`0.0` when no digit was read).
    value: f64,
    /// Every byte was consumed, up to trailing whitespace.
    well_formed: bool,
    /// Well formed, no `.`, no exponent, and the digits fit `i64`.
    integer: Option<i64>,
}

fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// Consume an optional `+` or `-` at `*i`; `true` for a minus sign.
fn sign(bytes: &[u8], i: &mut usize) -> bool {
    let negative = bytes.get(*i) == Some(&b'-');
    if matches!(bytes.get(*i), Some(b'-' | b'+')) {
        *i += 1;
    }
    negative
}

/// Consume the run of ASCII digits at `*i`, as text (empty when none).
fn digits<'a>(bytes: &'a [u8], i: &mut usize) -> &'a str {
    let start = *i;
    while bytes.get(*i).is_some_and(u8::is_ascii_digit) {
        *i += 1;
    }
    std::str::from_utf8(&bytes[start..*i]).expect("ASCII digits are UTF-8")
}

/// `digits`, or `"0"` when the run is empty: the normalized form's filler.
fn digits_or_zero(run: &str) -> &str {
    if run.is_empty() {
        "0"
    } else {
        run
    }
}

fn scan(bytes: &[u8]) -> Scan {
    let mut i = 0;
    while bytes.get(i).is_some_and(|&b| is_space(b)) {
        i += 1;
    }
    let negative = sign(bytes, &mut i);
    let whole = digits(bytes, &mut i);
    let has_dot = bytes.get(i) == Some(&b'.');
    let mut fraction = "";
    if has_dot {
        i += 1;
        fraction = digits(bytes, &mut i);
    }
    if whole.is_empty() && fraction.is_empty() {
        return Scan {
            value: 0.0,
            well_formed: false,
            integer: None,
        };
    }
    // An exponent counts only with at least one digit; otherwise the `e` and
    // its sign are left unconsumed and make the text not well formed.
    let mut exponent_negative = false;
    let mut exponent = "";
    if matches!(bytes.get(i), Some(b'e' | b'E')) {
        let mut j = i + 1;
        let negative_exponent = sign(bytes, &mut j);
        let run = digits(bytes, &mut j);
        if !run.is_empty() {
            exponent = run;
            exponent_negative = negative_exponent;
            i = j;
        }
    }
    while bytes.get(i).is_some_and(|&b| is_space(b)) {
        i += 1;
    }
    let well_formed = i == bytes.len();
    let minus = if negative { "-" } else { "" };
    let normalized = format!(
        "{minus}{}.{}e{}{}",
        digits_or_zero(whole),
        digits_or_zero(fraction),
        if exponent_negative { "-" } else { "" },
        digits_or_zero(exponent),
    );
    let value = normalized
        .parse()
        .expect("the normalized form is always a valid float");
    let integer = if well_formed && !has_dot && exponent.is_empty() {
        format!("{minus}{whole}").parse().ok()
    } else {
        None
    };
    Scan {
        value,
        well_formed,
        integer,
    }
}

/// `scan` of a TEXT value. SQLite reads a TEXT up to its first NUL byte
/// (M2c spec §4), so `"1\0x"` is `"1"`; a BLOB is read whole.
fn scan_text(text: &str) -> Scan {
    let bytes = text.as_bytes();
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    scan(&bytes[..end])
}

fn term_of(scanned: &Scan) -> NumericTerm {
    scanned
        .integer
        .map_or(NumericTerm::Approx(scanned.value), NumericTerm::Integer)
}

/// How SUM/TOTAL/AVG count `value`; `None` for NULL. A BLOB counts as its
/// bytes read as text and is never an integer.
pub fn numeric_term(value: &Value) -> Option<NumericTerm> {
    Some(match value {
        Value::Null => return None,
        Value::Int(n) => NumericTerm::Integer(*n),
        Value::Real(x) => NumericTerm::Approx(*x),
        Value::Text(s) => term_of(&scan_text(s)),
        Value::Blob(b) => NumericTerm::Approx(scan(b).value),
    })
}

/// SQLite's reading of a TEXT as a number, as `sum` takes it (M2c spec §4,
/// §5): `Some` when the whole text is a well-formed number, `None`
/// otherwise. An integer text that fits `i64` is `Integer`; any other
/// well-formed number, including `"3.0"`, `"2.5e1"` and an integer that
/// overflows `i64`, is `Approx`. This is a pure parse: it does not apply
/// column affinity, which also turns an integral `Approx` into an integer
/// and `-0.0` into `0`.
pub fn parse_numeric_text(text: &str) -> Option<NumericTerm> {
    let scanned = scan_text(text);
    scanned.well_formed.then(|| term_of(&scanned))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Value;
    use NumericTerm::{Approx, Integer};

    fn text(s: &str) -> Option<NumericTerm> {
        numeric_term(&Value::Text(s.into()))
    }

    #[test]
    fn integers_and_reals_count_as_themselves() {
        assert_eq!(numeric_term(&Value::Int(-3)), Some(Integer(-3)));
        assert_eq!(numeric_term(&Value::Real(1.0)), Some(Approx(1.0)));
        assert_eq!(numeric_term(&Value::Null), None);
    }

    #[test]
    fn text_follows_sqlites_measured_classification() {
        // Each pair measured with `SELECT sum(?1), typeof(sum(?1))` on SQLite 3.53.
        assert_eq!(text("12"), Some(Integer(12)));
        assert_eq!(text(" 12 "), Some(Integer(12)));
        assert_eq!(text("+7"), Some(Integer(7)));
        assert_eq!(text("-0"), Some(Integer(0)));
        assert_eq!(text("12abc"), Some(Approx(12.0)));
        assert_eq!(text("abc"), Some(Approx(0.0)));
        assert_eq!(text("1.0"), Some(Approx(1.0)));
        assert_eq!(text("1."), Some(Approx(1.0)));
        assert_eq!(text(".5"), Some(Approx(0.5)));
        assert_eq!(text("1e2"), Some(Approx(100.0)));
        assert_eq!(text("1e"), Some(Approx(1.0)));
        assert_eq!(text(" 0x10"), Some(Approx(0.0)));
        assert_eq!(text("inf"), Some(Approx(0.0)));
        assert_eq!(text("1e400"), Some(Approx(f64::INFINITY)));
        assert_eq!(
            text("9223372036854775808"),
            Some(Approx(9_223_372_036_854_775_808.0))
        );
        assert_eq!(text("-9223372036854775808"), Some(Integer(i64::MIN)));
        assert_eq!(text(""), Some(Approx(0.0)));
    }

    #[test]
    fn a_blob_counts_as_its_bytes_read_as_text_and_is_approximate() {
        assert_eq!(
            numeric_term(&Value::Blob(b"12".to_vec())),
            Some(Approx(12.0))
        );
        assert_eq!(numeric_term(&Value::Blob(vec![0xff])), Some(Approx(0.0)));
    }

    #[test]
    fn parse_numeric_text_reads_only_well_formed_numbers_as_sum_does() {
        assert_eq!(parse_numeric_text(" 3 "), Some(Integer(3)));
        assert_eq!(parse_numeric_text("3.0"), Some(Approx(3.0)));
        assert_eq!(parse_numeric_text("2.5e1"), Some(Approx(25.0)));
        assert_eq!(parse_numeric_text("3x"), None);
        assert_eq!(parse_numeric_text("abc"), None);
    }

    #[test]
    fn a_text_ends_at_its_first_nul_but_a_blob_does_not() {
        assert_eq!(text("1\0"), Some(Integer(1)));
        assert_eq!(text("1\0x"), Some(Integer(1)));
        assert_eq!(text("\x001"), Some(Approx(0.0)));
        assert_eq!(text("12 \0"), Some(Integer(12)));
        assert_eq!(text("1.5\0x"), Some(Approx(1.5)));
        assert_eq!(parse_numeric_text("1\0x"), Some(Integer(1)));
        assert_eq!(
            numeric_term(&Value::Blob(b"1\0".to_vec())),
            Some(Approx(1.0))
        );
    }
}
