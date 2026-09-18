use ivmlite_core::{Row, Value};
use rand::rngs::StdRng;
use rand::RngExt;

use crate::{ColumnType, Schema};

/// 生成器的值域配置。
///
/// `distinct` 刻意很小：若某列有上百万个不同值，每个 group 只有一行，
/// 就永远测不到"同一个 group 反复增删"——而那正是 retraction 与僵尸行
/// bug 的产地（spec §9.2）。
#[derive(Debug, Clone)]
pub struct Domain {
    pub distinct: usize,
    pub null_rate: f64,
}

impl Default for Domain {
    fn default() -> Self {
        Domain {
            distinct: 8,
            null_rate: 0.2,
        }
    }
}

pub fn gen_row(rng: &mut StdRng, schema: &Schema, domain: &Domain) -> Row {
    let values = schema
        .columns
        .iter()
        .map(|col| {
            if col.nullable && rng.random_bool(domain.null_rate) {
                return Value::Null;
            }
            let n = rng.random_range(0..domain.distinct) as i64;
            match col.ty {
                ColumnType::Integer => Value::Int(n),
                ColumnType::Text => Value::Text(format!("v{n}")),
            }
        })
        .collect();
    Row::new(values)
}

pub fn gen_rows(rng: &mut StdRng, schema: &Schema, domain: &Domain, count: usize) -> Vec<Row> {
    (0..count).map(|_| gen_row(rng, schema, domain)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Column, ColumnType, Schema};
    use rand::SeedableRng;
    use std::collections::HashSet;

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
    fn domain_is_narrow_by_default() {
        let d = Domain::default();
        assert_eq!(
            d.distinct, 8,
            "窄值域是抓 retraction bug 的前提（spec §9.2）"
        );
    }

    /// spec §6.1：SQLite 的整数 SUM 溢出时报错，且是否报错取决于扫描顺序，
    /// 因此增量与全量重算会在溢出区分叉。生成器必须让溢出不可达。
    #[test]
    fn domain_cannot_overflow_integer_sum() {
        let d = Domain::default();
        let worst_case_sum = (d.distinct as i128) * 1_000_000;
        assert!(
            worst_case_sum < (1i128 << 62),
            "即使百万行全落在同一个 group，和也必须远小于 2^62"
        );
    }

    #[test]
    fn generated_values_stay_within_the_domain() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(1);
        let schema = orders();
        let domain = Domain::default();
        let rows = gen_rows(&mut rng, &schema, &domain, 500);

        let distinct_regions: HashSet<&Value> = rows.iter().map(|r| r.get(0)).collect();
        assert!(
            distinct_regions.len() <= domain.distinct + 1,
            "不同值数量必须受 domain 限制（+1 容纳 NULL），实得 {}",
            distinct_regions.len()
        );
    }

    #[test]
    fn nullable_columns_actually_produce_nulls() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(2);
        let rows = gen_rows(&mut rng, &orders(), &Domain::default(), 500);
        let nulls = rows.iter().filter(|r| r.get(0) == &Value::Null).count();
        assert!(
            nulls > 0,
            "NULL 在 GROUP BY 中自成一组，是经典 bug 点，必须高频出现"
        );
    }

    #[test]
    fn non_nullable_columns_never_produce_nulls() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(3);
        let rows = gen_rows(&mut rng, &orders(), &Domain::default(), 500);
        assert!(rows.iter().all(|r| r.get(1) != &Value::Null));
    }

    #[test]
    fn same_seed_yields_same_rows() {
        let schema = orders();
        let a = gen_rows(
            &mut rand::rngs::StdRng::seed_from_u64(7),
            &schema,
            &Domain::default(),
            20,
        );
        let b = gen_rows(
            &mut rand::rngs::StdRng::seed_from_u64(7),
            &schema,
            &Domain::default(),
            20,
        );
        assert_eq!(a, b);
    }
}
