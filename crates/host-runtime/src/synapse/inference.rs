//! This CPU-only backend owns dynamic ORT initialization and performs a structural startup probe and semantic certification against the bundle corpus.

#[cfg(target_os = "linux")]
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use fastembed::{
    InitOptionsUserDefined, OutputKey, Pooling, QuantizationMode, TextEmbedding, TokenizerFiles,
    UserDefinedEmbeddingModel,
};

use super::bundle::{Corpus, CorpusItem, SelectedOutput, VerifiedBundle};
#[cfg(target_os = "linux")]
use super::bundle::{OpenRegularFileError, open_regular_file, validate_sha256_hex};

/// CPU ONNX Runtime contains executable code and static runtime tables, not model weights.
/// The 512 MiB limit bounds the verification source buffer and sealed memfd copy to 1 GiB.
const MAX_ORT_LIBRARY_BYTES: u64 = 512 * 1024 * 1024;

/// `OrtIdentity` binds a library path to certified SHA-256 bytes, so `ensure_ort` rejects a different build before calling `ort::init_from`.
#[derive(Debug, Clone)]
pub struct OrtIdentity {
    pub library: PathBuf,
    pub sha256: String,
}

/// `Input` errors reject the affected request as a caller fault: tokenization failures and zero-token texts.
/// `Execution` errors are native runtime failures over one call (ORT session or tensor faults); the model stays usable, so the request is retryable and the lane keeps serving.
/// `Artifact` errors disable the component.
/// `Invariant` errors mark the component failing and prevent suspect vectors from being returned; only the dimension, finiteness, and norm postconditions raise them.
#[derive(Debug, Clone)]
pub enum InferenceError {
    Input(String),
    Execution(String),
    Artifact(String),
    Invariant(String),
}

impl std::fmt::Display for InferenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Input(reason) => write!(f, "invalid inference input: {reason}"),
            Self::Execution(reason) => write!(f, "inference execution failure: {reason}"),
            Self::Artifact(reason) => write!(f, "inference artifact failure: {reason}"),
            Self::Invariant(reason) => write!(f, "inference invariant failure: {reason}"),
        }
    }
}

impl std::error::Error for InferenceError {}

/// `ORT_COMMITTED` permits one process-global ORT identity because dynamic loading is first-wins; its mutex lets only one racing initializer commit.
static ORT_COMMITTED: Mutex<Option<OrtIdentity>> = Mutex::new(None);

#[cfg(target_os = "linux")]
struct VerifiedOrtLibrary {
    file: std::fs::File,
}

#[cfg(target_os = "linux")]
impl VerifiedOrtLibrary {
    fn load_path(&self) -> PathBuf {
        use std::os::fd::AsRawFd;

        PathBuf::from(format!("/proc/self/fd/{}", self.file.as_raw_fd()))
    }

    /// `ort::init_from` loads a library at most once per process and reports success even when an earlier load won, so the memfd mapping is the evidence that the certified bytes are the ones executing.
    fn is_mapped(&self) -> Result<bool, InferenceError> {
        use std::os::unix::fs::MetadataExt;

        let metadata = self
            .file
            .metadata()
            .map_err(|_| InferenceError::Artifact("ONNX Runtime memfd stat failed".to_owned()))?;
        let maps = std::fs::read_to_string("/proc/self/maps")
            .map_err(|_| InferenceError::Artifact("process memory map is unreadable".to_owned()))?;
        Ok(memfd_inode_is_mapped(&maps, metadata.dev(), metadata.ino()))
    }
}

/// `/proc/self/maps` lines begin with `address perms offset dev inode`; a pathname, when present, follows.
/// `dev` is `major:minor` in hex; inode numbers are unique only within one device, so both must match the memfd's `st_dev` and `st_ino`.
#[cfg(target_os = "linux")]
fn memfd_inode_is_mapped(maps: &str, dev: u64, inode: u64) -> bool {
    let (major, minor) = (rustix::fs::major(dev), rustix::fs::minor(dev));
    maps.lines().any(|line| {
        let mut fields = line.split_whitespace();
        let mapped_dev = fields.nth(3).and_then(|field| {
            let (major, minor) = field.split_once(':')?;
            Some((
                u32::from_str_radix(major, 16).ok()?,
                u32::from_str_radix(minor, 16).ok()?,
            ))
        });
        let mapped_inode = fields.next().and_then(|field| field.parse::<u64>().ok());
        let pathname = fields.next();
        mapped_dev == Some((major, minor))
            && mapped_inode == Some(inode)
            && pathname.is_some_and(|path| path.starts_with("/memfd:"))
    })
}

#[cfg(target_os = "linux")]
fn ensure_ort(identity: &OrtIdentity) -> Result<(), InferenceError> {
    // An invalid `identity` reports a verification error even after another initializer commits a different identity.
    let verified = verify_ort_library(identity)?;
    let mut committed = ORT_COMMITTED
        .lock()
        .map_err(|_| InferenceError::Invariant("ORT init state is poisoned".to_owned()))?;
    if let Some(existing) = committed.as_ref() {
        if existing.library == identity.library && existing.sha256 == identity.sha256 {
            return Ok(());
        }
        return Err(InferenceError::Artifact(
            "a different ONNX Runtime identity is already committed".to_owned(),
        ));
    }
    // `encode_batch` defaults to a process-wide rayon pool; the `cpu` semaphore in `super` admits one native call at a time, so tokenization stays on the calling thread.
    // commentlint: allow(JUDGE)
    tokenizers::utils::parallelism::set_parallelism(false);
    let builder = ort::init_from(verified.load_path())
        .map_err(|_| InferenceError::Artifact("ONNX Runtime library failed to load".to_owned()))?;
    // A library loaded before this call (for example from `ORT_DYLIB_PATH`) wins the loader's first-load slot; `init_from` does not report that, so the memfd mapping is checked directly.
    if !verified.is_mapped()? {
        return Err(InferenceError::Artifact(
            "an uncertified ONNX Runtime library was already loaded".to_owned(),
        ));
    }
    if !builder.commit() {
        // `ort` was initialized before the certified library could be selected.
        return Err(InferenceError::Artifact(
            "ONNX Runtime environment was already initialized".to_owned(),
        ));
    }
    *committed = Some(identity.clone());
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn ensure_ort(_identity: &OrtIdentity) -> Result<(), InferenceError> {
    Err(InferenceError::Artifact(
        "secure ONNX Runtime staging requires Linux".to_owned(),
    ))
}

#[cfg(target_os = "linux")]
fn verify_ort_library(identity: &OrtIdentity) -> Result<VerifiedOrtLibrary, InferenceError> {
    validate_sha256_hex(&identity.sha256).map_err(|_| {
        InferenceError::Artifact("expected ONNX Runtime hash is not a real digest".to_owned())
    })?;
    let open = open_regular_file(&identity.library).map_err(|error| match error {
        OpenRegularFileError::Missing(errno) => {
            InferenceError::Artifact(format!("ONNX Runtime library is missing: {errno}"))
        }
        OpenRegularFileError::NotRegular => {
            InferenceError::Artifact("ONNX Runtime library is not a regular file".to_owned())
        }
    })?;
    if open.len > MAX_ORT_LIBRARY_BYTES {
        return Err(InferenceError::Artifact(
            "ONNX Runtime library exceeds the size bound".to_owned(),
        ));
    }
    let bytes = open
        .read()
        .map_err(|_| InferenceError::Artifact("ONNX Runtime library read failed".to_owned()))?;
    if super::protocol::sha256_hex(&bytes) != identity.sha256 {
        return Err(InferenceError::Artifact(
            "ONNX Runtime library hash mismatch".to_owned(),
        ));
    }

    let flags = rustix::fs::MemfdFlags::CLOEXEC
        | rustix::fs::MemfdFlags::ALLOW_SEALING
        | rustix::fs::MemfdFlags::EXEC;
    let fd = rustix::fs::memfd_create("host-onnxruntime", flags).or_else(|error| {
        if error == rustix::io::Errno::INVAL {
            // Linux before MFD_EXEC treats memfds as executable by default.
            rustix::fs::memfd_create(
                "host-onnxruntime",
                rustix::fs::MemfdFlags::CLOEXEC | rustix::fs::MemfdFlags::ALLOW_SEALING,
            )
        } else {
            Err(error)
        }
    });
    // The errno separates a host policy denial (`EACCES` under `vm.memfd_noexec=2`) from descriptor exhaustion.
    let mut file = std::fs::File::from(fd.map_err(|errno| {
        InferenceError::Artifact(format!(
            "ONNX Runtime memfd creation failed: {errno} ({})",
            errno.raw_os_error()
        ))
    })?);
    file.write_all(&bytes)
        .map_err(|_| InferenceError::Artifact("ONNX Runtime memfd write failed".to_owned()))?;
    drop(bytes);
    rustix::fs::fcntl_add_seals(
        &file,
        rustix::fs::SealFlags::SHRINK
            | rustix::fs::SealFlags::GROW
            | rustix::fs::SealFlags::WRITE
            | rustix::fs::SealFlags::SEAL,
    )
    .map_err(|_| InferenceError::Artifact("ONNX Runtime memfd sealing failed".to_owned()))?;
    Ok(VerifiedOrtLibrary { file })
}

/// The model mutex serializes `TextEmbedding::embed` because it requires `&mut`; the CPU permit prevents callers from queueing on that mutex.
/// Shortens `text` to at most `max_bytes` without splitting a character.
fn truncate_to_char_boundary(text: &mut String, max_bytes: usize) {
    let mut end = text.len().min(max_bytes);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
}

/// The served-vector contract every engine must meet: `dims` finite components with an L2 norm within `UNIT_NORM_TOLERANCE` of 1.
/// Accumulating in f64 keeps summation roundoff below the tolerance at `MAX_DIMS`; an f32 sum can drift past it and fail a correctly normalized vector.
pub(crate) fn validate_unit_vector(dims: usize, vector: &[f32]) -> Result<(), String> {
    if vector.len() != dims {
        return Err(format!(
            "vector has {} dimensions, manifest requires {dims}",
            vector.len()
        ));
    }
    if vector.iter().any(|v| !v.is_finite()) {
        return Err("vector contains a non-finite component".to_owned());
    }
    let norm = vector
        .iter()
        .map(|v| f64::from(*v).powi(2))
        .sum::<f64>()
        .sqrt();
    if (norm - 1.0).abs() > super::bundle::UNIT_NORM_TOLERANCE {
        return Err("vector is not L2-normalized".to_owned());
    }
    Ok(())
}

pub struct Backend {
    model: Mutex<TextEmbedding>,
    dims: usize,
    /// The tokenizer truncates every text to this many tokens.
    max_tokens: usize,
    /// When the tokenizer's post-processor adds special tokens, every encoding is non-empty and the per-text zero-token pass is skipped.
    zero_token_inputs_possible: bool,
}

impl Backend {
    pub fn load(bundle: VerifiedBundle, ort: &OrtIdentity) -> Result<Self, InferenceError> {
        ensure_ort(ort)?;

        let VerifiedBundle {
            manifest,
            max_text_bytes,
            max_batch_text_bytes,
            certification_rows,
            onnx,
            initializers,
            tokenizer_file,
            config_file,
            special_tokens_map_file,
            tokenizer_config_file,
            corpus,
        } = bundle;
        let pooling = match manifest.pooling.as_str() {
            "mean" => Pooling::Mean,
            "cls" => Pooling::Cls,
            other => {
                return Err(InferenceError::Artifact(format!(
                    "unsupported pooling: {other}"
                )));
            }
        };
        let quantization = match manifest.quantization.as_str() {
            "none" => QuantizationMode::None,
            "static" => QuantizationMode::Static,
            "dynamic" => QuantizationMode::Dynamic,
            other => {
                return Err(InferenceError::Artifact(format!(
                    "unsupported quantization: {other}"
                )));
            }
        };
        let output_key = match manifest
            .selected_output()
            .map_err(|e| InferenceError::Artifact(e.0))?
        {
            SelectedOutput::OnlyOne => OutputKey::OnlyOne,
            SelectedOutput::ByOrder(index) => OutputKey::ByOrder(index),
            SelectedOutput::ByName(name) => OutputKey::ByName(name),
        };

        let mut model = UserDefinedEmbeddingModel::new(
            onnx,
            TokenizerFiles {
                tokenizer_file,
                config_file,
                special_tokens_map_file,
                tokenizer_config_file,
            },
        )
        .with_pooling(pooling)
        .with_quantization(quantization);
        for (name, buffer) in initializers {
            model = model.with_external_initializer(name, buffer);
        }
        model.output_key = Some(output_key);

        let options = InitOptionsUserDefined::new()
            .with_max_length(manifest.max_tokens as usize)
            // One intra-op thread matches the single CPU inference permit.
            .with_intra_threads(1);
        let embedder = TextEmbedding::try_new_from_user_defined(model, options)
            .map_err(|e| InferenceError::Artifact(format!("model construction failed: {e}")))?;

        // The post-processor adds its special tokens independent of the content, so the empty probe is the minimum encoding length for any text. commentlint: allow(JUDGE)
        let zero_token_inputs_possible = embedder
            .tokenizer
            .encode("", true)
            .map_err(|_| {
                InferenceError::Artifact("tokenizer failed to encode the empty probe".to_owned())
            })?
            .get_ids()
            .is_empty();

        let backend = Self {
            model: Mutex::new(embedder),
            dims: manifest.dims as usize,
            max_tokens: manifest.max_tokens as usize,
            zero_token_inputs_possible,
        };
        backend.structural_probe()?;
        backend.certify(&corpus, certification_rows, max_batch_text_bytes)?;
        backend.long_input_probe(
            &corpus,
            max_text_bytes,
            certification_rows,
            max_batch_text_bytes,
        )?;
        Ok(backend)
    }

    /// embed blocks while running native inference over one ordered page of texts.
    /// embed returns one finite, unit-norm vector with `dims` components for each input text.
    pub fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, InferenceError> {
        if texts.is_empty() {
            return Err(InferenceError::Input("no texts to embed".to_owned()));
        }
        let mut model = self
            .model
            .lock()
            .map_err(|_| InferenceError::Invariant("inference state is poisoned".to_owned()))?;
        for text in texts {
            if text.is_empty() {
                return Err(InferenceError::Input("text is empty".to_owned()));
            }
            if !self.zero_token_inputs_possible {
                continue;
            }
            let encoding = model
                .tokenizer
                .encode(*text, true)
                .map_err(|_| InferenceError::Input("text failed to tokenize".to_owned()))?;
            // embed rejects zero-token inputs because mean pooling would divide by an all-zero attention mask.
            if encoding.get_ids().is_empty() {
                return Err(InferenceError::Input(
                    "text tokenizes to zero tokens".to_owned(),
                ));
            }
        }
        // Tokenizer faults are caller input; everything else is a native runtime fault that must not be reported as a schema violation, or callers would never retry it.
        let vectors = model.embed(texts, None).map_err(|error| match error {
            fastembed::Error::Tokenization(_) | fastembed::Error::EmptyTokenizations => {
                InferenceError::Input(format!("inference rejected the input: {error}"))
            }
            other => InferenceError::Execution(format!("inference failed: {other}")),
        })?;
        drop(model);
        if vectors.len() != texts.len() {
            return Err(InferenceError::Invariant(
                "inference returned a different item count".to_owned(),
            ));
        }
        for vector in &vectors {
            self.validate_vector(vector)?;
        }
        Ok(vectors)
    }

    fn validate_vector(&self, vector: &[f32]) -> Result<(), InferenceError> {
        validate_unit_vector(self.dims, vector).map_err(InferenceError::Invariant)
    }

    fn structural_probe(&self) -> Result<(), InferenceError> {
        let vectors = self.embed(&["structural probe"])?;
        if vectors.len() != 1 {
            return Err(InferenceError::Artifact(
                "structural probe returned a wrong item count".to_owned(),
            ));
        }
        Ok(())
    }

    /// certify uses a corpus that detects incorrect output selection, pooling, and truncation.
    /// certify rejects structurally healthy models with semantically incorrect output.
    /// load rejects semantically wrong models before returning a backend that can serve vectors.
    /// `batch_rows` is the size of the multi-row check: the largest batch the host admits, as sized by `load_bundle`.
    /// `max_batch_text_bytes` bounds every certification batch's aggregate text the way the routed parser bounds a request, so certification never executes a workload the lane would refuse.
    fn certify(
        &self,
        corpus: &Corpus,
        batch_rows: usize,
        max_batch_text_bytes: usize,
    ) -> Result<(), InferenceError> {
        let matches = |got: &[f32], item: &CorpusItem| {
            !super::bundle::certification_mismatch(got, &item.expected, corpus.tolerance)
        };
        for item in &corpus.items {
            let got = self.embed(&[item.text.as_str()])?;
            if !matches(&got[0], item) {
                return Err(InferenceError::Artifact(
                    "semantic certification failed".to_owned(),
                ));
            }
        }
        // Routed batches pass many items to one backend call. A graph with a fixed or row-permuting batch dimension passes the singleton checks above, so multi-row calls are certified as well.
        // The attribution call uses only pairwise-distinct expectations, so any permutation of its rows changes some row's output and is caught; repeated expectations would let a permutation among equal rows pass unseen.
        // The set is seeded with a mismatching pair so it always holds two rows, then grows greedily up to the admitted batch size.
        let differ = |a: &CorpusItem, b: &CorpusItem| {
            super::bundle::certification_mismatch(&a.expected, &b.expected, corpus.tolerance)
        };
        let mut distinct: Vec<&CorpusItem> = corpus
            .items
            .iter()
            .enumerate()
            .find_map(|(index, first)| {
                corpus.items[index + 1..]
                    .iter()
                    .find(|second| differ(first, second))
                    .map(|second| vec![first, second])
            })
            .unwrap_or_else(|| vec![&corpus.items[0]]);
        for item in &corpus.items {
            if distinct.len() >= batch_rows.max(1) {
                break;
            }
            if distinct.iter().all(|kept| differ(kept, item)) {
                distinct.push(item);
            }
        }
        // A lane that admits one item per batch is never asked for two rows, so its certification stays within that contract; the aggregate cap trims the batch the same way.
        distinct.truncate(batch_rows.max(1));
        while distinct.len() > 1
            && distinct.iter().map(|item| item.text.len()).sum::<usize>() > max_batch_text_bytes
        {
            distinct.pop();
        }
        if distinct.len() >= 2 {
            self.certify_batch(&distinct, &matches)?;
        }
        // Full-size batches certify every admitted position with only two distinct items: batch `k` labels each position by bit `k` of its index and its complement swaps the labels, so any two positions differ in some batch and every position sees both items. A graph that mixes rows at any pair of positions, or mishandles one input at one position, produces a mismatch. The same batches are the capacity check, so a graph that fails only at the admitted size is caught too.
        // The labeled batches use the shortest mismatching pair; `load_bundle` requires that pair to fit `batch_rows` rows under the aggregate cap, so every admitted position is exercised by a request the lane would admit.
        let (first, second) =
            super::bundle::shortest_mismatching_pair(corpus).unwrap_or((distinct[0], distinct[0]));
        let per_row = first.text.len().max(second.text.len()).max(1);
        let rows = batch_rows.max(1).min(max_batch_text_bytes / per_row).max(1);
        debug_assert_eq!(
            rows,
            batch_rows.max(1),
            "load_bundle admits only certifiable corpora"
        );
        if rows > 1 {
            for bit in 0..usize::BITS - (rows - 1).leading_zeros() {
                for complement in [false, true] {
                    let labeled: Vec<&CorpusItem> = (0..rows)
                        .map(|position| {
                            if ((position >> bit) & 1 == 0) != complement {
                                first
                            } else {
                                second
                            }
                        })
                        .collect();
                    self.certify_batch(&labeled, &matches)?;
                }
            }
        }
        Ok(())
    }

    /// Corpus texts are short, so a graph with a fixed short sequence axis passes every semantic probe and then fails an ordinary long request. This probe embeds a text at the advertised byte limit that reaches the truncation window; the result must be a valid vector, with no expectation to compare.
    /// Corpus phrases can tokenize sparsely, so every candidate's encoding is measured and the densest one is embedded; a lane whose candidates all fall short of `max_tokens` is not published.
    /// The window is then exercised jointly with the batch axis: batched tokenization pads every row to the longest sequence, so the long text is embedded once more in the largest legal batch alongside short corpus items, whose rows must still match their expectations under that padding.
    fn long_input_probe(
        &self,
        corpus: &Corpus,
        max_text_bytes: usize,
        batch_rows: usize,
        max_batch_text_bytes: usize,
    ) -> Result<(), InferenceError> {
        let mut text = String::new();
        for item in corpus.items.iter().cycle() {
            if text.len() >= max_text_bytes {
                break;
            }
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(&item.text);
        }
        truncate_to_char_boundary(&mut text, max_text_bytes);
        // Candidates that tokenize densely for common vocabularies: isolated ASCII letters, isolated digits, and CJK punctuation; the probe is the candidate with the most tokens, so it reaches the longest sequence a request can send.
        let mut best = (self.token_count(&text)?, text);
        if best.0 < self.max_tokens {
            for pattern in ["x ", "7 ", "\u{3002}"] {
                let mut candidate = pattern.repeat(max_text_bytes / pattern.len() + 1);
                truncate_to_char_boundary(&mut candidate, max_text_bytes);
                let tokens = self.token_count(&candidate)?;
                if tokens > best.0 {
                    best = (tokens, candidate);
                }
                if best.0 >= self.max_tokens {
                    break;
                }
            }
        }
        // The lane is published only when the window was exercised: truncation caps every request at `max_tokens`, so a probe that reaches it covers the longest sequence any request can produce, while a shortfall leaves the advertised axis unverified.
        if best.0 < self.max_tokens {
            return Err(InferenceError::Artifact(format!(
                "no probe within {max_text_bytes} bytes reaches the advertised max_tokens ({}); the longest reached {} tokens; raise max_text_bytes or lower the bundle's max_tokens",
                self.max_tokens, best.0
            )));
        }
        let text = best.1;
        let vectors = self.embed(&[text.as_str()])?;
        if vectors.len() != 1 {
            return Err(InferenceError::Artifact(
                "long-input probe returned a wrong item count".to_owned(),
            ));
        }
        // `[batch_rows, max_tokens]` is the largest tensor a legal batch can produce: one text that reaches the window padded against short items, within the aggregate cap. That shape is legal when a window-reaching text fits in the bytes left beside `batch_rows - 1` shortest items, so that byte budget is measured directly; when no examined prefix within it reaches the window, the largest shape this candidate supports is probed instead.
        let short = corpus
            .items
            .iter()
            .min_by_key(|item| item.text.len())
            .expect("corpus parsing requires items");
        let batch_rows = batch_rows.max(1);
        let short_len = short.text.len().max(1);
        let long_budget = max_batch_text_bytes
            .saturating_sub((batch_rows - 1).saturating_mul(short_len))
            .min(text.len());
        let (text, rows) = match self.window_prefix_within(&text, long_budget)? {
            Some(end) => (&text[..end], batch_rows),
            None => {
                let remaining = max_batch_text_bytes.saturating_sub(text.len());
                (text.as_str(), batch_rows.min(1 + remaining / short_len))
            }
        };
        if rows > 1 {
            let mut texts = vec![text];
            texts.extend(std::iter::repeat_n(short.text.as_str(), rows - 1));
            let rows_out = self.embed(&texts)?;
            let short_rows_match = rows_out.len() == texts.len()
                && rows_out[1..].iter().all(|row| {
                    !super::bundle::certification_mismatch(row, &short.expected, corpus.tolerance)
                });
            if !short_rows_match {
                return Err(InferenceError::Artifact(
                    "long-input batch certification failed".to_owned(),
                ));
            }
        }
        Ok(())
    }

    /// End of a non-empty char-boundary prefix of `text` no longer than `budget` bytes that reaches `max_tokens`, or `None` when none of the examined prefixes does.
    /// Token counts are not monotone in prefix length: the character after a boundary can merge with the characters before it and lower the count at that boundary. The longest prefix within the budget is measured first, then the boundaries just below it, which covers a merge at the cut without scanning every boundary of a megabyte-scale candidate.
    fn window_prefix_within(
        &self,
        text: &str,
        budget: usize,
    ) -> Result<Option<usize>, InferenceError> {
        const BOUNDARIES_BELOW_BUDGET: usize = 8;
        let mut end = budget.min(text.len());
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        for _ in 0..=BOUNDARIES_BELOW_BUDGET {
            if end == 0 {
                return Ok(None);
            }
            if self.token_count(&text[..end])? >= self.max_tokens {
                return Ok(Some(end));
            }
            end -= 1;
            while end > 0 && !text.is_char_boundary(end) {
                end -= 1;
            }
        }
        Ok(None)
    }

    /// Tokens in `text` after the tokenizer's truncation, so a count equal to `max_tokens` means the text reached the window.
    fn token_count(&self, text: &str) -> Result<usize, InferenceError> {
        let model = self
            .model
            .lock()
            .map_err(|_| InferenceError::Invariant("inference state is poisoned".to_owned()))?;
        model
            .tokenizer
            .encode(text, true)
            .map(|encoding| encoding.get_ids().len())
            .map_err(|_| {
                InferenceError::Artifact("tokenizer failed to encode the probe".to_owned())
            })
    }

    fn certify_batch(
        &self,
        batch: &[&CorpusItem],
        matches: &impl Fn(&[f32], &CorpusItem) -> bool,
    ) -> Result<(), InferenceError> {
        let texts: Vec<&str> = batch.iter().map(|item| item.text.as_str()).collect();
        let rows = self.embed(&texts)?;
        if rows.len() != batch.len()
            || !rows.iter().zip(batch).all(|(row, item)| matches(row, item))
        {
            return Err(InferenceError::Artifact(
                "multi-row semantic certification failed".to_owned(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn a_memfd_mapping_is_recognized_only_by_device_inode_and_memfd_path() {
        let maps = "\
7f0000000000-7f0000001000 r--p 00000000 00:1d 4242 /memfd:host-onnxruntime (deleted)
7f0000001000-7f0000002000 r-xp 00001000 fd:01 4242 /usr/lib/libonnxruntime.so
7f0000002000-7f0000003000 rw-p 00000000 00:00 0
7f0000003000-7f0000004000 r--p 00000000 00:1d 99 /memfd:other (deleted)
7f0000004000-7f0000005000 r--p 00000000 00:1e 4242 /memfd:elsewhere (deleted)
";
        let memfd_dev = rustix::fs::makedev(0, 0x1d);
        assert!(memfd_inode_is_mapped(maps, memfd_dev, 4242));
        assert!(memfd_inode_is_mapped(maps, memfd_dev, 99));
        // A memfd mapping with the same inode number on another device does not count.
        assert!(!memfd_inode_is_mapped(
            "7f0000004000-7f0000005000 r--p 00000000 00:1e 4242 /memfd:elsewhere (deleted)\n",
            memfd_dev,
            4242
        ));
        assert!(memfd_inode_is_mapped(
            maps,
            rustix::fs::makedev(0, 0x1e),
            4242
        ));
        // A regular-file mapping with the same inode number does not count.
        assert!(!memfd_inode_is_mapped(
            "7f0000001000-7f0000002000 r-xp 00001000 fd:01 4242 /usr/lib/libonnxruntime.so\n",
            memfd_dev,
            4242
        ));
        assert!(!memfd_inode_is_mapped(
            "7f0000001000-7f0000002000 r-xp 00001000 fd:01 4242 /usr/lib/libonnxruntime.so\n",
            rustix::fs::makedev(0xfd, 1),
            4242
        ));
        assert!(!memfd_inode_is_mapped(maps, memfd_dev, 4243));
        assert!(!memfd_inode_is_mapped(maps, memfd_dev, 0));
        assert!(!memfd_inode_is_mapped("", memfd_dev, 4242));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn an_unloaded_verified_library_is_not_mapped() {
        let source_dir = tempfile::tempdir().expect("source directory");
        let source = source_dir.path().join("libonnxruntime.so");
        let bytes = b"certified but never dlopened";
        std::fs::write(&source, bytes).expect("write source");
        let identity = OrtIdentity {
            library: source,
            sha256: super::super::protocol::sha256_hex(bytes),
        };
        let verified = verify_ort_library(&identity).expect("verified staging");
        assert!(!verified.is_mapped().expect("maps readable"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn source_replacement_cannot_change_verified_loader_bytes() {
        let source_dir = tempfile::tempdir().expect("source directory");
        let source = source_dir.path().join("libonnxruntime.so");
        let replacement = source_dir.path().join("replacement.so");
        let verified_bytes = b"certified ONNX Runtime bytes";
        let replacement_bytes = b"unverified replacement bytes";
        std::fs::write(&source, verified_bytes).expect("write source");
        let identity = OrtIdentity {
            library: source.clone(),
            sha256: super::super::protocol::sha256_hex(verified_bytes),
        };
        let verified = verify_ort_library(&identity).expect("verified staging");
        let seals = rustix::fs::fcntl_get_seals(&verified.file).expect("read memfd seals");
        assert!(seals.contains(
            rustix::fs::SealFlags::SHRINK
                | rustix::fs::SealFlags::GROW
                | rustix::fs::SealFlags::WRITE
                | rustix::fs::SealFlags::SEAL
        ));
        let mut writer = verified.file.try_clone().expect("clone memfd");
        assert!(writer.write_all(b"replacement").is_err());

        std::fs::write(&replacement, replacement_bytes).expect("write replacement");
        std::fs::rename(&replacement, &source).expect("replace source");

        let loaded_path = verified.load_path().to_path_buf();
        assert_ne!(loaded_path, source);
        assert!(loaded_path.starts_with("/proc/self/fd"));
        let loaded_bytes = std::fs::read(&loaded_path).expect("read loader path");
        assert_eq!(loaded_bytes, verified_bytes);
        assert_eq!(
            super::super::protocol::sha256_hex(&loaded_bytes),
            identity.sha256
        );

        assert_eq!(
            std::fs::read(&source).expect("read replaced source"),
            replacement_bytes
        );

        drop(verified);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn oversized_sparse_ort_library_fails_before_reading_or_allocating_its_length() {
        let source_dir = tempfile::tempdir().expect("source directory");
        let source = source_dir.path().join("oversized-libonnxruntime.so");
        std::fs::File::create(&source)
            .expect("create sparse library")
            .set_len(MAX_ORT_LIBRARY_BYTES + 1)
            .expect("size sparse library");
        let identity = OrtIdentity {
            library: source,
            sha256: super::super::protocol::sha256_hex(b"unread oversized library"),
        };

        let error = match verify_ort_library(&identity) {
            Err(error) => error,
            Ok(_) => panic!("oversized library is accepted"),
        };
        match error {
            InferenceError::Artifact(reason) => assert!(
                reason.contains("size bound"),
                "reason {reason:?} does not identify the descriptor-length bound"
            ),
            other => panic!("expected artifact error, got {other}"),
        }
    }
}
