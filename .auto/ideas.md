# Ideas backlog (autoresearch tokenizer)

Untried or deferred; each needs its own iteration with guard + report.

- Cross-call scratch reuse done differently: keep `Scratch` per call but pre-size `parts`
  from the longest piece seen in a first scan pass (avoids the growth memmove on short strings
  without the TLS codegen effect seen in iterations 28-29).
- Heap engine: skip the pair-rank lookup for the left neighbour when its current rank is
  already below the just-merged rank (it cannot be selected before the recomputed one anyway
  only if the recomputed span is a superset; needs a proof or differential test).
- `piece_end`: for `' ?\p{L}+'` pieces after a space, the run-end SWAR could start at `pos+1`
  with the 8-byte word already loaded for the space test.
- AVX2 (runtime-dispatched) 32-byte class scan only inside the heap engine's long-run path
  (iteration 23 showed SIMD hurts on the short-piece common case).
- Real-text validation of any piece cache before considering it (iteration 30): the corpus's
  ~600-word vocabulary inflates hit rates.
