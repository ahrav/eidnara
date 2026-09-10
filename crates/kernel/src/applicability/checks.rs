//! Bounded cheap checks against a checkout snapshot's worktree.

use std::collections::HashMap;
use std::io::Read;
use std::sync::{Arc, OnceLock};

use sha2::{Digest, Sha256};

use super::checkout::{
    CheckoutSnapshot, EvalBudget, WorktreeEntry, normalized_blob_id_in, tracked_mode_matches,
};
use super::payloads::CheckSpec;

/// Maximum config bytes read for one cheap check.
pub const MAX_CONFIG_BYTES: u64 = 1 << 20;

/// Maximum config bytes one batch retains across every check path. Each path
/// is read once per batch and its content held for the rest of it. Without
/// this cap, a checkout can control resident memory.
pub const MAX_CHECK_CACHE_BYTES: u64 = 16 * MAX_CONFIG_BYTES;

/// The evaluator maps `Unsupported` to uncertain rather than pass or fail.
#[must_use]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckOutcome {
    Passed,
    Failed { evidence: String },
    Unsupported { evidence: String },
    BudgetExhausted,
}

/// Absence is settled by the shape probe before a read is attempted, so a read
/// either yields content or leaves the key unevaluated: a permission error, a
/// transient I/O error, a vanished path, or a file past the size cap.
#[derive(Debug, Clone)]
enum ConfigRead {
    Content(Arc<ConfigContent>),
    Unevaluated(String),
}

impl ConfigRead {
    /// Digest material naming the content this batch read, so a cache key can
    /// distinguish two runs that read different bytes at one path.
    fn observation(&self) -> String {
        match self {
            Self::Content(content) => content.observation.clone(),
            Self::Unevaluated(reason) => format!("unevaluated:{reason}"),
        }
    }
}

/// One config file's bytes with the derived values every check against it
/// shares: the digest is fixed at read time and each parse runs at most
/// once, so K checks on one path cost one hash and one parse per batch.
#[derive(Debug)]
struct ConfigContent {
    text: String,
    observation: String,
    json: OnceLock<Option<serde_json::Value>>,
    yaml: OnceLock<Option<Vec<serde_norway::Value>>>,
}

impl ConfigContent {
    fn new(text: String) -> Self {
        let mut hash = Sha256::new();
        hash.update(text.as_bytes());
        Self {
            text,
            observation: format!("content:{:x}", hash.finalize()),
            json: OnceLock::new(),
            yaml: OnceLock::new(),
        }
    }

    /// `None` when the document is not JSON.
    fn json(&self) -> Option<&serde_json::Value> {
        self.json
            .get_or_init(|| serde_json::from_str(&self.text).ok())
            .as_ref()
    }

    /// `None` unless every document in the YAML stream parses and at least one
    /// is a mapping or sequence. A TOML or INI file parses as one plain scalar
    /// or fails, so structure here means the file is YAML; the line heuristic
    /// applies otherwise.
    fn yaml(&self) -> Option<&[serde_norway::Value]> {
        self.yaml_documents()
            .filter(|documents| documents.iter().any(yaml_is_structured))
    }

    /// Every document parsed, whatever its shape, or `None` when the stream is
    /// not YAML.
    fn yaml_documents(&self) -> Option<&[serde_norway::Value]> {
        self.yaml
            .get_or_init(|| {
                use serde::Deserialize;
                serde_norway::Deserializer::from_str(&self.text)
                    .map(serde_norway::Value::deserialize)
                    .collect::<Result<Vec<_>, _>>()
                    .ok()
            })
            .as_deref()
    }
}

/// Whether a declared path is a file a check can read, and if not, why.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Resolved {
    /// Present and a regular file, established without following a symlink.
    RegularFile { executable: bool },
    /// Resolved beneath the worktree and definitely not there.
    Absent,
    /// Present, and established to be a shape no check can read: a directory,
    /// socket, device, or FIFO. No regular file is at this path, which is as
    /// definite an answer about the declared file as absence — unlike a
    /// symlink, whose unfollowed target could be one.
    NotAFile(String),
    /// Nothing was read: the spelling leaves the worktree, or an inspection
    /// failed, or a symlink was refused rather than followed.
    Unresolvable(String),
}

impl Resolved {
    fn observation(&self) -> &str {
        match self {
            Self::RegularFile { .. } => "regular-file",
            Self::Absent => "absent",
            Self::NotAFile(reason) | Self::Unresolvable(reason) => reason,
        }
    }
}

/// Single authority on what one batch observed in the worktree.
///
/// The object cache key and the check that produces a verdict both read
/// through here, so a cached verdict can never describe bytes its key does
/// not. Reading the live filesystem twice would let a path change between the
/// two reads and revert afterwards, storing a verdict under a key no later
/// request reproduces.
#[derive(Debug, Default)]
pub struct CheckCache {
    resolved: HashMap<String, Resolved>,
    contents: HashMap<String, ConfigRead>,
    /// Total content bytes held in `contents`; never exceeds
    /// `MAX_CHECK_CACHE_BYTES`.
    retained_bytes: u64,
}

impl CheckCache {
    pub fn new() -> Self {
        Self::default()
    }

    fn resolve(&mut self, snapshot: &CheckoutSnapshot, path: &str) -> Resolved {
        if let Some(cached) = self.resolved.get(path) {
            return cached.clone();
        }
        let resolved = match snapshot.worktree_entry(path) {
            WorktreeEntry::RegularFile { executable } => Resolved::RegularFile { executable },
            WorktreeEntry::Absent => Resolved::Absent,
            // A terminal symlink is where the target could sit outside the
            // checkout, so no check follows one to a verdict.
            WorktreeEntry::Symlink => Resolved::Unresolvable(format!(
                "check path {path} is a symlink, whose target this check will not follow"
            )),
            WorktreeEntry::Directory => Resolved::NotAFile(format!(
                "check path {path} is a directory, not a file a check can read"
            )),
            WorktreeEntry::Other => {
                Resolved::NotAFile(format!("check path {path} is not a regular file"))
            }
            WorktreeEntry::Unresolvable(reason) => Resolved::Unresolvable(reason),
        };
        self.resolved.insert(path.to_string(), resolved.clone());
        resolved
    }

    /// The exhausted-budget outcome is cached like any other, so the cache
    /// key and the verdict describe the same single read.
    fn read(&mut self, snapshot: &CheckoutSnapshot, path: &str) -> ConfigRead {
        if let Some(cached) = self.contents.get(path) {
            return cached.clone();
        }
        let outcome = match read_bounded(snapshot, path) {
            ConfigRead::Content(content)
                if self.retained_bytes + content.text.len() as u64 > MAX_CHECK_CACHE_BYTES =>
            {
                ConfigRead::Unevaluated("config read budget exhausted".to_string())
            }
            ConfigRead::Content(content) => {
                self.retained_bytes += content.text.len() as u64;
                ConfigRead::Content(content)
            }
            unevaluated => unevaluated,
        };
        self.contents.insert(path.to_string(), outcome.clone());
        outcome
    }
}

/// Digest material for the worktree state `check` reads, or `None` for a check
/// whose verdict does not depend on the filesystem.
pub(super) fn check_observation(
    cache: &mut CheckCache,
    snapshot: &CheckoutSnapshot,
    check: &CheckSpec,
) -> Option<String> {
    match check {
        CheckSpec::FileExists { path } => {
            Some(cache.resolve(snapshot, path).observation().to_string())
        }
        CheckSpec::ConfigKey { path, .. } => {
            let resolved = cache.resolve(snapshot, path);
            let shape = resolved.observation().to_string();
            // Only a regular file is read, so only then is there content to
            // name; the shape alone settles the other cases.
            match resolved {
                Resolved::RegularFile { .. } => {
                    let content = cache.read(snapshot, path).observation();
                    Some(format!("{shape}\u{1f}{content}"))
                }
                Resolved::Absent | Resolved::NotAFile(_) | Resolved::Unresolvable(_) => Some(shape),
            }
        }
        // Both are `Unsupported` whatever the worktree holds.
        CheckSpec::Symbol { .. } | CheckSpec::Unrecognized => None,
    }
}

/// Reads through a descriptor walk that follows no symlink at any level, and
/// sizes the file by that descriptor.
///
/// A pathname re-resolved after a containment check can escape: a concurrent
/// checkout that replaces an ancestor directory with a symlink redirects every
/// later pathname operation, and `NOFOLLOW` guards only the final component.
/// FIFOs can block reads and character devices such as `/dev/zero` produce
/// valid UTF-8 indefinitely, both of which the open refuses.
fn read_bounded(snapshot: &CheckoutSnapshot, path: &str) -> ConfigRead {
    let file = match snapshot.open_worktree_regular(path) {
        Ok(Some(file)) => file,
        // Resolution already saw a regular file here, so the path moved.
        Ok(None) => {
            return ConfigRead::Unevaluated("path is no longer a regular file".to_string());
        }
        Err(error) => return ConfigRead::Unevaluated(error.to_string()),
    };
    match file.metadata() {
        Ok(metadata) if metadata.len() > MAX_CONFIG_BYTES => {
            return ConfigRead::Unevaluated(format!("file exceeds {MAX_CONFIG_BYTES} bytes"));
        }
        Ok(_) => {}
        Err(error) => return ConfigRead::Unevaluated(error.to_string()),
    }
    let mut content = String::new();
    // Metadata size can race file growth and does not always bound the stream,
    // so `MAX_CONFIG_BYTES` is enforced on bytes read.
    match file.take(MAX_CONFIG_BYTES + 1).read_to_string(&mut content) {
        Ok(_) if content.len() as u64 > MAX_CONFIG_BYTES => {
            ConfigRead::Unevaluated(format!("file exceeds {MAX_CONFIG_BYTES} bytes"))
        }
        Ok(_) => ConfigRead::Content(Arc::new(ConfigContent::new(content))),
        Err(error) => ConfigRead::Unevaluated(error.to_string()),
    }
}

/// `Unsupported` rather than `Failed`: the check was never evaluated, so the
/// checked object is uncertain rather than definitely stale.
fn unevaluated(evidence: String) -> CheckOutcome {
    CheckOutcome::Unsupported { evidence }
}

/// Runs one check natively against the snapshot's worktree. File existence
/// and config-key presence ship here; symbol resolution returns
/// `Unsupported` until a real resolver exists.
pub fn run_cheap_check(
    snapshot: &CheckoutSnapshot,
    check: &CheckSpec,
    budget: &EvalBudget,
    cache: &mut CheckCache,
) -> CheckOutcome {
    if budget.is_exhausted() {
        return CheckOutcome::BudgetExhausted;
    }
    match check {
        CheckSpec::FileExists { path } => match cache.resolve(snapshot, path) {
            Resolved::RegularFile { .. } => CheckOutcome::Passed,
            Resolved::Absent => CheckOutcome::Failed {
                evidence: format!("file {path} does not exist in the checkout"),
            },
            // No regular file is here, which the check asked about; repair can
            // act on that exactly as it acts on an absent path.
            Resolved::NotAFile(reason) => CheckOutcome::Failed { evidence: reason },
            // A path that was never read is not a definite absence.
            Resolved::Unresolvable(reason) => unevaluated(reason),
        },
        CheckSpec::ConfigKey { path, key } => {
            match cache.resolve(snapshot, path) {
                Resolved::RegularFile { .. } => {}
                Resolved::Absent => {
                    return CheckOutcome::Failed {
                        evidence: format!("config file {path} does not exist in the checkout"),
                    };
                }
                // `Failed` here would claim the file exists and omits the key,
                // which is what repair would then try to edit. A shape that is
                // not a config file leaves the key unevaluated instead.
                Resolved::NotAFile(reason) | Resolved::Unresolvable(reason) => {
                    return unevaluated(reason);
                }
            }
            let content = match cache.read(snapshot, path) {
                ConfigRead::Content(content) => content,
                // The key may well be defined; the read never got to look.
                ConfigRead::Unevaluated(reason) => {
                    return unevaluated(format!("config file {path} could not be read: {reason}"));
                }
            };
            match config_contains_key(&content, key) {
                KeyPresence::Present => CheckOutcome::Passed,
                KeyPresence::Absent => CheckOutcome::Failed {
                    evidence: format!("config file {path} does not define key {key}"),
                },
                KeyPresence::Undecidable(reason) => {
                    unevaluated(format!("config file {path}: {reason}"))
                }
            }
        }
        CheckSpec::Symbol { path, symbol } => CheckOutcome::Unsupported {
            evidence: format!("symbol check for {symbol} in {path} is not supported yet"),
        },
        CheckSpec::Unrecognized => CheckOutcome::Unsupported {
            evidence: "check kind is not recognized".to_string(),
        },
    }
}

/// JSON carries no line structure, so a minified document has to be parsed
/// rather than scanned; the line heuristic below reports every key in
/// `{"flag":true}` missing.
fn json_contains_key(value: &serde_json::Value, key: &str) -> bool {
    match value {
        serde_json::Value::Object(map) => {
            // Any depth, matching what the line scan finds in a pretty-printed
            // document.
            map.contains_key(key) || map.values().any(|value| json_contains_key(value, key))
        }
        serde_json::Value::Array(items) => items.iter().any(|item| json_contains_key(item, key)),
        _ => false,
    }
}

/// A scalar (or nothing), possibly behind a root tag.
fn yaml_is_scalar(value: &serde_norway::Value) -> bool {
    match value {
        serde_norway::Value::Tagged(tagged) => yaml_is_scalar(&tagged.value),
        other => !yaml_is_structured(other),
    }
}

/// A mapping or sequence, possibly behind a root tag such as `!Config { … }`.
fn yaml_is_structured(value: &serde_norway::Value) -> bool {
    match value {
        serde_norway::Value::Mapping(_) | serde_norway::Value::Sequence(_) => true,
        serde_norway::Value::Tagged(tagged) => yaml_is_structured(&tagged.value),
        _ => false,
    }
}

/// A line scan cannot tell a mapping key from the same text inside a block
/// scalar (`description: |` followed by an indented `enabled: true`), so a
/// parsed YAML document is walked structurally like JSON. Only string keys
/// are compared.
fn yaml_contains_key(value: &serde_norway::Value, key: &str) -> bool {
    match value {
        serde_norway::Value::Mapping(map) => map.iter().any(|(name, nested)| {
            matches!(name, serde_norway::Value::String(name) if name == key)
                || yaml_contains_key(nested, key)
        }),
        serde_norway::Value::Sequence(items) => {
            items.iter().any(|item| yaml_contains_key(item, key))
        }
        serde_norway::Value::Tagged(tagged) => yaml_contains_key(&tagged.value, key),
        _ => false,
    }
}

/// What a config document says about one key.
enum KeyPresence {
    Present,
    Absent,
    /// The document has a shape the line heuristic cannot read safely.
    Undecidable(String),
}

/// Structured documents (JSON, YAML) are walked; the remaining line-oriented
/// formats (TOML, INI) use a presence heuristic: the key must open a line
/// (after whitespace and optional quoting) and be followed by a delimiter.
/// A TOML multi-line string can hold a line shaped exactly like an
/// assignment, so a document containing one is undecidable rather than
/// scanned.
fn config_contains_key(content: &ConfigContent, key: &str) -> KeyPresence {
    if let Some(value) = content.json() {
        return present(json_contains_key(value, key));
    }
    if let Some(documents) = content.yaml() {
        // `[server]` alone parses as a YAML flow sequence of one scalar and as
        // a TOML table header; only the TOML reading defines a key, so a
        // document of scalar-only sequences that has table-header lines is
        // read as TOML.
        let scalar_sequences_only = documents.iter().all(|document| {
            matches!(document, serde_norway::Value::Sequence(items)
                if items.iter().all(|item| !yaml_is_structured(item)))
        });
        if !(scalar_sequences_only
            && content
                .text
                .lines()
                .any(|line| table_header(line.trim()).is_some()))
        {
            return present(
                documents
                    .iter()
                    .any(|document| yaml_contains_key(document, key)),
            );
        }
    }
    // A YAML document whose root is a block scalar (`|` or `>`) or a quoted
    // scalar parses as one string and holds no keys; its lines are content,
    // not assignments. A bare TOML or INI line also parses as a YAML plain
    // scalar, which is why only these marked forms decide here.
    if content
        .yaml_documents()
        .is_some_and(|documents| documents.iter().all(yaml_is_scalar))
        && yaml_documents_text(&content.text).any(yaml_root_opens_scalar)
    {
        return KeyPresence::Absent;
    }
    match toml_keys(&content.text) {
        Some(keys) => present(keys.iter().any(|found| found == key)),
        None => KeyPresence::Undecidable(
            "line-oriented config has a shape the key scan cannot read".to_string(),
        ),
    }
}

/// Every key a line-oriented config (TOML, INI) defines: table headers,
/// dotted assignments at every segment, inline-table keys at every depth,
/// and INI `key: value`. Strings (basic with escapes, literal), arrays,
/// inline tables, and comments are tokenized, so text inside a value is
/// never a key. `None` when the document holds a multi-line string, whose
/// lines the tokenizer does not model.
fn toml_keys(text: &str) -> Option<Vec<String>> {
    if text.contains("\"\"\"") || text.contains("'''") {
        return None;
    }
    let mut keys = Vec::new();
    let mut value = ValueScan::default();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with(['#', ';']) {
            continue;
        }
        // An open array treats a following `[` line as an element, not a
        // table header.
        if value.is_open() {
            value.collect_keys(line, &mut keys);
            continue;
        }
        if let Some(inner) = table_header(line) {
            if let Some(segments) = key_segments(inner) {
                keys.extend(segments);
            }
            continue;
        }
        let Some((lhs, rest)) = split_at_delimiter(line) else {
            continue;
        };
        let Some(segments) = key_segments(lhs) else {
            continue;
        };
        keys.extend(segments);
        value.collect_keys(rest, &mut keys);
    }
    // A container still open at the end consumed every later line as value
    // content; whatever those lines defined is unreadable, not absent.
    if value.is_open() {
        return None;
    }
    Some(keys)
}

/// The key text of a `[a.b]` or `[[a.b]]` header. Accepts only whitespace or
/// a `#` comment after the closing bracket. A quote starts a quoted segment
/// only at a segment start and only when the line closes it; brackets inside
/// quoted segments are not structural, and any other quote is a literal
/// character of an INI section name.
fn table_header(line: &str) -> Option<&str> {
    let (open, close) = if line.starts_with("[[") {
        ("[[", "]]")
    } else if line.starts_with('[') {
        ("[", "]")
    } else {
        return None;
    };
    let body = &line[open.len()..];
    let mut pos = 0usize;
    let mut segment_start = true;
    while pos < body.len() {
        let rest = &body[pos..];
        if let Some(after) = rest.strip_prefix(close) {
            let after = after.trim_start();
            return (after.is_empty() || after.starts_with('#')).then_some(&body[..pos]);
        }
        if segment_start
            && rest.starts_with(['"', '\''])
            && let Some((_, after)) = quoted(rest)
        {
            pos = body.len() - after.len();
            segment_start = false;
            continue;
        }
        let ch = rest.chars().next()?;
        segment_start = ch == '.' || (segment_start && ch.is_whitespace());
        pos += ch.len_utf8();
    }
    None
}

/// Segments of a dotted key, with a quoted segment as one key whatever dots it
/// holds and a basic-quoted segment honoring escapes. A quote-led segment the
/// line never closes is a literal INI key. `None` for an empty segment or for
/// text that follows a quoted segment without a dot.
fn key_segments(lhs: &str) -> Option<Vec<String>> {
    let mut segments = Vec::new();
    let mut rest = lhs.trim();
    while !rest.is_empty() {
        let quoted_segment = if rest.starts_with(['"', '\'']) {
            quoted(rest)
        } else {
            None
        };
        let segment = if let Some((segment, after)) = quoted_segment {
            rest = after.trim_start();
            segment
        } else {
            let end = rest.find('.').unwrap_or(rest.len());
            let segment = rest[..end].trim().to_string();
            rest = &rest[end..];
            segment
        };
        if segment.is_empty() {
            return None;
        }
        segments.push(segment);
        rest = match rest.strip_prefix('.') {
            Some(after) => after.trim_start(),
            None if rest.is_empty() => rest,
            None => return None,
        };
    }
    Some(segments)
}

/// The unescaped contents of the quoted string at the start of `text` and the
/// text after its closing quote. A basic string honors `\\` escapes; a literal
/// string has none.
fn quoted(text: &str) -> Option<(String, &str)> {
    let mut chars = text.char_indices().peekable();
    let (_, quote) = chars.next()?;
    let mut out = String::new();
    while let Some((offset, ch)) = chars.next() {
        if ch == quote {
            return Some((out, &text[offset + ch.len_utf8()..]));
        }
        if ch == '\\' && quote == '"' {
            let (_, escape) = chars.next()?;
            match escape {
                'b' => out.push('\u{8}'),
                't' => out.push('\t'),
                'n' => out.push('\n'),
                'f' => out.push('\u{c}'),
                'r' => out.push('\r'),
                'e' => out.push('\u{1b}'),
                '"' | '\\' => out.push(escape),
                'u' | 'U' => {
                    let digits = if escape == 'u' { 4 } else { 8 };
                    let mut code = 0u32;
                    for _ in 0..digits {
                        let (_, digit) = chars.next()?;
                        code = code.checked_mul(16)?.checked_add(digit.to_digit(16)?)?;
                    }
                    out.push(char::from_u32(code)?);
                }
                // Any other escape is not TOML; the key is unreadable.
                _ => return None,
            }
            continue;
        }
        out.push(ch);
    }
    None
}

/// The key text before the first `=` or `:` outside quotes, and the text after
/// that delimiter. `None` when the line has no key.
fn split_at_delimiter(line: &str) -> Option<(&str, &str)> {
    let mut pos = 0usize;
    while pos < line.len() {
        let rest = &line[pos..];
        let ch = rest.chars().next()?;
        match ch {
            '"' | '\'' => {
                let (_, after) = quoted(rest)?;
                pos = line.len() - after.len();
            }
            '=' | ':' => return Some((&line[..pos], &line[pos + 1..])),
            '#' => return None,
            _ => pos += ch.len_utf8(),
        }
    }
    None
}

/// Walks the value side of assignments, carrying open containers across
/// lines so a multi-line array is read as one value.
#[derive(Default)]
struct ValueScan {
    // The innermost open container decides what a comma separates: keys in
    // an inline table, elements in an array. Counts cannot tell the two
    // apart once they nest, so the containers are kept in order.
    containers: Vec<Container>,
    expecting_key: bool,
}

impl ValueScan {
    fn is_open(&self) -> bool {
        !self.containers.is_empty()
    }

    /// Keys defined inside the value: an inline table at any depth defines its
    /// keys, including inside arrays; strings and comments define none. An
    /// unterminated string ends the value and closes its containers.
    fn collect_keys(&mut self, text: &str, keys: &mut Vec<String>) {
        let mut pos = 0usize;
        while pos < text.len() {
            let rest = &text[pos..];
            let Some(ch) = rest.chars().next() else {
                break;
            };
            let in_table = self.containers.last() == Some(&Container::Table);
            match ch {
                '#' => break,
                '"' | '\'' => {
                    let Some((quoted_text, after)) = quoted(rest) else {
                        self.containers.clear();
                        self.expecting_key = false;
                        return;
                    };
                    pos = text.len() - after.len();
                    if self.expecting_key {
                        keys.push(quoted_text);
                        self.expecting_key = false;
                    }
                    continue;
                }
                '{' => {
                    self.containers.push(Container::Table);
                    self.expecting_key = true;
                }
                '}' => {
                    self.containers.pop();
                    self.expecting_key = false;
                }
                '[' => {
                    self.containers.push(Container::Array);
                    self.expecting_key = false;
                }
                ']' => {
                    self.containers.pop();
                    self.expecting_key = false;
                }
                ',' => self.expecting_key = in_table,
                '=' => self.expecting_key = false,
                '.' if self.expecting_key => {}
                c if c.is_whitespace() => {}
                _ if self.expecting_key && in_table => {
                    let end = rest
                        .find(|c: char| c.is_whitespace() || matches!(c, '=' | ',' | '}' | '.'))
                        .unwrap_or(rest.len());
                    keys.push(rest[..end].to_string());
                    pos += end;
                    if !rest[end..].starts_with('.') {
                        self.expecting_key = false;
                    }
                    continue;
                }
                _ => {}
            }
            pos += ch.len_utf8();
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Container {
    Table,
    Array,
}

/// The text of each document in a YAML stream, split at `---` markers that
/// open a line. A stream with one document yields it whole.
fn yaml_documents_text(text: &str) -> impl Iterator<Item = &str> {
    let mut starts = vec![0usize];
    let mut offset = 0usize;
    for line in text.split_inclusive('\n') {
        if offset > 0 && line.trim_start().starts_with("---") {
            starts.push(offset);
        }
        offset += line.len();
    }
    starts.push(text.len());
    starts
        .windows(2)
        .map(move |window| &text[window[0]..window[1]])
        .collect::<Vec<_>>()
        .into_iter()
}

/// The first significant line of a YAML document, after comments, directives,
/// a `---` marker, and any leading node properties (`!Config`, `!!str`,
/// `&anchor`), opens with a block scalar indicator or a quote.
fn yaml_root_opens_scalar(text: &str) -> bool {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#') && !line.starts_with('%'))
        .map(|line| line.strip_prefix("---").map_or(line, str::trim_start))
        // A comment after `---` is not content.
        .map(|line| if line.starts_with('#') { "" } else { line })
        .map(|mut line| {
            // Node properties (`!tag`, `&anchor`) precede the content
            // indicator, on the same line or on lines of their own.
            while line.starts_with(['!', '&']) {
                line = line
                    .split_once(char::is_whitespace)
                    .map_or("", |(_, rest)| rest.trim_start());
            }
            line
        })
        .find(|line| !line.is_empty())
        .is_some_and(|line| line.starts_with(['|', '>', '"', '\'']))
}

fn present(found: bool) -> KeyPresence {
    if found {
        KeyPresence::Present
    } else {
        KeyPresence::Absent
    }
}

/// The nearest proper ancestor of `tracked` that `index` records as a gitlink.
fn enclosing_gitlink<'p>(index: &gix::index::State, tracked: &'p str) -> Option<&'p str> {
    tracked
        .match_indices('/')
        .map(|(offset, _)| &tracked[..offset])
        .filter(|ancestor| !ancestor.is_empty())
        .find(|ancestor| {
            index
                .entry_by_path((*ancestor).into())
                .is_some_and(|entry| entry.mode == gix::index::entry::Mode::COMMIT)
        })
}

/// Whether the worktree state a check observed for `path` is still the state
/// the index records under `tracked`, the normalized spelling of the same
/// path, so the snapshot's clean dirty gate still describes it.
///
/// The snapshot's dirty gate and a check's live read are two observations of
/// one path; an edit between them pairs a clean gate with content the gate
/// never saw. A tracked path is compared by blob id against its index entry;
/// an untracked path that is present and not ignored appeared after the
/// snapshot. `None` when the path is tracked but the check read no content,
/// which leaves nothing to compare.
pub(super) fn observation_matches_index(
    cache: &mut CheckCache,
    snapshot: &CheckoutSnapshot,
    path: &str,
    tracked: &str,
) -> Option<bool> {
    let repo = snapshot.repo();
    let index = snapshot.index();
    // A path beneath a tracked gitlink lives in the submodule's index; the
    // superproject index only says the gitlink exists.
    if let Some(gitlink) = enclosing_gitlink(index, tracked) {
        // A gitlink that was clean at the snapshot has its worktree equal to
        // the commit the superproject index records, so that commit's tree is
        // the snapshot-time reference; the submodule's live index is not,
        // since a stage after the snapshot moves it. A dirty gitlink is in the
        // dirty set and the gate catches the overlap before this runs.
        let commit = index.entry_by_path(gitlink.into())?.id;
        let Some(nested) = snapshot.nested_index(gitlink) else {
            return Some(false);
        };
        let relative = &tracked[gitlink.len() + 1..];
        // The recorded commit is the only snapshot-time reference; without it
        // the live observation cannot be validated and is not accepted.
        let Some(mut tree) = nested
            .repo
            .find_commit(commit)
            .ok()
            .and_then(|commit| commit.tree().ok())
        else {
            return Some(false);
        };
        let Ok(tree_entry) = tree.peel_to_entry_by_path(relative) else {
            return Some(false);
        };
        let recorded = match tree_entry {
            Some(entry) => {
                let mode = gix::index::entry::Mode::from(entry.mode());
                // A gitlink nested inside the submodule has no reference here.
                if mode == gix::index::entry::Mode::COMMIT {
                    return Some(false);
                }
                Some((mode, entry.object_id()))
            }
            None => None,
        };
        return observation_matches_entry(
            cache,
            snapshot,
            &nested.repo,
            &nested.index,
            path,
            relative,
            recorded,
        );
    }
    let recorded = index
        .entry_by_path(tracked.into())
        .map(|entry| (entry.mode, entry.id));
    observation_matches_entry(cache, snapshot, repo, index, path, tracked, recorded)
}

/// [`observation_matches_index`] against one repository, where `tracked` is
/// relative to that repository's worktree and `recorded` is the mode and blob
/// the reference (index or tree) holds for it.
fn observation_matches_entry(
    cache: &mut CheckCache,
    snapshot: &CheckoutSnapshot,
    repo: &gix::Repository,
    index: &gix::index::State,
    path: &str,
    tracked: &str,
    recorded: Option<(gix::index::entry::Mode, gix::ObjectId)>,
) -> Option<bool> {
    use gix::index::entry::{Flags, Mode};
    let entry = recorded;
    let executable = match cache.resolve(snapshot, path) {
        Resolved::RegularFile { executable } => executable,
        // An absent path is consistent with no entry, or with a skip-worktree
        // entry the checkout never materializes.
        Resolved::Absent => {
            return Some(
                entry.is_none()
                    || index
                        .entry_by_path(tracked.into())
                        .is_some_and(|entry| entry.flags.contains(Flags::SKIP_WORKTREE)),
            );
        }
        // A directory is what a recorded gitlink looks like on disk; any other
        // non-file shape under a tracked entry diverged from the index.
        Resolved::NotAFile(_) => {
            return Some(match entry {
                None => true,
                Some((mode, _)) => {
                    mode == Mode::COMMIT
                        && matches!(snapshot.worktree_entry(path), WorktreeEntry::Directory)
                }
            });
        }
        Resolved::Unresolvable(_) => return None,
    };
    let Some((entry_mode, entry_id)) = entry else {
        // Present and untracked: only an ignored path is consistent with the
        // clean gate the snapshot took.
        let mut excludes = repo
            .excludes(
                index,
                None,
                gix::worktree::stack::state::ignore::Source::WorktreeThenIdMappingIfNotSkipped,
            )
            .ok()?;
        let platform = excludes
            .at_entry(tracked, Some(gix::index::entry::Mode::FILE))
            .ok()?;
        return Some(platform.is_excluded());
    };
    // A chmod alone moves git's mode between 100644 and 100755 and counts as
    // a modification where the filesystem tracks the bit, so the mode is
    // compared before the bytes.
    let capabilities = repo.filesystem_options().ok()?;
    let observed = if executable { "exec" } else { "file" };
    if !tracked_mode_matches(entry_mode, observed, capabilities) {
        return Some(false);
    }
    let ConfigRead::Content(content) = cache.read(snapshot, path) else {
        return None;
    };
    let blob = gix::objs::compute_hash(
        repo.object_hash(),
        gix::objs::Kind::Blob,
        content.text.as_bytes(),
    )
    .ok()?;
    // Raw bytes first; a `text eol=crlf` file only matches after the
    // conversion git applies on the way into the index.
    Some(
        blob == entry_id
            || normalized_blob_id_in(repo, index, tracked, content.text.as_bytes())
                == Some(entry_id),
    )
}
