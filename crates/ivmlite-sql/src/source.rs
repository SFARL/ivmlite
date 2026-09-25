//! Recovering the SELECT list's source text, so an unaliased result column
//! gets the name SQLite gives it (measured against SQLite 3.53 through
//! `rusqlite` in `result_column_names_match_sqlite`): the item's text exactly
//! as written, from its first real token through its last, with leading
//! whitespace and comments (and, for the first item, a leading `ALL` or
//! `DISTINCT`) skipped and trailing whitespace trimmed — a trailing comment
//! is kept, since SQLite keeps it too.
//!
//! `sqlparser` 0.63's own `Span`s cannot give us this directly: a call's span
//! ends before its `)` (`COUNT(*)`'s span covers only `COUNT`), a
//! parenthesized expression's span starts at the inner expression (dropping
//! the `(`), and no span at all marks where a comment or piece of whitespace
//! sits relative to it. So this re-tokenizes the SQL — with `sqlparser`'s own
//! tokenizer, which already handles quoting and comments — and works from the
//! token stream instead.

use sqlparser::dialect::SQLiteDialect;
use sqlparser::keywords::Keyword;
use sqlparser::tokenizer::{Location, Token, TokenWithSpan, Tokenizer, Whitespace};

use crate::SqlError;

fn malformed() -> SqlError {
    SqlError(
        "cannot recover the SELECT list's source text (the view's SQL has an unexpected shape)"
            .to_string(),
    )
}

fn is_keyword(token: &Token, keyword: Keyword) -> bool {
    matches!(token, Token::Word(w) if w.keyword == keyword)
}

/// Pure whitespace — not a comment. `sqlparser` tokenizes both as
/// `Token::Whitespace`; only this narrower set is trimmed from the end of a
/// piece (a trailing comment stays, matching SQLite).
fn is_pure_whitespace(token: &Token) -> bool {
    matches!(
        token,
        Token::Whitespace(Whitespace::Space | Whitespace::Newline | Whitespace::Tab)
    )
}

fn is_whitespace_or_comment(token: &Token) -> bool {
    matches!(token, Token::Whitespace(_))
}

/// The byte offsets, in `sql`, of the start of each of its lines — index `i`
/// is line `i + 1`, matching how `sqlparser` counts lines from 1.
fn line_starts(sql: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(sql.match_indices('\n').map(|(i, _)| i + 1))
        .collect()
}

/// The byte offset of `location` in `sql`. `sqlparser` counts lines and
/// columns from 1, in characters. Every `location` this module passes here
/// comes from tokenizing `sql` itself, so it always lands inside `sql` — but
/// this clamps instead of trusting that, so a future caller (or a `sqlparser`
/// quirk this crate has not seen) gets a slightly wrong slice, never a panic.
fn offset(sql: &str, line_starts: &[usize], location: Location) -> usize {
    let line_start = usize::try_from(location.line.saturating_sub(1))
        .ok()
        .and_then(|i| line_starts.get(i))
        .copied()
        .unwrap_or(sql.len());
    let column = usize::try_from(location.column.saturating_sub(1)).unwrap_or(0);
    sql[line_start..]
        .char_indices()
        .nth(column)
        .map_or(sql.len(), |(i, _)| line_start + i)
}

/// The text of one SELECT-list piece — tokens `start..end` of `tokens` — the
/// way SQLite would name it if left unaliased.
fn piece_text(
    sql: &str,
    line_starts: &[usize],
    tokens: &[TokenWithSpan],
    start: usize,
    end: usize,
    is_first_item: bool,
) -> Result<String, SqlError> {
    let mut first = start;
    let mut skip_all_distinct = is_first_item;
    while first < end {
        match &tokens[first].token {
            t if is_whitespace_or_comment(t) => first += 1,
            Token::Word(w)
                if skip_all_distinct && matches!(w.keyword, Keyword::ALL | Keyword::DISTINCT) =>
            {
                skip_all_distinct = false;
                first += 1;
            }
            _ => break,
        }
    }
    if first >= end {
        return Err(malformed());
    }
    let mut last = end - 1;
    while last > first && is_pure_whitespace(&tokens[last].token) {
        last -= 1;
    }
    let from = offset(sql, line_starts, tokens[first].span.start);
    let to = offset(sql, line_starts, tokens[last].span.end);
    sql.get(from..to).map(str::to_string).ok_or_else(malformed)
}

/// The SELECT list's items, each as the source text SQLite would use to name
/// it if left unaliased. Aligned in order with `select.projection` — the
/// caller is expected to check `pieces.len() == item_count`, which this does
/// for its own consistency check (a `sqlparser` bug or a construct this crate
/// has not yet special-cased could break the alignment; either is a
/// `SqlError`, never a panic, since it depends on the input SQL).
pub fn select_list_pieces(sql: &str, item_count: usize) -> Result<Vec<String>, SqlError> {
    let tokens = Tokenizer::new(&SQLiteDialect {}, sql)
        .tokenize_with_location()
        .map_err(|e| SqlError(format!("cannot tokenize the view's SQL: {e}")))?;

    // The query's only SELECT keyword: a compound query or one with WITH is
    // rejected before this runs, and v0 accepts no subquery anywhere, so
    // exactly one appears, at the very start.
    let list_start = tokens
        .iter()
        .position(|t| is_keyword(&t.token, Keyword::SELECT))
        .map(|i| i + 1)
        .ok_or_else(malformed)?;

    let mut depth = 0i32;
    let mut piece_start = list_start;
    let mut boundaries: Vec<(usize, usize)> = Vec::new();
    let mut found_from = false;
    let mut i = list_start;
    while i < tokens.len() {
        match &tokens[i].token {
            Token::LParen => depth += 1,
            Token::RParen => depth -= 1,
            Token::Comma if depth == 0 => {
                boundaries.push((piece_start, i));
                piece_start = i + 1;
            }
            t if depth == 0 && is_keyword(t, Keyword::FROM) => {
                boundaries.push((piece_start, i));
                found_from = true;
                break;
            }
            _ => {}
        }
        i += 1;
    }
    if !found_from || boundaries.len() != item_count {
        return Err(malformed());
    }

    let line_starts = line_starts(sql);
    boundaries
        .into_iter()
        .enumerate()
        .map(|(index, (start, end))| piece_text(sql, &line_starts, &tokens, start, end, index == 0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pieces(sql: &str, item_count: usize) -> Vec<String> {
        select_list_pieces(sql, item_count).unwrap_or_else(|e| panic!("{sql}: {e}"))
    }

    #[test]
    fn a_plain_call_is_its_own_text() {
        assert_eq!(
            pieces("SELECT k, COUNT(*) FROM t0", 2),
            vec!["k".to_string(), "COUNT(*)".to_string()]
        );
    }

    #[test]
    fn a_comment_inside_a_call_does_not_unbalance_it() {
        // Final review Critical 1: a `(`, `'`, `"` or `` ` `` inside a
        // comment must not confuse the recovery, whatever character it is.
        for sql in [
            "SELECT COUNT(/*(*/*) FROM t0",
            "SELECT COUNT(/*'*/*) FROM t0",
            "SELECT SUM(v /* \" */) FROM t0",
        ] {
            let p = pieces(sql, 1);
            assert_eq!(p.len(), 1, "{sql}");
        }
    }

    #[test]
    fn a_single_line_comment_inside_a_call_runs_to_the_newline() {
        let p = pieces("SELECT COUNT(* -- (\n) FROM t0", 1);
        assert_eq!(p, vec!["COUNT(* -- (\n)".to_string()]);
    }

    #[test]
    fn a_parenthesized_call_keeps_its_parentheses() {
        // `sqlparser`'s span for `Expr::Nested` starts at the inner
        // expression; the token-based piece keeps the outer `(` and `)`,
        // matching SQLite.
        assert_eq!(pieces("SELECT (COUNT(*)) FROM t0", 1), vec!["(COUNT(*))"]);
        assert_eq!(pieces("SELECT ( SUM(v) ) FROM t0", 1), vec!["( SUM(v) )"]);
    }

    #[test]
    fn a_comment_before_an_item_is_not_part_of_its_name() {
        assert_eq!(
            pieces("SELECT k, /*c*/ COUNT(*) FROM t0", 2),
            vec!["k".to_string(), "COUNT(*)".to_string()]
        );
    }

    #[test]
    fn a_trailing_comment_stays_but_trailing_whitespace_is_trimmed() {
        assert_eq!(
            pieces("SELECT COUNT(*) -- )\nFROM t0", 1),
            vec!["COUNT(*) -- )".to_string()]
        );
        assert_eq!(
            pieces("SELECT COUNT(*)   FROM t0", 1),
            vec!["COUNT(*)".to_string()]
        );
    }

    #[test]
    fn a_leading_all_is_skipped_for_the_first_item_only() {
        assert_eq!(pieces("SELECT ALL COUNT(*) FROM t0", 1), vec!["COUNT(*)"]);
    }
}
