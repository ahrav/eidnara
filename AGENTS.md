# AGENTS.md

Rust 1.98 workspace with a thin Bun/TypeScript layer.

- Treat `docs/host-wire-protocol.md` as the normative host wire contract. Wire names and literals require a versioned protocol change.
- Use `.github/workflows/ci.yml` as the source of truth for required checks. Pass `--locked` to Cargo commands except deliberate dependency-update checks.

## Agent skills

### Work tracking

Specs and implementation tickets use the backend configured in
`docs/agents/issue-tracker.md`. Read that file before any tracker operation.
