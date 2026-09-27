//! The stale-preference shell around a real harness. `stale-world` writes the
//! fact world a harness driver lives; the driver
//! (`packages/e2e-tests/scripts/stale-preference.ts`) lives it through
//! OpenCode, the Eidnara plugin, and the daemon and captures the provider
//! request of every question turn; `stale-arms` turns that capture into the
//! export with the five M0 arms.

use std::path::{Path, PathBuf};

use eval_core::{StaleCapture, StaleExport, export_capture, fact_world};

use super::campaign::{SEED, parse_flags, prepare_publish, publish_file};

pub const WORLD_FILE: &str = "stale-world.json";
pub const EXPORT_FILE: &str = "stale-preference-export.json";

pub const WORLD_USAGE: &str = "stale-world --subjects <n> --publish <dir>";
pub const ARMS_USAGE: &str = "stale-arms --capture <file> --publish <dir>";

fn publish(dir: &Path, file: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    let refused =
        |(path, kind): (PathBuf, std::io::ErrorKind)| format!("{}: {kind}", path.display());
    prepare_publish(dir, &[file]).map_err(refused)?;
    let path = dir.join(file);
    publish_file(&path, bytes).map_err(refused)?;
    Ok(path)
}

/// Writes the fact world of `--subjects` subjects under the campaign's seed.
pub fn world(args: impl IntoIterator<Item = String>) -> Result<PathBuf, String> {
    let values = parse_flags(args, &["subjects", "publish"], WORLD_USAGE)?;
    let subjects = values["subjects"]
        .parse::<usize>()
        .map_err(|error| format!("--subjects: {error}"))?;
    if subjects == 0 || subjects > eval_core::MAX_SUBJECTS {
        return Err(format!(
            "--subjects must be 1..={}",
            eval_core::MAX_SUBJECTS
        ));
    }
    let world = fact_world(SEED, subjects);
    let bytes = serde_json::to_vec_pretty(&world).unwrap();
    publish(Path::new(&values["publish"]), WORLD_FILE, &bytes)
}

/// Reads a harness capture and writes its export.
pub fn arms(args: impl IntoIterator<Item = String>) -> Result<(PathBuf, StaleExport), String> {
    let values = parse_flags(args, &["capture", "publish"], ARMS_USAGE)?;
    let bytes = std::fs::read(&values["capture"])
        .map_err(|error| format!("--capture {}: {error}", values["capture"]))?;
    let capture: StaleCapture =
        serde_json::from_slice(&bytes).map_err(|error| format!("--capture: {error}"))?;
    let export = export_capture(&capture).map_err(|error| error.to_string())?;
    let bytes = serde_json::to_vec_pretty(&export).unwrap();
    let path = publish(Path::new(&values["publish"]), EXPORT_FILE, &bytes)?;
    Ok((path, export))
}
