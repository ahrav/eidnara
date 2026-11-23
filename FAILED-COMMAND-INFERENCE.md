# Failed-command inference (parked)

This branch keeps the code that reads a command's failure from shell text. Pull request #1232 records a
failed command from the harness's error mark alone; the code here was cut from it and waits for a change
with its own evidence and review. The branch head is #1232 at 1c9cfcb4d plus this file.

## What the code reads

All of it is in `crates/daemon/src/memory_capture/failures.rs`, with its contract in section 7.9 of
`docs/host-wire-protocol.md` at this head.

| Reading | Code | Rule |
|---|---|---|
| Status line of an unmarked result | `stated_exit_code`, `outcome_of` | a whole line that states an exit code, in the first line or the last three lines; a runner's tagged line such as `ELIFECYCLE Command failed with exit code 1.` |
| Final labeled echo | `status_echo`, `printed_word`, `echoed_status`, `keeps_shell`, `echoes_unfollowed` | `cmd; echo "label=$?"` read from the last output line |
| Command-line grammar | `commands_of`, `LineReader`, `expansion`, `status_parameter` | words, quotes, redirections, `&&`, `||`, `|`, `;`, line breaks; serves the echo rule and the words a fact shows |
| Reading mark in the digest | `Reading`, `command_named` | a command line's lineage depends on how its unmarked result is read |
| Pi's truncation note | `truncation_note`, `output_log`, the `fragment` of `outcome_of` | the note is read as no line; a line cut at its start states no status and names no path |
| Harness refusals | `harness_decided`, `HARNESS_*` | marked results Pi wrote for a call it ended or refused are skipped by their text |

Commit db2c53699 holds a wider version: failure lines and a shell's missing-program report behind a hidden
status.

## Review state

Six review rounds read this code (rounds 5 to 10 of #1232, multi-model panel per `REVIEW.md`).

| Round | Head | Critical confirmed | High confirmed |
|---|---|---|---|
| 5 | 6cacbbf57 | 3 | 20 |
| 6 | 6148df1ed | 0 | 19 |
| 7 | 1f5105980 | 0 | 22 |
| 8 | 79b8d01dd | 2 | 11 |
| 9 | 7c8f1fdd4 | 0 | 15 |
| 10 | 872f64584 | 0 | 9 |

The five critical findings were paths on which text reached a fact beside the redactor's verdict; each is
fixed here and in #1232. The high findings are almost all cases of the shell reading and of matching the
texts Pi writes. Each round's fixes were followed by new cases on the surface that remained.

### Open: the nine high findings of round 10

Each has a fix in 4f1a18331 that no review round has read, except C11, which is the shared redactor's
behavior and is stated in section 7.9.

| Finding | Location at 872f64584 | Title | State |
|---|---|---|---|
| C11 | `failures.rs:992` | The redactor can rewrite a program word after an empty assignment, merging different commands into one lineage | stated in section 7.9; redactor behavior |
| C34 | `failures.rs:992` | A literal redaction marker in a command collides with a redactor-inserted marker | fix in 4f1a18331, unreviewed |
| C35 | `failures.rs:992` | A result is read under a rule chosen from the raw command line but filed under the redacted line | fix in 4f1a18331, unreviewed |
| C14 | `failures.rs:1156` | Bash's `time` reserved word before exit, exec, trap, source, or eval passes the echo guard (C1 sibling) | fix in 4f1a18331, unreviewed |
| C13 | `failures.rs:1174` | An assignment with an attached redirect and a separate target word hides a later shell-control command (C1 residual) | fix in 4f1a18331, unreviewed |
| C12 | `failures.rs:1416` | A pathless Node spawn error is still recorded as a command failure (C2 partly open) | fix in 4f1a18331, unreviewed |
| C27 | `failures.rs:1494` | The cut-line fragment still supplies the failing path and error name | fix in 4f1a18331, unreviewed |
| C1 | `failures.rs:1518` | Declined final status echo is recognized only when the last command's raw first word is `echo` (C37 partly open) | fix in 4f1a18331, unreviewed |
| C2 | `failures.rs:1524` | The cut-line fragment is removed from the status window instead of counted as a line that states no code (C24 residual) | fix in 4f1a18331, unreviewed |

### Recurring classes a later review should expect

- Echo guard: a command before the final echo that ends or replaces the shell, or runs code the reader
  cannot see (`time exit`, an assignment with an attached redirection, `sh -c`, `eval`).
- Quoting and expansion in the grammar: `$[...]`, `$$?`, braced parameters, here-documents, a backslash and
  line break, `|&`, carriage returns.
- Status lines the command prints itself: the head or the tail of a log, a child's status, a JSON value.
- Pi's texts: the three forms of the truncation note, the spawn refusal with and without a path, refusal
  texts of new Pi releases.
- The digest against redaction: a line the redactor rewrites, a literal redaction mark, two lines the
  redactor reads into one text.

Declined in earlier rounds with a recorded reason: round 6 C3 (a fact id another row holds), round 7 C22 (a
script given to another shell), round 9 C18 (parameter spellings the grammar admits).

## Evidence a later change needs

On Evaluator B (recorded sessions replayed without the harness's error mark, 156 development events) the
code at this head named a past failure correctly in 0.442 of events; the wider version at 6148df1ed scored
0.635 and the first version of #1232, which recorded every failure line, 0.853.

Evaluator B2 replays each result with the error mark its own harness recorded. Of the 156 development
events, 91 results carry the mark, 33 carry a field that says no error (31 of them behind a `|` or a `;`,
2 plain), and 32 come from a format with no field for it. The 31 hidden-status events and the events
without a field are what inference could add over the mark; a change that brings it back should measure
those subsets on B2 and state its wrong-assertion share beside them.
