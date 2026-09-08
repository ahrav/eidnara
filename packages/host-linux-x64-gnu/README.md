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
```

The manifest also carries the release identity, the digest of
`release/host-release.json` (trailing newline trimmed), the digest of
`release/production-inputs.lock.json` (full bytes), the package identity and
target, the platform floor, and the Synapse lane. `scripts/build-host-payload.ts`
assembles the development payload from a locally built daemon and addon; the
daemon accepts a development payload only in a debug build.
