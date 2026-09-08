import { existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import hostRelease from "../../../../release/host-release.json";
import { getHarness, type HarnessId } from "./harness";

export function getDataDir(): string {
    // `||` rather than `??`: `xdg-basedir` treats an empty `XDG_DATA_HOME` as unset, matching OpenCode's data directory.
    return process.env.XDG_DATA_HOME || path.join(os.homedir(), ".local", "share");
}

/**
 *
 * Layout:
 *
 *
 */
function getEidnaraTempDir(harness: HarnessId = getHarness()): string {
    return path.join(os.tmpdir(), harness, "eidnara");
}

/**
 * The default path includes `harness`, preventing default-path collisions.
 *
 */
export function getEidnaraLogPath(harness: HarnessId = getHarness()): string {
    const envPath = process.env.EIDNARA_LOG_PATH?.trim();
    if (envPath) return envPath;
    return path.join(getEidnaraTempDir(harness), "eidnara.log");
}

/**
 * Each harness stores historian artifacts separately.
 */
export function getEidnaraHistorianDir(harness: HarnessId = getHarness()): string {
    return path.join(getEidnaraTempDir(harness), "historian");
}

/**
 *
 * Layout: `<project-directory>/.eidnara/context/`
 *
 *
 * The ignore rule leaves `eidnara.jsonc` trackable.
 *
 *
 */
export function getProjectEidnaraDir(directory: string): string {
    return path.join(directory, ".eidnara", "context");
}

const GITIGNORE_GUARD_OPEN = "# >>> eidnara";
const GITIGNORE_GUARD_CLOSE = "# <<< eidnara";

/**
 * If no opening guard exists, preserve existing entries and append the `context/` block.
 *
 * The opening guard prevents duplicate block insertion.
 *
 */
export function ensureEidnaraArtifactGitignore(directory: string): void {
    try {
        const eidnaraDir = path.join(directory, ".eidnara");
        const gitignorePath = path.join(eidnaraDir, ".gitignore");
        let existing = "";
        if (existsSync(gitignorePath)) {
            existing = readFileSync(gitignorePath, "utf8");
            if (existing.includes(GITIGNORE_GUARD_OPEN)) return;
        }
        const block = `${GITIGNORE_GUARD_OPEN}\ncontext/\n${GITIGNORE_GUARD_CLOSE}\n`;
        const needsLeadingNewline = existing.length > 0 && !existing.endsWith("\n");
        const next = existing + (needsLeadingNewline ? "\n" : "") + block;
        mkdirSync(eidnaraDir, { recursive: true });
        writeFileSync(gitignorePath, next, "utf8");
    } catch {
        // Ignore errors while reading or updating `.eidnara/.gitignore`.
    }
}

/**
 *
 * Layout: `<project-directory>/.eidnara/context/historian/`
 *
 * Used for:
 *
 * Callers must create this directory before writing because a fresh project may not contain `.eidnara/`.
 */
export function getProjectEidnaraHistorianDir(directory: string): string {
    return path.join(getProjectEidnaraDir(directory), "historian");
}

export function getOpenCodeStorageDir(): string {
    return path.join(getDataDir(), "opencode", "storage");
}

/**
 *
 * `OpenCode` and `Pi` use this path for shared persistent storage.
 *
 * Layout: <XDG_DATA_HOME>/eidnara/context/
 *
 * Tests must not resolve the user's shared database path.
 * When `XDG_DATA_HOME` is unset, `EIDNARA_TEST_DATA_DIR` overrides the storage root.
 * When `XDG_DATA_HOME` is unset, `EIDNARA_TEST_DATA_DIR` overrides the storage root.
 * `EIDNARA_TEST_DATA_DIR` overrides the storage root only when `XDG_DATA_HOME` is unset.
 *
 * When `XDG_DATA_HOME` is unset, `NODE_ENV=test` without `EIDNARA_TEST_DATA_DIR` uses a throwaway directory.
 * When `XDG_DATA_HOME` is unset, `NODE_ENV=test`, and `EIDNARA_TEST_DATA_DIR` is unset, the resolver uses a memoized throwaway directory.
 *
 * `XDG_DATA_HOME` takes precedence over test isolation.
 */
export function getEidnaraStorageDir(): string {
    if (!process.env.XDG_DATA_HOME) {
        const testDataDir = process.env.EIDNARA_TEST_DATA_DIR;
        if (testDataDir) {
            return storageSubtreePath(testDataDir);
        }
        if (process.env.NODE_ENV === "test") {
            return getTestBackstopStorageDir();
        }
    }
    return storageSubtreePath(getDataDir());
}

/**
 * Eidnara's storage subtree under an explicit data root
 * (`${dataRoot}/eidnara/context`), with the segment names taken from
 * the release contract the Rust daemon conforms to. Root-parameterized so
 * harnesses and scripts that manage their own data root name the same tree
 * the daemon writes.
 */
export function storageSubtreePath(dataRoot: string): string {
    return path.join(
        dataRoot,
        hostRelease.layout.managed_subtree,
        hostRelease.layout.storage_subdirectory,
    );
}

let testBackstopDataRoot: string | null = null;
let testBackstopWarned = false;

/**
 * The resolver uses this throwaway data root when `XDG_DATA_HOME` is unset, `NODE_ENV=test`, and `EIDNARA_TEST_DATA_DIR` is unset.
 * Memoization keeps repeated calls on one database path.
 * A fresh temporary directory per call would bypass `openDatabase()`'s path cache.
 *
 */
export function getTestBackstopDataRoot(): string {
    if (!testBackstopDataRoot) {
        testBackstopDataRoot = mkdtempSync(path.join(os.tmpdir(), "eidnara-test-db-backstop-"));
    }
    if (!testBackstopWarned) {
        testBackstopWarned = true;
        console.warn(
            "[eidnara] TEST BACKSTOP: NODE_ENV=test with no EIDNARA_TEST_DATA_DIR " +
                `— redirecting storage to a throwaway temp dir (${testBackstopDataRoot}) so no ` +
                "test can touch the user's real shared database or daemon state. Wire " +
                "`[test] preload` in this package's bunfig.toml.",
        );
    }
    return testBackstopDataRoot;
}

function getTestBackstopStorageDir(): string {
    return storageSubtreePath(getTestBackstopDataRoot());
}

/**
 *
 * OpenCode falls back to `<homedir>/.cache` when `XDG_CACHE_HOME` is unset, including on Windows.
 * untouched.
 */
export function getCacheDir(): string {
    return process.env.XDG_CACHE_HOME ?? path.join(os.homedir(), ".cache");
}

export function getOpenCodeCacheDir(): string {
    return path.join(getCacheDir(), "opencode");
}
