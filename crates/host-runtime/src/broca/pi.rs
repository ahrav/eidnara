//!
//! Pi runs print-mode JSON without a persisted session or tools.
//! Pi stores the system prompt in a private `0600` file.
//! Pi maps canonical provider names to Pi prefixes and accepts an optional thinking level.
//! Pi disables extension discovery with `--no-approve --no-extensions`.
//! Pi loads only daemon-owned provider extensions in descriptor order.
//! Pi loads the bundled Broca payload hook last so it owns the final generation contract.
//! The Eidnara Pi recursion guard suppresses full plugin startup in the child.

use std::ffi::OsString;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::backend::{
    self, BackendError, BackendEvent, BackendFuture, BackendRequest, BackendTerminal, ErrorClass,
    EventSink, FinishReason, Harness, LlmExecutionBackend,
};
use super::config::MAX_PI_PROVIDER_EXTENSIONS;
use super::subprocess::group_registry::StateRoot;
use super::subprocess::{
    self, EnvSnapshot, PrivateDir, ProbeSignal, SubprocessLimits, SubprocessSpec,
};
use crate::harness_closure::ValidatedHarnessClosure;

/// The Eidnara Pi recursion guard causes the plugin to return before full extension registration.
/// extension registration).
pub const EIDNARA_PI_SUBAGENT_ENV: &str = "EIDNARA_PI_SUBAGENT";

/// PiBackend passes requested generation values to the bundled payload hook.
/// The variables carry only bounded numbers, never requested text.
pub const BROCA_MAX_OUTPUT_TOKENS_ENV: &str = "EIDNARA_BROCA_MAX_OUTPUT_TOKENS";
pub const BROCA_TEMPERATURE_ENV: &str = "EIDNARA_BROCA_TEMPERATURE";

/// Pi materializes the hook per run in an owner-only temporary file and loads it last.
/// explicit extension.
pub const PI_BROCA_EXTENSION_BYTES: &[u8] = include_bytes!("../../assets/pi-broca-extension.mjs");

/// Pi materializes the hook in each run's private directory under this name.
pub const PI_BROCA_EXTENSION_FILE: &str = "pi-broca-extension.mjs";

/// The daemon retains a trusted Pi runtime closure.
/// Node names resolve only inside the content-addressed closure.
#[derive(Clone, Debug)]
pub struct PiRuntimeDescriptor {
    pub closure: Arc<ValidatedHarnessClosure>,
    pub interpreter_node: String,
    pub entrypoint_node: String,
    pub provider_extension_nodes: Vec<String>,
}

pub struct PiBackend {
    descriptor: PiRuntimeDescriptor,
    /// Pi receives `--thinking <level>` only when `thinking_level` is set; otherwise Pi resolves the level.
    /// resolution runs.
    thinking_level: Option<String>,
    limits: SubprocessLimits,
    env: EnvSnapshot,
    state_root: StateRoot,
}

impl PiBackend {
    pub fn new(descriptor: PiRuntimeDescriptor, env: EnvSnapshot, state_root: StateRoot) -> Self {
        Self::with_limits(
            descriptor,
            env,
            state_root,
            None,
            SubprocessLimits::default(),
        )
    }

    pub fn with_limits(
        descriptor: PiRuntimeDescriptor,
        env: EnvSnapshot,
        state_root: StateRoot,
        thinking_level: Option<String>,
        limits: SubprocessLimits,
    ) -> Self {
        Self {
            descriptor,
            thinking_level,
            limits,
            env,
            state_root,
        }
    }
}

impl LlmExecutionBackend for PiBackend {
    fn execute(
        &self,
        request: BackendRequest,
        events: EventSink,
        cancel: CancellationToken,
    ) -> BackendFuture {
        // A run bound to another harness must fail, not silently execute
        // PiBackend must not execute a non-Pi request with Pi provider aliases or credentials.
        if request.harness != Harness::Pi {
            let terminal = backend::harness_mismatch(Harness::Pi, request.harness);
            return Box::pin(async move { terminal });
        }
        if events.emit(BackendEvent::HarnessDispatch {
            harness: Harness::Pi,
        }) == backend::SinkStatus::Closed
        {
            let terminal = backend::dispatch_closed(Harness::Pi);
            return Box::pin(async move { terminal });
        }
        let descriptor = self.descriptor.clone();
        let thinking_level = self.thinking_level.clone();
        let limits = self.limits.clone();
        let env = self.env.clone();
        let state_root = self.state_root.clone();
        Box::pin(run_pi_with_provider_fallback(
            PiRun {
                descriptor,
                thinking_level,
                limits,
                env,
                state_root,
                deadline: None,
            },
            request,
            events,
            cancel,
        ))
    }

    /// Resolving every node `run_pi` needs re-proves the same closure invariants, so a rejected send and a failed run report the same subreason.
    fn unavailable_reason(&self, harness: Harness) -> Option<&'static str> {
        if harness != Harness::Pi {
            return None;
        }
        // `run_pi` holds every resolved extension descriptor at once, so an oversized set fails the run; a rejected send must report the same subreason.
        if self.descriptor.provider_extension_nodes.len() > MAX_PI_PROVIDER_EXTENSIONS {
            return Some("extension_budget_exceeded");
        }
        let closure = &self.descriptor.closure;
        let mut required = [
            &self.descriptor.interpreter_node,
            &self.descriptor.entrypoint_node,
        ]
        .into_iter()
        .chain(self.descriptor.provider_extension_nodes.iter());
        required
            .any(|node| closure.resolve_node_descriptor(node).is_err())
            .then_some("closure_incomplete")
    }
}

/// Known auth-plugin aliases map to Pi provider prefixes.
/// Known auth-plugin aliases map to Pi provider prefixes; unknown providers pass through unchanged.
pub fn pi_model_ref(provider: &str, model: &str) -> String {
    let pi_provider = match provider {
        "openai" => "openai-codex",
        "google" => "google-antigravity",
        other => other,
    };
    format!("{pi_provider}/{model}")
}

/// The backend-owned inputs one Pi child invocation needs, separate from the per-request values.
#[derive(Clone)]
struct PiRun {
    descriptor: PiRuntimeDescriptor,
    thinking_level: Option<String>,
    limits: SubprocessLimits,
    env: EnvSnapshot,
    state_root: StateRoot,
    /// The retry shares the first attempt's wall-clock budget, so it carries an absolute end instant rather than a duration that setup time would escape.
    deadline: Option<tokio::time::Instant>,
}

/// Pi tries the aliased provider first and retries the canonical provider once after a credential failure.
/// Direct `openai` and `google` API-key users lack credentials under subscription-extension aliases.
/// Pi tries each provider at most once.
/// Both attempts share one wall-clock budget; the retry receives only the remainder.
/// A first-attempt cleanup failure blocks the retry so retry success cannot mask disk residue.
async fn run_pi_with_provider_fallback(
    run: PiRun,
    request: BackendRequest,
    events: EventSink,
    cancel: CancellationToken,
) -> BackendTerminal {
    let aliased = pi_model_ref(&request.provider, &request.model);
    let canonical = format!("{}/{}", request.provider, request.model);
    // Requests without an alias do not retry.
    // The no-alias path creates no clone, so it holds one prompt copy rather than two.
    if aliased == canonical {
        return run_pi(run, request, events, cancel, aliased).await;
    }
    let started = tokio::time::Instant::now();
    let first = run_pi(
        run.clone(),
        request.clone(),
        events.clone(),
        cancel.clone(),
        aliased,
    )
    .await;
    // An attempt that left private prompt material on disk must surface its cleanup failure.
    // A first-attempt cleanup failure blocks the retry.
    let credential_failure = matches!(
        &first,
        BackendTerminal::Failed(error) if error.class == ErrorClass::AuthRequired
            && !error.message.contains(subprocess::CLEANUP_FAILURE_MARKER)
    );
    if !credential_failure || cancel.is_cancelled() {
        return first;
    }
    let Some(remaining) = run.limits.run_timeout.checked_sub(started.elapsed()) else {
        return first;
    };
    if remaining.is_zero() {
        return first;
    }
    // The retry drops `first` before retrying to keep concurrent capture buffers within the declared headroom.
    drop(first);
    let budget_end = started + run.limits.run_timeout;
    let mut retry = run;
    retry.limits.run_timeout = remaining;
    retry.deadline = Some(budget_end);
    run_pi(retry, request, events, cancel, canonical).await
}

async fn run_pi(
    run: PiRun,
    request: BackendRequest,
    events: EventSink,
    cancel: CancellationToken,
    model_ref: String,
) -> BackendTerminal {
    let PiRun {
        descriptor,
        thinking_level,
        mut limits,
        env,
        state_root,
        deadline,
    } = run;
    let mut child_env = match env.provider_row("pi", &request.provider) {
        Ok(row) => row,
        Err(error) => {
            return subprocess::credential_failure(Harness::Pi, error);
        }
    };
    // The bound is checked before resolution so an oversized descriptor set opens no files.
    if descriptor.provider_extension_nodes.len() > MAX_PI_PROVIDER_EXTENSIONS {
        return subprocess::harness_unavailable_failure(Harness::Pi, "extension_budget_exceeded");
    }
    // Closure resolution precedes the private directory so an unavailable harness never writes the caller-private system prompt to disk.
    // Resolution opens and stats every node, so it runs on the blocking pool in one hop rather than on a runtime worker.
    let closure = Arc::clone(&descriptor.closure);
    let interpreter_node = descriptor.interpreter_node.clone();
    let entrypoint_node = descriptor.entrypoint_node.clone();
    let extension_nodes = descriptor.provider_extension_nodes.clone();
    let resolved = subprocess::off_runtime(move || {
        let interpreter = closure.resolve_node_descriptor(&interpreter_node).ok()?;
        let entrypoint = closure.resolve_node_descriptor(&entrypoint_node).ok()?;
        let mut extensions = Vec::with_capacity(extension_nodes.len());
        for node in &extension_nodes {
            extensions.push(closure.resolve_node_descriptor(node).ok()?);
        }
        Some((interpreter, entrypoint, extensions))
    })
    .await;
    let (interpreter, entrypoint, resolved_extensions) = match resolved {
        Ok(Some(resolved)) => resolved,
        Ok(None) => {
            return subprocess::harness_unavailable_failure(Harness::Pi, "closure_incomplete");
        }
        Err(err) => return subprocess::spawn_failure(Harness::Pi, &err),
    };

    let dir = match PrivateDir::create_async(state_root.clone(), "broca-pi").await {
        Ok(dir) => dir,
        Err(err) => return subprocess::spawn_failure(Harness::Pi, &err),
    };
    // `run_pi` writes the compiled-in hook bytes to a 0600 file in the per-run 0700 directory so no installed hook path can be swapped under the daemon.
    let hook_path = match dir
        .write_private_async(
            PI_BROCA_EXTENSION_FILE.to_owned(),
            PI_BROCA_EXTENSION_BYTES.to_vec(),
        )
        .await
    {
        Ok(path) => path,
        Err(err) => {
            return subprocess::merge_cleanup(
                subprocess::spawn_failure(Harness::Pi, &err),
                dir.cleanup_async().await,
            );
        }
    };
    // `run_pi` writes caller-private system prompts to a private 0600 file instead of passing them through argv.
    let system_prompt_path = match &request.system {
        None => None,
        Some(system) => match dir
            .write_private_async("system-prompt.txt".to_owned(), system.as_bytes().to_vec())
            .await
        {
            Ok(path) => Some(path),
            Err(err) => {
                return subprocess::merge_cleanup(
                    subprocess::spawn_failure(Harness::Pi, &err),
                    dir.cleanup_async().await,
                );
            }
        },
    };

    // `request.prompt` travels only over stdin, so `run_pi` passes no positional message.
    // Node resolves sibling modules relative to the entrypoint path, so the descriptor path must lead back to the closure tree.
    // Path arguments stay `OsString` end to end: a lossy UTF-8 conversion would rename a non-UTF-8 path before the child resolves it.
    let mut args: Vec<OsString> = vec![
        entrypoint.module_path().as_os_str().to_owned(),
        OsString::from("--print"),
        OsString::from("--mode"),
        OsString::from("json"),
        OsString::from("--no-session"),
        OsString::from("--no-skills"),
        OsString::from("--no-prompt-templates"),
        OsString::from("--no-context-files"),
        // The run exposes a closed tool surface: it maps the prompt to text.
        OsString::from("--no-tools"),
        // `--no-approve` treats project-local settings and extensions as untrusted for the run.
        OsString::from("--no-approve"),
        OsString::from("--no-extensions"),
    ];
    for extension in &resolved_extensions {
        args.push(OsString::from("--extension"));
        args.push(extension.module_path().as_os_str().to_owned());
    }
    args.push(OsString::from("--extension"));
    args.push(hook_path.into_os_string());
    if let Some(path) = &system_prompt_path {
        args.push(OsString::from("--system-prompt"));
        args.push(path.as_os_str().to_owned());
    }
    args.push(OsString::from("--model"));
    args.push(OsString::from(model_ref));
    if let Some(level) = &thinking_level {
        args.push(OsString::from("--thinking"));
        args.push(OsString::from(level.clone()));
    }

    child_env.push((OsString::from(EIDNARA_PI_SUBAGENT_ENV), OsString::from("1")));
    child_env.push((
        OsString::from(BROCA_MAX_OUTPUT_TOKENS_ENV),
        OsString::from(request.max_output_tokens.to_string()),
    ));
    child_env.push((
        OsString::from(BROCA_TEMPERATURE_ENV),
        OsString::from(request.temperature.to_string()),
    ));
    child_env.push((OsString::from("HOME"), dir.path().as_os_str().to_owned()));

    let spec = SubprocessSpec {
        executable: interpreter.path().to_path_buf(),
        args,
        env: child_env,
        working_dir: dir.path().to_path_buf(),
        // The runner moves the prompt because no code reads it after spec construction.
        stdin: request.prompt.into_bytes(),
        // The child does not inherit the closure directory descriptor because no argument references it.
        // Inheriting the closure directory descriptor would give the harness a rename-immune handle.
        // The harness could write through that handle into the validated closure tree.
        inherit_fds: std::iter::once(interpreter.inherited_fd())
            .chain(entrypoint.module_inherited_fd())
            .chain(
                resolved_extensions
                    .iter()
                    .filter_map(|extension| extension.module_inherited_fd()),
            )
            .collect(),
        state_root,
    };

    // The subprocess receives the deadline remaining after attempt setup.
    if let Some(deadline) = deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return subprocess::merge_cleanup(
                subprocess::budget_exhausted_failure(Harness::Pi),
                dir.cleanup_async().await,
            );
        }
        limits.run_timeout = remaining;
    }

    let result = match subprocess::run(spec, &limits, &cancel, Some(pi_terminal_probe)).await {
        Ok(result) => result,
        Err(err) => {
            return subprocess::merge_cleanup(
                subprocess::spawn_failure(Harness::Pi, &err),
                dir.cleanup_async().await,
            );
        }
    };

    let parsed = subprocess::parse_clean_transcript(&result, &events, parse_pi_transcript);
    let cleanup = dir.cleanup_async().await;
    subprocess::finalize(Harness::Pi, &result, parsed, &limits, cleanup)
}

/// The probe starts the drain grace after a terminal stdout event.
/// Pi can finish an agent loop without closing stdout.
/// Open stdout would otherwise hold the run until the full timeout.
/// The caller supplies each complete line once, so probing is linear.
/// Arming on a transcript that parsing later rejects only shortens the wait for the same failure.
fn pi_terminal_probe(lines: &[u8]) -> ProbeSignal {
    // The last non-quiet line wins so a retry's opening event overrides a preceding failed terminal event.
    // The last non-quiet line determines whether the run is over.
    lines
        .split(|byte| *byte == b'\n')
        .map(pi_line_probe_signal)
        .fold(ProbeSignal::Quiet, |carried, signal| match signal {
            ProbeSignal::Quiet => carried,
            decided => decided,
        })
}

/// The probe returns terminal events so the drain loop can start its short grace.
///
/// Pi print mode emits terminal assistant `message_end` on stdout, not `agent_end`.
/// exists for.
///
/// `stop` and `length` are decisive; `error` and `aborted` are provisional.
/// A full-timeout idle run yields `TimedOut` rather than `AuthRequired`, preventing canonical-provider fallback.
/// `error` and `aborted` arm the drain provisionally because Pi can retry a failed turn.
/// `message_start` and `auto_retry_*` restore the full deadline because the runner's one-shot latch cannot.
/// A completion that still requests tools is an intermediate turn, not terminal.
/// Noise and malformed lines do not arm the drain; the full parse determines their verdict.
fn pi_line_probe_signal(line: &[u8]) -> ProbeSignal {
    let Ok(text) = std::str::from_utf8(line) else {
        return ProbeSignal::Quiet;
    };
    // The probe applies the full parser's node-graph bound to every completed line.
    // A line exceeding the node-graph bound does not arm the drain.
    if !subprocess::json_nodes_within_bound(text) {
        return ProbeSignal::Quiet;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return ProbeSignal::Quiet;
    };
    let message = match value.get("type").and_then(serde_json::Value::as_str) {
        // `message_start` and `auto_retry_*` indicate that the run is not over.
        // `message_start` and `auto_retry_*` clear a preceding provisional terminal.
        Some("message_start" | "auto_retry_start" | "auto_retry_end") => {
            return ProbeSignal::Continues;
        }
        Some("message_end") => value.get("message"),
        Some("agent_end") => value
            .get("messages")
            .and_then(serde_json::Value::as_array)
            .and_then(|messages| {
                messages.iter().rev().find(|message| {
                    message.get("role").and_then(serde_json::Value::as_str) == Some("assistant")
                })
            }),
        _ => None,
    };
    let Some(message) = message else {
        return ProbeSignal::Quiet;
    };
    if message.get("role").and_then(serde_json::Value::as_str) != Some("assistant") {
        return ProbeSignal::Quiet;
    }
    if message_requests_tools(message) {
        return ProbeSignal::Quiet;
    }
    let decisive = value.get("type").and_then(serde_json::Value::as_str) == Some("agent_end");
    match message
        .get("stopReason")
        .and_then(serde_json::Value::as_str)
    {
        Some("stop" | "length") => ProbeSignal::Decisive,
        Some("error" | "aborted") if decisive => ProbeSignal::Decisive,
        Some("error" | "aborted") => ProbeSignal::Provisional,
        _ => ProbeSignal::Quiet,
    }
}

/// The parser accepts only the Pi print-mode JSON vocabulary.
/// A decisive final assistant message in `agent_end` replaces a provisional `message_end` decision.
/// `message_end` one.
/// Lifecycle, compaction, retry, queue, and session-state events do not decide the run.
/// `message_start` and `auto_retry_*` clear the provisional decision they supersede.
/// `tool_execution_*` events violate this zero-tool transform's transcript contract.
/// Lines that do not claim to be JSON objects are skipped as extension stdout noise.
/// Lines that start with `{` but fail to parse are failures, not extension stdout noise.
/// Malformed or undocumented JSON returns one error without including the input line.
fn parse_pi_transcript(stdout: &[u8]) -> Result<(Vec<BackendEvent>, BackendTerminal), String> {
    let mut provisional: Option<(Option<BackendEvent>, BackendTerminal)> = None;
    let mut agent_end_final: Option<(serde_json::Value, usize)> = None;
    for (index, line) in stdout.split(|byte| *byte == b'\n').enumerate() {
        if line.is_empty() {
            continue;
        }
        if line
            .iter()
            .find(|byte| !byte.is_ascii_whitespace())
            .copied()
            != Some(b'{')
        {
            continue;
        }
        let line_no = index + 1;
        let Ok(text) = std::str::from_utf8(line) else {
            return Err(format!("non-utf8 output at line {line_no}"));
        };
        if !subprocess::json_nodes_within_bound(text) {
            return Err(format!("json structure too large at line {line_no}"));
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
            return Err(format!("malformed json at line {line_no}"));
        };
        let Some(event_type) = value.get("type").and_then(serde_json::Value::as_str) else {
            return Err(format!("missing event type at line {line_no}"));
        };
        match event_type {
            "session"
            | "agent_start"
            | "turn_start"
            | "turn_end"
            | "message_update"
            | "compaction_start"
            | "compaction_end"
            | "queue_update"
            | "session_info_changed"
            | "thinking_level_changed" => {}
            // `message_start`, `auto_retry_start`, and `auto_retry_end` clear `provisional`; without a later terminal, parsing returns a missing-terminal error rather than a superseded result.
            "message_start" | "auto_retry_start" | "auto_retry_end" => {
                provisional = None;
            }
            "tool_execution_start" | "tool_execution_update" | "tool_execution_end" => {
                return Err(format!(
                    "tool execution event in a tool-less run at line {line_no}"
                ));
            }
            "message_end" => {
                let Some(message) = value.get("message") else {
                    return Err(format!("message_end without message at line {line_no}"));
                };
                if message.get("role").and_then(serde_json::Value::as_str) != Some("assistant") {
                    continue;
                }
                // A later terminal supersedes a retried attempt's terminal and assistant text; first-terminal-wins would reject valid retries.
                // contradictory transcript.
                if let Some(decision) = assistant_message_terminal(message, line_no)? {
                    provisional = Some(decision);
                }
            }
            "agent_end" => {
                if let Some(message) = value
                    .get("messages")
                    .and_then(serde_json::Value::as_array)
                    .and_then(|messages| {
                        messages.iter().rev().find(|message| {
                            message.get("role").and_then(serde_json::Value::as_str)
                                == Some("assistant")
                        })
                    })
                {
                    agent_end_final = Some((message.clone(), line_no));
                }
            }
            _ => return Err(format!("unknown event type at line {line_no}")),
        }
    }
    // A decisive `agent_end` decision overrides `message_end`; a nonterminal final assistant or missing `agent_end` uses the last provisional `message_end` decision.
    if let Some((message, line_no)) = &agent_end_final
        && let Some((text, terminal)) = assistant_message_terminal(message, *line_no)?
    {
        return Ok((text.into_iter().collect(), terminal));
    }
    let Some((text, terminal)) = provisional else {
        return Err("output ended without a terminal event".to_owned());
    };
    Ok((text.into_iter().collect(), terminal))
}

/// `message_end` and `agent_end` share this classification: `stop` and `length` succeed unless content requests tools; `error` and `aborted` fail; other spellings return `None`.
///
/// The executor publishes only the winning decision's assistant text.
fn assistant_message_terminal(
    message: &serde_json::Value,
    line_no: usize,
) -> Result<Option<(Option<BackendEvent>, BackendTerminal)>, String> {
    let stop_reason = message
        .get("stopReason")
        .and_then(serde_json::Value::as_str);
    let text = assistant_text(message);
    let decision = match stop_reason {
        // This executor does not run tools.
        // The parser accepts `toolUse` as an intermediate stop-reason spelling.
        None | Some("toolUse") => None,
        // `stop` and `length` messages with tool requests are not terminal because this executor does not execute tools.
        Some("stop" | "length") if message_requests_tools(message) => None,
        Some("stop") => Some((
            Some(BackendEvent::AssistantText {
                text,
                finish_reason: None,
            }),
            BackendTerminal::Completed {
                finish_reason: FinishReason::Completed,
            },
        )),
        Some("length") => Some((
            Some(BackendEvent::AssistantText {
                text,
                finish_reason: Some(FinishReason::Length),
            }),
            BackendTerminal::Completed {
                finish_reason: FinishReason::Length,
            },
        )),
        Some(reason @ ("error" | "aborted")) => {
            let provider_text = message
                .get("errorMessage")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            Some((
                None,
                // The emitted message uses the stop reason rather than provider text.
                BackendTerminal::Failed(BackendError {
                    class: subprocess::classify_failure_text(provider_text),
                    retry_after_secs: subprocess::retry_after_secs_in_text(provider_text),
                    message: format!("pi assistant stopped with reason \"{reason}\""),
                    provider_code: None,
                }),
            ))
        }
        Some(_) => {
            return Err(format!("unknown stop reason at line {line_no}"));
        }
    };
    Ok(decision)
}

fn assistant_text(message: &serde_json::Value) -> String {
    let Some(content) = message.get("content").and_then(serde_json::Value::as_array) else {
        return String::new();
    };
    let mut text = String::new();
    for block in content {
        if block.get("type").and_then(serde_json::Value::as_str) == Some("text")
            && let Some(part) = block.get("text").and_then(serde_json::Value::as_str)
        {
            text.push_str(part);
        }
    }
    text
}

/// `stop` is non-terminal when `message_requests_tools(message)` is true because this executor does not execute tools.
fn message_requests_tools(message: &serde_json::Value) -> bool {
    message
        .get("content")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|content| {
            content.iter().any(|block| {
                block.get("type").and_then(serde_json::Value::as_str) == Some("toolCall")
            })
        })
}
