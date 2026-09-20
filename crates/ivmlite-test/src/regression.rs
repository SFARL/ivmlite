use std::fs;
use std::path::{Path, PathBuf};

use crate::TestCase;

/// 把一个最小失败用例写进回归目录。文件名用 seed + 操作数，重复运行会覆盖
/// 同一个文件而不是堆积——同一个 seed 缩出来的最小用例应当是确定的。
pub fn save_regression(dir: &Path, case: &TestCase) -> std::io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let path = dir.join(format!("seed{}-ops{}.json", case.seed, case.ops.len()));
    let json = serde_json::to_string_pretty(case)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    fs::write(&path, json)?;
    Ok(path)
}

/// 读出回归目录下的全部用例。目录不存在时返回空 vec——首次运行尚未固化
/// 任何用例是正常状态，不是错误。
pub fn load_regressions(dir: &Path) -> std::io::Result<Vec<TestCase>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut cases = Vec::new();
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    paths.sort(); // 顺序确定，便于复现

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
