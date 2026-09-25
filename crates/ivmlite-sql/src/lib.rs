//! SQL → plan IR (spec §4.3): parse a view's `SELECT` with `sqlparser`'s SQLite
//! dialect, resolve its names against a [`Catalog`], and lower it through
//! `ivmlite_core::lower`, which holds every legality check. Any query outside
//! v0's subset is a hard error (spec §12.5).

mod catalog;
mod compile;
mod source;

pub use catalog::{Catalog, CatalogError};
pub use compile::{compile, CompiledView, SqlError};
