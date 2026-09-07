use std::cmp::Reverse;

use secret_scanner::{Finding, RuleSource};

use super::{
    DETECTOR_ID, Detection, Redaction, RedactionError, RedactionErrorKind, redaction_type_for_key,
};

fn provider_label(rule_id: &str) -> Option<(&'static str, &'static str)> {
    Some(match rule_id {
        "magic-anthropic-api-key" => ("anthropic_api_key", "<ANTHROPIC_API_KEY_REDACTED>"),
        "magic-openai-api-key" => ("openai_api_key", "<OPENAI_API_KEY_REDACTED>"),
        "magic-github-pat" => ("github_pat", "<GITHUB_PAT_REDACTED>"),
        "magic-github-token" => ("github_token", "<GITHUB_TOKEN_REDACTED>"),
        "magic-huggingface-token" => ("huggingface_token", "<HUGGINGFACE_TOKEN_REDACTED>"),
        "magic-aws-access-key-id" => ("aws_access_key_id", "<AWS_ACCESS_KEY_ID_REDACTED>"),
        "magic-slack-token" => ("slack_token", "<SLACK_TOKEN_REDACTED>"),
        "magic-google-api-key" => ("google_api_key", "<GOOGLE_API_KEY_REDACTED>"),
        "magic-bearer-token" => ("bearer", "<REDACTED:bearer>"),
        "magic-jwt" => ("jwt", "<JWT_REDACTED>"),
        _ => return None,
    })
}

#[cfg(test)]
const KEYED_RULE_IDS: &[&str] = &[
    "magic-keyed-assignment",
    "magic-keyed-assignment-double-quoted",
    "magic-keyed-assignment-single-quoted",
    "magic-keyed-double-quoted",
    "magic-keyed-single-quoted",
    "magic-keyed-double-single",
    "magic-keyed-single-double",
];

#[cfg(test)]
fn is_known_rule(rule_id: &str) -> bool {
    provider_label(rule_id).is_some() || KEYED_RULE_IDS.contains(&rule_id)
}

/// Precedence among findings that cover overlapping bytes; the lowest value wins.
///
/// A key name states the operator's own intent for the value, so it outranks a
/// value-shape guess. An unclassified upstream shape ranks last because its
/// label carries no provider or key information.
const KEYED_PRECEDENCE: u8 = 0;
const PROVIDER_PRECEDENCE: u8 = 1;
const GENERIC_PRECEDENCE: u8 = 2;

#[derive(Debug)]
pub(super) struct Replacement {
    start: usize,
    end: usize,
    /// Lowest `specificity` supplies the label when findings overlap.
    specificity: u8,
    secret_type: String,
    replacement: String,
}

/// Describes each finding in `input` as a replacement positioned at `base +
/// span`, so findings from one scan window can be rendered against the text
/// that window was cut from.
///
/// Detection offsets and lengths are UTF-8 byte positions in `input`. Findings
/// may arrive in any order. Returns [`RedactionErrorKind::InvalidSpan`] when
/// any span is not a valid string range, and [`RedactionErrorKind::UnknownRule`]
/// for an unclassified conservative-overlay rule.
pub(super) fn describe_findings(
    input: &str,
    findings: &[Finding],
    base: usize,
) -> Result<Vec<Replacement>, RedactionError> {
    let mut replacements = Vec::with_capacity(findings.len());
    for finding in findings {
        let value = finding.value_span;
        input
            .get(value.start()..value.end())
            .ok_or_else(invalid_span)?;
        let key = match finding.key_span {
            Some(span) => Some(
                input
                    .get(span.start()..span.end())
                    .ok_or_else(invalid_span)?,
            ),
            None => None,
        };
        replacements.push(describe(
            &finding.rule_id,
            finding.rule_source,
            key,
            base.checked_add(value.start()).ok_or_else(invalid_span)?,
            base.checked_add(value.end()).ok_or_else(invalid_span)?,
        )?);
    }
    Ok(replacements)
}

/// Replaces all described spans and reports one detection per overlapping
/// cluster. Keyed labels win provider labels, which win generic labels.
///
/// Merged output has no overlaps, so merging again is a no-op and callers may
/// pass raw or already-merged replacements. commentlint: allow(JUDGE)
pub(super) fn render(
    input: &str,
    replacements: Vec<Replacement>,
) -> Result<Redaction, RedactionError> {
    render_merged(input, merge(replacements))
}

/// Collapses every overlapping cluster into one replacement covering its union, labelled by its winner.
///
/// Rendering the result is identical to rendering the input, so a windowed scan can merge after every
/// window and count detections rather than raw findings.
pub(super) fn merge(mut replacements: Vec<Replacement>) -> Vec<Replacement> {
    sort_for_clustering(&mut replacements);
    let mut merged: Vec<Replacement> = Vec::with_capacity(replacements.len());
    for replacement in replacements {
        match merged.last_mut() {
            Some(cluster) if replacement.start < cluster.end => {
                cluster.end = cluster.end.max(replacement.end);
                if replacement.specificity < cluster.specificity {
                    cluster.specificity = replacement.specificity;
                    cluster.secret_type = replacement.secret_type;
                    cluster.replacement = replacement.replacement;
                }
            }
            _ => merged.push(replacement),
        }
    }
    merged
}

/// `merged` must hold the output of [`merge`]: sorted by start and overlap-free.
///
/// Re-merges only the `merged` suffix with `end > earliest`, because earlier clusters cannot
/// overlap `incoming`. commentlint: allow(JUDGE)
pub(super) fn merge_into(merged: &mut Vec<Replacement>, mut incoming: Vec<Replacement>) {
    let Some(earliest) = incoming.iter().map(|replacement| replacement.start).min() else {
        return;
    };
    let suffix_start = merged.partition_point(|cluster| cluster.end <= earliest);
    incoming.extend(merged.drain(suffix_start..));
    merged.extend(merge(incoming));
}

/// Widest span first so a cluster's union is known from its first member, then
/// lowest specificity, so `merge` can pick a winner without rescanning.
fn sort_for_clustering(replacements: &mut [Replacement]) {
    replacements.sort_by(|left, right| {
        (left.start, Reverse(left.end), left.specificity).cmp(&(
            right.start,
            Reverse(right.end),
            right.specificity,
        ))
    });
}

fn describe(
    rule_id: &str,
    source: RuleSource,
    key: Option<&str>,
    start: usize,
    end: usize,
) -> Result<Replacement, RedactionError> {
    if let Some(key) = key {
        let secret_type = redaction_type_for_key(key);
        return Ok(Replacement {
            start,
            end,
            specificity: KEYED_PRECEDENCE,
            replacement: format!("<REDACTED:{secret_type}>"),
            secret_type,
        });
    }
    if let Some((secret_type, replacement)) = provider_label(rule_id) {
        return Ok(Replacement {
            start,
            end,
            specificity: PROVIDER_PRECEDENCE,
            secret_type: secret_type.to_owned(),
            replacement: replacement.to_owned(),
        });
    }
    if source == RuleSource::ConservativeOverlay {
        return Err(RedactionError {
            kind: RedactionErrorKind::UnknownRule,
        });
    }
    Ok(Replacement {
        start,
        end,
        specificity: GENERIC_PRECEDENCE,
        secret_type: "secret".to_owned(),
        replacement: "<REDACTED:secret>".to_owned(),
    })
}

/// Callers must provide replacements sorted by start with non-overlapping spans.
fn render_merged(input: &str, replacements: Vec<Replacement>) -> Result<Redaction, RedactionError> {
    // Reserving the whole upper bound keeps the buffer from doubling on its
    // first byte past `input.len()`, which for a payload near the cap would
    // hold twice the payload while the input is still resident.
    let bound = input.len()
        + replacements
            .iter()
            .map(|replacement| replacement.replacement.len())
            .sum::<usize>();
    let mut text = String::with_capacity(bound);
    let reserved = text.capacity();
    let mut detections = Vec::with_capacity(replacements.len());
    let mut cursor = 0;
    for replacement in replacements {
        // Merged spans never overlap, so `replacement.start >= cursor`; a span with
        // `replacement.start < cursor` makes this range invalid and fails closed.
        text.push_str(
            input
                .get(cursor..replacement.start)
                .ok_or_else(invalid_span)?,
        );
        text.push_str(&replacement.replacement);
        detections.push(Detection {
            detector_id: DETECTOR_ID,
            secret_type: replacement.secret_type,
            offset: replacement.start,
            length: replacement
                .end
                .checked_sub(replacement.start)
                .ok_or_else(invalid_span)?,
        });
        cursor = replacement.end;
    }
    text.push_str(input.get(cursor..).ok_or_else(invalid_span)?);
    debug_assert!(text.capacity() == reserved, "rendering must not reallocate");
    Ok(Redaction { text, detections })
}

const fn invalid_span() -> RedactionError {
    RedactionError {
        kind: RedactionErrorKind::InvalidSpan,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rendering_reserves_its_bound_once_and_never_grows() {
        // Every one-byte span becomes a 17-byte placeholder, so the output is
        // far longer than the input.
        let input = "x".repeat(64);
        let replacements: Vec<Replacement> = (0..64)
            .map(|start| Replacement {
                start,
                end: start + 1,
                specificity: GENERIC_PRECEDENCE,
                secret_type: "secret".to_owned(),
                replacement: "<REDACTED:secret>".to_owned(),
            })
            .collect();
        let redaction = render(&input, replacements).unwrap();
        assert_eq!(redaction.text.len(), 64 * "<REDACTED:secret>".len());
        assert!(redaction.text.len() > input.len());
        assert_eq!(redaction.detections.len(), 64);
    }

    fn replacement(start: usize, end: usize, specificity: u8, label: &str) -> Replacement {
        Replacement {
            start,
            end,
            specificity,
            secret_type: label.to_owned(),
            replacement: format!("<REDACTED:{label}>"),
        }
    }

    /// A transitively overlapping chain collapses to its union labelled by the
    /// lowest specificity, and a span that only touches the union stays apart.
    #[test]
    fn merge_and_render_collapse_the_same_clusters() {
        let input = "y".repeat(25);
        let build = || {
            vec![
                replacement(14, 20, PROVIDER_PRECEDENCE, "c"),
                replacement(0, 10, GENERIC_PRECEDENCE, "a"),
                replacement(20, 25, KEYED_PRECEDENCE, "d"),
                replacement(5, 15, KEYED_PRECEDENCE, "b"),
            ]
        };

        let merged = merge(build());
        assert_eq!(merged.len(), 2);
        assert_eq!((merged[0].start, merged[0].end), (0, 20));
        assert_eq!(merged[0].specificity, KEYED_PRECEDENCE);
        assert_eq!(merged[0].secret_type, "b");
        assert_eq!(merged[0].replacement, "<REDACTED:b>");
        assert_eq!((merged[1].start, merged[1].end), (20, 25));
        assert_eq!(merged[1].secret_type, "d");

        let redaction = render(&input, build()).unwrap();
        assert_eq!(redaction.text, "<REDACTED:b><REDACTED:d>");
        assert_eq!(redaction.detections.len(), 2);
        assert_eq!(redaction.detections[0].secret_type, "b");
        assert_eq!(redaction.detections[0].offset, 0);
        assert_eq!(redaction.detections[0].length, 20);
        assert_eq!(redaction.detections[1].secret_type, "d");
        assert_eq!(redaction.detections[1].offset, 20);
        assert_eq!(redaction.detections[1].length, 5);
    }

    /// Later batches can begin inside or before existing clusters.
    #[test]
    fn merge_into_matches_merging_everything_at_once() {
        let batches = || {
            [
                vec![
                    replacement(0, 10, GENERIC_PRECEDENCE, "a"),
                    replacement(30, 40, GENERIC_PRECEDENCE, "b"),
                    replacement(60, 70, PROVIDER_PRECEDENCE, "c"),
                ],
                vec![],
                vec![
                    replacement(38, 45, KEYED_PRECEDENCE, "d"),
                    replacement(55, 62, GENERIC_PRECEDENCE, "e"),
                ],
                vec![replacement(10, 12, GENERIC_PRECEDENCE, "f")],
                vec![replacement(90, 95, GENERIC_PRECEDENCE, "g")],
            ]
        };
        let project = |clusters: &[Replacement]| {
            clusters
                .iter()
                .map(|cluster| {
                    (
                        cluster.start,
                        cluster.end,
                        cluster.specificity,
                        cluster.secret_type.clone(),
                        cluster.replacement.clone(),
                    )
                })
                .collect::<Vec<_>>()
        };
        let mut incremental = Vec::new();
        for batch in batches() {
            merge_into(&mut incremental, batch);
        }
        let all_at_once = merge(batches().into_iter().flatten().collect());
        assert_eq!(project(&incremental), project(&all_at_once));
        assert_eq!(
            incremental
                .iter()
                .map(|cluster| (cluster.start, cluster.end, cluster.secret_type.as_str()))
                .collect::<Vec<_>>(),
            [
                (0, 10, "a"),
                (10, 12, "f"),
                (30, 45, "d"),
                (55, 70, "c"),
                (90, 95, "g")
            ]
        );
    }

    fn overlay_rule_names() -> Vec<&'static str> {
        let overlay = include_str!("../../../secret-scanner/conservative_overlay.yaml");
        overlay
            .lines()
            .filter_map(|line| {
                let trimmed = line.trim_start();
                trimmed
                    .strip_prefix("- name:")
                    .or_else(|| trimmed.strip_prefix("name:"))
            })
            .map(|name| name.trim().trim_matches('"'))
            .filter(|name| name.starts_with("magic-"))
            .collect()
    }

    #[test]
    fn every_overlay_rule_is_classified() {
        let names = overlay_rule_names();
        for name in &names {
            assert!(is_known_rule(name), "unclassified overlay rule: {name}");
        }
        assert_eq!(
            names.len(),
            17,
            "overlay rule count changed; update the classifier table"
        );
    }

    /// `memory-store` persists `Detection::secret_type` as `scan_detections.label_id`,
    /// whose `CHECK` admits 1..=64 bytes of `[a-z0-9_]`. A provider label outside that
    /// shape reaches `memory-store` and aborts its durable write when that secret type is
    /// detected. commentlint: allow(JUDGE)
    #[test]
    fn every_provider_label_fits_the_persisted_label_shape() {
        let mut providers = 0;
        for name in overlay_rule_names() {
            let Some((secret_type, _)) = provider_label(name) else {
                continue;
            };
            providers += 1;
            assert_persistable_label(secret_type);
        }
        assert_eq!(
            providers, 10,
            "provider label count changed; re-check the shape"
        );
        // The key-derived fallback label takes the same column.
        assert_persistable_label("secret");
    }

    fn assert_persistable_label(label: &str) {
        assert!(
            (1..=64).contains(&label.len()),
            "label {label:?} is {} bytes; scan_detections.label_id admits 1..=64",
            label.len()
        );
        assert!(
            label
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'),
            "label {label:?} leaves [a-z0-9_]; scan_detections.label_id rejects it"
        );
    }

    /// Each overlay replacement must match a shape `contains_redaction_token`
    /// hard-codes, or redacted text reads as unredacted.
    #[test]
    fn every_replacement_is_a_recognized_redaction_token() {
        use super::super::contains_redaction_token;

        for name in overlay_rule_names() {
            let key = KEYED_RULE_IDS.contains(&name).then_some("password");
            let replacement = describe(name, RuleSource::ConservativeOverlay, key, 0, 4).unwrap();
            assert!(
                contains_redaction_token(&replacement.replacement),
                "{name}: {} is not recognized as a redaction token",
                replacement.replacement
            );
        }
        let generic = describe("age-secret-key", RuleSource::Upstream, None, 0, 4).unwrap();
        assert!(contains_redaction_token(&generic.replacement));
    }

    #[test]
    fn unmapped_overlay_rule_is_rejected() {
        assert_eq!(
            describe(
                "magic-renamed-provider",
                RuleSource::ConservativeOverlay,
                None,
                0,
                4
            )
            .unwrap_err()
            .kind(),
            RedactionErrorKind::UnknownRule
        );
    }

    #[test]
    fn unmapped_upstream_rule_uses_the_generic_label() {
        let replacement = describe("age-secret-key", RuleSource::Upstream, None, 0, 4).unwrap();
        assert_eq!(replacement.secret_type, "secret");
        assert_eq!(replacement.replacement, "<REDACTED:secret>");
    }

    #[test]
    fn keyed_finding_without_a_known_label_still_redacts() {
        let replacement = describe(
            "magic-keyed-assignment",
            RuleSource::ConservativeOverlay,
            Some("apikey"),
            0,
            4,
        )
        .unwrap();
        assert_eq!(replacement.secret_type, "secret");
        assert_eq!(replacement.replacement, "<REDACTED:secret>");
    }
}
