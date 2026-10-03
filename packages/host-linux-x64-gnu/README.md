# @eidnara/host-linux-x64-gnu

Native `eidnara-host` payload package for Linux x64 glibc. It is an
exact-version optional dependency of the Eidnara parent packages
(`@eidnara/cli`, `@eidnara/opencode`, `@eidnara/pi`). Nothing runs it
directly.

## Platform floor

- Linux kernel >= 4.18, glibc >= 2.28, x64 only (musl unsupported)
- Real procfs self-fd execution (`/proc/self/fd/<n>`) is required before any
  native byte runs; below-floor hosts return `unsupported_platform`

## Payload layout

Every shipped file is listed (relative path, type, size, mode, SHA-256) in
`payload-manifest.json` (schema `eidnara.payload-manifest/v1`). The daemon
rejects files the manifest does not list.

```text
payload/
  bin/eidnara-host              daemon launcher binary (mode 755)
  native/shm_native.node        release-profile fixed-ring addon (mode 644)
  model/gte-modernbert-base-f32/ f32 ONNX model, tokenizer files, certification
                                corpus, and bundle manifest (mode 644)
  ort/libonnxruntime.so         ONNX Runtime 1.24.2 CPU library (mode 644)
```

The manifest also carries the release identity, the digest of
`release/host-release.json` (trailing newline trimmed), the digest of
`release/production-inputs.lock.json` (full bytes), the package identity and
target, the platform floor, and the LocalEmbeddings lane.

`bun run inputs:fetch` (`scripts/host-inputs.ts`) fills `target/host-inputs/`
with every input `release/production-inputs.lock.json` names, keyed by sha256.
Each download is bounded by its locked size and published only after its size
and sha256 match; the ORT library is the one locked member of the locked
release archive. `bun run payload:dev` (debug launcher, `mode: "development"`)
and `bun run payload:release` (release launcher, `mode: "production"`) stage
the same verified inputs at their locked paths. The daemon stages either
manifest through its trusted path and accepts a development manifest only in a
debug build. A validated payload whose embedding component fails to initialize
or certify reports `degraded`.
