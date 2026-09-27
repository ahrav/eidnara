//! The stale-preference shell around a real harness. `stale-world` writes the
//! fact world a harness driver lives; the driver
//! (`packages/e2e-tests/scripts/stale-preference.ts`) lives it through
//! OpenCode, the Eidnara plugin, and the daemon and captures the provider
//! request of every question turn; `stale-arms` turns that capture into the
//! export with the five M0 arms.

use std::path::{Path, PathBuf};

use eval_core::{StaleCapture, StaleExport, export_capture, fact_world};

use super::campaign::{parse_flags, prepare_publish, publish_file};

pub const WORLD_FILE: &str = "stale-world.json";
pub const EXPORT_FILE: &str = "stale-preference-export.json";

pub const WORLD_USAGE: &str = "stale-world --subjects <n> --seed <u64> --publish <dir>";
pub const ARMS_USAGE: &str = "stale-arms --capture <file> --publish <dir>";
pub const MERGE_USAGE: &str = "stale-merge --inputs <file,...> --publish <dir>";

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
    let values = parse_flags(args, &["subjects", "seed", "publish"], WORLD_USAGE)?;
    let subjects = values["subjects"]
        .parse::<usize>()
        .map_err(|error| format!("--subjects: {error}"))?;
    if subjects == 0 || subjects > eval_core::MAX_SUBJECTS {
        return Err(format!(
            "--subjects must be 1..={}",
            eval_core::MAX_SUBJECTS
        ));
    }
    let seed = values["seed"]
        .parse::<u64>()
        .map_err(|error| format!("--seed: {error}"))?;
    let world = fact_world(seed, subjects);
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

/// Merges independent harness exports. Pair and unlocatable task ids are
/// prefixed by each input's one-based position, so equal per-world ids cannot
/// collide. Every input must name one schema, harness, and summarizer.
pub fn merge(args: impl IntoIterator<Item = String>) -> Result<(PathBuf, StaleExport), String> {
    let values = parse_flags(args, &["inputs", "publish"], MERGE_USAGE)?;
    let mut inputs = values["inputs"].split(',');
    let first = inputs
        .next()
        .filter(|p| !p.is_empty())
        .ok_or("--inputs is empty")?;
    let read = |path: &str| -> Result<StaleExport, String> {
        let bytes = std::fs::read(path).map_err(|error| format!("--inputs {path}: {error}"))?;
        serde_json::from_slice(&bytes).map_err(|error| format!("--inputs {path}: {error}"))
    };
    let mut merged = read(first)?;
    let prefix = |world: usize, task: &str| format!("world-{world}:{task}");
    for pair in &mut merged.pairs {
        pair.task = prefix(1, &pair.task);
    }
    merged.unlocatable = merged
        .unlocatable
        .into_iter()
        .map(|(task, value)| (prefix(1, &task), value))
        .collect();
    for (index, path) in inputs.enumerate() {
        let mut next = read(path)?;
        if (
            next.schema.as_str(),
            next.harness.as_str(),
            next.summarizer.as_str(),
        ) != (
            merged.schema.as_str(),
            merged.harness.as_str(),
            merged.summarizer.as_str(),
        ) {
            return Err(format!(
                "--inputs {path}: schema, harness, or summarizer mismatch"
            ));
        }
        for pair in &mut next.pairs {
            pair.task = prefix(index + 2, &pair.task);
        }
        merged.pairs.extend(next.pairs);
        merged.unlocatable.extend(
            next.unlocatable
                .into_iter()
                .map(|(task, value)| (prefix(index + 2, &task), value)),
        );
        merged.stale_delivered += next.stale_delivered;
        merged.root_seed.push(',');
        merged.root_seed.push_str(&next.root_seed);
    }
    let bytes = serde_json::to_vec_pretty(&merged).unwrap();
    let path = publish(Path::new(&values["publish"]), EXPORT_FILE, &bytes)?;
    Ok((path, merged))
}
