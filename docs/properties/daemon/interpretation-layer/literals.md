# Literal classification

Every field, tool, and argument-key string literal in the production source of
the interpretation layer, with one class per row. The record
[`il-literal-classified-contract-or-vocabulary`](catalog.md#il-literal-classified-contract-or-vocabulary)
states the rule; this file is its artifact.

## Scope and extraction

The nine files of [the catalog's scope](catalog.md#scope), all under
`crates/daemon/src/`: `tool_vocabulary.rs`, `selection.rs`, `chunk_text.rs`,
`wire.rs`, `tool_outcome.rs`, `injection.rs`, `history_summarizer_chunk.rs`,
`codec/opencode.rs`, and `codec/pi.rs`.

Production source is every line outside a top-level `#[cfg(test)]` module
region (the rule `scripts/forbid-tool-literals.sh` applies) and outside a
top-level `#[cfg(test)]` item. Comment lines are skipped. A literal is one
Rust string literal on one line; no production string literal in these files
spans lines. Code references are verified at `41d889ea4`.

The extraction finds 256 distinct string literals at 895 occurrences.
156 of them name a field, a tool, an argument key, or a discriminator
value; they fill the 167 rows below (39 vocabulary, 34 Eidnara-owned,
94 contract) and cover 755 occurrences. The other 100 are listed under
[Excluded literals](#excluded-literals) with the reason each is outside the rule.

## Classes

- **contract**: a harness with zero tools registered still emits the field:
  message and part shapes, role and type discriminators, status fields,
  identity fields, and settled timestamps. Contract literals stay with the
  codec or reader that parses them.
- **vocabulary**: correctness depends on a choice a tool author made outside
  this repository: tool names, argument keys, and tool-authored metadata such
  as `state.metadata.exit`. Vocabulary literals belong in `tool_vocabulary.rs`.
- **Eidnara-owned**: Eidnara defines the spelling itself: its reduction kinds,
  transcript tags, reserved id prefixes, block kind tags, and the keys of its
  own sidecar and reduced-call representations.

Where Eidnara's own media-source object reuses a harness spelling (`type`,
`url`, `data`, `mime`, `filename`), the row takes the harness's class,
contract, because the spelling originates in the harness record.

One row is one literal under one class and one meaning. Ten spellings carry
more than one meaning at different sites and so have more than one row:
`action`, `cancelled`, `completed`, `file`, `image`, `path`, `read`,
`status`, and `summary` have two rows each, and `content` has three (a diff
key and a `todowrite` item key, both vocabulary, and the contract message
field). Every occurrence of every in-scope literal appears in exactly one
row.

## Distinctive vocabulary and the gate

A vocabulary literal is distinctive when its spelling is not a generic word:
a compound joined by an underscore or a camelCase hump, the closed compound
`todowrite`, or the `mcp_` prefix. A single word (`edit`, `bash`, `path`,
`exit`) and a JSON pointer built from single words (`/metadata/truncated`)
are generic: prose, shell text, and contract fields use the same tokens, so a
token-level scan cannot tell them apart. The plan states the same rule as
"generic words are not gated" (KTD4).

The distinctive subset of the vocabulary rows is `aft_outline`, `aft_search`,
`aft_zoom`, `apply_patch`, `bash_kill`, `bash_status`, `filePath`, `file_path`,
`mcp_`, `newString`, `new_string`, `oldString`, `old_string`, and `todowrite`.
It equals the literal set of `PATTERN` in `scripts/forbid-tool-literals.sh:16`
(thirteen whole literals plus the `mcp_` prefix).

## Vocabulary outside the module

Every distinctive vocabulary literal is in `tool_vocabulary.rs`. Two groups of
generic vocabulary literals are not:

- `injection.rs:21-24`, `:407-409`, `:465` read the `todowrite` input schema:
  `todos`, `content`, `status`, `priority`, the status values `completed` and
  `cancelled`, and the default priority `medium`. The tool author chose these
  keys; a harness with no to-do tool never emits them.
- `history_summarizer_chunk.rs:1292` reads the `description` argument key.

The gate does not see them because they are generic words. The record
[`il-vocabulary-literals-live-in-one-module`](catalog.md#il-vocabulary-literals-live-in-one-module)
carries the finding.

## Eidnara-owned identifiers defined more than once

`eidnara_reduce` and `eidnara_note` are defined once, at
`prompt_surface.rs:65` and `:67`, and `tool_vocabulary.rs:23` imports them;
neither is a literal in the nine files. These Eidnara-owned spellings are
not defined once:

- `synthetic_todo_` at `injection.rs:20` and `codec/opencode.rs:1074`.
- The `eidnara_note` actions `read` and `dismiss` at `tool_vocabulary.rs:37`,
  beside the note handler's own spellings at `lib.rs:12855`, `:13172` and the
  tool schema at `lib.rs:18610`, and `conditional_note_evaluation.rs:1905`.

## Rows

| Literal | Class | Meaning | Distinctive | Locations |
| --- | --- | --- | --- | --- |
| `/details/truncation/truncated` | vocabulary | pointer to the Pi tool-authored truncation flag | no | `tool_vocabulary.rs:215` |
| `/metadata/truncated` | vocabulary | pointer to the OpenCode truncation flag under `state` | no | `tool_vocabulary.rs:214` |
| `action` | vocabulary | display key shown whole | no | `tool_vocabulary.rs:45` |
| `aft_outline` | vocabulary | tool name in a tool-vocabulary table | yes | `tool_vocabulary.rs:31` |
| `aft_search` | vocabulary | tool name in a tool-vocabulary table | yes | `tool_vocabulary.rs:33` |
| `aft_zoom` | vocabulary | tool name in a tool-vocabulary table | yes | `tool_vocabulary.rs:31` |
| `apply_patch` | vocabulary | tool name in a tool-vocabulary table | yes | `tool_vocabulary.rs:33` |
| `bash` | vocabulary | tool name in a tool-vocabulary table | no | `tool_vocabulary.rs:35` |
| `bash_kill` | vocabulary | tool name in a tool-vocabulary table | yes | `tool_vocabulary.rs:29` |
| `bash_status` | vocabulary | tool name in a tool-vocabulary table | yes | `tool_vocabulary.rs:29` |
| `cancelled` | vocabulary | `todowrite` item status value | no | `injection.rs:23` |
| `completed` | vocabulary | `todowrite` item status value | no | `injection.rs:22, 23, 24` |
| `content` | vocabulary | `todowrite` item key the injection read-back parses | no | `injection.rs:407` |
| `content` | vocabulary | diff key | no | `tool_vocabulary.rs:50` |
| `description` | vocabulary | argument key the summary line prefers | no | `history_summarizer_chunk.rs:1292` |
| `edit` | vocabulary | tool name in a tool-vocabulary table | no | `tool_vocabulary.rs:28, 33` |
| `exit` | vocabulary | tool-authored `state.metadata.exit` key | no | `tool_vocabulary.rs:203` |
| `file_path` | vocabulary | path key | yes | `tool_vocabulary.rs:41` |
| `filePath` | vocabulary | path and display key | yes | `tool_vocabulary.rs:41, 43` |
| `glob` | vocabulary | tool name in a tool-vocabulary table | no | `tool_vocabulary.rs:33` |
| `grep` | vocabulary | tool name in a tool-vocabulary table | no | `tool_vocabulary.rs:33` |
| `mcp_` | vocabulary | MCP name prefix the normalization strips | yes | `tool_vocabulary.rs:55` |
| `medium` | vocabulary | `todowrite` default priority value | no | `injection.rs:21` |
| `module` | vocabulary | display key shown whole | no | `tool_vocabulary.rs:45` |
| `new_string` | vocabulary | diff key | yes | `tool_vocabulary.rs:52` |
| `newString` | vocabulary | diff key | yes | `tool_vocabulary.rs:49` |
| `old_string` | vocabulary | diff key | yes | `tool_vocabulary.rs:51` |
| `oldString` | vocabulary | diff key | yes | `tool_vocabulary.rs:48` |
| `path` | vocabulary | path and display key | no | `tool_vocabulary.rs:41, 43` |
| `pattern` | vocabulary | display key truncated | no | `tool_vocabulary.rs:43` |
| `priority` | vocabulary | `todowrite` item key | no | `injection.rs:409` |
| `query` | vocabulary | display key truncated | no | `tool_vocabulary.rs:43` |
| `read` | vocabulary | tool name in `T1_TOOLS` | no | `tool_vocabulary.rs:31` |
| `status` | vocabulary | `todowrite` item key | no | `injection.rs:408` |
| `symbol` | vocabulary | display key shown whole | no | `tool_vocabulary.rs:45` |
| `task` | vocabulary | tool name in a tool-vocabulary table | no | `tool_vocabulary.rs:31` |
| `todos` | vocabulary | `todowrite` input key holding the list | no | `injection.rs:465` |
| `todowrite` | vocabulary | tool name in a tool-vocabulary table | yes | `tool_vocabulary.rs:26` |
| `write` | vocabulary | tool name in a tool-vocabulary table | no | `tool_vocabulary.rs:28, 33` |
| `A` | Eidnara-owned | transcript role tag for assistant | - | `chunk_text.rs:173`; `history_summarizer_chunk.rs:286, 305, 345` |
| `action` | Eidnara-owned | `eidnara_note` argument key naming the action | - | `tool_vocabulary.rs:38` |
| `audio` | Eidnara-owned | media-kind label in the summarizer line | - | `history_summarizer_chunk.rs:1221` |
| `data_base64` | Eidnara-owned | media-source type | - | `codec/opencode.rs:637, 642, 1243`; `codec/pi.rs:969, 1840, 1864` |
| `dismiss` | Eidnara-owned | `eidnara_note` action whose call carries no value once read | - | `tool_vocabulary.rs:37` |
| `document` | Eidnara-owned | media-kind label in the summarizer line | - | `history_summarizer_chunk.rs:1224` |
| `drop` | Eidnara-owned | reduction kind | - | `selection.rs:65, 1125` |
| `edit_marker` | Eidnara-owned | reduction kind | - | `selection.rs:67, 1126` |
| `eidnara:` | Eidnara-owned | reserved prefix of a Pi row id Eidnara writes | - | `codec/pi.rs:38` |
| `file` | Eidnara-owned | media-kind label in the summarizer line | - | `history_summarizer_chunk.rs:1223` |
| `harness` | Eidnara-owned | media-source type and key | - | `codec/opencode.rs:692`; `codec/pi.rs:951, 952, 1735, 1743` |
| `image` | Eidnara-owned | media-kind label in the summarizer line | - | `history_summarizer_chunk.rs:1220` |
| `itemId` | Eidnara-owned | extras key holding the split item id of a Pi call id | - | `codec/pi.rs:857, 883, 1137, 1642, 1664` |
| `M` | Eidnara-owned | transcript role tag for any other role | - | `chunk_text.rs:180` |
| `nativeToolCallId` | Eidnara-owned | extras key holding the split native call id of a Pi call id | - | `codec/pi.rs:859, 883, 1140, 1635, 1668` |
| `opaque` | Eidnara-owned | media-source type | - | `codec/opencode.rs:646`; `codec/pi.rs:973, 1844` |
| `opencode` | Eidnara-owned | harness identifier the codec writes into a media source | - | `codec/opencode.rs:29` |
| `path` | Eidnara-owned | key of the reduced call input the wire writes | - | `wire.rs:488` |
| `pi` | Eidnara-owned | harness identifier the codec writes into a media source | - | `codec/pi.rs:34` |
| `placeholder:` | Eidnara-owned | reserved prefix of a placeholder run id | - | `history_summarizer_chunk.rs:743` |
| `raw` | Eidnara-owned | media-source key holding the native part | - | `codec/opencode.rs:646`; `codec/pi.rs:973, 1844` |
| `rawAttachment` | Eidnara-owned | extras key holding the native attachment | - | `codec/opencode.rs:678, 1313` |
| `rawResultPart` | Eidnara-owned | extras key holding the native result part | - | `codec/pi.rs:942, 1725, 1785` |
| `read` | Eidnara-owned | `eidnara_note` action whose call carries no value once read | - | `tool_vocabulary.rs:37` |
| `reduced` | Eidnara-owned | key of the reduced call input | - | `wire.rs:485` |
| `skeleton` | Eidnara-owned | reduction kind | - | `selection.rs:66, 1127` |
| `summary` | Eidnara-owned | key of the reduced call input | - | `wire.rs:486` |
| `synthetic_todo_` | Eidnara-owned | reserved prefix of a synthetic to-do call id | - | `injection.rs:20`; `codec/opencode.rs:1074` |
| `syntheticTodoMarker` | Eidnara-owned | extras key marking a synthetic to-do part | - | `codec/opencode.rs:1085` |
| `tool_call` | Eidnara-owned | CK block kind tag | - | `history_summarizer_chunk.rs:398`; `codec/opencode.rs:510, 962`; `codec/pi.rs:609, 626, 1104` |
| `tool_result` | Eidnara-owned | CK block kind tag | - | `history_summarizer_chunk.rs:399`; `codec/opencode.rs:529, 965`; `codec/pi.rs:630, 1153` |
| `U` | Eidnara-owned | transcript role tag for user | - | `chunk_text.rs:174` |
| `unknown` | Eidnara-owned | kind label of an opaque block | - | `codec/opencode.rs:178`; `codec/pi.rs:932, 1034, 1035, 1115, 1116, 1744` |
| `video` | Eidnara-owned | media-kind label in the summarizer line | - | `history_summarizer_chunk.rs:1222` |
| `agent` | contract | OpenCode part type | - | `codec/opencode.rs:238` |
| `api` | contract | Pi assistant field | - | `codec/pi.rs:1521, 1613` |
| `args` | contract | call input field | - | `codec/opencode.rs:487` |
| `arguments` | contract | Pi call field | - | `codec/pi.rs:736, 1087, 1276, 1478, 1590` |
| `assistant` | contract | role value | - | `chunk_text.rs:28, 173`; `wire.rs:344, 397, 607, 619`; `injection.rs:155`; `codec/opencode.rs:1037, 1058, 1148`; `codec/pi.rs:77, 90, 110, 1512, 1519` |
| `attachments` | contract | OpenCode tool state field | - | `codec/opencode.rs:652, 653, 893, 1346, 1348` |
| `bashExecution` | contract | Pi role value | - | `codec/pi.rs:79, 92` |
| `branchSummary` | contract | Pi role value | - | `codec/pi.rs:81, 94` |
| `cache_control` | contract | provider metadata key | - | `codec/opencode.rs:589` |
| `cacheRead` | contract | Pi usage field | - | `codec/pi.rs:1527, 1530` |
| `cacheWrite` | contract | Pi usage field | - | `codec/pi.rs:1528, 1530` |
| `callID` | contract | OpenCode call id field | - | `codec/opencode.rs:454, 490, 555, 890, 892, 1009, 1023, 1082, 1178, 1199` |
| `callId` | contract | call id field | - | `codec/opencode.rs:490`; `codec/pi.rs:735, 1089` |
| `cancelled` | contract | Pi `bashExecution` field | - | `codec/pi.rs:331` |
| `command` | contract | Pi `bashExecution` field | - | `codec/pi.rs:328` |
| `compaction` | contract | OpenCode part type | - | `codec/opencode.rs:228, 835` |
| `compactionSummary` | contract | Pi role value | - | `codec/pi.rs:82, 95` |
| `completed` | contract | OpenCode tool status value | - | `tool_outcome.rs:27`; `codec/opencode.rs:514, 1265, 1266, 1287` |
| `content` | contract | message content field | - | `codec/pi.rs:459, 464, 466, 467, 471, 492, 493, 520, 584, 585, 689, 747, 1012, 1050, 1219, 1224, 1227, 1238, 1316, 1354, 1396, 1434, 1503, 1520, 1536, 1690` |
| `cost` | contract | Pi usage field | - | `codec/pi.rs:1530` |
| `created` | contract | OpenCode time field | - | `codec/opencode.rs:126` |
| `custom` | contract | Pi role value | - | `codec/pi.rs:80, 93` |
| `data` | contract | part data field | - | `codec/opencode.rs:210, 636, 637, 642, 1244, 1326, 1357`; `codec/pi.rs:741, 969, 1805, 1839, 1840, 1865, 1866` |
| `end` | contract | OpenCode tool time field | - | `codec/opencode.rs:1229` |
| `error` | contract | OpenCode status value or error field | - | `tool_outcome.rs:28`; `codec/opencode.rs:267, 514, 517, 518, 522, 893, 1267, 1268, 1270, 1285, 1339, 1340`; `codec/pi.rs:219` |
| `excludeFromContext` | contract | Pi `bashExecution` field | - | `codec/pi.rs:265` |
| `exitCode` | contract | Pi `bashExecution` field | - | `codec/pi.rs:330` |
| `file` | contract | part or content item type | - | `codec/opencode.rs:219, 222, 683, 968, 1235`; `codec/pi.rs:927, 930, 1730, 1857` |
| `filename` | contract | file part field | - | `codec/opencode.rs:625, 1239, 1326`; `codec/pi.rs:743, 1805, 1849, 1860` |
| `finish` | contract | OpenCode finish field | - | `codec/opencode.rs:105, 268` |
| `fullOutputPath` | contract | Pi `bashExecution` field | - | `codec/pi.rs:337` |
| `id` | contract | identity field | - | `codec/opencode.rs:157, 158, 490, 555, 851, 852, 861, 936, 1102, 1142, 1152`; `codec/pi.rs:169, 299, 734, 1088, 1178, 1251, 1252, 1470, 1474, 1556, 1588` |
| `ignored` | contract | OpenCode text part flag | - | `codec/opencode.rs:180` |
| `image` | contract | part or content item type | - | `codec/opencode.rs:219, 683`; `codec/pi.rs:599, 633, 779, 927, 930, 1025, 1027, 1730, 1855` |
| `info` | contract | OpenCode message info field | - | `codec/opencode.rs:101, 111, 398, 847, 848, 850, 861, 931, 1154` |
| `input` | contract | call input field or Pi usage field | - | `codec/opencode.rs:485, 486, 890, 1011, 1182, 1202`; `codec/pi.rs:1525, 1530` |
| `interrupted` | contract | OpenCode abort flag in tool metadata | - | `tool_outcome.rs:55` |
| `isError` | contract | Pi tool result field | - | `tool_outcome.rs:63`; `codec/pi.rs:746, 1131, 1431` |
| `message` | contract | Pi entry field | - | `codec/pi.rs:300, 499, 587, 1208, 1244, 1556` |
| `metadata` | contract | OpenCode tool or part metadata container | - | `tool_outcome.rs:29, 52`; `codec/opencode.rs:201, 498, 589, 981, 983, 989, 992, 998, 999, 1013, 1163, 1168, 1187, 1360` |
| `mime` | contract | file part field | - | `codec/opencode.rs:622, 679, 1236, 1326`; `codec/pi.rs:740, 1805, 1837` |
| `mimeType` | contract | file part field | - | `codec/opencode.rs:623, 680, 1326`; `codec/pi.rs:739, 1805, 1836, 1855` |
| `model` | contract | model field | - | `codec/opencode.rs:605, 611, 613`; `codec/pi.rs:1523, 1615` |
| `modelID` | contract | OpenCode model field | - | `codec/opencode.rs:610, 612` |
| `name` | contract | error name or file part name field | - | `tool_outcome.rs:44`; `codec/opencode.rs:625, 1326, 1381`; `codec/pi.rs:738, 1477, 1589, 1885` |
| `output` | contract | tool output field or Pi usage field | - | `codec/opencode.rs:517, 518, 893, 1339, 1340`; `codec/pi.rs:329, 1526, 1530` |
| `parts` | contract | OpenCode parts field | - | `codec/opencode.rs:167, 399, 410, 447, 757, 843, 846, 862, 935, 1154` |
| `patch` | contract | OpenCode part type | - | `codec/opencode.rs:238` |
| `provider` | contract | model provider field | - | `codec/opencode.rs:607, 609`; `codec/pi.rs:1522, 1614` |
| `providerExecuted` | contract | OpenCode metadata flag | - | `codec/opencode.rs:499, 1013, 1187` |
| `providerID` | contract | OpenCode model field | - | `codec/opencode.rs:606, 608` |
| `reasoning` | contract | part type | - | `codec/opencode.rs:188, 207, 955, 957, 959, 987, 996, 1161, 1168`; `codec/pi.rs:604, 621, 623, 624, 1083` |
| `redacted` | contract | redacted reasoning field | - | `codec/opencode.rs:211, 999, 1168, 1358, 1361`; `codec/pi.rs:733, 1063, 1465, 1581` |
| `redacted_reasoning` | contract | part type | - | `codec/opencode.rs:214, 955, 959`; `codec/pi.rs:602, 621, 624, 1077` |
| `redacted_thinking` | contract | content item type | - | `codec/opencode.rs:209` |
| `retry` | contract | OpenCode part type | - | `codec/opencode.rs:238` |
| `role` | contract | role field | - | `codec/opencode.rs:163, 164, 854, 855, 861, 938, 1090, 1134, 1152`; `codec/pi.rs:312, 1237, 1417, 1503, 1519, 1536` |
| `running` | contract | OpenCode tool status value | - | `codec/opencode.rs:1181` |
| `sessionID` | contract | OpenCode session field | - | `codec/opencode.rs:1094, 1138` |
| `signature` | contract | reasoning signature field | - | `codec/opencode.rs:203, 992, 1163, 1368` |
| `snapshot` | contract | OpenCode part type | - | `codec/opencode.rs:238` |
| `start` | contract | OpenCode tool time field | - | `codec/opencode.rs:1228` |
| `state` | contract | OpenCode tool part field | - | `tool_outcome.rs:25`; `codec/opencode.rs:484, 516, 651, 904, 1011, 1084, 1180, 1201, 1222, 1338, 1341, 1342, 1346, 1348, 1386` |
| `status` | contract | tool state or Pi result status field | - | `tool_outcome.rs:26`; `codec/opencode.rs:893, 1181, 1338, 1387, 1388` |
| `step-finish` | contract | OpenCode part type | - | `codec/opencode.rs:224` |
| `step-start` | contract | OpenCode part type | - | `codec/opencode.rs:224` |
| `stop` | contract | OpenCode finish value or Pi stop reason value | - | `codec/opencode.rs:105`; `codec/pi.rs:1516` |
| `stopReason` | contract | Pi message field | - | `codec/pi.rs:219, 1532` |
| `subtask` | contract | OpenCode part type | - | `codec/opencode.rs:224` |
| `summary` | contract | OpenCode `info.summary` flag or Pi summary field | - | `codec/opencode.rs:104, 266`; `codec/pi.rs:274, 284` |
| `synthetic` | contract | OpenCode part flag | - | `codec/opencode.rs:1130` |
| `system` | contract | role value | - | `history_summarizer_chunk.rs:530, 899, 1149` |
| `text` | contract | part type or text field | - | `codec/opencode.rs:180, 182, 186, 189, 889, 953, 976, 977, 988, 997, 1159, 1161, 1168`; `codec/pi.rs:598, 620, 729, 753, 778, 793, 911, 924, 930, 1017, 1018, 1023, 1055, 1056, 1059, 1163, 1275, 1327, 1443, 1444, 1562, 1603, 1604, 1706, 1708, 1709, 1727, 1728, 1766, 1769, 1772, 1773, 1789, 1790, 1791` |
| `textSignature` | contract | Pi text item field | - | `codec/pi.rs:730, 826, 1448, 1451, 1566, 1569, 1652, 1653` |
| `thinking` | contract | content item type or field | - | `codec/opencode.rs:188, 190`; `codec/pi.rs:600, 731, 794, 1061, 1068, 1080, 1455, 1456, 1462, 1463, 1574, 1581` |
| `thinkingSignature` | contract | Pi thinking item field | - | `codec/pi.rs:732, 1067, 1081, 1458, 1464, 1576, 1581` |
| `thoughtSignature` | contract | Pi call field | - | `codec/pi.rs:737, 854, 1482, 1485, 1595, 1598, 1660, 1661` |
| `time` | contract | OpenCode time field | - | `codec/opencode.rs:123, 1183, 1203, 1222` |
| `time_created` | contract | OpenCode store column | - | `codec/opencode.rs:127` |
| `timeCreated` | contract | OpenCode time field | - | `codec/opencode.rs:128` |
| `timestamp` | contract | Pi message field | - | `codec/pi.rs:220, 1235, 1239, 1503, 1533, 1536` |
| `tool` | contract | role value, OpenCode part type, and the OpenCode part field naming the tool | - | `wire.rs:397`; `tool_outcome.rs:24`; `injection.rs:170`; `history_summarizer_chunk.rs:1152`; `codec/opencode.rs:216, 451, 890, 892, 1008, 1010, 1022, 1024, 1037, 1059, 1081, 1083, 1146, 1177, 1179, 1198, 1200, 1379, 1382`; `codec/pi.rs:111, 458, 862, 878, 889, 1127, 1129, 1501, 1885` |
| `toolCall` | contract | Pi content item type | - | `codec/pi.rs:608, 808, 1086, 1471, 1513, 1587` |
| `toolCallId` | contract | Pi tool result field | - | `codec/pi.rs:456, 744, 1127, 1178, 1415, 1420` |
| `toolName` | contract | Pi tool result field | - | `tool_outcome.rs:62`; `codec/opencode.rs:1380`; `codec/pi.rs:745, 1129, 1423` |
| `toolResult` | contract | Pi role value | - | `codec/pi.rs:78, 91, 1417, 1503` |
| `toolUse` | contract | Pi stop reason value | - | `codec/pi.rs:1514` |
| `total` | contract | Pi cost field | - | `codec/pi.rs:1530` |
| `totalTokens` | contract | Pi usage field | - | `codec/pi.rs:1529` |
| `truncated` | contract | Pi `bashExecution` field | - | `codec/pi.rs:336` |
| `type` | contract | part, item, or row type field | - | `codec/opencode.rs:176, 451, 637, 642, 644, 646, 682, 692, 693, 835, 889, 890, 892, 976, 987, 996, 1008, 1022, 1081, 1159, 1161, 1168, 1177, 1198, 1235, 1242, 1326`; `codec/pi.rs:728, 753, 951, 964, 1016, 1054, 1327, 1443, 1455, 1462, 1471, 1513, 1562, 1574, 1581, 1587, 1603, 1706, 1708, 1726, 1735, 1743, 1766, 1769, 1772, 1789, 1790, 1805, 1840, 1842, 1844, 1855, 1857, 1863` |
| `url` | contract | file part field | - | `codec/opencode.rs:639, 644, 1247, 1252, 1253, 1254, 1326`; `codec/pi.rs:742, 971, 1805, 1841, 1842, 1869, 1870, 1871` |
| `usage` | contract | Pi assistant field | - | `codec/pi.rs:1524` |
| `user` | contract | role value | - | `chunk_text.rs:174`; `history_summarizer_chunk.rs:1153, 1204`; `codec/opencode.rs:165, 1128, 1133`; `codec/pi.rs:76, 89, 116, 1237, 1536` |

## Excluded literals

These literals are not a field, a tool, an argument key, or a discriminator
value. A format template, a regular expression, an `expect` or error message,
or rendered output text carries no interpretation of a harness record.

| Literal | Reason | Locations |
| --- | --- | --- |
| <code> commits: {}</code> | format template | `chunk_text.rs:113` |
| <code>; bytes={}</code> | format template | `tool_outcome.rs:167` |
| <code>&lt;output&gt;&lt;history_segments&gt;\n&lt;history_segment ...</code> | format template | `history_summarizer_chunk.rs:729` |
| <code>Messages {start}-{end} were not summarized: {...</code> | format template | `history_summarizer_chunk.rs:726` |
| <code>OpenCode serialization produced duplicate too...</code> | format template | `codec/opencode.rs:470` |
| <code>TC: {description}</code> | format template | `history_summarizer_chunk.rs:1296` |
| <code>TC: {name}</code> | format template | `history_summarizer_chunk.rs:1300` |
| <code>TC: {name}({key_arg})</code> | format template | `history_summarizer_chunk.rs:1298` |
| <code>[media:{kind} {} {filename}]</code> | format template | `history_summarizer_chunk.rs:1227` |
| <code>[media:{kind} {}]</code> | format template | `history_summarizer_chunk.rs:1228` |
| <code>[{start_ordinal}-{end_ordinal}]</code> | format template | `chunk_text.rs:108` |
| <code>[{start_ordinal}]</code> | format template | `chunk_text.rs:106` |
| <code>[{} items]</code> | format template | `selection.rs:556, 568` |
| <code>[{}]</code> | format template | `selection.rs:513` |
| <code>\n\nCommand exited with code {code}</code> | format template | `codec/pi.rs:334` |
| <code>daemon: history_summarizer project memory wit...</code> | format template | `history_summarizer_chunk.rs:867` |
| <code>data:{media_type};base64,</code> | format template | `codec/opencode.rs:640` |
| <code>data:{};base64,{data}</code> | format template | `codec/opencode.rs:1248` |
| <code>message id at ordinal {ordinal} is empty</code> | format template | `wire.rs:197` |
| <code>message id contains reserved &#x27;#&#x27;: {0}</code> | format template | `wire.rs:199` |
| <code>message id {0} appears more than once</code> | format template | `wire.rs:201` |
| <code>opencode-hash-{}</code> | format template | `codec/opencode.rs:159` |
| <code>opencode-{}</code> | format template | `codec/opencode.rs:1100, 1112` |
| <code>pi native message {index} ({}) must carry {sp...</code> | format template | `codec/pi.rs:154` |
| <code>pi native message {index} has role {role:?}, ...</code> | format template | `codec/pi.rs:144` |
| <code>pi native message {index} is not an {{id, mes...</code> | format template | `codec/pi.rs:140` |
| <code>tool_result {tool_call_id} at {mid}#{block_in...</code> | format template | `wire.rs:209` |
| <code>unsupported wire block {kind} at {mid}#{block...</code> | format template | `wire.rs:203` |
| <code>{ALIAS_OPEN}{alias}{ALIAS_CLOSE}</code> | format template | `history_summarizer_chunk.rs:1378` |
| <code>{PI_RESERVED_ID_PREFIX}synthetic:{}</code> | format template | `codec/pi.rs:1546` |
| <code>{PLACEHOLDER_RUN_ID_PREFIX}{session_id}</code> | format template | `history_summarizer_chunk.rs:736` |
| <code>{active_count} todos</code> | format template | `injection.rs:146` |
| <code>{digest:x}</code> | format template | `injection.rs:125, 461` |
| <code>{id}:{n}</code> | format template | `codec/pi.rs:1553` |
| <code>{id}&#124;{item_id}</code> | format template | `codec/pi.rs:1645` |
| <code>{mantissa}e{sign}{exponent}</code> | format template | `codec/pi.rs:369` |
| <code>{mid}#call:{call_id}</code> | format template | `wire.rs:596` |
| <code>{number:e}</code> | format template | `codec/pi.rs:366` |
| <code>{number}</code> | format template | `codec/pi.rs:364` |
| <code>{session_id}#retry{failures}</code> | format template | `history_summarizer_chunk.rs:647` |
| <code>{{{}}}</code> | format template | `selection.rs:509` |
| <code>{} {}:{} {}</code> | format template | `chunk_text.rs:115` |
| <code>{}:{}</code> | format template | `selection.rs:503` |
| <code>{}{}</code> | format template | `selection.rs:468, 550`; `injection.rs:126`; `history_summarizer_chunk.rs:1089, 1312` |
| <code>CK wire block identity must serialize</code> | panic, expect, or error text | `wire.rs:699` |
| <code>a host entry id</code> | panic, expect, or error text | `codec/pi.rs:150` |
| <code>a reserved id</code> | panic, expect, or error text | `codec/pi.rs:148` |
| <code>a selected identity serializes</code> | panic, expect, or error text | `history_summarizer_chunk.rs:146` |
| <code>decimal digits are ASCII</code> | panic, expect, or error text | `wire.rs:423` |
| <code>defer returned early</code> | panic, expect, or error text | `selection.rs:1001` |
| <code>firing assembly charges selected identities</code> | panic, expect, or error text | `history_summarizer_chunk.rs:950` |
| <code>hex digits are ASCII</code> | panic, expect, or error text | `wire.rs:656` |
| <code>one receipt per projected block</code> | panic, expect, or error text | `wire.rs:381` |
| <code>$1</code> | regular expression or replacement | `chunk_text.rs:45` |
| <code>(?i)\b(?:commit(?:ted&#124;ting&#124;s)?&#124;cherry-?pick(?...</code> | regular expression or replacement | `chunk_text.rs:198` |
| <code>(?i)&#96;?\b([0-9a-f]{7,12})\b&#96;?</code> | regular expression or replacement | `chunk_text.rs:191` |
| <code>(?i:&lt;system-reminder&gt;)[\s\S]*?(?i:&lt;/system-re...</code> | regular expression or replacement | `chunk_text.rs:186` |
| <code>,\s*,+</code> | regular expression or replacement | `chunk_text.rs:216` |
| <code>\(\s*\)</code> | regular expression or replacement | `chunk_text.rs:206` |
| <code>\s+([,.;:])</code> | regular expression or replacement | `chunk_text.rs:226` |
| <code>\s+,</code> | regular expression or replacement | `chunk_text.rs:211` |
| <code>\s{2,}</code> | regular expression or replacement | `chunk_text.rs:221` |
| <code>(empty)</code> | rendered text, marker, separator, or value | `selection.rs:234`; `chunk_text.rs:40, 41, 144, 146`; `codec/opencode.rs:520, 997, 1168`; `codec/pi.rs:368, 907, 1463, 1521, 1522, 1523, 1581` |
| <code> </code> | rendered text, marker, separator, or value | `chunk_text.rs:44` |
| <code> / </code> | rendered text, marker, separator, or value | `chunk_text.rs:115`; `tool_outcome.rs:199`; `history_summarizer_chunk.rs:1162, 1174` |
| <code>%Y-%m-%d</code> | rendered text, marker, separator, or value | `history_summarizer_chunk.rs:1117` |
| <code>(no output)</code> | rendered text, marker, separator, or value | `codec/pi.rs:344` |
| <code>+</code> | rendered text, marker, separator, or value | `codec/pi.rs:368` |
| <code>,</code> | rendered text, marker, separator, or value | `selection.rs:509, 513`; `chunk_text.rs:42` |
| <code>, </code> | rendered text, marker, separator, or value | `chunk_text.rs:43, 113` |
| <code>...[truncated]</code> | rendered text, marker, separator, or value | `selection.rs:36` |
| <code>0123456789abcdef</code> | rendered text, marker, separator, or value | `wire.rs:651` |
| <code>; </code> | rendered text, marker, separator, or value | `tool_outcome.rs:160` |
| <code>; head=\&quot;</code> | rendered text, marker, separator, or value | `tool_outcome.rs:170` |
| <code>; tail=\&quot;</code> | rendered text, marker, separator, or value | `tool_outcome.rs:170` |
| <code>&lt;!-- OMO_INTERNAL_INITIATOR --&gt;</code> | rendered text, marker, separator, or value | `chunk_text.rs:14` |
| <code>&lt;/summary&gt;</code> | rendered text, marker, separator, or value | `codec/pi.rs:48` |
| <code>Execution denied</code> | rendered text, marker, separator, or value | `codec/opencode.rs:1273`; `codec/pi.rs:1773, 1823` |
| <code>Ran &#96;</code> | rendered text, marker, separator, or value | `codec/pi.rs:341` |
| <code>The conversation history before this point wa...</code> | rendered text, marker, separator, or value | `codec/pi.rs:42` |
| <code>The following is a summary of a branch that t...</code> | rendered text, marker, separator, or value | `codec/pi.rs:47` |
| <code>[SYSTEM DIRECTIVE: EIDNARA</code> | rendered text, marker, separator, or value | `chunk_text.rs:13` |
| <code>[]</code> | rendered text, marker, separator, or value | `injection.rs:233, 328`; `history_summarizer_chunk.rs:124` |
| <code>[dropped]</code> | rendered text, marker, separator, or value | `selection.rs:447` |
| <code>[object]</code> | rendered text, marker, separator, or value | `selection.rs:559` |
| <code>\&quot;</code> | rendered text, marker, separator, or value | `history_summarizer_chunk.rs:1367` |
| <code>\n</code> | rendered text, marker, separator, or value | `history_summarizer_chunk.rs:331, 521`; `codec/opencode.rs:1283`; `codec/pi.rs:1831` |
| <code>\n&lt;/summary&gt;</code> | rendered text, marker, separator, or value | `codec/pi.rs:43` |
| <code>\n[… tokens truncated by the daemon to fit th...</code> | rendered text, marker, separator, or value | `history_summarizer_chunk.rs:1055` |
| <code>\n\n(command cancelled)</code> | rendered text, marker, separator, or value | `codec/pi.rs:332` |
| <code>\n\n[Output truncated. Full output: </code> | rendered text, marker, separator, or value | `codec/pi.rs:350` |
| <code>\n&#96;&#96;&#96;</code> | rendered text, marker, separator, or value | `codec/pi.rs:343` |
| <code>\u{1}</code> | rendered text, marker, separator, or value | `injection.rs:87` |
| <code>]</code> | rendered text, marker, separator, or value | `codec/pi.rs:350` |
| <code>&#96;\n</code> | rendered text, marker, separator, or value | `codec/pi.rs:341` |
| <code>&#96;&#96;&#96;\n</code> | rendered text, marker, separator, or value | `codec/pi.rs:343` |
| <code>application/octet-stream</code> | rendered text, marker, separator, or value | `codec/opencode.rs:624`; `codec/pi.rs:960, 1838` |
| <code>attachment</code> | rendered text, marker, separator, or value | `codec/opencode.rs:694` |
| <code>…</code> | rendered text, marker, separator, or value | `tool_outcome.rs:154` |
| <code>Option::is_none</code> | serde attribute path | `wire.rs:128, 130, 133, 140, 142` |
