use ivmlite_core::{Database, Schema};

/// Where the SQL front end looks tables up (spec §4.3).
///
/// A trait so the front end is testable without SQLite: `Database` implements
/// it here. M1b Phase 3's SQLite implementation reads `PRAGMA table_info` and
/// the table's DDL, and reports what v0 cannot represent — a non-STRICT table,
/// an `ANY` column, a `COLLATE` clause (spec §7.1) — as a `CatalogError`.
pub trait Catalog {
    /// The table called `name`, matched the way SQLite matches identifiers:
    /// ASCII case-insensitively, quoted or not. `Ok(None)` if there is none.
    fn table(&self, name: &str) -> Result<Option<Schema>, CatalogError>;
}

/// A table exists but cannot be used, or the catalog could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogError(pub String);

impl Catalog for Database {
    fn table(&self, name: &str) -> Result<Option<Schema>, CatalogError> {
        Ok(self
            .tables()
            .iter()
            .find(|s| s.table.eq_ignore_ascii_case(name))
            .cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ivmlite_core::{Column, ColumnType};

    fn db() -> Database {
        Database::single(Schema {
            table: "Orders".into(),
            columns: vec![Column {
                name: "amount".into(),
                ty: ColumnType::Integer,
                nullable: false,
            }],
        })
    }

    #[test]
    fn a_database_finds_tables_case_insensitively_and_keeps_the_declared_name() {
        let found = db().table("ORDERS").unwrap().expect("ORDERS names Orders");
        assert_eq!(found.table, "Orders");
    }

    #[test]
    fn a_database_reports_an_unknown_table_as_none() {
        assert_eq!(db().table("customers"), Ok(None));
    }
}
