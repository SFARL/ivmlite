use std::collections::BTreeMap;

use crate::Row;

/// spec §6.3。key → 多个 (value, weight)。
///
/// `get` 返回迭代器而非 `Option`：v0 的 group-by 每个 key 只存一个值，用不上
/// 多值，但 join 的每一侧都是 key → 多行（§6.3 明写这是 M0 就不许做出会让
/// join 返工的决定的主要落点）。
///
/// 用 `Box<dyn Iterator>` 而非 RPITIT 是为了 object-safety：算子需要持有
/// `dyn Arrangement`（M1b 的实现来自 `ivmlite-sqlite`），RPITIT 会让该 trait
/// 不是 object-safe，迫使类型参数在整个算子树上传播。
pub trait Arrangement {
    fn get(&self, key: &Row) -> Box<dyn Iterator<Item = (Row, i64)> + '_>;
    fn update(&mut self, key: &Row, val: &Row, weight_delta: i64);
    fn scan(&self) -> Box<dyn Iterator<Item = (Row, Row, i64)> + '_>;
}

/// M1a 的内存实现。M1b 会另加一个走 SQLite shadow table 的实现。
///
/// 两层都用 `BTreeMap`：spec §9.4 要求任何可能影响输出的迭代顺序都确定，
/// 而 `scan()` 的顺序直接进 delta 流。
#[derive(Debug, Clone, Default)]
pub struct MemArrangement {
    inner: BTreeMap<Row, BTreeMap<Row, i64>>,
}

impl MemArrangement {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Arrangement for MemArrangement {
    fn get(&self, key: &Row) -> Box<dyn Iterator<Item = (Row, i64)> + '_> {
        match self.inner.get(key) {
            Some(vals) => Box::new(vals.iter().map(|(v, &w)| (v.clone(), w))),
            None => Box::new(std::iter::empty()),
        }
    }

    fn update(&mut self, key: &Row, val: &Row, weight_delta: i64) {
        if weight_delta == 0 {
            return;
        }
        let vals = self.inner.entry(key.clone()).or_default();
        let w = vals.entry(val.clone()).or_insert(0);
        *w += weight_delta;
        // spec §5.1：归零即删，不留僵尸条目。key 空掉后连 key 一起删——这
        // 本身不会让 scan() 的输出变大：scan() 用 flat_map 展开每个 key 的
        // 值集合，一个空集合天然贡献零条记录，不管外层 key 是否还留在
        // `inner` 里。真正会随历史（每个用过又清空的 key）而非当前状态
        // 单调增长的是 `inner` 自身的条目数——这是内存足迹问题，不是输出
        // 大小问题，但同样值得删：否则 `inner` 会无限累积永不再被访问的
        // 空壳，`scan()` 的遍历成本也随之只增不减。
        if *w == 0 {
            vals.remove(val);
            if vals.is_empty() {
                self.inner.remove(key);
            }
        }
    }

    fn scan(&self) -> Box<dyn Iterator<Item = (Row, Row, i64)> + '_> {
        Box::new(
            self.inner
                .iter()
                .flat_map(|(k, vals)| vals.iter().map(move |(v, &w)| (k.clone(), v.clone(), w))),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Value;

    fn r(vals: Vec<i64>) -> Row {
        Row::new(vals.into_iter().map(Value::Int).collect())
    }

    #[test]
    fn one_key_can_hold_multiple_values() {
        // spec §6.3：get 返回迭代器而非 Option，因为 join 的每一侧都是
        // key → 多行。v0 的 group-by 用不上，但这个形状现在就必须成立，
        // 否则 Phase 3 要改的是整个算子树的签名。
        let mut a = MemArrangement::new();
        a.update(&r(vec![1]), &r(vec![10]), 1);
        a.update(&r(vec![1]), &r(vec![20]), 3);
        let mut got: Vec<(Row, i64)> = a.get(&r(vec![1])).collect();
        got.sort();
        assert_eq!(got, vec![(r(vec![10]), 1), (r(vec![20]), 3)]);
    }

    #[test]
    fn weights_accumulate_and_zero_removes_the_entry() {
        // spec §5.1：权重归零的行必须删除，不留僵尸条目。
        let mut a = MemArrangement::new();
        a.update(&r(vec![1]), &r(vec![10]), 2);
        a.update(&r(vec![1]), &r(vec![10]), -2);
        assert_eq!(a.get(&r(vec![1])).count(), 0, "归零后不得留下条目");
        assert_eq!(a.scan().count(), 0, "scan 也不得看到它");
    }

    #[test]
    fn a_key_with_no_values_left_disappears_from_scan() {
        // 仅删掉 (key,val) 还不够——key 本身也不能留成空壳，
        // 否则 scan 的行数会随历史增长而不随当前状态。
        let mut a = MemArrangement::new();
        a.update(&r(vec![1]), &r(vec![10]), 1);
        a.update(&r(vec![2]), &r(vec![20]), 1);
        a.update(&r(vec![1]), &r(vec![10]), -1);
        let keys: Vec<Row> = a.scan().map(|(k, _, _)| k).collect();
        assert_eq!(keys, vec![r(vec![2])], "空掉的 key 必须消失");
        // 白盒检查：仅删掉 `if vals.is_empty() { self.inner.remove(key); }`
        // 这一行时，上面的 scan() 断言其实不会红——scan() 的 flat_map 对着
        // 一个空的内层 BTreeMap 天然产出零条记录，不管外层 key 是否还在
        // `inner` 里。真正会随历史增长的是 `inner` 自身的条目数（每个用过又
        // 清空的 key 都会作为空壳永久占位），这一点只能直接查内部状态。
        assert!(
            !a.inner.contains_key(&r(vec![1])),
            "清空的 key 不能作为空壳留在 inner 里"
        );
    }

    #[test]
    fn negative_weights_are_representable() {
        // 中间 delta 里负权重合法（spec §5.1）——只有最终物化结果里不该有。
        let mut a = MemArrangement::new();
        a.update(&r(vec![1]), &r(vec![10]), -5);
        assert_eq!(
            a.get(&r(vec![1])).collect::<Vec<_>>(),
            vec![(r(vec![10]), -5)]
        );
    }

    /// spec §9.4：失败用例要凭 seed 精确重放，任何可能影响输出的迭代都必须
    /// 来自有序容器。
    ///
    /// 这仍然只是一条统计意义上的守卫，不是绝对证明：`MemArrangement` 内部
    /// 若把外层 `BTreeMap` 换成 `HashMap`，从这个类型外部没有办法把"顺序
    /// 确定"这件事钉死成必然——`HashMap` 的 `RandomState` 逐次构造重新
    /// 播种，每次运行都可能凑巧给出正确顺序。3 个 key 凑巧排对的概率是
    /// 1/3! ≈ 16.7%，跑几次独立进程就能把巧合筛掉（见
    /// `docs/mutation-gates.md` 对应行：连续跑了 15 次独立进程全部变红）。
    /// 插入顺序 `[3, 1, 2]` 特意选了非字典序（也非字典序的反序），这样一个
    /// "看似保序、实则在内部按值排序"的实现同样会被测出来。
    #[test]
    fn scan_order_is_deterministic() {
        let build = || {
            let mut a = MemArrangement::new();
            for k in [3, 1, 2] {
                for v in [30, 10, 20] {
                    a.update(&r(vec![k]), &r(vec![v]), 1);
                }
            }
            a.scan().collect::<Vec<_>>()
        };
        assert_eq!(build(), build());
        let keys: Vec<Row> = build().into_iter().map(|(k, _, _)| k).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted, "scan 必须按 key 有序");
    }

    #[test]
    fn scan_order_is_independent_of_update_history() {
        // spec §9.4：`scan_order_is_deterministic` 只检查"同一条代码路径
        // 重放两次自洽"和"key 有序"，测不出一件更根本的事——两个
        // `MemArrangement`，用不同的 update 顺序（甚至走过一段插入又撤回的
        // 弯路）到达完全相同的最终状态，`scan()` 是否给出逐元素相同的输出。
        // 如果不是，delta 流就不可能只凭最终状态和 seed 精确重放，因为它
        // 还偷偷依赖了"怎么走到这个状态"。
        let ascending = {
            let mut a = MemArrangement::new();
            a.update(&r(vec![1]), &r(vec![10]), 1);
            a.update(&r(vec![1]), &r(vec![20]), 1);
            a.update(&r(vec![1]), &r(vec![30]), 1);
            a.scan().collect::<Vec<_>>()
        };
        let descending_with_a_detour = {
            let mut a = MemArrangement::new();
            a.update(&r(vec![1]), &r(vec![30]), 1);
            // 插入又撤回一个不相关的值：确保两条路径不只是插入序列的反转，
            // 历史也真正不同（内部容器如果按插入顺序追加，撤回这一步会在
            // 中间留下/腾出一个位置，进一步偏离"只是顺序颠倒"这种简单情形）。
            a.update(&r(vec![1]), &r(vec![5]), 1);
            a.update(&r(vec![1]), &r(vec![5]), -1);
            a.update(&r(vec![1]), &r(vec![20]), 1);
            a.update(&r(vec![1]), &r(vec![10]), 1);
            a.scan().collect::<Vec<_>>()
        };
        assert_eq!(
            ascending, descending_with_a_detour,
            "scan() 的输出不得依赖到达该状态所走的路径"
        );
    }

    #[test]
    fn get_on_a_missing_key_is_empty_not_a_panic() {
        let a = MemArrangement::new();
        assert_eq!(a.get(&r(vec![99])).count(), 0);
    }

    #[test]
    fn mem_arrangement_is_usable_as_a_trait_object() {
        // spec §6.3 实现注记：trait 必须 object-safe，否则算子树上会被迫
        // 传播类型参数。这个测试就是那条约束的编译期门禁。
        let mut a: Box<dyn Arrangement> = Box::new(MemArrangement::new());
        a.update(&r(vec![1]), &r(vec![10]), 1);
        assert_eq!(a.get(&r(vec![1])).count(), 1);
    }
}
