//! Exact untruncated model token counts and the embedding preflight that stands between a text and inference.
//! Expected token sequences come from hand-derived fixtures, never from the counting implementation.

mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use host_runtime::synapse::bundle::load_bundle;
use host_runtime::synapse::embed_tokens::UntruncatedTokenizer;
use host_runtime::synapse::inference::InferenceError;
use host_runtime::synapse::{
    DenseUnavailable, EmbedTokens, EmbeddingEngine, EmbeddingIdentity, EmbeddingInputLimits,
    InferenceFailureKind, LaneInfo, LaneUnavailableState, SynapseComponent, SynapseLimits,
    SynapseStatus,
};
use support::synapse::{DeterministicEngine, ready_component, test_lane};
use tokenizers::{AddedToken, PaddingParams, PaddingStrategy, Tokenizer, TruncationParams};

fn fixture_dir(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn read(name: &str, file: &str) -> Vec<u8> {
    std::fs::read(fixture_dir(name).join(file)).expect("fixture file")
}

fn expected() -> serde_json::Value {
    serde_json::from_slice(&read("embed-tokens", "expected.json")).expect("expected.json")
}

fn u32s(value: &serde_json::Value) -> Vec<u32> {
    value
        .as_array()
        .expect("array")
        .iter()
        .map(|v| u32::try_from(v.as_u64().expect("u64")).expect("u32"))
        .collect()
}

/// The counter is a clone of the tokenizer as inference configures it, padded and truncated at four, and must undo both.
fn fixture_counter() -> UntruncatedTokenizer {
    UntruncatedTokenizer::from_tokenizer(inference_shaped(Some(4), PaddingStrategy::BatchLongest))
}

/// The fixture tokenizer as inference configures it: padded, truncated at `window`, and with every `special_tokens_map.json` entry added after loading, the way the inference engine registers them.
fn inference_shaped(window: Option<usize>, padding: PaddingStrategy) -> Tokenizer {
    let mut tokenizer =
        Tokenizer::from_bytes(read("embed-tokens", "tokenizer.json")).expect("tokenizer");
    tokenizer.with_padding(Some(PaddingParams {
        strategy: padding,
        pad_token: "[PAD]".to_owned(),
        pad_id: 0,
        ..Default::default()
    }));
    if let Some(max_length) = window {
        tokenizer
            .with_truncation(Some(TruncationParams {
                max_length,
                ..Default::default()
            }))
            .expect("truncation");
    }
    let map: serde_json::Value =
        serde_json::from_slice(&read("embed-tokens", "special_tokens_map.json")).unwrap();
    for value in map.as_object().unwrap().values() {
        let contents: Vec<&str> = match value {
            serde_json::Value::String(content) => vec![content.as_str()],
            serde_json::Value::Array(items) => items.iter().map(|i| i.as_str().unwrap()).collect(),
            serde_json::Value::Object(object) => vec![object["content"].as_str().unwrap()],
            _ => unreachable!(),
        };
        for content in contents {
            tokenizer.add_special_tokens(&[AddedToken {
                content: content.to_owned(),
                special: true,
                ..Default::default()
            }]);
        }
    }
    tokenizer
}

#[test]
fn pinned_token_sequences_certify_full_counts_special_tokens_and_padding() {
    let counter = fixture_counter();
    let expected = expected();
    let cases = expected["sequences"].as_array().expect("sequences");
    assert_eq!(cases.len(), 10, "every pinned sequence is exercised");
    for case in cases {
        let label = case["label"].as_str().unwrap();
        let text = case["text"].as_str().unwrap();
        let sequence = counter
            .encode(text)
            .unwrap_or_else(|e| panic!("{label}: {e}"));
        assert_eq!(sequence.ids(), u32s(&case["ids"]), "{label}: ids");
        assert_eq!(
            sequence.attention_mask(),
            u32s(&case["attention_mask"]),
            "{label}: attention"
        );
        assert_eq!(
            sequence.special_tokens_mask(),
            u32s(&case["special_tokens_mask"]),
            "{label}: special tokens"
        );
        assert_eq!(
            sequence.tokens(),
            EmbedTokens::new(sequence.ids().len() as u32)
        );
        assert_eq!(
            counter.count(text).unwrap(),
            EmbedTokens::new(u32s(&case["ids"]).len() as u32),
            "{label}: count is the pinned sequence length"
        );
    }

    // A padded batch pads to its longest member; each count is that text's own length.
    let batch = &expected["padded_batch"];
    let texts: Vec<&str> = batch["texts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t.as_str().unwrap())
        .collect();
    let padded = inference_shaped(None, PaddingStrategy::BatchLongest)
        .encode_batch(texts.clone(), true)
        .expect("padded batch");
    let padded_length = batch["padded_length"].as_u64().unwrap() as usize;
    assert_eq!(texts.len(), 3);
    assert_eq!(u32s(&batch["counts"]).len(), texts.len());
    assert_eq!(batch["padded_ids"].as_array().unwrap().len(), texts.len());
    assert_eq!(padded.len(), texts.len());
    for ((text, encoding), (count, ids)) in texts.iter().zip(&padded).zip(
        u32s(&batch["counts"])
            .into_iter()
            .zip(batch["padded_ids"].as_array().unwrap()),
    ) {
        assert_eq!(
            encoding.get_ids(),
            u32s(ids).as_slice(),
            "{text}: padded ids"
        );
        assert_eq!(encoding.get_ids().len(), padded_length);
        assert_eq!(
            encoding.get_attention_mask().iter().sum::<u32>(),
            count,
            "{text}: real tokens under padding"
        );
        assert_eq!(
            counter.count(text).unwrap(),
            EmbedTokens::new(count),
            "{text}"
        );
    }

    // Fixed padding pads even a single text; the count still names the real tokens.
    let fixed = &expected["fixed_padding_control"];
    let text = fixed["text"].as_str().unwrap();
    let pad_to = fixed["pad_to"].as_u64().unwrap() as usize;
    let fixed_tokenizer = inference_shaped(None, PaddingStrategy::Fixed(pad_to));
    assert_eq!(
        fixed_tokenizer.encode(text, true).unwrap().get_ids().len(),
        fixed["padded_length"].as_u64().unwrap() as usize
    );
    let fixed_counter = UntruncatedTokenizer::from_tokenizer(fixed_tokenizer);
    assert_eq!(
        fixed_counter.count(text).unwrap(),
        EmbedTokens::new(fixed["count"].as_u64().unwrap() as u32)
    );

    // Negative control: a count taken through the truncating tokenizer fits the window the full sequence exceeds.
    let control = &expected["truncation_control"];
    let text = control["text"].as_str().unwrap();
    let window = control["window"].as_u64().unwrap() as usize;
    let truncated = inference_shaped(Some(window), PaddingStrategy::BatchLongest)
        .encode(text, true)
        .expect("truncated encoding")
        .get_ids()
        .len();
    assert_eq!(
        truncated,
        control["truncated_count"].as_u64().unwrap() as usize
    );
    assert!(truncated <= window, "a truncating count always fits");
    let full = counter.count(text).unwrap();
    assert_eq!(full.get(), control["full_count"].as_u64().unwrap() as u32);
    assert!(full.get() as usize > window, "the full count does not fit");
}

/// Word counts of the tiny bundle's corpus, derived from its whitespace vocabulary by hand.
const TINY_CORPUS_COUNTS: &[(&str, u32)] = &[
    ("alpha beta gamma", 3),
    ("kappa iota theta eta", 4),
    ("alpha", 1),
    ("alpha beta gamma delta epsilon zeta eta theta", 8),
    (
        "alpha beta gamma delta epsilon zeta eta theta iota kappa",
        10,
    ),
    ("unknownword alpha", 2),
];

#[test]
fn the_verified_bundle_counts_its_corpus_in_process_without_truncation() {
    let bundle = load_bundle(
        &fixture_dir("synapse-tiny"),
        &SynapseLimits::default(),
        None,
    )
    .expect("the tiny bundle verifies");
    assert_eq!(bundle.manifest.max_tokens, 8);
    let counter = UntruncatedTokenizer::from_tokenizer(
        Tokenizer::from_bytes(&bundle.tokenizer_file).expect("verified tokenizer bytes"),
    );
    for (text, count) in TINY_CORPUS_COUNTS {
        assert_eq!(
            counter.count(text).unwrap(),
            EmbedTokens::new(*count),
            "{text}"
        );
    }
    // The ten-word corpus row is over the bundle's window; inference would truncate it to eight.
    let over = counter.count(TINY_CORPUS_COUNTS[4].0).unwrap();
    assert!(over.get() > bundle.manifest.max_tokens as u32);
    assert_eq!(counter.count("   ").unwrap(), EmbedTokens::new(0));
}

fn words(count: usize) -> String {
    (0..count)
        .map(|index| format!("w{index}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn limits_of(lane: &LaneInfo) -> EmbeddingInputLimits {
    EmbeddingInputLimits::of_lane(lane)
}

/// The AC2 boundary: the at-limit text reaches inference unchanged, and the one-over text makes no call.
fn assert_limit_boundary(
    component: &SynapseComponent,
    engine: &DeterministicEngine,
    lane: &LaneInfo,
) {
    let limits = limits_of(lane);
    // Certification is complete; every inference call from here is the product's.
    let baseline = engine.calls.load(Ordering::SeqCst);

    let at_limit = words(lane.max_tokens as usize);
    let admitted = component
        .preflight_embedding(limits, &at_limit)
        .expect("at-limit input is admitted");
    assert_eq!(admitted.tokens(), EmbedTokens::new(lane.max_tokens));
    assert_eq!(admitted.identity(), &EmbeddingIdentity::of_lane(lane));
    let vector = component
        .embed_admitted(&admitted)
        .expect("admitted input embeds");
    assert_eq!(vector, engine.vector_for(&at_limit));
    assert_eq!(engine.calls.load(Ordering::SeqCst), baseline + 1);
    assert_eq!(
        engine.call_texts.lock().unwrap().as_slice(),
        std::slice::from_ref(&at_limit),
        "the text reaches inference unchanged"
    );

    let one_over = words(lane.max_tokens as usize + 1);
    assert_eq!(
        component.preflight_embedding(limits, &one_over),
        Err(DenseUnavailable::TokenOverflow {
            tokens: EmbedTokens::new(lane.max_tokens + 1),
            max_tokens: EmbedTokens::new(lane.max_tokens),
        })
    );
    assert_eq!(
        engine.calls.load(Ordering::SeqCst),
        baseline + 1,
        "one-over input makes zero inference calls"
    );
}

#[test]
fn exact_limit_input_reaches_inference_unchanged_and_one_over_makes_no_call() {
    let engine = DeterministicEngine::new();
    let component = ready_component(Arc::clone(&engine), SynapseLimits::default());
    let SynapseStatus::Ready(lane) = component.status() else {
        panic!("ready lane");
    };
    assert_limit_boundary(&component, &engine, &lane);
    let limits = limits_of(&lane);
    let baseline = engine.calls.load(Ordering::SeqCst);
    let one_over = words(lane.max_tokens as usize + 1);

    // A product limit narrower than the window is honored; one wider than the window cannot enlarge it.
    let narrow = EmbeddingInputLimits {
        max_tokens: EmbedTokens::new(3),
        ..limits
    };
    assert!(matches!(
        component.preflight_embedding(narrow, &words(4)),
        Err(DenseUnavailable::TokenOverflow { tokens, max_tokens })
            if tokens == EmbedTokens::new(4) && max_tokens == EmbedTokens::new(3)
    ));
    let wide = EmbeddingInputLimits {
        max_tokens: EmbedTokens::new(lane.max_tokens * 2),
        max_bytes: lane.max_text_bytes * 2,
    };
    assert!(matches!(
        component.preflight_embedding(wide, &one_over),
        Err(DenseUnavailable::TokenOverflow { max_tokens, .. })
            if max_tokens == EmbedTokens::new(lane.max_tokens)
    ));
    // Nor can a wide byte limit enlarge the lane's byte cap or reach the tokenizer.
    let counts_before = engine.count_calls.load(Ordering::SeqCst);
    let too_long = "x".repeat(lane.max_text_bytes + 1);
    assert_eq!(
        component.preflight_embedding(wide, &too_long),
        Err(DenseUnavailable::ByteOverflow {
            bytes: too_long.len(),
            max_bytes: lane.max_text_bytes,
        })
    );
    assert_eq!(engine.count_calls.load(Ordering::SeqCst), counts_before);
    assert_eq!(engine.calls.load(Ordering::SeqCst), baseline);
}

/// An engine whose count truncates at the window, as a count read off the inference tokenizer would.
struct TruncatingCounter {
    inner: Arc<DeterministicEngine>,
    window: u32,
}

impl EmbeddingEngine for TruncatingCounter {
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, InferenceError> {
        self.inner.embed(texts)
    }

    fn untruncated_token_len(&self, text: &str) -> Result<EmbedTokens, InferenceError> {
        let full = self.inner.untruncated_token_len(text)?;
        Ok(EmbedTokens::new(full.get().min(self.window)))
    }
}

/// Negative control: the same boundary assertions reject a count that truncates at the window.
#[test]
#[should_panic(expected = "TokenOverflow")]
fn a_truncating_count_fails_the_limit_boundary() {
    let engine = DeterministicEngine::new();
    let lane = test_lane();
    let component = SynapseComponent::ready_with_engine(
        lane.clone(),
        Arc::new(TruncatingCounter {
            inner: Arc::clone(&engine),
            window: lane.max_tokens,
        }),
        SynapseLimits::default(),
    )
    .expect("lane");
    assert_limit_boundary(&component, &engine, &lane);
}

/// An engine that counts with the fixture's real tokenizer and never expects to embed.
struct FixtureTokenizerEngine {
    counter: UntruncatedTokenizer,
    count_calls: AtomicUsize,
}

impl EmbeddingEngine for FixtureTokenizerEngine {
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, InferenceError> {
        Ok(texts
            .iter()
            .map(|_| vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0])
            .collect())
    }

    fn untruncated_token_len(&self, text: &str) -> Result<EmbedTokens, InferenceError> {
        self.count_calls.fetch_add(1, Ordering::SeqCst);
        self.counter.count(text)
    }
}

fn fixture_component() -> (Arc<FixtureTokenizerEngine>, SynapseComponent, LaneInfo) {
    let engine = Arc::new(FixtureTokenizerEngine {
        counter: fixture_counter(),
        count_calls: AtomicUsize::new(0),
    });
    let mut lane = test_lane();
    lane.max_tokens = 10;
    let component = SynapseComponent::ready_with_engine(
        lane.clone(),
        Arc::clone(&engine) as Arc<dyn EmbeddingEngine>,
        SynapseLimits::default(),
    )
    .expect("lane");
    let SynapseStatus::Ready(lane) = component.status() else {
        panic!("ready lane");
    };
    (engine, component, lane)
}

const CANARY: &str = "CANARY-PAYLOAD-7f3e";

/// A refusal must not carry the text it refused.
fn assert_non_content(reason: &DenseUnavailable) {
    let display = reason.to_string();
    let debug = format!("{reason:?}");
    assert!(!display.contains(CANARY), "{display}");
    assert!(!debug.contains(CANARY), "{debug}");
}

#[test]
fn byte_overflow_missing_identity_empty_input_and_lane_failures_have_exact_dispositions() {
    let (engine, component, lane) = fixture_component();
    let limits = EmbeddingInputLimits {
        max_bytes: 32,
        max_tokens: EmbedTokens::new(lane.max_tokens),
    };

    // Byte overflow is judged before any tokenizer work.
    let oversized = format!("{CANARY} {}", "x".repeat(40));
    let refusal = component
        .preflight_embedding(limits, &oversized)
        .unwrap_err();
    assert_eq!(
        refusal,
        DenseUnavailable::ByteOverflow {
            bytes: oversized.len(),
            max_bytes: 32,
        }
    );
    assert_non_content(&refusal);
    assert_eq!(engine.count_calls.load(Ordering::SeqCst), 0);

    // Nothing to embed, judged before any tokenizer work.
    assert_eq!(
        component.preflight_embedding(limits, ""),
        Err(DenseUnavailable::EmptyInput)
    );
    assert_eq!(engine.count_calls.load(Ordering::SeqCst), 0);

    // Whitespace has bytes but, under a tokenizer without a template, no tokens.
    let bundle = load_bundle(
        &fixture_dir("synapse-tiny"),
        &SynapseLimits::default(),
        None,
    )
    .unwrap();
    let tiny = SynapseComponent::ready_with_engine(
        test_lane(),
        Arc::new(FixtureTokenizerEngine {
            counter: UntruncatedTokenizer::from_tokenizer(
                Tokenizer::from_bytes(&bundle.tokenizer_file).unwrap(),
            ),
            count_calls: AtomicUsize::new(0),
        }),
        SynapseLimits::default(),
    )
    .unwrap();
    let refusal = tiny
        .preflight_embedding(limits_of(&test_lane()), " \t ")
        .unwrap_err();
    assert_eq!(refusal, DenseUnavailable::ZeroTokens { bytes: 3 });

    // No verified identity: a lane that never loaded, and a lane the platform does not support.
    let not_initialized = SynapseComponent::new(None);
    assert_eq!(
        not_initialized.preflight_embedding(limits, "alpha"),
        Err(DenseUnavailable::LaneUnavailable {
            state: LaneUnavailableState::Disabled,
        })
    );
    let unsupported = SynapseComponent::unsupported("synapse_unsupported");
    assert_eq!(
        unsupported.preflight_embedding(limits, "alpha"),
        Err(DenseUnavailable::LaneUnavailable {
            state: LaneUnavailableState::Disabled,
        })
    );

    // A count the lane cannot produce names its failure class, not the text.
    let deterministic = DeterministicEngine::new();
    let component = ready_component(Arc::clone(&deterministic), SynapseLimits::default());
    let deterministic_limits = limits_of(&test_lane());
    *deterministic.fail_next_count.lock().unwrap() =
        Some(InferenceError::Artifact(format!("{CANARY} tokenizer lost")));
    let refusal = component
        .preflight_embedding(limits_of(&test_lane()), "alpha beta")
        .unwrap_err();
    assert_eq!(
        refusal,
        DenseUnavailable::CountUnavailable(InferenceFailureKind::Artifact)
    );
    assert_non_content(&refusal);
    assert_eq!(deterministic.calls.load(Ordering::SeqCst), 0);

    // An invariant failure marks the lane failing; automatic dispatch stops until an operator repairs it.
    deterministic.fail_next(InferenceError::Invariant(format!("{CANARY} bad vector")));
    let admitted = component
        .preflight_embedding(deterministic_limits, "alpha beta")
        .unwrap();
    let failed = component.embed_admitted(&admitted).unwrap_err();
    assert_eq!(
        failed,
        DenseUnavailable::Inference(InferenceFailureKind::Invariant)
    );
    assert_non_content(&failed);
    assert!(matches!(component.status(), SynapseStatus::Failing { .. }));
    assert_eq!(
        component.preflight_embedding(deterministic_limits, "alpha beta"),
        Err(DenseUnavailable::LaneUnavailable {
            state: LaneUnavailableState::Failing,
        })
    );
}

#[test]
fn a_held_inference_permit_is_reported_as_busy_not_as_an_artifact_fault() {
    let engine = DeterministicEngine::new();
    let component = Arc::new(ready_component(
        Arc::clone(&engine),
        SynapseLimits::default(),
    ));
    let SynapseStatus::Ready(lane) = component.status() else {
        panic!("ready lane");
    };
    let limits = limits_of(&lane);
    let gate = engine.block_calls();
    let holder = {
        let component = Arc::clone(&component);
        std::thread::spawn(move || {
            let admitted = component.preflight_embedding(limits, "alpha beta").unwrap();
            component.embed_admitted(&admitted)
        })
    };
    while engine.calls.load(Ordering::SeqCst) == 0 {
        std::thread::yield_now();
    }
    // Counting does not need the permit, so admission still succeeds while inference is busy.
    let admitted = component
        .preflight_embedding(limits, "gamma delta")
        .expect("counting runs outside the permit");
    assert_eq!(
        component.embed_admitted(&admitted),
        Err(DenseUnavailable::LaneBusy {
            retry_after_ms: SynapseLimits::default().query_retry_after_ms,
        })
    );
    assert!(matches!(component.status(), SynapseStatus::Ready(_)));
    DeterministicEngine::release_calls(&gate);
    holder.join().unwrap().expect("the held call completes");
    component
        .embed_admitted(&admitted)
        .expect("the lane is free again");
}

#[test]
fn admission_keeps_exact_bytes_and_repeats_identically() {
    let (engine, component, lane) = fixture_component();
    let limits = limits_of(&lane);
    for (text, count) in [
        ("alpha beta", 4),
        ("   ", 2),
        ("alpha \t\n beta", 4),
        ("αβγ beta", 4),
        ("alpha [SEP] beta", 5),
        ("alpha beta gamma delta alpha beta gamma delta", 10),
    ] {
        let first = component.preflight_embedding(limits, text).unwrap();
        let second = component.preflight_embedding(limits, text).unwrap();
        assert!(
            std::ptr::eq(first.text(), text),
            "the admitted text is the caller's own bytes"
        );
        assert_eq!(first.text(), text);
        assert_eq!(first.bytes(), text.len());
        assert_eq!(first.tokens(), EmbedTokens::new(count), "{text}");
        assert_eq!(first, second, "repeated preflight is identical");
        assert_eq!(first.identity(), &EmbeddingIdentity::of_lane(&lane));
    }
    // Nine words exceed the ten-token window because the template adds two.
    assert_eq!(
        component.preflight_embedding(
            limits,
            "alpha beta gamma delta alpha beta gamma delta alpha"
        ),
        Err(DenseUnavailable::TokenOverflow {
            tokens: EmbedTokens::new(11),
            max_tokens: EmbedTokens::new(10),
        })
    );
    assert_eq!(engine.count_calls.load(Ordering::SeqCst), 13);
}

#[test]
fn an_admitted_input_embeds_only_under_the_lane_that_admitted_it() {
    let engine = DeterministicEngine::new();
    let component = ready_component(Arc::clone(&engine), SynapseLimits::default());
    let SynapseStatus::Ready(lane) = component.status() else {
        panic!("ready lane");
    };
    let admitted = component
        .preflight_embedding(limits_of(&lane), "alpha beta")
        .unwrap();

    let mut other_fingerprint = test_lane();
    other_fingerprint.fingerprint = "f0e1d2c3b4a59687".repeat(4);
    let mut other_epoch = test_lane();
    other_epoch.table_epoch += 1;
    for other_lane in [other_fingerprint, other_epoch] {
        let other = SynapseComponent::ready_with_engine(
            other_lane,
            DeterministicEngine::new(),
            SynapseLimits::default(),
        )
        .expect("lane");
        assert_eq!(
            other.embed_admitted(&admitted),
            Err(DenseUnavailable::IdentityChanged)
        );
    }
    assert_eq!(engine.calls.load(Ordering::SeqCst), 0);
    component.embed_admitted(&admitted).unwrap();
    assert_eq!(engine.calls.load(Ordering::SeqCst), 1);
}
