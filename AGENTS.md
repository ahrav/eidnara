# AGENTS.md

Rust 1.98 workspace with a thin Bun/TypeScript layer.

- Treat `docs/host-wire-protocol.md` as the normative host wire contract. Wire names and literals require a versioned protocol change.
- Use `.github/workflows/ci.yml` as the source of truth for required checks. Pass `--locked` to Cargo commands except deliberate dependency-update checks.
- `docs/properties/` catalogs subsystem safety, liveness, and reachability properties. Read its scoped `AGENTS.md` before changing those catalogs.
- TypeScript packages live under `packages/`: `@eidnara/shm-native`, `@eidnara/retina-local-fs`, `@eidnara/opencode` (`opencode-plugin/`), `@eidnara/pi` (`pi-plugin/`), `@eidnara/cli`, `@eidnara/host-linux-x64-gnu`, and `@eidnara/e2e-tests`, all at version `0.1.0`. Each package owns its `tsconfig`, lint configuration, and test preload. The root `package.json` scripts `typecheck`, `lint`, `test`, and `build` name each package explicitly, and `bun run check:repo` runs those four in order; root `build` carries only Bun and `tsc` steps, and cargo-backed builds such as the `shm-native` addon stay in the CI `native-addon` job.

## Agent skills

### Work tracking

Specs and implementation tickets use the backend configured in
`docs/agents/issue-tracker.md`. Read that file before any tracker operation.
