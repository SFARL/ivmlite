//! Same-host benchmark for the noop, Zcash and Kener demand-shaped demos.
//!
//! Usage:
//! `cargo run --release --locked -p ivmlite-test --example demand_case_bench -- \
//!  <case> [rows] [batch] [repeats] [--extension <path>]`
//!
//! The protocol, modes and columns are documented in `docs/demos/README.md`
//! and implemented once in `ivmlite_test::demo_bench`.

use ivmlite_test::demand_cases::{by_name, DemandCase, ALL};
use ivmlite_test::demo_bench::{self, take_extension_flag, Extension, Result, RunConfig};

fn usage() -> String {
    let names = ALL
        .iter()
        .map(|case| case.name)
        .collect::<Vec<_>>()
        .join("|");
    format!(
        "usage: demand_case_bench <{names}> [positive rows] [positive batch] \
         [positive repeats] [--extension <path>]"
    )
}

fn arguments(args: Vec<String>) -> Result<(&'static DemandCase, RunConfig)> {
    let mut args = args.into_iter();
    let name = args.next().ok_or_else(usage)?;
    let case = by_name(&name).ok_or_else(usage)?;
    let mut number = |default: usize| -> Result<usize> {
        match args.next() {
            None => Ok(default),
            Some(value) => value
                .parse()
                .map_err(|e| format!("{value:?}: {e}; {}", usage()).into()),
        }
    };
    let config = RunConfig {
        rows: number(case.default_rows)?,
        batch: number(case.default_batch)?,
        repeats: number(5)?,
    };
    if args.next().is_some() {
        return Err(usage().into());
    }
    Ok((case, config))
}

fn main() -> Result<()> {
    let (extension, args) = take_extension_flag(std::env::args().skip(1).collect())?;
    let (case, config) = arguments(args)?;
    let extension = Extension::resolve(extension)?;
    demo_bench::run(case, &config, &extension)
}
