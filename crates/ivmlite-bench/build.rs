//! Records the compiler that builds the runner, so run provenance names the toolchain that
//! produced the binary rather than whichever `rustc` is on `PATH` when it runs.
use std::process::Command;

fn main() {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let output = Command::new(&rustc)
        .arg("-vV")
        .output()
        .unwrap_or_else(|e| panic!("cannot run `{rustc} -vV`: {e}"));
    assert!(output.status.success(), "`{rustc} -vV` failed");
    let verbose = String::from_utf8(output.stdout).expect("`rustc -vV` is not UTF-8");
    // A build script directive is a single line, so the report's lines are joined with "; ".
    let single_line = verbose.lines().collect::<Vec<_>>().join("; ");
    println!("cargo:rustc-env=M2A_RUSTC_VV={single_line}");
    println!("cargo:rerun-if-env-changed=RUSTC");
    println!("cargo:rerun-if-changed=build.rs");
}
