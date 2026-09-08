import { createHash } from "node:crypto";
import {
    closeSync,
    constants as fsConstants,
    fstatSync,
    openSync,
    readFileSync,
    readSync,
} from "node:fs";
import { join } from "node:path";
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
const LAUNCHER_REL_PATH = "payload/bin/eidnara-host";
/** The manifest schema `eidnara-host` accepts in trusted mode. */
const PAYLOAD_MANIFEST_SCHEMA = "eidnara.payload-manifest/v1";

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
    /** Start of the lexical `node_modules` walk; unused once `explicitExternalRoot` is set. */
    declaringParentRoot?: string;
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

function verifyManifestFile(packageDir: string, raw: unknown, previous: string | null): string {
    const entry = record(raw, "payload manifest file");
    const path = entry.path;
    if (
        typeof path !== "string" ||
        !path.startsWith("payload/") ||
        path.split("/").some((part) => part.length === 0 || part === "." || part === "..") ||
        (previous !== null && previous >= path) ||
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
        if (
            !before.isFile() ||
            before.size !== entry.size ||
            (before.mode & 0o777) !== Number.parseInt(entry.mode, 8)
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
        return readFileSync(fd);
    } finally {
        closeSync(fd);
    }
}

function parseJson(bytes: Buffer, label: string): unknown {
    try {
        return JSON.parse(bytes.toString("utf8")) as unknown;
    } catch {
        fail(`${label} is malformed`);
    }
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
    const manifest = record(parseJson(manifestBytes, "payload manifest"), "payload manifest");
    if (manifest.schema !== PAYLOAD_MANIFEST_SCHEMA) {
        fail("payload manifest schema is not the trusted-mode schema");
    }
    const identity = record(manifest.package, "payload manifest package");
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
    if (typeof launcher?.sha256 !== "string") {
        fail("payload manifest names no launcher file");
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

function resolveVerifiedPayload(options: ResolveManagedPayloadDirOptions): VerifiedPayload {
    const resolution = resolvePayloadPackageDir({
        packageName: payloadPackageFor(options.target),
        ...(options.declaringParentRoot === undefined
            ? {}
            : { declaringParentRoot: options.declaringParentRoot }),
        ...(options.explicitExternalRoot === undefined
            ? {}
            : { explicitExternalRoot: options.explicitExternalRoot }),
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
