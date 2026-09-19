export const EIDNARA_NOTE_DESCRIPTION = `Working notes for this session's future — reminders, follow-ups, and things to revisit later.

Use a note when something matters LATER but not in the next few steps: "revisit the retry logic after the release", "user wants the dashboard polish batched", "flaky test to investigate when touching CI". Don't use notes for active multi-step work (use todos) or for durable project knowledge that should outlive this session (use eidnara_memory). Read your notes at natural work boundaries (after a commit, when a todo list completes, before starting the next piece of work); conditional notes also surface on their own once their condition is confirmed.

Actions:
- write: save a note (content). Add surface_condition to make it a conditional note (below).
- read: list notes, newest first. Default: latest active session notes + ready conditional notes; page older ones with limit/offset, or inspect other states with filter.
- update / dismiss: change or retire a note by note_id.

Conditional notes are unsupported unless another host registers a live evaluator. Rust refuses conditioned writes and updates rather than storing an inactive condition. When a scheduled-wake integration is active, conditioned writes become plain notes with scheduling guidance; conditioned updates remain refused.

Example: eidnara_note(action="write", content="Re-run the perf benchmark once the boundary rework ships", surface_condition="When the latest release tag is >= v0.23.0")`;
