//! The names of every shadow object a view creates (Phase 3a spec §4), and
//! identifier quoting. Every statement the extension issues quotes every
//! identifier it did not write itself.

use ivmlite_core::{ArrangementId, ArrangementRole};

/// `name` as an SQL identifier: double-quoted, with each `"` doubled.
pub fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

pub const META: &str = "__ivm_meta";
pub const VIEWS: &str = "__ivm_view";
pub const DEPS: &str = "__ivm_dep";
pub const PROGRESS: &str = "__ivm_progress";

/// Every name the extension creates starts with this; the catalog refuses a
/// view over such a table.
pub const PREFIX: &str = "__ivm_";

pub fn delta_table(table: &str) -> String {
    format!("__ivm_delta_{table}")
}

pub fn trigger(table: &str, event: &str) -> String {
    format!("__ivm_{table}_{event}")
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
