# Output certificate: dispute evidence (proposed obligations)

Extracted from [domain-model.md](domain-model.md) section 15; the model is the full record.

Reference note (PR A, #1160): this certificate records the design at
`3a3b916b7`. PR A implements its first nine obligations; the active
records are in [catalog.md](catalog.md).


```text
SCOPE AND AUTHORITY: daemon + Kernel store; plugins are bearer-authenticated
  claimants; daemon admits requests and derives classes; Kernel decides
  transitions; wire/SQL/language routed.
BEHAVIOR: FileDispute, PreviewDispute; witnesses and forbidden outcomes F1-F8.
RULES: A1-A14 with temporal kind, dependencies, validity point (section 5).
IDENTITY AND VALUES: subject and evidence entities existing; evidence_ref a
  value; dispute report NOT EARNED as entity (operation identity + row id);
  reporter a stamped value.
TRANSITIONS: existing Kernel disposition table; dispute adds data, no state.
PRESERVATION: enforcement sets 7.1-7.4; Kernel API path is an open bypass
  for A2/A3.
COMMAND OUTCOMES: certainty/cause/action in 6.1.
QUERY CONTRACTS: 6.3 internal, 6.4 readers.
PERSISTENCE AND CONCURRENCY: one snapshot per envelope; checks inside the
  writer closure; replay by identity.
DERIVED STATE: preview, structured reason, use-outcome link, citation
  liveness, serving projections (section 9).
STRUCTURE INPUTS: visibility predicate and reason grammar as hidden
  decisions; harness actor NOT EARNED as abstraction.
VARIATION / ASSEMBLY: closed two-kind evidence value; no assembly change.
FALSIFIERS: section 5 table; bounds in section 13.
HANDOFF: section 14; open conflicts C1-C6 and questions in
  conflicts-and-questions.md.
```
