use std::fs;
use std::path::{Path, PathBuf};

use crate::TestCase;

/// Write a minimal failing case into the regressions directory. The file name
/// is seed + operation count, so repeated runs overwrite the same file instead
/// of piling up — the minimal case shrunk from a given seed should be
/// deterministic.
pub fn save_regression(dir: &Path, case: &TestCase) -> std::io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let path = dir.join(format!("seed{}-ops{}.json", case.seed, case.ops.len()));
    let json = serde_json::to_string_pretty(case)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    fs::write(&path, json)?;
    Ok(path)
}

/// Read every case in the regressions directory. A missing directory returns
/// an empty vec — on a first run no case has been frozen yet, which is normal,
/// not an error.
pub fn load_regressions(dir: &Path) -> std::io::Result<Vec<TestCase>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut cases = Vec::new();
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    paths.sort(); // deterministic order, for reproducibility

    for path in paths {
        let text = fs::read_to_string(&path)?;
        let case: TestCase = serde_json::from_str(&text).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{}: {e}", path.display()),
            )
        })?;
        cases.push(case);
    }
    Ok(cases)
}
