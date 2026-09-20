use crate::Schema;

/// 差分用例涉及的全部基表。
///
/// 用 `Vec` 而非 `HashMap`：表的数量是个位数，按名查找的线性扫描无关紧要，
/// 而顺序确定是硬要求——失败用例必须能凭 seed 精确重放（spec §9.4）。
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Database {
    tables: Vec<Schema>,
}

impl Database {
    pub fn new(tables: Vec<Schema>) -> Self {
        Database { tables }
    }

    /// 单表用例就是只有一张表的多表用例。
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

    #[test]
    fn table_order_is_preserved() {
        let db = Database::new(vec![s("b"), s("a")]);
        let names: Vec<&str> = db.tables().iter().map(|t| t.table.as_str()).collect();
        assert_eq!(
            names,
            vec!["b", "a"],
            "顺序必须保留——失败用例要凭 seed 精确重放"
        );
    }
}
