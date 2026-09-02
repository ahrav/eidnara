# Durable store consumer inventory

Provenance: captured at `primitives@89abb40` for the four crates, from
default-branch reads of each consumer source taken on 2026-09-01. Of the
consumers below, only the host source's module store is in migration scope; it
becomes `crates/memory-store`, `crates/daemon`, and the kernel crates in U4 and
U5, and its unfenced `with_conn` mutations are an open issue in the `host`
source. The credentials store, the sibling module store, and the two
repositories that returned 404 are outside Eidnara; the credentials-store
review duty is released to the single maintainer in `migration/owners.json`.
The PostgreSQL backend in the source (`primitives@89abb40`) is not carried, so
the "PostgreSQL consumers" row is closed by removal rather than migration.

Receipts are reads of consumer sources outside this tree. Docs cite only
in-tree code plus a source alias and commit, so the rows below carry the
finding and the read date, not paths or line ranges into those sources.

| Consumer | Receipt | Finding |
|---|---|---|
| A credentials store outside Eidnara | Commit-pinned source read, 2026-09-01 | `fenced_write` delegates durable writes to `with_conn_fenced`. Unfenced `with_conn` remains in the file for reads and connection setup. |
| A module store in a sibling repository outside Eidnara | Commit-pinned source read, 2026-09-01 | Open and migration use the SQLite store; the first read-modify-write shown uses `with_conn_fenced`. The file contains additional fenced writes and unfenced reads. |
| The host source's module store (`host`) | Commit-pinned source read, 2026-09-01 | The store has many uses of both callback APIs. Unfenced durable mutations include a delete and a block of inserts; other write paths are fenced. Removing unrestricted SQLite access breaks this default branch. |
| PostgreSQL consumers | A code search of the source organization for the PostgreSQL store type, run 2026-09-01, not commit-pinned | The result showed no downstream `PostgresStore` or `with_client` use. A live search reflects the default branches at query time and no snapshot of the result set is stored, so the no-downstream-use conclusion is unverified until a pinned snapshot (result list with each repository's commit) exists. The PostgreSQL backend in the source (`primitives@89abb40`) exposes read-only and fenced transaction APIs in place of an unrestricted callback, with no known consumer of either. |
| Two further repositories | Repository lookup returned HTTP 404 | Source was unavailable, so this inventory makes no claim about either repository. |

Two external blockers remain. The host source's module store has durable
mutations through `with_conn`, which fails `SQLITE_READONLY` rather than
committing unfenced, so upgrading that consumer requires moving those mutations
to `with_conn_fenced` before the version bump reaches it. Defining the complete
protected write set is a separate owner decision, because `with_conn_unfenced`
can carry the same mutations without a fence check. The credentials store has
no supplied receipt for its real-daemon two-process review. Both blockers stay
open.
