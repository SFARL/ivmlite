//! Source-shaped FluxFlow aggregate demo and same-host benchmark.
//!
//! Usage:
//! `cargo run --release --locked -p ivmlite-test --example fluxflow_demo -- \
//!  [rows] [batch] [repeats] [--extension <path>]`
//!
//! See `docs/demos/fluxflow.md` for provenance and adaptations, and
//! `docs/demos/README.md` for the protocol, implemented once in
//! `ivmlite_test::demo_bench`.

use ivmlite_test::demo_bench::{self, take_extension_flag, Extension, Result, RunConfig};
use ivmlite_test::fluxflow::FLUXFLOW;

const USAGE: &str =
    "usage: fluxflow_demo [positive rows] [positive batch] [positive repeats] [--extension <path>]";

fn arguments(args: Vec<String>) -> Result<RunConfig> {
    let mut args = args.into_iter();
    let mut number = |default: usize| -> Result<usize> {
        match args.next() {
            None => Ok(default),
            Some(value) => value
                .parse()
                .map_err(|e| format!("{value:?}: {e}; {USAGE}").into()),
        }
    };
    let config = RunConfig {
        rows: number(250_000)?,
        batch: number(100)?,
        repeats: number(5)?,
    };
    if args.next().is_some() {
        return Err(USAGE.into());
    }
    Ok(config)
}

fn main() -> Result<()> {
    let (extension, args) = take_extension_flag(std::env::args().skip(1).collect())?;
    let config = arguments(args)?;
    let extension = Extension::resolve(extension)?;
    demo_bench::run(&FLUXFLOW, &config, &extension)
}
