//! The two routed methods a harness answers stage-1 classifier work through. `memory.review.classify.next` hands the calling route the oldest classifier request of its project, waiting a bounded time for one; `memory.review.classify.submit` returns the classifier's answers, or one content-free failure code, under the lease the work carried. The classifier's credential and its call stay in the harness.

use std::time::Duration;

use serde_json::{Value, json};

use super::classifier::{MAX_CLASSIFY_DURATION_MS, MAX_NEXT_WAIT_MS};
use crate::{HandlerCore, PreparedOutcome, RouteHandle, invalid_params_error, respond};

pub(crate) const NEXT: &str = "memory.review.classify.next";
pub(crate) const SUBMIT: &str = "memory.review.classify.submit";
/// The failure codes a harness may submit in place of answers.
const FAILURE_CODES: [&str; 3] = ["cancelled", "provider_unavailable", "model_failed"];
const MAX_LEASE_BYTES: usize = 64;
/// The largest answers object a harness may submit: four typed answers with their probabilities.
const MAX_ANSWERS_BYTES: usize = 16 * 1024;

impl HandlerCore {
    /// The project digest and classifier of the route's project, when its user tier set the review switch and named the classifier the request names.
    fn review_classifier(
        &self,
        channel: RouteHandle,
        request: &Value,
        operation: &str,
        fields: &[&str],
    ) -> Result<Option<(String, String)>, PreparedOutcome> {
        let (_, binding) = self.management_binding(channel, request, operation)?;
        // The body is closed: the routed envelope and this method's own fields.
        if request.as_object().is_none_or(|body| {
            body.keys().any(|key| {
                !["method", "session_id", "project_root"].contains(&key.as_str())
                    && !fields.contains(&key.as_str())
            })
        }) {
            return Err(invalid_params_error(
                "memory review classifier request has unknown fields",
            ));
        }
        if let Some(root) = request.get("project_root") {
            let bound = root.as_str().is_some_and(|root| {
                let path = std::path::Path::new(root);
                path.is_absolute() && binding.kernel_project.accepts(path)
            });
            if !bound {
                return Err(invalid_params_error(
                    "memory review classifier project does not match the route",
                ));
            }
        }
        let Some(named) = request.get("classifier").and_then(Value::as_str) else {
            return Err(invalid_params_error(
                "classifier must be a provider/model string",
            ));
        };
        Ok(binding
            .config
            .memory_reviewer_classifier()
            .filter(|configured| *configured == named)
            .map(|configured| {
                (
                    binding.kernel_project.digest().to_string(),
                    configured.to_string(),
                )
            }))
    }

    /// Hands the harness one classifier request of its project, or reports that none waits. `wait_ms`, at most [`MAX_NEXT_WAIT_MS`], holds the request open for work to arrive. A project that has the switch off, names no classifier, or names another one answers `disabled`.
    pub(crate) async fn handle_review_classify_next(
        &self,
        channel: RouteHandle,
        request: &Value,
    ) -> PreparedOutcome {
        let (project, classifier) =
            match self.review_classifier(channel, request, NEXT, &["classifier", "wait_ms"]) {
                Ok(Some(bound)) => bound,
                Ok(None) => return respond(json!({"state":"disabled"})),
                Err(error) => return error,
            };
        let wait = match request.get("wait_ms") {
            None => 0,
            Some(value) => match value.as_u64() {
                Some(wait) if wait <= MAX_NEXT_WAIT_MS => wait,
                _ => return invalid_params_error("wait_ms is out of range"),
            },
        };
        let work = tokio::select! {
            biased;
            () = self.cancel.cancelled() => None,
            work = self
                .review_classifiers
                .next(&project, &classifier, Duration::from_millis(wait)) => work,
        };
        match work {
            Some(work) => respond(json!({
                "state": "work",
                "lease": work.lease,
                "classifier": work.classifier,
                "request": { "state": &*work.state, "questions": &*work.questions },
                "max_duration_ms": MAX_CLASSIFY_DURATION_MS,
            })),
            None => respond(json!({"state":"ready"})),
        }
    }

    /// Takes a harness's answers, or its failure code, for the request `lease` names. `processed` means the waiting review took them; `stale` means the lease had lapsed or the review had ended.
    pub(crate) fn handle_review_classify_submit(
        &self,
        channel: RouteHandle,
        request: &Value,
    ) -> PreparedOutcome {
        let (project, _) = match self.review_classifier(
            channel,
            request,
            SUBMIT,
            &["classifier", "lease", "answers", "error"],
        ) {
            Ok(Some(bound)) => bound,
            Ok(None) => return respond(json!({"state":"disabled"})),
            Err(error) => return error,
        };
        let Some(lease) = request
            .get("lease")
            .and_then(Value::as_str)
            .filter(|lease| !lease.is_empty() && lease.len() <= MAX_LEASE_BYTES)
        else {
            return invalid_params_error("lease must be a nonempty string");
        };
        let result = match (request.get("answers"), request.get("error")) {
            (Some(answers), None) if answers.is_object() => {
                if serde_json::to_vec(answers).map_or(usize::MAX, |bytes| bytes.len())
                    > MAX_ANSWERS_BYTES
                {
                    return invalid_params_error("answers exceed the answer bound");
                }
                Ok(answers.clone())
            }
            (None, Some(Value::String(code))) if FAILURE_CODES.contains(&code.as_str()) => {
                Err(code.clone())
            }
            _ => {
                return invalid_params_error(
                    "submit carries either an answers object or one failure code",
                );
            }
        };
        let state = if self.review_classifiers.submit(&project, lease, result) {
            "processed"
        } else {
            "stale"
        };
        respond(json!({ "state": state }))
    }
}
