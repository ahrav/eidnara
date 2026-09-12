//! Rendered-tail hygiene measurement for Channel-1 and Channel-2 nudges.
//!
//! `T` counts eligible rendered-tail tokens. `U` counts the unprotected,
//! actively tagged subset, so every result maintains `0 <= U <= T`.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::hash::{BuildHasher, RandomState};
use std::sync::{Mutex, MutexGuard, OnceLock};

use base64::Engine;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use cache_stability::{CoreState, FrozenUnit};
use memory_store::{
    MediaBlock, MediaKind, OutputKind, ResultBlockKind, TagRow, TailHygieneBaseline,
    TailHygienePartKind, TailHygienePartMeasurement,
};
use sha2::{Digest, Sha256};

use crate::wire::{FlatBlock, FlatProjection};

pub(crate) const CHANNEL1_MIN_TOKENS: i64 = 60_000;
pub(crate) const CHANNEL1_FLOOR_TOKENS: i64 = 25_000;
pub(crate) const CHANNEL2_FLOOR_TOKENS: i64 = 50_000;
pub(crate) const CHANNEL2_SEVERITY_THRESHOLD: f64 = 0.75;

const RED_KEY_PREFIX: &str = "red:";
const CAV_KEY_PREFIX: &str = "cav:";
const CHANNEL1_REMINDER_OPEN: &str = "\n\n<system-reminder>\n";
const CHANNEL1_REMINDER_CLOSE: &str = "\n</system-reminder>";
const IMAGE_TOKEN_DIVISOR: u64 = 750;
const IMAGE_FALLBACK_TOKENS: i64 = 1_200;
const IMAGE_TOKEN_CAP: i64 = 4_500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HygieneBand {
    Quiet,
    Gentle,
    Firm,
    Urgent,
    Channel2,
}

impl HygieneBand {
    #[cfg(test)]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Quiet => "quiet",
            Self::Gentle => "gentle",
            Self::Firm => "firm",
            Self::Urgent => "urgent",
            Self::Channel2 => "channel2",
        }
    }
}

/// One hygiene walk, with token counts and content-addressed part evidence.
///
/// `u` and `t` are estimated token counts. `content_signature` commits to part
/// order, identity, kind, and content without retaining rendered content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TailHygieneMeasurement {
    pub(crate) u: i64,
    pub(crate) t: i64,
    pub(crate) content_signature: String,
    pub(crate) parts: Vec<TailHygienePartMeasurement>,
}

const MEMO_SESSION_LIMIT: usize = 16;
const MEMO_SESSION_BYTES: usize = 1024 * 1024;
/// Includes session keys, all map allocations, and the process cache container.
pub(crate) const MEMO_RETAINED_BYTES_BOUND: usize =
    MEMO_SESSION_LIMIT * MEMO_SESSION_BYTES + std::mem::size_of::<OnceLock<HygieneMemos>>();

#[derive(Debug, Clone, PartialEq, Eq)]
struct MeasuredPart {
    content_hash: String,
    kind: TailHygienePartKind,
    tokens: i64,
}

struct MemoEntry {
    projection_hash: [u8; 32],
    caveman: Option<FrozenUnit>,
    excluded: bool,
    text_role: bool,
    measured: MeasuredPart,
}

impl MemoEntry {
    fn heap_bytes(&self) -> usize {
        self.measured.content_hash.capacity()
            + self.caveman.as_ref().map_or(0, |unit| {
                unit.key.capacity()
                    + unit.kind.capacity()
                    + unit.frozen_payload.capacity()
                    + unit.reset_rule.capacity()
            })
    }
}

/// Per-block facts only; attribution and eligibility are evaluated on every walk.
pub(crate) struct TailHygieneMemo {
    entries: HashMap<String, MemoEntry>,
    heap_bytes: usize,
    map_bytes: usize,
    budget: usize,
}

#[cfg(test)]
thread_local! {
    static MEMO_STATS: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}

#[cfg(test)]
pub(crate) fn local_memo_stats() -> (usize, usize) {
    MEMO_STATS.with(std::cell::Cell::get)
}

#[cfg(test)]
impl Default for TailHygieneMemo {
    fn default() -> Self {
        Self::new(MEMO_SESSION_BYTES)
    }
}

impl TailHygieneMemo {
    fn new(budget: usize) -> Self {
        Self {
            entries: HashMap::new(),
            heap_bytes: 0,
            map_bytes: 0,
            budget,
        }
    }

    fn retained_bytes(&self) -> usize {
        self.map_bytes.saturating_add(self.heap_bytes)
    }

    fn get(
        &self,
        key: &str,
        block: &FlatBlock,
        caveman: Option<&FrozenUnit>,
        excluded: bool,
    ) -> Option<MeasuredPart> {
        let measured = self
            .entries
            .get(key)
            .filter(|entry| {
                entry.projection_hash == block.content_hash
                    && entry.caveman.as_ref() == caveman
                    && entry.excluded == excluded
                    && entry.text_role == matches!(block.role.as_str(), "user" | "assistant")
            })
            .map(|entry| entry.measured.clone());
        #[cfg(test)]
        MEMO_STATS.with(|stats| {
            let (hits, misses) = stats.get();
            stats.set((
                hits + usize::from(measured.is_some()),
                misses + usize::from(measured.is_none()),
            ));
        });
        measured
    }

    fn insert(
        &mut self,
        key: &str,
        block: &FlatBlock,
        caveman: Option<&FrozenUnit>,
        excluded: bool,
        measured: &MeasuredPart,
    ) {
        if let Some((old_key, old)) = self.entries.remove_entry(key) {
            self.heap_bytes -= old_key.capacity() + old.heap_bytes();
        }
        // Reject oversized payloads before copying them into retained storage.
        let payload_bytes = caveman.map_or(0, |unit| {
            unit.key.len() + unit.kind.len() + unit.frozen_payload.len() + unit.reset_rule.len()
        });
        if key
            .len()
            .saturating_add(payload_bytes)
            .saturating_add(64)
            .saturating_add(std::mem::size_of::<MemoEntry>())
            > self.budget
        {
            return;
        }
        let key = key.to_string();
        let entry = MemoEntry {
            projection_hash: block.content_hash,
            caveman: caveman.cloned(),
            excluded,
            text_role: matches!(block.role.as_str(), "user" | "assistant"),
            measured: measured.clone(),
        };
        self.heap_bytes += key.capacity() + entry.heap_bytes();
        self.entries.insert(key, entry);
        // Deletions can reduce admission capacity while the bucket allocation stays live.
        self.map_bytes = self
            .map_bytes
            .max(crate::retained_size::hash_map_allocation_bytes(&self.entries).saturating_add(16));
        if self.retained_bytes() > self.budget {
            *self = Self::new(self.budget);
        }
    }

    fn retain_parts(&mut self, parts: &[TailHygienePartMeasurement]) {
        let keys = parts
            .iter()
            .map(|part| part.key.as_str())
            .collect::<HashSet<_>>();
        self.entries.retain(|key, entry| {
            if keys.contains(key.as_str()) {
                return true;
            }
            self.heap_bytes -= key.capacity() + entry.heap_bytes();
            false
        });
        if self.entries.is_empty() {
            self.entries = HashMap::new();
            self.map_bytes = 0;
        }
    }
}

struct MemoSession {
    namespace: u64,
    id: Box<str>,
    memo: TailHygieneMemo,
}

/// Fixed session slots bound container storage independently of payload budgets.
#[derive(Default)]
pub(crate) struct HygieneMemos {
    slots: [Mutex<Option<MemoSession>>; MEMO_SESSION_LIMIT],
    hasher: RandomState,
}

impl HygieneMemos {
    fn slot_index(&self, namespace: u64, id: &str) -> usize {
        (self.hasher.hash_one((namespace, id)) % MEMO_SESSION_LIMIT as u64) as usize
    }

    fn lock_slot(slot: &Mutex<Option<MemoSession>>) -> MutexGuard<'_, Option<MemoSession>> {
        slot.lock().unwrap_or_else(|poisoned| {
            let mut guard = poisoned.into_inner();
            *guard = None;
            slot.clear_poison();
            guard
        })
    }

    pub(crate) fn with_session<R>(
        &self,
        namespace: u64,
        id: &str,
        measure: impl FnOnce(&mut TailHygieneMemo) -> R,
    ) -> R {
        let mut slot = Self::lock_slot(&self.slots[self.slot_index(namespace, id)]);
        if id.len() > MEMO_SESSION_BYTES {
            return measure(&mut TailHygieneMemo::new(0));
        }
        if slot
            .as_ref()
            .is_none_or(|session| session.namespace != namespace || session.id.as_ref() != id)
        {
            *slot = Some(MemoSession {
                namespace,
                id: id.into(),
                memo: TailHygieneMemo::new(MEMO_SESSION_BYTES - id.len()),
            });
        }
        let session = slot.as_mut().expect("selected hygiene session");
        let result = measure(&mut session.memo);
        debug_assert!(session.id.len() + session.memo.retained_bytes() <= MEMO_SESSION_BYTES);
        result
    }

    #[cfg(test)]
    fn retained_bytes(&self) -> usize {
        self.slots
            .iter()
            .fold(std::mem::size_of::<OnceLock<Self>>(), |bytes, slot| {
                bytes
                    + Self::lock_slot(slot).as_ref().map_or(0, |session| {
                        session.id.len() + session.memo.retained_bytes()
                    })
            })
    }

    pub(crate) fn remove(&self, namespace: u64, id: &str) {
        let mut slot = Self::lock_slot(&self.slots[self.slot_index(namespace, id)]);
        if slot
            .as_ref()
            .is_some_and(|session| session.namespace == namespace && session.id.as_ref() == id)
        {
            *slot = None;
        }
    }

    pub(crate) fn remove_session(&self, id: &str) {
        for slot in &self.slots {
            let mut occupant = Self::lock_slot(slot);
            if occupant
                .as_ref()
                .is_some_and(|session| session.id.as_ref() == id)
            {
                *occupant = None;
            }
        }
    }

    pub(crate) fn clear(&self) {
        for slot in &self.slots {
            *Self::lock_slot(slot) = None;
        }
    }
}

pub(crate) fn hygiene_memos() -> &'static HygieneMemos {
    static MEMOS: OnceLock<HygieneMemos> = OnceLock::new();
    MEMOS.get_or_init(HygieneMemos::default)
}

fn hex_digest(bytes: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(bytes.as_ref()))
}

fn strip_channel1_reminder_spans(output: &str) -> &str {
    let mut stripped = output;
    while stripped.ends_with(CHANNEL1_REMINDER_CLOSE) {
        let Some(opener) = stripped.rfind(CHANNEL1_REMINDER_OPEN) else {
            break;
        };
        stripped = &stripped[..opener];
    }
    stripped
}

fn is_drop_sentinel(content: &str) -> bool {
    let mut head = content.trim_start();
    if let Some(rest) = head.strip_prefix('§')
        && let Some((_, suffix)) = rest.split_once("§")
    {
        head = suffix.trim_start();
    }
    let head = head.to_ascii_lowercase();
    head.starts_with("[dropped") || head.starts_with("[truncated")
}

#[cfg(test)]
fn estimated_tokens(content: &str) -> i64 {
    tokenizer::estimate_tokens(content) as i64
}

fn media_content(media: &MediaBlock) -> String {
    media
        .source
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| serde_json::to_string(&media.source).unwrap_or_default())
}

/// Standard alphabet, indifferent to padding and to non-zero trailing bits:
/// the TypeScript reference decodes the same prefix through `atob`, which
/// accepts both, and the two estimators agree on whitespace-free payloads.
const PREVIEW_BASE64: GeneralPurpose = GeneralPurpose::new(
    &base64::alphabet::STANDARD,
    GeneralPurposeConfig::new()
        .with_decode_padding_mode(DecodePaddingMode::Indifferent)
        .with_decode_allow_trailing_bits(true),
);

/// Decodes the first 512 base64 characters, enough to hold every image
/// header the dimension parsers read.
fn decode_base64_preview(payload: &str) -> Option<Vec<u8>> {
    let bytes = payload.as_bytes();
    PREVIEW_BASE64.decode(&bytes[..bytes.len().min(512)]).ok()
}

fn image_dimensions(header: &str, bytes: &[u8]) -> Option<(u64, u64)> {
    if header.contains("image/png")
        && bytes.len() >= 24
        && bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a])
    {
        let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?) as u64;
        let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?) as u64;
        return (width > 0 && height > 0).then_some((width, height));
    }
    if header.contains("image/gif") && bytes.len() >= 10 && bytes.starts_with(b"GIF") {
        let width = u16::from_le_bytes(bytes[6..8].try_into().ok()?) as u64;
        let height = u16::from_le_bytes(bytes[8..10].try_into().ok()?) as u64;
        return (width > 0 && height > 0).then_some((width, height));
    }
    if (header.contains("image/jpeg") || header.contains("image/jpg"))
        && bytes.starts_with(&[0xff, 0xd8])
    {
        let mut index = 2usize;
        while index + 8 < bytes.len() {
            if bytes[index] != 0xff {
                index += 1;
                continue;
            }
            let marker = bytes[index + 1];
            let is_sof = matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf);
            if is_sof {
                let height = u16::from_be_bytes([bytes[index + 5], bytes[index + 6]]) as u64;
                let width = u16::from_be_bytes([bytes[index + 7], bytes[index + 8]]) as u64;
                return (width > 0 && height > 0).then_some((width, height));
            }
            if matches!(marker, 0xd8 | 0xd9 | 0x01) {
                index += 2;
                continue;
            }
            let segment_len = u16::from_be_bytes([bytes[index + 2], bytes[index + 3]]) as usize;
            if segment_len < 2 {
                return None;
            }
            index = index.saturating_add(2 + segment_len);
        }
    }
    if header.contains("image/webp")
        && bytes.len() >= 30
        && bytes.starts_with(b"RIFF")
        && &bytes[8..12] == b"WEBP"
    {
        let variant = &bytes[12..16];
        let (width, height) = if variant == b"VP8 " {
            (
                u16::from_le_bytes([bytes[26], bytes[27]]) as u64 & 0x3fff,
                u16::from_le_bytes([bytes[28], bytes[29]]) as u64 & 0x3fff,
            )
        } else if variant == b"VP8L" {
            let width = 1 + (u16::from_le_bytes([bytes[21], bytes[22]]) as u64 & 0x3fff);
            let height = 1
                + (((bytes[22] as u64 >> 6)
                    | ((bytes[23] as u64) << 2)
                    | ((bytes[24] as u64) << 10))
                    & 0x3fff);
            (width, height)
        } else if variant == b"VP8X" {
            (
                1 + bytes[24] as u64 + ((bytes[25] as u64) << 8) + ((bytes[26] as u64) << 16),
                1 + bytes[27] as u64 + ((bytes[28] as u64) << 8) + ((bytes[29] as u64) << 16),
            )
        } else {
            return None;
        };
        return (width > 0 && height > 0).then_some((width, height));
    }
    None
}

/// Estimates image tokens from dimensions in a bounded base64 header preview.
///
/// Unknown formats use a fixed fallback. Known dimensions use one token per
/// 750 pixels, clamped to 1 through 4,500 tokens.
fn estimate_image_tokens(data_url: &str) -> i64 {
    let Some((header, payload)) = data_url.split_once(',') else {
        return IMAGE_FALLBACK_TOKENS;
    };
    let Some(bytes) = decode_base64_preview(payload) else {
        return IMAGE_FALLBACK_TOKENS;
    };
    let Some((width, height)) = image_dimensions(header, &bytes) else {
        return IMAGE_FALLBACK_TOKENS;
    };
    let pixels = width.saturating_mul(height);
    let tokens = pixels.saturating_add(IMAGE_TOKEN_DIVISOR - 1) / IMAGE_TOKEN_DIVISOR;
    (tokens as i64).clamp(1, IMAGE_TOKEN_CAP)
}

fn tool_output_content(output: &OutputKind) -> String {
    match output {
        OutputKind::Text { text } | OutputKind::ErrorText { text } => text.clone(),
        OutputKind::Json { value } | OutputKind::ErrorJson { value } => {
            serde_json::to_string(value).unwrap_or_default()
        }
        OutputKind::ExecutionDenied { reason } => reason.clone().unwrap_or_default(),
        OutputKind::Content { blocks } | OutputKind::ErrorContent { blocks } => {
            let mut content = String::new();
            for block in blocks {
                match &block.kind {
                    ResultBlockKind::Text { text } => content.push_str(text),
                    ResultBlockKind::Media { media } => content.push_str(&media_content(media)),
                    ResultBlockKind::Opaque { .. } => {}
                }
            }
            content
        }
    }
}

enum PartTokens {
    Zero,
    Bpe,
    Image,
}

fn part_measurement(kind: TailHygienePartKind, content: &str, tokens: PartTokens) -> MeasuredPart {
    let kind_name = match kind {
        TailHygienePartKind::Text => "text",
        TailHygienePartKind::ToolInput => "toolInput",
        TailHygienePartKind::ToolOutput => "toolOutput",
        TailHygienePartKind::File => "file",
        TailHygienePartKind::Excluded => "excluded",
    };
    let mut hash_input = String::with_capacity(kind_name.len() + content.len() + 1);
    hash_input.push_str(kind_name);
    hash_input.push('\0');
    hash_input.push_str(content);
    let digest = Sha256::digest(hash_input.as_bytes());
    let tokens = match tokens {
        PartTokens::Zero => 0,
        PartTokens::Bpe => crate::token_cache::count_with_digest(digest.into(), content) as i64,
        PartTokens::Image => estimate_image_tokens(content),
    };
    MeasuredPart {
        content_hash: format!("{digest:x}"),
        kind,
        tokens,
    }
}

fn excluded_part(content: &str) -> MeasuredPart {
    part_measurement(TailHygienePartKind::Excluded, content, PartTokens::Zero)
}

fn projection_message_indexes(projection: &FlatProjection) -> HashMap<&str, usize> {
    let mut indexes = HashMap::new();
    for block in &projection.blocks {
        let next = indexes.len();
        indexes.entry(block.mid.as_str()).or_insert(next);
    }
    indexes
}

fn neighborhood_consistent(
    orphan_tag_number: i64,
    message_index: usize,
    message_count: usize,
    bounds_by_message: &HashMap<usize, (i64, i64)>,
) -> bool {
    let previous_max = (0..=message_index)
        .filter_map(|index| bounds_by_message.get(&index).map(|(_, max)| *max))
        .max();
    let next_min = ((message_index + 1)..message_count)
        .filter_map(|index| bounds_by_message.get(&index).map(|(min, _)| *min))
        .min();
    matches!((previous_max, next_min), (Some(previous), Some(next)) if orphan_tag_number >= previous && orphan_tag_number <= next)
}

/// Part attribution uses exact block identities.
/// A legacy row with only a raw call ID uses the fallback only when one owner arc and its tag-number neighborhood are unambiguous.
/// Rows with recurring raw call IDs remain T-only when the fallback owner arc or tag-number neighborhood is ambiguous.
fn tag_numbers_by_block_and_arc<'a>(
    projection: &FlatProjection,
    tag_rows: impl IntoIterator<Item = &'a TagRow> + Clone,
) -> (HashMap<String, i64>, HashMap<String, i64>) {
    let block_ids = projection
        .blocks
        .iter()
        .map(|block| block.id.as_str())
        .collect::<HashSet<_>>();
    let message_indexes = projection_message_indexes(projection);
    let mut by_block = HashMap::new();
    let mut by_arc = HashMap::new();
    let mut bounds_by_message = HashMap::<usize, (i64, i64)>::new();

    for row in tag_rows
        .clone()
        .into_iter()
        .filter(|row| block_ids.contains(row.block_id.as_str()))
    {
        by_block.insert(row.block_id.clone(), row.tag_number);
        let Some(block) = projection
            .blocks
            .iter()
            .find(|block| block.id == row.block_id)
        else {
            continue;
        };
        if let Some(arc_id) = &block.arc_id {
            by_arc.entry(arc_id.clone()).or_insert(row.tag_number);
        }
        if let Some(index) = message_indexes.get(block.mid.as_str()) {
            bounds_by_message
                .entry(*index)
                .and_modify(|(min, max)| {
                    *min = (*min).min(row.tag_number);
                    *max = (*max).max(row.tag_number);
                })
                .or_insert((row.tag_number, row.tag_number));
        }
    }

    let call_ids = projection
        .blocks
        .iter()
        .filter_map(|block| block.tool_call_id.as_deref())
        .collect::<HashSet<_>>();
    let mut orphan_rows = HashMap::<&str, Vec<&TagRow>>::new();
    for row in tag_rows.into_iter().filter(|row| {
        !block_ids.contains(row.block_id.as_str()) && call_ids.contains(row.block_id.as_str())
    }) {
        orphan_rows
            .entry(row.block_id.as_str())
            .or_default()
            .push(row);
    }

    for (call_id, rows) in orphan_rows {
        if rows.len() != 1 {
            continue;
        }
        let candidate_arcs = projection
            .blocks
            .iter()
            .filter(|block| block.tool_call_id.as_deref() == Some(call_id))
            .filter_map(|block| block.arc_id.as_deref())
            .filter(|arc_id| !by_arc.contains_key(*arc_id))
            .collect::<BTreeSet<_>>();
        if candidate_arcs.len() != 1 {
            continue;
        }
        let arc_id = *candidate_arcs.first().expect("one candidate arc");
        let Some(owner_index) = projection
            .blocks
            .iter()
            .filter(|block| block.arc_id.as_deref() == Some(arc_id))
            .filter_map(|block| message_indexes.get(block.mid.as_str()).copied())
            .min()
        else {
            continue;
        };
        if neighborhood_consistent(
            rows[0].tag_number,
            owner_index,
            message_indexes.len(),
            &bounds_by_message,
        ) {
            by_arc.insert(arc_id.to_string(), rows[0].tag_number);
        }
    }

    (by_block, by_arc)
}

fn protected_tag_numbers<'a>(
    tag_rows: impl IntoIterator<Item = &'a TagRow>,
    protected_tags: usize,
) -> HashSet<i64> {
    if protected_tags == 0 {
        return HashSet::new();
    }
    tag_rows
        .into_iter()
        .map(|row| row.tag_number)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .rev()
        .take(protected_tags)
        .collect()
}

fn red_targets(core: &CoreState) -> HashSet<&str> {
    core.frozen_units
        .iter()
        .filter_map(|unit| unit.key.strip_prefix(RED_KEY_PREFIX))
        .collect()
}

fn caveman_units(core: &CoreState) -> HashMap<&str, &FrozenUnit> {
    let mut units = HashMap::new();
    for unit in &core.frozen_units {
        if let Some(id) = unit.key.strip_prefix(CAV_KEY_PREFIX) {
            units.entry(id).or_insert(unit);
        }
    }
    units
}

fn block_tag_number(
    block: &FlatBlock,
    by_block: &HashMap<String, i64>,
    by_arc: &HashMap<String, i64>,
) -> Option<i64> {
    by_block.get(&block.id).copied().or_else(|| {
        block
            .arc_id
            .as_ref()
            .and_then(|arc_id| by_arc.get(arc_id).copied())
    })
}

fn block_is_protected(
    block: &FlatBlock,
    tag_number: Option<i64>,
    protected_numbers: &HashSet<i64>,
    protected_block_ids: &HashSet<String>,
    protected_arc_ids: &HashSet<&str>,
) -> bool {
    tag_number.is_some_and(|number| protected_numbers.contains(&number))
        || protected_block_ids.contains(&block.id)
        || block
            .arc_id
            .as_deref()
            .is_some_and(|arc_id| protected_arc_ids.contains(arc_id))
}

/// Measures eligible projection parts in projection order.
///
/// Synthetic, system, covered, reduced, sentinel, reasoning, and opaque parts
/// contribute zero. Protected parts contribute to `T` but not `U`. Arithmetic
/// saturates, and the returned counts preserve `0 <= U <= T`.
/// Each clone of `tag_rows` must independently yield the same row sequence.
pub(crate) fn measure_tail_hygiene<'a>(
    projection: &FlatProjection,
    core: &CoreState,
    coverage_ordinal: Option<u64>,
    tag_rows: impl IntoIterator<Item = &'a TagRow> + Clone,
    protected_tags: usize,
    protected_block_ids: &HashSet<String>,
    memo: &mut TailHygieneMemo,
) -> TailHygieneMeasurement {
    let (tags_by_block, tags_by_arc) = tag_numbers_by_block_and_arc(projection, tag_rows.clone());
    let protected_numbers = protected_tag_numbers(tag_rows, protected_tags);
    let protected_arc_ids = projection
        .blocks
        .iter()
        .filter(|block| protected_block_ids.contains(&block.id))
        .filter_map(|block| block.arc_id.as_deref())
        .collect::<HashSet<_>>();
    let red_targets = red_targets(core);
    let reduced_arcs = projection
        .blocks
        .iter()
        .filter(|block| red_targets.contains(block.id.as_str()))
        .filter_map(|block| block.arc_id.as_deref())
        .collect::<HashSet<_>>();
    let sentinel_arcs = projection
        .blocks
        .iter()
        .filter_map(|block| {
            let memory_store::BlockKind::ToolResult { output, .. } = block.wire.kind() else {
                return None;
            };
            if is_drop_sentinel(&tool_output_content(&output.kind)) {
                block.arc_id.as_deref()
            } else {
                None
            }
        })
        .collect::<HashSet<_>>();

    let caveman_units = caveman_units(core);
    let mut parts = Vec::with_capacity(projection.blocks.len());
    let mut u = 0i64;
    let mut t = 0i64;
    for block in &projection.blocks {
        let key = format!("{}\0{}", block.id, block.kind_tag);
        let excluded = block.synthetic
            || block.role == "system"
            || coverage_ordinal.is_some_and(|coverage| block.ordinal <= coverage)
            || block
                .arc_id
                .as_deref()
                .is_some_and(|arc| reduced_arcs.contains(arc) || sentinel_arcs.contains(arc))
            || red_targets.contains(block.id.as_str());
        let caveman = caveman_units.get(block.id.as_str()).copied();

        let tag_number = block_tag_number(block, &tags_by_block, &tags_by_arc);
        let protected = block_is_protected(
            block,
            tag_number,
            &protected_numbers,
            protected_block_ids,
            &protected_arc_ids,
        );
        let measured = if let Some(measured) = memo.get(&key, block, caveman, excluded) {
            measured
        } else {
            let measured = if excluded {
                excluded_part(&block.bytes)
            } else {
                match block.wire.kind() {
                    memory_store::BlockKind::Text { text }
                        if block.role == "user" || block.role == "assistant" =>
                    {
                        let content =
                            caveman.map_or(text.as_str(), |unit| unit.frozen_payload.as_str());
                        let content = strip_channel1_reminder_spans(content);
                        if content.is_empty() || is_drop_sentinel(content) {
                            excluded_part(content)
                        } else {
                            part_measurement(TailHygienePartKind::Text, content, PartTokens::Bpe)
                        }
                    }
                    memory_store::BlockKind::ToolCall { input, .. } => {
                        let content = serde_json::to_string(input).unwrap_or_default();
                        part_measurement(TailHygienePartKind::ToolInput, &content, PartTokens::Bpe)
                    }
                    memory_store::BlockKind::ToolResult { output, .. } => {
                        let raw_content = tool_output_content(&output.kind);
                        let content = strip_channel1_reminder_spans(&raw_content);
                        if content.is_empty() || is_drop_sentinel(content) {
                            excluded_part(content)
                        } else {
                            part_measurement(
                                TailHygienePartKind::ToolOutput,
                                content,
                                PartTokens::Bpe,
                            )
                        }
                    }
                    memory_store::BlockKind::Media(media) => {
                        let content = media_content(media);
                        if content.is_empty() || is_drop_sentinel(&content) {
                            excluded_part(&content)
                        } else {
                            part_measurement(
                                TailHygienePartKind::File,
                                &content,
                                if matches!(media.kind, MediaKind::Image) {
                                    PartTokens::Image
                                } else {
                                    PartTokens::Bpe
                                },
                            )
                        }
                    }
                    memory_store::BlockKind::Reasoning { .. }
                    | memory_store::BlockKind::RedactedReasoning { .. }
                    | memory_store::BlockKind::Opaque(_) => excluded_part(&block.bytes),
                    memory_store::BlockKind::Text { .. } => excluded_part(&block.bytes),
                }
            };
            memo.insert(&key, block, caveman, excluded, &measured);
            measured
        };
        let active = measured.kind != TailHygienePartKind::Excluded;
        let tag_number = tag_number.filter(|_| active);
        let protected = active && protected;
        let measured = TailHygienePartMeasurement {
            key,
            content_hash: measured.content_hash,
            kind: measured.kind,
            tokens: measured.tokens,
            u_tokens: if tag_number.is_some() && !protected {
                measured.tokens
            } else {
                0
            },
            tag_number,
            tag_status: tag_number.map(|_| "active".to_string()),
            protected,
        };
        t = t.saturating_add(measured.tokens.max(0));
        u = u.saturating_add(measured.u_tokens.max(0));
        parts.push(measured);
    }
    memo.retain_parts(&parts);
    let mut signature_input = String::new();
    for part in &parts {
        let _ = write!(signature_input, "{}:{}\0", part.key, part.content_hash);
    }
    let t = t.max(0);
    TailHygieneMeasurement {
        u: u.clamp(0, t),
        t,
        content_signature: hex_digest(signature_input),
        parts,
    }
}

fn same_measured_prefix(
    baseline: &[TailHygienePartMeasurement],
    current: &[TailHygienePartMeasurement],
) -> Option<i64> {
    if current.len() < baseline.len() {
        return None;
    }
    let mut boundary_advance_u = 0i64;
    for (before, after) in baseline.iter().zip(current) {
        if before.key != after.key
            || before.content_hash != after.content_hash
            || before.kind != after.kind
            || before.tokens != after.tokens
            || before.tag_number != after.tag_number
            || before.tag_status != after.tag_status
            || (!before.protected && after.protected)
        {
            return None;
        }
        if before.protected && !after.protected {
            if after.tag_status.as_deref() != Some("active") {
                return None;
            }
            boundary_advance_u = boundary_advance_u.saturating_add(after.tokens);
        } else if before.u_tokens != after.u_tokens {
            return None;
        }
    }
    Some(boundary_advance_u)
}

/// Refreshes a baseline after a full cache bust or an append-only defer pass.
///
/// Non-append mutation invalidates evaluation until the next cache-busting
/// walk. Generation numbers increase only on full replacement.
pub(crate) fn refresh_tail_hygiene_baseline(
    measured: TailHygieneMeasurement,
    cache_busting: bool,
    previous: Option<&TailHygieneBaseline>,
    now_ms: i64,
) -> TailHygieneBaseline {
    if !cache_busting && previous.is_some_and(|baseline| baseline.generation_invalidated) {
        let mut baseline = previous.expect("checked previous baseline").clone();
        baseline.content_signature = measured.content_signature;
        return baseline;
    }
    if cache_busting || previous.is_none() {
        return TailHygieneBaseline {
            baseline_u: measured.u,
            baseline_t: measured.t,
            turn_delta_u: 0,
            turn_delta_t: 0,
            baseline_generation: previous
                .map_or(0, |baseline| baseline.baseline_generation)
                .saturating_add(1),
            computed_at_ms: now_ms,
            evaluable: true,
            generation_invalidated: false,
            baseline_parts: measured.parts,
            content_signature: measured.content_signature,
        };
    }

    let previous = previous.expect("non-busting refresh has a previous baseline");
    let Some(mut turn_delta_u) = same_measured_prefix(&previous.baseline_parts, &measured.parts)
    else {
        let mut invalidated = previous.clone();
        invalidated.evaluable = false;
        invalidated.generation_invalidated = true;
        invalidated.content_signature = measured.content_signature;
        return invalidated;
    };
    let mut turn_delta_t = 0i64;
    for part in &measured.parts[previous.baseline_parts.len()..] {
        turn_delta_t = turn_delta_t.saturating_add(part.tokens);
        // A just-completed output remains T-only in the newest recency reserve to prevent a defer pass from inflating U before the next full bust walk.
        // Keeping a just-completed output T-only prevents a defer pass from inflating U before the next full bust walk.
        if part.kind != TailHygienePartKind::ToolOutput {
            turn_delta_u = turn_delta_u.saturating_add(part.u_tokens);
        }
    }
    TailHygieneBaseline {
        turn_delta_u,
        turn_delta_t,
        evaluable: true,
        generation_invalidated: false,
        content_signature: measured.content_signature,
        ..previous.clone()
    }
}

pub(crate) fn effective_tail_hygiene(baseline: &TailHygieneBaseline) -> (i64, i64) {
    let t = baseline
        .baseline_t
        .saturating_add(baseline.turn_delta_t)
        .max(0);
    let u = baseline
        .baseline_u
        .saturating_add(baseline.turn_delta_u)
        .clamp(0, t);
    (u, t)
}

/// Classifies clamped token counts using fixed floor and severity thresholds.
pub(crate) fn hygiene_band(u: i64, t: i64) -> HygieneBand {
    let t = t.max(0);
    let u = u.clamp(0, t);
    if t < CHANNEL1_MIN_TOKENS || u < CHANNEL1_FLOOR_TOKENS {
        return HygieneBand::Quiet;
    }
    let severity = u as f64 / t.max(1) as f64;
    if u >= CHANNEL2_FLOOR_TOKENS && severity >= CHANNEL2_SEVERITY_THRESHOLD {
        HygieneBand::Channel2
    } else if severity >= 0.60 {
        HygieneBand::Urgent
    } else if severity >= 0.40 {
        HygieneBand::Firm
    } else if severity >= 0.20 {
        HygieneBand::Gentle
    } else {
        HygieneBand::Quiet
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{IngressMessage, project_messages};
    use memory_store::{
        BlockKind, HarnessMeta, MediaBlock, MediaKind, ProviderExtras, ToolOutput, WireBlock,
        WireMessage,
    };
    use serde::{Deserialize, Serialize};
    use serde_json::{Value, json};
    use std::sync::Arc;

    fn message(mid: &str, ordinal: u64, role: &str, blocks: Vec<BlockKind>) -> Arc<IngressMessage> {
        Arc::new(IngressMessage {
            mid: mid.to_string(),
            ordinal,
            ck: WireMessage::from_parts(
                role,
                blocks.into_iter().map(WireBlock::bare).collect(),
                None,
                ProviderExtras::new(),
                HarnessMeta::default(),
            ),
        })
    }

    fn text(mid: &str, ordinal: u64, value: &str) -> Arc<IngressMessage> {
        message(
            mid,
            ordinal,
            "user",
            vec![BlockKind::Text {
                text: value.to_string(),
            }],
        )
    }

    fn tag(number: i64, block_id: &str) -> TagRow {
        TagRow {
            tag_number: number,
            block_id: block_id.to_string(),
            kind: "message".to_string(),
            token_count: 0,
            created_at_ms: 0,
            source_bytes: Vec::new(),
        }
    }

    #[test]
    fn measurement_is_identical_with_cold_and_warm_token_cache() {
        let messages = vec![
            text("a", 1, &"the retry loop needs jittered backoff ".repeat(20)),
            message(
                "b",
                2,
                "assistant",
                vec![BlockKind::ToolCall {
                    id: "call_1".to_string(),
                    name: "bash".to_string(),
                    input: json!({"command": "grep -rn pattern src/ && cargo test -p daemon"}),
                    provider_executed: false,
                }],
            ),
            message(
                "c",
                3,
                "tool",
                vec![BlockKind::ToolResult {
                    id: "call_1".to_string(),
                    tool_name: "bash".to_string(),
                    output: ToolOutput::bare(memory_store::OutputKind::Text {
                        text: "src/lib.rs:42: pattern matched here\n".repeat(30),
                    }),
                    provider_executed: false,
                }],
            ),
        ];
        let tags = vec![tag(1, "a#0")];
        let projection = project_messages(&messages).unwrap();
        let mut memo = TailHygieneMemo::default();
        let measure = |memo: &mut TailHygieneMemo| {
            measure_tail_hygiene(
                &projection,
                &CoreState::empty(),
                None,
                &tags,
                0,
                &HashSet::new(),
                memo,
            )
        };
        let _cache_guard = crate::token_cache::test_cache_guard();
        crate::token_cache::clear();
        let cold = measure(&mut memo);
        let before = crate::token_cache::local_stats();
        let warm = measure(&mut memo);
        assert_eq!(
            crate::token_cache::local_stats(),
            before,
            "memo hit must skip token lookup"
        );
        assert_eq!(cold, warm, "warm-cache measurement diverged from cold");
        crate::token_cache::clear();
        let recold = measure(&mut TailHygieneMemo::default());
        assert_eq!(cold, recold, "cache clear changed the measurement");
        assert!(cold.t > 0, "fixture must measure real tokens");
    }

    #[test]
    fn hygiene_digest_and_token_key_use_kind_prefixed_content() {
        let content = "same content across distinct part kinds ".repeat(10);
        let projection = project_messages(&[text("digest", 1, &content)]).unwrap();
        let block = &projection.blocks[0];
        let expected: [u8; 32] = Sha256::digest(format!("text\0{content}")).into();
        assert_ne!(expected, block.content_hash);
        let _guard = crate::token_cache::test_cache_guard();
        crate::token_cache::clear();
        crate::token_cache::count_with_digest(block.content_hash, "projection key poison");
        let before = crate::token_cache::local_stats();
        let mut memo = TailHygieneMemo::default();
        let measured = measure_tail_hygiene(
            &projection,
            &CoreState::empty(),
            None,
            &[],
            0,
            &HashSet::new(),
            &mut memo,
        );
        let entry = &memo.entries["digest#0\0text"];
        assert_eq!(entry.measured.content_hash, measured.parts[0].content_hash);
        assert_ne!(
            entry.measured.content_hash,
            format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(block.wire.as_ref()).unwrap())
            )
        );
        assert_eq!(
            measured.parts[0].content_hash,
            hex_digest(format!("text\0{content}"))
        );
        assert_eq!(measured.t, estimated_tokens(&content));
        assert_eq!(crate::token_cache::local_stats().misses - before.misses, 1);
        let before = crate::token_cache::local_stats();
        assert_eq!(
            crate::token_cache::count_with_digest(expected, &content) as i64,
            measured.t
        );
        assert_eq!(crate::token_cache::local_stats().hits - before.hits, 1);
        assert_ne!(
            expected,
            Sha256::digest(format!("toolOutput\0{content}")).as_slice()
        );
    }

    #[test]
    fn memo_preserves_each_derived_digest_domain() {
        let messages = vec![
            message(
                "text",
                1,
                "assistant",
                vec![BlockKind::Text {
                    text: "source\n\n<system-reminder>\nnudge\n</system-reminder>".into(),
                }],
            ),
            message(
                "input",
                2,
                "assistant",
                vec![BlockKind::ToolCall {
                    id: "call".into(),
                    name: "read".into(),
                    input: json!({"path":"x"}),
                    provider_executed: false,
                }],
            ),
            message(
                "output",
                3,
                "tool",
                vec![BlockKind::ToolResult {
                    id: "call".into(),
                    tool_name: "read".into(),
                    provider_executed: false,
                    output: ToolOutput::bare(OutputKind::Content {
                        blocks: vec![memory_store::ResultBlock {
                            kind: ResultBlockKind::Text {
                                text: "result".into(),
                            },
                            provider_extras: ProviderExtras::new(),
                        }],
                    }),
                }],
            ),
            message(
                "file",
                4,
                "user",
                vec![BlockKind::Media(MediaBlock {
                    kind: MediaKind::File,
                    media_type: "text/plain".into(),
                    filename: None,
                    source: json!("file:///x"),
                })],
            ),
            text("empty", 5, ""),
            text("sentinel", 6, "[dropped text]"),
            message(
                "reasoning",
                7,
                "assistant",
                vec![BlockKind::Reasoning {
                    text: "private".into(),
                    signature: None,
                }],
            ),
        ];
        let projection = project_messages(&messages).unwrap();
        let expected = [
            ("text", "source"),
            ("toolInput", "{\"path\":\"x\"}"),
            ("toolOutput", "result"),
            ("file", "file:///x"),
            ("excluded", ""),
            ("excluded", "[dropped text]"),
            ("excluded", projection.blocks[6].bytes.as_ref()),
        ];
        let mut memo = TailHygieneMemo::default();
        for _ in 0..2 {
            let measured = measure_tail_hygiene(
                &projection,
                &CoreState::empty(),
                None,
                &[],
                0,
                &HashSet::new(),
                &mut memo,
            );
            for ((part, block), (kind, content)) in
                measured.parts.iter().zip(&projection.blocks).zip(expected)
            {
                let expected = format!("{:x}", Sha256::digest(format!("{kind}\0{content}")));
                assert_eq!(part.content_hash, expected);
                assert_ne!(
                    part.content_hash,
                    format!(
                        "{:x}",
                        Sha256::digest(serde_json::to_vec(block.wire.as_ref()).unwrap())
                    )
                );
                assert_eq!(memo.entries[&part.key].measured.content_hash, expected);
            }
        }
    }

    #[test]
    fn memo_rechecks_caveman_identity_payload_and_context() {
        let mut projection = project_messages(&[text("m", 1, "original text")]).unwrap();
        let mut core = CoreState::empty();
        let mut memo = TailHygieneMemo::default();
        let mut tags = vec![tag(1, "m#0")];
        let mut coverage = None;
        let mut protected_tags = 0;
        let mut protected = HashSet::new();
        for change in [
            "original",
            "same-id-edit",
            "caveman",
            "duplicate-caveman",
            "unit-kind",
            "unit-reset",
            "unit-durability",
            "payload",
            "unit-key",
            "remove-unit",
            "tag",
            "protected-tag",
            "protected-id",
            "unprotected",
            "coverage",
            "uncovered",
            "reduced",
            "unreduced",
            "system",
            "tool-role",
            "user",
            "synthetic",
            "ordinary",
        ] {
            match change {
                "same-id-edit" => {
                    projection = project_messages(&[text("m", 1, "modified text")]).unwrap()
                }
                "caveman" => core.frozen_units.push(FrozenUnit {
                    key: "cav:m#0".into(),
                    kind: "caveman".into(),
                    frozen_payload: "compact".into(),
                    durability_class: cache_stability::DurabilityClass::Lineage,
                    reset_rule: String::new(),
                }),
                "unit-kind" => core.frozen_units[0].kind = "alternate".into(),
                "duplicate-caveman" => {
                    let mut duplicate = core.frozen_units[0].clone();
                    duplicate.frozen_payload = "ignored duplicate payload".into();
                    core.frozen_units.push(duplicate);
                }
                "unit-reset" => core.frozen_units[0].reset_rule = "reset".into(),
                "unit-durability" => {
                    core.frozen_units[0].durability_class =
                        cache_stability::DurabilityClass::Episode
                }
                "payload" => core.frozen_units[0].frozen_payload = "changed compact payload".into(),
                "unit-key" => core.frozen_units[0].key = "cav:other#0".into(),
                "remove-unit" | "unreduced" => core.frozen_units.clear(),
                "tag" => tags[0].tag_number = 9,
                "protected-tag" => protected_tags = 1,
                "protected-id" => {
                    protected_tags = 0;
                    protected.insert("m#0".to_string());
                }
                "unprotected" => protected.clear(),
                "coverage" => coverage = Some(1),
                "uncovered" => coverage = None,
                "reduced" => core.frozen_units.push(FrozenUnit {
                    key: "red:m#0".into(),
                    kind: "red".into(),
                    frozen_payload: "redacted".into(),
                    durability_class: cache_stability::DurabilityClass::Lineage,
                    reset_rule: String::new(),
                }),
                "system" => projection.blocks[0].role = "system".into(),
                "tool-role" => projection.blocks[0].role = "tool".into(),
                "user" => projection.blocks[0].role = "user".into(),
                "synthetic" => projection.blocks[0].synthetic = true,
                "ordinary" => projection.blocks[0].synthetic = false,
                "original" => {}
                _ => unreachable!(),
            }
            let before = crate::token_cache::local_stats();
            let measured = measure_tail_hygiene(
                &projection,
                &core,
                coverage,
                &tags,
                protected_tags,
                &protected,
                &mut memo,
            );
            if matches!(change, "unit-kind" | "unit-reset" | "unit-durability") {
                assert_eq!(
                    crate::token_cache::local_stats().calls - before.calls,
                    1,
                    "{change} must invalidate despite equal payload"
                );
            }
            let fresh = measure_tail_hygiene(
                &projection,
                &core,
                coverage,
                &tags,
                protected_tags,
                &protected,
                &mut TailHygieneMemo::default(),
            );
            assert_eq!(measured, fresh, "{change}");
            let excluded = matches!(
                change,
                "coverage" | "reduced" | "system" | "tool-role" | "synthetic"
            );
            let content = if excluded {
                projection.blocks[0].bytes.as_ref()
            } else if let Some(unit) = core.frozen_units.iter().find(|unit| unit.key == "cav:m#0") {
                &unit.frozen_payload
            } else if change == "original" {
                "original text"
            } else {
                "modified text"
            };
            let prefix = if excluded { "excluded" } else { "text" };
            assert_eq!(
                measured.parts[0].content_hash,
                format!("{:x}", Sha256::digest(format!("{prefix}\0{content}"))),
                "{change}"
            );
            assert_eq!(
                measured.parts[0].tag_number,
                (!excluded).then_some(tags[0].tag_number)
            );
            assert_eq!(
                measured.u == 0,
                excluded || protected_tags > 0 || !protected.is_empty()
            );
        }
    }

    #[test]
    fn memo_bounds_sessions_bytes_resets_and_oversize_bypass() {
        let projection = project_messages(&[text("m", 1, "measured content")]).unwrap();
        let measure = |memo: &mut TailHygieneMemo| {
            measure_tail_hygiene(
                &projection,
                &CoreState::empty(),
                None,
                &[],
                0,
                &HashSet::new(),
                memo,
            )
        };
        let memos = HygieneMemos::default();
        let ids = slot_sessions(&memos);
        let a = &ids[0][0];
        let b = &ids[1][0];
        let expected = memos.with_session(1, a, measure);
        memos.with_session(1, b, measure);
        let before = crate::token_cache::local_stats();
        memos.with_session(1, a, measure);
        memos.with_session(1, b, measure);
        memos.with_session(1, a, measure);
        assert_eq!(
            crate::token_cache::local_stats(),
            before,
            "interleaved sessions stay warm"
        );
        memos.remove(1, a);
        memos.with_session(1, a, |memo| assert!(memo.entries.is_empty()));
        let before_bytes = memos.retained_bytes();
        assert_eq!(
            memos.with_session(1, &"x".repeat(MEMO_SESSION_BYTES + 1), measure),
            expected
        );
        assert_eq!(
            memos.retained_bytes(),
            before_bytes,
            "oversize session retains nothing"
        );
        assert!(memos.retained_bytes() <= MEMO_RETAINED_BYTES_BOUND);
        memos.clear();
        assert_eq!(
            memos.retained_bytes(),
            std::mem::size_of::<OnceLock<HygieneMemos>>()
        );
        let mut memo = TailHygieneMemo::default();
        measure(&mut memo);
        assert_memo_accounting(&memo);
        let expanded =
            project_messages(&[text("m", 1, "measured content"), text("n", 2, "other")]).unwrap();
        measure_tail_hygiene(
            &expanded,
            &CoreState::empty(),
            None,
            &[],
            0,
            &HashSet::new(),
            &mut memo,
        );
        let allocated = memo.map_bytes;
        measure(&mut memo);
        assert_memo_accounting(&memo);
        assert_eq!(
            memo.map_bytes, allocated,
            "pruning must keep charging the live table"
        );
        measure_tail_hygiene(
            &expanded,
            &CoreState::empty(),
            None,
            &[],
            0,
            &HashSet::new(),
            &mut memo,
        );
        assert_memo_accounting(&memo);
        let mut bounded = TailHygieneMemo::new(0);
        assert_eq!(measure(&mut bounded), expected);
        assert!(bounded.entries.is_empty());
        assert_eq!(
            bounded.retained_bytes(),
            0,
            "eviction must release bucket storage"
        );
        let mut core = CoreState::empty();
        core.frozen_units.push(FrozenUnit {
            key: "cav:m#0".into(),
            kind: "caveman".into(),
            frozen_payload: "x".repeat(MEMO_SESSION_BYTES + 1),
            reset_rule: String::new(),
            durability_class: cache_stability::DurabilityClass::Lineage,
        });
        measure_tail_hygiene(&projection, &core, None, &[], 0, &HashSet::new(), &mut memo);
        assert_eq!(
            memo.retained_bytes(),
            0,
            "oversize caveman payload must bypass"
        );
        assert_memo_accounting(&memo);
        measure(&mut memo);
        assert_memo_accounting(&memo);
        measure_tail_hygiene(
            &project_messages(&[]).unwrap(),
            &CoreState::empty(),
            None,
            &[],
            0,
            &HashSet::new(),
            &mut memo,
        );
        assert_eq!(
            memo.retained_bytes(),
            0,
            "empty session drops maps and data"
        );
        assert_memo_accounting(&memo);
        let many = project_messages(
            &(0..64)
                .map(|i| text(&format!("m{i}"), i, "tail text"))
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let expected = measure_tail_hygiene(
            &many,
            &CoreState::empty(),
            None,
            &[],
            0,
            &HashSet::new(),
            &mut TailHygieneMemo::new(0),
        );
        let mut bounded = TailHygieneMemo::new(512);
        for _ in 0..3 {
            assert_eq!(
                measure_tail_hygiene(
                    &many,
                    &CoreState::empty(),
                    None,
                    &[],
                    0,
                    &HashSet::new(),
                    &mut bounded
                ),
                expected
            );
            assert_memo_accounting(&bounded);
            assert!(bounded.retained_bytes() <= 512);
        }
    }

    fn slot_sessions(pool: &HygieneMemos) -> [Vec<String>; MEMO_SESSION_LIMIT] {
        let mut ids: [Vec<String>; MEMO_SESSION_LIMIT] = std::array::from_fn(|_| Vec::new());
        for i in 0..65_536 {
            let id = format!("slot-session-{i}");
            let bucket = &mut ids[pool.slot_index(1, &id)];
            if bucket.len() < 2 {
                bucket.push(id);
            }
            if ids.iter().all(|bucket| bucket.len() == 2) {
                return ids;
            }
        }
        panic!("hash fixture did not populate every slot");
    }

    fn assert_memo_accounting(memo: &TailHygieneMemo) -> usize {
        let heap = memo
            .entries
            .iter()
            .map(|(key, entry)| {
                key.capacity()
                    + entry.measured.content_hash.capacity()
                    + entry.caveman.as_ref().map_or(0, |unit| {
                        unit.key.capacity()
                            + unit.kind.capacity()
                            + unit.frozen_payload.capacity()
                            + unit.reset_rule.capacity()
                    })
            })
            .sum::<usize>();
        let table = if memo.entries.capacity() == 0 {
            0
        } else {
            (memo.entries.capacity() * 8).div_ceil(7)
                * (std::mem::size_of::<String>() + std::mem::size_of::<MemoEntry>() + 1)
                + 16
        };
        assert_eq!(memo.heap_bytes, heap);
        assert!(memo.map_bytes >= table);
        assert_eq!(memo.retained_bytes(), heap + memo.map_bytes);
        assert!(memo.retained_bytes() <= memo.budget);
        heap + table
    }

    #[test]
    fn memo_slot_collisions_namespace_reentry_and_poison_are_cold_misses() {
        use std::panic::{AssertUnwindSafe, catch_unwind};
        let pool = HygieneMemos::default();
        let ids = slot_sessions(&pool);
        let a = &ids[0][0];
        let collision = &ids[0][1];
        let namespace_b = (2..65_536).find(|&ns| pool.slot_index(ns, a) == 0).unwrap();
        let mut last = None;
        for (namespace, id, content) in [
            (1, a, "alpha"),
            (1, collision, "foreign beta"),
            (1, a, "alpha"),
            (namespace_b, a, "other namespace"),
            (1, a, "alpha"),
        ] {
            let projection = project_messages(&[text("same", 1, content)]).unwrap();
            let measure = |memo: &mut TailHygieneMemo| {
                measure_tail_hygiene(
                    &projection,
                    &CoreState::empty(),
                    None,
                    &[],
                    0,
                    &HashSet::new(),
                    memo,
                )
            };
            let reference = measure(&mut TailHygieneMemo::new(0));
            let measured = pool.with_session(namespace, id, |memo| {
                assert!(
                    memo.entries.is_empty(),
                    "occupant mismatch must discard entries"
                );
                let result = measure(memo);
                assert_memo_accounting(memo);
                result
            });
            assert_eq!(measured, reference);
            if let Some(last) = last {
                assert_ne!(measured, last);
            }
            last = Some(measured);
        }
        pool.remove(namespace_b, a);
        pool.with_session(1, a, |memo| assert!(!memo.entries.is_empty()));
        let projection = project_messages(&[text("same", 1, "alpha")]).unwrap();
        let measure = |memo: &mut TailHygieneMemo| {
            measure_tail_hygiene(
                &projection,
                &CoreState::empty(),
                None,
                &[],
                0,
                &HashSet::new(),
                memo,
            )
        };
        for recovery in ["use", "remove", "remove-session", "clear"] {
            assert!(
                catch_unwind(AssertUnwindSafe(|| pool.with_session(1, a, |memo| {
                    measure(memo);
                    memo.heap_bytes = usize::MAX;
                    memo.map_bytes = usize::MAX;
                    panic!("interrupted memo update");
                })))
                .is_err()
            );
            assert!(pool.slots[0].is_poisoned());
            match recovery {
                "remove" => pool.remove(1, a),
                "remove-session" => pool.remove_session(a),
                "clear" => pool.clear(),
                "use" => {}
                _ => unreachable!(),
            }
            pool.with_session(1, a, |memo| {
                assert!(memo.entries.is_empty());
                assert_eq!(memo.retained_bytes(), 0);
                assert_eq!(measure(memo), measure(&mut TailHygieneMemo::new(0)));
                assert_memo_accounting(memo);
            });
            assert!(!pool.slots[0].is_poisoned());
        }
        pool.with_session(namespace_b, a, measure);
        pool.remove_session(a);
        pool.with_session(1, a, |memo| assert!(memo.entries.is_empty()));
        pool.with_session(namespace_b, a, |memo| assert!(memo.entries.is_empty()));
    }

    #[test]
    fn noncolliding_memo_sessions_overlap_inside_slot_locks() {
        use std::sync::mpsc;
        use std::time::Duration;
        let pool = HygieneMemos::default();
        let ids = slot_sessions(&pool);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_a, wait_a) = mpsc::channel();
        let (release_b, wait_b) = mpsc::channel();
        std::thread::scope(|scope| {
            for (id, wait) in [(&ids[0][0], wait_a), (&ids[1][0], wait_b)] {
                let entered = &entered_tx;
                let pool = &pool;
                scope.spawn(move || {
                    pool.with_session(1, id, |_| {
                        entered.send(()).unwrap();
                        wait.recv_timeout(Duration::from_secs(10)).unwrap();
                    })
                });
            }
            let both_entered = entered_rx.recv_timeout(Duration::from_secs(5)).is_ok()
                && entered_rx.recv_timeout(Duration::from_secs(5)).is_ok();
            assert!(pool.slots[0].try_lock().is_err());
            release_a.send(()).unwrap();
            release_b.send(()).unwrap();
            assert!(
                both_entered,
                "distinct slots must reach the gate before either is released"
            );
        });
    }

    #[test]
    fn all_memo_slots_near_budget_match_independent_retained_accounting() {
        let pool = HygieneMemos::default();
        let ids = slot_sessions(&pool);
        let projection = project_messages(&[text("m", 1, "source")]).unwrap();
        let mut core = CoreState::empty();
        core.frozen_units.push(FrozenUnit {
            key: "cav:m#0".into(),
            kind: "caveman".into(),
            frozen_payload: "x".repeat(MEMO_SESSION_BYTES - 8192),
            reset_rule: String::new(),
            durability_class: cache_stability::DurabilityClass::Lineage,
        });
        let mut independent = std::mem::size_of::<OnceLock<HygieneMemos>>();
        for bucket in &ids {
            let id = &bucket[0];
            pool.with_session(1, id, |memo| {
                measure_tail_hygiene(&projection, &core, None, &[], 0, &HashSet::new(), memo);
                let bytes = id.len() + assert_memo_accounting(memo);
                assert!(bytes > MEMO_SESSION_BYTES - 16_384 && bytes <= MEMO_SESSION_BYTES);
                independent += bytes;
            });
        }
        assert_eq!(
            pool.slots
                .iter()
                .filter(|slot| HygieneMemos::lock_slot(slot).is_some())
                .count(),
            16
        );
        assert_eq!(pool.retained_bytes(), independent);
        assert_eq!(
            MEMO_RETAINED_BYTES_BOUND,
            std::mem::size_of::<OnceLock<HygieneMemos>>() + 16 * MEMO_SESSION_BYTES
        );
        assert!(independent <= MEMO_RETAINED_BYTES_BOUND);
        pool.clear();
        assert_eq!(
            pool.retained_bytes(),
            std::mem::size_of::<OnceLock<HygieneMemos>>()
        );
    }

    #[test]
    fn defer_delta_and_boundary_advance_are_additive() {
        let mut memo = TailHygieneMemo::default();
        let base = vec![
            text("old", 1, &"old mass ".repeat(2_000)),
            text("recent", 2, "recent"),
        ];
        let tags = vec![tag(1, "old#0"), tag(2, "recent#0")];
        let projection = project_messages(&base).unwrap();
        let measured = measure_tail_hygiene(
            &projection,
            &CoreState::empty(),
            None,
            &tags,
            2,
            &HashSet::new(),
            &mut memo,
        );
        let baseline = refresh_tail_hygiene_baseline(measured, true, None, 10);
        assert_eq!(baseline.baseline_u, 0);

        let mut appended = base;
        appended.push(text("new", 3, &"new mass ".repeat(2_000)));
        let tags = vec![tag(1, "old#0"), tag(2, "recent#0"), tag(3, "new#0")];
        let projection = project_messages(&appended).unwrap();
        let measured = measure_tail_hygiene(
            &projection,
            &CoreState::empty(),
            None,
            &tags,
            2,
            &HashSet::new(),
            &mut memo,
        );
        let defer = refresh_tail_hygiene_baseline(measured, false, Some(&baseline), 20);
        assert!(defer.evaluable);
        assert!(defer.turn_delta_t > 0);
        assert!(
            defer.turn_delta_u > 0,
            "old protected mass should advance into U"
        );
        assert_eq!(defer.baseline_generation, baseline.baseline_generation);
    }

    #[test]
    fn non_append_mutation_invalidates_until_a_bust() {
        let mut memo = TailHygieneMemo::default();
        let messages = vec![text("m", 1, "original")];
        let tags = vec![tag(1, "m#0")];
        let projection = project_messages(&messages).unwrap();
        let baseline = refresh_tail_hygiene_baseline(
            measure_tail_hygiene(
                &projection,
                &CoreState::empty(),
                None,
                &tags,
                0,
                &HashSet::new(),
                &mut memo,
            ),
            true,
            None,
            10,
        );
        let projection = project_messages(&[text("m", 1, "changed")]).unwrap();
        let invalid = refresh_tail_hygiene_baseline(
            measure_tail_hygiene(
                &projection,
                &CoreState::empty(),
                None,
                &tags,
                0,
                &HashSet::new(),
                &mut memo,
            ),
            false,
            Some(&baseline),
            20,
        );
        assert!(!invalid.evaluable);
        assert!(invalid.generation_invalidated);
    }

    #[derive(Debug, Deserialize)]
    struct HygieneGolden {
        schema: u32,
        provenance: HygieneGoldenProvenance,
        cases: Vec<HygieneGoldenCase>,
    }

    #[derive(Debug, Deserialize)]
    struct HygieneGoldenProvenance {
        generator_version: String,
        input_sha256: String,
    }

    #[derive(Debug, Deserialize)]
    struct HygieneGoldenCase {
        id: String,
        protected_tags: usize,
        messages: Vec<HygieneFixtureMessage>,
        tags: Vec<HygieneFixtureTag>,
        expected: HygieneExpected,
    }

    fn is_false(value: &bool) -> bool {
        !*value
    }

    #[derive(Debug, Deserialize, Serialize)]
    struct HygieneFixtureMessage {
        mid: String,
        ordinal: u64,
        role: String,
        #[serde(default, skip_serializing_if = "is_false")]
        synthetic: bool,
        blocks: Vec<HygieneFixtureBlock>,
    }

    #[derive(Debug, Deserialize, Serialize)]
    #[serde(tag = "type", rename_all = "snake_case")]
    enum HygieneFixtureBlock {
        Text {
            unit: String,
            repeat: usize,
        },
        Reasoning {
            unit: String,
            repeat: usize,
        },
        ToolCall {
            id: String,
            name: String,
            input: Value,
        },
        ToolResult {
            id: String,
            name: String,
            unit: String,
            repeat: usize,
        },
        File {
            mime: String,
            url: String,
        },
    }

    #[derive(Debug, Deserialize, Serialize)]
    struct HygieneFixtureTag {
        tag_number: i64,
        block_id: String,
        kind: String,
    }

    #[derive(Debug, Deserialize)]
    struct HygieneExpected {
        u: i64,
        t: i64,
        band: String,
    }

    #[derive(Serialize)]
    struct HygieneFixtureInput<'a> {
        id: &'a str,
        protected_tags: usize,
        messages: &'a [HygieneFixtureMessage],
        tags: &'a [HygieneFixtureTag],
    }

    fn hygiene_fixture_canonical(cases: &[HygieneGoldenCase]) -> String {
        let input = cases
            .iter()
            .map(|case| HygieneFixtureInput {
                id: &case.id,
                protected_tags: case.protected_tags,
                messages: &case.messages,
                tags: &case.tags,
            })
            .collect::<Vec<_>>();
        format!(
            "{}\n",
            serde_json::to_string_pretty(&input).expect("serialize hygiene fixture inputs")
        )
    }

    fn hygiene_fixture_hash(cases: &[HygieneGoldenCase]) -> String {
        format!(
            "{:x}",
            Sha256::digest(hygiene_fixture_canonical(cases).as_bytes())
        )
    }

    fn fixture_message(input: &HygieneFixtureMessage) -> Arc<IngressMessage> {
        let blocks = input
            .blocks
            .iter()
            .map(|block| match block {
                HygieneFixtureBlock::Text { unit, repeat } => BlockKind::Text {
                    text: unit.repeat(*repeat),
                },
                HygieneFixtureBlock::Reasoning { unit, repeat } => BlockKind::Reasoning {
                    text: unit.repeat(*repeat),
                    signature: Some("fixture-signature".to_string()),
                },
                HygieneFixtureBlock::ToolCall { id, name, input } => BlockKind::ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                    provider_executed: false,
                },
                HygieneFixtureBlock::ToolResult {
                    id,
                    name,
                    unit,
                    repeat,
                } => BlockKind::ToolResult {
                    id: id.clone(),
                    tool_name: name.clone(),
                    output: ToolOutput::bare(OutputKind::Text {
                        text: unit.repeat(*repeat),
                    }),
                    provider_executed: false,
                },
                HygieneFixtureBlock::File { mime, url } => BlockKind::Media(MediaBlock {
                    kind: if mime.starts_with("image/") {
                        MediaKind::Image
                    } else {
                        MediaKind::File
                    },
                    media_type: mime.clone(),
                    filename: None,
                    source: Value::String(url.clone()),
                }),
            })
            .map(WireBlock::bare)
            .collect();
        Arc::new(IngressMessage {
            mid: input.mid.clone(),
            ordinal: input.ordinal,
            ck: WireMessage::from_parts(
                input.role.clone(),
                blocks,
                None,
                ProviderExtras::new(),
                HarnessMeta {
                    synthetic: input.synthetic,
                    ..HarnessMeta::default()
                },
            ),
        })
    }

    fn fixture_tag(input: &HygieneFixtureTag) -> TagRow {
        TagRow {
            tag_number: input.tag_number,
            block_id: input.block_id.clone(),
            kind: input.kind.clone(),
            token_count: 0,
            created_at_ms: 0,
            source_bytes: Vec::new(),
        }
    }

    #[test]
    fn parity_golden_matches_ts_reference_across_full_corpus() {
        let golden: HygieneGolden =
            serde_json::from_str(include_str!("../testdata/nudge-hygiene-golden.json"))
                .expect("parse nudge hygiene golden");
        assert_eq!(golden.schema, 1);
        assert_eq!(golden.provenance.generator_version, "nudge-hygiene-ts-v2");
        assert_eq!(
            hygiene_fixture_hash(&golden.cases),
            golden.provenance.input_sha256,
            "committed fixture inputs must match the TypeScript generator provenance"
        );
        assert!(golden.cases.len() >= 12);

        let mut frozen = Sha256::new();
        for case in &golden.cases {
            let messages = case
                .messages
                .iter()
                .map(fixture_message)
                .collect::<Vec<_>>();
            let tags = case.tags.iter().map(fixture_tag).collect::<Vec<_>>();
            let projection = project_messages(&messages).expect("project parity fixture");
            let mut memo = TailHygieneMemo::default();
            let measured = measure_tail_hygiene(
                &projection,
                &CoreState::empty(),
                None,
                &tags,
                case.protected_tags,
                &HashSet::new(),
                &mut memo,
            );
            let warm = measure_tail_hygiene(
                &projection,
                &CoreState::empty(),
                None,
                &tags,
                case.protected_tags,
                &HashSet::new(),
                &mut memo,
            );
            assert_eq!(measured, warm, "{} warm parity", case.id);
            frozen.update(
                serde_json::to_vec(&(
                    measured.u,
                    measured.t,
                    &measured.content_signature,
                    &measured.parts,
                ))
                .unwrap(),
            );
            for (label, rust, ts) in [
                ("U", measured.u, case.expected.u),
                ("T", measured.t, case.expected.t),
            ] {
                let tolerance = 12.max((ts.unsigned_abs() as f64 * 0.03).ceil() as i64);
                assert!(
                    (rust - ts).abs() <= tolerance,
                    "{} {label} drifted outside tokenizer tolerance: Rust={rust}, TS={ts}, tolerance={tolerance}",
                    case.id
                );
            }
            if case.id == "reasoning-excluded-both-terms" {
                let reasoning_tokens = case
                    .messages
                    .iter()
                    .flat_map(|message| &message.blocks)
                    .filter_map(|block| match block {
                        HygieneFixtureBlock::Reasoning { unit, repeat } => {
                            Some(estimated_tokens(&unit.repeat(*repeat)))
                        }
                        _ => None,
                    })
                    .sum::<i64>();
                let tolerance =
                    12.max((case.expected.t.unsigned_abs() as f64 * 0.03).ceil() as i64);
                assert!(
                    (measured.t + reasoning_tokens - case.expected.t).abs() > tolerance,
                    "Rust reasoning-arm counting mutant must redden its parity leg"
                );
            }
            assert_eq!(
                hygiene_band(measured.u, measured.t).as_str(),
                case.expected.band,
                "{} band drifted",
                case.id
            );
            assert!(measured.u <= measured.t, "{} violated U subset T", case.id);
        }
        assert_eq!(
            format!("{:x}", frozen.finalize()),
            "01a4b82d5f0ce2853388f8c5e4f81509ca4e8b0bef4cb98d964f24ffd3deb4bf"
        );
    }

    #[test]
    fn shared_row_iterator_matches_slice_for_protected_legacy_orphan() {
        let golden: HygieneGolden =
            serde_json::from_str(include_str!("../testdata/nudge-hygiene-golden.json"))
                .expect("parse nudge hygiene golden");
        let case = golden
            .cases
            .iter()
            .find(|case| case.id == "unambiguous-legacy-orphan")
            .expect("legacy orphan fixture");
        let messages = case
            .messages
            .iter()
            .map(fixture_message)
            .collect::<Vec<_>>();
        let tags = case.tags.iter().map(fixture_tag).collect::<Vec<_>>();
        let projection = project_messages(&messages).unwrap();
        let core = CoreState::empty();
        let protected_blocks = HashSet::new();
        let mut memo = TailHygieneMemo::default();
        let slice = measure_tail_hygiene(
            &projection,
            &core,
            None,
            &tags,
            2,
            &protected_blocks,
            &mut memo,
        );
        let shared = tags.into_iter().map(Arc::new).collect::<Vec<_>>();
        let measured = measure_tail_hygiene(
            &projection,
            &core,
            None,
            shared.iter().map(Arc::as_ref),
            2,
            &protected_blocks,
            &mut memo,
        );
        assert_eq!(measured, slice);
        let orphan = &measured.parts[2];
        assert_eq!(orphan.tag_number, Some(2));
        assert!(orphan.protected);
        assert_eq!(orphan.u_tokens, 0);
        assert!(orphan.tokens > 0);
        assert!(measured.u > 0 && measured.u < measured.t);
    }

    #[test]
    fn provenance_guard_rejects_mutated_fixture_input() {
        let golden: HygieneGolden =
            serde_json::from_str(include_str!("../testdata/nudge-hygiene-golden.json"))
                .expect("parse nudge hygiene golden");
        let canonical = hygiene_fixture_canonical(&golden.cases);
        let mutated = canonical.replacen(
            "live-incident-mixed-tail",
            "mutated-live-incident-mixed-tail",
            1,
        );
        assert_ne!(
            format!("{:x}", Sha256::digest(mutated.as_bytes())),
            golden.provenance.input_sha256
        );
    }

    #[test]
    fn reasoning_mutant_changes_neither_term_but_tagged_text_mutant_reddens() {
        let mut memo = TailHygieneMemo::default();
        let tags = vec![tag(1, "visible#0")];
        let base = vec![
            text("visible", 1, "kept text"),
            message(
                "thinking",
                2,
                "assistant",
                vec![BlockKind::Reasoning {
                    text: "private".repeat(10_000),
                    signature: Some("signed".repeat(1_000)),
                }],
            ),
        ];
        let measured = measure_tail_hygiene(
            &project_messages(&base).unwrap(),
            &CoreState::empty(),
            None,
            &tags,
            0,
            &HashSet::new(),
            &mut memo,
        );
        let mut reasoning_mutant = base.clone();
        *Arc::make_mut(&mut reasoning_mutant[1]).ck.content_mut()[0].kind_mut() =
            BlockKind::Reasoning {
                text: "different private".repeat(20_000),
                signature: Some("different signature".repeat(2_000)),
            };
        let reasoning_measured = measure_tail_hygiene(
            &project_messages(&reasoning_mutant).unwrap(),
            &CoreState::empty(),
            None,
            &tags,
            0,
            &HashSet::new(),
            &mut memo,
        );
        assert_eq!(
            (reasoning_measured.u, reasoning_measured.t),
            (measured.u, measured.t)
        );

        let text_mutant = vec![text("visible", 1, "kept text plus loud mutation")];
        let text_measured = measure_tail_hygiene(
            &project_messages(&text_mutant).unwrap(),
            &CoreState::empty(),
            None,
            &tags,
            0,
            &HashSet::new(),
            &mut memo,
        );
        assert_ne!((text_measured.u, text_measured.t), (measured.u, measured.t));
    }

    #[test]
    fn consumed_protected_set_excludes_the_whole_exemplar_tool_arc_from_u() {
        let messages = vec![
            message(
                "owner",
                1,
                "assistant",
                vec![BlockKind::ToolCall {
                    id: "exemplar-call".to_string(),
                    name: "read".to_string(),
                    input: json!({"path":"fixture"}),
                    provider_executed: false,
                }],
            ),
            message(
                "result",
                2,
                "tool",
                vec![BlockKind::ToolResult {
                    id: "exemplar-call".to_string(),
                    tool_name: "read".to_string(),
                    output: ToolOutput::bare(OutputKind::Text {
                        text: "large exemplar output".repeat(5_000),
                    }),
                    provider_executed: false,
                }],
            ),
        ];
        let projection = project_messages(&messages).unwrap();
        let tags = vec![tag(1, "result#0")];
        let mut memo = TailHygieneMemo::default();
        let unprotected = measure_tail_hygiene(
            &projection,
            &CoreState::empty(),
            None,
            &tags,
            0,
            &HashSet::new(),
            &mut memo,
        );
        assert!(unprotected.u > 0);
        assert_eq!(unprotected.u, unprotected.t);
        let measured = measure_tail_hygiene(
            &projection,
            &CoreState::empty(),
            None,
            &tags,
            0,
            &HashSet::from(["owner#0".to_string()]),
            &mut memo,
        );
        assert_eq!(measured.u, 0);
        assert!(measured.t > 0);
        assert_eq!(measured.t, unprotected.t);
        let mut core = CoreState::empty();
        core.frozen_units.push(FrozenUnit {
            key: "red:result#0".into(),
            kind: "red".into(),
            frozen_payload: String::new(),
            durability_class: cache_stability::DurabilityClass::Lineage,
            reset_rule: String::new(),
        });
        let reduced = measure_tail_hygiene(
            &projection,
            &core,
            None,
            &tags,
            0,
            &HashSet::new(),
            &mut memo,
        );
        assert_eq!((reduced.u, reduced.t), (0, 0));
        for (part, block) in reduced.parts.iter().zip(&projection.blocks) {
            assert_eq!(
                part.content_hash,
                format!("{:x}", Sha256::digest(format!("excluded\0{}", block.bytes)))
            );
        }
    }

    #[test]
    fn recurring_raw_call_id_orphan_is_conservative_t_only() {
        let messages = vec![
            text("before", 1, "before"),
            message(
                "owner-a",
                2,
                "assistant",
                vec![BlockKind::ToolCall {
                    id: "repeat".to_string(),
                    name: "read".to_string(),
                    input: json!({"path":"a"}),
                    provider_executed: false,
                }],
            ),
            message(
                "result-a",
                3,
                "tool",
                vec![BlockKind::ToolResult {
                    id: "repeat".to_string(),
                    tool_name: "read".to_string(),
                    output: ToolOutput::bare(OutputKind::Text {
                        text: "first".to_string(),
                    }),
                    provider_executed: false,
                }],
            ),
            message(
                "owner-b",
                4,
                "assistant",
                vec![BlockKind::ToolCall {
                    id: "repeat".to_string(),
                    name: "read".to_string(),
                    input: json!({"path":"b"}),
                    provider_executed: false,
                }],
            ),
            message(
                "result-b",
                5,
                "tool",
                vec![BlockKind::ToolResult {
                    id: "repeat".to_string(),
                    tool_name: "read".to_string(),
                    output: ToolOutput::bare(OutputKind::Text {
                        text: "second".to_string(),
                    }),
                    provider_executed: false,
                }],
            ),
            text("after", 6, "after"),
        ];
        let projection = project_messages(&messages).unwrap();
        let tags = vec![tag(1, "before#0"), tag(2, "repeat"), tag(3, "after#0")];
        let measured = measure_tail_hygiene(
            &projection,
            &CoreState::empty(),
            None,
            &tags,
            0,
            &HashSet::new(),
            &mut TailHygieneMemo::default(),
        );
        let tagged_text_tokens = estimated_tokens("before") + estimated_tokens("after");
        assert_eq!(measured.u, tagged_text_tokens);
        assert!(measured.t > measured.u);
    }
}
