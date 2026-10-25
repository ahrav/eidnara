# cap-declaration-canonical-and-total

## Discovery trigger
Parent specification #1119 C3 and Part B record
`cap-declaration-canonical-and-total`; implemented by #1121.

## Evidence trail
- `crates/eval-core/src/capture.rs:29` `CaptureDeclaration` with
  `deny_unknown_fields`; `CaptureSwitch` and `WriteKeyScheme` are closed
  `snake_case` enums.
- `crates/eval-core/src/manifest.rs:365` `parse_capture_declaration`: decode,
  then compare the input's canonical bytes with the decoded value's; a
  mismatch is `MalformedCapture`.
- `crates/eval-core/tests/manifest.rs::a_capture_declaration_is_closed_total_and_canonical`
  pins the bytes
  `{"memory_capture":"on","source_capture":"off","write_key":"task_arm"}` and
  refuses seven malformed inputs.
- `crates/eval-core/tests/statistics.rs::a_family_document_declaration_is_closed`
  refuses `On` and a missing `capture` in a family document.

## Failure scenario
serde_json's derived struct decoder accepts an array, and its enum decoder
accepts `{"on": null}`; without the byte comparison, one declaration would
enter `eval_run_id` under several spellings and split one comparable run set
into several identities.

## Timing windows and dependencies
None.

## What a test must construct
Inputs with reordered keys, an unknown variant, `write_key: "pair"`, a missing
`memory_capture`, and the decoder's array and map spellings.

Legal witness:
`{"memory_capture":"on","source_capture":"off","write_key":"task_arm"}` parses
and encodes to those bytes. Smallest falsifier: `"On"` admitted, or two
spellings of one declaration producing two run ids.

## Investigation log
### Q: Does key order need its own test?
- Sources examined: the canonical encoder in `context-core`
- Findings: serde_json is built without `preserve_order`, so a parsed object
  is key-sorted already, and the canonical encoder sorts keys
- Missing evidence: none.
- Conclusion: resolved with answer - the pinned bytes carry the property
