/**
 * The `layout` row of `release/host-release.json`, written out once for the
 * modules the `./tui` export loads as source from the installed package. The
 * package ships no copy of `release/`, so a JSON import here would resolve
 * outside the package root; the test beside this module asserts the object
 * equals the JSON row, and the JSON stays the owner of the values.
 */
export const RELEASE_LAYOUT = {
    connection_file: "connection.json",
    managed_subtree: "eidnara",
    runtime_directory: "run",
    storage_subdirectory: "context",
} as const;
