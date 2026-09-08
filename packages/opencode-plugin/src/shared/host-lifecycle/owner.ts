import { createHash } from "node:crypto";
import { closeSync, constants as fsConstants, fstatSync, openSync, readSync } from "node:fs";
import { join, resolve } from "node:path";
import hostRelease from "../../../../../release/host-release.json";
import {
    BootstrapError,
    type RetainedBootstrap,
    resolvePayloadPackageDir,
    revalidateRetainedBootstrap,
    stageBootstrap,
} from "./bootstrap";
import { managedSubtreePath } from "./paths";

const SHA256_RE = /^[0-9a-f]{64}$/;
const MAX_METADATA_BYTES = 1024 * 1024;
/** `MAX_PATH_BYTES` and `MAX_PATH_COMPONENTS` must match the limits enforced by `validate_rel_path` in `crates/host-runtime/src/generation.rs`. */
const MAX_PATH_BYTES = 4096;
const MAX_PATH_COMPONENTS = 128;
const LAUNCHER_REL_PATH = "payload/bin/eidnara-host";
/** The manifest schema `eidnara-host` accepts in trusted mode. */
const PAYLOAD_MANIFEST_SCHEMA = "eidnara.payload-manifest/v1";
/** Key sets `deny_unknown_fields` enforces on `TrustedPayloadManifest` in `crates/daemon/src/bin/eidnara-host.rs`. commentlint: allow(JUDGE) */
const MANIFEST_KEYS = [
    "schema",
    "release",
    "release_contract_sha256",
    "production_inputs_lock_sha256",
    "mode",
    "package",
    "platform_floor",
    "synapse",
    "launcher",
    "files",
];
const RELEASE_KEYS = ["id", "version"];
const PACKAGE_KEYS = ["name", "version", "target"];
const FILE_KEYS = ["path", "type", "size", "mode", "sha256"];
/** `fatal` rejects invalid UTF-8 as `serde_json::from_slice` does; `ignoreBOM` preserves a byte-order mark so `JSON.parse` rejects it. */
const STRICT_UTF8 = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });

export type PayloadTarget = "linux-x64-gnu";

export interface PreparedManagedLaunchTarget {
    kind: "retained-fd";
    fd: number;
    retained: RetainedBootstrap;
    payloadManifestDigest: string;
    payloadDir?: string;
}

export interface PrepareManagedLaunchTargetOptions {
    dataRoot: string;
    declaringParentRoot: string;
    target: PayloadTarget;
    /** Whether a missing or stale retained bootstrap may be staged from the package; observation never stages. */
    allowStaging: boolean;
    explicitExternalRoot?: string;
}

export type ResolveManagedPayloadDirOptions = Omit<
    PrepareManagedLaunchTargetOptions,
    "dataRoot" | "allowStaging"
>;

/** A package whose manifest and files verified; the digests the daemon and the retained bootstrap are keyed by. */
interface VerifiedPayload {
    payloadDir: string;
    launcherPath: string;
    payloadManifestDigest: string;
    launcherDigest: string;
}

function fail(message: string): never {
    throw new BootstrapError("native_payload_invalid", message);
}

function sha256(value: Buffer): string {
    return createHash("sha256").update(value).digest("hex");
}

/**
 * UTF-8 byte comparison matches Rust `str` ordering for `trusted_payload_sources`.
 * A JS relational comparison orders UTF-16 code units, which disagrees whenever
 * a BMP character at or above U+E000 meets an astral character.
 */
function compareManifestPaths(a: string, b: string): number {
    return Buffer.compare(Buffer.from(a, "utf8"), Buffer.from(b, "utf8"));
}

function verifyManifestFile(packageDir: string, raw: unknown, previous: string | null): string {
    const entry = exactRecord(raw, FILE_KEYS, "payload manifest file");
    const path = entry.path;
    if (
        typeof path !== "string" ||
        !path.startsWith("payload/") ||
        Buffer.byteLength(path) > MAX_PATH_BYTES ||
        path.split("/").length > MAX_PATH_COMPONENTS ||
        path.split("/").some((part) => part.length === 0 || part === "." || part === "..") ||
        (previous !== null && compareManifestPaths(previous, path) >= 0) ||
        entry.type !== "file" ||
        !Number.isSafeInteger(entry.size) ||
        (entry.size as number) <= 0 ||
        (entry.mode !== "644" && entry.mode !== "755") ||
        typeof entry.sha256 !== "string" ||
        !SHA256_RE.test(entry.sha256)
    ) {
        fail("payload manifest file entry is invalid");
    }
    let fd: number;
    try {
        fd = openSync(
            join(packageDir, path),
            fsConstants.O_RDONLY | fsConstants.O_NOFOLLOW | fsConstants.O_NONBLOCK,
        );
    } catch {
        fail("payload file is not openable without following links");
    }
    try {
        const before = fstatSync(fd);
        // The installer's umask clears archive permission bits (644 lands as 600 under umask 077). The on-disk file may omit declared bits but cannot add any.
        const declaredMode = Number.parseInt(entry.mode, 8);
        if (
            !before.isFile() ||
            before.size !== entry.size ||
            (before.mode & 0o777 & ~declaredMode) !== 0
        ) {
            fail("payload file metadata does not match its manifest");
        }
        const hash = createHash("sha256");
        const buffer = Buffer.allocUnsafe(128 * 1024);
        let position = 0;
        for (;;) {
            const count = readSync(fd, buffer, 0, buffer.length, position);
            if (count === 0) break;
            position += count;
            if (position > before.size) fail("payload file grew during verification");
            hash.update(buffer.subarray(0, count));
        }
        const after = fstatSync(fd);
        if (
            before.dev !== after.dev ||
            before.ino !== after.ino ||
            before.size !== after.size ||
            before.mtimeMs !== after.mtimeMs ||
            position !== before.size ||
            hash.digest("hex") !== entry.sha256
        ) {
            fail("payload file bytes do not match its manifest");
        }
    } finally {
        closeSync(fd);
    }
    return path;
}

function readNoFollowBytes(path: string, label: string): Buffer {
    let fd: number;
    try {
        fd = openSync(path, fsConstants.O_RDONLY | fsConstants.O_NOFOLLOW | fsConstants.O_NONBLOCK);
    } catch {
        fail(`${label} is not an openable regular file`);
    }
    try {
        const stat = fstatSync(fd);
        if (!stat.isFile() || stat.size <= 0 || stat.size > MAX_METADATA_BYTES) {
            fail(`${label} size or type is invalid`);
        }
        // An in-place append can race with `fstatSync`; the extra byte detects growth without an EOF-bounded read.
        const buffer = Buffer.allocUnsafe(stat.size + 1);
        let position = 0;
        for (;;) {
            const count = readSync(fd, buffer, position, buffer.length - position, position);
            if (count === 0) break;
            position += count;
            if (position > stat.size) fail(`${label} grew during verification`);
        }
        if (position !== stat.size) fail(`${label} shrank during verification`);
        return buffer.subarray(0, position);
    } finally {
        closeSync(fd);
    }
}

/**
 * `JSON.parse` keeps the last of two equal keys; serde's derived deserializer
 * reports `duplicate field`. Runs on text `JSON.parse` has already accepted,
 * so the scan handles well-formed input only.
 */
function hasDuplicateKey(text: string): boolean {
    // One frame per open container: a key set for an object, `null` for an array.
    const frames: (Set<string> | null)[] = [];
    let expectKey = false;
    for (let i = 0; i < text.length; i++) {
        const ch = text[i];
        if (ch === '"') {
            const start = i;
            for (i++; text[i] !== '"'; i++) if (text[i] === "\\") i++;
            const frame = frames.at(-1);
            if (expectKey && frame) {
                const key = JSON.parse(text.slice(start, i + 1)) as string;
                if (frame.has(key)) return true;
                frame.add(key);
                expectKey = false;
            }
        } else if (ch === "{") {
            frames.push(new Set());
            expectKey = true;
        } else if (ch === "[") {
            frames.push(null);
            expectKey = false;
        } else if (ch === "}" || ch === "]") {
            frames.pop();
            expectKey = false;
        } else if (ch === ",") {
            expectKey = frames.at(-1) instanceof Set;
        }
    }
    return false;
}

function parseJson(bytes: Buffer, label: string): unknown {
    let text: string;
    let value: unknown;
    try {
        text = STRICT_UTF8.decode(bytes);
        value = JSON.parse(text) as unknown;
    } catch {
        fail(`${label} is malformed`);
    }
    if (hasDuplicateKey(text)) fail(`${label} repeats a key`);
    return value;
}

function readNoFollowJson(path: string, label: string): unknown {
    return parseJson(readNoFollowBytes(path, label), label);
}

function record(value: unknown, label: string): Record<string, unknown> {
    if (value === null || typeof value !== "object" || Array.isArray(value)) {
        fail(`${label} must be an object`);
    }
    return value as Record<string, unknown>;
}

function exactRecord(
    value: unknown,
    keys: readonly string[],
    label: string,
): Record<string, unknown> {
    const entry = record(value, label);
    const present = Object.keys(entry);
    if (present.length !== keys.length || !keys.every((key) => Object.hasOwn(entry, key))) {
        fail(`${label} keys do not match the trusted-mode schema`);
    }
    return entry;
}

function bootstrapDir(dataRoot: string): string {
    return join(managedSubtreePath(dataRoot), "host-bootstrap", hostRelease.release.version);
}

function retainedTarget(
    retained: RetainedBootstrap,
    payloadManifestDigest: string,
    payloadDir?: string,
): PreparedManagedLaunchTarget {
    return {
        kind: "retained-fd",
        fd: retained.fd,
        retained,
        payloadManifestDigest,
        ...(payloadDir === undefined ? {} : { payloadDir }),
    };
}

/**
 * The manifest digest is over the file's bytes with one trailing newline
 * stripped, the same bytes `eidnara-host` digests before it trusts a payload,
 * so the value this returns is the one the daemon's `--payload-manifest-digest`
 * argument must carry.
 *
 * `release_contract_sha256` and `production_inputs_lock_sha256` get a shape
 * check only: the daemon holds the release files' bytes via `include_str!`,
 * this module holds the parsed contract, so equality is not computable here.
 */
function verifyPackage(packageDir: string, target: PayloadTarget): VerifiedPayload {
    const packageJson = record(
        readNoFollowJson(join(packageDir, "package.json"), "package.json"),
        "package.json",
    );
    const payloads: readonly string[] = hostRelease.packages.payloads;
    if (
        typeof packageJson.name !== "string" ||
        !payloads.includes(packageJson.name) ||
        packageJson.version !== hostRelease.release.version
    ) {
        fail("payload package identity does not match the release contract");
    }
    const manifestBytes = readNoFollowBytes(
        join(packageDir, "payload-manifest.json"),
        "payload manifest",
    );
    const manifest = exactRecord(
        parseJson(manifestBytes, "payload manifest"),
        MANIFEST_KEYS,
        "payload manifest",
    );
    if (manifest.schema !== PAYLOAD_MANIFEST_SCHEMA) {
        fail("payload manifest schema is not the trusted-mode schema");
    }
    const release = exactRecord(manifest.release, RELEASE_KEYS, "payload manifest release");
    if (
        release.id !== hostRelease.release.id ||
        release.version !== hostRelease.release.version ||
        manifest.mode !== "production" ||
        typeof manifest.release_contract_sha256 !== "string" ||
        !SHA256_RE.test(manifest.release_contract_sha256) ||
        typeof manifest.production_inputs_lock_sha256 !== "string" ||
        !SHA256_RE.test(manifest.production_inputs_lock_sha256) ||
        typeof manifest.synapse !== "string"
    ) {
        fail("payload manifest release identity does not match the release contract");
    }
    const identity = exactRecord(manifest.package, PACKAGE_KEYS, "payload manifest package");
    if (
        identity.name !== packageJson.name ||
        identity.version !== hostRelease.release.version ||
        identity.target !== target ||
        manifest.launcher !== LAUNCHER_REL_PATH
    ) {
        fail("payload manifest identity does not match its package");
    }
    if (!Array.isArray(manifest.files)) fail("payload manifest files must be an array");
    let previous: string | null = null;
    for (const raw of manifest.files) {
        previous = verifyManifestFile(packageDir, raw, previous);
    }
    const launcher = manifest.files.find(
        (raw) =>
            raw !== null &&
            typeof raw === "object" &&
            (raw as Record<string, unknown>).path === LAUNCHER_REL_PATH,
    ) as Record<string, unknown> | undefined;
    if (typeof launcher?.sha256 !== "string" || launcher.mode !== "755") {
        fail("payload manifest names no executable launcher file");
    }
    const trailingNewline = manifestBytes.at(-1) === 0x0a ? 1 : 0;
    return {
        payloadDir: packageDir,
        launcherPath: join(packageDir, LAUNCHER_REL_PATH),
        payloadManifestDigest: sha256(
            manifestBytes.subarray(0, manifestBytes.length - trailingNewline),
        ),
        launcherDigest: launcher.sha256,
    };
}

/** The contract's payload package for a target; package names end in the target triple. */
function payloadPackageFor(target: PayloadTarget): string {
    const payloads: readonly string[] = hostRelease.packages.payloads;
    const name = payloads.find((candidate) => candidate.endsWith(`-${target}`));
    if (name === undefined) fail(`release contract names no payload package for ${target}`);
    return name;
}

/** `runNativeLifecycle` rejects a relative `--payload-dir`; the daemon it spawns runs with `cwd: "/"`. */
function resolveVerifiedPayload(options: ResolveManagedPayloadDirOptions): VerifiedPayload {
    const resolution = resolvePayloadPackageDir({
        declaringParentRoot: resolve(options.declaringParentRoot),
        packageName: payloadPackageFor(options.target),
        ...(options.explicitExternalRoot === undefined
            ? {}
            : { explicitExternalRoot: resolve(options.explicitExternalRoot) }),
    });
    if (!resolution.ok) throw new BootstrapError(resolution.reason, resolution.detail);
    return verifyPackage(resolution.packageDir, options.target);
}

/**
 * Resolves the launch target for the installed payload: the retained bootstrap
 * staged from its launcher when one exists, or a fresh staging when the caller
 * allows it. The package is read to learn the digests either way; only staging
 * writes.
 */
export function prepareManagedLaunchTarget(
    options: PrepareManagedLaunchTargetOptions,
): PreparedManagedLaunchTarget | null {
    const payload = resolveVerifiedPayload(options);
    const retainedPath = join(bootstrapDir(options.dataRoot), payload.launcherDigest);
    try {
        return retainedTarget(
            revalidateRetainedBootstrap(retainedPath, payload.launcherDigest),
            payload.payloadManifestDigest,
            payload.payloadDir,
        );
    } catch (error) {
        if (!(error instanceof BootstrapError)) throw error;
        if (!options.allowStaging) return null;
    }
    return retainedTarget(
        stageBootstrap({
            sourcePath: payload.launcherPath,
            destDir: bootstrapDir(options.dataRoot),
            expectedSha256: payload.launcherDigest,
        }),
        payload.payloadManifestDigest,
        payload.payloadDir,
    );
}

export function resolveManagedPayloadDir(options: ResolveManagedPayloadDirOptions): string {
    return resolveVerifiedPayload(options).payloadDir;
}
