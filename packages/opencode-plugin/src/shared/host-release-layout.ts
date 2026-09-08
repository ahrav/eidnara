/**
 * Storage layout values mirror `release/host-release.json`.
 * The `./tui` export loads `src/` from the tarball, but `package.json` `files` excludes the manifest, so shipped code cannot import it. commentlint: allow(JUDGE)
 * The test beside this module asserts each constant equals its JSON field.
 */

/** `layout.managed_subtree` */
export const MANAGED_SUBTREE = "eidnara";

/** `layout.storage_subdirectory` */
export const STORAGE_SUBDIRECTORY = "context";
