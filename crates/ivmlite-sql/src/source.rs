//! Recovering a result column's text from the view's SQL, so an unaliased
//! aggregate gets the name SQLite gives it: its text exactly as written.

use sqlparser::tokenizer::Location;

/// The byte offset of `location` in `sql`. `sqlparser` counts lines and
/// columns from 1, in characters.
pub fn offset(sql: &str, location: Location) -> usize {
    let mut line_start = 0;
    for _ in 1..location.line {
        line_start += sql[line_start..]
            .find('\n')
            .expect("a location the parser reported lies inside the SQL")
            + 1;
    }
    let column = usize::try_from(location.column).expect("a column fits in usize") - 1;
    sql[line_start..]
        .char_indices()
        .nth(column)
        .map_or(sql.len(), |(i, _)| line_start + i)
}

/// The text of the function call that starts at byte `start`: its name
/// through its matching `)`.
///
/// `sqlparser` 0.63's span for a call ends at its last argument, before the
/// `)` (`COUNT(*)`'s span covers only `COUNT`), so the end is found here by
/// matching parentheses, skipping quoted strings and identifiers.
pub fn call_text(sql: &str, start: usize) -> &str {
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    for (i, c) in sql[start..].char_indices() {
        match quote {
            Some(close) => {
                if c == close {
                    quote = None;
                }
            }
            None => match c {
                '\'' | '"' | '`' => quote = Some(c),
                '[' => quote = Some(']'),
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return &sql[start..start + i + c.len_utf8()];
                    }
                }
                _ => {}
            },
        }
    }
    panic!(
        "the parser accepted this call, so its parentheses balance: {}",
        &sql[start..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_counts_lines_and_characters() {
        let sql = "SELECT\n  é, COUNT(*)";
        let at = offset(sql, Location::new(2, 6));
        assert_eq!(&sql[at..], "COUNT(*)");
    }

    #[test]
    fn call_text_runs_to_the_matching_parenthesis() {
        let sql = r#"SELECT sum ( "a)b" ), 1"#;
        assert_eq!(call_text(sql, 7), r#"sum ( "a)b" )"#);
    }
}
