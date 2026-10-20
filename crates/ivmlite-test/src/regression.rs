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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{gen_case, gen_database, Batching, Domain, Op};
    use ivmlite_core::{Row, Value};
    use rand::{RngExt, SeedableRng};

    #[test]
    fn real_regressions_round_trip_every_bit_including_infinities() {
        let mut case = gen_case(
            23,
            &gen_database(1),
            &Domain::default(),
            0,
            0,
            Batching::All,
        );
        let values = [f64::INFINITY, f64::NEG_INFINITY, -0.0, 0.1 + 0.2];
        let rows: Vec<_> = values
            .into_iter()
            .map(|x| Row::new(vec![Value::Null, Value::Null, Value::Real(x)]))
            .collect();
        case.initial.insert("t0".into(), rows.clone());
        case.ops = rows
            .iter()
            .cloned()
            .map(|row| ("t0".into(), Op::Delete(row)))
            .collect();
        let dir =
            std::env::temp_dir().join(format!("ivmlite-real-roundtrip-{}", std::process::id()));
        let path = save_regression(&dir, &case).unwrap();
        let loaded = load_regressions(&dir).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].initial, case.initial);
        assert_eq!(loaded[0].ops, case.ops);
        fs::remove_file(path).unwrap();
        fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn finite_real_json_round_trips_difficult_mantissas() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(23);
        for _ in 0..1024 {
            let x = f64::from_bits(rng.random());
            if x.is_finite() {
                let value = Value::Real(x);
                let json = serde_json::to_string(&value).unwrap();
                assert_eq!(
                    serde_json::from_str::<Value>(&json).unwrap(),
                    value,
                    "{json}"
                );
            }
        }
    }

    #[test]
    fn real_regression_json_keeps_finite_compatibility_and_rejects_nan() {
        assert_eq!(
            serde_json::from_str::<Value>(r#"{"Real":0.30000000000000004}"#).unwrap(),
            Value::Real(0.1 + 0.2)
        );
        assert_eq!(
            serde_json::to_string(&Value::Real(0.5)).unwrap(),
            r#"{"Real":0.5}"#
        );
        assert!(serde_json::to_string(&Value::Real(f64::NAN)).is_err());
        for json in [r#"{"Real":"NaN"}"#, r#"{"Real":null}"#] {
            assert!(serde_json::from_str::<Value>(json).is_err(), "{json}");
        }
    }
}
