use crate::Schema;

/// Every base table a differential test case involves.
///
/// A `Vec` rather than a `HashMap`: there are only a handful of tables, so a
/// linear scan to look one up by name does not matter, while a deterministic
/// order is a hard requirement — a failing case must replay exactly from its
/// seed (spec §9.4).
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Database {
    tables: Vec<Schema>,
}

impl Database {
    pub fn new(tables: Vec<Schema>) -> Self {
        Database { tables }
    }

    /// A single-table case is just a multi-table case with one table.
    pub fn single(schema: Schema) -> Self {
        Database {
            tables: vec![schema],
        }
    }

    pub fn tables(&self) -> &[Schema] {
        &self.tables
    }

    pub fn get(&self, table: &str) -> Option<&Schema> {
        self.tables.iter().find(|s| s.table == table)
    }

    pub fn len(&self) -> usize {
        self.tables.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tables.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Column, ColumnType};

    fn s(name: &str) -> Schema {
        Schema {
            table: name.into(),
            columns: vec![Column {
                name: "a".into(),
                ty: ColumnType::Integer,
                nullable: false,
            }],
        }
    }

    #[test]
    fn single_wraps_one_schema() {
        let db = Database::single(s("orders"));
        assert_eq!(db.len(), 1);
        assert_eq!(db.get("orders").map(|x| x.table.as_str()), Some("orders"));
    }

    #[test]
    fn get_returns_none_for_unknown_table() {
        assert!(Database::single(s("orders")).get("nope").is_none());
    }

    /// Order must be preserved — a failing case must replay exactly from its
    /// seed (spec §9.4).
    ///
    /// This is still only a statistical guard, not a proof: if `Database` were
    /// changed internally to route by name through a `HashMap`, nothing outside
    /// the type could pin "the order is deterministic" as a certainty —
    /// `HashMap`'s `RandomState` re-seeds per process, so any run might happen to
    /// yield the insertion order. With two tables the chance of a coincidentally
    /// correct order is 1/2! = 50%, which makes the test worthless; five distinct
    /// tables push it down to 1/5! ≈ 0.83%, and a few runs filter the coincidence
    /// out. The insertion order is deliberately neither lexicographic nor its
    /// reverse, so an implementation that "looks order-preserving but sorts by
    /// name internally" is caught as well.
    #[test]
    fn table_order_is_preserved() {
        let db = Database::new(vec![s("c"), s("a"), s("e"), s("b"), s("d")]);
        let names: Vec<&str> = db.tables().iter().map(|t| t.table.as_str()).collect();
        assert_eq!(
            names,
            vec!["c", "a", "e", "b", "d"],
            "order must be preserved — a failing case must replay exactly from its seed"
        );
    }
}
