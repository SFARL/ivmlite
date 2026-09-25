//! Small, source-backed demand demos. See docs/demos/README.md.
//! Uses the in-memory core with manually supplied deltas, not a SQLite extension.

use std::collections::BTreeMap;
use std::error::Error;

use ivmlite_core::{Row, Value, ZSet};
use ivmlite_test::{
    create_table_sql, recompute_via_sqlite, view_query_to_sql, Agg, AggFn, CmpOp, Column,
    ColumnType, Database, IncrementalEngine, Op, Predicate, Schema, ViewQuery,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

struct View {
    name: &'static str,
    query: ViewQuery,
    final_counts: ZSet,
}

struct Step {
    description: &'static str,
    operations: Vec<Op>,
}

struct Demo {
    name: &'static str,
    source: &'static str,
    schema: Schema,
    initial: Vec<Row>,
    views: Vec<View>,
    steps: Vec<Step>,
}

fn column(name: &str, ty: ColumnType) -> Column {
    Column {
        name: name.into(),
        ty,
        nullable: false,
    }
}

fn count_query(group_column: usize, predicate: Predicate) -> ViewQuery {
    ViewQuery {
        group_by: vec![group_column],
        aggs: vec![Agg {
            func: AggFn::Count,
            column: None,
        }],
        predicate,
        join: None,
    }
}

fn equal(column: usize, value: Value) -> Predicate {
    Predicate::Compare {
        column,
        op: CmpOp::Eq,
        value,
    }
}

fn counts(items: impl IntoIterator<Item = (Value, i64)>) -> ZSet {
    ZSet::from_rows(
        items
            .into_iter()
            .map(|(key, count)| (Row::new(vec![key, Value::Int(count)]), 1)),
    )
}

fn user(id: i64, member: i64, visible: i64) -> Row {
    Row::new(vec![
        Value::Int(id),
        Value::Int(member),
        Value::Int(visible),
    ])
}

fn members() -> Demo {
    const IS_MEMBER: usize = 1;
    const IS_VISIBLE: usize = 2;
    Demo {
        name: "members: visible member / non-member landing-page counters",
        source: "https://samuelplumppu.se/blog/using-sqlite-triggers-to-boost-performance-of-select-count",
        schema: Schema {
            table: "users".into(),
            columns: vec![
                column("id", ColumnType::Integer),
                column("is_member", ColumnType::Integer),
                column("is_visible", ColumnType::Integer),
            ],
        },
        initial: vec![user(1, 1, 1), user(2, 0, 1), user(3, 1, 0)],
        views: vec![View {
            name: "visible users by membership (0 = non-member, 1 = member)",
            query: count_query(IS_MEMBER, equal(IS_VISIBLE, Value::Int(1))),
            final_counts: counts([(Value::Int(1), 2)]),
        }],
        steps: vec![
            Step {
                description: "A visible member signs up",
                operations: vec![Op::Insert(user(4, 1, 1))],
            },
            Step {
                description: "A member opts out of public visibility",
                operations: vec![Op::Update {
                    old: user(1, 1, 1),
                    new: user(1, 1, 0),
                }],
            },
            Step {
                description: "The last visible non-member becomes a member",
                operations: vec![Op::Update {
                    old: user(2, 0, 1),
                    new: user(2, 1, 1),
                }],
            },
            Step {
                description: "One account is deleted while a hidden member becomes visible",
                operations: vec![
                    Op::Delete(user(4, 1, 1)),
                    Op::Update {
                        old: user(3, 1, 0),
                        new: user(3, 1, 1),
                    },
                ],
            },
        ],
    }
}

fn item(id: i64, category: &str, region: &str) -> Row {
    Row::new(vec![
        Value::Int(id),
        Value::Text(category.into()),
        Value::Text(region.into()),
    ])
}

fn text_counts(items: &[(&str, i64)]) -> ZSet {
    counts(
        items
            .iter()
            .map(|(key, n)| (Value::Text((*key).into()), *n)),
    )
}

fn facets() -> Demo {
    const CATEGORY: usize = 1;
    const REGION: usize = 2;
    Demo {
        name: "facets: three fixed sidebar counts over a changing catalog",
        source: "https://sqlite.org/forum/forumpost/c0e0fcbe36?hist=&t=c",
        schema: Schema {
            table: "catalog".into(),
            columns: vec![
                column("id", ColumnType::Integer),
                column("category", ColumnType::Text),
                column("region", ColumnType::Text),
            ],
        },
        initial: vec![
            item(1, "book", "EU"),
            item(2, "book", "US"),
            item(3, "tool", "EU"),
            item(4, "art", "US"),
        ],
        views: vec![
            View {
                name: "all categories",
                query: count_query(CATEGORY, Predicate::None),
                final_counts: text_counts(&[("book", 1), ("tool", 3)]),
            },
            View {
                name: "all regions",
                query: count_query(REGION, Predicate::None),
                final_counts: text_counts(&[("EU", 2), ("US", 2)]),
            },
            View {
                name: "categories for the fixed EU filter",
                query: count_query(CATEGORY, equal(REGION, Value::Text("EU".into()))),
                final_counts: text_counts(&[("book", 1), ("tool", 1)]),
            },
        ],
        steps: vec![
            Step {
                description: "A tool is added to the EU catalog",
                operations: vec![Op::Insert(item(5, "tool", "EU"))],
            },
            Step {
                description: "An item is recategorized from book to tool",
                operations: vec![Op::Update {
                    old: item(2, "book", "US"),
                    new: item(2, "tool", "US"),
                }],
            },
            Step {
                description: "A tool moves from EU to US: the filtered count must retract it",
                operations: vec![Op::Update {
                    old: item(3, "tool", "EU"),
                    new: item(3, "tool", "US"),
                }],
            },
            Step {
                description: "The last art item is deleted: its facet bucket disappears",
                operations: vec![Op::Delete(item(4, "art", "US"))],
            },
        ],
    }
}

fn show_counts(name: &str, result: &ZSet) {
    let display: Vec<String> = result
        .iter()
        .map(|(row, weight)| {
            assert_eq!(*weight, 1, "an aggregate row must have weight one");
            format!(
                "{}: {}",
                display_value(row.get(0)),
                display_value(row.get(1))
            )
        })
        .collect();
    println!("  {name}: {}", display.join(", "));
}

fn display_value(value: &Value) -> String {
    match value {
        Value::Int(n) => n.to_string(),
        Value::Text(text) => text.clone(),
        Value::Null => "NULL".into(),
    }
}

fn member_cards(result: &ZSet) -> [i64; 2] {
    let mut cards = [0, 0];
    for (row, _) in result.iter() {
        match row.0.as_slice() {
            [Value::Int(member @ 0..=1), Value::Int(count)] => {
                cards[*member as usize] = *count;
            }
            _ => panic!("unexpected membership result"),
        }
    }
    cards
}

fn verify(
    db: &Database,
    bases: &BTreeMap<String, ZSet>,
    views: &[View],
    engines: &[IncrementalEngine],
) -> Result<()> {
    for (view, engine) in views.iter().zip(engines) {
        let actual = engine.snapshot();
        let expected = recompute_via_sqlite(db, &view.query, bases)?;
        assert_eq!(actual, expected, "SQLite mismatch for {}", view.name);
        show_counts(view.name, &actual);
    }
    println!("  CHECK: every view equals SQLite full recomputation");
    Ok(())
}

fn run(demo: Demo) -> Result<Vec<ZSet>> {
    println!("\n{}\nSource: {}", demo.name, demo.source);
    println!("{};", create_table_sql(&demo.schema));
    let db = Database::single(demo.schema.clone());
    let mut bases = BTreeMap::from([(
        demo.schema.table.clone(),
        ZSet::from_rows(demo.initial.into_iter().map(|row| (row, 1))),
    )]);
    let mut engines = Vec::new();
    for view in &demo.views {
        println!("{};", view_query_to_sql(&view.query, &db));
        let mut engine = IncrementalEngine::new();
        engine.create_view(&db, &view.query, &bases)?;
        engines.push(engine);
    }
    println!("Bootstrap:");
    verify(&db, &bases, &demo.views, &engines)?;

    for step in demo.steps {
        println!("\n{}", step.description);
        let before: Vec<ZSet> = engines.iter().map(IncrementalEngine::snapshot).collect();
        for operation in step.operations {
            let delta = operation.to_delta();
            let base = bases.get_mut(&demo.schema.table).unwrap();
            for (row, weight) in &delta {
                assert!(base.weight_of(row) + weight >= 0, "illegal demo deletion");
                base.update(row.clone(), *weight);
            }
            for engine in &mut engines {
                engine.apply(&demo.schema.table, &delta)?;
            }
        }
        for (engine, previous) in engines.iter_mut().zip(&before) {
            assert_eq!(&engine.snapshot(), previous, "apply must wait for refresh");
            engine.refresh()?;
        }
        println!("  Before refresh: previous snapshot retained. After explicit refresh:");
        verify(&db, &bases, &demo.views, &engines)?;
        for engine in &mut engines {
            let refreshed = engine.snapshot();
            engine.refresh()?;
            assert_eq!(
                engine.snapshot(),
                refreshed,
                "a second refresh changed counts"
            );
        }
    }
    for (view, engine) in demo.views.iter().zip(&engines) {
        assert_eq!(
            engine.snapshot(),
            view.final_counts,
            "unexpected final story outcome"
        );
    }
    println!("PASS: final story outcome and repeated-refresh checks");
    Ok(engines.iter().map(IncrementalEngine::snapshot).collect())
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let selected = args.next().unwrap_or_else(|| "all".into());
    if args.next().is_some() || !matches!(selected.as_str(), "all" | "members" | "facets") {
        return Err(
            "usage: cargo run -p ivmlite-test --example demand_demos -- [all|members|facets]"
                .into(),
        );
    }
    println!("CORE DEMOS: synthetic records inspired by documented demand.");
    println!("Deltas are supplied manually; SQLite is the correctness oracle.");
    println!("No extension, persistence, UI integration, or performance claim is demonstrated.");
    if selected == "all" || selected == "members" {
        let results = run(members())?;
        let [non_members, members] = member_cards(&results[0]);
        assert_eq!([non_members, members], [0, 2]);
        println!("Landing-page rendering must fill an absent membership bucket with zero.");
        println!("The final cards are: members = {members}, non-members = {non_members}.");
    }
    if selected == "all" || selected == "facets" {
        run(facets())?;
    }
    Ok(())
}
