/**
 * Pre-native trust pipeline: platform gate, retained-bootstrap revalidation,
 * certified physical install-layout resolution, and capacity-preflighted
 * no-follow staging. Nothing in this module executes a
 * byte — it only decides whether a trusted launcher object exists and stages
 * one when the certified package path allows it. Every failure is one closed
 * lifecycle reason.
 */

import { createHash, type Hash } from "node:crypto";
import {
    closeSync,
    fchmodSync,
    constants as fsConstants,
    fstatSync,
    fsyncSync,
    lstatSync,
    mkdirSync,
    openSync,
    readdirSync,
    readSync,
    realpathSync,
    renameSync,
    statfsSync,
    statSync,
    unlinkSync,
    writeSync,
} from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import hostRelease from "../../../../../release/host-release.json";
import type { InstallLayout } from "./contract-vocabulary";

const SHA256_HEX = /^[0-9a-f]{64}$/;

export type LifecycleFailureReason =
    | "unsupported_platform"
    | "unsupported_install_layout"
    | "native_payload_missing"
    | "native_payload_invalid"
    | "insufficient_storage"
    | "internal_error";

export class BootstrapError extends Error {
    constructor(
        readonly reason: LifecycleFailureReason,
        message: string,
    ) {
        super(message);
        this.name = "BootstrapError";
    }
}

/**
 * Kernel permission checks and newly created files use the effective UID, so
 * ownership checks compare against the effective UID rather than the real one.
 * A missing `process.geteuid` is an unsupported platform, not a file defect.
 * Comparing numeric `stat.uid` with `undefined` reports owned files as foreign.
 */
function currentUid(): number {
    if (typeof process.geteuid !== "function") {
        throw new BootstrapError("unsupported_platform", "cannot determine process uid");
    }
    return process.geteuid();
}

// ---------------------------------------------------------------------------
// Platform gate (R24). Runs before any package byte is opened.
// ---------------------------------------------------------------------------

export interface PlatformReaders {
    platform: NodeJS.Platform;
    arch: string;
    /** Kernel release, e.g. `5.10.220-x`. */
    kernelRelease: () => string;
    /** glibc runtime version, e.g. `2.34`, or null when unverifiable. */
    glibcVersion: () => string | null;
    /** True only when `/proc/self/fd` resolves on a real procfs. */
    procSelfFdUsable: () => boolean;
}

function detectGlibcVersion(): string | null {
    try {
        const report: unknown = process.report?.getReport?.();
        if (typeof report === "object" && report !== null) {
            const header = (report as { header?: { glibcVersionRuntime?: unknown } }).header;
            const version = header?.glibcVersionRuntime;
            if (typeof version === "string" && version.length > 0) return version;
        }
    } catch {
        // fall through to the conservative null below
    }
    return null;
}

/** `statfs` reports `PROC_SUPER_MAGIC` as `f_type` for procfs mounts. */
const PROC_SUPER_MAGIC = 0x9fa0n;

const PROC_SELF_FD = "/proc/self/fd";

/**
 * Whether this host can execute a retained payload through `/proc/self/fd`,
 * the Linux `procfs_self_fd_exec` capability the certified lane requires.
 *
 * Exported so qualification evidence records the same fact this gate decides:
 * a smoke report that omits it, or derives it some other way, describes a host
 * the platform gate never evaluated.
 */
export function detectProcSelfFd(): boolean {
    let fd: number | null = null;
    try {
        // `/proc/self/fd` must be mounted from procfs; reject other filesystem
        // types. Only procfs gives its `fd` entries magic-link semantics.
        if (statfsSync(PROC_SELF_FD, { bigint: true }).type !== PROC_SUPER_MAGIC) return false;
        // `/proc/self/fd/0` disappears when file descriptor 0 is closed, so the probe opens its own descriptor.
        // Path traversal requires search permission, not read permission, on `/`.
        fd = openSync(PROC_SELF_FD, fsConstants.O_RDONLY | fsConstants.O_DIRECTORY);
        // A magic link resolves to the open description itself, so the path and
        // the descriptor name one inode.
        const viaLink = statSync(`${PROC_SELF_FD}/${fd}`);
        const direct = fstatSync(fd);
        return viaLink.dev === direct.dev && viaLink.ino === direct.ino;
    } catch {
        return false;
    } finally {
        if (fd !== null) {
            try {
                closeSync(fd);
            } catch {
                // The probe descriptor's close is best-effort: the capability
                // answer is already decided and a close error cannot change it.
            }
        }
    }
}

export const defaultPlatformReaders: PlatformReaders = {
    platform: process.platform,
    arch: process.arch,
    kernelRelease: () => os.release(),
    glibcVersion: detectGlibcVersion,
    procSelfFdUsable: detectProcSelfFd,
};

function parseVersionPair(value: string): [number, number] | null {
    const match = /^(\d+)\.(\d+)/.exec(value);
    if (!match) return null;
    return [Number.parseInt(match[1] as string, 10), Number.parseInt(match[2] as string, 10)];
}

function meetsFloor(value: string, floor: string): boolean {
    const parsed = parseVersionPair(value);
    const min = parseVersionPair(floor);
    if (!parsed || !min) return false;
    if (parsed[0] !== min[0]) return parsed[0] > min[0];
    return parsed[1] >= min[1];
}

export type PlatformGate =
    | { ok: true; target: "linux-x64-gnu" }
    | { ok: false; reason: "unsupported_platform"; detail: string };

/**
 * Enforce the exact release-contract target table: Linux x64 with kernel and
 * glibc at or above their floors plus usable procfs self-fd execution.
 * Unknown, below-floor, and unverifiable hosts are `unsupported_platform`.
 */
export function checkPlatform(readers: PlatformReaders = defaultPlatformReaders): PlatformGate {
    const rejected = (detail: string): PlatformGate => ({
        ok: false,
        reason: "unsupported_platform",
        detail,
    });
    const platforms = hostRelease.platforms.supported;
    if (readers.platform === "linux") {
        if (readers.arch !== "x64") return rejected("unsupported architecture");
        const linux = platforms.find((entry) => entry.target === "linux-x64-gnu");
        if (!linux || !("kernel_min" in linux) || !("glibc_min" in linux)) {
            return rejected("no linux target in the release contract");
        }
        if (!meetsFloor(readers.kernelRelease(), linux.kernel_min)) {
            return rejected("kernel below the supported floor");
        }
        const glibc = readers.glibcVersion();
        if (glibc === null) return rejected("glibc version is unverifiable");
        if (!meetsFloor(glibc, linux.glibc_min)) {
            return rejected("glibc below the supported floor");
        }
        if (!readers.procSelfFdUsable()) {
            return rejected("procfs self-fd execution is unavailable");
        }
        return { ok: true, target: "linux-x64-gnu" };
    }
    return rejected("unsupported operating system");
}

// ---------------------------------------------------------------------------
// Certified physical install layouts (KTD10).
// ---------------------------------------------------------------------------

const MAX_HOIST_PARENT_SEGMENTS = 8;

export type LayoutResolution =
    | {
          ok: true;
          layout: InstallLayout;
          packageDir: string;
      }
    | {
          ok: false;
          reason: "unsupported_install_layout" | "native_payload_missing";
          detail: string;
      };

type EntryKind = "dir" | "symlink" | "absent" | "other" | "inaccessible";

/**
 * Classify one candidate path by its own metadata. Only a genuine absence is
 * `absent`, because absence is the single kind that lets the hoist walk climb
 * past a candidate: EACCES on a parent directory, ELOOP on an intermediate
 * component, descriptor exhaustion, and I/O faults all mean a nearer candidate
 * may exist but cannot be certified, so reporting them as absence would let a
 * more distant ancestor package win. ENOTDIR is absence because a
 * non-directory ancestor cannot contain the candidate at all.
 */
function classifyEntry(entryPath: string): EntryKind {
    try {
        const stat = lstatSync(entryPath);
        if (stat.isSymbolicLink()) return "symlink";
        if (stat.isDirectory()) return "dir";
        return "other";
    } catch (error) {
        const code = (error as NodeJS.ErrnoException).code;
        return code === "ENOENT" || code === "ENOTDIR" ? "absent" : "inaccessible";
    }
}

/**
 * Resolve the exact payload package from the lexical declaring-parent root:
 * the nested `node_modules/<pkg>` path, a hoisted sibling `node_modules`
 * within eight lexical parent segments, one Bun package-manager symlink that
 * resolves under the SAME install's `node_modules/.bun/.../node_modules/`,
 * or an explicit external compiled-host root. No importer, cwd, global
 * store, or unrelated ancestor is ever consulted, so nothing here can
 * trigger an auto-install.
 */
export function resolvePayloadPackageDir(options: {
    declaringParentRoot: string;
    packageName: string;
    /** Compiled-Bun external root; checked before lexical walking. */
    explicitExternalRoot?: string;
}): LayoutResolution {
    const { declaringParentRoot, packageName } = options;
    // A relative root is resolved against `process.cwd()` by every syscall
    // below, so an install elsewhere could certify a payload from whatever
    // directory the process happens to run in.
    if (
        !path.isAbsolute(declaringParentRoot) ||
        (options.explicitExternalRoot !== undefined &&
            !path.isAbsolute(options.explicitExternalRoot))
    ) {
        return {
            ok: false,
            reason: "unsupported_install_layout",
            detail: "install root is not an absolute path",
        };
    }
    if (options.explicitExternalRoot !== undefined) {
        const externalCandidate = path.join(
            options.explicitExternalRoot,
            "node_modules",
            ...packageName.split("/"),
        );
        if (classifyEntry(externalCandidate) === "dir") {
            if (!containedWithin(options.explicitExternalRoot, externalCandidate)) {
                return {
                    ok: false,
                    reason: "unsupported_install_layout",
                    detail: "external payload directory resolves outside its declared root",
                };
            }
            return { ok: true, layout: "compiled_bun_external", packageDir: externalCandidate };
        }
        return {
            ok: false,
            reason: "unsupported_install_layout",
            detail: "explicit external root has no physical payload directory",
        };
    }
    const segments = packageName.split("/");
    let current = declaringParentRoot;
    for (let depth = 0; depth <= MAX_HOIST_PARENT_SEGMENTS; depth++) {
        const nodeModules = path.join(current, "node_modules");
        const candidate = path.join(nodeModules, ...segments);
        const kind = classifyEntry(candidate);
        if (kind === "dir") {
            // A real directory that resolves outside this install is a foreign
            // payload reached through a symlinked ancestor, not this install's.
            // It is present, so it does not license climbing either — only
            // absence does.
            if (!containedWithin(current, candidate)) {
                return {
                    ok: false,
                    reason: "unsupported_install_layout",
                    detail: "payload directory resolves outside the declaring install",
                };
            }
            return {
                ok: true,
                layout: depth === 0 ? "npm_nested" : "npm_hoisted",
                packageDir: candidate,
            };
        }
        if (kind === "symlink") {
            const bunLayout = resolveBunLink(current, nodeModules, candidate, segments);
            if (bunLayout) return bunLayout;
            return {
                ok: false,
                reason: "unsupported_install_layout",
                detail: "payload entry is a non-certified symlink",
            };
        }
        if (kind === "other") {
            return {
                ok: false,
                reason: "unsupported_install_layout",
                detail: "payload entry is not a directory",
            };
        }
        if (kind === "inaccessible") {
            return {
                ok: false,
                reason: "unsupported_install_layout",
                detail: "payload entry is not inspectable",
            };
        }
        const parent = path.dirname(current);
        if (parent === current) break;
        current = parent;
    }
    return {
        ok: false,
        reason: "native_payload_missing",
        detail: "no certified physical layout carries the payload package",
    };
}

/**
 * Prove `candidate` still resolves to somewhere inside `root`'s own tree.
 *
 * {@link classifyEntry} lstats only the final component, so by the time it
 * answers `"dir"` the kernel has already followed every ancestor: a
 * `node_modules`, scope directory, or `.bun` entry replaced by a symlink to a
 * different install yields a final component that looks like a real directory
 * belonging to this one. That defeats one level up the same cross-install
 * redirection the symlink branch rejects at the final component.
 *
 * Canonical *equality* would be the wrong test. macOS resolves `/var` to
 * `/private/var` and symlinked home directories are common, so requiring the
 * resolved path to equal the literal one would reject benign installs.
 * Containment is the property actually needed: wherever the declaring parent's
 * tree really lives, the payload must be inside it.
 *
 * A resolution failure is not containment. This is a check-then-use, so an
 * ancestor swapped afterwards is still not excluded — that needs the `*at`
 * syscalls Node does not expose, and belongs to the native layer.
 */
export function containedWithin(root: string, candidate: string): boolean {
    let realRoot: string;
    let realCandidate: string;
    try {
        realRoot = realpathSync(root);
        realCandidate = realpathSync(candidate);
    } catch {
        return false;
    }
    // Compared through `path.relative` rather than a string prefix. A prefix test
    // has to append a separator to avoid matching a sibling whose name merely
    // starts with the root's, and that breaks when the root already ends in one:
    // for `/` the prefix becomes `//` and every real descendant fails, rejecting
    // a valid root-level `node_modules` layout. `relative` handles the root,
    // trailing separators, and `..` uniformly, and is the same boundary
    // discipline `redactLifecyclePath` uses.
    const relative = path.relative(realRoot, realCandidate);
    if (relative === "") return true;
    return relative !== ".." && !relative.startsWith(`..${path.sep}`) && !path.isAbsolute(relative);
}

function resolveBunLink(
    walkRoot: string,
    nodeModules: string,
    candidate: string,
    segments: string[],
): LayoutResolution | null {
    let resolved: string;
    try {
        resolved = realpathSync(candidate);
    } catch {
        return null;
    }
    if (classifyEntry(resolved) !== "dir") return null;
    const bunStore = path.join(nodeModules, ".bun");
    let bunStoreReal: string;
    try {
        bunStoreReal = realpathSync(bunStore);
    } catch {
        return null;
    }
    // The store must belong to the install being walked. Without this the
    // containment below is self-referential: a symlinked `node_modules` or
    // `.bun` makes `bunStoreReal` the attacker's store, and every path under it
    // then certifies as "the same install's" store.
    if (!containedWithin(walkRoot, bunStoreReal)) return null;
    const withinStore = resolved.startsWith(`${bunStoreReal}${path.sep}`);
    // The suffix is anchored on a separator so only a real `node_modules`
    // path component certifies: a bare suffix match also accepts a sibling
    // directory whose name merely ends in `node_modules`.
    const expectedSuffix = `${path.sep}${path.join("node_modules", ...segments)}`;
    if (!withinStore || !resolved.endsWith(expectedSuffix)) return null;
    return { ok: true, layout: "bun_physical_link", packageDir: resolved };
}

// ---------------------------------------------------------------------------
// Capacity preflight (KTD22 / R46). Checked arithmetic via BigInt.
// ---------------------------------------------------------------------------

const RESERVE_FLOOR_BYTES = 256n * 1024n * 1024n;
const MAX_REASONABLE_REQUIRED = 1n << 60n;

export type CapacityVerdict =
    | { ok: true }
    | { ok: false; reason: "insufficient_storage" | "native_payload_invalid"; detail: string };

/**
 * `available >= required + max(256 MiB, ceil(required * 10%))` with checked
 * arithmetic: a negative, non-integral, or implausibly large requirement is
 * `native_payload_invalid`, and a true shortfall is `insufficient_storage`.
 */
export function checkCapacity(requiredBytes: bigint, availableBytes: bigint): CapacityVerdict {
    if (requiredBytes < 0n || requiredBytes > MAX_REASONABLE_REQUIRED) {
        return {
            ok: false,
            reason: "native_payload_invalid",
            detail: "required byte count is impossible",
        };
    }
    if (availableBytes < 0n) {
        return {
            ok: false,
            reason: "native_payload_invalid",
            detail: "available bytes is negative",
        };
    }
    const tenPercent = (requiredBytes + 9n) / 10n;
    const reserve = tenPercent > RESERVE_FLOOR_BYTES ? tenPercent : RESERVE_FLOOR_BYTES;
    if (availableBytes < requiredBytes + reserve) {
        return {
            ok: false,
            reason: "insufficient_storage",
            detail: "destination filesystem lacks the required reserve",
        };
    }
    return { ok: true };
}

export function availableBytesFor(dirPath: string): bigint {
    const stats = statfsSync(dirPath, { bigint: true });
    return stats.bavail * stats.bsize;
}

// ---------------------------------------------------------------------------
// Retained-bootstrap revalidation and no-follow staging (KTD18 / R29-R31).
// ---------------------------------------------------------------------------

export interface RetainedBootstrap {
    /** Open O_NOFOLLOW read descriptor for the verified launcher object. */
    fd: number;
    path: string;
    sha256: string;
}

function invalid(detail: string): BootstrapError {
    return new BootstrapError("native_payload_invalid", detail);
}

/**
 * Reading until live EOF lets an endlessly appended object block the reader indefinitely.
 * A digest covers only bytes read; a grown object produces a corruption-like mismatch.
 */
function readExactBytes(
    fd: number,
    expectedBytes: number,
    sink: (chunk: Buffer) => void,
    mismatch: { shrank: string; grew: string },
): void {
    const buffer = Buffer.alloc(64 * 1024);
    let position = 0;
    while (position < expectedBytes) {
        const want = Math.min(buffer.length, expectedBytes - position);
        const read = readSync(fd, buffer, 0, want, position);
        if (read === 0) throw invalid(mismatch.shrank);
        sink(buffer.subarray(0, read));
        position += read;
    }
    // One readable byte past the expected size means the object exceeds its certified size.
    if (readSync(fd, buffer, 0, 1, position) !== 0) throw invalid(mismatch.grew);
}

function sha256OfFd(fd: number, expectedBytes: number): string {
    const hash = createHash("sha256");
    readExactBytes(fd, expectedBytes, (chunk) => hash.update(chunk), {
        shrank: "retained bootstrap shrank during revalidation",
        grew: "retained bootstrap grew during revalidation",
    });
    return hash.digest("hex");
}

/**
 * The byte count is the one capacity was preflighted against, so a source that
 * grows cannot drag the copy past the reserve `checkCapacity` approved.
 */
export function copyExactBytes(
    sourceFd: number,
    outFd: number,
    expectedBytes: number,
    hash: Hash,
): void {
    readExactBytes(
        sourceFd,
        expectedBytes,
        (chunk) => {
            hash.update(chunk);
            let written = 0;
            while (written < chunk.length) {
                written += writeSync(outFd, chunk, written, chunk.length - written);
            }
        },
        {
            shrank: "launcher source shrank during staging",
            grew: "launcher source grew during staging",
        },
    );
}

/**
 * Complete revalidation of a retained digest-addressed bootstrap: owner-only
 * regular file, single link, owner-executable, and byte-for-byte digest
 * match through one retained O_NOFOLLOW descriptor whose identity is checked
 * against the path before AND after hashing. Any failure closes the
 * descriptor and throws; a failed retained object is never executed and
 * never causes fallback to a different generation.
 */
export function revalidateRetainedBootstrap(
    bootstrapPath: string,
    expectedSha256: string,
): RetainedBootstrap {
    if (!SHA256_HEX.test(expectedSha256)) throw invalid("expected digest is noncanonical");
    const uid = currentUid();
    let fd: number;
    try {
        fd = openSync(
            bootstrapPath,
            // O_NONBLOCK so a FIFO at this name fails fstat instead of
            // blocking the open until a writer appears; regular-file reads
            // are unaffected.
            fsConstants.O_RDONLY | fsConstants.O_NOFOLLOW | fsConstants.O_NONBLOCK,
        );
    } catch (error) {
        // Only ENOENT means the retained bootstrap is absent.
        // ENOTDIR denotes a non-directory path component, not an absent bootstrap.
        const code = (error as NodeJS.ErrnoException).code;
        if (code === "ENOENT") {
            throw new BootstrapError("native_payload_missing", "retained bootstrap is absent");
        }
        throw invalid("retained bootstrap is not openable");
    }
    try {
        const stat = fstatSync(fd);
        if (!stat.isFile()) throw invalid("retained bootstrap is not a regular file");
        if (stat.nlink !== 1) throw invalid("retained bootstrap is not single-link");
        if (stat.uid !== uid) throw invalid("retained bootstrap has a foreign owner");
        if ((stat.mode & 0o077) !== 0)
            throw invalid("retained bootstrap is group/world accessible");
        if ((stat.mode & 0o100) === 0) throw invalid("retained bootstrap is not owner-executable");
        // Staging writes 0o500, so a writable retained object did not come from
        // this code path. Accepting one leaves the digest below describing bytes
        // that can still change: nothing snapshots the inode between
        // `sha256OfFd` and the exec of `/proc/self/fd/3`, and the post-read
        // identity check compares dev/ino, which an in-place overwrite
        // preserves.
        //
        // This enforces the invariant staging already establishes rather than
        // achieving immutability: the owner can always chmod the file back and
        // rewrite it, so the digest-to-exec window is only truly closed by a
        // sealed object, which needs the native layer.
        if ((stat.mode & 0o200) !== 0) throw invalid("retained bootstrap is owner-writable");
        const digest = sha256OfFd(fd, stat.size);
        if (digest !== expectedSha256) throw invalid("retained bootstrap digest mismatch");
        // This is the one path-addressed stat
        // in the sequence, so a bootstrap unlinked after its descriptor was read
        // would otherwise escape as a raw errno with no lifecycle reason.
        let after: ReturnType<typeof lstatSync>;
        try {
            after = lstatSync(bootstrapPath);
        } catch {
            throw invalid("retained bootstrap identity could not be reconfirmed");
        }
        if (after.ino !== stat.ino || after.dev !== stat.dev || after.nlink !== 1) {
            throw invalid("retained bootstrap identity drifted during revalidation");
        }
        return { fd, path: bootstrapPath, sha256: digest };
    } catch (error) {
        try {
            closeSync(fd);
        } catch {
            // The outer catch reports the original failure.
        }
        // `fstat` and hashing reads can fail with filesystem errors after opening the descriptor.
        // Wrap filesystem failures so callers receive a `BootstrapError` reason.
        if (error instanceof BootstrapError) throw error;
        const code = (error as { code?: string } | null)?.code;
        throw invalid(
            `retained bootstrap revalidation failed: ${code ?? "unknown filesystem error"}`,
        );
    }
}

const STAGING_TEMP_PREFIX = ".staging-";
const STAGING_TEMP_NAME = /^\.staging-(\d+)-\d+-\d+$/;

/**
 * `kill(pid, 0)` returns `ESRCH` only when no process has that PID, so an active stager's copy is not removed.
 * A reused PID delays reclamation until its process exits.
 * Only regular, single-link, owner-owned entries qualify, so a foreign object
 * planted under the prefix is left alone.
 */
function reclaimStaleStagingTemps(destDir: string, uid: number): void {
    for (const name of readdirSync(destDir)) {
        const match = STAGING_TEMP_NAME.exec(name);
        if (!match) continue;
        const pid = Number.parseInt(match[1] as string, 10);
        if (!Number.isSafeInteger(pid) || pid < 1 || pid === process.pid) continue;
        const tempPath = path.join(destDir, name);
        try {
            const stat = lstatSync(tempPath);
            if (!stat.isFile() || stat.nlink !== 1 || stat.uid !== uid) continue;
            process.kill(pid, 0);
        } catch (error) {
            if ((error as NodeJS.ErrnoException).code !== "ESRCH") continue;
            try {
                unlinkSync(tempPath);
            } catch {
                // Another stager may have reclaimed the same temp first.
            }
        }
    }
}

/**
 * Open the staging destination through one retained descriptor and prove it is
 * an owner-only real directory. `O_NOFOLLOW` makes a symlink at `destDir` fail
 * the open instead of redirecting every path built beneath it, `O_DIRECTORY`
 * rejects a non-directory at the same name, and the owner and mode checks
 * reject an existing insecure directory — `mkdirSync` with a mode applies that
 * mode only when it creates the directory and never chmods one that is
 * already there. The mode check rejects every group and world bit, matching
 * the 0o700 a fresh destination is created with and the 0o077 test the
 * retained object itself must pass: a traversable or listable store lets other
 * users inspect what staging reports as private.
 *
 * The proof binds to this descriptor. What it does NOT establish: Node exposes
 * no `openat`/`renameat`, so the temp create and the rename below still address
 * the destination by pathname and an attacker who can replace `destDir` or any
 * of its ancestors between this check and those operations is not excluded.
 * The identity re-check after the rename detects such a swap after the fact
 * rather than preventing it. Race-free staging requires component-wise
 * `openat(O_DIRECTORY|O_NOFOLLOW)` plus `renameat`, which lives in the native
 * layer.
 */
function openStagingDir(destDir: string, uid: number): number {
    let fd: number;
    try {
        fd = openSync(
            destDir,
            fsConstants.O_RDONLY | fsConstants.O_DIRECTORY | fsConstants.O_NOFOLLOW,
        );
    } catch {
        throw invalid("staging destination is not an openable real directory");
    }
    try {
        const stat = fstatSync(fd);
        if (!stat.isDirectory()) throw invalid("staging destination is not a directory");
        if (stat.uid !== uid) throw invalid("staging destination has a foreign owner");
        if ((stat.mode & 0o077) !== 0) {
            throw invalid("staging destination is group/world accessible");
        }
        return fd;
    } catch (error) {
        closeSync(fd);
        throw error;
    }
}

/**
 * Stage a package launcher into an owner-only digest-addressed bootstrap:
 * one O_NOFOLLOW source descriptor supplies both the hash and the copied
 * bytes (a hardlinked package-cache source is acceptable as bytes only),
 * output goes through an exclusive owner-only temp plus fsync plus atomic
 * rename, and the promoted object is completely revalidated before its
 * retained descriptor is returned. The destination is proved to be an
 * owner-only real directory through a retained descriptor before any output
 * exists, with the residual pathname race documented on
 * {@link openStagingDir}. Temps left by stagers whose process is gone are
 * reclaimed, then capacity is preflighted with the checked reserve before the
 * temp is created; post-preflight failures remove only the owned temp.
 */
export function stageBootstrap(options: {
    sourcePath: string;
    destDir: string;
    expectedSha256: string;
    availableBytesOverride?: bigint;
}): RetainedBootstrap {
    const { sourcePath, destDir, expectedSha256 } = options;
    if (!SHA256_HEX.test(expectedSha256)) throw invalid("expected digest is noncanonical");
    // The uid is resolved before the first filesystem effect so a host that
    // cannot report one fails without creating the destination or a temp.
    const uid = currentUid();
    let sourceFd: number;
    try {
        sourceFd = openSync(
            sourcePath,
            fsConstants.O_RDONLY | fsConstants.O_NOFOLLOW | fsConstants.O_NONBLOCK,
        );
    } catch (error) {
        // Only true absence is "missing". A source that is present but cannot be
        // opened safely — a symlink rejected by O_NOFOLLOW (ELOOP), an
        // unreadable mode, an unsearchable parent — is an installed payload that
        // cannot be trusted, which the contract calls `native_payload_invalid`
        // with `reinstall_eidnara`. Reporting `install_native_payload`
        // instead tells the operator to install what is already installed, and
        // also lowers the reason's precedence.
        // ENOTDIR denotes a non-directory path component, not an absent launcher.
        const code = (error as NodeJS.ErrnoException).code;
        if (code === "ENOENT") {
            throw new BootstrapError("native_payload_missing", "launcher source is absent");
        }
        throw invalid("launcher source is not openable");
    }
    let tempPath: string | null = null;
    let destFd: number | null = null;
    try {
        const before = fstatSync(sourceFd);
        if (!before.isFile()) throw invalid("launcher source is not a regular file");
        mkdirSync(destDir, { recursive: true, mode: 0o700 });
        destFd = openStagingDir(destDir, uid);
        const destBefore = fstatSync(destFd);
        // Reclaim before preflight so the freed bytes count toward capacity.
        reclaimStaleStagingTemps(destDir, uid);
        const capacity = checkCapacity(
            BigInt(before.size),
            options.availableBytesOverride ?? availableBytesFor(destDir),
        );
        if (!capacity.ok) throw new BootstrapError(capacity.reason, capacity.detail);
        tempPath = path.join(
            destDir,
            `${STAGING_TEMP_PREFIX}${process.pid}-${Date.now()}-${Math.floor(Math.random() * 1e9)}`,
        );
        const outFd = openSync(
            tempPath,
            fsConstants.O_WRONLY | fsConstants.O_CREAT | fsConstants.O_EXCL,
            0o700,
        );
        const hash = createHash("sha256");
        try {
            copyExactBytes(sourceFd, outFd, before.size, hash);
            const digest = hash.digest("hex");
            if (digest !== expectedSha256) throw invalid("launcher source digest mismatch");
            const after = fstatSync(sourceFd);
            if (
                after.ino !== before.ino ||
                after.dev !== before.dev ||
                after.size !== before.size ||
                after.mtimeMs !== before.mtimeMs
            ) {
                throw invalid("launcher source mutated during staging");
            }
            fchmodSync(outFd, 0o500);
            fsyncSync(outFd);
        } finally {
            closeSync(outFd);
        }
        const finalPath = path.join(destDir, expectedSha256);
        renameSync(tempPath, finalPath);
        tempPath = null;
        // The retained descriptor is the identity the checks above certified,
        // so comparing it against the path now reports a destination that was
        // swapped while the pathname-addressed temp and rename ran.
        const destAfter = lstatSync(destDir);
        if (destAfter.dev !== destBefore.dev || destAfter.ino !== destBefore.ino) {
            throw invalid("staging destination identity drifted during staging");
        }
        fsyncSync(destFd);
        return revalidateRetainedBootstrap(finalPath, expectedSha256);
    } catch (error) {
        // This module's contract is that every failure is one closed lifecycle
        // reason, but the syscalls above can still fail in ways no explicit
        // check anticipates — a dangling-symlink `destDir` makes `mkdirSync`
        // throw EEXIST before the no-follow open can reject it, and
        // statfs/fsync/rename can fail for reasons of their own. Left raw,
        // those escape as a bare `Error` with no `reason` and every caller
        // branching on the closed union mishandles them. Storage exhaustion
        // keeps its own reason; anything else is an untrusted payload.
        if (error instanceof BootstrapError) throw error;
        const code = (error as { code?: string } | null)?.code;
        if (code === "ENOSPC" || code === "EDQUOT") {
            throw new BootstrapError("insufficient_storage", `staging failed: ${code}`);
        }
        throw invalid(`staging failed: ${code ?? "unknown filesystem error"}`);
    } finally {
        closeSync(sourceFd);
        if (destFd !== null) closeSync(destFd);
        if (tempPath !== null) {
            try {
                unlinkSync(tempPath);
            } catch {
                // temp removal is best-effort; the exclusive name is unreachable
            }
        }
    }
}
