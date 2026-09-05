# tokenizer-bom-is-its-own-token

## Discovery trigger

The catalog's scope note treats BOM handling as a deliberate divergence from
`ai-tokenizer@1.0.6`, but no record owned it, and the only check derives its
expected ids from the function under test.

## Evidence trail

- `crates/tokenizer/tests/token_golden.rs:97-109`: the doc comment states the
  oracle drops the BOM because its rank lookup decodes byte slices with a
  BOM-stripping `TextDecoder`, scoring `EF BB BF 0A` as `"\n"` and yielding
  `[92, 203]`; the test encodes `"\u{feff}"`, `"\n"`, and `"x"` separately,
  asserts each is one id, and asserts `encode_ordinary("x\u{feff}\n")` equals
  `[x[0], bom[0], newline[0]]`.
- `crates/tokenizer/assets/claude.tiktoken`: row 88 is `eA== 92` (`x`), row
  199 is `Cg== 203` (`\n`), row 57534 is `77u/ 57538` (base64 of
  `EF BB BF`). These give the independent expected encoding `[92, 57538, 203]`.
- `crates/tokenizer/src/lib.rs:15-17` (crate docs) records that the oracle has
  two bugs producing ids that decode to different bytes than the input and that
  the crate does not reproduce them; `:25-28` describes the BOM case.
- `src/bpe.rs:175-193`: `Vocab::rank_of` keys every tier on raw bytes and never
  decodes, so a BOM-stripping lookup cannot arise without a lookup rewrite.
- The tokenizer portfolio evaluation (gap 4) called the existing oracle
  circular; this record adopts the asset ranks as the fix.

## Failure scenario

A BOM-stripping step on the rank-lookup path drops the middle id; a vocabulary
regeneration that renumbers or merges the `EF BB BF` row changes it. In both
cases the existing test passes if the single-character encodings move with it.

## Timing windows and dependencies

None; a static property of one input.

## What a test must construct

`encode_ordinary("x\u{feff}\n")` compared to ranks read from the asset file, not
from `encode_ordinary`.

## Investigation log

### Q: Is there an independent oracle for the BOM id?

- Sources examined: `assets/claude.tiktoken` rows for `eA==`, `Cg==`, `77u/`.
- Findings: the three ranks are 92, 203, 57538.
- Missing evidence: none.
- Conclusion: the guarantee is stated against those ranks.

### Q: Is the divergence about any BOM, or about one piece shape?

- Sources examined: `gen/gen-token-golden.ts:34-38`, `:76`, `:101-102`;
  `testdata/token-golden.json` entries `zero-width`, `bom-leading`,
  `bom-between-punct`; `lib.rs:25-28`.
- Findings: the oracle-produced `bom-leading` case has ids
  `[57538, 2666, 679, 284, 355, 31]`, BOM token included, so a leading BOM
  before a non-whitespace character is encoded correctly by the stock oracle;
  the defect fires only when the BOM shares a whitespace piece with a following
  whitespace character. A renumbered `EF BB BF` row already fails parity on
  the three BOM cases. The `[92, 203]` output is asserted only in docs.
- Missing evidence: a fixture pinning the stock oracle's output for
  `"x\u{feff}\n"`.
- Conclusion: the record's residue is the BOM-plus-adjacent-whitespace piece
  shape; the Impact and Existing check are narrowed accordingly.

### Q: Can the oracle's BOM-stripping defect recur at HEAD?

- Sources examined: `src/bpe.rs:70-108`, `:175-193`.
- Findings: the short and mid keys are built from raw bytes (`short_key`,
  `mid_key`) and the long tier indexes a byte-slice map; no text decoding sits
  on the lookup path.
- Missing evidence: none.
- Conclusion: the record's hazard is a lookup rewrite or a vocabulary edit; the
  self-referential test shape is unchanged.
