# p6-new-verifiability-pure-function-of-snapshot

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface N1. A proposed obligation: `proposed - converts in [#1171](https://github.com/ahrav/eidnara/issues/1171)`.

## Evidence trail

The specification's Property Catalog defines this obligation for N1. [#1171](https://github.com/ahrav/eidnara/issues/1171) lands the code and the check that converts it.

## Failure scenario

Classification reads HEAD or the clock.

## Timing windows and dependencies

See the fault map.

## What a test must construct

Two classifications at one commit sequence with a worktree edit between.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record, read at the branch that adds this part.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.
