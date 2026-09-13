# Property lens: security boundaries

Provenance: HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.

Malformed input under a discarded field must still follow the existing tree
acceptance boundary. The gate checks range, nesting, UTF-8, escapes, and the
serde_json raw-value token at `crates/daemon/src/lib.rs:15864-15949`.
Removing retained unknown fields is not permission to remove that gate.

Keep payload Value content distinct from envelope sanitation. Tool inputs,
provider extras, and opaque raw parts are observable data, not unknown fields
that an indiscriminate recursive filter may delete.

No authentication or privilege change is proposed. The relevant security
obligation is parser consistency before dispatch, covered by the fallback and
payload records. Redaction enforcement and storage policy remain their
existing catalogs; fewer copies do not prove those policies hold.
