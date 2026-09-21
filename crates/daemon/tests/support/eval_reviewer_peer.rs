//! The MemoryReviewer keyed peer: reviewer requests are served strictly from
//! recorded entries keyed on the attempt-marker tuple (body digest, provider
//! identity, model, credential id), so a request the recording never saw is a
//! typed miss and every later request is refused too.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::tls_peer::{Observed, Peer, json_response};

/// The reviewer's attempt-marker tuple as the peer can recover it from one
/// request: `provider` is `{host}/v1/messages@{anthropic-version}` from the
/// request head, `model` from the body, `body_digest` over the body bytes, and
/// `credential_id` from the peer's configuration because the header carries
/// only the secret.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ReviewerKey {
    pub body_digest: String,
    pub provider: String,
    pub model: String,
    pub credential_id: String,
}

impl ReviewerKey {
    pub fn of(observed: &Observed, credential_id: &str) -> Self {
        let header = |name: &str| {
            observed
                .head
                .lines()
                .find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.eq_ignore_ascii_case(name)
                        .then(|| value.trim().to_string())
                })
                .unwrap_or_default()
        };
        let model = serde_json::from_slice::<Value>(&observed.body)
            .ok()
            .and_then(|value| value["model"].as_str().map(str::to_string))
            .unwrap_or_default();
        Self {
            body_digest: format!("{:x}", Sha256::digest(&observed.body)),
            provider: format!(
                "{}/v1/messages@{}",
                header("host"),
                header("anthropic-version")
            ),
            model,
            credential_id: credential_id.to_string(),
        }
    }
}

/// Serves `turns` reviewer connections strictly from `entries`. A request
/// whose key has no entry is answered with a typed `cassette_miss` refusal,
/// and every later request is refused too, so the run stops at the first miss
/// as it does at the other two boundaries.
pub fn serve_keyed(
    peer: &mut Peer,
    turns: usize,
    entries: BTreeMap<ReviewerKey, Vec<u8>>,
    credential_id: &str,
) -> tokio::task::JoinHandle<Vec<Observed>> {
    let credential_id = credential_id.to_string();
    let mut missed = false;
    peer.serve_each(turns, move |request| {
        let key = ReviewerKey::of(request, &credential_id);
        match entries.get(&key) {
            Some(response) if !missed => response.clone(),
            _ => {
                missed = true;
                let body = json!({"type": "error", "error": {"type": "cassette_miss", "body_digest": key.body_digest}});
                json_response("409 Conflict", &body.to_string(), "")
            }
        }
    })
}
