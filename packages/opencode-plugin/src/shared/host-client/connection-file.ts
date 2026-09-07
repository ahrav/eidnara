/**
 * The reader anchors each snapshot to an open descriptor.
 *
 * Section 4 of `docs/host-wire-protocol.md` defines the connection-file snapshot contract.
 *
 * Every ancestor directory is subject to the Section 4.2 client-discovery policy: reached without a symlink, owned by the current user or root, and not group- or other-writable unless the sticky bit is set.
 * A writable ancestor lets another user rename a whole subtree into place, so the policy covers the full chain, not only the immediate parent.
 * The immediate parent must additionally be owner-only.
 * After the read, ownership and mode are validated on the descriptor again: a file that becomes group- or world-readable during the read has exposed its key and fails closed without a restart.
 * Identity, size, mtime, and ctime must be unchanged, and the directory entry must still name the opened file; either mismatch is a replacement and permits one restart.
 *
 * Errors expose typed, redacted failures and never include key or daemon-ID bytes.
 * The ancestor checks are pathname checks, not descriptor-anchored: Node exposes no `openat`, `openat2`, or `RESOLVE_BENEATH` API, so a component can be swapped between an ancestor `lstat` and the final `open`.
 * `O_NOFOLLOW` guards only the final path component.
 * `ctime` is compared because an owner can rewrite a file and restore its `mtime` with `utimensat`, but cannot set `ctime`; every write and metadata change bumps it.
 */

import { type BigIntStats, constants as fsConstants } from "node:fs";
import { type FileHandle, lstat, open } from "node:fs/promises";
import { dirname, isAbsolute, resolve } from "node:path";
import type { Deadline } from "./deadline";

/** The wire protocol caps snapshots at 65,536 bytes. */
export const MAX_CONNECTION_FILE_LEN = 65_536;
export const CONNECTION_FILE_SCHEMA = 2;
export const KEY_LEN = 32;
export const DAEMON_ID_LEN = 16;
/** The reader rejects every wire-version value other than 2. */
export const WIRE_VERSION = 2;

export type ConnectionFileErrorCode =
    | "unsupported_platform"
    | "deadline_expired"
    | "not_found"
    | "open_failed"
    | "stat_failed"
    | "read_failed"
    | "not_directory"
    | "not_regular_file"
    | "multiply_linked"
    | "foreign_owner"
    | "insecure_permissions"
    | "oversize"
    | "replaced_during_read"
    | "invalid_utf8"
    | "invalid_json"
    | "invalid_schema"
    | "invalid_wire_version"
    | "invalid_setup_socket"
    | "invalid_key"
    | "invalid_daemon_id"
    | "invalid_pid"
    | "invalid_daemon_ver";

/** Callers must not pass credential bytes in `message` or `cause`. */
export class ConnectionFileError extends Error {
    constructor(
        message: string,
        readonly code: ConnectionFileErrorCode,
        readonly cause?: unknown,
    ) {
        super(message);
        this.name = "ConnectionFileError";
    }
}

/* */
export interface ConnectionSnapshot {
    readonly setupSocket: string;
    /** Exactly 32 key bytes. Bearer capability; must never be logged. */
    readonly key: Uint8Array;
    /** The daemon ID contains exactly 16 bytes. */
    readonly daemonId: Uint8Array;
    readonly pid: number;
    readonly daemonVer: string;
}

export interface ReadConnectionFileOptions {
    /** The reader checks the deadline between filesystem steps and bounds the entire snapshot. */
    deadline: Deadline;
    /** When omitted, `platform` defaults to the real platform. */
    platform?: NodeJS.Platform;
    /** When omitted, `uid` defaults to `process.geteuid()`. */
    uid?: number;
    /**
     * The reader invokes `afterOpen` once per attempt after opening the target descriptor and before reading.
     * `afterOpen` lets tests race replacements deterministically.
     */
    afterOpen?: () => void | Promise<void>;
    /**
     * The reader invokes `afterRead` once per attempt after the bounded read and before the post-read revalidation.
     * `afterRead` lets tests race in-place rewrites and permission changes against the post-read gate deterministically.
     */
    afterRead?: () => void | Promise<void>;
}

/**
 */
import { toExactByteArray } from "./bytes";

export { toExactByteArray };

function checkDeadline(deadline: Deadline): void {
    if (deadline.isExpired()) {
        throw new ConnectionFileError(
            "connection file snapshot deadline expired",
            "deadline_expired",
        );
    }
}

interface FileIdentity {
    dev: bigint;
    ino: bigint;
}

function sameIdentity(a: FileIdentity, b: FileIdentity): boolean {
    return a.dev === b.dev && a.ino === b.ino;
}

/**
 * Two stats describe the same unchanged inode when identity, size, mtime, and ctime all agree.
 * `size` and `mtime` catch an in-place rewrite; `ctime` catches a rewrite whose `mtime` was restored, and every chmod or chown.
 */
function sameSnapshot(a: BigIntStats, b: BigIntStats): boolean {
    return (
        sameIdentity(a, b) &&
        a.size === b.size &&
        a.mtimeNs === b.mtimeNs &&
        a.ctimeNs === b.ctimeNs
    );
}

function statErrno(error: unknown): string | undefined {
    if (error && typeof error === "object" && "code" in error) {
        const code = (error as { code?: unknown }).code;
        return typeof code === "string" ? code : undefined;
    }
    return undefined;
}

function currentUid(): number {
    // The effective uid is the principal the kernel authorizes filesystem access against.
    if (typeof process.geteuid !== "function") {
        throw new ConnectionFileError(
            "cannot determine process uid on this platform",
            "unsupported_platform",
        );
    }
    return process.geteuid();
}

async function openNoFollow(filePath: string): Promise<FileHandle> {
    try {
        return await open(
            filePath,
            fsConstants.O_RDONLY | fsConstants.O_NOFOLLOW | fsConstants.O_NONBLOCK,
        );
    } catch (error) {
        // The reader retries only `ENOENT`.
        // The reader fails closed for `EACCES`, `ELOOP`, descriptor exhaustion, and every other open fault.
        if (statErrno(error) === "ENOENT") {
            throw new ConnectionFileError(
                `connection file ${filePath} does not exist`,
                "not_found",
                error,
            );
        }
        throw new ConnectionFileError(
            `failed to open connection file ${filePath}`,
            "open_failed",
            error,
        );
    }
}

/**
 * The reader reads `MAX_CONNECTION_FILE_LEN + 1` bytes to detect oversize content without relying on stale metadata.
 * The loop rechecks `deadline` between reads; an in-flight `handle.read()` cannot be cancelled and can exceed the deadline by one syscall.
 */
async function readBounded(
    handle: FileHandle,
    deadline: Deadline,
    what: string,
): Promise<Uint8Array> {
    const buffer = Buffer.alloc(MAX_CONNECTION_FILE_LEN + 1);
    let total = 0;
    while (total < buffer.length) {
        checkDeadline(deadline);
        const { bytesRead } = await handle
            .read(buffer, total, buffer.length - total, total)
            .catch((error: unknown) => {
                throw new ConnectionFileError(`failed to read ${what}`, "read_failed", error);
            });
        if (bytesRead === 0) break;
        total += bytesRead;
    }
    if (total > MAX_CONNECTION_FILE_LEN) {
        throw new ConnectionFileError(
            `connection file exceeds the ${MAX_CONNECTION_FILE_LEN}-byte snapshot cap`,
            "oversize",
        );
    }
    return buffer.subarray(0, total);
}

/** An `fstat` failure becomes a `stat_failed` `ConnectionFileError`, never a raw `SystemError`. */
async function statDescriptor(handle: FileHandle, what: string): Promise<BigIntStats> {
    return handle.stat({ bigint: true }).catch((error: unknown) => {
        throw new ConnectionFileError(`failed to stat ${what}`, "stat_failed", error);
    });
}

interface OwnerModeStat {
    uid: bigint;
    mode: bigint;
}

/** Sticky bit on a directory: only the entry owner, directory owner, or root may rename or unlink an entry. */
const S_ISVTX = 0o1000;
const ROOT_UID = 0;

/** Owner-only means the current uid owns the entry and no group or other permission bit is set. */
function requireOwnerOnly(stat: OwnerModeStat, uid: number, what: string): void {
    if (Number(stat.uid) !== uid) {
        throw new ConnectionFileError(`${what} is not owned by the current user`, "foreign_owner");
    }
    if ((Number(stat.mode) & 0o077) !== 0) {
        throw new ConnectionFileError(
            `${what} has group or other permission bits; expected owner-only mode`,
            "insecure_permissions",
        );
    }
}

/**
 * A safe ancestor is owned by the current user or root and is not group- or other-writable unless sticky.
 * Root ownership permits standard root-owned ancestors such as `/`, `/home`, and `/tmp`.
 */
function requireSafeAncestor(stat: OwnerModeStat, uid: number, what: string): void {
    const owner = Number(stat.uid);
    if (owner !== uid && owner !== ROOT_UID) {
        throw new ConnectionFileError(
            `${what} is not owned by the current user or root`,
            "foreign_owner",
        );
    }
    const mode = Number(stat.mode);
    if ((mode & 0o022) !== 0 && (mode & S_ISVTX) === 0) {
        throw new ConnectionFileError(
            `${what} is writable by other users and not sticky; another user could rename over the connection-file tree`,
            "insecure_permissions",
        );
    }
}

/**
 * validateOpenStat rejects non-regular descriptors before reads because FIFO reads can block.
 * A second hard link is a name outside the validated directory through which the same inode can be rewritten without changing `dev`/`ino`.
 * A link count of zero means the opened inode was unlinked or renamed over, which is replacement churn rather than an alias.
 */
function validateOpenStat(
    stat: OwnerModeStat & { nlink: bigint; isFile(): boolean },
    uid: number,
    what: string,
): void {
    if (!stat.isFile()) {
        throw new ConnectionFileError(`${what} is not a regular file`, "not_regular_file");
    }
    if (stat.nlink === 0n) {
        throw new ConnectionFileError(
            `${what} was unlinked during the snapshot`,
            "replaced_during_read",
        );
    }
    if (stat.nlink !== 1n) {
        throw new ConnectionFileError(
            `${what} has ${stat.nlink} hard links; a published credential must have exactly one name`,
            "multiply_linked",
        );
    }
    requireOwnerOnly(stat, uid, what);
}

function ancestorsRootFirst(filePath: string): string[] {
    const chain: string[] = [];
    let dir = dirname(filePath);
    for (;;) {
        chain.push(dir);
        const next = dirname(dir);
        if (next === dir) break;
        dir = next;
    }
    return chain.reverse();
}

/**
 * The walk runs root first so a component that is not a directory is reported as `not_directory` rather than surfacing as `ENOTDIR` on a deeper component.
 * `lstat` on a symlinked component reports the link, which is not a directory.
 * The deadline is rechecked before each component so a deep path on a slow filesystem cannot keep issuing `lstat` calls after expiry.
 */
async function validateAncestors(filePath: string, uid: number, deadline: Deadline): Promise<void> {
    const chain = ancestorsRootFirst(filePath);
    const parent = chain[chain.length - 1];
    for (const dirPath of chain) {
        checkDeadline(deadline);
        const stat = await lstat(dirPath, { bigint: true }).catch((error: unknown) => {
            if (statErrno(error) === "ENOENT") {
                throw new ConnectionFileError(
                    `connection file directory ${dirPath} does not exist`,
                    "not_found",
                    error,
                );
            }
            throw new ConnectionFileError(
                `failed to stat connection file directory ${dirPath}`,
                "stat_failed",
                error,
            );
        });
        if (!stat.isDirectory()) {
            throw new ConnectionFileError(
                `connection file directory ${dirPath} is not a directory; symlinked or non-directory ancestors are rejected`,
                "not_directory",
            );
        }
        requireSafeAncestor(stat, uid, `connection file ancestor ${dirPath}`);
        if (dirPath === parent) {
            requireOwnerOnly(stat, uid, `connection file directory ${dirPath}`);
        }
    }
}

/* */
async function snapshotDirect(
    filePath: string,
    deadline: Deadline,
    uid: number,
    afterOpen?: () => void | Promise<void>,
    afterRead?: () => void | Promise<void>,
): Promise<Uint8Array> {
    const what = `connection file ${filePath}`;
    await validateAncestors(filePath, uid, deadline);
    checkDeadline(deadline);
    const before = await lstat(filePath, { bigint: true }).catch((error: unknown) => {
        if (statErrno(error) === "ENOENT") {
            throw new ConnectionFileError(`${what} does not exist`, "not_found", error);
        }
        throw new ConnectionFileError(`failed to stat ${what}`, "stat_failed", error);
    });
    if (before.isSymbolicLink()) {
        throw new ConnectionFileError(
            `${what} is a symlink; client discovery must reject symbolic links`,
            "not_regular_file",
        );
    }
    if (!before.isFile()) {
        throw new ConnectionFileError(`${what} is not a regular file`, "not_regular_file");
    }
    checkDeadline(deadline);
    const handle = await openNoFollow(filePath);
    try {
        await afterOpen?.();
        const during = await statDescriptor(handle, what);
        if (!sameIdentity(before, during)) {
            throw new ConnectionFileError(
                `${what} was replaced between lstat and open`,
                "replaced_during_read",
            );
        }
        validateOpenStat(during, uid, what);
        checkDeadline(deadline);
        const bytes = await readBounded(handle, deadline, what);
        await afterRead?.();
        checkDeadline(deadline);
        // The snapshot fails instead of restarting because a mode relaxed mid-read may expose key bytes already read.
        const after = await statDescriptor(handle, what);
        validateOpenStat(after, uid, what);
        if (!sameSnapshot(during, after)) {
            throw new ConnectionFileError(
                `${what} was rewritten during the snapshot`,
                "replaced_during_read",
            );
        }
        checkDeadline(deadline);
        const entry = await lstat(filePath, { bigint: true }).catch((error: unknown) => {
            if (statErrno(error) === "ENOENT") {
                throw new ConnectionFileError(
                    `${what} was removed during the snapshot`,
                    "replaced_during_read",
                    error,
                );
            }
            throw new ConnectionFileError(`failed to re-stat ${what}`, "stat_failed", error);
        });
        if (!entry.isFile() || !sameIdentity(during, entry)) {
            throw new ConnectionFileError(
                `${what} was replaced during the snapshot`,
                "replaced_during_read",
            );
        }
        checkDeadline(deadline);
        return bytes;
    } finally {
        await handle.close().catch(() => {});
    }
}

function invalid(code: ConnectionFileErrorCode, message: string): ConnectionFileError {
    return new ConnectionFileError(message, code);
}

/**
 * Validate the decoded JSON against wire doc Section 4.1: schema 2,
 * a required wire version of exactly 2, an absolute setup-socket path,
 * exactly 32 key bytes, exactly 16 daemon-ID bytes, a safe
 * integer PID, and a nonempty daemon version. No coercion anywhere.
 */
function validateSnapshotJson(parsed: unknown): ConnectionSnapshot {
    if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
        throw invalid("invalid_json", "connection file JSON must be an object");
    }
    const record = parsed as Record<string, unknown>;
    if (record.schema !== CONNECTION_FILE_SCHEMA) {
        throw invalid("invalid_schema", `connection file schema must be ${CONNECTION_FILE_SCHEMA}`);
    }
    if (record.wire_version !== WIRE_VERSION) {
        throw invalid(
            "invalid_wire_version",
            `connection file wire_version must be exactly ${WIRE_VERSION}`,
        );
    }
    const setupSocket = record.setup_socket;
    if (typeof setupSocket !== "string" || setupSocket.length === 0 || !isAbsolute(setupSocket)) {
        throw invalid(
            "invalid_setup_socket",
            "connection file setup_socket must be an absolute path",
        );
    }
    const key = toExactByteArray(record.key, KEY_LEN);
    if (key === null) {
        throw invalid(
            "invalid_key",
            `connection file key must be exactly ${KEY_LEN} integer bytes in 0..=255`,
        );
    }
    const daemonId = toExactByteArray(record.daemon_id, DAEMON_ID_LEN);
    if (daemonId === null) {
        throw invalid(
            "invalid_daemon_id",
            `connection file daemon_id must be exactly ${DAEMON_ID_LEN} integer bytes in 0..=255`,
        );
    }
    const pid = record.pid;
    if (typeof pid !== "number" || !Number.isSafeInteger(pid) || pid <= 0) {
        throw invalid("invalid_pid", "connection file pid must be a positive integer");
    }
    const daemonVer = record.daemon_ver;
    if (typeof daemonVer !== "string" || daemonVer.length === 0) {
        throw invalid("invalid_daemon_ver", "connection file daemon_ver must be a nonempty string");
    }
    return Object.freeze({
        setupSocket,
        key,
        daemonId,
        pid,
        daemonVer,
    });
}

function decodeAndValidate(bytes: Uint8Array): ConnectionSnapshot {
    let text: string;
    try {
        text = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
    } catch (error) {
        throw new ConnectionFileError("connection file is not valid UTF-8", "invalid_utf8", error);
    }
    let parsed: unknown;
    try {
        parsed = JSON.parse(text);
    } catch {
        // The parse error is dropped, not chained: V8 quotes the source text around the fault
        // in `SyntaxError.message`, and that text can include the key array.
        throw new ConnectionFileError("connection file is not valid JSON", "invalid_json");
    }
    return validateSnapshotJson(parsed);
}

/**
 */
export async function readConnectionFile(
    filePath: string,
    options: ReadConnectionFileOptions,
): Promise<ConnectionSnapshot> {
    const platform = options.platform ?? process.platform;
    if (platform === "win32") {
        throw new ConnectionFileError(
            "connection-file discovery is unsupported on win32; secure publication has no reviewed Windows contract",
            "unsupported_platform",
        );
    }
    const uid = options.uid ?? currentUid();
    // The resolved path is what `open` receives, so the ancestor chain checked is the chain opened.
    const resolvedPath = resolve(filePath);
    const snapshot = (): Promise<Uint8Array> =>
        snapshotDirect(resolvedPath, options.deadline, uid, options.afterOpen, options.afterRead);
    let bytes: Uint8Array;
    try {
        bytes = await snapshot();
    } catch (error) {
        if (!(error instanceof ConnectionFileError) || error.code !== "replaced_during_read") {
            throw error;
        }
        bytes = await snapshot();
    }
    return decodeAndValidate(bytes);
}
