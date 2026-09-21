//! The evaluator's campaign driver. `cassette-oracle` serves the TypeScript
//! MockProvider over line-delimited JSON on stdin and stdout: Rust computes
//! every request digest, admits every recorded exchange through the header
//! allowlist and the secret scanner, and writes the cassette; TypeScript only
//! forwards requests and compares the digest strings it gets back.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Component, Path, PathBuf};

use eval_core::{Boundary, Cassette, CassetteError, Lookup, OpenCodeRequest};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

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
/// text), never request content; see `CassetteError::detail`.
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
            Self::Cassette(error) => error.detail(),
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
                let cassette = self.cassette()?;
                // A request that cannot be projected leaves the recording with no file form.
                let covered = covered(request).map_err(|error| cassette.refuse(error))?;
                let entry = cassette.record(&namespace, Boundary::Opencode, covered, response)?;
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
/// followed link. A temporary file this attempt created is removed when a
/// later step fails, so it cannot block the next recording to the same path.
fn write_then_rename(path: &Path, text: &str) -> io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let temp = path.with_extension("json.tmp");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)?;
    let published = file
        .write_all(text.as_bytes())
        .and_then(|()| file.sync_all())
        .and_then(|()| fs::rename(&temp, path));
    if published.is_err() {
        let _ = fs::remove_file(&temp);
    }
    published
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

fn main() -> io::Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("cassette-oracle") => serve(io::stdin().lock(), io::stdout().lock()),
        other => Err(io::Error::other(format!(
            "usage: eval_runner cassette-oracle (got {other:?})"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh directory under the system temp dir; the file names inside are
    /// absolute, as the oracle requires.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("eval-runner-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Runs `lines` through the oracle and returns one parsed reply per line.
    fn oracle(lines: &[Value]) -> Vec<Value> {
        let input: String = lines.iter().map(|line| format!("{line}\n")).collect();
        let mut output = Vec::new();
        serve(input.as_bytes(), &mut output).unwrap();
        String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn open(path: &Path) -> Value {
        json!({"op": "open", "mode": "record", "namespace": "eval-run:fresh:0", "path": path})
    }

    fn record(body: Value) -> Value {
        json!({"op": "record", "namespace": "eval-run:fresh:0",
            "request": {"path": "/messages", "headers": {}, "body_text": body.to_string()},
            "response": {"status": 200, "frames": []}})
    }

    #[test]
    fn a_record_the_oracle_cannot_project_leaves_close_with_no_file() {
        let dir = scratch("unprojectable");
        let path = dir.join("cassette.json");
        let replies = oracle(&[
            open(&path),
            record(json!({"model": "m"})),
            record(json!({"model": "m", "metadata": {"user_id": "u1"}})),
            json!({"op": "close"}),
        ]);
        assert_eq!(replies[0], json!({"ok": {"open": {"cases": 0}}}));
        assert!(replies[1]["ok"]["record"]["request_digest"].is_string());
        assert_eq!(replies[2]["error"]["kind"], json!("UnknownRequestField"));
        assert_eq!(replies[2]["error"]["detail"], json!(""));
        assert_eq!(replies[3]["error"]["kind"], json!("UnknownRequestField"));
        assert!(!path.exists(), "a partial recording is never published");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_publication_removes_the_temp_file_it_created() {
        let dir = scratch("publication");
        // A directory at the target path makes the rename fail after the
        // temporary file exists.
        let path = dir.join("cassette.json");
        fs::create_dir(&path).unwrap();
        let replies = oracle(&[
            open(&path),
            record(json!({"model": "m"})),
            json!({"op": "close"}),
        ]);
        assert_eq!(replies[2]["error"]["kind"], json!("Io"));
        assert!(
            !dir.join("cassette.json.tmp").exists(),
            "the attempt's temporary file is gone"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
