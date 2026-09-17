//! Rendered-delta charging under each enabled accounting profile.

mod support;

use std::num::{NonZeroU64, NonZeroUsize};

use daemon::packing::{
    AccountingBound, AccountingBounds, AccountingProfile, Authority, Charged, ClaudeTokens, Ledger,
    PreparationRefusal, admit_render,
};
use kernel::applicability::EvalBudget;
use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};
use retrieval::fusion::OccurrenceId;
use support::packing::{Fixture, bounds, byte_profile, required_render_total, tool_span};

const SEED: [u8; 32] = *b"packing-accounting-seed-00000001";

fn profiles() -> Vec<AccountingProfile> {
    vec![
        AccountingProfile::exact_tokenizer(),
        byte_profile(),
        AccountingProfile::heuristic("bytes-over-four", "bytes/4", 250, |text| {
            text.len().div_ceil(4)
        }),
    ]
}

fn synthetic(byte: u8) -> OccurrenceId {
    OccurrenceId::parse(&format!("{byte:02x}").repeat(32)).unwrap()
}

/// The whole-render delta with no window: the oracle each ledger entry must
/// equal.
fn whole_render_delta(profile: &AccountingProfile, prefix: &str, fragment: &str) -> u64 {
    let before = profile.charge(prefix).tokens().get();
    let after = profile
        .charge(&format!("{prefix}{fragment}"))
        .tokens()
        .get();
    after.saturating_sub(before)
}

#[test]
fn every_charge_equals_the_whole_render_delta_and_every_byte_is_charged() {
    let fragment = prop::string::string_regex(
        "[A-Za-z0-9 <>&\"'\\n\\t.,;:(){}\\[\\]_\\-\u{e9}\u{3000}\u{1f389}]{0,2500}",
    )
    .unwrap();
    let fragments = prop::collection::vec(fragment, 1..12);
    let mut runner = TestRunner::new_with_rng(
        Config {
            cases: 96,
            rng_algorithm: RngAlgorithm::ChaCha,
            ..Config::default()
        },
        TestRng::from_seed(RngAlgorithm::ChaCha, &SEED),
    );
    let windowed_cases = std::cell::Cell::new(0usize);
    let escaped_cases = std::cell::Cell::new(0usize);
    runner
        .run(&fragments, |fragments| {
            if fragments
                .iter()
                .any(|fragment| fragment.contains(['<', '>', '&', '"', '\'']))
            {
                escaped_cases.set(escaped_cases.get() + 1);
            }
            for profile in profiles() {
                let mut ledger = Ledger::open(profile.clone());
                let mut expected_total = profile
                    .charge(daemon::packing::BLOCK_OPEN_FRAGMENT)
                    .tokens()
                    .get();
                prop_assert_eq!(ledger.entries()[0].charge.tokens().get(), expected_total);
                for (index, fragment) in fragments.iter().enumerate() {
                    let prefix = ledger.text().to_owned();
                    let expected = whole_render_delta(&profile, &prefix, fragment);
                    let charge = ledger.append(Charged::Required(synthetic(index as u8)), fragment);
                    prop_assert_eq!(
                        charge.tokens().get(),
                        expected,
                        "{} fragment {}",
                        profile.identity(),
                        index
                    );
                    prop_assert_eq!(charge.authority(), profile.authority());
                    prop_assert!(charge.with_headroom() >= charge.tokens());
                    expected_total += expected;
                }
                let close = whole_render_delta(
                    &profile,
                    ledger.text(),
                    daemon::packing::BLOCK_CLOSE_FRAGMENT,
                );
                ledger.close();
                expected_total += close;
                prop_assert_eq!(ledger.total().get(), expected_total);
                prop_assert_eq!(
                    ledger
                        .entries()
                        .iter()
                        .map(|entry| entry.bytes)
                        .sum::<usize>(),
                    ledger.text().len(),
                    "every rendered byte belongs to an entry"
                );
                if ledger.rendered_bytes() > daemon::packing::render::DELTA_LOOKBACK_BYTES {
                    windowed_cases.set(windowed_cases.get() + 1);
                }
                let whole = profile.charge(ledger.text()).tokens().get();
                prop_assert_eq!(
                    ledger.total().get(),
                    whole,
                    "{}: the sum of deltas is the whole-render estimate",
                    profile.identity()
                );
            }
            Ok(())
        })
        .unwrap();
    assert!(
        windowed_cases.get() > 0,
        "some renders must exceed the lookback so the anchored path is exercised"
    );
    assert!(
        escaped_cases.get() > 0,
        "some fragments must carry XML-significant bytes so escaping is charged"
    );
}

/// A tail run of one character class longer than the lookback is the one
/// shape where a byte-aligned window would mis-tokenize; the piece-aligned
/// anchor must still equal the whole-render delta.
#[test]
fn a_tail_run_longer_than_the_lookback_still_charges_the_whole_render_delta() {
    let profile = AccountingProfile::exact_tokenizer();
    let lookback = daemon::packing::render::DELTA_LOOKBACK_BYTES;
    for (run, continuation) in [
        (" ".repeat(lookback + 801), "x\n"),
        ("a".repeat(lookback + 801), "qz\n"),
        (".".repeat(lookback + 801), ">\n"),
        ("7".repeat(lookback + 801), "42\n"),
        ("\n".repeat(lookback + 801), "\n\n"),
    ] {
        let mut ledger = Ledger::open(profile.clone());
        ledger.append(Charged::Required(synthetic(1)), &run);
        let prefix = ledger.text().to_owned();
        let expected = whole_render_delta(&profile, &prefix, continuation);
        let charge = ledger.append(Charged::Required(synthetic(2)), continuation);
        assert_eq!(charge.tokens().get(), expected, "{continuation:?}");
        ledger.close();
        assert_eq!(
            ledger.total().get(),
            profile.charge(ledger.text()).tokens().get(),
            "{continuation:?}"
        );
    }
}

#[test]
fn a_heuristic_count_carries_its_authority_and_headroom_and_never_the_exact_label() {
    let heuristic = AccountingProfile::heuristic("bytes-over-four", "bytes/4", 250, |text| {
        text.len().div_ceil(4)
    });
    let charge = heuristic.charge("twelve bytes");
    assert_eq!(
        charge.authority(),
        Authority::Heuristic {
            degradation: "bytes/4",
            headroom_permille: 250,
        }
    );
    assert_eq!(charge.tokens(), ClaudeTokens::new(3));
    assert_eq!(charge.with_headroom(), ClaudeTokens::new(4));
    assert_ne!(charge.authority(), Authority::Exact);
    // The headroom product exceeds `u64`; the adjusted count still fits and
    // is charged at the requested ratio, not at a saturated product.
    let wide =
        AccountingProfile::heuristic("wide", "wide headroom", 4_000_000_000, |_| 10_000_000_000);
    assert_eq!(
        wide.charge_uncached("x").with_headroom(),
        ClaudeTokens::new(10_000_000_000 + 40_000_000_000_000_000)
    );
    let saturated =
        AccountingProfile::heuristic("saturated", "saturated", u32::MAX, |_| usize::MAX);
    assert_eq!(
        saturated.charge_uncached("x").with_headroom(),
        ClaudeTokens::new(u64::MAX)
    );

    let exact = AccountingProfile::exact_tokenizer();
    assert_eq!(exact.charge("twelve bytes").authority(), Authority::Exact);
    assert_eq!(
        exact.charge("twelve bytes").with_headroom(),
        exact.charge("twelve bytes").tokens()
    );
    let identity = exact.identity();
    assert!(
        exact
            .revision()
            .as_str()
            .starts_with(&format!("5:exact;{}:{identity};64:", identity.len())),
        "the exact revision names its own identity: {}",
        exact.revision().as_str()
    );
    assert_ne!(exact.revision(), heuristic.revision());
    let other_degradation =
        AccountingProfile::heuristic("bytes-over-four", "bytes/3", 250, |text| text.len() / 3);
    assert_ne!(heuristic.revision(), other_degradation.revision());
    for profile in profiles() {
        assert_eq!(
            profile.declared_uncharged(),
            daemon::packing::DECLARED_UNCHARGED,
            "{}: the declared exclusions do not vary by profile",
            profile.identity()
        );
    }
    assert_eq!(
        daemon::packing::DECLARED_UNCHARGED,
        &["separator-before-memory-block"]
    );
}

#[test]
fn a_heuristic_impersonating_the_exact_revision_shares_neither_revision_nor_cache_entry() {
    use sha2::Digest;
    let exact = AccountingProfile::exact_tokenizer();
    let digest: &'static str =
        Box::leak(format!("{:x}", sha2::Sha256::digest(tokenizer::vocab_blob())).into_boxed_str());
    let impostor = AccountingProfile::heuristic(exact.identity(), digest, 0, |_| 1);
    assert_ne!(impostor.revision(), exact.revision());

    let content = "content long enough to enter the shared cache under either revision ".repeat(2);
    let expected = tokenizer::estimate_tokens(&content) as u64;
    assert_ne!(expected, 1, "the fixture must discriminate the two counts");
    assert_eq!(impostor.charge(&content).tokens(), ClaudeTokens::new(1));
    let charge = exact.charge(&content);
    assert_eq!(charge.tokens(), ClaudeTokens::new(expected));
    assert_eq!(charge.authority(), Authority::Exact);
    assert_eq!(impostor.charge(&content).tokens(), ClaudeTokens::new(1));
}

#[test]
fn a_count_cached_under_one_revision_is_not_served_under_another_profile() {
    let content = "shared content long enough to enter the cache under both revisions".repeat(2);
    let first = AccountingProfile::heuristic("first-profile", "constant", 0, |_| 7);
    let second = AccountingProfile::heuristic("second-profile", "constant", 0, |_| 11);
    assert_eq!(first.charge(&content).tokens(), ClaudeTokens::new(7));
    assert_eq!(second.charge(&content).tokens(), ClaudeTokens::new(11));
    assert_eq!(first.charge(&content).tokens(), ClaudeTokens::new(7));
    assert_eq!(second.charge(&content).tokens(), ClaudeTokens::new(11));
}

#[test]
fn rendered_bytes_and_estimated_tokens_bounds_refuse_at_limit_plus_one() {
    for profile in profiles() {
        let mut ledger = Ledger::open(profile.clone());
        ledger.append(Charged::Required(synthetic(1)), "some rendered bytes\n");
        ledger.close();
        let bytes = ledger.rendered_bytes();
        let tokens = ledger.total_with_headroom();
        let at = AccountingBounds {
            max_rendered_bytes: bytes,
            max_estimated_tokens: tokens,
        };
        assert_eq!(admit_render(&ledger, &at), Ok(()), "{}", profile.identity());
        let one_byte_short = AccountingBounds {
            max_rendered_bytes: bytes - 1,
            ..at
        };
        let exceeded = admit_render(&ledger, &one_byte_short).unwrap_err();
        assert_eq!(exceeded.bound, AccountingBound::RenderedBytes);
        assert_eq!(
            (exceeded.value, exceeded.limit),
            (bytes as u64, bytes as u64 - 1)
        );
        let one_token_short = AccountingBounds {
            max_estimated_tokens: ClaudeTokens::new(tokens.get() - 1),
            ..at
        };
        let exceeded = admit_render(&ledger, &one_token_short).unwrap_err();
        assert_eq!(exceeded.bound, AccountingBound::EstimatedTokens);
        assert_eq!(
            (exceeded.value, exceeded.limit),
            (tokens.get(), tokens.get() - 1)
        );
    }
}

#[test]
fn the_required_phase_refuses_a_render_beyond_the_accounting_bounds() {
    let span = tool_span("call-1", "1", "required bytes for the accounting bound\n");
    let fixture = Fixture::new(&[span]);
    let total_bytes = required_render_total(&[span]) as usize;
    let mut trace = daemon::packing::PackingTrace::default();
    let profile = byte_profile();
    let inputs = daemon::packing::RequiredInputs {
        kernel: &fixture.kernel,
        project: &fixture.project,
        destination: kernel::ArtifactDestination::Local,
        budget: &EvalBudget::unbounded(),
        profile: &profile,
    };
    let at = AccountingBounds {
        max_rendered_bytes: total_bytes,
        max_estimated_tokens: ClaudeTokens::new(total_bytes as u64),
    };
    let ok = daemon::packing::prepare_required(
        &fixture.store,
        inputs,
        &[span.request()],
        &bounds(1 << 20),
        &at,
        &mut trace,
    );
    assert!(ok.is_ok());
    let short = AccountingBounds {
        max_rendered_bytes: total_bytes - 1,
        ..at
    };
    let refused = daemon::packing::prepare_required(
        &fixture.store,
        inputs,
        &[span.request()],
        &bounds(1 << 20),
        &short,
        &mut trace,
    );
    match refused.unwrap_err() {
        PreparationRefusal::Accounting(exceeded) => {
            assert_eq!(exceeded.bound, AccountingBound::RenderedBytes);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_optional_phase_refuses_a_render_beyond_the_accounting_bounds_and_a_foreign_profile() {
    let required = tool_span("call-1", "1", "required\n");
    let optional = tool_span("call-2", "1", "optional bytes that push the render over\n");
    let fixture = Fixture::new(&[required, optional]);
    let profile = byte_profile();
    let inputs = daemon::packing::RequiredInputs {
        kernel: &fixture.kernel,
        project: &fixture.project,
        destination: kernel::ArtifactDestination::Local,
        budget: &EvalBudget::unbounded(),
        profile: &profile,
    };
    let wide = AccountingBounds {
        max_rendered_bytes: 1 << 20,
        max_estimated_tokens: ClaudeTokens::new(1 << 20),
    };
    let optional_bounds = retrieval::packing::OptionalBounds {
        max_fused_candidates: NonZeroUsize::new(4).unwrap(),
        max_parents: NonZeroUsize::new(4).unwrap(),
        max_spans_per_parent: NonZeroUsize::new(4).unwrap(),
        max_payload_loads: NonZeroUsize::new(4).unwrap(),
        max_payload_bytes: NonZeroU64::new(1 << 20).unwrap(),
        max_item_bytes: NonZeroU64::new(1 << 20).unwrap(),
    };
    let mut trace = daemon::packing::PackingTrace::default();
    let materialized = daemon::packing::prepare_required(
        &fixture.store,
        inputs,
        &[required.request()],
        &bounds(1 << 20),
        &wide,
        &mut trace,
    )
    .unwrap();
    let request = daemon::packing::OptionalRequest {
        occurrence: optional.id(),
        revision: 1,
    };
    let closed = daemon::packing::prepare_optional(
        &fixture.store,
        inputs,
        &materialized,
        &[request],
        &optional_bounds,
        &wide,
        &mut trace,
    )
    .unwrap();
    let bytes = closed.ledger.rendered_bytes();
    let tokens = closed.ledger.total_with_headroom();
    assert!(bytes > materialized.ledger().rendered_bytes());

    for (bounds, expected) in [
        (
            AccountingBounds {
                max_rendered_bytes: bytes,
                max_estimated_tokens: tokens,
            },
            None,
        ),
        (
            AccountingBounds {
                max_rendered_bytes: bytes - 1,
                max_estimated_tokens: tokens,
            },
            Some(AccountingBound::RenderedBytes),
        ),
        (
            AccountingBounds {
                max_rendered_bytes: bytes,
                max_estimated_tokens: ClaudeTokens::new(tokens.get() - 1),
            },
            Some(AccountingBound::EstimatedTokens),
        ),
    ] {
        let result = daemon::packing::prepare_optional(
            &fixture.store,
            inputs,
            &materialized,
            &[request],
            &optional_bounds,
            &bounds,
            &mut trace,
        );
        match (result, expected) {
            (Ok(_), None) => {}
            (Err(PreparationRefusal::Accounting(exceeded)), Some(bound)) => {
                assert_eq!(exceeded.bound, bound);
            }
            (other, expected) => panic!("{other:?} vs {expected:?}"),
        }
    }

    let foreign = AccountingProfile::exact_tokenizer();
    let foreign_inputs = daemon::packing::RequiredInputs {
        profile: &foreign,
        ..inputs
    };
    let result = daemon::packing::prepare_optional(
        &fixture.store,
        foreign_inputs,
        &materialized,
        &[request],
        &optional_bounds,
        &wide,
        &mut trace,
    );
    assert_eq!(result.unwrap_err(), PreparationRefusal::ProfileMismatch);
}

/// At every token limit the closed render either fits, with the limit split
/// exactly between what the ledger charged and what remains, or the phase
/// refuses; no admitted render exceeds the limit under any profile.
#[test]
fn consumed_budget_plus_remaining_is_the_token_limit_under_every_profile() {
    let required = tool_span("call-1", "1", "required bytes for the headroom check\n");
    let a = tool_span("call-2", "1", "an optional span, the first of two\n");
    let b = tool_span(
        "call-3",
        "1",
        "another optional span, priced after the first\n",
    );
    let fixture = Fixture::new(&[required, a, b]);
    let optional_bounds = retrieval::packing::OptionalBounds {
        max_fused_candidates: NonZeroUsize::new(4).unwrap(),
        max_parents: NonZeroUsize::new(4).unwrap(),
        max_spans_per_parent: NonZeroUsize::new(4).unwrap(),
        max_payload_loads: NonZeroUsize::new(4).unwrap(),
        max_payload_bytes: NonZeroU64::new(1 << 20).unwrap(),
        max_item_bytes: NonZeroU64::new(1 << 20).unwrap(),
    };
    let wide = AccountingBounds {
        max_rendered_bytes: 1 << 20,
        max_estimated_tokens: ClaudeTokens::new(1 << 20),
    };
    let requests = [
        daemon::packing::OptionalRequest {
            occurrence: a.id(),
            revision: 1,
        },
        daemon::packing::OptionalRequest {
            occurrence: b.id(),
            revision: 1,
        },
    ];
    for profile in profiles() {
        let inputs = daemon::packing::RequiredInputs {
            kernel: &fixture.kernel,
            project: &fixture.project,
            destination: kernel::ArtifactDestination::Local,
            budget: &EvalBudget::unbounded(),
            profile: &profile,
        };
        let mut admitted_counts = std::collections::BTreeSet::new();
        for token_limit in (40u64..=400).chain([1 << 20]) {
            let mut trace = daemon::packing::PackingTrace::default();
            let Ok(materialized) = daemon::packing::prepare_required(
                &fixture.store,
                inputs,
                &[required.request()],
                &bounds(token_limit),
                &wide,
                &mut trace,
            ) else {
                continue;
            };
            assert_eq!(
                materialized.charged().get() + materialized.remaining().get(),
                token_limit,
                "{}: the required phase splits the limit exactly",
                profile.identity()
            );
            let Ok(closed) = daemon::packing::prepare_optional(
                &fixture.store,
                inputs,
                &materialized,
                &requests,
                &optional_bounds,
                &wide,
                &mut trace,
            ) else {
                continue;
            };
            admitted_counts.insert(closed.admitted.len());
            assert_eq!(
                closed.ledger.total_with_headroom().get() + closed.remaining.get(),
                token_limit,
                "{} at {token_limit}: {} admitted",
                profile.identity(),
                closed.admitted.len()
            );
        }
        assert_eq!(
            admitted_counts,
            [0, 1, 2].into_iter().collect(),
            "{}: the sweep reaches every admission count",
            profile.identity()
        );
    }
}
