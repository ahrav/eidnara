# Capacity model results

Uncalibrated sizing input. Not performance evidence. Inputs are invented, not
measured traffic or a production SLO.

Provenance: `simulate.py` in this directory is byte-identical to the settled
plan model (`docs/plans/independent-payload-pools/simulate.py`, SHA-256
`372e5be3660de581be96823d28ce0a7e13b39354ff64250a9d5a0528634d6768`); the copy
here hashes to `372e5be3660de581be96823d28ce0a7e13b39354ff64250a9d5a0528634d6768`. Run with `Python 3.9.25` by
`python3 docs/properties/independent-payload-pools/capacity-model/simulate.py`.

The first output line was `Self-checks passed. Scenario results, not measured performance:`.

Refusal buckets are disjoint: class first when both are exhausted, otherwise
descriptor; `both_exhausted` is a diagnostic subset. For every row below,
`descriptor_refusals + sum(class_refusals) == would_block` and
`unfinished_after_drain == 0`, checked when this file was generated.

| Scenario | Offered | Admitted | Would block | Descriptor refusals | Class refusals | Both exhausted | Held at 2 s | Peak descriptors | Backing MiB |
| --- | ---: | ---: | ---: | ---: | --- | ---: | ---: | ---: | ---: |
| `base` | 2001 | 2001 | 0 | 0 | [0, 0, 0, 0, 0] | 0 | 11 | 2 | 89.254 |
| `fifo-isolation` | 2001 | 91 | 1910 | 0 | [1736, 144, 12, 18, 0] | 0 | 91 | 2 | 89.254 |
| `rate-5000` | 10001 | 9935 | 66 | 0 | [0, 0, 0, 66, 0] | 0 | 13 | 2 | 89.254 |
| `large-count-8` | 10001 | 10001 | 0 | 0 | [0, 0, 0, 0, 0] | 0 | 16 | 2 | 137.254 |
| `holds-10x` | 2001 | 1985 | 16 | 0 | [0, 0, 0, 16, 0] | 0 | 24 | 2 | 89.254 |
| `burst-64-depth-8` | 2001 | 256 | 1745 | 1745 | [0, 0, 0, 0, 0] | 0 | 1 | 8 | 89.254 |
| `burst-64-depth-32` | 2001 | 1008 | 993 | 993 | [0, 0, 0, 0, 0] | 0 | 3 | 32 | 89.254 |
| `burst-64-depth-128` | 2001 | 2001 | 0 | 0 | [0, 0, 0, 0, 0] | 0 | 3 | 65 | 89.254 |
| `accept-10ms` | 10001 | 6400 | 3601 | 3601 | [0, 0, 0, 0, 0] | 0 | 33 | 32 | 89.254 |
| `return-10ms` | 2001 | 2001 | 0 | 0 | [0, 0, 0, 0, 0] | 0 | 14 | 2 | 89.254 |

What the model excludes: CPU, bandwidth, SQLite, JavaScript GC, cancellation,
wakes, quarantine, reserves, and retries. FIFO mode isolates reclamation order
with identical classes, not the old ring geometry. Burst rows are sensitive to
arrival ordering and immediate-refusal semantics; they do not justify shipping
depth 128. The selected inventory (32 ordinary descriptors, 16 reserved, the
class table in `docs/payload-pool-protocol.md`) follows the zero-base-refusal
and complete-drain rule; the six extra 8 MiB blocks that remove the 5,000/s
refusals are deferred with every other tuning decision.
