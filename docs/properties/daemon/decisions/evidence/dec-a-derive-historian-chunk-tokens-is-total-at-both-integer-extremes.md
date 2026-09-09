# dec-a-derive-historian-chunk-tokens-is-total-at-both-integer-extremes

## Discovery trigger

The chunk-budget derivation converts an integer to a float, scales and rounds it,
then converts back to an integer and clamps. The property retains its whole-domain
range guarantee. Source revision: `74044960ee91641dec95c8552f15282844a18b13`.
Rust paths below are relative to `crates/daemon/src/`.

## Evidence trail

`derive_historian_chunk_tokens` computes one quarter of the context limit, rounds
to a token, casts to `usize`, and clamps the integer (`config.rs:39-46`). The
bounds are `8_000` and `50_000` (`config.rs:28-29`). Their fixed ordering makes
the integer clamp well-defined.

Zero and one reach the lower clamp. `usize::MAX` reaches the upper clamp. On
the 64-bit target, converting the maximum input to `f64` rounds it to `2^64`;
quartering gives `2^62`, which is still within `usize`. The result becomes
`50_000` because of the integer clamp, not because this input saturates the
float-to-integer cast. The former explanation confused the cast's general
semantics with the arithmetic of this function.

`historian_budget_derivation_clamps_at_both_bounds` (`config.rs:1449-1455`)
asserts inputs `1`, `32_000`, `128_000`, `200_000`, and `400_000`, covering both
clamps and an interior result. It does not assert zero or `usize::MAX`.
Status: `unaudited`.

The user-only context limit defaults to `128_000` (`config.rs:32`,
`config.rs:122`, `config.rs:668`). The parser uses `positive_usize_at`
(`config.rs:885-889`, `config.rs:955-961`), so a configured zero is discarded.
The reattach chunk builder and the normal and wrapup firing assemblers all pass
that effective limit to the derivation (`lib.rs:4708-4714`, `lib.rs:5114-5119`,
`lib.rs:5270-5276`).

## Failure scenario

No range violation is identified in the derivation. The regression contract
prevents a changed calculation from producing an out-of-range chunk budget.
A very large valid user limit yields the maximum chunk budget rather than a
small wrapped value.

## Timing windows and dependencies

The function is pure. Reachability is `default-production` because the normal
firing path derives a budget from the default context limit. The zero input
needs a direct call; a maximum-size input can be supplied directly or through
a representable positive user integer.

## What a test must construct

Extend the existing boundary check with zero and `usize::MAX`, or check the
range for arbitrary `usize` inputs. Assert `[8_000, 50_000]` independently of
the implementation expression. No file, timer, or process fixture is necessary.

## Investigation log

### Q: Does the maximum input exercise a saturating cast?

- Sources examined: `config.rs:28-29`, `config.rs:39-46`, and
  `config.rs:1449-1455`.
- Findings: quartering the maximum input leaves a representable positive integer;
  the final integer clamp supplies the upper bound.
- Missing evidence: the existing test omits the two integer extremes.
- Conclusion: resolved with answer. Retain the totality property and correct its
  proof; do not claim cast saturation that this input does not exercise.

### Historical investigation

The [pre-refresh evidence](https://github.com/ahrav/eidnara/blob/74044960ee91641dec95c8552f15282844a18b13/docs/properties/daemon/decisions/evidence/dec-a-derive-historian-chunk-tokens-is-total-at-both-integer-extremes.md)
contains the original investigation. Its source coordinates and explanation of
the maximum-input cast do not support the current confidence claim.
