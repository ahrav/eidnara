import {
    closeSync,
    constants,
    lstatSync,
    mkdirSync,
    mkdtempSync,
    openSync,
    readFileSync,
    type Stats,
    writeSync,
} from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import hostRelease from "../../../../release/host-release.json";
import { getHarness, type HarnessId } from "./harness";

/**
 * The absolute `XDG_DATA_HOME` override, or `null` when the variable is
 * unset, empty, or relative. A relative value is rejected rather than joined
 * against the working directory, which would move the storage tree with cwd.
 */
function configuredDataHome(): string | null {
    const value = process.env.XDG_DATA_HOME?.trim();
    return value && path.isAbsolute(value) ? value : null;
}

export function getDataDir(): string {
    return configuredDataHome() ?? path.join(os.homedir(), ".local", "share");
}

function getEidnaraTempDir(harness: HarnessId = getHarness()): string {
    const owner = process.getuid?.() ?? os.userInfo().username;
    return path.join(os.tmpdir(), `eidnara-${owner}`, harness);
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
 * Whole-line match: a sibling block such as `# >>> eidnara-cache` must not
 * pass for Eidnara's own guard and suppress the `context/` rule.
 */
function hasGitignoreGuard(text: string): boolean {
    return text.split(/\r?\n/).some((line) => line.trim() === GITIGNORE_GUARD_OPEN);
}

/**
 * Symlinks are rejected because project contents choose their target.
 */
function isPlainEntry(stat: Stats, kind: "directory" | "file"): boolean {
    if (stat.isSymbolicLink()) return false;
    return kind === "directory" ? stat.isDirectory() : stat.isFile();
}

/**
 * `O_NOFOLLOW` fails with `ELOOP` if a symlink appears at `filePath` after
 * its `lstat` check, instead of writing through it.
 */
function writeFileNoFollow(filePath: string, data: string): void {
    const { O_WRONLY, O_CREAT, O_TRUNC, O_NOFOLLOW } = constants;
    const fd = openSync(filePath, O_WRONLY | O_CREAT | O_TRUNC | (O_NOFOLLOW ?? 0), 0o666);
    try {
        writeSync(fd, data);
    } finally {
        closeSync(fd);
    }
}

/**
 * If no opening guard exists, preserve existing entries and append the `context/` block.
 *
 * The opening guard prevents duplicate block insertion.
 *
 * Reject symlinks so project contents cannot redirect writes outside `directory`.
 */
export function ensureEidnaraArtifactGitignore(directory: string): void {
    try {
        const eidnaraDir = path.join(directory, ".eidnara");
        const gitignorePath = path.join(eidnaraDir, ".gitignore");
        const dirStat = lstatSync(eidnaraDir, { throwIfNoEntry: false });
        if (dirStat && !isPlainEntry(dirStat, "directory")) return;
        const fileStat = lstatSync(gitignorePath, { throwIfNoEntry: false });
        if (fileStat && !isPlainEntry(fileStat, "file")) return;
        let existing = "";
        if (fileStat) {
            existing = readFileSync(gitignorePath, "utf8");
            if (hasGitignoreGuard(existing)) return;
        }
        const block = `${GITIGNORE_GUARD_OPEN}\ncontext/\n${GITIGNORE_GUARD_CLOSE}\n`;
        const needsLeadingNewline = existing.length > 0 && !existing.endsWith("\n");
        const next = existing + (needsLeadingNewline ? "\n" : "") + block;
        mkdirSync(eidnaraDir, { recursive: true });
        writeFileNoFollow(gitignorePath, next);
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

/**
 * `OpenCode` and `Pi` use this path for shared persistent storage.
 *
 * Layout: <XDG_DATA_HOME>/eidnara/context/
 *
 * Tests must not resolve the user's shared database path.
 * Without an absolute `XDG_DATA_HOME`, `EIDNARA_TEST_DATA_DIR` overrides the storage root.
 * Without an absolute `XDG_DATA_HOME` or `EIDNARA_TEST_DATA_DIR`, `NODE_ENV=test` uses a memoized throwaway directory.
 * An absolute `XDG_DATA_HOME` takes precedence over both test overrides.
 */
export function getEidnaraStorageDir(): string {
    if (configuredDataHome() === null) {
        const testDataDir = process.env.EIDNARA_TEST_DATA_DIR?.trim();
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
