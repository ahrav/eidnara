# Shared-memory native addon

Native verification requires Linux x86-64. Run `bun run --cwd packages/shm-native build:native`, then test with `EIDNARA_SHM_NATIVE_CLAIMED_TARGET=1` so an addon-load failure fails instead of skipping.

An `addon_unavailable` result on another platform does not verify the addon. Treat `index.js`, `shm_native.node`, and native build outputs as generated files; do not edit or commit them.
