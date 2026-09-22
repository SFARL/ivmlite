use std::collections::BTreeMap;

use crate::{Agg, AggFn, Row, Value, ZSet};

/// 一个 agg 的累加器。
///
/// `Sum` 必须同时维护 `sum` 与 `non_null`：spec §6.1 明写，只维护累加值的
/// 实现会在「组非空但该列全为 NULL」时输出 `0`，而 SQLite 输出 `NULL`，
/// 且这个不一致是静默的。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Acc {
    sum: i64,
    non_null: i64,
}

/// 一个 group 的状态。
#[derive(Debug, Clone)]
struct Group {
    /// 组内行的权重和。`COUNT(*)` 的输出即此值；归零时该组从输出中消失。
    rows: i64,
    accs: Vec<Acc>,
    /// 本组上一次对外发出过的行。spec §6.2：聚合必须记住自己发过什么
    /// 才能撤回它——这是聚合需要状态的真正原因。
    emitted: Option<Row>,
}

/// spec §6.2 的聚合算子状态。
///
/// `BTreeMap` 而非 `HashMap`：group 的遍历顺序进入 delta 流，而 spec §9.4
/// 要求失败用例能凭 seed 精确重放。
#[derive(Debug, Clone)]
pub struct AggState {
    group_by: Vec<usize>,
    aggs: Vec<Agg>,
    groups: BTreeMap<Row, Group>,
}

impl AggState {
    pub fn new(group_by: Vec<usize>, aggs: Vec<Agg>) -> AggState {
        AggState {
            group_by,
            aggs,
            groups: BTreeMap::new(),
        }
    }

    /// 吸收一批输入 delta，返回本算子**对外**发出的 delta。
    pub fn absorb(&mut self, input: &ZSet) -> ZSet {
        // 先把本批的全部变更并进组状态，记下哪些组被触及；发射统一在之后做。
        // 分两阶段是必要的：同一个组在一批里可能被多行触及，逐行发射会发出
        // 一串中间状态的 retraction 对，而对外只应看到本批的净变化。
        let mut touched: Vec<Row> = Vec::new();
        for (row, &w) in input.iter() {
            let key = Row::new(self.group_by.iter().map(|&c| row.get(c).clone()).collect());
            if !touched.contains(&key) {
                touched.push(key.clone());
            }
            let g = self.groups.entry(key).or_insert_with(|| Group {
                rows: 0,
                accs: vec![Acc::default(); self.aggs.len()],
                emitted: None,
            });
            g.rows += w;
            for (i, agg) in self.aggs.iter().enumerate() {
                if agg.func != AggFn::Sum {
                    continue;
                }
                let col = agg.column.expect("SUM 必须带列（lower 已校验）");
                if let Value::Int(v) = row.get(col) {
                    g.accs[i].sum += v * w;
                    g.accs[i].non_null += w;
                }
                // NULL 输入既不进 sum 也不进 non_null——这正是「全为 NULL 时
                // 输出 NULL」那条契约在状态层面的落点。
            }
        }

        let mut out = ZSet::new();
        // 发射顺序取自 group key 的排序而非 `touched` 的到达顺序（spec §9.4）。
        //
        // **这一行今天不可观察，实测确认**：`absorb` 的返回值是 `ZSet`，
        // 而 `ZSet` 内部是 `BTreeMap`——对不同的行调用 `update` 的先后
        // 与最终内容无关；两个不同的 group 又必然产生不同的输出行（输出行
        // 以 group key 开头）。删掉 `keys.sort()`、同时把 `groups` 换成
        // `HashMap`，全套测试连跑 12 个独立进程 12/12 全绿。留着它是因为
        // 一旦下游改成消费**有序的** delta 序列（而不是 `ZSet`），这个顺序
        // 立刻就进入输出——见 docs/mutation-gates.md 对应的「不适用」行。
        let mut keys: Vec<Row> = touched;
        keys.sort();
        for key in keys {
            let Some(g) = self.groups.get_mut(&key) else {
                continue;
            };
            let new_out = if g.rows > 0 {
                let mut vals: Vec<Value> = key.0.clone();
                for (i, agg) in self.aggs.iter().enumerate() {
                    vals.push(match agg.func {
                        AggFn::Count => Value::Int(g.rows),
                        AggFn::Sum => {
                            if g.accs[i].non_null == 0 {
                                Value::Null
                            } else {
                                Value::Int(g.accs[i].sum)
                            }
                        }
                    });
                }
                Some(Row::new(vals))
            } else {
                None
            };

            // `new_out == g.emitted` 时什么都不发。
            //
            // **这个判断今天也不可观察，实测确认**：把它改成恒真之后全套
            // 仍然全绿（148/148）。原因是输出未变时撤回与重发的是**同一行**，
            // `ZSet::update` 把 `-1` 与 `+1` 精确相消并删掉条目，多发的这一对
            // 在返回值里一点痕迹都不留。所以在当前形状下它是一处优化
            // （省掉两次 `BTreeMap` 操作），不是可证伪的语义——
            // 见 docs/mutation-gates.md 对应的「不适用」行。
            // 漏发（该发却不发）则完全是另一回事，由 `if let Some(old)`
            // 那条撤回守着，删掉它会让四个测试变红。
            if new_out != g.emitted {
                if let Some(old) = &g.emitted {
                    out.update(old.clone(), -1);
                }
                if let Some(new) = &new_out {
                    out.update(new.clone(), 1);
                }
                g.emitted = new_out;
            }

            // spec §5.1：组彻底空掉后不留僵尸状态。
            if g.rows == 0 && g.emitted.is_none() {
                self.groups.remove(&key);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AggFn, Value, ZSet};

    fn row(vals: Vec<Value>) -> Row {
        Row::new(vals)
    }

    fn txt(s: &str) -> Value {
        Value::Text(s.into())
    }

    fn int(i: i64) -> Value {
        Value::Int(i)
    }

    fn sum_state() -> AggState {
        // group key 是列 0，SUM 的是列 1
        AggState::new(
            vec![0],
            vec![Agg {
                func: AggFn::Sum,
                column: Some(1),
            }],
        )
    }

    fn count_state() -> AggState {
        AggState::new(
            vec![0],
            vec![Agg {
                func: AggFn::Count,
                column: None,
            }],
        )
    }

    #[test]
    fn a_changed_sum_emits_a_retraction_pair_not_a_bare_insert() {
        // spec §6.2：SUM 从 100 变 150 时发的是 (key,100) w=-1 与 (key,150) w=+1，
        // 不是单独一行 +1。这是 IVM 最大的 bug 来源。
        let mut s = sum_state();
        let first = s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(100)]), 1)]));
        assert_eq!(first, ZSet::from_rows([(row(vec![txt("a"), int(100)]), 1)]));

        let second = s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(50)]), 1)]));
        assert_eq!(
            second,
            ZSet::from_rows([
                (row(vec![txt("a"), int(100)]), -1),
                (row(vec![txt("a"), int(150)]), 1),
            ]),
            "必须撤回旧输出行并发出新行"
        );
    }

    #[test]
    fn an_unchanged_group_emits_nothing() {
        // 组被触及、但它的**输出**没变时，一对 (-1,+1) 也不该发。
        //
        // 输入必须是两条**不同的**行（一进一出），不能是同一行的 +1/-1：
        // 后者在 `ZSet::from_rows` 里就相消成空集了，`absorb` 根本不会看到
        // 任何输入，于是 `touched` 为空、发射循环一次都不执行——测试会通过，
        // 但通过的理由与它声称守护的东西无关。
        //
        // **这条测试守的不是 `new_out != emitted` 那个判断**（实测：把它改成
        // 恒真，本测试仍然绿——撤回与重发的是同一行，`ZSet::update` 精确相消）。
        // 它真正钉住的是 `COUNT(*)` 必须等于组内权重和：把 `g.rows += w` 改成
        // `g.rows += 1` 之后这批的行数会从 2 变成 4，输出随之改变，本测试变红。
        // 见 docs/mutation-gates.md 对应的两行。
        let mut s = count_state();
        s.absorb(&ZSet::from_rows([
            (row(vec![txt("a"), int(1)]), 1),
            (row(vec![txt("a"), int(2)]), 1),
        ]));
        // 换掉组内一行：行变了，但组的行数没变，于是 COUNT 的输出不变。
        let d = s.absorb(&ZSet::from_rows([
            (row(vec![txt("a"), int(3)]), 1),
            (row(vec![txt("a"), int(1)]), -1),
        ]));
        assert!(d.is_empty(), "组被触及但输出未变时不得发射：{d:?}");
    }

    #[test]
    fn a_group_that_empties_is_retracted_and_not_replaced() {
        // spec §5.2：分组聚合在空表时返回 0 行（与全局聚合不同）。
        // 组内计数归零时只发撤回，不发任何新行。
        let mut s = count_state();
        s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(1)]), 1)]));
        let d = s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(1)]), -1)]));
        assert_eq!(
            d,
            ZSet::from_rows([(row(vec![txt("a"), int(1)]), -1)]),
            "只撤回，不发新行"
        );
    }

    #[test]
    fn sum_over_only_null_inputs_is_null_not_zero() {
        // spec §6.1（已实测）：组非空但该列全为 NULL 时，组出现、COUNT(*) 为正、
        // 而 SUM 为 NULL。只维护累加值的实现会输出 0，与 SQLite 静默不一致。
        let mut s = sum_state();
        let d = s.absorb(&ZSet::from_rows([
            (row(vec![txt("a"), Value::Null]), 1),
            (row(vec![txt("a"), Value::Null]), 1),
        ]));
        assert_eq!(
            d,
            ZSet::from_rows([(row(vec![txt("a"), Value::Null]), 1)]),
            "SUM 必须是 NULL 而不是 Int(0)"
        );
    }

    #[test]
    fn sum_that_genuinely_totals_zero_is_int_zero_not_null() {
        // 与上一条相对：有非 NULL 输入、其和恰为 0 时必须是 Int(0)。
        // 只看「和是否为 0」的实现会在这里输出 NULL。
        let mut s = sum_state();
        let d = s.absorb(&ZSet::from_rows([
            (row(vec![txt("a"), int(5)]), 1),
            (row(vec![txt("a"), int(-5)]), 1),
        ]));
        assert_eq!(d, ZSet::from_rows([(row(vec![txt("a"), int(0)]), 1)]));
    }

    #[test]
    fn a_group_whose_last_non_null_input_leaves_falls_back_to_null() {
        // 非 NULL 输入被删光、但组仍非空时，SUM 必须从 Int 变回 NULL——
        // 这条路径只有同时维护 sum 与 non_null 计数才走得对。
        let mut s = sum_state();
        s.absorb(&ZSet::from_rows([
            (row(vec![txt("a"), int(5)]), 1),
            (row(vec![txt("a"), Value::Null]), 1),
        ]));
        let d = s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(5)]), -1)]));
        assert_eq!(
            d,
            ZSet::from_rows([
                (row(vec![txt("a"), int(5)]), -1),
                (row(vec![txt("a"), Value::Null]), 1),
            ]),
            "组还在（那行 NULL 仍在），但 SUM 退回 NULL"
        );
    }

    #[test]
    fn sum_scales_each_input_by_its_weight() {
        // SUM 必须按 `v * w` 累加，而不是忽略权重直接 `+= v`。
        //
        // 上面所有 SUM 测试的输入权重都是 ±1，`v * w` 与 `v` 在那里要么相等、
        // 要么被「non_null 归零 → 输出 NULL」这条规则掩盖掉，于是「忽略权重」
        // 这个变异在它们下面全部是绿的（实测：把 `+= v * w` 改成 `+= v`，
        // 全套 148/148 全绿）。这条测试专门补上那个缺口：权重 3 的一行必须
        // 贡献 15，撤回其中 1 份后必须降回 10——两步都在非 NULL 输入仍然存在
        // 的情况下发生，所以 NULL 那条规则掩盖不住。
        let mut s = sum_state();
        let first = s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(5)]), 3)]));
        assert_eq!(
            first,
            ZSet::from_rows([(row(vec![txt("a"), int(15)]), 1)]),
            "权重 3 的一行贡献 5*3=15，不是 5"
        );

        let second = s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(5)]), -1)]));
        assert_eq!(
            second,
            ZSet::from_rows([
                (row(vec![txt("a"), int(15)]), -1),
                (row(vec![txt("a"), int(10)]), 1),
            ]),
            "撤回 1 份后和从 15 降到 10；忽略权重的实现会升到 20"
        );
    }

    #[test]
    fn groups_are_independent() {
        let mut s = count_state();
        s.absorb(&ZSet::from_rows([
            (row(vec![txt("a"), int(1)]), 1),
            (row(vec![txt("b"), int(1)]), 1),
        ]));
        let d = s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(1)]), 1)]));
        assert_eq!(
            d,
            ZSet::from_rows([
                (row(vec![txt("a"), int(1)]), -1),
                (row(vec![txt("a"), int(2)]), 1),
            ]),
            "只有 a 组受影响，b 组不得出现在 delta 里"
        );
    }

    #[test]
    fn a_null_group_key_is_a_group_like_any_other() {
        // NULL 作为 group key 在 SQL GROUP BY 里自成一组（与 WHERE 的三值
        // 逻辑不同）。差分测试的值域 NULL 高频，这条路径一定会被走到。
        let mut s = count_state();
        let d = s.absorb(&ZSet::from_rows([(row(vec![Value::Null, int(1)]), 1)]));
        assert_eq!(d, ZSet::from_rows([(row(vec![Value::Null, int(1)]), 1)]));
    }

    #[test]
    fn emitted_output_weight_is_always_one() {
        // spec §5.2：group key → 恰好一个输出行，__w 在最终输出中恒为 1。
        // 权重只出现在内部 delta 与算子状态里。
        let mut s = count_state();
        let d = s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(1)]), 5)]));
        assert_eq!(
            d,
            ZSet::from_rows([(row(vec![txt("a"), int(5)]), 1)]),
            "输入权重 5 变成 COUNT=5 的一行，输出权重是 1 而不是 5"
        );
    }
}
