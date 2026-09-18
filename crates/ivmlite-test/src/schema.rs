#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnType {
    Integer,
    Text,
}

impl ColumnType {
    fn sql(self) -> &'static str {
        match self {
            ColumnType::Integer => "INTEGER",
            ColumnType::Text => "TEXT",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Column {
    pub name: String,
    pub ty: ColumnType,
    pub nullable: bool,
}

#[derive(Debug, Clone)]
pub struct Schema {
    pub table: String,
    pub columns: Vec<Column>,
}

impl Schema {
    /// 生成 STRICT 建表语句。STRICT 是 v0 的硬性要求：它把列类型钉死，
    /// 从而消灭 SQLite 的 type affinity 导致 group key 分裂的整类问题（spec §7.1）。
    pub fn create_table_sql(&self) -> String {
        let cols: Vec<String> = self
            .columns
            .iter()
            .map(|c| {
                let null = if c.nullable { "" } else { " NOT NULL" };
                format!("\"{}\" {}{}", c.name, c.ty.sql(), null)
            })
            .collect();
        format!(
            "CREATE TABLE \"{}\" ({}) STRICT",
            self.table,
            cols.join(", ")
        )
    }

    pub fn arity(&self) -> usize {
        self.columns.len()
    }

    pub fn column_names(&self) -> Vec<&str> {
        self.columns.iter().map(|c| c.name.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn orders() -> Schema {
        Schema {
            table: "orders".into(),
            columns: vec![
                Column {
                    name: "region".into(),
                    ty: ColumnType::Text,
                    nullable: true,
                },
                Column {
                    name: "amount".into(),
                    ty: ColumnType::Integer,
                    nullable: false,
                },
            ],
        }
    }

    #[test]
    fn create_table_sql_is_strict() {
        let sql = orders().create_table_sql();
        assert!(sql.contains("STRICT"), "spec §7.1 要求 STRICT table：{sql}");
        assert!(sql.contains("\"region\" TEXT"));
        assert!(sql.contains("\"amount\" INTEGER NOT NULL"));
    }

    #[test]
    fn arity_counts_columns() {
        assert_eq!(orders().arity(), 2);
    }
}
