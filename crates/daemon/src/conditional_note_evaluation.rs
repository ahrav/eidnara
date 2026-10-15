//! Pure scheduling, lifecycle reduction, and selection policy for conditional-note checks.
//!
//! Timestamps and intervals use Unix epoch milliseconds. Cron matching uses the
//! supplied timezone's wall clock. Reducers clone input state and return updated
//! state without I/O.

use chrono::{Datelike, Offset, TimeZone, Timelike};
use serde::Deserialize;

/// Compiled-check policy version; other versions force recompilation.
pub const CONDITIONAL_NOTE_CHECK_POLICY_VERSION: i64 = 1;
pub const CONDITIONAL_NOTE_CHECK_FLOOR_MS: i64 = 5 * 60 * 1000;
pub const CONDITIONAL_NOTE_CHECK_CEILING_MS: i64 = 24 * 60 * 60 * 1000;
/// Delay used when a note has no usable cron schedule.
pub const CONDITIONAL_NOTE_CHECK_DEFAULT_INTERVAL_MS: i64 = 60 * 60 * 1000;
/// A check false for this long becomes eligible for a liveness recheck.
pub const CONDITIONAL_NOTE_CHECK_MAX_STALENESS_MS: i64 = 7 * 24 * 60 * 60 * 1000;
pub const CONDITIONAL_NOTE_CHECK_LIVENESS_RECHECK_MS: i64 = 24 * 60 * 60 * 1000;
pub const MAX_COMPILE_PER_RUN: usize = 5;
pub const MAX_FALLBACK_PER_RUN: usize = 3;
/// Liveness-phase fresh-claim quota per full selection cycle.
pub(crate) const MAX_LIVENESS_PER_RUN: usize = 3;
/// Due- and liveness-phase fresh-claim quotas per nonbillable selection cycle.
const NONBILLABLE_PHASE_QUOTA: usize = 10;
/// Consecutive compilation failures before a note enters fallback.
pub const MAX_COMPILATION_FAILURES: i64 = 3;
/// Consecutive check failures before a compiled note needs reauthoring.
pub const MAX_FAILURES_BEFORE_REAUTHOR: i64 = 3;
pub const DEFAULT_MAX_DUE_CHECKS: usize = 10;

const MINUTE_MS: i64 = 60_000;
/// Forward search bound for cron next-occurrence.
/// No occurrence within the bound returns `None`.
const MAX_SEARCH_MS: i64 = 4 * 366 * 24 * 60 * MINUTE_MS;

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

/// Parsed 5-field cron: per-field membership bitmasks plus the Vixie
/// dom/dow restriction flags.
struct ParsedCron {
    minute: u64,
    hour: u64,
    dom: u64,
    month: u64,
    dow: u64,
    dom_restricted: bool,
    dow_restricted: bool,
}

fn read_number(bytes: &[u8], at: &mut usize) -> Option<u64> {
    let start = *at;
    let mut value = 0u64;
    while let Some(&byte) = bytes.get(*at) {
        let digit = byte.wrapping_sub(b'0');
        if digit > 9 {
            break;
        }
        value = value.checked_mul(10)?.checked_add(u64::from(digit))?;
        *at += 1;
    }
    (*at > start).then_some(value)
}

/// The ASCII bytes `char::is_whitespace` accepts.
fn is_cron_space(byte: u8) -> bool {
    matches!(byte, b'\t' | b'\n' | 0x0B | 0x0C | b'\r' | b' ')
}

/// Reads one field from `bytes[*at..]` up to the end or an ASCII space and
/// moves `*at` past it.
fn parse_field(bytes: &[u8], at: &mut usize, min: u64, max: u64) -> Option<u64> {
    let mut mask = 0u64;
    loop {
        let (lo, mut hi, single) = if bytes.get(*at) == Some(&b'*') {
            *at += 1;
            (min, max, false)
        } else {
            let lo = read_number(bytes, at)?;
            if bytes.get(*at) == Some(&b'-') {
                *at += 1;
                (lo, read_number(bytes, at)?, false)
            } else {
                (lo, lo, true)
            }
        };
        let step = if bytes.get(*at) == Some(&b'/') {
            *at += 1;
            // `a/step` (no upper bound) means a..max by step.
            if single {
                hi = max;
            }
            read_number(bytes, at)?
        } else {
            1
        };
        if step == 0 || lo < min || hi > max || lo > hi {
            return None;
        }
        mask |= if step == 1 {
            (2u64 << hi) - (1u64 << lo)
        } else {
            let mut values = 0u64;
            let mut value = lo;
            while value <= hi {
                values |= 1u64 << value;
                // An overflowing step saturates above `hi`, ending the loop.
                value = value.saturating_add(step);
            }
            values
        };
        match bytes.get(*at) {
            Some(b',') => *at += 1,
            Some(&byte) if !is_cron_space(byte) => return None,
            _ => return Some(mask),
        }
    }
}

/// `parse_cron` accepts exactly five numeric fields: `minute hour dom month dow`.
/// `parse_cron` rejects empty or whitespace-only input.
fn parse_cron(expression: &str) -> Option<ParsedCron> {
    if expression.is_ascii() {
        parse_fields(expression.as_bytes())
    } else {
        // Non-ASCII whitespace separates fields too; rejoining the tokens
        // with ASCII spaces keeps the same fields.
        parse_fields(
            expression
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .as_bytes(),
        )
    }
}

/// Fields split on the ASCII bytes `char::is_whitespace` accepts, the only
/// separators in ASCII text.
fn parse_fields(bytes: &[u8]) -> Option<ParsedCron> {
    const BOUNDS: [(u64, u64); 5] = [(0, 59), (0, 23), (1, 31), (1, 12), (0, 7)];
    let mut at = 0;
    let mut masks = [0u64; 5];
    let mut starred = [false; 5];
    for (field, &(min, max)) in BOUNDS.iter().enumerate() {
        skip_spaces(bytes, &mut at);
        starred[field] = bytes.get(at) == Some(&b'*');
        // A lone `*`, the common field, is the whole range.
        let lone_star = starred[field] && bytes.get(at + 1).is_none_or(|&byte| is_cron_space(byte));
        masks[field] = if lone_star {
            at += 1;
            (2u64 << max) - (1u64 << min)
        } else {
            parse_field(bytes, &mut at, min, max)?
        };
    }
    skip_spaces(bytes, &mut at);
    let [minute, hour, dom, month, mut dow] = masks;
    // Day-of-week `7` is Sunday's alias, accepted by the range check above and
    // folded into `0` here.
    if dow & (1 << 7) != 0 {
        dow = dow & !(1 << 7) | 1;
    }
    (at == bytes.len()).then_some(ParsedCron {
        minute,
        hour,
        dom,
        month,
        dow,
        // Vixie cron sets DOM_STAR/DOW_STAR from a leading `*`, so `*/2` and `*,5`
        // are unrestricted like a bare `*`.
        dom_restricted: !starred[2],
        dow_restricted: !starred[4],
    })
}

fn skip_spaces(bytes: &[u8], at: &mut usize) {
    while bytes.get(*at).copied().is_some_and(is_cron_space) {
        *at += 1;
    }
}

fn mask_has(mask: u64, value: u32) -> bool {
    (mask >> value) & 1 == 1
}

/// Vixie OR-semantics: when BOTH dom and dow are restricted, either may match.
fn matches_day(cron: &ParsedCron, dom_value: u32, dow_value: u32) -> bool {
    let dom = mask_has(cron.dom, dom_value);
    let dow = mask_has(cron.dow, dow_value);
    match (cron.dom_restricted, cron.dow_restricted) {
        (true, true) => dom || dow,
        (true, false) => dom,
        (false, true) => dow,
        (false, false) => true,
    }
}

/// Epoch spacing of the offset probes a jump takes in a zone whose offsets
/// each last at least this long. Every zone in the IANA time zone database
/// meets it: its closest pair of transitions is days apart.
const OFFSET_PROBE_MS: i64 = 60 * MINUTE_MS;

/// What a cron search may assume about how a zone's UTC offset changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OffsetChanges {
    /// The offset never changes.
    Never,
    /// Each offset lasts at least [`OFFSET_PROBE_MS`] before the next change.
    HourApart,
    /// Changes may come arbitrarily close together, so the search visits
    /// every minute.
    Unbounded,
}

/// A time zone a cron search can evaluate, with what the search may assume
/// about its offset changes.
pub trait CronZone: TimeZone {
    fn offset_changes(&self) -> OffsetChanges;
}

impl CronZone for chrono::Utc {
    fn offset_changes(&self) -> OffsetChanges {
        OffsetChanges::Never
    }
}

impl CronZone for chrono::FixedOffset {
    fn offset_changes(&self) -> OffsetChanges {
        OffsetChanges::Never
    }
}

impl CronZone for chrono::Local {
    fn offset_changes(&self) -> OffsetChanges {
        local_offset_changes(std::env::var_os("TZ").as_deref())
    }
}

fn local_offset_changes(tz: Option<&std::ffi::OsStr>) -> OffsetChanges {
    match tz {
        Some(value) if value.as_encoded_bytes().contains(&b',') => OffsetChanges::Unbounded,
        _ => OffsetChanges::HourApart,
    }
}

/// `next_occurrence` returns the first instant strictly after `after_ms` whose local civil time in `tz` matches `cron`.
/// The search skips an epoch minute only when its civil time cannot match.
/// [`earliest_possible_match_ms`] bounds civil time by the zone's offsets,
/// [`minutes_until_candidate`] assumes the current offset, and
/// [`advance_while_offset_holds`] stops at the first changed offset in a zone
/// whose offsets last an hour. Other zones advance one minute at a time.
/// `next_occurrence` returns `None` when no match occurs by `after_ms + min(MAX_SEARCH_MS, max_search_ms)`.
fn next_occurrence<Tz: CronZone>(
    cron: &ParsedCron,
    after_ms: i64,
    max_search_ms: i64,
    tz: &Tz,
) -> Option<i64> {
    let mut cursor_ms = after_ms
        .div_euclid(MINUTE_MS)
        .checked_mul(MINUTE_MS)?
        .checked_add(MINUTE_MS)?;
    let cap_ms = after_ms.saturating_add(max_search_ms.clamp(0, MAX_SEARCH_MS));
    if !some_date_could_match(cron) {
        return None;
    }
    // The search asks the zone for its claim at its first civil time, so a
    // search the date bound settles never asks, and narrows the date bound to
    // the exact offset once a zone claims it never changes.
    let mut changes = None;
    let mut offsets_ms = -MAX_OFFSET_MS..=MAX_OFFSET_MS;
    // The date bound leaves the cursor in place for every cursor before
    // `bound_until_ms`. When every date could match, it never moves the cursor
    // and never runs out before the cap.
    let every_date = cron.month == ALL_MONTHS && !cron.dom_restricted && !cron.dow_restricted;
    let mut bound_until_ms = if every_date { i64::MAX } else { i64::MIN };
    let mut cursor_civil = None;
    while cursor_ms <= cap_ms {
        if cursor_ms >= bound_until_ms {
            let (possible_ms, until_ms) =
                earliest_possible_match_ms(cron, cursor_ms, cap_ms, &offsets_ms)?;
            if possible_ms > cursor_ms {
                cursor_ms = possible_ms;
                cursor_civil = None;
                continue;
            }
            bound_until_ms = until_ms;
        }
        // `timestamp_millis_opt` returns no civil time outside chrono's representable
        // date range; an unrepresentable `cursor_ms` makes the search return `None`.
        let civil = match cursor_civil.take() {
            Some(civil) => civil,
            None => tz.timestamp_millis_opt(cursor_ms).single()?,
        };
        let changes = *changes.get_or_insert_with(|| tz.offset_changes());
        let offset = civil.offset().fix();
        if changes == OffsetChanges::Unbounded {
            // Stepping minute by minute, most minutes fail on the time of
            // day, which the epoch minute and offset give without a date.
            let local_minute =
                (cursor_ms + i64::from(offset.local_minus_utc()) * 1000).div_euclid(MINUTE_MS);
            let hour = local_minute.div_euclid(60).rem_euclid(24) as u32;
            let minute = local_minute.rem_euclid(60) as u32;
            if !mask_has(cron.minute, minute) || !mask_has(cron.hour, hour) {
                cursor_ms = cursor_ms.checked_add(MINUTE_MS)?;
                continue;
            }
        }
        // A `DateTime` reads its fields from the local time chrono wraps past
        // the ends of its range, so only a representable local time is read
        // directly.
        let skip_minutes = match civil.naive_utc().checked_add_offset(offset) {
            Some(local) => minutes_until_candidate(cron, &local, changes),
            None => minutes_until_candidate(cron, &civil, changes),
        };
        if skip_minutes == 0 {
            return Some(cursor_ms);
        }
        cursor_ms = match changes {
            OffsetChanges::Never => {
                let offset_ms = i64::from(offset.local_minus_utc()) * 1000;
                if offsets_ms != (offset_ms..=offset_ms) {
                    offsets_ms = offset_ms..=offset_ms;
                    if !every_date {
                        bound_until_ms = i64::MIN;
                    }
                }
                cursor_ms.checked_add(skip_minutes * MINUTE_MS)?
            }
            OffsetChanges::HourApart => {
                // A skip stops where the date bound must be taken again. A
                // skip past the cap stops at the last minute at or before it:
                // once the offset holds through that minute, every minute
                // within the cap misses, and the search ends there.
                let bound_stop_ms = if bound_until_ms > cursor_ms {
                    ceil_to_minute(bound_until_ms)
                } else {
                    i64::MAX
                };
                let last_ms = cap_ms.div_euclid(MINUTE_MS) * MINUTE_MS;
                let target_ms = cursor_ms
                    .checked_add(skip_minutes * MINUTE_MS)?
                    .min(bound_stop_ms);
                if target_ms <= last_ms {
                    let (reached_ms, reached_civil) =
                        advance_while_offset_holds(tz, cursor_ms, offset, target_ms);
                    cursor_civil = reached_civil;
                    reached_ms
                } else if cursor_ms == last_ms {
                    return None;
                } else {
                    match advance_while_offset_holds(tz, cursor_ms, offset, last_ms) {
                        (_, Some(_)) => return None,
                        (changed_ms, None) => {
                            cursor_civil = None;
                            changed_ms
                        }
                    }
                }
            }
            OffsetChanges::Unbounded => cursor_ms.checked_add(skip_minutes * MINUTE_MS)?,
        };
    }
    None
}

fn ceil_to_minute(ms: i64) -> i64 {
    ms.saturating_add(MINUTE_MS - 1).div_euclid(MINUTE_MS) * MINUTE_MS
}

/// Bits 1 through 12 of [`ParsedCron::month`].
const ALL_MONTHS: u64 = 0x1FFE;
const MONTHS_OF_31_DAYS: u64 = 0b1_0101_1010_1010;
const MONTHS_OF_30_DAYS: u64 = 0b0_1010_0101_0000;

/// Whether any date in any year passes the date fields. A day-of-week list
/// matches some day of every month, so only a day-of-month list, with the day
/// of the week unrestricted, can rule out every date: each listed month is
/// shorter than every listed day, as in `0 0 30 2 *`.
fn some_date_could_match(cron: &ParsedCron) -> bool {
    if !cron.dom_restricted || cron.dow_restricted {
        return true;
    }
    let longest_month = if cron.month & MONTHS_OF_31_DAYS != 0 {
        31
    } else if cron.month & MONTHS_OF_30_DAYS != 0 {
        30
    } else {
        29
    };
    cron.dom & ((2u64 << longest_month) - 2) != 0
}

/// Whether a civil date could match `cron`: its month is listed and its day
/// passes the Vixie day-of-month and day-of-week rule.
fn date_could_match(cron: &ParsedCron, date: &impl Datelike) -> bool {
    mask_has(cron.month, date.month())
        && matches_day(cron, date.day(), date.weekday().num_days_from_sunday())
}

fn first_matching_date(
    cron: &ParsedCron,
    date: chrono::NaiveDate,
    last: chrono::NaiveDate,
) -> Option<chrono::NaiveDate> {
    let (mut year, mut month, mut from_day) = (date.year(), date.month(), date.day());
    loop {
        if mask_has(cron.month, month) {
            let first = chrono::NaiveDate::from_ymd_opt(year, month, 1)?;
            let days = matching_days(cron, &first) >> from_day << from_day;
            if days != 0 {
                let found = first.with_day(days.trailing_zeros())?;
                return (found <= last).then_some(found);
            }
        }
        let later = cron.month >> (month + 1);
        (year, month) = if later == 0 {
            (year.checked_add(1)?, cron.month.trailing_zeros())
        } else {
            (year, month + 1 + later.trailing_zeros())
        };
        from_day = 1;
        if (year, month) > (last.year(), last.month()) {
            return None;
        }
    }
}

fn matching_days(cron: &ParsedCron, first: &chrono::NaiveDate) -> u64 {
    let month_days = (2u64 << first.num_days_in_month()) - 2;
    let weekday_days = || {
        // Day `d` falls on weekday `(w + d - 1) % 7`, where `w` is the
        // weekday of the 1st; rotating the weekday bits by `w` and repeating
        // them every seven days gives that pattern from day 1.
        let weekdays = cron.dow & 0x7F;
        let w = first.weekday().num_days_from_sunday();
        let rotated = ((weekdays >> w) | (weekdays << (7 - w))) & 0x7F;
        (rotated * 0x1020_4081) << 1
    };
    let days = match (cron.dom_restricted, cron.dow_restricted) {
        (true, true) => cron.dom | weekday_days(),
        (true, false) => cron.dom,
        (false, true) => weekday_days(),
        (false, false) => month_days,
    };
    days & month_days
}

/// The largest offset chrono represents: a [`chrono::FixedOffset`] is whole
/// seconds strictly within a day of UTC.
const MAX_OFFSET_MS: i64 = 24 * 60 * MINUTE_MS - 1000;

/// The first epoch minute at or after `cursor_ms` whose civil time could match
/// `cron` when every offset, civil time minus epoch time, lies in `offsets_ms`,
/// paired with the instant from which the bound must be taken again.
/// Each minute from `cursor_ms` on has a civil time of at least
/// `earliest = cursor_ms + offsets_ms.start()`, so the first civil minute
/// that could match and still contains a time from `earliest` on bounds every
/// match; no earlier minute reaches it with an offset up to
/// `offsets_ms.end()`. That civil minute stays the bound for every cursor
/// whose `earliest` is still inside or before it. `None` means no epoch minute
/// up to `cap_ms` can match; at the ends of chrono's representable range the
/// result is `cursor_ms`, to be bounded again at the next cursor.
fn earliest_possible_match_ms(
    cron: &ParsedCron,
    cursor_ms: i64,
    cap_ms: i64,
    offsets_ms: &std::ops::RangeInclusive<i64>,
) -> Option<(i64, i64)> {
    let civil = |ms: Option<i64>| Some(chrono::DateTime::from_timestamp_millis(ms?)?.naive_utc());
    let unbounded = Some((cursor_ms, i64::MIN));
    let (Some(earliest), Some(latest)) = (
        civil(cursor_ms.checked_add(*offsets_ms.start())),
        civil(cap_ms.checked_add(*offsets_ms.end())),
    ) else {
        return unbounded;
    };
    let listed_today = date_could_match(cron, &earliest)
        .then(|| next_listed_time(cron, earliest.hour(), earliest.minute()))
        .flatten();
    let (date, (hour, minute)) = match listed_today {
        Some(time) => (earliest.date(), time),
        None => {
            // A date past `latest` is later than the civil time of every
            // epoch minute up to `cap_ms`.
            let tomorrow = earliest.date().succ_opt()?;
            let date = first_matching_date(cron, tomorrow, latest.date())?;
            (
                date,
                (cron.hour.trailing_zeros(), cron.minute.trailing_zeros()),
            )
        }
    };
    let Some(first) = date.and_hms_opt(hour, minute, 0) else {
        return unbounded;
    };
    let first_ms = first.and_utc().timestamp_millis();
    // A minute matches only when its civil time reaches `first`, so its epoch
    // time is at least `first_ms - offsets_ms.end()`, rounded up here to a
    // whole minute. `first` holds civil times before `first_ms + MINUTE_MS`.
    let possible_ms =
        (first_ms - offsets_ms.end() + MINUTE_MS - 1).div_euclid(MINUTE_MS) * MINUTE_MS;
    let until_ms = first_ms + MINUTE_MS - offsets_ms.start();
    Some((cursor_ms.max(possible_ms), until_ms))
}

/// The first listed time of day at or after `hour:minute`.
fn next_listed_time(cron: &ParsedCron, hour: u32, minute: u32) -> Option<(u32, u32)> {
    if mask_has(cron.hour, hour) {
        let minutes = cron.minute >> minute << minute;
        if minutes != 0 {
            return Some((hour, minutes.trailing_zeros()));
        }
    }
    let hours = cron.hour >> (hour + 1) << (hour + 1);
    (hours != 0).then(|| (hours.trailing_zeros(), cron.minute.trailing_zeros()))
}

/// Whole minutes from `local` to the next civil minute that could match
/// `cron` while the offset holds, or zero when `local` matches. A zone whose
/// offsets may change at any minute moves one minute. Otherwise the next
/// minute is later today, the first listed time of the next of the following
/// [`LOOKAHEAD_DAYS`] dates that could match, or the next civil midnight.
fn minutes_until_candidate(
    cron: &ParsedCron,
    local: &(impl Datelike + Timelike),
    changes: OffsetChanges,
) -> i64 {
    let (hour, minute) = (local.hour(), local.minute());
    let hour_listed = mask_has(cron.hour, hour);
    let date_listed = date_could_match(cron, local);
    if mask_has(cron.minute, minute) && hour_listed && date_listed {
        return 0;
    }
    if changes == OffsetChanges::Unbounded {
        return 1;
    }
    if date_listed {
        // Bits above 59 and 23 are never set, so an empty remainder means
        // no later minute this hour or no later hour today.
        let later_minutes = (cron.minute >> minute) >> 1;
        if hour_listed && later_minutes != 0 {
            return i64::from(later_minutes.trailing_zeros()) + 1;
        }
        let later_hours = (cron.hour >> hour) >> 1;
        if later_hours != 0 {
            let hours = later_hours.trailing_zeros() + 1;
            return i64::from(hours * 60 - minute + cron.minute.trailing_zeros());
        }
    }
    let to_midnight = i64::from(24 * 60 - (hour * 60 + minute));
    let first_listed = i64::from(cron.hour.trailing_zeros() * 60 + cron.minute.trailing_zeros());
    let today = local.num_days_from_ce();
    (1..=LOOKAHEAD_DAYS)
        .map_while(|days| {
            let date = chrono::NaiveDate::from_num_days_from_ce_opt(today.checked_add(days)?)?;
            Some((days, date))
        })
        .find(|(_, date)| date_could_match(cron, date))
        .map_or(to_midnight, |(days, _)| {
            to_midnight + i64::from(days - 1) * 24 * 60 + first_listed
        })
}

/// Days past today a single skip may cross. A search that the date bound
/// leaves in place is within about two days of the first civil time that
/// could match, so two days let one skip, and its hourly offset probes, run
/// from there to that time.
const LOOKAHEAD_DAYS: i32 = 2;

/// The first epoch minute in `(from_ms, target_ms]` whose offset differs from
/// `offset` or that has no civil time, or `target_ms` when none does, with the
/// civil time at `target_ms` in the second case. Probes
/// run every [`OFFSET_PROBE_MS`] and at `target_ms`; because each offset lasts
/// at least that long, two probes that report `offset` hold it between them.
fn advance_while_offset_holds<Tz: TimeZone>(
    tz: &Tz,
    from_ms: i64,
    offset: chrono::FixedOffset,
    target_ms: i64,
) -> (i64, Option<chrono::DateTime<Tz>>) {
    let civil_holding = |ms: i64| {
        tz.timestamp_millis_opt(ms)
            .single()
            .filter(|civil| civil.offset().fix() == offset)
    };
    let mut held_ms = from_ms;
    loop {
        let probe_ms = held_ms.saturating_add(OFFSET_PROBE_MS).min(target_ms);
        match civil_holding(probe_ms) {
            Some(civil) if probe_ms == target_ms => return (target_ms, Some(civil)),
            Some(_) => held_ms = probe_ms,
            None => {
                let changed_ms =
                    first_change_after(held_ms, probe_ms, |ms| civil_holding(ms).is_some());
                return (changed_ms, None);
            }
        }
    }
}

/// Bisects `(held_ms, changed_ms]`, both epoch minutes, to the first minute
/// where `holds` is false; `holds(held_ms)` is true and `holds(changed_ms)` false.
fn first_change_after(mut held_ms: i64, mut changed_ms: i64, holds: impl Fn(i64) -> bool) -> i64 {
    while changed_ms - held_ms > MINUTE_MS {
        let middle_minutes = (changed_ms - held_ms) / MINUTE_MS / 2;
        let middle_ms = held_ms + middle_minutes * MINUTE_MS;
        if holds(middle_ms) {
            held_ms = middle_ms;
        } else {
            changed_ms = middle_ms;
        }
    }
    changed_ms
}

/// `next_due_at_ms` returns `None` when parsing fails or no occurrence is found within the search cap.
fn next_due_at_ms<Tz: CronZone>(
    expression: &str,
    after_ms: i64,
    max_search_ms: i64,
    tz: &Tz,
) -> Option<i64> {
    let cron = parse_cron(expression)?;
    next_occurrence(&cron, after_ms, max_search_ms, tz)
}

/// Returns whether `expression` follows the supported five-field numeric cron grammar.
///
/// Names and macros are unsupported. Sunday accepts both `0` and `7`.
pub fn is_valid_conditional_note_cron(expression: &str) -> bool {
    parse_cron(expression).is_some()
}

/// The first occurrence of `expression` strictly after `after_ms`, searched up
/// to the parser's own cap; `None` for an invalid expression or one with no
/// occurrence in that window. Unlike the conditional-note due time this applies no
/// clamp or jitter, so a scheduler sees the cron instant itself.
pub fn next_cron_occurrence<Tz: CronZone>(expression: &str, after_ms: i64, tz: &Tz) -> Option<i64> {
    next_due_at_ms(expression, after_ms, MAX_SEARCH_MS, tz).filter(|&ms| ms != 0)
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

/// Computes the next check time in Unix epoch milliseconds.
///
/// Invalid, empty, or matchless cron expressions use the one-hour default interval.
/// The interval is clamped to five minutes through 24 hours, then receives stable
/// note/hash jitter of at most 10 percent or 60 seconds, whichever is smaller.
pub fn next_conditional_note_check_due_at<Tz: CronZone>(
    cron: Option<&str>,
    now: i64,
    note_id: i64,
    hash: Option<&str>,
    tz: &Tz,
) -> i64 {
    let raw_next = cron
        .filter(|c| !c.trim().is_empty())
        .and_then(|c| next_due_at_ms(c, now, CONDITIONAL_NOTE_CHECK_CEILING_MS, tz))
        // A zero epoch denotes no occurrence.
        .filter(|&ms| ms != 0);
    let raw_delta = match raw_next {
        Some(next) => next - now,
        None => CONDITIONAL_NOTE_CHECK_DEFAULT_INTERVAL_MS,
    };
    let clamped = raw_delta.clamp(
        CONDITIONAL_NOTE_CHECK_FLOOR_MS,
        CONDITIONAL_NOTE_CHECK_CEILING_MS,
    );
    let jittered = clamped + deterministic_jitter_ms(clamped, note_id, hash);
    let bounded = jittered.clamp(
        CONDITIONAL_NOTE_CHECK_FLOOR_MS,
        CONDITIONAL_NOTE_CHECK_CEILING_MS,
    );
    // A `now` within one interval of `i64::MAX` has no representable next due
    // time; saturating keeps the note in the future rather than wrapping it
    // negative and making it appear overdue.
    now.saturating_add(bounded)
}

fn deterministic_jitter_ms(interval_ms: i64, note_id: i64, hash: Option<&str>) -> i64 {
    let max = 60_000i64.min((interval_ms as f64 * 0.1).floor() as i64);
    if max <= 0 {
        return 0;
    }
    // The jitter hash applies FNV-1a to the UTF-16 units of
    // `"{note_id}:{hash}"`; the decimal id and the colon are ASCII, so their
    // units equal their bytes.
    let mut digits = [0u8; 20];
    let mut start = digits.len();
    let mut rest = note_id.unsigned_abs();
    loop {
        start -= 1;
        digits[start] = b'0' + (rest % 10) as u8;
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    let sign = if note_id < 0 { &b"-"[..] } else { &[] };
    let ascii = sign.iter().chain(&digits[start..]).chain(b":");
    let units = ascii
        .map(|&byte| u16::from(byte))
        .chain(hash.unwrap_or("").encode_utf16());
    let h = units.fold(2166136261u32, |h, unit| {
        (h ^ u32::from(unit)).wrapping_mul(16777619)
    });
    i64::from(h) % (max * 2 + 1) - max
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ConditionalNoteLifecycleState {
    pub status: String,
    pub ready_at: Option<i64>,
    pub ready_reason: Option<String>,
    pub last_checked_at: Option<i64>,
    pub updated_at: i64,
    pub compiled_check: Option<String>,
    pub manifest_json: Option<String>,
    pub check_hash: Option<String>,
    pub check_cron: Option<String>,
    pub check_version: i64,
    pub check_status: String,
    pub check_failure_count: i64,
    pub check_network_failure_count: i64,
    pub check_quarantined_until: Option<i64>,
    pub check_next_due_at: Option<i64>,
    pub check_compiled_at: Option<i64>,
    pub check_false_since_at: Option<i64>,
    pub check_last_liveness_at: Option<i64>,
    pub policy_version: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledCheckArtifact {
    pub compiled_check: String,
    pub manifest_json: String,
    pub check_hash: String,
    pub check_cron: String,
}

/// Compile-phase result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompileOutcome {
    CompiledMet(CompiledCheckArtifact),
    CompiledFalse(CompiledCheckArtifact),
    CompilationFailed,
}

/// Due/liveness-phase result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckOutcome {
    Met,
    False,
    LogicFailed,
    NetworkFailed,
}

/// Fallback-phase result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackOutcome {
    Met,
    False,
}

/// Phase-specific outcome types prevent cross-phase results from type-checking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConditionalNoteEvaluationOutcome {
    Compile(CompileOutcome),
    Due(CheckOutcome),
    Liveness(CheckOutcome),
    Fallback(FallbackOutcome),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConditionalNoteReduction {
    pub next: ConditionalNoteLifecycleState,
    pub surfaced: bool,
}

/// Returns exponential failure backoff in milliseconds.
///
/// The schedule is `min(24 hours, 5 * 2^(N-1) minutes)`. Counts below one behave as
/// one; counts above ten remain capped at 24 hours.
pub fn evaluation_backoff_ms(failure_count: i64) -> i64 {
    // 5 * 2^9 already exceeds the 24h cap, so larger exponents can saturate.
    let exponent = failure_count.clamp(1, 10) - 1;
    let minutes = (24 * 60).min(5 << exponent);
    minutes * 60 * 1000
}

/// The ready reason uses the signal, then its summary, then a fixed default, and truncates to 240 UTF-16 code units.
///
/// The bound is 240 UTF-16 code units because TypeScript `String.prototype.slice` uses UTF-16 units; scalar counting would persist a different `ready_reason` for non-BMP input.
pub fn due_ready_reason(note_id: i64, manifest_json: Option<&str>) -> String {
    let signal = manifest_signal_or_summary(manifest_json)
        .unwrap_or_else(|| "compiled check returned met=true".to_string());
    truncate_utf16_units(&format!("Conditional note #{note_id}: {signal}"), 240)
}

/// A split surrogate pair is dropped because Rust `String` cannot hold an unpaired surrogate.
fn truncate_utf16_units(value: &str, max_units: usize) -> String {
    if value.chars().map(char::len_utf16).sum::<usize>() <= max_units {
        return value.to_string();
    }
    let mut used = 0usize;
    let mut out = String::new();
    for ch in value.chars() {
        let width = ch.len_utf16();
        if used + width > max_units {
            break;
        }
        used += width;
        out.push(ch);
    }
    out
}

fn manifest_signal_or_summary(manifest_json: Option<&str>) -> Option<String> {
    let json = manifest_json.filter(|j| !j.is_empty())?;
    let parsed: serde_json::Value = serde_json::from_str(json).ok()?;
    let signals: Option<Vec<&str>> = parsed.get("signals").and_then(|v| v.as_array()).map(|arr| {
        arr.iter()
            .filter_map(serde_json::Value::as_str)
            .collect::<Vec<_>>()
    });
    if let Some(first) = signals.and_then(|s| s.first().map(ToString::to_string)) {
        return Some(first);
    }
    parsed
        .get("summary")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

fn ready_fields(
    mut state: ConditionalNoteLifecycleState,
    reason: String,
    now: i64,
) -> ConditionalNoteLifecycleState {
    state.status = "ready".to_string();
    state.ready_at = Some(now);
    state.ready_reason = Some(reason);
    state.last_checked_at = Some(now);
    state.updated_at = now;
    state
}

fn false_fields<Tz: CronZone>(
    mut state: ConditionalNoteLifecycleState,
    note_id: i64,
    now: i64,
    cron: Option<&str>,
    hash: Option<&str>,
    tz: &Tz,
) -> ConditionalNoteLifecycleState {
    state.last_checked_at = Some(now);
    state.updated_at = now;
    state.check_next_due_at = Some(next_conditional_note_check_due_at(
        cron, now, note_id, hash, tz,
    ));
    state.check_failure_count = 0;
    state.check_network_failure_count = 0;
    state.check_false_since_at = state.check_false_since_at.or(Some(now));
    state
}

fn reduce_compile<Tz: CronZone>(
    pre: &ConditionalNoteLifecycleState,
    outcome: &CompileOutcome,
    note_id: i64,
    now: i64,
    tz: &Tz,
) -> ConditionalNoteReduction {
    let artifact = match outcome {
        CompileOutcome::CompilationFailed => {
            // A `failing` note's count is the check-phase tally that triggered reauthoring,
            // so compilation attempts start from zero.
            let prior_failures = if pre.check_status == "failing" {
                0
            } else {
                pre.check_failure_count
            };
            let failure_count = prior_failures.saturating_add(1);
            let mut next = pre.clone();
            next.check_failure_count = failure_count;
            next.check_status = if failure_count >= MAX_COMPILATION_FAILURES {
                "fallback".to_string()
            } else {
                "uncompiled".to_string()
            };
            next.check_next_due_at = Some(now.saturating_add(evaluation_backoff_ms(failure_count)));
            next.updated_at = now;
            return ConditionalNoteReduction {
                next,
                surfaced: false,
            };
        }
        CompileOutcome::CompiledMet(artifact) | CompileOutcome::CompiledFalse(artifact) => artifact,
    };
    let next_due_at = next_conditional_note_check_due_at(
        Some(&artifact.check_cron),
        now,
        note_id,
        Some(&artifact.check_hash),
        tz,
    );
    let mut stored = pre.clone();
    stored.compiled_check = Some(artifact.compiled_check.clone());
    stored.manifest_json = Some(artifact.manifest_json.clone());
    stored.check_hash = Some(artifact.check_hash.clone());
    stored.check_cron = Some(artifact.check_cron.clone());
    stored.check_version = 1;
    stored.check_status = "compiled".to_string();
    stored.check_failure_count = 0;
    stored.check_network_failure_count = 0;
    stored.check_quarantined_until = None;
    stored.check_next_due_at = Some(next_due_at);
    stored.check_compiled_at = Some(now);
    stored.check_false_since_at = pre.check_false_since_at.or(Some(now));
    stored.check_last_liveness_at = None;
    stored.policy_version = CONDITIONAL_NOTE_CHECK_POLICY_VERSION;
    stored.updated_at = now;
    if matches!(outcome, CompileOutcome::CompiledMet(_)) {
        return ConditionalNoteReduction {
            next: ready_fields(
                stored,
                format!("Conditional note #{note_id}: compiled check returned met=true"),
                now,
            ),
            surfaced: true,
        };
    }
    ConditionalNoteReduction {
        next: false_fields(
            stored,
            note_id,
            now,
            Some(&artifact.check_cron),
            Some(&artifact.check_hash),
            tz,
        ),
        surfaced: false,
    }
}

fn reduce_check_failure(
    pre: &ConditionalNoteLifecycleState,
    outcome: CheckOutcome,
    now: i64,
) -> ConditionalNoteLifecycleState {
    let mut next = pre.clone();
    if outcome == CheckOutcome::LogicFailed {
        let failure_count = pre.check_failure_count.saturating_add(1);
        next.check_failure_count = failure_count;
        next.check_status = if failure_count >= MAX_FAILURES_BEFORE_REAUTHOR {
            "failing".to_string()
        } else {
            "compiled".to_string()
        };
        next.check_next_due_at = Some(now.saturating_add(evaluation_backoff_ms(failure_count)));
        next.updated_at = now;
        return next;
    }
    let network_count = pre.check_network_failure_count.saturating_add(1);
    let quarantined_until = now.saturating_add(evaluation_backoff_ms(network_count));
    next.check_network_failure_count = network_count;
    next.check_status = if network_count >= MAX_FAILURES_BEFORE_REAUTHOR {
        "failing".to_string()
    } else {
        "compiled".to_string()
    };
    next.check_next_due_at = Some(quarantined_until);
    next.check_quarantined_until = Some(quarantined_until);
    next.updated_at = now;
    next
}

fn reduce_due<Tz: CronZone>(
    pre: &ConditionalNoteLifecycleState,
    outcome: CheckOutcome,
    note_id: i64,
    now: i64,
    tz: &Tz,
) -> ConditionalNoteReduction {
    match outcome {
        CheckOutcome::Met => ConditionalNoteReduction {
            next: ready_fields(
                pre.clone(),
                due_ready_reason(note_id, pre.manifest_json.as_deref()),
                now,
            ),
            surfaced: true,
        },
        CheckOutcome::False => ConditionalNoteReduction {
            next: false_fields(
                pre.clone(),
                note_id,
                now,
                pre.check_cron.as_deref(),
                pre.check_hash.as_deref(),
                tz,
            ),
            surfaced: false,
        },
        CheckOutcome::LogicFailed | CheckOutcome::NetworkFailed => ConditionalNoteReduction {
            next: reduce_check_failure(pre, outcome, now),
            surfaced: false,
        },
    }
}

fn reduce_liveness<Tz: CronZone>(
    pre: &ConditionalNoteLifecycleState,
    outcome: CheckOutcome,
    note_id: i64,
    now: i64,
    tz: &Tz,
) -> ConditionalNoteReduction {
    let mut attempted = pre.clone();
    attempted.check_last_liveness_at = Some(now);
    attempted.updated_at = now;
    match outcome {
        CheckOutcome::Met => ConditionalNoteReduction {
            next: ready_fields(
                attempted,
                format!(
                    "Conditional note #{note_id}: max-staleness liveness check returned met=true"
                ),
                now,
            ),
            surfaced: true,
        },
        CheckOutcome::False => ConditionalNoteReduction {
            next: false_fields(
                attempted,
                note_id,
                now,
                pre.check_cron.as_deref(),
                pre.check_hash.as_deref(),
                tz,
            ),
            surfaced: false,
        },
        // Liveness runs a previously healthy compiled check; a logic error during liveness means the check itself broke, so reauthoring is immediate.
        // The compile selector requires `check_next_due_at <= now`, so a due time still ahead is pulled back to `now`.
        CheckOutcome::LogicFailed => {
            attempted.check_status = "failing".to_string();
            attempted.check_next_due_at =
                Some(attempted.check_next_due_at.map_or(now, |due| due.min(now)));
            ConditionalNoteReduction {
                next: attempted,
                surfaced: false,
            }
        }
        CheckOutcome::NetworkFailed => ConditionalNoteReduction {
            next: attempted,
            surfaced: false,
        },
    }
}

fn reduce_fallback(
    pre: &ConditionalNoteLifecycleState,
    outcome: FallbackOutcome,
    note_id: i64,
    now: i64,
) -> ConditionalNoteReduction {
    match outcome {
        FallbackOutcome::Met => ConditionalNoteReduction {
            next: ready_fields(
                pre.clone(),
                format!(
                    "Conditional note #{note_id}: read-only confirmation evaluator returned met=true"
                ),
                now,
            ),
            surfaced: true,
        },
        FallbackOutcome::False => {
            let mut next = pre.clone();
            next.last_checked_at = Some(now);
            next.updated_at = now;
            next.check_status = "fallback".to_string();
            ConditionalNoteReduction {
                next,
                surfaced: false,
            }
        }
    }
}

/// Reduces a lifecycle snapshot without mutating it.
///
/// `now` is Unix epoch milliseconds. `surfaced` is true only when this call marks the
/// note ready.
pub fn reduce_conditional_note_evaluation<Tz: CronZone>(
    pre: &ConditionalNoteLifecycleState,
    outcome: &ConditionalNoteEvaluationOutcome,
    note_id: i64,
    now: i64,
    tz: &Tz,
) -> ConditionalNoteReduction {
    match outcome {
        ConditionalNoteEvaluationOutcome::Compile(o) => reduce_compile(pre, o, note_id, now, tz),
        ConditionalNoteEvaluationOutcome::Due(o) => reduce_due(pre, *o, note_id, now, tz),
        ConditionalNoteEvaluationOutcome::Liveness(o) => reduce_liveness(pre, *o, note_id, now, tz),
        ConditionalNoteEvaluationOutcome::Fallback(o) => reduce_fallback(pre, *o, note_id, now),
    }
}

// ---------------------------------------------------------------------------
// note snapshots)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConditionalNoteSelectionSnapshot {
    pub id: i64,
    pub status: String,
    pub created_at: i64,
    /// Only artifact presence affects selection, so the snapshot avoids copying artifact bodies for every pending note on every acquisition poll.
    pub has_compiled_check: bool,
    pub last_checked_at: Option<i64>,
    pub check_status: String,
    pub check_quarantined_until: Option<i64>,
    pub check_next_due_at: Option<i64>,
    pub check_false_since_at: Option<i64>,
    pub check_last_liveness_at: Option<i64>,
    pub policy_version: i64,
}

fn eligible(note: &ConditionalNoteSelectionSnapshot) -> bool {
    note.status == "pending"
}

/// Selects due compiled checks by due time, then note ID.
///
/// `now` and stored times use Unix epoch milliseconds. A zero `limit` still returns
/// at most one note.
pub fn get_due_compiled_conditional_note_checks(
    notes: &[ConditionalNoteSelectionSnapshot],
    now: i64,
    limit: usize,
) -> Vec<&ConditionalNoteSelectionSnapshot> {
    let mut selected: Vec<&ConditionalNoteSelectionSnapshot> = notes
        .iter()
        .filter(|note| {
            eligible(note)
                && note.check_status == "compiled"
                && note.has_compiled_check
                && note.policy_version == CONDITIONAL_NOTE_CHECK_POLICY_VERSION
                && note.check_quarantined_until.is_none_or(|q| q <= now)
                && note.check_next_due_at.is_none_or(|d| d <= now)
        })
        .collect();
    selected.sort_by_key(|note| (note.check_next_due_at.unwrap_or(0), note.id));
    selected.truncate(limit.max(1));
    selected
}

/// Selects compilation candidates by creation time, then note ID.
///
/// A zero `limit` still returns at most one note.
pub fn get_conditional_notes_needing_compilation(
    notes: &[ConditionalNoteSelectionSnapshot],
    now: i64,
    limit: usize,
) -> Vec<&ConditionalNoteSelectionSnapshot> {
    let mut selected: Vec<&ConditionalNoteSelectionSnapshot> = notes
        .iter()
        .filter(|note| {
            eligible(note)
                && note.check_next_due_at.is_none_or(|d| d <= now)
                && (note.check_status == "uncompiled"
                    || note.check_status == "failing"
                    || !note.has_compiled_check
                    || note.policy_version != CONDITIONAL_NOTE_CHECK_POLICY_VERSION)
        })
        .collect();
    selected.sort_by_key(|note| (note.created_at, note.id));
    selected.truncate(limit.max(1));
    selected
}

/// Selects liveness candidates by false-since time, then note ID.
///
/// `now` and stored times use Unix epoch milliseconds.
/// Both staleness comparisons are inclusive, so a note exactly at the maximum false duration or the liveness recheck interval is already a candidate.
/// A network-quarantined note is ineligible before `check_quarantined_until`.
/// A zero `limit` still returns at most one note.
pub fn get_stale_compiled_conditional_notes(
    notes: &[ConditionalNoteSelectionSnapshot],
    now: i64,
    limit: usize,
) -> Vec<&ConditionalNoteSelectionSnapshot> {
    let stale_before = now.saturating_sub(CONDITIONAL_NOTE_CHECK_MAX_STALENESS_MS);
    let liveness_before = now.saturating_sub(CONDITIONAL_NOTE_CHECK_LIVENESS_RECHECK_MS);
    let mut selected: Vec<&ConditionalNoteSelectionSnapshot> = notes
        .iter()
        .filter(|note| {
            eligible(note)
                && note.check_status == "compiled"
                && note.has_compiled_check
                && note.policy_version == CONDITIONAL_NOTE_CHECK_POLICY_VERSION
                && note.check_quarantined_until.is_none_or(|q| q <= now)
                && note.check_false_since_at.is_some_and(|f| f <= stale_before)
                && note
                    .check_last_liveness_at
                    .is_none_or(|l| l <= liveness_before)
        })
        .collect();
    selected.sort_by_key(|note| (note.check_false_since_at.unwrap_or(0), note.id));
    selected.truncate(limit.max(1));
    selected
}

/// Selects fallback candidates with never-checked notes first, then check time and ID.
///
/// A zero `limit` still returns at most one note.
pub fn get_fallback_conditional_notes(
    notes: &[ConditionalNoteSelectionSnapshot],
    limit: usize,
) -> Vec<&ConditionalNoteSelectionSnapshot> {
    let mut selected: Vec<&ConditionalNoteSelectionSnapshot> = notes
        .iter()
        .filter(|note| eligible(note) && note.check_status == "fallback")
        .collect();
    selected.sort_by_key(|note| {
        (
            note.last_checked_at.is_some(),
            note.last_checked_at.unwrap_or(0),
            note.id,
        )
    });
    selected.truncate(limit.max(1));
    selected
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConditionalNoteCycleMode {
    Full,
    /// Nonbillable mode permits only sandbox due and liveness claims.
    Nonbillable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConditionalNotePhase {
    Due,
    Compile,
    Liveness,
    Fallback,
}

impl ConditionalNotePhase {
    fn name(self) -> &'static str {
        match self {
            ConditionalNotePhase::Due => "due",
            ConditionalNotePhase::Compile => "compile",
            ConditionalNotePhase::Liveness => "liveness",
            ConditionalNotePhase::Fallback => "fallback",
        }
    }
}

const FULL_CYCLE_PROFILE: &[(ConditionalNotePhase, usize)] = &[
    (ConditionalNotePhase::Due, DEFAULT_MAX_DUE_CHECKS),
    (ConditionalNotePhase::Compile, MAX_COMPILE_PER_RUN),
    (ConditionalNotePhase::Liveness, MAX_LIVENESS_PER_RUN),
    (ConditionalNotePhase::Fallback, MAX_FALLBACK_PER_RUN),
];
const NONBILLABLE_CYCLE_PROFILE: &[(ConditionalNotePhase, usize)] = &[
    (ConditionalNotePhase::Due, NONBILLABLE_PHASE_QUOTA),
    (ConditionalNotePhase::Liveness, NONBILLABLE_PHASE_QUOTA),
];

/// Durable cursor and remaining quota for one phase-ordered selection cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConditionalNoteSelectionCycle {
    mode: ConditionalNoteCycleMode,
    /// Earlier phases are skipped until the next cycle; newly eligible work waits.
    phase_index: usize,
    remaining: usize,
    /// Excluding attempted fallback IDs lets later fallback notes receive claims before retries.
    attempted_fallback: Vec<i64>,
}

impl ConditionalNoteSelectionCycle {
    pub(crate) fn new(mode: ConditionalNoteCycleMode) -> Self {
        Self {
            mode,
            phase_index: 0,
            remaining: cycle_profile(mode)[0].1,
            attempted_fallback: Vec::new(),
        }
    }
}

fn cycle_profile(mode: ConditionalNoteCycleMode) -> &'static [(ConditionalNotePhase, usize)] {
    match mode {
        ConditionalNoteCycleMode::Full => FULL_CYCLE_PROFILE,
        ConditionalNoteCycleMode::Nonbillable => NONBILLABLE_CYCLE_PROFILE,
    }
}

/// Selects one note from the earliest nonempty phase with quota remaining.
///
/// Phase order is due, compile, liveness, fallback in full mode and due, liveness in
/// nonbillable mode. Empty or exhausted phases advance without transferring quota.
/// Returned cursor state applies only after the caller commits the claim.
pub(crate) fn select_conditional_note_evaluation_cycle(
    notes: &[ConditionalNoteSelectionSnapshot],
    now: i64,

    cycle: &ConditionalNoteSelectionCycle,
) -> Option<(i64, String, ConditionalNoteSelectionCycle)> {
    let profile = cycle_profile(cycle.mode);
    for (index, &(phase, quota)) in profile.iter().enumerate().skip(cycle.phase_index) {
        let remaining = if index == cycle.phase_index {
            cycle.remaining
        } else {
            quota
        };
        if remaining == 0 {
            continue;
        }
        let selected = match phase {
            ConditionalNotePhase::Due => get_due_compiled_conditional_note_checks(notes, now, 1)
                .first()
                .map(|note| note.id),
            ConditionalNotePhase::Compile => {
                get_conditional_notes_needing_compilation(notes, now, 1)
                    .first()
                    .map(|note| note.id)
            }
            ConditionalNotePhase::Liveness => get_stale_compiled_conditional_notes(notes, now, 1)
                .first()
                .map(|note| note.id),
            ConditionalNotePhase::Fallback => get_fallback_conditional_notes(notes, notes.len())
                .into_iter()
                .find(|note| !cycle.attempted_fallback.contains(&note.id))
                .map(|note| note.id),
        };
        let Some(note_id) = selected else { continue };
        let mut next = cycle.clone();
        next.phase_index = index;
        next.remaining = remaining - 1;
        if phase == ConditionalNotePhase::Fallback {
            next.attempted_fallback.push(note_id);
        }
        return Some((note_id, phase.name().to_string(), next));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::Tz as ChronoTz;

    /// A `now` within one interval of `i64::MAX` saturates instead of wrapping
    /// negative, which would make the note read as already overdue.
    #[test]
    fn a_due_time_near_the_end_of_representable_time_saturates() {
        let now = i64::MAX - 1_000;
        let due = next_conditional_note_check_due_at(None, now, 7, None, &chrono::Utc);
        assert_eq!(due, i64::MAX);
        let with_cron =
            next_conditional_note_check_due_at(Some("* * * * *"), now, 7, None, &chrono::Utc);
        assert!(with_cron >= now, "{with_cron} < {now}");

        // The staleness selector saturates at the other end of time.
        let selection = ConditionalNoteSelectionSnapshot {
            id: 7,
            status: "pending".to_string(),

            created_at: i64::MIN,
            has_compiled_check: true,
            last_checked_at: Some(i64::MIN),
            check_status: "compiled".to_string(),
            check_quarantined_until: None,
            check_next_due_at: None,
            check_false_since_at: Some(i64::MIN),
            check_last_liveness_at: None,
            policy_version: CONDITIONAL_NOTE_CHECK_POLICY_VERSION,
        };
        let _ = get_stale_compiled_conditional_notes(
            std::slice::from_ref(&selection),
            i64::MIN + 1,
            10,
        );
        // Backoff follows its contract for every count below one.
        assert_eq!(evaluation_backoff_ms(i64::MIN), evaluation_backoff_ms(1));
        assert_eq!(evaluation_backoff_ms(0), evaluation_backoff_ms(1));
        assert_eq!(evaluation_backoff_ms(i64::MAX), evaluation_backoff_ms(10));

        // Failure backoffs saturate the same way, for every failure kind, and
        // so do the failure counters they advance.
        let mut pre = compiled_state(now);
        pre.check_failure_count = i64::MAX;
        pre.check_network_failure_count = i64::MAX;
        for outcome in [CheckOutcome::LogicFailed, CheckOutcome::NetworkFailed] {
            let next = reduce_check_failure(&pre, outcome, now);
            assert!(next.check_next_due_at.is_none_or(|due| due >= now));
            assert!(
                next.check_quarantined_until
                    .is_none_or(|until| until >= now)
            );
            assert!(next.check_failure_count >= pre.check_failure_count);
            assert!(next.check_network_failure_count >= pre.check_network_failure_count);
        }
    }
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct Golden {
        provenance: Provenance,
        constants: Constants,
        transition_cases: Vec<TransitionCase>,
        schedule_cases: Vec<ScheduleCase>,
        selection_cases: Vec<SelectionCase>,
    }

    #[derive(Deserialize)]
    struct Provenance {
        timezone: String,
    }

    #[derive(Deserialize)]
    struct Constants {
        policy_version: i64,
        floor_ms: i64,
        ceiling_ms: i64,
        default_interval_ms: i64,
        max_staleness_ms: i64,
        liveness_recheck_ms: i64,
        max_compile_per_run: usize,
        max_fallback_per_run: usize,
        max_liveness_per_run: usize,
        max_compilation_failures: i64,
        default_max_checks: usize,
        max_failures_before_reauthor: i64,
        backoff_minutes: Vec<i64>,
    }

    #[derive(Deserialize)]
    struct TransitionCase {
        id: String,
        phase: String,
        outcome: OutcomeJson,
        note_id: i64,
        now: i64,
        pre: ConditionalNoteLifecycleState,
        expected: ConditionalNoteLifecycleState,
    }

    #[derive(Deserialize)]
    struct OutcomeJson {
        kind: String,
        compiled_check: Option<String>,
        manifest_json: Option<String>,
        check_hash: Option<String>,
        check_cron: Option<String>,
    }

    #[derive(Deserialize)]
    struct ScheduleCase {
        id: String,
        cron: Option<String>,
        now_ms: i64,
        note_id: i64,
        hash: Option<String>,
        expected_due_at: i64,
    }

    #[derive(Deserialize)]
    struct SelectionCase {
        id: String,
        phase: String,
        now: i64,
        limit: usize,

        notes: Vec<SelectionNoteJson>,
        expected_ids: Vec<i64>,
    }

    #[derive(Deserialize)]
    struct SelectionNoteJson {
        id: i64,
        status: Option<String>,
        created_at: Option<i64>,
        compiled_check: Option<String>,
        last_checked_at: Option<i64>,
        check_status: Option<String>,
        check_quarantined_until: Option<i64>,
        check_next_due_at: Option<i64>,
        check_false_since_at: Option<i64>,
        check_last_liveness_at: Option<i64>,
        policy_version: Option<i64>,
    }

    fn snapshot_with_defaults(
        note: &SelectionNoteJson,
        now: i64,
    ) -> ConditionalNoteSelectionSnapshot {
        ConditionalNoteSelectionSnapshot {
            id: note.id,
            status: note.status.clone().unwrap_or_else(|| "pending".to_string()),

            created_at: note.created_at.unwrap_or(now - 1_000_000),
            has_compiled_check: note.compiled_check.is_some(),
            last_checked_at: note.last_checked_at,
            check_status: note
                .check_status
                .clone()
                .unwrap_or_else(|| "uncompiled".to_string()),
            check_quarantined_until: note.check_quarantined_until,
            check_next_due_at: note.check_next_due_at,
            check_false_since_at: note.check_false_since_at,
            check_last_liveness_at: note.check_last_liveness_at,
            policy_version: note.policy_version.unwrap_or(1),
        }
    }

    fn build_outcome(case: &TransitionCase) -> ConditionalNoteEvaluationOutcome {
        let artifact = || CompiledCheckArtifact {
            compiled_check: case.outcome.compiled_check.clone().expect("compiled_check"),
            manifest_json: case.outcome.manifest_json.clone().expect("manifest_json"),
            check_hash: case.outcome.check_hash.clone().expect("check_hash"),
            check_cron: case.outcome.check_cron.clone().expect("check_cron"),
        };
        let check = |kind: &str| match kind {
            "met" => CheckOutcome::Met,
            "false" => CheckOutcome::False,
            "logic_failed" => CheckOutcome::LogicFailed,
            "network_failed" => CheckOutcome::NetworkFailed,
            other => panic!("unknown check outcome kind {other:?}"),
        };
        match (case.phase.as_str(), case.outcome.kind.as_str()) {
            ("compile", "compiled_met") => {
                ConditionalNoteEvaluationOutcome::Compile(CompileOutcome::CompiledMet(artifact()))
            }
            ("compile", "compiled_false") => {
                ConditionalNoteEvaluationOutcome::Compile(CompileOutcome::CompiledFalse(artifact()))
            }
            ("compile", "compilation_failed") => {
                ConditionalNoteEvaluationOutcome::Compile(CompileOutcome::CompilationFailed)
            }
            ("due", kind) => ConditionalNoteEvaluationOutcome::Due(check(kind)),
            ("liveness", kind) => ConditionalNoteEvaluationOutcome::Liveness(check(kind)),
            ("fallback", "met") => ConditionalNoteEvaluationOutcome::Fallback(FallbackOutcome::Met),
            ("fallback", "false") => {
                ConditionalNoteEvaluationOutcome::Fallback(FallbackOutcome::False)
            }
            (phase, kind) => panic!("unknown outcome {phase}/{kind}"),
        }
    }

    #[test]
    fn conditional_note_evaluation_golden_matches_production_behaviour() {
        let raw = include_str!("../testdata/conditional-note-evaluation-golden.json");
        let golden: Golden = serde_json::from_str(raw).expect("parse fixture");
        assert!(
            !golden.transition_cases.is_empty(),
            "conditional-note golden fixture must not be empty"
        );
        let tz: ChronoTz = golden
            .provenance
            .timezone
            .parse()
            .expect("parse fixture timezone");

        let c = &golden.constants;
        assert_eq!(CONDITIONAL_NOTE_CHECK_POLICY_VERSION, c.policy_version);
        assert_eq!(CONDITIONAL_NOTE_CHECK_FLOOR_MS, c.floor_ms);
        assert_eq!(CONDITIONAL_NOTE_CHECK_CEILING_MS, c.ceiling_ms);
        assert_eq!(
            CONDITIONAL_NOTE_CHECK_DEFAULT_INTERVAL_MS,
            c.default_interval_ms
        );
        assert_eq!(CONDITIONAL_NOTE_CHECK_MAX_STALENESS_MS, c.max_staleness_ms);
        assert_eq!(
            CONDITIONAL_NOTE_CHECK_LIVENESS_RECHECK_MS,
            c.liveness_recheck_ms
        );
        assert_eq!(MAX_COMPILE_PER_RUN, c.max_compile_per_run);
        assert_eq!(MAX_FALLBACK_PER_RUN, c.max_fallback_per_run);
        assert_eq!(MAX_LIVENESS_PER_RUN, c.max_liveness_per_run);
        assert_eq!(MAX_COMPILATION_FAILURES, c.max_compilation_failures);
        assert_eq!(DEFAULT_MAX_DUE_CHECKS, c.default_max_checks);
        assert_eq!(MAX_FAILURES_BEFORE_REAUTHOR, c.max_failures_before_reauthor);
        for (i, minutes) in c.backoff_minutes.iter().enumerate() {
            let failure_count = i as i64 + 1;
            assert_eq!(
                evaluation_backoff_ms(failure_count),
                minutes * 60 * 1000,
                "backoff for failure {failure_count}"
            );
        }

        for case in &golden.transition_cases {
            let outcome = build_outcome(case);
            let reduction = reduce_conditional_note_evaluation(
                &case.pre,
                &outcome,
                case.note_id,
                case.now,
                &tz,
            );
            assert_eq!(reduction.next, case.expected, "transition case {}", case.id);
            assert_eq!(
                reduction.surfaced,
                case.expected.status == "ready",
                "surfaced for transition case {}",
                case.id
            );
        }

        for case in &golden.schedule_cases {
            let due_at = next_conditional_note_check_due_at(
                case.cron.as_deref(),
                case.now_ms,
                case.note_id,
                case.hash.as_deref(),
                &tz,
            );
            assert_eq!(due_at, case.expected_due_at, "schedule case {}", case.id);
        }

        for case in &golden.selection_cases {
            let notes: Vec<ConditionalNoteSelectionSnapshot> = case
                .notes
                .iter()
                .map(|n| snapshot_with_defaults(n, case.now))
                .collect();
            let selected = match case.phase.as_str() {
                "due" => get_due_compiled_conditional_note_checks(&notes, case.now, case.limit),
                "compile" => {
                    get_conditional_notes_needing_compilation(&notes, case.now, case.limit)
                }
                "liveness" => get_stale_compiled_conditional_notes(&notes, case.now, case.limit),
                "fallback" => get_fallback_conditional_notes(&notes, case.limit),
                other => panic!("unknown selection phase {other:?}"),
            };
            let ids: Vec<i64> = selected.iter().map(|n| n.id).collect();
            assert_eq!(ids, case.expected_ids, "selection case {}", case.id);
        }
    }

    #[test]
    fn conditional_note_revision_matrix_normative_matches_memory_store() {
        use memory_store::{
            MemoryStore, NoteCasOutcome, NoteEvalAcquireOutcome, NoteEvalClaim,
            NoteEvalRenewOutcome, NoteTransitionInput, NoteWriteInput, StoredNote,
        };
        use storage::{Isolation, StorageBackend, StorageDescriptor};

        #[derive(Deserialize)]
        struct Normative {
            revision_matrix_cases: Vec<RevisionCase>,
        }

        #[derive(Deserialize)]
        struct RevisionCase {
            id: String,
            event: String,
            pre: Option<RevisionPre>,
            expected: RevisionExpected,
        }

        #[derive(Deserialize, Default)]
        struct RevisionPre {
            #[serde(default)]
            source_revision: i64,
            #[serde(default)]
            state_version: i64,
            status_version: i64,
        }

        #[derive(Deserialize)]
        struct RevisionExpected {
            source_revision: i64,
            state_version: i64,
            status_version: i64,
            artifact_cleared: Option<bool>,
            active_claims_fenced: Option<bool>,
        }

        const PROJECT: &str = "git:rev-matrix";

        fn open_store(dir: &std::path::Path) -> MemoryStore {
            let store = MemoryStore::open(&StorageDescriptor {
                module_id: "eidnara-test".to_string(),
                storage_namespace: "memory".to_string(),
                isolation: Isolation::Module,
                backend: StorageBackend::Sqlite {
                    path: dir.join("store.db").to_string_lossy().to_string(),
                },
            })
            .unwrap();
            let preparing = store
                .authority_begin_prepare("ctx", PROJECT, "notes")
                .unwrap();
            store
                .authority_finish_prepare(
                    "ctx",
                    PROJECT,
                    "notes",
                    preparing.generation,
                    "hash",
                    "hash",
                    true,
                )
                .unwrap();
            store
        }

        fn insert_note(store: &MemoryStore) -> StoredNote {
            store
                .insert_project_note(NoteWriteInput {
                    project_path: PROJECT,
                    route_project_root: None,
                    session_id: Some("writer"),
                    content: "base content",
                    surface_condition: Some("base condition"),
                    anchor_block_id: None,
                    anchor_ordinal: None,
                    compiled_provider: None,
                    compiled_config: None,
                    compiled_at: None,
                    compile_status: None,
                    now_ms: 1,
                })
                .unwrap()
        }

        fn stage(store: &MemoryStore, mut note: StoredNote, src: i64, state: i64) -> StoredNote {
            for i in 0..src {
                note = match store
                    .update_note_cas(
                        PROJECT,
                        note.id,
                        "pending",
                        note.status_version,
                        Some(&format!("staged content {i}")),
                        None,
                        None,
                        2,
                    )
                    .unwrap()
                {
                    NoteCasOutcome::Applied(next) => next,
                    other => panic!("staging content edit failed: {other:?}"),
                };
            }
            while note.state_version < state {
                note = match store
                    .transition_note(NoteTransitionInput {
                        project_path: PROJECT,
                        note_id: note.id,
                        from_status: "pending",
                        source_revision: note.status_version,
                        to_status: "pending",
                        result: None,
                        now_ms: 3,
                    })
                    .unwrap()
                {
                    NoteCasOutcome::Applied(next) => next,
                    other => panic!("staging transition failed: {other:?}"),
                };
            }
            assert_eq!(
                (
                    note.source_revision,
                    note.state_version,
                    note.status_version
                ),
                (src, state, state),
                "pre-state staging"
            );
            note
        }

        fn stage_artifact(store: &MemoryStore, note: &StoredNote) {
            store
                .execute_tag_sql_for_test(&format!(
                    "UPDATE notes SET compiled_check = 'check-code', manifest_json = '{{}}',
                        check_hash = 'hash', check_status = 'compiled', check_version = 1,
                        compiled_source_revision = {}, compiled_project_path = '{PROJECT}'
                      WHERE id = {}",
                    note.source_revision, note.id
                ))
                .unwrap();
        }

        fn stage_claim(store: &MemoryStore, note_id: i64) -> NoteEvalClaim {
            match store
                .acquire_note_evaluation(
                    PROJECT,
                    "acq-1",
                    "eval-a",
                    0,
                    1,
                    |notes| {
                        notes.iter().find(|note| note.id == note_id).map_or(
                            memory_store::NoteEvalSelection::NoWork {
                                cycle_exhausted: false,
                            },
                            |note| memory_store::NoteEvalSelection::Claim {
                                note_id: note.id,
                                phase: "due".to_string(),
                            },
                        )
                    },
                    10,
                )
                .unwrap()
            {
                NoteEvalAcquireOutcome::Claim { claim, .. } => claim,
                other => panic!("claim staging failed: {other:?}"),
            }
        }

        let raw = include_str!("../testdata/conditional-note-evaluation-normative.json");
        let normative: Normative = serde_json::from_str(raw).expect("parse normative fixture");
        assert!(
            !normative.revision_matrix_cases.is_empty(),
            "conditional-note normative fixture must not be empty"
        );

        for case in &normative.revision_matrix_cases {
            let dir = tempfile::tempdir().unwrap();
            let store = open_store(dir.path());
            let note = insert_note(&store);
            let pre = case.pre.as_ref();
            let mut claim = None;

            let post = match case.event.as_str() {
                "migrate" => {
                    let pre = pre.expect("migrate pre");
                    store
                        .execute_tag_sql_for_test(&format!(
                            "UPDATE notes SET status_version = {}, source_revision = 0,
                                state_version = 0 WHERE id = {};
                             UPDATE notes SET source_revision = status_version,
                                state_version = status_version;",
                            pre.status_version, note.id
                        ))
                        .unwrap();
                    store
                        .get_note_by_id(PROJECT, "writer", note.id)
                        .unwrap()
                        .unwrap()
                }
                "create" => note,
                "edit_compiler_input" => {
                    if case.id == "edit_project" {
                        continue;
                    }
                    let pre = pre.expect("edit pre");
                    let staged = stage(&store, note, pre.source_revision, pre.state_version);
                    stage_artifact(&store, &staged);
                    claim = Some(stage_claim(&store, staged.id));
                    let (content, condition) = if case.id.contains("condition") {
                        (None, Some(Some("edited condition")))
                    } else {
                        (Some("edited body"), None)
                    };
                    match store
                        .update_note_cas(
                            PROJECT,
                            staged.id,
                            "pending",
                            staged.status_version,
                            content,
                            condition,
                            None,
                            20,
                        )
                        .unwrap()
                    {
                        NoteCasOutcome::Applied(next) => next,
                        other => panic!("case {}: edit failed: {other:?}", case.id),
                    }
                }
                "lifecycle_transition" => {
                    let pre = pre.expect("transition pre");
                    let staged = stage(&store, note, pre.source_revision, pre.state_version);
                    stage_artifact(&store, &staged);
                    claim = Some(stage_claim(&store, staged.id));
                    match store
                        .transition_note(NoteTransitionInput {
                            project_path: PROJECT,
                            note_id: staged.id,
                            from_status: "pending",
                            source_revision: staged.status_version,
                            to_status: "ready",
                            result: Some("condition_true"),
                            now_ms: 20,
                        })
                        .unwrap()
                    {
                        NoteCasOutcome::Applied(next) => next,
                        other => panic!("case {}: transition failed: {other:?}", case.id),
                    }
                }
                "dismiss" => {
                    let pre = pre.expect("dismiss pre");
                    let staged = stage(&store, note, pre.source_revision, pre.state_version);
                    stage_artifact(&store, &staged);
                    claim = Some(stage_claim(&store, staged.id));
                    store
                        .dismiss_note(PROJECT, "writer", staged.id, Some("done"), 20)
                        .unwrap()
                        .expect("dismissal applies")
                }
                "authority_transfer" => {
                    let pre = pre.expect("transfer pre");
                    let staged = stage(&store, note, pre.source_revision, pre.state_version);
                    store
                        .authority_begin_drain("ctx", PROJECT, "notes", "lease", 1_000_000, 20)
                        .unwrap();
                    store
                        .get_note_by_id(PROJECT, "writer", staged.id)
                        .unwrap()
                        .unwrap()
                }
                other => panic!("unknown revision matrix event {other:?}"),
            };

            let expected = &case.expected;
            assert_eq!(
                (
                    post.source_revision,
                    post.state_version,
                    post.status_version
                ),
                (
                    expected.source_revision,
                    expected.state_version,
                    expected.status_version
                ),
                "revisions for case {}",
                case.id
            );
            if let Some(cleared) = expected.artifact_cleared {
                assert_eq!(
                    post.compiled_check.is_none(),
                    cleared,
                    "artifact for case {}",
                    case.id
                );
            }
            if let Some(should_fence) = expected.active_claims_fenced {
                let claim = claim.expect("fence-pinned case stages a claim");
                let outcome = store
                    .renew_note_evaluation_claim(PROJECT, &claim.claim_id, "eval-a", 0, 1, 30)
                    .unwrap();
                if should_fence {
                    assert!(
                        matches!(outcome, NoteEvalRenewOutcome::TerminalReplay { .. }),
                        "claim fencing for case {}",
                        case.id
                    );
                } else {
                    assert!(
                        matches!(outcome, NoteEvalRenewOutcome::Renewed { .. }),
                        "claim must stay live for case {}",
                        case.id
                    );
                }
            }
            assert_eq!(
                post.state_version, post.status_version,
                "state/status invariant for case {}",
                case.id
            );
        }
    }

    /// The TypeScript reducer truncates `ready_reason` with `String.slice`, which counts UTF-16 code units.
    #[test]
    fn due_ready_reason_truncates_on_utf16_units_like_the_ts_reducer() {
        // A non-BMP scalar value uses two UTF-16 code units.
        let signal = "\u{1F600}".repeat(200);
        let manifest = format!("{{\"signals\":[\"{signal}\"]}}");
        let reason = due_ready_reason(1, Some(manifest.as_str()));

        let units: usize = reason.chars().map(char::len_utf16).sum();
        assert!(
            units <= 240,
            "expected at most 240 UTF-16 units, got {units}"
        );
        assert!(
            reason.chars().count() < 240,
            "scalar-counted truncation would exceed the JS bound"
        );
        assert!(reason.is_char_boundary(reason.len()));
    }

    const DAY_MS: i64 = 24 * 60 * MINUTE_MS;

    /// Every IANA zone keeps each offset for days, as `OFFSET_PROBE_MS` requires.
    impl CronZone for ChronoTz {
        fn offset_changes(&self) -> OffsetChanges {
            OffsetChanges::HourApart
        }
    }

    /// A zone in the shape a POSIX `TZ` rule allows: `shift_seconds` ahead of
    /// UTC from `start_minute` to `end_minute` (UTC minutes of the day, the
    /// window wrapping past midnight when `end_minute < start_minute`) every
    /// day. `changes` is what the zone claims to the search.
    #[derive(Debug, Clone, Copy)]
    struct DailyDaylight {
        start_minute: u32,
        end_minute: u32,
        shift_seconds: i32,
        changes: OffsetChanges,
    }

    impl DailyDaylight {
        fn offset_at(&self, utc: &chrono::NaiveDateTime) -> chrono::FixedOffset {
            let minute = utc.hour() * 60 + utc.minute();
            let inside = if self.start_minute <= self.end_minute {
                (self.start_minute..self.end_minute).contains(&minute)
            } else {
                minute >= self.start_minute || minute < self.end_minute
            };
            chrono::FixedOffset::east_opt(if inside { self.shift_seconds } else { 0 }).unwrap()
        }
    }

    impl TimeZone for DailyDaylight {
        type Offset = chrono::FixedOffset;

        fn from_offset(_offset: &chrono::FixedOffset) -> Self {
            unreachable!("the search never rebuilds a zone from an offset")
        }

        fn offset_from_local_date(
            &self,
            _local: &chrono::NaiveDate,
        ) -> chrono::LocalResult<chrono::FixedOffset> {
            chrono::LocalResult::None
        }

        fn offset_from_local_datetime(
            &self,
            local: &chrono::NaiveDateTime,
        ) -> chrono::LocalResult<chrono::FixedOffset> {
            let standard = chrono::FixedOffset::east_opt(0).unwrap();
            let daylight = chrono::FixedOffset::east_opt(self.shift_seconds).unwrap();
            let fits = |offset: chrono::FixedOffset| self.offset_at(&(*local - offset)) == offset;
            match (fits(standard), fits(daylight)) {
                (true, true) => chrono::LocalResult::Ambiguous(daylight, standard),
                (true, false) => chrono::LocalResult::Single(standard),
                (false, true) => chrono::LocalResult::Single(daylight),
                (false, false) => chrono::LocalResult::None,
            }
        }

        fn offset_from_utc_date(&self, _utc: &chrono::NaiveDate) -> chrono::FixedOffset {
            chrono::FixedOffset::east_opt(0).unwrap()
        }

        fn offset_from_utc_datetime(&self, utc: &chrono::NaiveDateTime) -> chrono::FixedOffset {
            self.offset_at(utc)
        }
    }

    impl CronZone for DailyDaylight {
        fn offset_changes(&self) -> OffsetChanges {
            self.changes
        }
    }

    /// The minute stepper as it read before the field-jumping search replaced
    /// it, kept verbatim as the differential oracle.
    fn frozen_minute_stepper<Tz: TimeZone>(
        cron: &ParsedCron,
        after_ms: i64,
        max_search_ms: i64,
        tz: &Tz,
    ) -> Option<i64> {
        let mut cursor_ms = after_ms
            .div_euclid(MINUTE_MS)
            .checked_mul(MINUTE_MS)?
            .checked_add(MINUTE_MS)?;
        let cap_ms = after_ms.saturating_add(max_search_ms.clamp(0, MAX_SEARCH_MS));
        while cursor_ms <= cap_ms {
            let civil = tz.timestamp_millis_opt(cursor_ms).single()?;
            if mask_has(cron.minute, civil.minute())
                && mask_has(cron.hour, civil.hour())
                && mask_has(cron.month, civil.month())
                && matches_day(cron, civil.day(), civil.weekday().num_days_from_sunday())
            {
                return Some(cursor_ms);
            }
            cursor_ms = cursor_ms.checked_add(MINUTE_MS)?;
        }
        None
    }

    /// Zones whose transitions stress the jump: DST in both hemispheres, a
    /// 30-minute DST shift, a quarter-hour offset, a skipped calendar day,
    /// transitions at midnight, a seconds-bearing historical offset, negative
    /// DST, a two-hour DST shift, and DST suspended for Ramadan.
    const DIFFERENTIAL_ZONES: &[ChronoTz] = &[
        ChronoTz::UTC,
        ChronoTz::America__New_York,
        ChronoTz::Europe__London,
        ChronoTz::Australia__Lord_Howe,
        ChronoTz::Asia__Kathmandu,
        ChronoTz::Pacific__Apia,
        ChronoTz::America__Sao_Paulo,
        ChronoTz::America__Havana,
        ChronoTz::Europe__Amsterdam,
        ChronoTz::Europe__Dublin,
        ChronoTz::Antarctica__Troll,
        ChronoTz::Africa__Casablanca,
    ];

    fn assert_matches_stepper<Tz: CronZone>(
        expression: &str,
        after_ms: i64,
        max_search_ms: i64,
        tz: &Tz,
    ) -> Option<i64> {
        let cron = parse_cron(expression).unwrap();
        let expected = frozen_minute_stepper(&cron, after_ms, max_search_ms, tz);
        assert_eq!(
            next_occurrence(&cron, after_ms, max_search_ms, tz),
            expected,
            "{expression} after {after_ms} within {max_search_ms}"
        );
        expected
    }

    fn utc_ms(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> i64 {
        chrono::Utc
            .with_ymd_and_hms(year, month, day, hour, minute, 0)
            .unwrap()
            .timestamp_millis()
    }

    #[test]
    fn a_spring_forward_gap_skips_the_day_and_a_fall_back_overlap_fires_the_earlier_instant() {
        let new_york = ChronoTz::America__New_York;
        // 2026-03-08 02:30 does not exist in New York; the next 02:30 is a day later.
        let spring = assert_matches_stepper(
            "30 2 * * *",
            utc_ms(2026, 3, 8, 5, 0),
            DAY_MS * 3,
            &new_york,
        );
        assert_eq!(spring, Some(utc_ms(2026, 3, 9, 6, 30)));
        // 2026-11-01 01:30 happens twice; the earlier instant is EDT, 05:30 UTC.
        let fall =
            assert_matches_stepper("30 1 * * *", utc_ms(2026, 11, 1, 4, 0), DAY_MS, &new_york);
        assert_eq!(fall, Some(utc_ms(2026, 11, 1, 5, 30)));
        // After the earlier instant, the repeated 01:30 EST matches too.
        let repeat =
            assert_matches_stepper("30 1 * * *", utc_ms(2026, 11, 1, 5, 30), DAY_MS, &new_york);
        assert_eq!(repeat, Some(utc_ms(2026, 11, 1, 6, 30)));
        // Lord Howe moves by 30 minutes and Kathmandu sits at +05:45.
        for zone in [ChronoTz::Australia__Lord_Howe, ChronoTz::Asia__Kathmandu] {
            for start in [utc_ms(2026, 4, 4, 0, 0), utc_ms(2026, 10, 3, 0, 0)] {
                for expression in ["*/7 2 * * *", "45 1,2 * * *", "0 0 * * *"] {
                    assert_matches_stepper(expression, start, 3 * DAY_MS, &zone);
                }
            }
        }
        // Apia skipped 2011-12-30 entirely.
        let apia = assert_matches_stepper(
            "0 12 30 12 *",
            utc_ms(2011, 12, 29, 0, 0),
            400 * DAY_MS,
            &ChronoTz::Pacific__Apia,
        );
        assert!(apia.is_some_and(|ms| ms > utc_ms(2012, 12, 1, 0, 0)));
    }

    #[test]
    fn a_zone_without_an_hour_between_changes_is_searched_minute_by_minute() {
        // Daylight runs 02:10 to 02:40 UTC, so 02:15 UTC is 03:15. An hourly
        // probe at 02:00 and 03:00 sees standard time at both ends and would
        // jump past it, so the zone must not claim the hour bound.
        let after_ms = utc_ms(2026, 3, 1, 0, 0);
        let zone = |changes| DailyDaylight {
            start_minute: 130,
            end_minute: 160,
            shift_seconds: 3600,
            changes,
        };
        let found = assert_matches_stepper(
            "15 3 * * *",
            after_ms,
            DAY_MS,
            &zone(OffsetChanges::Unbounded),
        );
        assert_eq!(found, Some(utc_ms(2026, 3, 1, 2, 15)));
        let cron = parse_cron("15 3 * * *").unwrap();
        assert_ne!(
            next_occurrence(&cron, after_ms, DAY_MS, &zone(OffsetChanges::HourApart)),
            found
        );
    }

    #[test]
    fn a_minute_by_minute_search_matches_the_stepper_on_both_sides_of_the_epoch() {
        // A seconds-bearing daylight shift, with instants before 1970 where
        // epoch minutes are negative.
        let zone = DailyDaylight {
            start_minute: 130,
            end_minute: 160,
            shift_seconds: 3600 + 37,
            changes: OffsetChanges::Unbounded,
        };
        for after_ms in [
            utc_ms(1969, 12, 30, 22, 0),
            utc_ms(1900, 3, 1, 0, 7),
            utc_ms(2026, 3, 1, 23, 59),
        ] {
            for expression in ["15 3 * * *", "*/7 2,3 * * *", "0 0 * * 1", "59 23 31 * *"] {
                assert_matches_stepper(expression, after_ms, 3 * DAY_MS, &zone);
            }
        }
    }

    #[test]
    fn a_local_tz_rule_with_transition_dates_is_searched_minute_by_minute() {
        use std::ffi::OsStr;
        for rule in ["STD0DST,J60/1:10,J60/2:40", "EST5EDT,M3.2.0,M11.1.0"] {
            assert_eq!(
                local_offset_changes(Some(OsStr::new(rule))),
                OffsetChanges::Unbounded,
                "{rule}"
            );
        }
        for zone in [
            "",
            "UTC",
            "UTC0",
            "America/New_York",
            ":Europe/London",
            "/etc/localtime",
        ] {
            assert_eq!(
                local_offset_changes(Some(OsStr::new(zone))),
                OffsetChanges::HourApart,
                "{zone}"
            );
        }
        assert_eq!(local_offset_changes(None), OffsetChanges::HourApart);
    }

    #[test]
    fn a_jump_across_a_shift_resumes_at_the_first_shifted_minute() {
        // Daylight starts at 23:50 UTC, moving civil time from 23:50 to 00:20
        // of the next day, and lasts four hours. From 23:01 the hour fails
        // until midnight, so the search jumps toward 00:00 civil; the shift
        // lands civil time at 00:20 first, and 00:25 is the next match.
        let zone = DailyDaylight {
            start_minute: 1430,
            end_minute: 230,
            shift_seconds: 1800,
            changes: OffsetChanges::HourApart,
        };
        let found = assert_matches_stepper("25 0 * * *", utc_ms(2026, 3, 1, 23, 0), DAY_MS, &zone);
        assert_eq!(found, Some(utc_ms(2026, 3, 1, 23, 55)));
        let lord_howe = ChronoTz::Australia__Lord_Howe;
        for start_hour in 10..16 {
            let after_ms = utc_ms(2026, 10, 3, start_hour, 7);
            assert_matches_stepper("*/7 2 4 10 *", after_ms, 2 * DAY_MS, &lord_howe);
            assert_matches_stepper("31,37 2 * * *", after_ms, 2 * DAY_MS, &lord_howe);
        }
    }

    #[test]
    fn a_date_jump_holds_at_the_largest_offsets_chrono_represents() {
        // Offsets one second short of a day put civil time almost a day from
        // epoch time. Closing a window with `shift_seconds = largest` moves
        // civil time back 86,399 seconds, so a later minute can fall on an
        // earlier date; closing one with `-largest` moves it forward as far.
        let largest = 24 * 60 * 60 - 1;
        let windows = [
            (0, 24 * 60, OffsetChanges::Unbounded),
            (600, 900, OffsetChanges::HourApart),
        ];
        let mut zones = Vec::new();
        for (start_minute, end_minute, changes) in windows {
            for shift_seconds in [largest, -largest] {
                zones.push(DailyDaylight {
                    start_minute,
                    end_minute,
                    shift_seconds,
                    changes,
                });
            }
        }
        let fixed = [
            chrono::FixedOffset::east_opt(largest).unwrap(),
            chrono::FixedOffset::west_opt(largest).unwrap(),
            chrono::FixedOffset::east_opt(5 * 3600 + 45 * 60 + 7).unwrap(),
        ];
        let expressions = [
            "0 0 1 * *",
            "59 23 31 * *",
            "30 12 1,15 * *",
            "0 0 * * 0",
            "15 10 * 2 *",
        ];
        for day in [1, 28, 31] {
            for hour in [0, 13] {
                let after_ms = utc_ms(2026, 1, day, hour, 7);
                for expression in expressions {
                    for zone in &zones {
                        assert_matches_stepper(expression, after_ms, 35 * DAY_MS, zone);
                    }
                    for zone in &fixed {
                        assert_matches_stepper(expression, after_ms, 35 * DAY_MS, zone);
                    }
                }
            }
        }
    }

    #[test]
    fn calendar_impossible_expressions_return_none_at_the_search_cap() {
        let start = utc_ms(2026, 1, 1, 0, 0);
        for zone in [ChronoTz::UTC, ChronoTz::America__New_York] {
            for expression in ["0 0 30 2 *", "0 0 31 4,6,9,11 *"] {
                assert_eq!(
                    assert_matches_stepper(expression, start, MAX_SEARCH_MS, &zone),
                    None
                );
            }
        }
        // February 29 is satisfiable within four years.
        assert!(assert_matches_stepper("0 0 29 2 *", start, MAX_SEARCH_MS, &chrono::Utc).is_some());
        // A cap that ends one minute before the occurrence returns none.
        let next = utc_ms(2026, 1, 1, 0, 5);
        assert_eq!(
            assert_matches_stepper("5 0 * * *", start, next - start - MINUTE_MS, &chrono::Utc),
            None
        );
        assert_eq!(
            assert_matches_stepper("5 0 * * *", start, next - start, &chrono::Utc),
            Some(next)
        );
    }

    /// A zone that records the instant of every offset lookup the search makes.
    #[derive(Clone)]
    struct Recording {
        inner: DailyDaylight,
        seen: std::cell::RefCell<Vec<i64>>,
    }

    impl TimeZone for Recording {
        type Offset = chrono::FixedOffset;

        fn from_offset(_offset: &chrono::FixedOffset) -> Self {
            unreachable!("the search never rebuilds a zone from an offset")
        }

        fn offset_from_local_date(
            &self,
            local: &chrono::NaiveDate,
        ) -> chrono::LocalResult<chrono::FixedOffset> {
            self.inner.offset_from_local_date(local)
        }

        fn offset_from_local_datetime(
            &self,
            local: &chrono::NaiveDateTime,
        ) -> chrono::LocalResult<chrono::FixedOffset> {
            self.inner.offset_from_local_datetime(local)
        }

        fn offset_from_utc_date(&self, utc: &chrono::NaiveDate) -> chrono::FixedOffset {
            self.inner.offset_from_utc_date(utc)
        }

        fn offset_from_utc_datetime(&self, utc: &chrono::NaiveDateTime) -> chrono::FixedOffset {
            self.seen
                .borrow_mut()
                .push(utc.and_utc().timestamp_millis());
            self.inner.offset_from_utc_datetime(utc)
        }
    }

    impl CronZone for Recording {
        fn offset_changes(&self) -> OffsetChanges {
            self.inner.changes
        }
    }

    #[test]
    fn the_search_looks_up_only_minutes_the_stepper_visits() {
        // Daylight runs 02:00 to 05:00 UTC, so each offset lasts an hour and
        // the zone may claim `HourApart`.
        let zone = Recording {
            inner: DailyDaylight {
                start_minute: 120,
                end_minute: 300,
                shift_seconds: 3600,
                changes: OffsetChanges::HourApart,
            },
            seen: std::cell::RefCell::new(Vec::new()),
        };
        let after_ms = utc_ms(2026, 3, 1, 3, 0);
        for (expression, max_search_ms) in [
            ("30 2 * * *", 60 * MINUTE_MS),
            ("30 2 * * *", 150 * MINUTE_MS),
            ("30 2 * * *", DAY_MS),
            ("0 9 1 * *", 10 * DAY_MS),
            ("0 9 1 * *", 40 * DAY_MS),
            ("*/15 * * * *", 3 * MINUTE_MS),
            ("*/15 * * * *", 20 * MINUTE_MS),
        ] {
            zone.seen.borrow_mut().clear();
            let found = assert_matches_stepper(expression, after_ms, max_search_ms, &zone);
            // The stepper visits the epoch minutes after `after_ms` up to the
            // cap, or up to the match it returns.
            let first_ms = after_ms + MINUTE_MS;
            let last_ms = found.unwrap_or(after_ms + max_search_ms);
            let seen = zone.seen.borrow();
            let outside: Vec<i64> = seen
                .iter()
                .copied()
                .filter(|&ms| ms < first_ms || ms > last_ms || ms.rem_euclid(MINUTE_MS) != 0)
                .collect();
            assert!(
                outside.is_empty(),
                "{expression} within {max_search_ms}: lookups {outside:?} outside {first_ms}..={last_ms}"
            );
        }
    }

    #[test]
    fn the_jump_matches_the_stepper_at_the_edges_of_the_representable_range() {
        let utc = chrono::Utc;
        let last = chrono::DateTime::<chrono::Utc>::MAX_UTC.timestamp_millis();
        let first = chrono::DateTime::<chrono::Utc>::MIN_UTC.timestamp_millis();
        for after_ms in [
            last - 3 * DAY_MS,
            last - 90 * MINUTE_MS,
            last - MINUTE_MS,
            last,
            first - MINUTE_MS,
            first,
            first + 5,
        ] {
            for expression in ["* * * * *", "0 0 30 2 *", "59 23 31 12 *", "0 * * * *"] {
                assert_matches_stepper(expression, after_ms, 10 * DAY_MS, &utc);
                assert_matches_stepper(
                    expression,
                    after_ms,
                    10 * DAY_MS,
                    &ChronoTz::America__New_York,
                );
            }
        }
        for after_ms in [i64::MIN, i64::MIN + 1, i64::MAX - MINUTE_MS, i64::MAX] {
            assert_matches_stepper("* * * * *", after_ms, MAX_SEARCH_MS, &utc);
        }
    }

    fn cron_field(min: u64, max: u64) -> impl proptest::strategy::Strategy<Value = String> {
        use proptest::prelude::*;
        let value = min..=max;
        prop_oneof![
            Just("*".to_string()),
            (1..=max.max(1)).prop_map(|step| format!("*/{step}")),
            value.clone().prop_map(|value| value.to_string()),
            (value.clone(), value.clone()).prop_map(|(a, b)| format!("{}-{}", a.min(b), a.max(b))),
            (value.clone(), 1..=max.max(1)).prop_map(|(lo, step)| format!("{lo}/{step}")),
            proptest::collection::vec(value, 1..4).prop_map(|values| values
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(",")),
        ]
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(192))]

        #[test]
        fn the_field_jumping_search_matches_the_frozen_minute_stepper(
            minute in cron_field(0, 59),
            hour in cron_field(0, 23),
            dom in cron_field(1, 31),
            month in cron_field(1, 12),
            dow in cron_field(0, 7),
            after_ms in -5_364_662_400_000_i64..4_102_444_800_000,
            max_search_ms in 0..=(14 * DAY_MS),
            zone in 0..DIFFERENTIAL_ZONES.len(),
        ) {
            let expression = format!("{minute} {hour} {dom} {month} {dow}");
            proptest::prop_assume!(parse_cron(&expression).is_some());
            assert_matches_stepper(&expression, after_ms, max_search_ms, &DIFFERENTIAL_ZONES[zone]);
        }
    }

    #[test]
    fn the_jump_matches_the_stepper_across_every_transition_of_a_zone_year() {
        // Finds each zone's offset changes in 1900, 1916, 1937, and 2026 by an
        // hourly scan, then starts searches from 30 hours before to 2 hours
        // after each change with expressions that sit near transitions. A
        // day-of-week expression makes the search jump to midnight across a
        // weekend transition.
        let hour_ms = 60 * MINUTE_MS;
        for zone in DIFFERENTIAL_ZONES {
            let offset = |ms: i64| zone.timestamp_millis_opt(ms).unwrap().offset().fix();
            for year in [1900, 1916, 1937, 2026] {
                let start = utc_ms(year, 1, 1, 0, 0);
                let changes = (0..366 * 24)
                    .map(|hour| start + hour * hour_ms)
                    .filter(|&ms| offset(ms) != offset(ms + hour_ms))
                    .collect::<Vec<_>>();
                for change in changes {
                    for hours_before in -2..=30 {
                        let after_ms = change - hours_before * hour_ms + 13_000;
                        for expression in [
                            "0,30 0-3 * * *",
                            "15 * * * *",
                            "0 0 * * 0",
                            "30 0 * * 1",
                            "0 0 * * 1",
                            "45 23 * * 6",
                        ] {
                            assert_matches_stepper(expression, after_ms, 2 * DAY_MS, zone);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn next_occurrence_survives_extreme_instants() {
        // Extreme timestamps return no occurrence instead of overflowing in debug builds.
        let utc = chrono::Utc;
        assert_eq!(
            next_due_at_ms("* * * * *", i64::MIN, MAX_SEARCH_MS, &utc),
            None
        );
        assert_eq!(
            next_due_at_ms("* * * * *", i64::MAX, MAX_SEARCH_MS, &utc),
            None
        );
    }

    #[test]
    fn star_prefixed_day_fields_are_unrestricted_like_vixie_cron() {
        let utc = chrono::Utc;
        let monday_2026_09_07 = 1_788_739_200_000;
        let tuesday_2026_09_08 = 1_788_825_600_000;
        let monday_2026_09_14 = 1_789_344_000_000;

        // `*/1` in DOM leaves only DOW restricted: Mondays only.
        assert_eq!(
            next_due_at_ms("0 0 */1 * 1", monday_2026_09_07, MAX_SEARCH_MS, &utc),
            Some(monday_2026_09_14)
        );
        // `*/2` in DOW leaves only DOM restricted: the 8th, whatever its weekday.
        assert_eq!(
            next_due_at_ms("0 0 8 * */2", monday_2026_09_07, MAX_SEARCH_MS, &utc),
            Some(tuesday_2026_09_08)
        );
        // A bare list keeps DOM restricted, so the Vixie OR rule applies.
        assert_eq!(
            next_due_at_ms("0 0 1,8 * 1", monday_2026_09_07, MAX_SEARCH_MS, &utc),
            Some(tuesday_2026_09_08)
        );
    }

    #[test]
    fn a_cron_needs_exactly_five_whitespace_separated_fields() {
        for valid in ["* * * * *", "  0 9 * * 1-5 ", "0\t9\n*  *\u{a0}1"] {
            assert!(is_valid_conditional_note_cron(valid), "{valid:?}");
        }
        for invalid in [
            "",
            "   ",
            "* * * *",
            "* * * * * *",
            "* * * * ,",
            "* * * *  ",
        ] {
            assert!(!is_valid_conditional_note_cron(invalid), "{invalid:?}");
        }
    }

    /// `frozen_str_parser` serves as the byte scanner's test oracle.
    fn frozen_str_parser(expression: &str) -> Option<[u64; 5]> {
        fn is_digits(s: &str) -> bool {
            !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
        }
        fn field(token: &str, min: u64, max: u64, dow_field: bool) -> Option<u64> {
            let mut mask = 0u64;
            for part in token.split(',') {
                let piece = part.trim();
                if piece.is_empty() {
                    return None;
                }
                let mut split = piece.split('/');
                let range_part = split.next().unwrap_or_default();
                let step_part = split.next();
                if split.next().is_some() {
                    return None;
                }
                let step = match step_part {
                    Some(s) => {
                        if !is_digits(s) {
                            return None;
                        }
                        let step: u64 = s.parse().ok()?;
                        if step < 1 {
                            return None;
                        }
                        step
                    }
                    None => 1,
                };
                let (lo, hi) = if range_part == "*" {
                    (min, max)
                } else if range_part.contains('-') {
                    let mut bounds = range_part.split('-');
                    let lo_str = bounds.next().unwrap_or_default();
                    let hi_str = bounds.next().unwrap_or_default();
                    if bounds.next().is_some() || !is_digits(lo_str) || !is_digits(hi_str) {
                        return None;
                    }
                    (lo_str.parse().ok()?, hi_str.parse().ok()?)
                } else {
                    if !is_digits(range_part) {
                        return None;
                    }
                    let lo: u64 = range_part.parse().ok()?;
                    let hi = if step_part.is_some() { max } else { lo };
                    (lo, hi)
                };
                if lo < min || lo > max || hi < min || hi > max || lo > hi {
                    return None;
                }
                let mut v = lo;
                while v <= hi {
                    let normalized = if dow_field && v == 7 { 0 } else { v };
                    mask |= 1u64 << normalized;
                    let Some(next) = v.checked_add(step) else {
                        break;
                    };
                    v = next;
                }
            }
            if mask == 0 { None } else { Some(mask) }
        }
        let tokens: Vec<&str> = expression.split_whitespace().collect();
        if tokens.len() != 5 {
            return None;
        }
        Some([
            field(tokens[0], 0, 59, false)?,
            field(tokens[1], 0, 23, false)?,
            field(tokens[2], 1, 31, false)?,
            field(tokens[3], 1, 12, false)?,
            field(tokens[4], 0, 7, true)?,
        ])
    }

    fn cron_text() -> impl proptest::strategy::Strategy<Value = String> {
        use proptest::prelude::*;
        let piece = prop_oneof![
            Just("*"),
            Just("/"),
            Just("-"),
            Just(","),
            Just(" "),
            Just("\t"),
            Just("\u{b}"),
            Just("\u{a0}"),
            Just("\u{2003}"),
            Just("\u{1c}"),
            Just("0"),
            Just("1"),
            Just("7"),
            Just("8"),
            Just("12"),
            Just("23"),
            Just("24"),
            Just("31"),
            Just("59"),
            Just("60"),
            Just("007"),
            Just("+1"),
            Just("a"),
            Just("18446744073709551615"),
            Just("18446744073709551616"),
        ];
        proptest::collection::vec(piece, 0..24).prop_map(|pieces| pieces.concat())
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(1024))]

        #[test]
        fn the_byte_parser_accepts_exactly_what_the_str_parser_accepts(
            text in cron_text(),
            fields in (
                cron_field(0, 59),
                cron_field(0, 23),
                cron_field(1, 31),
                cron_field(1, 12),
                cron_field(0, 7),
            ),
        ) {
            let (minute, hour, dom, month, dow) = fields;
            let structured = format!("{minute} {hour} {dom} {month} {dow}");
            for expression in [text, structured] {
                let parsed = parse_cron(&expression)
                    .map(|cron| [cron.minute, cron.hour, cron.dom, cron.month, cron.dow]);
                proptest::prop_assert_eq!(parsed, frozen_str_parser(&expression), "{:?}", expression);
            }
        }

        #[test]
        fn the_month_scan_finds_the_date_a_day_by_day_scan_finds(
            dom in cron_field(1, 31),
            month in cron_field(1, 12),
            dow in cron_field(0, 7),
            start_days in -150_000_i32..1_200_000,
            span_days in 0_i64..3_000,
        ) {
            let expression = format!("0 0 {dom} {month} {dow}");
            let cron = parse_cron(&expression).unwrap();
            let start = chrono::NaiveDate::from_num_days_from_ce_opt(start_days).unwrap();
            let last = start + chrono::Days::new(span_days as u64);
            let mut expected = Some(start);
            while let Some(date) = expected.filter(|date| !date_could_match(&cron, date)) {
                expected = date.succ_opt().filter(|next| *next <= last);
            }
            proptest::prop_assert_eq!(first_matching_date(&cron, start, last), expected, "{}", expression);
        }

        #[test]
        fn the_jitter_hashes_the_formatted_note_seed(
            interval_ms in 0_i64..2 * CONDITIONAL_NOTE_CHECK_CEILING_MS,
            note_id in proptest::prelude::any::<i64>(),
            hash in proptest::option::of("\\PC{0,12}"),
        ) {
            let max = 60_000i64.min((interval_ms as f64 * 0.1).floor() as i64);
            let expected = if max <= 0 {
                0
            } else {
                let seed = format!("{note_id}:{}", hash.as_deref().unwrap_or(""));
                let h = seed.encode_utf16().fold(2166136261u32, |h, unit| {
                    (h ^ u32::from(unit)).wrapping_mul(16777619)
                });
                i64::from(h) % (max * 2 + 1) - max
            };
            proptest::prop_assert_eq!(deterministic_jitter_ms(interval_ms, note_id, hash.as_deref()), expected);
        }
    }

    #[test]
    fn the_jitter_seed_spells_extreme_note_ids() {
        for note_id in [i64::MIN, i64::MIN + 1, -1, 0, 9, 10, i64::MAX] {
            let seed = format!("{note_id}:abc");
            let h = seed.encode_utf16().fold(2166136261u32, |h, unit| {
                (h ^ u32::from(unit)).wrapping_mul(16777619)
            });
            let max = 60_000;
            assert_eq!(
                deterministic_jitter_ms(CONDITIONAL_NOTE_CHECK_CEILING_MS, note_id, Some("abc")),
                i64::from(h) % (max * 2 + 1) - max,
                "{note_id}"
            );
        }
    }

    fn compiled_state(now: i64) -> ConditionalNoteLifecycleState {
        ConditionalNoteLifecycleState {
            status: "pending".to_string(),
            ready_at: None,
            ready_reason: None,
            last_checked_at: Some(now - 1),
            updated_at: now - 1,
            compiled_check: Some("check".to_string()),
            manifest_json: Some("{}".to_string()),
            check_hash: Some("hash".to_string()),
            check_cron: Some("0 * * * *".to_string()),
            check_version: 1,
            check_status: "compiled".to_string(),
            check_failure_count: 0,
            check_network_failure_count: 0,
            check_quarantined_until: None,
            check_next_due_at: None,
            check_compiled_at: Some(now - 1),
            check_false_since_at: Some(now - CONDITIONAL_NOTE_CHECK_MAX_STALENESS_MS),
            check_last_liveness_at: None,
            policy_version: CONDITIONAL_NOTE_CHECK_POLICY_VERSION,
        }
    }

    #[test]
    fn a_liveness_logic_failure_is_compilable_at_once() {
        let now: i64 = 1_781_542_800_000;
        let tz = chrono::Utc;
        let mut pre = compiled_state(now);
        pre.check_next_due_at = Some(now + 12 * 60 * 60 * 1000);

        let reduced = reduce_conditional_note_evaluation(
            &pre,
            &ConditionalNoteEvaluationOutcome::Liveness(CheckOutcome::LogicFailed),
            7,
            now,
            &tz,
        );
        assert_eq!(reduced.next.check_status, "failing");
        assert_eq!(
            reduced.next.check_next_due_at,
            Some(now),
            "a future schedule must not delay reauthoring"
        );

        // A due time already in the past is left where it was.
        pre.check_next_due_at = Some(now - 60_000);
        let reduced = reduce_conditional_note_evaluation(
            &pre,
            &ConditionalNoteEvaluationOutcome::Liveness(CheckOutcome::LogicFailed),
            7,
            now,
            &tz,
        );
        assert_eq!(reduced.next.check_next_due_at, Some(now - 60_000));
    }

    #[test]
    fn reauthoring_a_failing_check_gets_the_full_compilation_budget() {
        let now: i64 = 1_781_542_800_000;
        let tz = chrono::Utc;
        let mut state = compiled_state(now);
        state.check_status = "failing".to_string();
        state.check_failure_count = MAX_FAILURES_BEFORE_REAUTHOR;

        let failed = ConditionalNoteEvaluationOutcome::Compile(CompileOutcome::CompilationFailed);
        for expected_count in 1..MAX_COMPILATION_FAILURES {
            state = reduce_conditional_note_evaluation(&state, &failed, 7, now, &tz).next;
            assert_eq!(state.check_failure_count, expected_count);
            assert_eq!(state.check_status, "uncompiled", "attempt {expected_count}");
        }
        state = reduce_conditional_note_evaluation(&state, &failed, 7, now, &tz).next;
        assert_eq!(state.check_failure_count, MAX_COMPILATION_FAILURES);
        assert_eq!(state.check_status, "fallback");
    }

    #[test]
    fn network_quarantine_excludes_a_stale_note_from_liveness_until_the_deadline() {
        let now: i64 = 1_781_542_800_000;
        let stale = ConditionalNoteSelectionSnapshot {
            id: 7,
            status: "pending".to_string(),

            created_at: 1,
            has_compiled_check: true,
            last_checked_at: Some(now - 1),
            check_status: "compiled".to_string(),
            check_quarantined_until: None,
            check_next_due_at: Some(now + 60_000),
            check_false_since_at: Some(now - CONDITIONAL_NOTE_CHECK_MAX_STALENESS_MS),
            check_last_liveness_at: None,
            policy_version: CONDITIONAL_NOTE_CHECK_POLICY_VERSION,
        };
        let ids = |notes: &[ConditionalNoteSelectionSnapshot]| -> Vec<i64> {
            get_stale_compiled_conditional_notes(notes, now, 10)
                .iter()
                .map(|n| n.id)
                .collect()
        };
        assert_eq!(ids(std::slice::from_ref(&stale)), vec![7]);

        let quarantined = ConditionalNoteSelectionSnapshot {
            check_quarantined_until: Some(now + 1),
            ..stale.clone()
        };
        assert!(
            ids(std::slice::from_ref(&quarantined)).is_empty(),
            "a quarantined note is excluded until check_quarantined_until expires"
        );

        let expired = ConditionalNoteSelectionSnapshot {
            check_quarantined_until: Some(now),
            ..stale
        };
        assert_eq!(ids(std::slice::from_ref(&expired)), vec![7]);
    }

    #[derive(Deserialize)]
    struct NormativeCycleTraces {
        cycle_trace_cases: Vec<CycleTraceCase>,
    }

    #[derive(Deserialize)]
    struct CycleTraceCase {
        id: String,
        mode: String,
        now: i64,
        #[serde(default)]
        notes: Vec<CycleNoteSeed>,
        steps: Vec<CycleStep>,
    }

    #[derive(Deserialize)]
    struct CycleNoteSeed {
        id: i64,
        status: Option<String>,
        created_at: Option<i64>,
        #[serde(default)]
        has_compiled_check: bool,
        last_checked_at: Option<i64>,
        check_status: Option<String>,
        check_quarantined_until: Option<i64>,
        check_next_due_at: Option<i64>,
        check_false_since_at: Option<i64>,
        check_last_liveness_at: Option<i64>,
        policy_version: Option<i64>,
    }

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum CycleExpectation {
        NoWork(String),
        Claim { note_id: i64, phase: String },
    }

    /// Each claim uses a fresh acquisition and removes the note from the eligible pool.
    /// An `after` patch returns the claimed note to the eligible pool after modeling its committed completion row.
    /// A bare `{}` patch releases the claim without changing the row.
    #[derive(Deserialize)]
    struct CycleStep {
        expect: CycleExpectation,
        #[serde(default)]
        after: Vec<CyclePatch>,
    }

    #[derive(Deserialize)]
    struct CyclePatch {
        note_id: i64,
        #[serde(default)]
        set: serde_json::Map<String, serde_json::Value>,
    }

    fn cycle_seed_snapshot(seed: &CycleNoteSeed, now: i64) -> ConditionalNoteSelectionSnapshot {
        ConditionalNoteSelectionSnapshot {
            id: seed.id,
            status: seed.status.clone().unwrap_or_else(|| "pending".to_string()),
            created_at: seed.created_at.unwrap_or(now - 1_000_000),
            has_compiled_check: seed.has_compiled_check,
            last_checked_at: seed.last_checked_at,
            check_status: seed
                .check_status
                .clone()
                .unwrap_or_else(|| "uncompiled".to_string()),
            check_quarantined_until: seed.check_quarantined_until,
            check_next_due_at: seed.check_next_due_at,
            check_false_since_at: seed.check_false_since_at,
            check_last_liveness_at: seed.check_last_liveness_at,
            policy_version: seed.policy_version.unwrap_or(1),
        }
    }

    fn apply_cycle_patch(
        note: &mut ConditionalNoteSelectionSnapshot,
        patch: &CyclePatch,
        case: &str,
    ) {
        let as_i64 = |value: &serde_json::Value| {
            value.as_i64().unwrap_or_else(|| {
                panic!(
                    "case {case}: patch value {value} for note {} must be an integer",
                    note.id
                )
            })
        };
        for (key, value) in &patch.set {
            match key.as_str() {
                "status" => note.status = value.as_str().expect("status string").to_string(),
                "check_status" => {
                    note.check_status = value.as_str().expect("check_status string").to_string()
                }
                "has_compiled_check" => {
                    note.has_compiled_check = value.as_bool().expect("has_compiled_check bool")
                }
                "last_checked_at" => note.last_checked_at = Some(as_i64(value)),
                "check_next_due_at" => note.check_next_due_at = Some(as_i64(value)),
                "check_false_since_at" => note.check_false_since_at = Some(as_i64(value)),
                "check_last_liveness_at" => note.check_last_liveness_at = Some(as_i64(value)),
                other => panic!("case {case}: unsupported patch field {other:?}"),
            }
        }
    }

    #[test]
    fn conditional_note_cycle_traces_normative_matches_selection_policy() {
        let raw = include_str!("../testdata/conditional-note-evaluation-normative.json");
        let normative: NormativeCycleTraces =
            serde_json::from_str(raw).expect("parse normative cycle traces");
        assert!(
            !normative.cycle_trace_cases.is_empty(),
            "normative fixture must carry cycle traces"
        );

        for case in &normative.cycle_trace_cases {
            let mode = match case.mode.as_str() {
                "full" => ConditionalNoteCycleMode::Full,
                "nonbillable" => ConditionalNoteCycleMode::Nonbillable,
                other => panic!("case {}: unknown mode {other:?}", case.id),
            };
            let mut pool: Vec<(ConditionalNoteSelectionSnapshot, bool)> = case
                .notes
                .iter()
                .map(|seed| (cycle_seed_snapshot(seed, case.now), false))
                .collect();
            let mut cycle = ConditionalNoteSelectionCycle::new(mode);
            for (step_index, step) in case.steps.iter().enumerate() {
                let visible: Vec<ConditionalNoteSelectionSnapshot> = pool
                    .iter()
                    .filter(|(_, claimed)| !claimed)
                    .map(|(note, _)| note.clone())
                    .collect();
                let selection =
                    select_conditional_note_evaluation_cycle(&visible, case.now, &cycle);
                match &step.expect {
                    CycleExpectation::NoWork(marker) => {
                        assert_eq!(marker, "no_work", "case {} step {step_index}", case.id);
                        assert_eq!(
                            selection
                                .as_ref()
                                .map(|(id, phase, _)| (*id, phase.clone())),
                            None,
                            "case {} step {step_index} expected no_work",
                            case.id
                        );
                        // A fresh durable no_work resets this mode's cycle.
                        cycle = ConditionalNoteSelectionCycle::new(mode);
                    }
                    CycleExpectation::Claim { note_id, phase } => {
                        let (selected_id, selected_phase, proposal) =
                            selection.unwrap_or_else(|| {
                                panic!(
                                    "case {} step {step_index} expected ({note_id}, {phase}), got no_work",
                                    case.id
                                )
                            });
                        assert_eq!(
                            (selected_id, selected_phase.as_str()),
                            (*note_id, phase.as_str()),
                            "case {} step {step_index}",
                            case.id
                        );
                        // A fresh committed claim installs the proposal.
                        // A committed claim marks the note unavailable until completion.
                        cycle = proposal;
                        pool.iter_mut()
                            .find(|(note, _)| note.id == selected_id)
                            .expect("claimed note exists")
                            .1 = true;
                    }
                }
                for patch in &step.after {
                    let entry = pool
                        .iter_mut()
                        .find(|(note, _)| note.id == patch.note_id)
                        .unwrap_or_else(|| {
                            panic!(
                                "case {}: patch targets unknown note {}",
                                case.id, patch.note_id
                            )
                        });
                    apply_cycle_patch(&mut entry.0, patch, &case.id);
                    entry.1 = false;
                }
            }
        }
    }
}
