//! Scale driver support. `scale-seed` writes #826's synthetic history into the store a
//! direct-host fixture opens at its state root, so a driver's first pass declares coverage that
//! already exists; `scale-report` builds an `eval-scale-report/v1` report from a driver's rows
//! and manifest.

use std::fs;
use std::io;
use std::path::PathBuf;

use daemon::synthetic_history::SyntheticHistory;
use eval_core::{
    ArtifactIdentity, DriverIdentity, HostManifest, OpenLoopCounts, ScaleInputs, ScaleReport,
    TierRatio, parse_pass_row,
};
use memory_store::MemoryStore;
use serde::Deserialize;
use serde_json::{Value, json};

/// Every flag takes one value; a repeated or unknown flag is refused.
fn flags(
    args: impl Iterator<Item = String>,
    known: &[&str],
) -> io::Result<std::collections::BTreeMap<String, String>> {
    let mut args = args;
    let mut values = std::collections::BTreeMap::new();
    while let Some(flag) = args.next() {
        if !known.contains(&flag.as_str()) {
            return Err(io::Error::other(format!("unknown flag {flag}")));
        }
        let value = args
            .next()
            .ok_or_else(|| io::Error::other(format!("{flag} needs a value")))?;
        if values.insert(flag.clone(), value).is_some() {
            return Err(io::Error::other(format!("{flag} given twice")));
        }
    }
    Ok(values)
}

fn required<'a>(
    values: &'a std::collections::BTreeMap<String, String>,
    flag: &str,
) -> io::Result<&'a str> {
    values
        .get(flag)
        .map(String::as_str)
        .ok_or_else(|| io::Error::other(format!("{flag} is required")))
}

/// Seeds `--segments` synthetic history segments of two messages each into `--session` of the
/// fixture store under `--state-root`, with the ordinal continuation base the newest segment
/// ends on, and prints the anchor the first pass declares.
pub fn run_seed(args: impl Iterator<Item = String>) -> io::Result<()> {
    let values = flags(args, &["--state-root", "--session", "--segments"])?;
    let root = PathBuf::from(required(&values, "--state-root")?);
    let session = required(&values, "--session")?;
    let segments: usize = required(&values, "--segments")?
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
    let mut meta = loaded.meta.clone();
    meta.ordinal_continuation_base = Some((end - 2) as u64);
    store
        .commit(session, None, &loaded.core, &meta)
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
/// `--out`, and prints each tier's pooled percentiles and ratio verdict.
pub fn run_report(args: impl Iterator<Item = String>) -> io::Result<()> {
    let values = flags(args, &["--rows", "--manifest", "--out"])?;
    let manifest: Manifest = serde_json::from_slice(&fs::read(required(&values, "--manifest")?)?)
        .map_err(io::Error::other)?;
    let text = fs::read_to_string(required(&values, "--rows")?)?;
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
    fs::write(required(&values, "--out")?, serde_json::to_vec(&value)?)?;
    let tiers: Vec<Value> = report
        .tiers
        .iter()
        .map(|tier| {
            json!({
                "harness": tier.harness,
                "tier": tier.tier,
                "sessions": tier.sessions.len(),
                "steady_passes": tier.steady_passes,
                "response_us": tier.response_percentiles,
                "service_us": tier.service_percentiles,
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
