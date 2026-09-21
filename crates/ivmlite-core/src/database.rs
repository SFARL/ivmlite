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

    /// 顺序必须保留——失败用例要凭 seed 精确重放（spec §9.4）。
    ///
    /// 这仍然只是一条统计意义上的守卫，不是绝对证明：`Database` 内部若
    /// 换成按名字路由的 `HashMap`，从这个类型外部没有办法把"顺序确定"
    /// 这件事钉死成必然——`HashMap` 的 `RandomState` 逐进程重新播种，
    /// 每次运行都可能凑巧给出插入顺序。两张表时凑巧排对的概率是
    /// 1/2! = 50%，测试形同虚设；五张互不相同的表把这个概率压到
    /// 1/5! ≈ 0.83%，多跑几次就能把巧合筛掉。插入顺序特意选了非字典序
    /// （也非字典序的反序），这样一个"看似保序、实则在内部按名字排序"
    /// 的实现同样会被测出来。
    #[test]
    fn table_order_is_preserved() {
        let db = Database::new(vec![s("c"), s("a"), s("e"), s("b"), s("d")]);
        let names: Vec<&str> = db.tables().iter().map(|t| t.table.as_str()).collect();
        assert_eq!(
            names,
            vec!["c", "a", "e", "b", "d"],
            "顺序必须保留——失败用例要凭 seed 精确重放"
        );
    }
}
