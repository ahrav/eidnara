//! Scale driver support. `scale-seed` writes #826's synthetic history into the store a
//! direct-host fixture opens at its state root, so a driver's first pass declares coverage that
//! already exists; `scale-report` builds an `eval-scale-report/v1` report from a driver's rows
//! and manifest.

use std::fs;
use std::io;
use std::path::PathBuf;

use super::campaign::parse_flags;
use daemon::synthetic_history::SyntheticHistory;
use eval_core::{
    ArtifactIdentity, DriverIdentity, HostManifest, OpenLoopCounts, ScaleInputs, ScaleReport,
    TierRatio, parse_pass_row,
};
use memory_store::MemoryStore;
use serde::Deserialize;
use serde_json::{Value, json};

const SEED_USAGE: &str =
    "usage: eval_runner scale-seed --state-root <dir> --session <id> --segments <n>";
const REPORT_USAGE: &str =
    "usage: eval_runner scale-report --rows <jsonl> --manifest <json> --out <report.json>";

/// Seeds `--segments` synthetic history segments of two messages each into `--session` of the
/// fixture store under `--state-root`, with the ordinal continuation base the newest segment
/// ends on and that segment's end as the rendered boundary, so `transform.boundary` pages
/// anchors before any pass ran, and prints the anchor the first pass declares.
pub fn run_seed(args: impl Iterator<Item = String>) -> io::Result<()> {
    let values = parse_flags(args, &["state-root", "session", "segments"], SEED_USAGE)
        .map_err(io::Error::other)?;
    let root = PathBuf::from(&values["state-root"]);
    let session = values["session"].as_str();
    let segments: usize = values["segments"]
        .parse()
        .map_err(|_| io::Error::other("--segments must be a count"))?;
    if segments == 0 {
        return Err(io::Error::other("--segments must be positive"));
    }
    fs::create_dir_all(&root)?;
    let descriptor = daemon::managed_store_descriptor(&root).map_err(io::Error::other)?;
    let store = MemoryStore::open(&descriptor).map_err(io::Error::other)?;
    let history = SyntheticHistory::mixed(segments);
    history.seed(&store, session);
    let end = history.span * segments as i64;
    let loaded = store.load(session).map_err(io::Error::other)?;
    let mut core = loaded.core.clone();
    core.boundary_id = format!("m{end}#0");
    let mut meta = loaded.meta.clone();
    meta.ordinal_continuation_base = Some((end - 2) as u64);
    meta.coverage_ordinal = Some(end as u64);
    store
        .commit(session, None, &core, &meta)
        .map_err(io::Error::other)?;
    println!(
        "{}",
        json!({
            "session": session,
            "segments": segments,
            "end_message": end,
            "anchor": { "mid": format!("m{end}"), "sequence": segments },
        })
    );
    Ok(())
}

/// What a driver records beside its rows.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    driver: DriverIdentity,
    artifact: ArtifactIdentity,
    host: HostManifest,
    open_loop: Option<OpenLoopCounts>,
    seed: u64,
}

/// Reads `--rows` (one pass row per line) and `--manifest`, writes the canonical report to
/// `--out`, and prints each tier's per-state percentiles and ratio verdict.
pub fn run_report(args: impl Iterator<Item = String>) -> io::Result<()> {
    let values =
        parse_flags(args, &["rows", "manifest", "out"], REPORT_USAGE).map_err(io::Error::other)?;
    let manifest: Manifest =
        serde_json::from_slice(&fs::read(&values["manifest"])?).map_err(io::Error::other)?;
    let text = fs::read_to_string(&values["rows"])?;
    let mut rows = Vec::new();
    for (index, line) in text
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.is_empty())
    {
        let value: Value = serde_json::from_str(line)
            .map_err(|error| io::Error::other(format!("row {index}: {error}")))?;
        rows.push(
            parse_pass_row(&value)
                .map_err(|error| io::Error::other(format!("row {index}: {error}")))?,
        );
    }
    let report = ScaleReport::build(ScaleInputs {
        driver: manifest.driver,
        artifact: manifest.artifact,
        host: manifest.host,
        open_loop: manifest.open_loop,
        seed: manifest.seed,
        rows,
    })
    .map_err(io::Error::other)?;
    let value = report.serialize().map_err(io::Error::other)?;
    fs::write(&values["out"], serde_json::to_vec(&value)?)?;
    let tiers: Vec<Value> = report
        .tiers
        .iter()
        .map(|tier| {
            let states: Vec<Value> = tier
                .states
                .iter()
                .map(|state| {
                    json!({
                        "state": state.state,
                        "passes": state.passes,
                        "response_us": state.response_percentiles,
                        "service_us": state.service_percentiles,
                    })
                })
                .collect();
            json!({
                "harness": tier.harness,
                "tier": tier.tier,
                "sessions": tier.sessions.len(),
                "states": states,
                "refusals": tier.refusals,
                "ratio": match &tier.ratio {
                    TierRatio::Computed(claim) => json!({
                        "upper": claim.upper,
                        "passes": claim.passes,
                    }),
                    other => serde_json::to_value(other).unwrap_or(Value::Null),
                },
            })
        })
        .collect();
    let digest = ScaleReport::result_digest(&value).map_err(io::Error::other)?;
    println!("{}", json!({ "result_digest": digest, "tiers": tiers }));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> impl Iterator<Item = String> {
        list.iter()
            .map(|arg| arg.to_string())
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn scale_seed_writes_history_the_continuation_base_and_the_rendered_boundary() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().to_str().unwrap();
        run_seed(args(&[
            "--state-root",
            path,
            "--session",
            "ses",
            "--segments",
            "3",
        ]))
        .unwrap();
        let store =
            MemoryStore::open(&daemon::managed_store_descriptor(root.path()).unwrap()).unwrap();
        assert_eq!(store.max_history_segment_end_ordinal("ses").unwrap(), 6);
        let loaded = store.load("ses").unwrap();
        assert_eq!(loaded.meta.ordinal_continuation_base, Some(4));
        assert_eq!(loaded.core.boundary_id, "m6#0");
        assert_eq!(loaded.meta.coverage_ordinal, Some(6));
        let anchors: Vec<(i64, String)> = store.coverage_anchor_page("ses", 0..=10, 8).unwrap();
        assert_eq!(anchors.first(), Some(&(3, "m6#0".to_string())));
        assert!(run_seed(args(&["--state-root", path, "--session", "ses"])).is_err());
    }

    #[test]
    fn scale_report_builds_a_parseable_report_from_the_writer_fixture() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join("manifest.json");
        fs::write(
            &manifest,
            serde_json::to_vec(&json!({
                "driver": { "name": "fixture", "budget_seconds": 1 },
                "artifact": {
                    "commit": "0123456789abcdef0123456789abcdef01234567",
                    "bun_version": "1.3.14",
                    "daemon_build": "debug",
                },
                "host": {
                    "cpu_model": "fixture", "core_count": 4, "memory_bytes": 1,
                    "kernel": "k", "glibc": "g", "disk": "ssd",
                },
                "open_loop": null,
                "seed": 1,
            }))
            .unwrap(),
        )
        .unwrap();
        let rows = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../eval-core/testdata/scale/writer-rows.jsonl"
        );
        let out = dir.path().join("report.json");
        run_report(args(&[
            "--rows",
            rows,
            "--manifest",
            manifest.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ]))
        .unwrap();
        let value: Value = serde_json::from_slice(&fs::read(&out).unwrap()).unwrap();
        let report = eval_core::parse_scale_report(&value).unwrap();
        assert_eq!(report.rows.len(), 6);
    }
}
