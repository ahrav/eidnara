import { execFile } from "node:child_process";
import { constants as fsConstants } from "node:fs";
import { type FileHandle, lstat, open, readlink, realpath } from "node:fs/promises";
import path from "node:path";
import { promisify } from "node:util";

import { guardedSmartNoteHttpGet, type SmartNoteResolver } from "./ssrf-guard";
import { SmartNoteNetworkError, smartNoteAbortError } from "./types";

const execFileAsync = promisify(execFile);

const DEFAULT_FILE_LIMIT_BYTES = 64 * 1024;
const DEFAULT_GIT_TIMEOUT_MS = 3_000;
const MAX_GIT_SINCE_CHARS = 128;

// `git -C <root>` does not override repository-local variables (`git rev-parse --local-env-vars`), so a
// process launched from a hook would query the hook's repository. Pathspec-mode variables conflict with
// the `--literal-pathspecs` flag every command passes.
const GIT_ENV_DENYLIST = [
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_OBJECT_DIRECTORY",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_GRAFT_FILE",
    "GIT_INDEX_FILE",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_REPLACE_REF_BASE",
    "GIT_PREFIX",
    "GIT_INTERNAL_SUPER_PREFIX",
    "GIT_SHALLOW_FILE",
    "GIT_COMMON_DIR",
    "GIT_LITERAL_PATHSPECS",
    "GIT_GLOB_PATHSPECS",
    "GIT_NOGLOB_PATHSPECS",
    "GIT_ICASE_PATHSPECS",
] as const;

export interface SmartNoteCapabilityApi {
    readFile(repoRelativePath: string): Promise<string | null>;
    gitHeadSha(): Promise<string | null>;
    gitTag(): Promise<string | null>;
    gitLog(opts?: {
        maxCount?: number;
        path?: string;
        since?: string;
    }): Promise<Array<{ sha: string; subject: string; authorDate: string }>>;
    httpGet(url: string): Promise<{ status: number; body: string }>;
}

export type SmartNoteCapabilityFactory = (signal: AbortSignal) => SmartNoteCapabilityApi;

export interface SmartNoteCapabilitiesOptions {
    projectRoot: string;
    signal: AbortSignal;
    fileLimitBytes?: number;
    resolver?: SmartNoteResolver;
}

export function createSmartNoteCapabilities(
    options: SmartNoteCapabilitiesOptions,
): SmartNoteCapabilityApi {
    const projectRoot = path.resolve(options.projectRoot);
    const fileLimitBytes = options.fileLimitBytes ?? DEFAULT_FILE_LIMIT_BYTES;
    return {
        readFile: (repoRelativePath) =>
            guardedReadFile(projectRoot, repoRelativePath, options.signal, fileLimitBytes),
        gitHeadSha: () => runGitScalar(projectRoot, ["rev-parse", "HEAD"], options.signal),
        // `--dirty[=<mark>]` appends `<mark>` to the tag when the worktree is dirty, so any value corrupts the tag; omit it.
        // `--always` would substitute the commit id when no tag exists; an untagged repository yields `null`.
        gitTag: () =>
            runGitScalar(projectRoot, ["describe", "--tags", "--abbrev=0"], options.signal),
        gitLog: (opts) => guardedGitLog(projectRoot, opts, options.signal),
        httpGet: (url) =>
            guardedSmartNoteHttpGet(url, { signal: options.signal, resolver: options.resolver }),
    };
}

const SECRET_KEY_EXTENSIONS = [".p12", ".pfx", ".crt", ".key", ".pem"] as const;

export function isSecretDeniedPath(repoRelativePath: string): boolean {
    const normalized = normalizeRepoPath(repoRelativePath).toLowerCase();
    if (!normalized) return true;
    const segments = normalized.split("/");
    if (segments.includes(".git") || segments.includes("secrets")) return true;
    const basename = segments.at(-1) ?? "";

    if (basename === ".npmrc" || basename.startsWith(".env")) return true;
    if (basename === ".pgpass" || basename === ".netrc") return true;
    if (SECRET_KEY_EXTENSIONS.some((extension) => basename.endsWith(extension))) return true;
    if (
        basename === "id_rsa" ||
        basename === "id_dsa" ||
        basename === "id_ecdsa" ||
        basename === "id_ed25519" ||
        basename.startsWith("id_")
    ) {
        return true;
    }
    if (segments.includes(".aws") && basename === "credentials") return true;
    if (basename.endsWith(".json")) {
        const serviceAccountJson =
            basename.includes("service-account") || basename.includes("service_account");
        const gcloudCredentialJson =
            segments.includes("gcloud") &&
            (basename === "application_default_credentials.json" ||
                basename.includes("credential") ||
                segments.includes("legacy_credentials"));
        if (serviceAccountJson || gcloudCredentialJson) return true;
    }
    return false;
}

export function normalizeRepoPath(repoRelativePath: string): string {
    // Leading and trailing whitespace are part of a valid name; only a blank input is rejected.
    const slash = repoRelativePath.replace(/\\/g, "/");
    if (!slash.trim() || slash.startsWith("/") || /^[a-zA-Z]:\//.test(slash)) return "";
    const normalized = path.posix.normalize(slash);
    if (normalized === "." || normalized.startsWith("../") || normalized === "..") return "";
    return normalized;
}

async function guardedReadFile(
    projectRoot: string,
    repoRelativePath: string,
    signal: AbortSignal,
    fileLimitBytes: number,
): Promise<string | null> {
    throwIfAborted(signal);
    const body = guardedReadFileBody(projectRoot, repoRelativePath, signal, fileLimitBytes);
    let onAbort: (() => void) | undefined;
    const abort = new Promise<never>((_, reject) => {
        onAbort = () => reject(smartNoteAbortError(signal));
        if (signal.aborted) onAbort();
        else signal.addEventListener("abort", onAbort, { once: true });
    });
    try {
        return await Promise.race([body, abort]);
    } finally {
        if (onAbort) signal.removeEventListener("abort", onAbort);
    }
}

async function guardedReadFileBody(
    projectRoot: string,
    repoRelativePath: string,
    signal: AbortSignal,
    fileLimitBytes: number,
): Promise<string | null> {
    const normalized = normalizeRepoPath(repoRelativePath);
    if (!normalized || isSecretDeniedPath(normalized)) return null;

    const rootReal = await realpath(projectRoot).catch(() => null);
    throwIfAborted(signal);
    if (!rootReal) return null;
    const target = path.resolve(rootReal, normalized);
    if (!isPathInside(rootReal, target)) return null;

    const parentReal = await realpath(path.dirname(target)).catch(() => null);
    throwIfAborted(signal);
    if (!parentReal || !isPathInside(rootReal, parentReal)) return null;

    // A symlinked parent can move `target` outside `rootReal` or into a denied path; recheck the canonical path.
    const canonicalTarget = path.join(parentReal, path.basename(target));
    const canonicalRelative = normalizeRepoPath(path.relative(rootReal, canonicalTarget));
    if (
        !canonicalRelative ||
        !isPathInside(rootReal, canonicalTarget) ||
        isSecretDeniedPath(canonicalRelative)
    ) {
        return null;
    }

    const targetStat = await lstat(canonicalTarget).catch((error) => {
        if (isNoFollowOrMissing(error)) return null;
        throw error;
    });
    throwIfAborted(signal);
    if (!targetStat?.isFile() || targetStat.size > fileLimitBytes) return null;

    const noFollow = typeof fsConstants.O_NOFOLLOW === "number" ? fsConstants.O_NOFOLLOW : 0;
    const nonBlock = typeof fsConstants.O_NONBLOCK === "number" ? fsConstants.O_NONBLOCK : 0;
    const openPromise = open(canonicalTarget, fsConstants.O_RDONLY | noFollow | nonBlock).catch(
        (error) => {
            if (isNoFollowOrMissing(error)) return null;
            throw error;
        },
    );
    const handle = await closeLateOpenOnAbort(openPromise, signal);
    if (!handle) return null;
    try {
        throwIfAborted(signal);
        const stat = await handle.stat();
        if (stat.dev !== targetStat.dev || stat.ino !== targetStat.ino) return null;
        if (!stat.isFile() || stat.size > fileLimitBytes) return null;
        if (!(await openedPathIs(handle, canonicalTarget))) return null;
        return (await readToEof(handle, stat.size, signal)).toString("utf8");
    } finally {
        await handle.close().catch(() => {});
    }
}

// `O_NOFOLLOW` protects only the final path component; a parent-directory symlink swap redirects a
// pathname-based `open` to another file, and Node exposes no `openat` to bind the open to a checked
// directory descriptor. Linux publishes the opened dentry's path under procfs; that path must equal the
// validated canonical path. Elsewhere the caller's `dev`/`ino` comparison is the only ancestry check.
async function openedPathIs(handle: FileHandle, canonicalTarget: string): Promise<boolean> {
    if (process.platform !== "linux") return true;
    let opened: string;
    try {
        opened = await readlink(`/proc/self/fd/${handle.fd}`);
    } catch {
        return true;
    }
    return opened === canonicalTarget;
}

// `read(2)` may return fewer bytes than requested before EOF even on regular files.
async function readToEof(handle: FileHandle, size: number, signal: AbortSignal): Promise<Buffer> {
    const buffer = Buffer.alloc(size);
    let total = 0;
    while (total < size) {
        const { bytesRead } = await handle.read(buffer, total, size - total, total);
        throwIfAborted(signal);
        if (bytesRead === 0) break;
        total += bytesRead;
    }
    return buffer.subarray(0, total);
}

async function closeLateOpenOnAbort(
    openPromise: Promise<FileHandle | null>,
    signal: AbortSignal,
): Promise<FileHandle | null> {
    const handle = await openPromise;
    if (!signal.aborted) return handle;
    if (handle) void handle.close().catch(() => {});
    throw smartNoteAbortError(signal);
}

function isPathInside(root: string, target: string): boolean {
    const relative = path.relative(root, target);
    if (relative === "") return true;
    if (path.isAbsolute(relative)) return false;
    // Only a whole `..` component escapes; a name such as `..generated` is an ordinary entry.
    return relative !== ".." && !relative.startsWith(`..${path.sep}`);
}

function isNoFollowOrMissing(error: unknown): boolean {
    const code = (error as { code?: string } | null)?.code;
    return code === "ENOENT" || code === "ELOOP" || code === "ENOTDIR" || code === "EINVAL";
}

async function runGitScalar(
    projectRoot: string,
    args: string[],
    signal: AbortSignal,
): Promise<string | null> {
    const stdout = await runGit(projectRoot, args, signal);
    const value = stdout.trim();
    return value ? value.split("\n")[0] : null;
}

async function guardedGitLog(
    projectRoot: string,
    opts: { maxCount?: number; path?: string; since?: string } | undefined,
    signal: AbortSignal,
): Promise<Array<{ sha: string; subject: string; authorDate: string }>> {
    const maxCount = Math.max(1, Math.min(50, Math.floor(opts?.maxCount ?? 10)));
    const args = ["log", `-${maxCount}`, "--format=%H%x1f%aI%x1f%s", "--no-ext-diff", "--no-color"];
    if (opts?.since !== undefined) {
        // Git decides which date spellings it accepts; an unparseable value selects no commits.
        // Dropping the filter instead would return history the check did not ask for.
        if (!isSaneGitArgument(opts.since, MAX_GIT_SINCE_CHARS)) return [];
        args.push(`--since=${opts.since}`);
    }
    if (opts?.path) {
        // `--literal-pathspecs` in runGit keeps `:(top)`, `:/`, and glob magic from widening this path.
        const normalized = normalizeRepoPath(opts.path);
        if (!normalized || isSecretDeniedPath(normalized)) return [];
        args.push("--", normalized);
    }
    const stdout = await runGit(projectRoot, args, signal);
    return stdout
        .split("\n")
        .map((line) => line.trim())
        .filter(Boolean)
        .map((line) => {
            const [sha, authorDate, subject] = line.split("\x1f");
            return { sha: sha ?? "", authorDate: authorDate ?? "", subject: subject ?? "" };
        })
        .filter((row) => row.sha.length > 0);
}

// Ordinary git failures (not a repository, unknown ref) resolve to "" so the guest sees `null`.
// Abort and timeout reject so the runner reports a network failure instead of a fabricated result.
async function runGit(projectRoot: string, args: string[], signal: AbortSignal): Promise<string> {
    throwIfAborted(signal);
    try {
        const result = await execFileAsync(
            "git",
            ["--literal-pathspecs", "-C", projectRoot, ...args],
            {
                timeout: DEFAULT_GIT_TIMEOUT_MS,
                maxBuffer: 128 * 1024,
                signal,
                env: gitEnvironment(),
            },
        );
        return result.stdout;
    } catch (error) {
        if (
            signal.aborted ||
            (error as { killed?: boolean; signal?: string }).signal === "SIGTERM"
        ) {
            throw new SmartNoteNetworkError("SMART_NOTE_NETWORK: git command timed out or aborted");
        }
        return "";
    }
}

function gitEnvironment(): NodeJS.ProcessEnv {
    const env: NodeJS.ProcessEnv = { ...process.env };
    for (const name of GIT_ENV_DENYLIST) delete env[name];
    return env;
}

// One argv element is never re-tokenized, so only length and control characters need bounding.
function isSaneGitArgument(value: string, maxChars: number): boolean {
    // biome-ignore lint/suspicious/noControlCharactersInRegex: control characters are the rejection target
    return value.length > 0 && value.length <= maxChars && !/[\u0000-\u001f\u007f]/.test(value);
}

function throwIfAborted(signal: AbortSignal): void {
    if (signal.aborted) throw new SmartNoteNetworkError("SMART_NOTE_NETWORK: aborted");
}
