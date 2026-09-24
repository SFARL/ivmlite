#!/usr/bin/env python3
"""Count the rows and verification statuses in docs/mutation-gates.md, and check
them against the totals sentence at the end of the document.

The gate table's whole value is that its claims can be checked mechanically. A
hand-written total is itself an unchecked claim — the first version was exactly
that, and it was wrong (12/26).

Rows are split into cells the way GitHub renders them (GitHub Flavored
Markdown): an unescaped `|` ends a cell **even inside a backtick code span**,
and `\\|` is a literal pipe that never does. A row that does not split into
exactly four cells is an error, never skipped: an earlier version skipped such
rows silently, and two rows that contained a `|` were missing from the totals.

Usage: scripts/count-mutation-gates.py        # check; non-zero exit on mismatch
       scripts/count-mutation-gates.py --fix  # rewrite the totals from the actual counts
"""
import re
import sys
from pathlib import Path

DOC = Path(__file__).resolve().parent.parent / "docs" / "mutation-gates.md"
SENTENCE = (
    "The table has **{total}** rows: **{verified}** verified, "
    "**{unverified}** unverified, **{na}** n/a."
)
SENTENCE_RE = re.compile(
    r"The table has \*\*\d+\*\* rows: \*\*\d+\*\* verified, "
    r"\*\*\d+\*\* unverified, \*\*\d+\*\* n/a\."
)
HEADER_FIRST_CELL = "Spec requirement"
COLUMNS = 4
DELIMITER_CELL = re.compile(r":?-+:?")


class TableError(ValueError):
    """A table row the script cannot read; reported instead of skipped."""


def split_row(line):
    """The cells of one table row, stripped, as GitHub splits them.

    A backslash escapes the character after it, so `\\|` stays inside its cell
    (and is kept verbatim); every other `|` separates cells. The row's leading
    and trailing pipes delimit it and do not open empty cells.
    """
    body = line.strip()
    cells, cell = [], []
    ends_with_separator = False
    i = 0
    while i < len(body):
        char = body[i]
        if char == "\\" and i + 1 < len(body):
            cell.append(body[i : i + 2])
            i += 2
            ends_with_separator = False
            continue
        if char == "|":
            cells.append("".join(cell))
            cell = []
            ends_with_separator = True
        else:
            cell.append(char)
            ends_with_separator = False
        i += 1
    if not ends_with_separator:
        cells.append("".join(cell))
    if body.startswith("|"):
        cells = cells[1:]
    return [c.strip() for c in cells]


def count(text):
    total = verified = unverified = na = 0
    for lineno, line in enumerate(text.splitlines(), start=1):
        if not line.startswith("|"):
            continue
        cells = split_row(line)
        if len(cells) != COLUMNS:
            raise TableError(
                f"docs/mutation-gates.md:{lineno}: a table row must split into exactly "
                f"{COLUMNS} cells, found {len(cells)}.\n"
                "An unescaped `|` ends a cell even inside a backtick code span (as on "
                "GitHub); write a literal pipe as `\\|`.\n"
                f"row begins: {line[:120]!r}"
            )
        if cells[0] == HEADER_FIRST_CELL or all(DELIMITER_CELL.fullmatch(c) for c in cells):
            continue
        total += 1
        # The status is decided by how the cell *begins*, never by substring
        # containment. Containment has a real trap: an n/a row explaining itself
        # can perfectly well say "this was previously marked verified in error",
        # and would then be counted as verified — a silent misclassification in a
        # document whose whole point is being mechanically checkable. The
        # implementer of M1a Phase 2 Task 2 hit exactly this and worked around it
        # by rewording; working around it is not fixing it.
        status = cells[3].lstrip("*").strip()
        if status.startswith("verified"):
            verified += 1
        elif status.startswith("unverified"):
            unverified += 1
        elif status.startswith("n/a"):
            na += 1
        else:
            raise TableError(
                f"docs/mutation-gates.md:{lineno}: cannot classify the status cell "
                f"{cells[3][:120]!r}\n"
                'it must begin with "verified", "unverified" or "n/a".'
            )
    return dict(total=total, verified=verified, unverified=unverified, na=na)


def main():
    text = DOC.read_text()
    try:
        actual = count(text)
    except TableError as error:
        print(error, file=sys.stderr)
        return 1
    want = SENTENCE.format(**actual)
    found = SENTENCE_RE.search(text)
    if not found:
        print(
            "the totals sentence is missing — it is the table's only checkable entry point",
            file=sys.stderr,
        )
        return 1
    if found.group(0) == want:
        print(f"consistent: {want}")
        return 0
    if "--fix" in sys.argv:
        DOC.write_text(text[: found.start()] + want + text[found.end() :])
        print(f"rewrote to: {want}")
        return 0
    print(f"mismatch\n  document: {found.group(0)}\n  actual:   {want}", file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(main())
