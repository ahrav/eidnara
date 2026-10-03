import { createHash } from "node:crypto";
import {
    closeSync,
    createReadStream,
    fsyncSync,
    lstatSync,
    mkdirSync,
    openSync,
    readFileSync,
    realpathSync,
    renameSync,
    rmSync,
    writeSync,
} from "node:fs";
import { dirname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { gunzipSync } from "node:zlib";

/** Production inputs the host payload stages, read from `release/production-inputs.lock.json`. */
const LOCK_PATH = "release/production-inputs.lock.json";
const SHA256_RE = /^[0-9a-f]{64}$/;
const REPO_SOURCE_PREFIX = "repo:";
/** Decompressed bytes one locked archive may expand to; the ORT release archive holds about 23 MB. */
const MAX_ARCHIVE_EXPANDED_BYTES = 256 * 1024 * 1024;
/** One download, including redirects, must finish within this many milliseconds. */
const DOWNLOAD_TIMEOUT_MS = 30 * 60 * 1000;
/** A download that delivers no bytes for this many milliseconds is abandoned. */
const DOWNLOAD_IDLE_MS = 60 * 1000;

function fail(message: string): never {
    throw new Error(`host inputs: ${message}`);
}

export interface LockedArchive {
    member: string;
    sha256: string;
    size_bytes: number;
}

/** One lock input: the bytes it names and the payload path they are staged at. */
export interface LockedInput {
    key: string;
    sha256: string;
    size_bytes: number;
    source: string;
    payload_path: string;
    archive?: LockedArchive;
}

interface Expected {
    sha256: string;
    size_bytes: number;
}

export function isRecord(value: unknown): value is Record<string, unknown> {
    return value !== null && typeof value === "object" && !Array.isArray(value);
}

function digestAndSize(value: Record<string, unknown>, where: string): Expected {
    const { sha256, size_bytes } = value;
    if (typeof sha256 !== "string" || !SHA256_RE.test(sha256)) {
        fail(`${where}: sha256 must be a lowercase 64-hex digest`);
    }
    if (typeof size_bytes !== "number" || !Number.isSafeInteger(size_bytes) || size_bytes <= 0) {
        fail(`${where}: size_bytes must be a positive integer`);
    }
    return { sha256, size_bytes };
}

/** Every lock input with its staging path; the lock is the only source of digests, sizes, and locations. */
export function lockedInputs(rootDir: string): LockedInput[] {
    let lock: unknown;
    try {
        lock = JSON.parse(readFileSync(join(rootDir, LOCK_PATH), "utf8"));
    } catch (error) {
        fail(`${LOCK_PATH} is unreadable: ${error instanceof Error ? error.message : error}`);
    }
    if (!isRecord(lock) || !isRecord(lock.inputs)) fail(`${LOCK_PATH}: inputs must be an object`);
    const inputs: LockedInput[] = [];
    const paths = new Set<string>();
    for (const [key, value] of Object.entries(lock.inputs)) {
        const where = `${LOCK_PATH}: inputs.${key}`;
        if (!isRecord(value)) fail(`${where} must be an object`);
        const expected = digestAndSize(value, where);
        const { source, payload_path } = value;
        if (
            typeof source !== "string" ||
            !(source.startsWith("https://") || source.startsWith(REPO_SOURCE_PREFIX))
        ) {
            fail(`${where}: source must be an https URL or a repo: path`);
        }
        if (typeof payload_path !== "string" || !payload_path.startsWith("payload/")) {
            fail(`${where}: payload_path must be payload-rooted`);
        }
        if (paths.has(payload_path)) fail(`${where}: duplicate payload_path`);
        paths.add(payload_path);
        let archive: LockedArchive | undefined;
        if (value.archive !== undefined) {
            if (!isRecord(value.archive) || typeof value.archive.member !== "string") {
                fail(`${where}: archive must name a member`);
            }
            archive = {
                member: value.archive.member,
                ...digestAndSize(value.archive, `${where}.archive`),
            };
        }
        inputs.push({
            key,
            ...expected,
            source,
            payload_path,
            ...(archive === undefined ? {} : { archive }),
        });
    }
    return inputs.sort((a, b) => (a.key < b.key ? -1 : a.key > b.key ? 1 : 0));
}

export function defaultInputsDir(rootDir: string): string {
    return join(rootDir, "target", "host-inputs");
}

/** Cache entries are named by the sha256 of their bytes. */
export function cachePath(inputsDir: string, sha256: string): string {
    return join(resolve(inputsDir), sha256);
}

function hashFile(path: string): Promise<{ size: number; sha256: string }> {
    return new Promise((resolvePromise, reject) => {
        const hash = createHash("sha256");
        let size = 0;
        const stream = createReadStream(path, { highWaterMark: 1024 * 1024 });
        stream.on("data", (chunk) => {
            size += chunk.length;
            hash.update(chunk);
        });
        stream.on("error", reject);
        stream.on("end", () => resolvePromise({ size, sha256: hash.digest("hex") }));
    });
}

/**
 * Proves the cache entry for `expected` holds exactly those bytes and returns its path.
 * A symlink, a non-regular file, or same-name content with another size or digest is refused.
 */
export async function verifiedCacheEntry(
    inputsDir: string,
    expected: Expected,
    what: string,
): Promise<string> {
    const path = cachePath(inputsDir, expected.sha256);
    let stat: ReturnType<typeof lstatSync>;
    try {
        stat = lstatSync(path);
    } catch {
        fail(`${what} is missing from ${inputsDir}; run \`bun run inputs:fetch\``);
    }
    if (!stat.isFile()) fail(`${what} cache entry is not a regular file`);
    if (stat.size !== expected.size_bytes) {
        fail(`${what} cache entry has ${stat.size} bytes; the lock requires ${expected.size_bytes}`);
    }
    const actual = await hashFile(path);
    if (actual.size !== expected.size_bytes || actual.sha256 !== expected.sha256) {
        fail(`${what} cache entry does not match its locked sha256`);
    }
    return path;
}

async function isVerified(inputsDir: string, expected: Expected, what: string): Promise<boolean> {
    return verifiedCacheEntry(inputsDir, expected, what).then(
        () => true,
        () => false,
    );
}

function writeAll(fd: number, bytes: Uint8Array): void {
    let offset = 0;
    while (offset < bytes.length) offset += writeSync(fd, bytes, offset);
}

/**
 * The cache names an entry by its digest only after its size and digest match.
 * Bytes are written to a process-private `.partial-<pid>` name and renamed into place; any failure removes that name, so an interrupted or invalid write is never published.
 */
async function publish(
    inputsDir: string,
    expected: Expected,
    write: (fd: number) => Promise<{ size: number; sha256: string }>,
): Promise<void> {
    mkdirSync(inputsDir, { recursive: true });
    const final = cachePath(inputsDir, expected.sha256);
    const partial = `${final}.partial-${process.pid}`;
    rmSync(partial, { force: true });
    const fd = openSync(partial, "wx", 0o644);
    let open = true;
    try {
        const actual = await write(fd);
        fsyncSync(fd);
        closeSync(fd);
        open = false;
        if (actual.size !== expected.size_bytes) {
            fail(`received ${actual.size} bytes; the lock requires ${expected.size_bytes}`);
        }
        if (actual.sha256 !== expected.sha256) fail("received bytes do not match the locked sha256");
        // A rename replaces a file at the digest name but fails on a directory.
        if (lstatSync(final, { throwIfNoEntry: false })?.isDirectory()) {
            rmSync(final, { recursive: true, force: true });
        }
        renameSync(partial, final);
    } catch (error) {
        if (open) closeSync(fd);
        rmSync(partial, { force: true });
        throw error;
    }
}

function publishBytes(inputsDir: string, expected: Expected, bytes: Buffer): Promise<void> {
    return publish(inputsDir, expected, async (fd) => {
        writeAll(fd, bytes);
        return { size: bytes.length, sha256: createHash("sha256").update(bytes).digest("hex") };
    });
}

export type FetchLike = (url: string, init: { signal: AbortSignal }) => Promise<Response>;

function download(inputsDir: string, url: string, expected: Expected, fetchImpl: FetchLike): Promise<void> {
    return publish(inputsDir, expected, async (fd) => {
        const idle = new AbortController();
        let timer = setTimeout(() => idle.abort(), DOWNLOAD_IDLE_MS);
        const signal = AbortSignal.any([idle.signal, AbortSignal.timeout(DOWNLOAD_TIMEOUT_MS)]);
        try {
            const response = await fetchImpl(url, { signal });
            if (!response.ok || response.body === null) {
                await response.body?.cancel();
                fail(`${url} answered HTTP ${response.status}`);
            }
            const hash = createHash("sha256");
            let size = 0;
            const reader = response.body.getReader();
            for (;;) {
                const { done, value } = await reader.read();
                if (done) break;
                clearTimeout(timer);
                timer = setTimeout(() => idle.abort(), DOWNLOAD_IDLE_MS);
                size += value.length;
                if (size > expected.size_bytes) {
                    await reader.cancel();
                    fail(`${url} sent more than the locked ${expected.size_bytes} bytes`);
                }
                hash.update(value);
                writeAll(fd, value);
            }
            return { size, sha256: hash.digest("hex") };
        } catch (error) {
            if (signal.aborted) {
                fail(`${url} ${idle.signal.aborted ? `sent no bytes for ${DOWNLOAD_IDLE_MS} ms` : "exceeded the download deadline"}`);
            }
            throw error;
        } finally {
            clearTimeout(timer);
        }
    });
}

function tarString(block: Buffer, offset: number, length: number): string {
    const field = block.subarray(offset, offset + length);
    const end = field.indexOf(0);
    return field.subarray(0, end === -1 ? length : end).toString("utf8");
}

/**
 * Returns the bytes of `member` from a gzip-compressed tar archive.
 * Every entry must be a regular file, directory, or symlink whose relative, dot-free path sits under the member's top-level directory; the member must be one regular file that appears once.
 */
export function extractArchiveMember(archive: Buffer, member: string): Buffer {
    const root = member.split("/")[0];
    if (root === undefined || root === "" || !member.includes("/")) {
        fail(`archive member ${member} has no top-level directory`);
    }
    let tar: Buffer;
    try {
        tar = gunzipSync(archive, { maxOutputLength: MAX_ARCHIVE_EXPANDED_BYTES });
    } catch (error) {
        fail(
            `archive does not decompress within ${MAX_ARCHIVE_EXPANDED_BYTES} bytes: ${error instanceof Error ? error.message : error}`,
        );
    }
    let found: Buffer | undefined;
    let offset = 0;
    while (offset + 512 <= tar.length) {
        const header = tar.subarray(offset, offset + 512);
        if (header.every((byte) => byte === 0)) break;
        // POSIX ustar headers carry a name prefix at 345; GNU headers keep timestamps there.
        const prefix = header.toString("latin1", 257, 263) === "ustar\0" ? tarString(header, 345, 155) : "";
        const base = tarString(header, 0, 100);
        const name = (prefix === "" ? base : `${prefix}/${base}`).replace(/\/$/, "");
        const type = String.fromCharCode(header[156] ?? 0);
        const sizeField = tarString(header, 124, 12).trim();
        if (!/^[0-7]+$/.test(sizeField)) fail(`archive entry ${name} has an invalid size`);
        const size = Number.parseInt(sizeField, 8);
        const segments = name.split("/");
        if (
            segments[0] !== root ||
            segments.some((segment) => segment === "" || segment === "." || segment === "..")
        ) {
            fail(`archive entry ${JSON.stringify(name)} is outside ${root}/`);
        }
        if (!["0", "\0", "5", "2"].includes(type)) {
            fail(`archive entry ${name} has unexpected type ${JSON.stringify(type)}`);
        }
        const start = offset + 512;
        if (start + size > tar.length) fail(`archive entry ${name} is truncated`);
        if (name === member) {
            if (type !== "0" && type !== "\0") fail(`archive member ${member} is not a regular file`);
            if (found !== undefined) fail(`archive member ${member} appears twice`);
            found = tar.subarray(start, start + size);
        }
        offset = start + Math.ceil(size / 512) * 512;
    }
    if (found === undefined) fail(`archive has no member ${member}`);
    return found;
}

function readRepoSource(rootDir: string, input: LockedInput): Buffer {
    const root = realpathSync(rootDir);
    let path: string;
    let stat: ReturnType<typeof lstatSync>;
    try {
        path = realpathSync(resolve(root, input.source.slice(REPO_SOURCE_PREFIX.length)));
        stat = lstatSync(path);
    } catch {
        fail(`${input.key}: ${input.source} is missing`);
    }
    const inside = relative(root, path);
    if (inside === "" || inside === ".." || inside.startsWith(`..${sep}`)) {
        fail(`${input.key}: ${input.source} is outside the repository`);
    }
    if (!stat.isFile()) fail(`${input.key}: ${input.source} is not a regular file`);
    if (stat.size !== input.size_bytes) {
        fail(`${input.key}: ${input.source} has ${stat.size} bytes; the lock requires ${input.size_bytes}`);
    }
    return readFileSync(path);
}

/** Fills the cache with every locked input whose entry is absent or wrong; each entry is published only after its size and digest match. */
export async function acquireInputs(
    rootDir: string,
    options: { inputsDir: string; fetchImpl?: FetchLike; inputs?: LockedInput[] },
): Promise<void> {
    const fetchImpl = options.fetchImpl ?? (fetch as FetchLike);
    const inputsDir = resolve(options.inputsDir);
    for (const input of options.inputs ?? lockedInputs(rootDir)) {
        if (await isVerified(inputsDir, input, input.key)) continue;
        if (input.source.startsWith(REPO_SOURCE_PREFIX)) {
            await publishBytes(inputsDir, input, readRepoSource(rootDir, input));
        } else if (input.archive !== undefined) {
            const archive = input.archive;
            if (!(await isVerified(inputsDir, archive, `${input.key} archive`))) {
                await download(inputsDir, input.source, archive, fetchImpl);
            }
            const bytes = readFileSync(cachePath(inputsDir, archive.sha256));
            await publishBytes(inputsDir, input, extractArchiveMember(bytes, archive.member));
        } else {
            await download(inputsDir, input.source, input, fetchImpl);
        }
    }
}

async function main(): Promise<void> {
    const args = process.argv.slice(2);
    const rootDir = join(dirname(fileURLToPath(import.meta.url)), "..");
    let inputsDir = defaultInputsDir(rootDir);
    if (args.length === 2 && args[0] === "--inputs-dir" && args[1] !== undefined) {
        inputsDir = args[1];
    } else if (args.length !== 0) {
        console.error("usage: bun scripts/host-inputs.ts [--inputs-dir <dir>]");
        process.exit(2);
    }
    try {
        await acquireInputs(rootDir, { inputsDir });
        console.log(`verified every locked host input in ${resolve(inputsDir)}`);
    } catch (error) {
        console.error(error instanceof Error ? error.message : String(error));
        process.exit(1);
    }
}

if (import.meta.main) await main();
