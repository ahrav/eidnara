# `epoch-update-interruption-window-is-reached`

- **Discovery:** coverage/vacuity evaluation of process-interruption behavior.
- **Primary evidence:** interruption can occur during marker padding or canonical overwrite in `persist_epoch`.
- **Existing evidence:** `interrupted_persist_never_leaves_a_lower_parseable_epoch` injects each ordered prefix-write failure, but no process termination.
- **Coverage condition:** process termination occurs after update progress and before the canonical write completes.
- **Why independent:** a correct implementation could reach the precondition and recover safely; the witness is not epoch regression itself.
- **Timing need:** deterministic process coordination; random kills are poor evidence for the short update sequence.
- **Instrumentation:** the injected short writer covers ordered prefix writes only, not returned `File` errors or process termination.
- **Open questions:** none.
