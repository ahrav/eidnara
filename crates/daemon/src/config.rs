//! This module reads JSONC config for autonomous historian firing.
//!
//! The reader loads user and project tiers directly without a daemon config plane.
//! The reader enforces per-leaf trust policy during reads.
//! Project config may only raise the execute threshold, causing historian firing less often.
//! Project config may override trusted memory, auto-search, caveman, promotion, and privacy settings.
//! User-profile and historian budgets remain user-tier only.
//! The Rust module uses stricter model-selection policy than the TypeScript implementation.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

/// `DEFAULT_EXECUTE_THRESHOLD_PERCENTAGE` must equal the TypeScript config schema's default, because the daemon reads config without the plugin.
pub const DEFAULT_EXECUTE_THRESHOLD_PERCENTAGE: f64 = 65.0;
/// `DEFAULT_MEMORY_BUDGET_TOKENS` must equal the TypeScript schema default of 4,000 tokens.
pub const DEFAULT_MEMORY_BUDGET_TOKENS: f64 = 4_000.0;
/// `DEFAULT_USER_PROFILE_BUDGET_TOKENS` must remain 4,000 tokens so Rust and the TypeScript renderer use the same default.
pub const DEFAULT_USER_PROFILE_BUDGET_TOKENS: f64 = 4_000.0;
/// The 90% cap reserves the final 10% of the usable window for mid-turn input growth because output capacity is already reserved.
const MAX_EXECUTE_THRESHOLD_PERCENTAGE: f64 = 90.0;
pub const MIN_HISTORIAN_CHUNK_TOKENS: usize = 8_000;
pub const MAX_HISTORIAN_CHUNK_TOKENS: usize = 50_000;
/// `DEFAULT_HISTORIAN_CONTEXT_LIMIT_TOKENS` matches the TypeScript historian fallback when no model catalog value is available.
/// The explicit config override still wins when a binding supplies one.
pub const DEFAULT_HISTORIAN_CONTEXT_LIMIT_TOKENS: usize = 128_000;
/// The auto-search defaults match the TypeScript `memory.auto_search` schema.
pub const DEFAULT_AUTO_SEARCH_SCORE_THRESHOLD: f64 = 0.6;
pub const DEFAULT_AUTO_SEARCH_MIN_PROMPT_CHARS: usize = 20;
/// The caveman defaults match the TypeScript `caveman_text_compression` schema.
pub const DEFAULT_CAVEMAN_MIN_SIZE: usize = 500;

/// Derives the historian producer budget as 25 percent of context capacity, in tokens.
///
/// Rounds to the nearest token, then clamps the result to 8,000 through 50,000 tokens. This
/// matches the TypeScript runner.
pub fn derive_historian_chunk_tokens(context_limit_tokens: usize) -> usize {
    (((context_limit_tokens as f64) * 0.25).round() as usize)
        .clamp(MIN_HISTORIAN_CHUNK_TOKENS, MAX_HISTORIAN_CHUNK_TOKENS)
}

#[derive(Debug, Clone, PartialEq)]
pub struct AutoSearchConfig {
    pub enabled: bool,
    pub score_threshold: f64,
    pub min_prompt_chars: usize,
}

impl Default for AutoSearchConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            score_threshold: DEFAULT_AUTO_SEARCH_SCORE_THRESHOLD,
            min_prompt_chars: DEFAULT_AUTO_SEARCH_MIN_PROMPT_CHARS,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CavemanConfig {
    pub enabled: bool,
    pub min_size: usize,
}

impl Default for CavemanConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            min_size: DEFAULT_CAVEMAN_MIN_SIZE,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DaemonConfig {
    pub model_chain: Vec<String>,
    pub execute_threshold_percentage: f64,
    /// Compaction resolution determines the component that controls request context-window compaction.
    pub compaction_enabled: bool,
    pub memory_enabled: bool,
    /// Auto-search hint controls operate independently at transform time.
    pub auto_search: AutoSearchConfig,
    /// Caveman compression uses deterministic age-tier controls.
    pub caveman: CavemanConfig,
    /// `auto_promote` mirrors the TS auto-promote switch; false drops facts.
    pub auto_promote: bool,
    /// The privacy gate controls whether historian user observations may be collected for later review and promotion.
    pub user_memory_collection_enabled: bool,
    pub historian_context_limit_tokens: usize,
    pub memory_budget_tokens: f64,
    pub user_profile_budget_tokens: f64,
    /// The m0 baseline option controls whether the frozen m0 baseline includes the canonical project-docs block.
    pub inject_docs: bool,
    /// The overlay option controls temporal gap overlays when the active wire surface supports overlays.
    pub temporal_awareness: bool,
    /// Contains trusted user-tier guidance text resolved during route binding.
    /// Relative override paths resolve from the user config directory.
    pub prompt_surface_guidance_override: Option<String>,
    pub smart_drops: bool,
    pub cache_ttl: String,
    /// Per-model TTL overrides use exact, bare, dash-stripped, provider-wildcard, then default matching.
    pub cache_ttl_by_model: std::collections::BTreeMap<String, String>,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            model_chain: Vec::new(),
            execute_threshold_percentage: DEFAULT_EXECUTE_THRESHOLD_PERCENTAGE,
            compaction_enabled: true,
            memory_enabled: true,
            auto_search: AutoSearchConfig::default(),
            caveman: CavemanConfig::default(),
            auto_promote: true,
            user_memory_collection_enabled: false,
            historian_context_limit_tokens: DEFAULT_HISTORIAN_CONTEXT_LIMIT_TOKENS,
            memory_budget_tokens: DEFAULT_MEMORY_BUDGET_TOKENS,
            user_profile_budget_tokens: DEFAULT_USER_PROFILE_BUDGET_TOKENS,
            inject_docs: true,
            temporal_awareness: true,
            prompt_surface_guidance_override: None,
            smart_drops: false,
            cache_ttl: "5m".to_string(),
            cache_ttl_by_model: std::collections::BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheTtlProvenance {
    Explicit,
    Default,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedCacheTtl {
    pub value: String,
    pub provenance: CacheTtlProvenance,
}

impl DaemonConfig {
    /// The resolver preserves whether the model walk matched an entry while resolving the effective cache TTL.
    ///
    /// The default TTL schedules host-side expiry but does not set provider cache markers.
    pub fn resolve_cache_ttl_with_provenance(&self, model_key: Option<&str>) -> ResolvedCacheTtl {
        let explicit = |value: &String| ResolvedCacheTtl {
            value: value.clone(),
            provenance: CacheTtlProvenance::Explicit,
        };
        let default = || ResolvedCacheTtl {
            value: self.cache_ttl.clone(),
            provenance: CacheTtlProvenance::Default,
        };

        // The matcher checks the exact key before splitting so a configured bare key matches before fallback to the default TTL.
        if let Some(ttl) = model_key.and_then(|key| self.cache_ttl_by_model.get(key)) {
            return explicit(ttl);
        }
        let Some((provider, mut model_id)) = model_key.and_then(|key| key.split_once('/')) else {
            return default();
        };
        if provider.is_empty() || model_id.is_empty() {
            return default();
        }

        loop {
            let exact = format!("{provider}/{model_id}");
            if let Some(ttl) = self.cache_ttl_by_model.get(&exact) {
                return explicit(ttl);
            }
            if let Some(ttl) = self.cache_ttl_by_model.get(model_id) {
                return explicit(ttl);
            }

            let Some(last_dash) = model_id.rfind('-').filter(|index| *index > 0) else {
                break;
            };
            model_id = &model_id[..last_dash];
        }

        if let Some(ttl) = self.cache_ttl_by_model.get(&format!("{provider}/*")) {
            return explicit(ttl);
        }
        default()
    }

    /// Resolves the effective cache TTL and discards whether it came from an explicit model rule.
    pub fn resolve_cache_ttl(&self, model_key: Option<&str>) -> String {
        self.resolve_cache_ttl_with_provenance(model_key).value
    }
}

#[derive(Debug, Clone, Default)]
struct TierConfig {
    path: PathBuf,
    mtime: Option<SystemTime>,
    value: Option<Value>,
    /// Records an unreadable file or malformed JSONC. A missing file records nothing.
    warning: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ConfigCache {
    user: TierConfig,
    project: TierConfig,
    effective: DaemonConfig,
}

impl ConfigCache {
    /// Loads user config from the platform path and project config below `project_root`.
    ///
    /// Missing, unreadable, and malformed files act as absent tiers. Unreadable and malformed
    /// files also produce warnings. Warnings go to stderr.
    pub fn effective_for_project(&mut self, project_root: &Path) -> DaemonConfig {
        let user_path = user_config_path();
        self.effective_for_user_path(user_path.as_deref(), project_root)
    }

    /// Loads and merges config from explicit user and project paths.
    ///
    /// Each tier is cached by path and modification time. User values apply first, then permitted
    /// project values. User guidance paths resolve relative to `user_path`.
    #[cfg(test)]
    pub fn effective_for_paths(&mut self, user_path: &Path, project_root: &Path) -> DaemonConfig {
        self.effective_for_user_path(Some(user_path), project_root)
    }

    fn effective_for_user_path(
        &mut self,
        user_path: Option<&Path>,
        project_root: &Path,
    ) -> DaemonConfig {
        let (effective, warnings) = self.effective_with_warnings(user_path, project_root);
        emit_warnings(warnings);
        effective
    }

    /// Tier read failures are reported on every load so a long-running daemon keeps
    /// surfacing a config file it cannot use. commentlint: allow(JUDGE)
    fn effective_with_warnings(
        &mut self,
        user_path: Option<&Path>,
        project_root: &Path,
    ) -> (DaemonConfig, Vec<String>) {
        let project_path = project_root.join(".eidnara").join("eidnara.jsonc");
        let user = match user_path {
            Some(user_path) => read_tier_cached(&mut self.user, user_path.to_path_buf()),
            None => None,
        };
        let project = read_tier_cached(&mut self.project, project_path);
        let (mut effective, mut warnings) =
            merge_tiers_with_warnings(user.as_ref(), project.as_ref());
        for tier in [&self.user, &self.project] {
            warnings.extend(tier.warning.iter().cloned());
        }
        if let Some(user_path) = user_path {
            resolve_user_guidance_override(&mut effective, user.as_ref(), user_path, &mut warnings);
        }
        self.effective = effective;
        (self.effective.clone(), warnings)
    }
}

fn user_config_path() -> Option<PathBuf> {
    user_config_path_from(
        std::env::var("XDG_CONFIG_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
    )
}

/// A CWD-relative fallback would let the untrusted project tree supply user-tier-only keys, so empty and relative values yield no user tier. commentlint: allow(JUDGE)
fn user_config_path_from(xdg_config_home: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    let absolute = |value: Option<&str>| {
        value
            .filter(|v| !v.is_empty() && Path::new(v).is_absolute())
            .map(PathBuf::from)
    };
    if let Some(xdg) = absolute(xdg_config_home) {
        return Some(xdg.join("eidnara").join("eidnara.jsonc"));
    }
    Some(
        absolute(home)?
            .join(".config")
            .join("eidnara")
            .join("eidnara.jsonc"),
    )
}

/// Largest config tier file read. A project controls its own `.eidnara`
/// directory, so the read is bounded before the daemon allocates for it. commentlint: allow(JUDGE)
const MAX_CONFIG_TIER_BYTES: u64 = 1 << 20;

/// Reads at most `MAX_CONFIG_TIER_BYTES` from `path`; a longer file is an
/// `InvalidData` error and reports as an ignored tier, not as absent.
fn read_bounded_config(path: &Path) -> io::Result<String> {
    use std::io::Read;

    let file = fs::File::open(path)?;
    let mut raw = String::new();
    file.take(MAX_CONFIG_TIER_BYTES + 1)
        .read_to_string(&mut raw)?;
    if raw.len() as u64 > MAX_CONFIG_TIER_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("config file exceeds {MAX_CONFIG_TIER_BYTES} bytes"),
        ));
    }
    Ok(raw)
}

fn read_tier_cached(cache: &mut TierConfig, path: PathBuf) -> Option<Value> {
    let mtime = fs::metadata(&path).and_then(|m| m.modified()).ok();
    // `chmod` restores readability without changing mtime, so a failed read is re-attempted.
    if cache.path == path && cache.mtime == mtime && cache.warning.is_none() {
        return cache.value.clone();
    }
    cache.path = path.clone();
    cache.mtime = mtime;
    let (value, warning) = match read_bounded_config(&path) {
        Ok(raw) => match serde_json::from_str(&strip_jsonc(&raw)) {
            Ok(value) => (Some(value), None),
            Err(error) => (
                None,
                Some(format!(
                    "config file {} is not valid JSONC ({error}); ignoring it",
                    path.display()
                )),
            ),
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => (None, None),
        Err(error) => (
            None,
            Some(format!(
                "config file {} could not be read ({error}); ignoring it",
                path.display()
            )),
        ),
    };
    cache.value = value;
    cache.warning = warning;
    cache.value.clone()
}

#[cfg(test)]
fn merge_tiers(user: Option<&Value>, project: Option<&Value>) -> DaemonConfig {
    let (cfg, warnings) = merge_tiers_with_warnings(user, project);
    emit_warnings(warnings);
    cfg
}

fn emit_warnings(warnings: Vec<String>) {
    for warning in warnings {
        eprintln!("daemon: config warning: {warning}");
    }
}

fn resolve_user_guidance_override(
    cfg: &mut DaemonConfig,
    user: Option<&Value>,
    user_config_path: &Path,
    warnings: &mut Vec<String>,
) {
    let Some(configured_path) = user
        .and_then(|value| value.pointer("/prompt_surface/guidance_override_path"))
        .and_then(Value::as_str)
    else {
        return;
    };
    if configured_path.is_empty() {
        return;
    }

    cfg.prompt_surface_guidance_override = None;
    let configured_path = Path::new(configured_path);
    let path = if configured_path.is_absolute() {
        configured_path.to_path_buf()
    } else {
        user_config_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(configured_path)
    };

    let metadata = match fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) => {
            warnings.push(format!(
                "prompt_surface.guidance_override_path ({}) could not be read ({error}); using built-in guidance.",
                path.display()
            ));
            return;
        }
    };
    if !metadata.is_file() {
        warnings.push(format!(
            "prompt_surface.guidance_override_path ({}) is not a file; using built-in guidance.",
            path.display()
        ));
        return;
    }

    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            warnings.push(format!(
                "prompt_surface.guidance_override_path ({}) could not be read ({error}); using built-in guidance.",
                path.display()
            ));
            return;
        }
    };
    let content = String::from_utf8_lossy(&bytes).into_owned();
    if content.trim().is_empty() {
        warnings.push(format!(
            "prompt_surface.guidance_override_path ({}) is empty; using built-in guidance.",
            path.display()
        ));
        return;
    }

    let markers = guidance_marker_count(&content);
    if markers != 1 {
        warnings.push(format!(
            "prompt_surface.guidance_override_path ({}) must contain exactly one {:?} section marker; found {markers}. Using built-in guidance.",
            path.display(),
            GUIDANCE_MARKER
        ));
        return;
    }

    cfg.prompt_surface_guidance_override = Some(content);
}

const GUIDANCE_MARKER: &str = "## Eidnara";

fn guidance_marker_count(content: &str) -> usize {
    content
        .split('\n')
        .filter(|line| {
            let line = line.strip_suffix('\r').unwrap_or(line);
            line.strip_prefix(GUIDANCE_MARKER)
                .is_some_and(|suffix| suffix.bytes().all(|byte| matches!(byte, b' ' | b'\t')))
        })
        .count()
}

fn merge_tiers_with_warnings(
    user: Option<&Value>,
    project: Option<&Value>,
) -> (DaemonConfig, Vec<String>) {
    let mut cfg = DaemonConfig::default();
    let mut warnings = Vec::new();

    if let Some(user) = user {
        let module_model = user
            .pointer("/historian/module_model")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        if let Some(model) = module_model {
            cfg.model_chain.push(model.to_string());
            if let Some(fallbacks) = user
                .pointer("/historian/module_fallback_models")
                .and_then(Value::as_array)
            {
                cfg.model_chain.extend(
                    fallbacks
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(ToOwned::to_owned),
                );
            }
        } else {
            if let Some(model) = user.pointer("/historian/model").and_then(Value::as_str)
                && !model.trim().is_empty()
            {
                cfg.model_chain.push(model.trim().to_string());
            }
            if let Some(fallbacks) = user
                .pointer("/historian/fallback_models")
                .and_then(Value::as_array)
            {
                cfg.model_chain.extend(
                    fallbacks
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(ToOwned::to_owned),
                );
            }
        }
        if let Some(threshold) = number_at(user, "/execute_threshold_percentage") {
            cfg.execute_threshold_percentage = threshold;
        }
        if let Some(enabled) = user.pointer("/compaction/enabled").and_then(Value::as_bool) {
            cfg.compaction_enabled = enabled;
        }
        if let Some(enabled) = user.pointer("/memory/enabled").and_then(Value::as_bool) {
            cfg.memory_enabled = enabled;
        }
        apply_auto_search_config(&mut cfg.auto_search, user);
        apply_caveman_config(&mut cfg.caveman, user);
        if let Some(budget) = number_at(user, "/memory/injection_budget_tokens") {
            cfg.memory_budget_tokens = budget.max(1.0);
        } else if let Some(budget) = number_at(user, "/memory/budget_tokens") {
            cfg.memory_budget_tokens = budget.max(1.0);
        }
        if user.pointer("/memory/budget_tokens").is_some() {
            warnings.push(
                "deprecated key /memory/budget_tokens in user tier; use /memory/injection_budget_tokens"
                    .to_string(),
            );
        }
        if let Some(budget) = number_at(user, "/memory/user_profile_budget_tokens") {
            cfg.user_profile_budget_tokens = budget.max(1.0);
        }
        if let Some(enabled) = user
            .pointer("/memory/auto_promote")
            .and_then(Value::as_bool)
        {
            cfg.auto_promote = enabled;
        }
        if let Some(enabled) = user_memory_collection_at(user) {
            cfg.user_memory_collection_enabled = enabled;
        }
        if let Some(limit) = positive_usize_at(user, "/historian/context_limit_tokens") {
            cfg.historian_context_limit_tokens = limit;
        }
        if let Some(enabled) = user.pointer("/smart_drops").and_then(Value::as_bool) {
            cfg.smart_drops = enabled;
        }
        if let Some(enabled) = user
            .pointer("/dreamer/inject_docs")
            .and_then(Value::as_bool)
        {
            cfg.inject_docs = enabled;
        }
        if let Some(enabled) = user.pointer("/temporal_awareness").and_then(Value::as_bool) {
            cfg.temporal_awareness = enabled;
        }
        if let Some(guidance) = user
            .pointer("/prompt_surface/guidance_override_text")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            let markers = guidance_marker_count(guidance);
            if markers == 1 {
                cfg.prompt_surface_guidance_override = Some(guidance.to_string());
            } else {
                warnings.push(format!(
                    "prompt_surface.guidance_override_text must contain exactly one {GUIDANCE_MARKER:?} section marker; found {markers}. Using built-in guidance."
                ));
            }
        }
        match user.pointer("/cache_ttl") {
            Some(Value::String(cache_ttl)) => {
                if !cache_ttl.trim().is_empty() {
                    cfg.cache_ttl = cache_ttl.trim().to_string();
                }
            }
            Some(Value::Object(map)) => {
                for (key, value) in map {
                    let Some(ttl) = value.as_str() else { continue };
                    if ttl.trim().is_empty() {
                        continue;
                    }
                    if key == "default" {
                        cfg.cache_ttl = ttl.trim().to_string();
                    } else {
                        cfg.cache_ttl_by_model
                            .insert(key.clone(), ttl.trim().to_string());
                    }
                }
            }
            _ => {}
        }
    }

    if let Some(project) = project {
        if let Some(project_threshold) = number_at(project, "/execute_threshold_percentage")
            && project_threshold > cfg.execute_threshold_percentage
        {
            cfg.execute_threshold_percentage = project_threshold;
        }
        warn_ignored_project_key(project, "/compaction/enabled", &mut warnings);
        if let Some(enabled) = project.pointer("/memory/enabled").and_then(Value::as_bool) {
            cfg.memory_enabled = enabled;
        }
        apply_auto_search_config(&mut cfg.auto_search, project);
        apply_caveman_config(&mut cfg.caveman, project);
        if let Some(budget) = number_at(project, "/memory/injection_budget_tokens") {
            cfg.memory_budget_tokens = budget.max(1.0);
        }
        if let Some(enabled) = project
            .pointer("/memory/auto_promote")
            .and_then(Value::as_bool)
        {
            cfg.auto_promote = enabled;
        }
        if let Some(enabled) = user_memory_collection_at(project) {
            cfg.user_memory_collection_enabled = enabled;
        }
        warn_ignored_project_key(project, "/memory/budget_tokens", &mut warnings);
        warn_ignored_project_key(project, "/memory/user_profile_budget_tokens", &mut warnings);
        warn_ignored_project_key(project, "/historian/context_limit_tokens", &mut warnings);
        if let Some(enabled) = project.pointer("/smart_drops").and_then(Value::as_bool) {
            cfg.smart_drops = enabled;
        }
        if let Some(enabled) = project
            .pointer("/dreamer/inject_docs")
            .and_then(Value::as_bool)
        {
            cfg.inject_docs = enabled;
        }
        if let Some(enabled) = project
            .pointer("/temporal_awareness")
            .and_then(Value::as_bool)
        {
            cfg.temporal_awareness = enabled;
        }
        warn_ignored_project_key(
            project,
            "/prompt_surface/guidance_override_text",
            &mut warnings,
        );
        warn_ignored_project_key(
            project,
            "/prompt_surface/guidance_override_path",
            &mut warnings,
        );
    }

    cfg.execute_threshold_percentage = cfg
        .execute_threshold_percentage
        .clamp(1.0, MAX_EXECUTE_THRESHOLD_PERCENTAGE);
    dedup_preserving_order(&mut cfg.model_chain);
    (cfg, warnings)
}

/// A repeated model would spend a bounded fallback attempt on a provider that already failed.
fn dedup_preserving_order(chain: &mut Vec<String>) {
    let mut seen = std::collections::HashSet::new();
    chain.retain(|model| seen.insert(model.clone()));
}

fn warn_ignored_project_key(value: &Value, pointer: &str, warnings: &mut Vec<String>) {
    if value.pointer(pointer).is_some() {
        warnings.push(format!(
            "ignoring {pointer} from project tier; setting is user-tier only"
        ));
    }
}

fn apply_auto_search_config(config: &mut AutoSearchConfig, value: &Value) {
    if let Some(enabled) = value
        .pointer("/memory/auto_search/enabled")
        .and_then(Value::as_bool)
    {
        config.enabled = enabled;
    }
    if let Some(threshold) = number_at(value, "/memory/auto_search/score_threshold") {
        config.score_threshold = threshold.clamp(0.3, 0.95);
    }
    if let Some(min_prompt_chars) = positive_usize_at(value, "/memory/auto_search/min_prompt_chars")
    {
        config.min_prompt_chars = min_prompt_chars.clamp(5, 500);
    }
}

fn apply_caveman_config(config: &mut CavemanConfig, value: &Value) {
    if let Some(enabled) = value
        .pointer("/caveman_text_compression/enabled")
        .and_then(Value::as_bool)
    {
        config.enabled = enabled;
    }
    if let Some(min_chars) = positive_usize_at(value, "/caveman_text_compression/min_chars") {
        config.min_size = min_chars.clamp(100, 10_000);
    }
}

fn user_memory_collection_at(value: &Value) -> Option<bool> {
    if let Some(schedule) = value
        .pointer("/dreamer/tasks/review-user-memories/schedule")
        .and_then(Value::as_str)
    {
        return Some(!schedule.trim().is_empty());
    }
    value
        .pointer("/user_memories/enabled")
        .and_then(Value::as_bool)
}

fn positive_usize_at(value: &Value, pointer: &str) -> Option<usize> {
    value
        .pointer(pointer)
        .and_then(Value::as_u64)
        .and_then(|v| usize::try_from(v).ok())
        .filter(|v| *v > 0)
}

fn number_at(value: &Value, pointer: &str) -> Option<f64> {
    value
        .pointer(pointer)
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite())
}

/// Removes line comments, block comments, and trailing commas outside JSON strings.
///
/// Preserves comment markers and escapes inside strings. Unterminated block comments consume the
/// remaining input. This function normalizes JSONC syntax but does not validate resulting JSON.
pub fn strip_jsonc(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    while i < chars.len() {
        let c = chars[i];
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_string = true;
            out.push(c);
            i += 1;
            continue;
        }
        let next = chars.get(i + 1).copied().unwrap_or('\0');
        if c == '/' && next == '/' {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && next == '*' {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i = (i + 2).min(chars.len());
            continue;
        }
        if c == ',' {
            let mut k = i + 1;
            loop {
                while k < chars.len() && chars[k].is_whitespace() {
                    k += 1;
                }
                if k + 1 < chars.len() && chars[k] == '/' && chars[k + 1] == '/' {
                    k += 2;
                    while k < chars.len() && chars[k] != '\n' {
                        k += 1;
                    }
                    continue;
                }
                if k + 1 < chars.len() && chars[k] == '/' && chars[k + 1] == '*' {
                    k += 2;
                    while k + 1 < chars.len() && !(chars[k] == '*' && chars[k + 1] == '/') {
                        k += 1;
                    }
                    k = (k + 2).min(chars.len());
                    continue;
                }
                break;
            }
            if k < chars.len() && matches!(chars[k], '}' | ']') {
                i += 1;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

#[cfg(test)]
mod cache_ttl_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn per_model_cache_ttl_object_shape_parses_and_resolves() {
        let user = json!({
            "cache_ttl": {
                "default": "10m",
                "anthropic/claude-opus-4-8": "300m",
                "gpt-5.6-sol": "30m"
            }
        });
        let cfg = merge_tiers(Some(&user), None);
        assert_eq!(cfg.cache_ttl, "10m");
        assert_eq!(
            cfg.resolve_cache_ttl(Some("anthropic/claude-opus-4-8")),
            "300m"
        );
        // Bare model id matches a provider-prefixed request key.
        assert_eq!(cfg.resolve_cache_ttl(Some("openai/gpt-5.6-sol")), "30m");
        assert_eq!(cfg.resolve_cache_ttl(Some("unknown/model")), "10m");
        assert_eq!(cfg.resolve_cache_ttl(None), "10m");
        // A bare key with an exact config entry must not fall back to the default TTL.
        assert_eq!(cfg.resolve_cache_ttl(Some("gpt-5.6-sol")), "30m");
    }

    #[test]
    fn provenance_distinguishes_an_explicit_value_equal_to_the_default() {
        let mut cfg = DaemonConfig::default();
        cfg.cache_ttl_by_model.insert(
            "anthropic/claude-haiku-4-5".to_string(),
            cfg.cache_ttl.clone(),
        );

        let explicit = cfg.resolve_cache_ttl_with_provenance(Some("anthropic/claude-haiku-4-5"));
        let fallback = cfg.resolve_cache_ttl_with_provenance(Some("anthropic/claude-nova-6-0"));
        assert_eq!(explicit.value, fallback.value);
        assert_eq!(explicit.provenance, CacheTtlProvenance::Explicit);
        assert_eq!(fallback.provenance, CacheTtlProvenance::Default);
    }

    #[test]
    fn cache_ttl_resolution_matches_shared_typescript_vectors() {
        let vectors: serde_json::Value =
            serde_json::from_str(include_str!("../testdata/cache-ttl-routing-vectors.json"))
                .unwrap();
        assert!(
            !vectors["cases"].as_array().unwrap().is_empty(),
            "cache-ttl routing vectors fixture must not be empty"
        );
        let mut cfg = DaemonConfig {
            cache_ttl: vectors["default"].as_str().unwrap().to_string(),
            ..DaemonConfig::default()
        };
        cfg.cache_ttl_by_model = vectors["models"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(key, value)| (key.clone(), value.as_str().unwrap().to_string()))
            .collect();

        for case in vectors["cases"].as_array().unwrap() {
            assert_eq!(
                cfg.resolve_cache_ttl(case["modelKey"].as_str()),
                case["expected"].as_str().unwrap(),
                "shared vector {}",
                case["name"].as_str().unwrap()
            );
        }
    }

    #[test]
    fn string_cache_ttl_shape_still_parses() {
        let user = json!({ "cache_ttl": "45m" });
        let cfg = merge_tiers(Some(&user), None);
        assert_eq!(cfg.cache_ttl, "45m");
        assert_eq!(
            cfg.resolve_cache_ttl(Some("anthropic/claude-opus-4-8")),
            "45m"
        );
    }

    #[test]
    fn project_tier_cannot_set_cache_ttl() {
        let project = json!({ "cache_ttl": { "default": "600m" } });
        let cfg = merge_tiers(None, Some(&project));
        assert_eq!(cfg.cache_ttl, "5m");
        assert!(cfg.cache_ttl_by_model.is_empty());
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    /// A tier file past the read cap is reported as an ignored tier, so a
    /// project-controlled config cannot make the daemon allocate for it.
    #[test]
    fn an_oversized_tier_file_is_ignored_with_a_warning() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("eidnara.jsonc");
        let mut body = String::from("{\"pad\":\"");
        body.push_str(&"x".repeat(MAX_CONFIG_TIER_BYTES as usize));
        body.push_str("\"}");
        std::fs::write(&path, body).unwrap();
        let mut cache = TierConfig::default();
        assert_eq!(read_tier_cached(&mut cache, path.clone()), None);
        assert!(
            cache
                .warning
                .as_deref()
                .is_some_and(|warning| warning.contains("exceeds")),
            "{:?}",
            cache.warning
        );

        std::fs::write(&path, "{\"ok\":true}").unwrap();
        let mut cache = TierConfig::default();
        assert_eq!(
            read_tier_cached(&mut cache, path),
            Some(serde_json::json!({"ok": true}))
        );
    }

    #[test]
    fn user_config_path_prefers_xdg_config_home_over_home() {
        for (xdg, home, expected) in [
            (
                Some("/xdg"),
                Some("/home/u"),
                Some(PathBuf::from("/xdg/eidnara/eidnara.jsonc")),
            ),
            (
                None,
                Some("/home/u"),
                Some(PathBuf::from("/home/u/.config/eidnara/eidnara.jsonc")),
            ),
            // An empty or relative XDG_CONFIG_HOME would resolve the trusted
            // user tier against the process working directory, so it is
            // treated as unset. commentlint: allow(JUDGE)
            (
                Some(""),
                Some("/home/u"),
                Some(PathBuf::from("/home/u/.config/eidnara/eidnara.jsonc")),
            ),
            (
                Some("rel/config"),
                Some("/home/u"),
                Some(PathBuf::from("/home/u/.config/eidnara/eidnara.jsonc")),
            ),
            // Without a usable home there is no user tier at all rather than
            // a CWD-relative one. commentlint: allow(JUDGE)
            (None, None, None),
            (Some(""), Some(""), None),
            (None, Some("rel/home"), None),
        ] {
            assert_eq!(
                user_config_path_from(xdg, home),
                expected,
                "xdg={xdg:?} home={home:?}"
            );
        }
    }

    #[test]
    fn tier_policy_ignores_project_models_and_rejects_project_lowering() {
        let user = serde_json::json!({
            "historian": { "model": "cheap", "fallback_models": ["fallback"] },
            "execute_threshold_percentage": 80,
            "memory": { "enabled": false }
        });
        let project = serde_json::json!({
            "historian": { "model": "expensive", "fallback_models": ["expensive2"] },
            "execute_threshold_percentage": 40,
            "memory": { "enabled": true }
        });
        let cfg = merge_tiers(Some(&user), Some(&project));
        assert_eq!(cfg.model_chain, vec!["cheap", "fallback"]);
        assert_eq!(cfg.execute_threshold_percentage, 80.0);
        assert!(cfg.memory_enabled);
    }

    #[test]
    fn project_threshold_may_only_raise() {
        let user = serde_json::json!({ "execute_threshold_percentage": 70 });
        let project = serde_json::json!({ "execute_threshold_percentage": 91 });
        let cfg = merge_tiers(Some(&user), Some(&project));
        assert_eq!(cfg.execute_threshold_percentage, 90.0);
    }

    #[test]
    fn default_threshold_matches_typescript_schema() {
        let cfg = merge_tiers(None, None);
        assert_eq!(cfg.execute_threshold_percentage, 65.0);
    }

    #[test]
    fn default_memory_budget_matches_typescript_schema() {
        assert_eq!(DEFAULT_MEMORY_BUDGET_TOKENS, 4_000.0);
        assert_eq!(merge_tiers(None, None).memory_budget_tokens, 4_000.0);
    }

    #[test]
    fn memory_injection_budget_uses_standard_key_and_deprecated_user_fallback() {
        let standard_user = serde_json::json!({
            "memory": { "injection_budget_tokens": 3_000, "budget_tokens": 9_000 }
        });
        let standard_project = serde_json::json!({
            "memory": { "injection_budget_tokens": 3_500 }
        });
        let (standard, warnings) =
            merge_tiers_with_warnings(Some(&standard_user), Some(&standard_project));
        assert_eq!(standard.memory_budget_tokens, 3_500.0);
        assert!(warnings.iter().any(|warning| {
            warning.contains("/memory/budget_tokens") && warning.contains("deprecated")
        }));

        let legacy_user = serde_json::json!({ "memory": { "budget_tokens": 3_250 } });
        let (legacy, warnings) = merge_tiers_with_warnings(Some(&legacy_user), None);
        assert_eq!(legacy.memory_budget_tokens, 3_250.0);
        assert!(warnings.iter().any(|warning| {
            warning.contains("/memory/budget_tokens")
                && warning.contains("user tier")
                && warning.contains("/memory/injection_budget_tokens")
        }));
    }

    #[test]
    fn rust_only_budget_leaves_are_user_tier_only_and_warn_when_project_supplies_them() {
        let user = serde_json::json!({
            "memory": {
                "injection_budget_tokens": 5_000,
                "user_profile_budget_tokens": 2_500
            },
            "historian": { "context_limit_tokens": 64_000 }
        });
        let project = serde_json::json!({
            "memory": {
                "budget_tokens": 19_000,
                "user_profile_budget_tokens": 12_000
            },
            "historian": { "context_limit_tokens": 200_000 }
        });
        let (cfg, warnings) = merge_tiers_with_warnings(Some(&user), Some(&project));

        assert_eq!(cfg.memory_budget_tokens, 5_000.0);
        assert_eq!(cfg.user_profile_budget_tokens, 2_500.0);
        assert_eq!(cfg.historian_context_limit_tokens, 64_000);
        for key in [
            "/memory/budget_tokens",
            "/memory/user_profile_budget_tokens",
            "/historian/context_limit_tokens",
        ] {
            assert!(
                warnings.iter().any(|warning| {
                    warning.contains(key)
                        && warning.contains("project tier")
                        && warning.contains("user-tier only")
                }),
                "missing warning for {key}: {warnings:?}"
            );
        }
    }

    #[test]
    fn compaction_enabled_defaults_true_and_is_user_tier_only() {
        assert!(merge_tiers(None, None).compaction_enabled);

        let user = serde_json::json!({ "compaction": { "enabled": false } });
        let project = serde_json::json!({ "compaction": { "enabled": true } });
        let (cfg, warnings) = merge_tiers_with_warnings(Some(&user), Some(&project));
        assert!(!cfg.compaction_enabled);
        assert!(warnings.iter().any(|warning| {
            warning.contains("/compaction/enabled") && warning.contains("project tier")
        }));

        let (project_only, warnings) = merge_tiers_with_warnings(None, Some(&project));
        assert!(project_only.compaction_enabled);
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn auto_search_and_caveman_config_follow_user_then_project_tiers() {
        let user = serde_json::json!({
            "memory": { "auto_search": {
                "enabled": false,
                "score_threshold": 0.4,
                "min_prompt_chars": 100
            }},
            "caveman_text_compression": { "enabled": true, "min_chars": 900 }
        });
        let project = serde_json::json!({
            "memory": { "auto_search": {
                "enabled": true,
                "score_threshold": 0.8,
                "min_prompt_chars": 50
            }},
            "caveman_text_compression": { "enabled": false, "min_chars": 700 }
        });
        let cfg = merge_tiers(Some(&user), Some(&project));
        assert_eq!(
            cfg.auto_search,
            AutoSearchConfig {
                enabled: true,
                score_threshold: 0.8,
                min_prompt_chars: 50,
            }
        );
        assert_eq!(
            cfg.caveman,
            CavemanConfig {
                enabled: false,
                min_size: 700,
            }
        );

        assert_eq!(
            merge_tiers(None, None).auto_search,
            AutoSearchConfig::default()
        );
        assert_eq!(merge_tiers(None, None).caveman, CavemanConfig::default());
    }

    #[test]
    fn historian_budget_derivation_clamps_at_both_bounds() {
        assert_eq!(derive_historian_chunk_tokens(1), 8_000);
        assert_eq!(derive_historian_chunk_tokens(32_000), 8_000);
        assert_eq!(derive_historian_chunk_tokens(128_000), 32_000);
        assert_eq!(derive_historian_chunk_tokens(200_000), 50_000);
        assert_eq!(derive_historian_chunk_tokens(400_000), 50_000);
    }

    #[test]
    fn docs_and_temporal_flags_follow_user_then_project_tiers() {
        let user = serde_json::json!({
            "dreamer": { "inject_docs": false },
            "temporal_awareness": false
        });
        let project = serde_json::json!({
            "dreamer": { "inject_docs": true },
            "temporal_awareness": true
        });
        let cfg = merge_tiers(Some(&user), Some(&project));
        assert!(cfg.inject_docs);
        assert!(cfg.temporal_awareness);
        let defaults = merge_tiers(None, None);
        assert!(defaults.inject_docs);
        assert!(defaults.temporal_awareness);
    }

    #[test]
    fn guidance_override_accepts_resolved_user_text_and_ignores_project_injection() {
        let user = serde_json::json!({
            "prompt_surface": {
                "guidance_override_text": "## Eidnara\n\nTrusted user guidance."
            }
        });
        let project = serde_json::json!({
            "prompt_surface": {
                "guidance_override_text": "## Eidnara\n\nProject injection.",
                "guidance_override_path": "/repo/untrusted.md"
            }
        });

        let (cfg, warnings) = merge_tiers_with_warnings(Some(&user), Some(&project));

        assert_eq!(
            cfg.prompt_surface_guidance_override.as_deref(),
            Some("## Eidnara\n\nTrusted user guidance.")
        );
        assert_eq!(warnings.len(), 2);
        assert!(
            warnings
                .iter()
                .all(|warning| warning.contains("user-tier only"))
        );
    }

    #[test]
    fn inline_guidance_override_requires_exactly_one_marker_like_the_file_form() {
        for (text, markers) in [
            ("Just prose, no marker.", 0),
            ("## Eidnara\n\n## Eidnara\n", 2),
        ] {
            let user = serde_json::json!({
                "prompt_surface": { "guidance_override_text": text }
            });
            let (cfg, warnings) = merge_tiers_with_warnings(Some(&user), None);
            assert!(cfg.prompt_surface_guidance_override.is_none());
            assert_eq!(warnings.len(), 1, "{warnings:?}");
            assert!(
                warnings[0].contains(&format!("found {markers}"))
                    && warnings[0].contains("guidance_override_text"),
                "{}",
                warnings[0]
            );
        }
    }

    #[test]
    fn guidance_override_path_resolves_relative_to_user_config_directory() {
        let dir = tempfile::tempdir().unwrap();
        let user_path = dir.path().join("eidnara.jsonc");
        let guidance_path = dir.path().join("guidance.md");
        let guidance = "## Eidnara\r\n\r\nTrusted route guidance.\r\n";
        fs::write(&guidance_path, guidance).unwrap();
        fs::write(
            &user_path,
            r#"{
                "prompt_surface": {
                    "guidance_override_path": "guidance.md"
                }
            }"#,
        )
        .unwrap();

        let mut cache = ConfigCache::default();
        let cfg = cache.effective_for_paths(&user_path, dir.path());

        assert_eq!(
            cfg.prompt_surface_guidance_override.as_deref(),
            Some(guidance)
        );
    }

    #[test]
    fn guidance_override_invalid_and_missing_files_warn_and_fall_back() {
        let dir = tempfile::tempdir().unwrap();
        let user_path = dir.path().join("eidnara.jsonc");
        let invalid_path = dir.path().join("invalid.md");
        fs::write(
            &invalid_path,
            "## Eidnara\n\nFirst.\n## Eidnara \t\n\nSecond.",
        )
        .unwrap();

        for (configured_path, expected_warning) in [
            (
                "invalid.md",
                "must contain exactly one \"## Eidnara\" section marker; found 2",
            ),
            ("missing.md", "could not be read"),
        ] {
            let user = serde_json::json!({
                "prompt_surface": {
                    "guidance_override_path": configured_path,
                    "guidance_override_text": "## Eidnara\n\nStale text"
                }
            });
            let (mut cfg, mut warnings) = merge_tiers_with_warnings(Some(&user), None);

            resolve_user_guidance_override(&mut cfg, Some(&user), &user_path, &mut warnings);

            assert!(cfg.prompt_surface_guidance_override.is_none());
            assert_eq!(warnings.len(), 1);
            assert!(warnings[0].contains(expected_warning), "{}", warnings[0]);
            assert!(
                warnings[0]
                    .to_ascii_lowercase()
                    .contains("using built-in guidance")
            );
        }
    }

    #[test]
    fn guidance_marker_validation_matches_the_typescript_line_rule() {
        assert_eq!(guidance_marker_count("## Eidnara"), 1);
        assert_eq!(guidance_marker_count("## Eidnara \t\r\nbody"), 1);
        assert_eq!(guidance_marker_count("prefix ## Eidnara\nbody"), 0);
        assert_eq!(guidance_marker_count("## Eidnara extra\nbody"), 0);
    }

    #[test]
    fn historian_gates_follow_tiers_but_context_limit_remains_user_tier_only() {
        let user = serde_json::json!({
            "memory": { "auto_promote": false },
            "dreamer": { "tasks": { "review-user-memories": { "schedule": "daily" } } },
            "historian": { "context_limit_tokens": 128000 }
        });
        let project = serde_json::json!({
            "memory": { "auto_promote": true },
            "user_memories": { "enabled": false },
            "historian": { "context_limit_tokens": 64000 }
        });
        assert!(user_memory_collection_at(&user).unwrap());
        let cfg = merge_tiers(Some(&user), Some(&project));
        assert!(cfg.auto_promote);
        assert!(!cfg.user_memory_collection_enabled);
        assert_eq!(cfg.historian_context_limit_tokens, 128_000);
        let legacy_disabled = serde_json::json!({
            "user_memories": { "enabled": false }
        });
        assert!(!user_memory_collection_at(&legacy_disabled).unwrap());
    }

    #[test]
    fn module_model_replaces_plugin_chain_entirely() {
        let user = serde_json::json!({
            "historian": {
                "model": "google/antigravity-gemini-3.5-flash",
                "fallback_models": ["google/antigravity-claude-opus-4-6-thinking"],
                "module_model": "google/gemini-3.5-flash",
                "module_fallback_models": ["ollama-cloud/kimi-k2.7-code"]
            }
        });
        let cfg = merge_tiers(Some(&user), None);
        assert_eq!(
            cfg.model_chain,
            vec!["google/gemini-3.5-flash", "ollama-cloud/kimi-k2.7-code"]
        );
    }

    #[test]
    fn module_model_absent_falls_back_to_plugin_keys() {
        let user = serde_json::json!({
            "historian": {
                "model": "deepseek/deepseek-v4-flash",
                "fallback_models": ["ollama-cloud/kimi-k2.7-code"],
                "module_fallback_models": ["ignored/without-module-model"]
            }
        });
        let cfg = merge_tiers(Some(&user), None);
        assert_eq!(
            cfg.model_chain,
            vec!["deepseek/deepseek-v4-flash", "ollama-cloud/kimi-k2.7-code"]
        );
    }

    #[test]
    fn module_model_blank_is_treated_as_absent() {
        let user = serde_json::json!({
            "historian": {
                "model": "deepseek/deepseek-v4-flash",
                "module_model": "   "
            }
        });
        let cfg = merge_tiers(Some(&user), None);
        assert_eq!(cfg.model_chain, vec!["deepseek/deepseek-v4-flash"]);
    }

    #[test]
    fn module_model_is_user_tier_only() {
        let user = serde_json::json!({
            "historian": { "module_model": "google/gemini-3.5-flash" }
        });
        let project = serde_json::json!({
            "historian": {
                "module_model": "evil/expensive-model",
                "module_fallback_models": ["evil/other"]
            }
        });
        let cfg = merge_tiers(Some(&user), Some(&project));
        assert_eq!(cfg.model_chain, vec!["google/gemini-3.5-flash"]);
    }

    #[test]
    fn jsonc_strip_preserves_comment_like_strings() {
        let parsed: Value = serde_json::from_str(&strip_jsonc(
            r#"{ "url": "http://x/y", "a": [1,], /* c */ }"#,
        ))
        .unwrap();
        assert_eq!(parsed["url"], "http://x/y");
        assert_eq!(parsed["a"], serde_json::json!([1]));
    }

    /// The project tier is read from `.eidnara/eidnara.jsonc` under the project root;
    /// a value only that file sets must reach the effective config.
    #[test]
    fn project_tier_is_read_from_the_eidnara_config_path() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user.jsonc");
        let project = dir.path().join("project");
        std::fs::create_dir_all(project.join(".eidnara")).unwrap();
        std::fs::write(&user, "{}").unwrap();
        std::fs::write(
            project.join(".eidnara/eidnara.jsonc"),
            r#"{ "memory": { "enabled": false } }"#,
        )
        .unwrap();

        let effective = ConfigCache::default().effective_for_paths(&user, &project);
        assert!(!effective.memory_enabled);
    }

    /// The four built-in guidance assets must each carry exactly one section marker,
    /// the same rule `resolve_user_guidance_override` applies to an override.
    #[test]
    fn built_in_guidance_assets_carry_one_eidnara_marker_each() {
        for asset in [
            crate::prompt_surface::GUIDANCE_FULL_PRIMARY,
            crate::prompt_surface::GUIDANCE_FULL_NO_REDUCE,
            crate::prompt_surface::GUIDANCE_LIGHT_PRIMARY_TEXT,
            crate::prompt_surface::GUIDANCE_LIGHT_NO_REDUCE_TEXT,
        ] {
            assert_eq!(guidance_marker_count(asset), 1);
            assert!(asset.starts_with("## Eidnara\n"));
        }
    }

    #[test]
    fn mtime_cache_reuses_unchanged_reads_and_invalidates_on_mtime_change() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user.jsonc");
        let project = dir.path().join("project");
        std::fs::create_dir_all(project.join(".eidnara")).unwrap();

        std::fs::write(&user, r#"{ "historian": { "model": "model-a" } }"#).unwrap();
        std::fs::write(
            project.join(".eidnara/eidnara.jsonc"),
            r#"{ "memory": { "enabled": true } }"#,
        )
        .unwrap();

        let mut cache = ConfigCache::default();
        let first = cache.effective_for_paths(&user, &project);
        assert_eq!(first.model_chain, vec!["model-a"]);

        // Without an mtime change, the cache ignores a different file body.
        let original_mtime = std::fs::metadata(&user).unwrap().modified().unwrap();
        std::fs::write(&user, r#"{ "historian": { "model": "model-b" } }"#).unwrap();
        filetime::set_file_mtime(&user, filetime::FileTime::from_system_time(original_mtime))
            .unwrap();
        let unchanged = cache.effective_for_paths(&user, &project);
        assert_eq!(unchanged.model_chain, vec!["model-a"]);

        // An mtime change reloads the cached file.
        let newer = filetime::FileTime::from_unix_time(
            original_mtime
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64
                + 2,
            0,
        );
        filetime::set_file_mtime(&user, newer).unwrap();
        let reloaded = cache.effective_for_paths(&user, &project);
        assert_eq!(reloaded.model_chain, vec!["model-b"]);
    }

    #[test]
    fn unreadable_and_malformed_tiers_warn_while_missing_tiers_stay_silent() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user.jsonc");
        let project = dir.path().join("project");
        let project_file = project.join(".eidnara/eidnara.jsonc");
        std::fs::create_dir_all(&project_file).unwrap();
        std::fs::write(&user, r#"{ "memory": { "enabled": false "#).unwrap();

        let mut cache = ConfigCache::default();
        let (effective, warnings) = cache.effective_with_warnings(Some(&user), &project);

        assert!(
            effective.memory_enabled,
            "a malformed tier contributes nothing and the default stands"
        );
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(
            warnings[0].contains(&user.display().to_string())
                && warnings[0].contains("not valid JSONC"),
            "{}",
            warnings[0]
        );
        assert!(
            warnings[1].contains(&project_file.display().to_string())
                && warnings[1].contains("could not be read"),
            "a directory at the project config path is unreadable, not missing: {}",
            warnings[1]
        );

        // The cached read keeps reporting the failure until the file changes.
        let (_, repeated) = cache.effective_with_warnings(Some(&user), &project);
        assert_eq!(repeated, warnings);

        // Repairing the file without moving its mtime is picked up, because a failed
        // read never takes the mtime fast path.
        let original_mtime = std::fs::metadata(&user).unwrap().modified().unwrap();
        std::fs::write(&user, r#"{ "memory": { "enabled": false } }"#).unwrap();
        filetime::set_file_mtime(&user, filetime::FileTime::from_system_time(original_mtime))
            .unwrap();
        let (repaired, repaired_warnings) = cache.effective_with_warnings(Some(&user), &project);
        assert!(!repaired.memory_enabled);
        assert_eq!(repaired_warnings.len(), 1, "{repaired_warnings:?}");
        assert!(repaired_warnings[0].contains("could not be read"));

        // A missing tier is an ordinary absent tier and warns about nothing.
        std::fs::remove_file(&user).unwrap();
        std::fs::remove_dir(&project_file).unwrap();
        let (_, silent) = cache.effective_with_warnings(Some(&user), &project);
        assert!(silent.is_empty(), "{silent:?}");
    }

    #[test]
    fn model_chain_drops_repeats_anywhere_and_keeps_first_occurrence_order() {
        let user = serde_json::json!({
            "historian": {
                "model": "a",
                "fallback_models": ["b", "a", "c", "b"]
            }
        });
        let cfg = merge_tiers(Some(&user), None);
        assert_eq!(cfg.model_chain, vec!["a", "b", "c"]);
    }
}
