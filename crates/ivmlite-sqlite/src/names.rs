//! The names of every shadow object a view creates (Phase 3a spec §4), and
//! identifier quoting. Every statement the extension issues quotes every
//! identifier it did not write itself.

use ivmlite_core::{ArrangementId, ArrangementRole};

/// `name` as an SQL identifier: double-quoted, with each `"` doubled.
pub fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// `name`, quoted and qualified to the `main` schema: `"main"."name"`.
///
/// Every statement that reads, writes, creates or drops a base table or any
/// table or trigger ivmlite owns (the global, delta, state, output and stage
/// tables and their triggers) uses this rather than an unqualified name.
/// SQLite resolves an unqualified table reference against `temp` before
/// `main`, so a same-named TEMP table would otherwise silently shadow the
/// real one — be read, written or dropped instead of it, or receive the
/// delta triggers instead of the base table itself. Two places cannot be
/// qualified: a trigger's `ON <table>` clause (schema-qualifying the
/// *trigger's own name* still binds it to `main`, because a schema-qualified
/// trigger's `ON` table must live in that same schema), and the tables named
/// inside a trigger body (SQLite rejects a qualified name there; a trigger in
/// `main` resolves its body's names in `main`).
pub fn main_qualified(name: &str) -> String {
    format!("\"main\".{}", quote(name))
}

pub const META: &str = "__ivm_meta";
pub const VIEWS: &str = "__ivm_view";
pub const DEPS: &str = "__ivm_dep";
pub const PROGRESS: &str = "__ivm_progress";

/// One row per tracked base table: its shape, as generated once when the
/// table was first tracked (Phase 3b spec §3). Every view that reads the
/// table relies on that same shape and the triggers built from it.
pub const TRACKED: &str = "__ivm_tracked";

/// Every capture trigger event a tracked base table gets (Phase 3b spec §3,
/// §6.2): the AFTER triggers `ins`, `del` and `upd`, and the BEFORE triggers
/// `preins` and `preupd` that record REPLACE candidates. Every loop over a
/// table's capture triggers uses this constant, not a literal list.
///
/// No event contains `_`, so `__ivm_trig_<table>_<event>` still splits
/// uniquely at its last `_`, and no two (table, event) pairs spell the same
/// trigger name.
pub const CAPTURE_EVENTS: [&str; 5] = ["ins", "del", "upd", "preins", "preupd"];

/// The recursive-triggers probe (Phase 3b spec §6.2): a global table whose
/// one trigger, `PROBE_STEP`, re-inserts into it. With `recursive_triggers`
/// OFF, inserting a 0 leaves 2 rows; with it ON, 3. A trigger body cannot
/// read `pragma_recursive_triggers`, so this is how the capture triggers
/// tell the two apart.
pub const PROBE: &str = "__ivm_probe";
pub const PROBE_STEP: &str = "__ivm_probe_step";

/// Every name the extension creates starts with this; the catalog refuses a
/// view over such a table, and refuses a base column with this prefix too (a
/// delta table's own columns are prefixed with it — see `DELTA_SEQ`/`DELTA_W`
/// below — so a colliding base column would otherwise be ambiguous).
pub const PREFIX: &str = "__ivm_";

/// Whether `name` starts with `PREFIX`, in any case (SQLite matches names
/// case-insensitively). Compared as bytes: slicing the `str` at
/// `PREFIX.len()` would panic on a name whose sixth byte falls inside a
/// multi-byte character.
pub fn has_reserved_prefix(name: &str) -> bool {
    name.len() >= PREFIX.len()
        && name.as_bytes()[..PREFIX.len()].eq_ignore_ascii_case(PREFIX.as_bytes())
}

/// The delta table's own two columns, prefixed so a base column literally
/// named `seq` or `w` is not shadowed (both are ordinary column names a base
/// table is free to use).
pub const DELTA_SEQ: &str = "__ivm_seq";
pub const DELTA_W: &str = "__ivm_w";

pub fn delta_table(table: &str) -> String {
    format!("__ivm_delta_{table}")
}

/// A base table's REPLACE candidates, held between a BEFORE capture trigger
/// and its AFTER trigger (Phase 3b spec §6.2).
pub fn pend_table(table: &str) -> String {
    format!("__ivm_pend_{table}")
}

/// A base table's delta-maintaining trigger. Named `__ivm_trig_<table>_<event>`
/// — the `trig_` infix, not just `__ivm_<table>_<event>`, so that no base
/// table name and event can ever spell the same string as
/// `apply_trigger`'s `__ivm_apply_<view>` (e.g. table `apply_v`, event `x` used
/// to collide with view `v_x`).
pub fn trigger(table: &str, event: &str) -> String {
    format!("__ivm_trig_{table}_{event}")
}

/// Where a refresh stages every change before applying them in one statement.
pub fn stage_table(view: &str) -> String {
    format!("__ivm_stage_{view}")
}

/// The trigger on the stage table that applies the staged changes.
pub fn apply_trigger(view: &str) -> String {
    format!("__ivm_apply_{view}")
}

/// `text` as an SQL string literal.
pub fn literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

pub fn out_table(view: &str) -> String {
    format!("__ivm_out_{view}")
}

/// The output table's index (Phase 4 spec §4): non-unique, over every output
/// column, so a retraction's existence check and its `DELETE` are a search
/// rather than a full scan. Created with the output table and dropped with
/// it — SQLite drops a table's indexes when the table itself is dropped, so
/// nothing else needs to name this index.
pub fn out_index(view: &str) -> String {
    format!("__ivm_outidx_{view}")
}

/// One table per `(node, role)` (spec §7). The role is a fixed string, never
/// the enum's `Debug` output, so renaming a variant cannot rename a table.
pub fn state_table(view: &str, id: ArrangementId) -> String {
    let role = match id.role {
        ArrangementRole::JoinLeft => "join_left",
        ArrangementRole::JoinRight => "join_right",
        ArrangementRole::AggregateGroups => "agg_groups",
    };
    format!("__ivm_state_{view}_{}_{role}", id.node)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trigger_name_never_collides_with_an_apply_trigger_name() {
        // Before the `trig_` infix, `trigger("apply_v", "x")` and
        // `apply_trigger("v_x")` both rendered as `__ivm_apply_v_x`: a base
        // table named `apply_v` and a view named `v_x` would fight over the
        // same trigger.
        assert_ne!(trigger("apply_v", "x"), apply_trigger("v_x"));
        assert_eq!(trigger("apply_v", "x"), "__ivm_trig_apply_v_x");
        assert_eq!(apply_trigger("v_x"), "__ivm_apply_v_x");
    }

    #[test]
    fn the_reserved_prefix_is_matched_in_any_case_and_never_panics() {
        assert!(has_reserved_prefix("__ivm_x"));
        assert!(has_reserved_prefix("__IVM_out_v"));
        assert!(!has_reserved_prefix("__ivm"));
        assert!(!has_reserved_prefix("ivm_x"));
        // The sixth byte falls inside `€`: a `str` slice there would panic.
        assert!(!has_reserved_prefix("abcde€"));
    }

    #[test]
    fn main_qualified_quotes_and_prefixes_the_schema() {
        assert_eq!(main_qualified("t"), "\"main\".\"t\"");
        assert_eq!(main_qualified("a\"b"), "\"main\".\"a\"\"b\"");
    }
}
