# Predecessor-captured vocabulary fixtures

This directory stores fixtures captured once by the predecessor build at a
pinned source commit. The destination build never regenerates these files.

| File | Producer | Wave |
| --- | --- | --- |
| `direct-format-vocabulary-v1.json` | copied verbatim from `magic-context/packages/plugin/src/features/magic-context/fixtures/direct-format-vocabulary-v1.json` | U4 |
| `kernel-format-vocabulary-v1.json` | captured from the predecessor build: exact DDL text, trigger bodies, indexes, registration order, dependency lists, provided-object lists, digest domains, `application_id`, `user_version`, `mc_format_marker` shape, incarnation format, `sqlite_schema` inventory for `core.sqlite` and `mc-store.db` | U4 |

Each receipt entry assigns these files the `predecessor-captured` class and
records the capture commit and command. Rust and TypeScript tests read the same
files.
