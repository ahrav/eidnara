# p6-new-cursor-never-skips-unresolved

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface N7. A proposed obligation: `proposed - converts in [#1173](https://github.com/ahrav/eidnara/issues/1173)`.

## Evidence trail

The specification's Property Catalog defines this obligation for N7. [#1173](https://github.com/ahrav/eidnara/issues/1173) lands the code and the check that converts it.

## Failure scenario

A `deadline` decision passed and never revisited, or a second `MarkStale`.

## Timing windows and dependencies

See the fault map.

## What a test must construct

F7, F13.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record, read at the branch that adds this part.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: What are N and K?

- Sources examined: OQ6-10.
- Findings: Recommended N = 8 and K = 3; not decided.
- Missing evidence: see the conclusion.
- Conclusion: unresolved, needs the #1173 decision on OQ6-10.
