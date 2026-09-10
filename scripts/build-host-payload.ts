import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
    chmodSync,
    copyFileSync,
    existsSync,
    lstatSync,
    mkdirSync,
    readdirSync,
    readFileSync,
    rmSync,
    writeFileSync,
} from "node:fs";
import { createRequire } from "node:module";
import { dirname, extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

function fail(message: string): never {
    throw new Error(`eidnara-host payload: ${message}`);
}

export const LAUNCHER_PATH = "payload/bin/eidnara-host";
export const ADDON_PATH = "payload/native/shm_native.node";
export const MANIFEST_SCHEMA = "eidnara.payload-manifest/v1";

const RELEASE_CONTRACT_PATH = "release/host-release.json";
const PRODUCTION_INPUTS_LOCK_PATH = "release/production-inputs.lock.json";
const MANIFEST_FILE_NAME = "payload-manifest.json";
const PACKAGE_DOCS = ["README.md", "LICENSE", "NOTICE"] as const;
const PACKAGE_FILES = ["payload", MANIFEST_FILE_NAME, ...PACKAGE_DOCS] as const;
const FORBIDDEN_PACKAGE_FIELDS = [
    "scripts",
    "dependencies",
    "devDependencies",
    "optionalDependencies",
    "peerDependencies",
    "bin",
] as const;

export const PAYLOAD_TARGET = {
    package: "@eidnara/host-linux-x64-gnu",
    dir: "packages/host-linux-x64-gnu",
    target: "linux-x64-gnu",
    /** `buildTarget()` string the addon must report; `packages/shm-native/index.ts` refuses any other value with `wrong_platform_binary`. */
    nativeTarget: "linux-x86_64",
    os: ["linux"],
    cpu: ["x64"],
    libc: ["glibc"],
} as const;

const PATH_SEGMENT_RE = /^[A-Za-z0-9][A-Za-z0-9._-]*$/;
const SHA256_RE = /^[0-9a-f]{64}$/;
/** Extensions the CLI accepts for `--addon`; `buildDevPayload` also accepts a CommonJS module so tests can run without a compiled addon. */
const NATIVE_ADDON_EXTENSIONS = new Set([".so", ".node"]);

export interface PayloadFileEntry {
    path: string;
    type: "file";
    size: number;
    mode: "644" | "755";
    sha256: string;
}

export interface PlatformFloor {
    kernel_min: string;
    glibc_min: string;
    procfs_self_fd_exec: boolean;
}

export interface PayloadManifest {
    schema: string;
    release: { id: string; version: string };
    release_contract_sha256: string;
    production_inputs_lock_sha256: string;
    mode: "development";
    package: { name: string; version: string; target: string };
    platform_floor: PlatformFloor;
    synapse: string;
    launcher: string;
    files: PayloadFileEntry[];
}

export interface ReleasePlatform {
    target: string;
    kernel_min: string;
    glibc_min: string;
    synapse: string;
    capabilities: { procfs_self_fd_exec: boolean };
}

export interface ReleaseContract {
    release: { id: string; version: string };
    platforms: { supported: ReleasePlatform[] };
    packages: { payloads: string[] };
}

export interface ReleaseContext {
    contract: ReleaseContract;
    contractSha256: string;
    lockSha256: string;
}

export interface DevPayloadResult {
    outDir: string;
    manifestPath: string;
    manifest: PayloadManifest;
    digest: string;
    launcherSha256: string;
    addonSha256: string;
}

type JsonValue = null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue };

/** The daemon hashes exact manifest bytes, so canonicalJson recursively sorts object keys, preserves array order, and emits no whitespace. */
export function canonicalJson(value: unknown): string {
    return JSON.stringify(sortKeys(value as JsonValue));
}

function sortKeys(value: JsonValue): JsonValue {
    if (Array.isArray(value)) return value.map(sortKeys);
    if (value !== null && typeof value === "object") {
        const out: { [key: string]: JsonValue } = {};
        // Default string ordering keeps the output independent of locale.
        for (const key of Object.keys(value).sort()) {
            out[key] = sortKeys(value[key] as JsonValue);
        }
        return out;
    }
    return value;
}

export function sha256Hex(bytes: Uint8Array | string): string {
    return createHash("sha256").update(bytes).digest("hex");
}

/** The daemon strips one trailing newline from the manifest file before digesting it (`trusted_payload_sources`). */
export function payloadManifestDigest(manifest: PayloadManifest): string {
    return sha256Hex(canonicalJson(manifest));
}

function readJson(rootDir: string, relative: string): unknown {
    const path = join(rootDir, relative);
    if (!existsSync(path)) fail(`missing ${relative}`);
    try {
        return JSON.parse(readFileSync(path, "utf8"));
    } catch (error) {
        fail(`${relative} is not valid JSON: ${error instanceof Error ? error.message : error}`);
    }
}

function isRecord(value: unknown): value is Record<string, unknown> {
    return value !== null && typeof value === "object" && !Array.isArray(value);
}

function asContract(value: unknown): ReleaseContract {
    const where = RELEASE_CONTRACT_PATH;
    if (!isRecord(value)) fail(`${where} must be an object`);
    const release = value.release;
    if (
        !isRecord(release) ||
        typeof release.id !== "string" ||
        typeof release.version !== "string"
    ) {
        fail(`${where}: release.id and release.version must be strings`);
    }
    const platforms = value.platforms;
    if (!isRecord(platforms) || !Array.isArray(platforms.supported)) {
        fail(`${where}: platforms.supported must be an array`);
    }
    for (const platform of platforms.supported) {
        if (
            !isRecord(platform) ||
            typeof platform.target !== "string" ||
            typeof platform.kernel_min !== "string" ||
            typeof platform.glibc_min !== "string" ||
            typeof platform.synapse !== "string" ||
            !isRecord(platform.capabilities) ||
            typeof platform.capabilities.procfs_self_fd_exec !== "boolean"
        ) {
            fail(`${where}: malformed platforms.supported entry`);
        }
    }
    const packages = value.packages;
    if (
        !isRecord(packages) ||
        !Array.isArray(packages.payloads) ||
        !packages.payloads.every((name) => typeof name === "string")
    ) {
        fail(`${where}: packages.payloads must be an array of strings`);
    }
    return value as unknown as ReleaseContract;
}

/** Digests match `release_contract_sha256` (contract, one trailing newline stripped) and `production_inputs_lock_sha256` (lock, full bytes). */
export function loadReleaseContext(rootDir: string): ReleaseContext {
    const contract = asContract(readJson(rootDir, RELEASE_CONTRACT_PATH));
    const contractBytes = readFileSync(join(rootDir, RELEASE_CONTRACT_PATH));
    const trimmed = contractBytes.at(-1) === 0x0a ? contractBytes.subarray(0, -1) : contractBytes;
    const contractSha256 = sha256Hex(trimmed);
    const lockBytes = readFileSync(join(rootDir, PRODUCTION_INPUTS_LOCK_PATH));
    if (lockBytes.length === 0) fail(`${PRODUCTION_INPUTS_LOCK_PATH} is empty`);
    const lockSha256 = sha256Hex(lockBytes);
    if (!contract.packages.payloads.includes(PAYLOAD_TARGET.package)) {
        fail(`${RELEASE_CONTRACT_PATH}: packages.payloads must include ${PAYLOAD_TARGET.package}`);
    }
    const packageJsonPath = `${PAYLOAD_TARGET.dir}/package.json`;
    const pkg = readJson(rootDir, packageJsonPath);
    if (!isRecord(pkg) || pkg.version !== contract.release.version) {
        fail(`${packageJsonPath}: version must be the release version ${contract.release.version}`);
    }
    return { contract, contractSha256, lockSha256 };
}

export function platformFloorFor(contract: ReleaseContract, target: string): PlatformFloor {
    const platform = contract.platforms.supported.find((entry) => entry.target === target);
    if (platform === undefined) fail(`unknown target ${target}`);
    return {
        kernel_min: platform.kernel_min,
        glibc_min: platform.glibc_min,
        procfs_self_fd_exec: platform.capabilities.procfs_self_fd_exec,
    };
}

function synapseFor(contract: ReleaseContract, target: string): string {
    const platform = contract.platforms.supported.find((entry) => entry.target === target);
    if (platform === undefined) fail(`unknown target ${target}`);
    return platform.synapse;
}

function assertExactKeys(
    value: unknown,
    keys: readonly string[],
    where: string,
): asserts value is Record<string, unknown> {
    if (!isRecord(value)) fail(`${where} must be an object`);
    for (const key of Object.keys(value)) {
        if (!keys.includes(key)) fail(`${where}: unknown key ${key}`);
    }
    for (const key of keys) {
        if (!(key in value)) fail(`${where}: missing key ${key}`);
    }
}

export function assertSafePayloadPath(path: unknown): asserts path is string {
    if (typeof path !== "string" || path.length === 0 || path.length > 512) {
        fail(`unsafe payload path ${JSON.stringify(path)}`);
    }
    if (path.includes("\\") || path.includes("\0")) {
        fail(`unsafe payload path ${JSON.stringify(path)}`);
    }
    const segments = path.split("/");
    if (segments[0] !== "payload" || segments.length < 2) {
        fail(`payload path must be payload-rooted: ${JSON.stringify(path)}`);
    }
    for (const segment of segments) {
        if (
            segment === "" ||
            segment === "." ||
            segment === ".." ||
            !PATH_SEGMENT_RE.test(segment)
        ) {
            fail(`unsafe payload path segment in ${JSON.stringify(path)}`);
        }
    }
}

/** The packager requires exact key sets at every level and an addon entry. */
export function validatePayloadManifest(
    manifest: unknown,
    context: ReleaseContext,
): PayloadManifest {
    assertExactKeys(
        manifest,
        [
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
        ],
        "payload manifest",
    );
    const { contract } = context;
    if (manifest.schema !== MANIFEST_SCHEMA) fail("unknown payload-manifest schema");
    assertExactKeys(manifest.release, ["id", "version"], "release");
    if (
        manifest.release.id !== contract.release.id ||
        manifest.release.version !== contract.release.version
    ) {
        fail("payload manifest must bind the current release identity");
    }
    if (manifest.release_contract_sha256 !== context.contractSha256) {
        fail("payload manifest cites a stale release contract digest");
    }
    if (manifest.production_inputs_lock_sha256 !== context.lockSha256) {
        fail("payload manifest cites a stale production-inputs lock digest");
    }
    if (manifest.mode !== "development") fail("payload manifest mode must be development");
    assertExactKeys(manifest.package, ["name", "version", "target"], "package");
    if (
        manifest.package.name !== PAYLOAD_TARGET.package ||
        manifest.package.version !== contract.release.version ||
        manifest.package.target !== PAYLOAD_TARGET.target
    ) {
        fail(`payload manifest package identity mismatch for ${PAYLOAD_TARGET.package}`);
    }
    assertExactKeys(
        manifest.platform_floor,
        ["kernel_min", "glibc_min", "procfs_self_fd_exec"],
        "platform_floor",
    );
    const floor = platformFloorFor(contract, PAYLOAD_TARGET.target);
    if (canonicalJson(manifest.platform_floor) !== canonicalJson(floor)) {
        fail(`payload manifest platform floor drift for ${PAYLOAD_TARGET.target}`);
    }
    if (manifest.synapse !== synapseFor(contract, PAYLOAD_TARGET.target)) {
        fail(`payload manifest synapse claim mismatch for ${PAYLOAD_TARGET.target}`);
    }
    if (manifest.launcher !== LAUNCHER_PATH) {
        fail(`payload manifest launcher must be ${LAUNCHER_PATH}`);
    }
    const files = manifest.files;
    if (!Array.isArray(files) || files.length === 0) {
        fail("payload manifest must list at least the launcher");
    }
    let previous: string | undefined;
    let launcherSeen = false;
    let addonSeen = false;
    for (const [index, entry] of files.entries()) {
        const where = `files[${index}]`;
        assertExactKeys(entry, ["path", "type", "size", "mode", "sha256"], where);
        assertSafePayloadPath(entry.path);
        if (previous !== undefined && previous >= entry.path) {
            fail("payload files must be strictly ascending by path");
        }
        previous = entry.path;
        if (entry.type !== "file") fail(`${where}: only regular files are allowed`);
        if (!Number.isSafeInteger(entry.size) || (entry.size as number) <= 0) {
            fail(`${where}: size must be a positive integer`);
        }
        const expectedMode = entry.path === LAUNCHER_PATH ? "755" : "644";
        if (entry.mode !== expectedMode) fail(`${where}: mode must be ${expectedMode}`);
        if (typeof entry.sha256 !== "string" || !SHA256_RE.test(entry.sha256)) {
            fail(`${where}: sha256 must be a lowercase 64-hex digest`);
        }
        if (entry.path === LAUNCHER_PATH) launcherSeen = true;
        if (entry.path === ADDON_PATH) addonSeen = true;
    }
    if (!launcherSeen) fail(`payload manifest must list the launcher ${LAUNCHER_PATH}`);
    if (!addonSeen) fail(`payload manifest must list the addon ${ADDON_PATH}`);
    return manifest as unknown as PayloadManifest;
}

function lstatIfPresent(path: string): ReturnType<typeof lstatSync> | undefined {
    try {
        return lstatSync(path);
    } catch {
        return undefined;
    }
}

/** Whether a payload root exists at `dir/payload`. A symlink or non-directory there is rejected because every per-file stat below would follow it. */
function payloadRootPresent(dir: string): boolean {
    const stat = lstatIfPresent(join(dir, "payload"));
    if (stat === undefined) return false;
    if (stat.isSymbolicLink()) fail("payload root must not be a symlink");
    if (!stat.isDirectory()) fail("payload root must be a directory");
    return true;
}

/** Whether a package file exists at `path`. npm omits symlinks from the tarball, so anything other than a regular file there ships as absent. */
function regularFilePresent(path: string, what: string): boolean {
    const stat = lstatIfPresent(path);
    if (stat === undefined) return false;
    if (!stat.isFile()) fail(`${what} must be a regular file`);
    return true;
}

export function verifyPayloadDir(dir: string, manifest: PayloadManifest): void {
    if (!payloadRootPresent(dir)) fail("missing payload directory");
    const listed = new Set(manifest.files.map((entry) => entry.path));
    for (const entry of manifest.files) {
        const path = join(dir, entry.path);
        let stat: ReturnType<typeof lstatSync>;
        try {
            stat = lstatSync(path);
        } catch {
            fail(`missing payload file ${entry.path}`);
        }
        if (!stat.isFile()) fail(`${entry.path} is not a regular file`);
        const bytes = readFileSync(path);
        if (bytes.length !== entry.size) {
            fail(`${entry.path}: size drift (${bytes.length} != ${entry.size})`);
        }
        if (sha256Hex(bytes) !== entry.sha256) fail(`${entry.path}: digest drift`);
        const actualMode = stat.mode & 0o777;
        const expectedMode = Number.parseInt(entry.mode, 8);
        if (actualMode !== expectedMode) {
            fail(`${entry.path}: mode drift (${actualMode.toString(8)} != ${entry.mode})`);
        }
    }
    const walk = (relative: string): void => {
        for (const name of readdirSync(join(dir, relative))) {
            const rel = `${relative}/${name}`;
            const stat = lstatSync(join(dir, rel));
            if (stat.isSymbolicLink()) fail(`symlink ${rel} is rejected in a payload`);
            if (stat.isDirectory()) {
                walk(rel);
            } else if (!listed.has(rel)) {
                fail(`unlisted payload file ${rel}`);
            }
        }
    };
    walk("payload");
}

/** `packages/shm-native/index.ts` performs the same two probes at runtime and refuses `"debug"` and any target other than `PAYLOAD_TARGET.nativeTarget`. Callers pass an absolute path: `require` resolves a relative one against this module's directory, not the working directory. */
export function probeAddon(addonPath: string): { profile: string; target: string } {
    const module: unknown = createRequire(import.meta.url)(addonPath);
    if (
        !isRecord(module) ||
        typeof module.buildProfile !== "function" ||
        typeof module.buildTarget !== "function"
    ) {
        fail(`addon ${addonPath} exports no buildProfile and buildTarget functions`);
    }
    return {
        profile: String((module.buildProfile as () => unknown)()),
        target: String((module.buildTarget as () => unknown)()),
    };
}

function readSourceFile(path: string, what: string): Buffer {
    // A FIFO or device would block the read; a symlink would hide the real source.
    if (!lstatSync(path).isFile()) fail(`${what} source must be a regular file`);
    const bytes = readFileSync(path);
    if (bytes.length === 0) fail(`${what} source is empty`);
    return bytes;
}

/** Locally built artifacts share the builder host's libc, which the addon's `buildTarget()` (`OS-ARCH` only) cannot report, and the manifest labels them `linux-x64-gnu`. */
function assertGlibcLinuxX64Host(): void {
    // `@types/node` types the report as `object`; `header.glibcVersionRuntime` is absent on musl.
    const report = process.report?.getReport?.() as
        | { header?: { glibcVersionRuntime?: unknown } }
        | undefined;
    if (
        process.platform !== "linux" ||
        process.arch !== "x64" ||
        typeof report?.header?.glibcVersionRuntime !== "string"
    ) {
        fail(`a ${PAYLOAD_TARGET.target} development payload must be built on x86-64 glibc Linux`);
    }
}

/** A development payload launches only through the daemon's unqualified path, which release builds refuse (`payload_sources` in `eidnara-host.rs`), so only the debug launcher is a candidate. */
function defaultLauncherPath(rootDir: string): string {
    const candidate = join(rootDir, "target", "debug", "eidnara-host");
    if (existsSync(candidate)) return candidate;
    return fail(
        "no locally compiled debug eidnara-host binary found; run " +
            "`cargo build -p daemon --bin eidnara-host --locked` first",
    );
}

function launcherOutput(launcherPath: string, subcommand: string): string {
    const run = spawnSync(launcherPath, [subcommand], { encoding: "utf8", timeout: 10_000 });
    if (run.error !== undefined || run.status !== 0) {
        const detail = run.error?.message ?? (run.stderr.trim() || `exit ${run.status}`);
        fail(`launcher ${launcherPath} failed \`${subcommand}\`: ${detail}`);
    }
    return run.stdout.endsWith("\n") ? run.stdout.slice(0, -1) : run.stdout;
}

/** The launcher's compiled release contract and production-inputs lock must be the ones the manifest cites; a stale or foreign executable fails here rather than at first launch. */
function assertLauncherMatchesRelease(launcherPath: string, context: ReleaseContext): void {
    // `release-info` prints the contract file, whose own trailing newline `release_contract_sha256` excludes.
    const contract = launcherOutput(launcherPath, "release-info").replace(/\n$/, "");
    if (sha256Hex(contract) !== context.contractSha256) {
        fail(`launcher ${launcherPath} was built from a different ${RELEASE_CONTRACT_PATH}`);
    }
    if (launcherOutput(launcherPath, "input-lock-digest") !== context.lockSha256) {
        fail(`launcher ${launcherPath} was built from a different ${PRODUCTION_INPUTS_LOCK_PATH}`);
    }
}

function defaultAddonPath(rootDir: string): string {
    const candidate = join(rootDir, "target", "release", "libshm_native.so");
    if (existsSync(candidate)) return candidate;
    return fail(
        "no locally compiled release shm_native addon found; run " +
            "`bun run --cwd packages/shm-native build:native` first",
    );
}

function stageFile(source: string, destination: string, mode: number): void {
    mkdirSync(dirname(destination), { recursive: true });
    copyFileSync(source, destination);
    chmodSync(destination, mode);
}

export function buildDevPayload(
    rootDir: string,
    options: { outDir: string; launcherPath?: string; addonPath?: string },
): DevPayloadResult {
    assertGlibcLinuxX64Host();
    const context = loadReleaseContext(rootDir);
    // Absolute paths keep `readSourceFile` (working-directory relative) and `probeAddon` (`require`, module-directory relative) reading the same file.
    const launcherPath = resolve(options.launcherPath ?? defaultLauncherPath(rootDir));
    const addonPath = resolve(options.addonPath ?? defaultAddonPath(rootDir));
    if (!existsSync(launcherPath)) fail(`launcher ${launcherPath} does not exist`);
    if (!existsSync(addonPath)) fail(`addon ${addonPath} does not exist`);
    const launcherBytes = readSourceFile(launcherPath, "launcher");
    const addonBytes = readSourceFile(addonPath, "addon");
    assertLauncherMatchesRelease(launcherPath, context);

    const outDir = resolve(options.outDir);
    const payloadDir = join(outDir, "payload");
    const manifestPath = join(outDir, MANIFEST_FILE_NAME);
    // Removing the output tree prevents files absent from the manifest from surviving.
    rmSync(payloadDir, { recursive: true, force: true });
    rmSync(manifestPath, { force: true });
    mkdirSync(outDir, { recursive: true });

    const launcherDest = join(outDir, LAUNCHER_PATH);
    const addonDest = join(outDir, ADDON_PATH);
    try {
        stageFile(launcherPath, launcherDest, 0o755);
        stageFile(addonPath, addonDest, 0o644);
        // Bun's `require` dispatches to the native-addon loader only for a `.node` extension, so a native source is probed through its staged copy, the file consumers load.
        const probePath = NATIVE_ADDON_EXTENSIONS.has(extname(addonPath)) ? addonDest : addonPath;
        const { profile, target } = probeAddon(probePath);
        if (profile !== "release") {
            fail(`dev payload requires a release-profile addon; ${addonPath} reports ${profile}`);
        }
        if (target !== PAYLOAD_TARGET.nativeTarget) {
            fail(
                `dev payload requires a ${PAYLOAD_TARGET.nativeTarget} addon; ` +
                    `${addonPath} reports ${target}`,
            );
        }
    } catch (error) {
        rmSync(payloadDir, { recursive: true, force: true });
        throw error;
    }

    const launcherSha256 = sha256Hex(launcherBytes);
    const addonSha256 = sha256Hex(addonBytes);
    const entries: PayloadFileEntry[] = [
        {
            path: LAUNCHER_PATH,
            type: "file",
            size: launcherBytes.length,
            mode: "755",
            sha256: launcherSha256,
        },
        {
            path: ADDON_PATH,
            type: "file",
            size: addonBytes.length,
            mode: "644",
            sha256: addonSha256,
        },
    ];
    const files = entries.sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
    const candidate: PayloadManifest = {
        schema: MANIFEST_SCHEMA,
        release: {
            id: context.contract.release.id,
            version: context.contract.release.version,
        },
        release_contract_sha256: context.contractSha256,
        production_inputs_lock_sha256: context.lockSha256,
        mode: "development",
        package: {
            name: PAYLOAD_TARGET.package,
            version: context.contract.release.version,
            target: PAYLOAD_TARGET.target,
        },
        platform_floor: platformFloorFor(context.contract, PAYLOAD_TARGET.target),
        synapse: synapseFor(context.contract, PAYLOAD_TARGET.target),
        launcher: LAUNCHER_PATH,
        files,
    };
    const manifest = validatePayloadManifest(candidate, context);
    verifyPayloadDir(outDir, manifest);
    writeFileSync(manifestPath, `${canonicalJson(manifest)}\n`);
    return {
        outDir,
        manifestPath,
        manifest,
        digest: payloadManifestDigest(manifest),
        launcherSha256,
        addonSha256,
    };
}

function sameStringSet(actual: unknown, expected: readonly string[]): boolean {
    return (
        Array.isArray(actual) &&
        actual.length === expected.length &&
        canonicalJson([...actual].sort()) === canonicalJson([...expected].sort())
    );
}

export function validatePayloadPackageDir(rootDir: string): void {
    const context = loadReleaseContext(rootDir);
    const packageDir = join(rootDir, PAYLOAD_TARGET.dir);
    const where = `${PAYLOAD_TARGET.dir}/package.json`;
    regularFilePresent(join(packageDir, "package.json"), where);
    const pkg = readJson(rootDir, where);
    if (!isRecord(pkg)) fail(`${where} must be an object`);
    if (pkg.name !== PAYLOAD_TARGET.package)
        fail(`${where}: name must be ${PAYLOAD_TARGET.package}`);
    if (pkg.version !== context.contract.release.version) {
        fail(`${where}: version must be the release version ${context.contract.release.version}`);
    }
    for (const field of ["os", "cpu", "libc"] as const) {
        if (canonicalJson(pkg[field]) !== canonicalJson(PAYLOAD_TARGET[field])) {
            fail(`${where}: ${field} must be ${JSON.stringify(PAYLOAD_TARGET[field])}`);
        }
    }
    if (!sameStringSet(pkg.files, PACKAGE_FILES)) {
        fail(`${where}: files must be exactly ${JSON.stringify(PACKAGE_FILES)}`);
    }
    // Install filtering only: a payload package carries no lifecycle scripts and pulls nothing.
    for (const field of FORBIDDEN_PACKAGE_FIELDS) {
        if (field in pkg) fail(`${where}: ${field} is not allowed`);
    }
    for (const doc of PACKAGE_DOCS) {
        if (lstatIfPresent(join(packageDir, doc))?.isFile() !== true) {
            fail(`${PAYLOAD_TARGET.dir}: missing ${doc}`);
        }
    }
    const manifestPresent = regularFilePresent(
        join(packageDir, MANIFEST_FILE_NAME),
        `${PAYLOAD_TARGET.dir}/${MANIFEST_FILE_NAME}`,
    );
    // npm packs `payload/` whether or not a manifest sits beside it, and `packages/shm-native/index.ts` refuses a package without one, so both must be present or both absent.
    if (payloadRootPresent(packageDir) && !manifestPresent) {
        fail(`${MANIFEST_FILE_NAME} is missing but a payload directory is staged`);
    }
    if (manifestPresent) {
        const manifest = validatePayloadManifest(
            readJson(rootDir, `${PAYLOAD_TARGET.dir}/${MANIFEST_FILE_NAME}`),
            context,
        );
        verifyPayloadDir(packageDir, manifest);
    }
}

const USAGE =
    "usage: bun scripts/build-host-payload.ts --dev [--out <dir>] [--launcher <path>] [--addon <path>]\n" +
    "       bun scripts/build-host-payload.ts --check";

function usageError(message: string): never {
    console.error(message);
    console.error(USAGE);
    process.exit(2);
}

function main(): void {
    const args = process.argv.slice(2);
    const flags = new Set<string>();
    const values: { out?: string; launcher?: string; addon?: string } = {};
    for (let i = 0; i < args.length; i += 1) {
        const arg = args[i];
        if (arg === "--out" || arg === "--launcher" || arg === "--addon") {
            const value = args[i + 1];
            // A path that begins with `--` is passed as `./--name`.
            if (value === undefined || value.startsWith("--")) {
                usageError(`${arg} requires a path`);
            }
            values[arg.slice(2) as keyof typeof values] = value;
            i += 1;
        } else if (arg === "--check" || arg === "--dev") {
            flags.add(arg);
        } else {
            usageError(`unknown argument: ${arg}`);
        }
    }
    if (flags.size !== 1) usageError("exactly one of --dev or --check is required");
    if (flags.has("--check") && Object.keys(values).length > 0) {
        usageError("--out, --launcher, and --addon apply to --dev only");
    }
    if (values.addon !== undefined && !NATIVE_ADDON_EXTENSIONS.has(extname(values.addon))) {
        usageError(`--addon must name a ${[...NATIVE_ADDON_EXTENSIONS].join(" or ")} file`);
    }
    const rootDir = join(dirname(fileURLToPath(import.meta.url)), "..");
    try {
        if (flags.has("--check")) {
            validatePayloadPackageDir(rootDir);
            console.log(
                `checked ${PAYLOAD_TARGET.dir} against the release contract ` +
                    `(${RELEASE_CONTRACT_PATH}, ${PRODUCTION_INPUTS_LOCK_PATH})`,
            );
            return;
        }
        const result = buildDevPayload(rootDir, {
            outDir: values.out ?? join(rootDir, PAYLOAD_TARGET.dir),
            ...(values.launcher === undefined ? {} : { launcherPath: values.launcher }),
            ...(values.addon === undefined ? {} : { addonPath: values.addon }),
        });
        console.log(
            `built development payload at ${result.outDir} ` +
                `(payload-manifest digest ${result.digest}, launcher sha256 ${result.launcherSha256})`,
        );
    } catch (error) {
        console.error(error instanceof Error ? error.message : String(error));
        process.exit(1);
    }
}

if (import.meta.main) main();
