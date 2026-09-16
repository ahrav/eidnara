# Resumable claim-read measurements

The collector runs one block per invocation, not a whole campaign inside one
outer tool timeout. A block has four fresh processes. With a 90-second attempt
limit, allow at least 400 seconds for the block command, including cleanup.

```sh
python3 -m unittest discover -s scripts -p 'test_bench_claim_read.py' -v
python3 scripts/bench_claim_read.py /absolute/evidence/plan.json 0
python3 scripts/bench_claim_read.py /absolute/evidence/plan.json 1
```

Repeating a completed block verifies its plan and evidence hashes without
rerunning measurements. A partially completed block resumes only positions that
never started. Failed or unresolved attempts stop the command; no result is
silently overwritten or replaced. During capture, SIGTERM, keyboard interruption,
and attempt timeout signal the child process group, reap the direct child, and
save a failed receipt. Escalation follows the group leader's lifetime; descendants
that outlive the leader are not guaranteed to stop. This collector targets the
single-process claim-read executable, not arbitrary process trees.
SIGKILL cannot be handled: a running receipt remains unresolved and needs manual
reconciliation. Do not treat its partial samples as a successful attempt.

Receipts are atomically replaced and fsynced at lifecycle boundaries. Affinity,
CPU-tick/page-fault samples, and timestamped stderr progress are flushed during
the run. This preserves process-interruption evidence; it is not a power-loss
protocol. A lock prevents two collectors from writing the same evidence root.

Create the evidence directory with mode 0700. Full environment records are mode
0600 and may contain credentials: keep them local and do not publish them. Raw
Criterion output, stdout/stderr, affinity/progress logs, plan hash, binary hash,
exit state, and checksums remain under each position's directory.

## Plan shape

Paths must be absolute. `collector_sha256` pins this script. Each artifact is
resolved from Cargo JSON and copied to a stable location before the plan is
frozen. Both builds must share compiler, feature, profile, and workload settings.

```json
{
  "collector_sha256": "<SHA256 of scripts/bench_claim_read.py>",
  "cpu": 24,
  "timeout_seconds": 90,
  "cwd": "/absolute/worktree/crates/retrieval",
  "artifacts": {
    "A": {"path": "/absolute/evidence/A", "sha256": "<SHA256>"},
    "B": {"path": "/absolute/evidence/B", "sha256": "<SHA256>"}
  },
  "blocks": ["ABBA", "BAAB"],
  "argv": [
    "^claim_read/(classify/(large|small|tombstone_history|large_payload|empty)|row_overflow|object_overflow)$",
    "--warm-up-time", "1", "--measurement-time", "1",
    "--sample-size", "10", "--nresamples", "1000", "--noplot", "--bench"
  ],
  "benchmark_ids": [
    "claim_read/classify/large", "claim_read/classify/small",
    "claim_read/classify/tombstone_history", "claim_read/classify/large_payload",
    "claim_read/classify/empty", "claim_read/row_overflow", "claim_read/object_overflow"
  ],
  "sample_count": 10,
  "fixture_lines": [
    "fixture=empty objects=0 rows=0",
    "fixture=small objects=16 rows=48 history=1 tombstones=0 payload_bytes=256",
    "fixture=large objects=256 rows=768 history=1 tombstones=0 payload_bytes=256",
    "fixture=admission_history objects=16 rows=48 history=8",
    "fixture=tombstone_history objects=16 rows=48 history=1 tombstones=4096",
    "fixture=large_payload objects=16 rows=48 history=1 tombstones=0 payload_bytes=16384"
  ]
}
```

The collector checks CPU permission against both its affinity and the effective
cpuset controller, including inherited controller settings. Every observed
benchmark thread must retain the planned mask. Pinning is not CPU isolation.
`RAYON_NUM_THREADS=1` applies equally to both arms. The collector checks sample
counts, finite positive values, exact benchmark IDs, and all fixture markers.
Its per-process mean is total measured time divided by total iterations.

## Experiment design is separate

Freeze the workload, estimand, block count/order, controls, invalid-attempt rules,
confidence construction, and local decision rule before collecting candidate
data. This script executes blocks; it does not invent a sample size or interpret
Criterion labels as a merge gate. The example has two blocks only to show syntax,
not to prescribe sufficient replication.

The earlier experiment's collector timed out after eight of nine blocks. Its
largest empty-read contrast came from one process whose samples were all around
53–59 microseconds, versus 36 microseconds for the same candidate binary in the
next process. That identifies process/window variation, not a proven code or
scheduler cause. The new CPU-tick/fault and phase logs let a future anomalous
window be examined instead of discarding it. Old timings are not comparable to
current main: the merged claim reader added identity, checkpoint, incarnation,
and deadline checks.
