//! The evaluator's campaign driver. `cassette-oracle` serves the TypeScript
//! MockProvider over line-delimited JSON on stdin and stdout: Rust computes
//! every request digest, admits every recorded exchange through the header
//! allowlist and the secret scanner, and writes the cassette; TypeScript only
//! forwards requests and compares the digest strings it gets back.
//! `campaign` runs one Suite B campaign through the direct-host fixture under
//! an approved profile and publishes its report and manifest. `aging` runs
//! one Suite C aging campaign in-process and publishes its report and
//! manifest.

#![forbid(unsafe_code)]

/// The aging shell is shared with the daemon's aging test, which drives its
/// stores directly and uses more of it than the subcommand does.
#[cfg(unix)]
#[allow(dead_code)]
mod aging;
#[cfg(unix)]
mod campaign;
/// The fixture and surface helpers are shared with the evaluator tests, which
/// use more of them than the campaign does.
#[cfg(unix)]
#[allow(dead_code)]
mod support;

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Component, Path, PathBuf};

use eval_core::{Boundary, Cassette, CassetteError, Lookup, OpenCodeRequest};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
#[cfg(unix)]
use sha2::Digest;

/// The largest request line accepted; a provider body is a few hundred KiB at
/// most and the redaction scanner refuses anything past 512 KiB anyway.
const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;

/// One request line. `namespace` binds the world variant on every operation so
/// an entry recorded under one variant can never answer another.
#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Op {
    Open {
        mode: Mode,
        namespace: String,
        path: PathBuf,
    },
    Lookup {
        namespace: String,
        request: RawRequest,
    },
    Record {
        namespace: String,
        request: RawRequest,
        response: Value,
    },
    Close,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Mode {
    Record,
    Replay,
}

/// The provider request as the mock received it. The body arrives as text so
/// Rust, not TypeScript, decides whether it parses.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRequest {
    path: String,
    headers: BTreeMap<String, String>,
    body_text: String,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum Reply {
    Open {
        cases: usize,
    },
    Hit {
        request_digest: String,
        response: Value,
    },
    Miss(eval_core::CassetteMiss),
    Record {
        request_digest: String,
    },
    Close {
        cases: usize,
        misses: usize,
        unconsumed: usize,
        input_sha256: Option<String>,
    },
}

/// Every refusal the oracle reports. `kind` is the wire contract; `detail` is
/// only ever the oracle's own values (paths, namespaces, digests, variant
/// text), never request content, because a serde message can quote input.
enum OracleError {
    NoOpenCassette,
    AlreadyOpen,
    LineTooLong,
    Json,
    UnsafePath(PathBuf),
    Io(io::Error),
    Cassette(CassetteError),
}

impl OracleError {
    fn kind(&self) -> &'static str {
        match self {
            Self::NoOpenCassette => "NoOpenCassette",
            Self::AlreadyOpen => "AlreadyOpen",
            Self::LineTooLong => "LineTooLong",
            Self::Json => "Json",
            Self::UnsafePath(_) => "UnsafePath",
            Self::Io(_) => "Io",
            Self::Cassette(error) => error.kind(),
        }
    }

    fn detail(&self) -> String {
        match self {
            Self::UnsafePath(path) => path.display().to_string(),
            Self::Io(error) => error.kind().to_string(),
            Self::Cassette(error) => error.to_string(),
            _ => String::new(),
        }
    }
}

impl From<CassetteError> for OracleError {
    fn from(error: CassetteError) -> Self {
        Self::Cassette(error)
    }
}

impl From<io::Error> for OracleError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

struct Open {
    cassette: Cassette,
    /// The file `close` writes; `None` for a replay, which never writes.
    write_to: Option<PathBuf>,
}

#[derive(Default)]
struct Oracle {
    open: Option<Open>,
}

impl Oracle {
    fn cassette(&mut self) -> Result<&mut Cassette, OracleError> {
        self.open
            .as_mut()
            .map(|open| &mut open.cassette)
            .ok_or(OracleError::NoOpenCassette)
    }

    fn apply(&mut self, op: Op) -> Result<Reply, OracleError> {
        match op {
            Op::Open {
                mode,
                namespace,
                path,
            } => {
                if self.open.is_some() {
                    return Err(OracleError::AlreadyOpen);
                }
                let path = safe_path(path)?;
                let (cassette, write_to) = match mode {
                    Mode::Record => (Cassette::recording(&namespace, Value::Null)?, Some(path)),
                    Mode::Replay => {
                        let text = fs::read_to_string(&path)?;
                        let value = serde_json::from_str(&text).map_err(|_| OracleError::Json)?;
                        (Cassette::replay(&value, &namespace)?, None)
                    }
                };
                let cases = cassette.cases().len();
                self.open = Some(Open { cassette, write_to });
                Ok(Reply::Open { cases })
            }
            Op::Lookup { namespace, request } => {
                let covered = covered(request)?;
                match self
                    .cassette()?
                    .lookup(&namespace, Boundary::Opencode, &covered)?
                {
                    Lookup::Hit(entry) => Ok(Reply::Hit {
                        request_digest: entry.request_digest.clone(),
                        response: entry.response.clone(),
                    }),
                    Lookup::Miss(miss) => Ok(Reply::Miss(miss)),
                }
            }
            Op::Record {
                namespace,
                request,
                response,
            } => {
                let covered = covered(request)?;
                let entry =
                    self.cassette()?
                        .record(&namespace, Boundary::Opencode, covered, response)?;
                Ok(Reply::Record {
                    request_digest: entry.request_digest.clone(),
                })
            }
            Op::Close => {
                let Open { cassette, write_to } =
                    self.open.take().ok_or(OracleError::NoOpenCassette)?;
                let mut input_sha256 = None;
                if let Some(path) = write_to {
                    // A refused cassette has no file form, so nothing is written for it.
                    let file = cassette.to_file()?;
                    input_sha256 = Some(file.provenance.input_sha256.clone());
                    write_then_rename(&path, &format!("{}\n", serde_json::to_string(&file)?))?;
                }
                Ok(Reply::Close {
                    cases: cassette.cases().len(),
                    misses: cassette.misses(),
                    unconsumed: cassette.unconsumed(),
                    input_sha256,
                })
            }
        }
    }
}

impl From<serde_json::Error> for OracleError {
    fn from(_: serde_json::Error) -> Self {
        Self::Json
    }
}

/// An absolute path with no `..` component, so the caller names the file
/// rather than a location relative to wherever the oracle happens to run.
fn safe_path(path: PathBuf) -> Result<PathBuf, OracleError> {
    let plain = path.is_absolute()
        && path
            .components()
            .all(|component| !matches!(component, Component::ParentDir));
    if plain {
        Ok(path)
    } else {
        Err(OracleError::UnsafePath(path))
    }
}

/// Publication is write-then-rename. Every entry was admitted before this
/// point, so the temporary file never holds an unscanned byte; it is created
/// fresh and owner-only, so a pre-existing sibling is an error rather than a
/// followed link.
fn write_then_rename(path: &Path, text: &str) -> io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let temp = path.with_extension("json.tmp");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    fs::rename(&temp, path)
}

fn covered(request: RawRequest) -> Result<Value, CassetteError> {
    OpenCodeRequest::from_raw(&request.path, request.headers, &request.body_text)?.covered()
}

fn error_reply(error: &OracleError) -> Value {
    json!({"error": {"kind": error.kind(), "detail": error.detail()}})
}

fn serve(input: impl BufRead, mut output: impl Write) -> io::Result<()> {
    let mut oracle = Oracle::default();
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let outcome = if line.len() > MAX_LINE_BYTES {
            Err(OracleError::LineTooLong)
        } else {
            serde_json::from_str::<Op>(&line)
                .map_err(|_| OracleError::Json)
                .and_then(|op| oracle.apply(op))
        };
        let reply = match outcome {
            Ok(reply) => json!({"ok": reply}),
            Err(error) => error_reply(&error),
        };
        writeln!(output, "{reply}")?;
        output.flush()?;
    }
    Ok(())
}

const USAGE: &str = "usage: eval_runner cassette-oracle | eval_runner ";

/// Runs the campaign and prints one JSON line naming what was published.
#[cfg(unix)]
fn run_campaign(args: impl Iterator<Item = String>) -> io::Result<()> {
    let config = campaign::config_from_args(args).map_err(io::Error::other)?;
    let run = campaign::run(&config).map_err(|error| io::Error::other(format!("{error:?}")))?;
    let digest = |bytes: &[u8]| format!("{:x}", sha2::Sha256::digest(bytes));
    let summary = json!({
        "report": config.publish.join(campaign::REPORT_FILE),
        "report_digest": digest(&run.report_bytes),
        "manifest": config.publish.join(campaign::MANIFEST_FILE),
        "manifest_digest": digest(&run.manifest_bytes),
        "eval_run_id": run.manifest.eval_run_id,
        "status": run.manifest.status,
        "pairs": run.set.pairs.len(),
        "samples": run.report.rates.samples,
        "attempted": run.report.samples.attempted(),
        "first_losses": run
            .verdicts
            .values()
            .filter(|verdict| matches!(verdict, eval_core::StageVerdict::FirstLoss(_)))
            .count(),
        "raw_pairs": run.outcomes.len(),
        "structured_pairs": run.structured_outcomes.len(),
        "aged_summarizer": {
            "firings": run.aged.firings,
            "refused": run.aged.refused,
            "segments": run.aged.covered.len(),
        },
    });
    println!("{summary}");
    Ok(())
}

/// Runs the aging campaign and prints one JSON line naming what was published.
#[cfg(unix)]
fn run_aging(args: impl Iterator<Item = String>) -> io::Result<()> {
    let config = aging::config_from_args(args).map_err(io::Error::other)?;
    let run = aging::run(&config).map_err(io::Error::other)?;
    let digest = |bytes: &[u8]| format!("{:x}", sha2::Sha256::digest(bytes));
    let summary = json!({
        "report": config.publish.join(aging::REPORT_FILE),
        "report_digest": digest(&run.report_bytes),
        "manifest": config.publish.join(aging::MANIFEST_FILE),
        "manifest_digest": digest(&run.manifest_bytes),
        "eval_run_id": run.manifest.eval_run_id,
        "status": run.manifest.status,
        "steps": run.report.steps,
        "checkpoint_step": run.report.checkpoint_step,
        "live_digests_equal": run.report.against_resumed.live_digests_equal,
        "resumed_projection": run.report.against_resumed.later,
        "divergences": run.report.against_resumed.divergences.len(),
        "bulk_divergences": run.report.against_bulk.divergences.len(),
        "markers": run.report.markers,
    });
    println!("{summary}");
    Ok(())
}

fn main() {
    let mut args = std::env::args().skip(1);
    let outcome = match args.next().as_deref() {
        Some("cassette-oracle") => serve(io::stdin().lock(), io::stdout().lock()),
        #[cfg(unix)]
        Some("campaign") => run_campaign(args),
        #[cfg(unix)]
        Some("aging") => run_aging(args),
        other => Err(io::Error::other(format!(
            "{USAGE}{} (got {other:?})",
            campaign_usage()
        ))),
    };
    if let Err(error) = outcome {
        eprintln!("eval_runner: {error}");
        std::process::exit(2);
    }
}

#[cfg(unix)]
fn campaign_usage() -> String {
    format!("{} | eval_runner {}", campaign::USAGE, aging::USAGE)
}

#[cfg(not(unix))]
fn campaign_usage() -> String {
    "campaign | aging (unix only)".to_string()
}
