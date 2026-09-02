# Captured vocabulary fixtures

This directory stores vocabulary fixtures captured once from the `host` source
at its pinned commit. The build never regenerates these files; a change to them
is a reviewed contract change.

| File | Content | Wave |
| --- | --- | --- |
| `direct-format-vocabulary-v1.json` | the direct-format vocabulary, copied verbatim from the `host` source at the pinned commit | U4 |
| `kernel-format-vocabulary-v1.json` | captured from a build of the `host` source at the pinned commit: exact DDL text, trigger bodies, indexes, registration order, dependency lists, provided-object lists, digest domains, the registered Eidnara application id, `user_version`, the format-marker row shape, the incarnation format, and the `sqlite_schema` inventory of the kernel store and the module store | U4 |

Each receipt entry records these files under the captured-fixture class: the
transformation is `verbatim`, and the entry carries the capture commit and the
capture command. Rust and TypeScript tests read the same files.
