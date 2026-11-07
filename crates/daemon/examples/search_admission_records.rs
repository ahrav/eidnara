//! A generator of test-only search admission records for a built payload's embedding bundle.
//!
//! Usage: `search_admission_records <bundle-manifest.json> <out-dir>`. `eidnara-host
//! install-search-admission` admits the payload's projection from the two records it writes.

#![forbid(unsafe_code)]

#[path = "../tests/support/projection_gate.rs"]
#[allow(dead_code)]
mod projection_gate;
#[path = "search_admission_records/records.rs"]
mod records;

use std::error::Error;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    let (Some(bundle), Some(out), None) = (args.next(), args.next(), args.next()) else {
        return Err("usage: search_admission_records <bundle-manifest.json> <out-dir>".into());
    };
    let bundle: serde_json::Value = serde_json::from_slice(&std::fs::read(&bundle)?)?;
    let (manifest, campaign) = records::records(&bundle)?;
    let out = PathBuf::from(out);
    std::fs::create_dir_all(&out)?;
    std::fs::write(
        out.join("runtime-manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    std::fs::write(
        out.join("campaign-evidence.json"),
        serde_json::to_vec_pretty(&campaign)?,
    )?;
    Ok(())
}
