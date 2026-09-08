import {
    existsSync,
    mkdirSync,
    mkdtempSync,
    readdirSync,
    readFileSync,
    rmSync,
    statSync,
    writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const rootDir = join(dirname(fileURLToPath(import.meta.url)), "..");
const VERSION = "0.1.0";
const PAYLOAD_PACKAGE = "@eidnara/host-linux-x64-gnu";
const PACKAGE_DIRS: Record<string, string> = {
    "@eidnara/shm-native": "packages/shm-native",
    "@eidnara/opencode": "packages/opencode-plugin",
    "@eidnara/pi": "packages/pi-plugin",
    "@eidnara/cli": "packages/cli",
    [PAYLOAD_PACKAGE]: "packages/host-linux-x64-gnu",
};
const PAYLOAD_FILES = [
    "payload-manifest.json",
    "payload/bin/eidnara-host",
    "payload/native/shm_native.node",
];
const PAYLOAD_TARBALL = ["LICENSE", "NOTICE", "README.md", "package.json", ...PAYLOAD_FILES]
    .map((path) => `package/${path}`)
    .sort();
const TEST_FILE = /\.test\./;
type Pattern = string | RegExp;
const TARBALL_RULES: Record<string, { ships: Pattern[]; omits: RegExp[] }> = {
    "@eidnara/opencode": {
        ships: [
            "package/dist/index.js",
            "package/src/tui/entry.mjs",
            /^package\/src\/tui-compiled\/./,
            "package/src/features/context/defaults.ts",
            /^package\/src\/shared\/./,
            /^package\/src\/config\/./,
            /^package\/src\/agents\/./,
        ],
        omits: [
            /^package\/dist\/tui\//,
            TEST_FILE,
            /\.typecheck\./,
            /^package\/src\/__tests__\//,
            /^package\/test-preload\.ts$/,
        ],
    },
    "@eidnara/pi": {
        ships: ["package/dist/index.js", "package/dist/subagent-entry.js"],
        omits: [TEST_FILE],
    },
    "@eidnara/cli": { ships: ["package/dist/index.js"], omits: [TEST_FILE] },
    "@eidnara/shm-native": {
        ships: ["package/index.js", "package/index.ts", "package/package.json"],
        omits: [],
    },
};
const FORBIDDEN_EVERYWHERE = [
    /\/features\/context\/memory\//,
    /\/storage/,
    /\/dreamer\//,
    /\/embedding/,
];
// Mirrors the `Predecessor tokens` step in `.github/workflows/ci.yml`; the bracketed
// characters keep this file itself from matching that step's `git grep`.
const PREDECESSOR_TOKENS = new RegExp(
    "(^|[^0-9a-z])(c[o]rtex[-_\\s]*kit|magi[c][-_\\s]*context|claustru[m]|m[c]tx|m[c])([^0-9a-z]|$)" +
        "|(^|[^0-9a-z])c[k][-_]" +
        "|(^|[^0-9a-z])(primitives|host)[@][0-9a-f]{7,40}([^0-9a-z]|$)" +
        '|"(source_)?repo"\\s*:\\s*"(primitives|host)"',
    "i",
);
const IMPORT_PROBE =
    'const a = await import("@eidnara/opencode"); const t = await import("@eidnara/opencode/tui"); ' +
    'const p = await import("@eidnara/pi"); console.log(typeof a.default, typeof t, typeof p.default)';
const START_TIMEOUT_MS = 180_000;
const DEFAULT_TIMEOUT_MS = 60_000;

class SmokeFailure extends Error {}

const failures: string[] = [];

/** Prints one line per check; a failure is recorded and stops the run so later steps do not act on broken state. */
function assert(condition: boolean, message: string, detail?: string): asserts condition {
    if (condition) {
        console.log(`  ok  ${message}`);
        return;
    }
    const text = detail === undefined || detail === "" ? message : `${message}: ${detail}`;
    failures.push(text);
    console.log(`FAIL  ${text}`);
    throw new SmokeFailure(text);
}

interface RunOptions {
    cwd?: string;
    env?: Record<string, string>;
    inherit?: boolean;
    timeoutMs?: number;
}

interface RunResult {
    code: number;
    stdout: string;
    stderr: string;
    timedOut: boolean;
}

function run(cmd: string[], options: RunOptions = {}): RunResult {
    const result = Bun.spawnSync({
        cmd,
        cwd: options.cwd ?? rootDir,
        env: options.env ?? scratchEnv(),
        stdin: "ignore",
        stdout: options.inherit ? "inherit" : "pipe",
        stderr: options.inherit ? "inherit" : "pipe",
        timeout: options.timeoutMs ?? DEFAULT_TIMEOUT_MS,
        killSignal: "SIGKILL",
    });
    const timedOut = result.exitedDueToTimeout === true;
    return {
        // A signal exit reports no exit code, so it is folded into a non-zero status.
        code: timedOut || result.signalCode !== undefined ? -1 : result.exitCode,
        stdout: result.stdout?.toString() ?? "",
        stderr: result.stderr?.toString() ?? "",
        timedOut,
    };
}

let tmpRoot = "";

/** Install and run steps see only PATH plus a scratch HOME so user bunfig, npmrc, and XDG state cannot leak in. */
function scratchEnv(): {
    PATH: string;
    HOME: string;
    XDG_DATA_HOME: string;
    XDG_CONFIG_HOME: string;
} {
    const home = join(tmpRoot, "home");
    return {
        PATH: process.env.PATH ?? "",
        HOME: home,
        XDG_DATA_HOME: join(home, ".local", "share"),
        XDG_CONFIG_HOME: join(home, ".config"),
    };
}

function developerEnv(): Record<string, string> {
    const env: Record<string, string> = {};
    for (const [key, value] of Object.entries(process.env)) {
        if (value !== undefined) env[key] = value;
    }
    return env;
}

function describe(result: RunResult): string {
    if (result.timedOut) return "timed out";
    const tail = `${result.stdout}\n${result.stderr}`.trim().split("\n").slice(-12).join("\n");
    return `exit ${result.code}\n${tail}`;
}

function tarballName(name: string): string {
    return `${name.slice(1).replace("/", "-")}-${VERSION}.tgz`;
}

function readJson(path: string): Record<string, unknown> {
    return JSON.parse(readFileSync(path, "utf8")) as Record<string, unknown>;
}

function matches(entry: string, pattern: Pattern): boolean {
    return typeof pattern === "string" ? entry === pattern : pattern.test(entry);
}

function assertEntries(name: string, entries: string[], ships: Pattern[], omits: RegExp[]): void {
    for (const pattern of ships) {
        assert(
            entries.some((entry) => matches(entry, pattern)),
            `${name} ships ${pattern}`,
        );
    }
    for (const pattern of omits) {
        const hits = entries.filter((entry) => pattern.test(entry));
        assert(hits.length === 0, `${name} omits ${pattern}`, hits.slice(0, 5).join(", "));
    }
}

function walkFiles(dir: string, out: string[] = []): string[] {
    for (const name of readdirSync(dir)) {
        const path = join(dir, name);
        if (statSync(path).isDirectory()) walkFiles(path, out);
        else out.push(path);
    }
    return out;
}

function readPiPeerVersions(rootDir: string): Record<string, string> {
    const manifest = JSON.parse(
        readFileSync(join(rootDir, "packages", "pi-plugin", "package.json"), "utf8"),
    ) as { peerDependencies?: Record<string, string>; devDependencies?: Record<string, string> };
    const versions: Record<string, string> = {};
    for (const name of Object.keys(manifest.peerDependencies ?? {})) {
        const pinned = manifest.devDependencies?.[name];
        if (pinned === undefined) throw new Error(`pi-plugin pins no dev version for peer ${name}`);
        versions[name] = pinned;
    }
    return versions;
}

function scanPredecessorTokens(extractedRoot: string): string[] {
    const hits: string[] = [];
    for (const path of walkFiles(extractedRoot)) {
        const rel = relative(extractedRoot, path);
        if (rel.endsWith(".node") || rel.endsWith(".tgz") || rel.includes("/payload/bin/"))
            continue;
        const bytes = readFileSync(path);
        // A NUL byte within the first 8 KiB marks a binary, matching `grep -I`.
        if (bytes.subarray(0, 8192).includes(0)) continue;
        const text = bytes.toString("utf8");
        const before = hits.length;
        for (const [index, line] of text.split("\n").entries()) {
            if (PREDECESSOR_TOKENS.test(line)) {
                hits.push(`${rel}:${index + 1}: ${line.trim().slice(0, 120)}`);
            }
        }
        // The whole-file pass catches a two-word token split across a line break.
        if (hits.length === before && PREDECESSOR_TOKENS.test(text))
            hits.push(`${rel}: token spans lines`);
    }
    return hits;
}

function parseBunLock(path: string): Record<string, unknown[]> {
    const text = readFileSync(path, "utf8").replace(/,(\s*[}\]])/g, "$1");
    const lock = JSON.parse(text) as { packages?: Record<string, unknown[]> };
    return lock.packages ?? {};
}

function daemon(cli: string, project: string, action: string, timeoutMs = DEFAULT_TIMEOUT_MS) {
    const result = run(["node", cli, "daemon", action, "--json"], { cwd: project, timeoutMs });
    let parsed: Record<string, unknown> = {};
    try {
        parsed = JSON.parse(result.stdout.trim()) as Record<string, unknown>;
    } catch {
        parsed = {};
    }
    return { result, parsed };
}

function assertDaemon(
    action: string,
    got: ReturnType<typeof daemon>,
    code: number,
    expected: Record<string, unknown>,
): void {
    const summary = Object.entries(expected)
        .map(([key, value]) => `${key}=${JSON.stringify(value)}`)
        .join(" ");
    assert(
        got.result.code === code &&
            Object.entries(expected).every(([key, value]) => got.parsed[key] === value),
        `eidnara daemon ${action} --json exits ${code} with ${summary}`,
        describe(got.result),
    );
}

function main(): void {
    const keep = process.argv.includes("--keep");
    const payloadDir = join(rootDir, PACKAGE_DIRS[PAYLOAD_PACKAGE] ?? "");
    for (const rel of PAYLOAD_FILES) {
        if (!existsSync(join(payloadDir, rel))) {
            throw new SmokeFailure(
                `missing ${relative(rootDir, join(payloadDir, rel))}; run \`bun run payload:dev\``,
            );
        }
    }
    for (const tool of ["node", "npm"]) {
        if (Bun.which(tool) === null) throw new SmokeFailure(`${tool} is not on PATH`);
    }

    tmpRoot = mkdtempSync(join(tmpdir(), "eidnara-tarball-smoke-"));
    const packsDir = join(tmpRoot, "packs");
    const extractedDir = join(tmpRoot, "extracted");
    const project = join(tmpRoot, "project");
    for (const dir of [packsDir, extractedDir, project, scratchEnv().HOME]) {
        mkdirSync(dir, { recursive: true });
    }
    const cli = join(project, "node_modules", ".bin", "eidnara");
    let started = false;
    let stopped = false;
    try {
        const build = run(["bun", "run", "build"], {
            env: developerEnv(),
            inherit: true,
            timeoutMs: 600_000,
        });
        assert(build.code === 0, "bun run build", describe(build));

        const tarballs: Record<string, string> = {};
        for (const [name, dir] of Object.entries(PACKAGE_DIRS)) {
            const version = readJson(join(rootDir, dir, "package.json")).version;
            assert(version === VERSION, `${name} is version ${VERSION}`, String(version));
            const pack = run(["npm", "pack", "--pack-destination", packsDir, "--silent"], {
                cwd: join(rootDir, dir),
                timeoutMs: 300_000,
            });
            assert(pack.code === 0, `npm pack ${name}`, describe(pack));
            tarballs[name] = join(packsDir, tarballName(name));
        }
        const produced = readdirSync(packsDir).sort();
        const expected = Object.keys(PACKAGE_DIRS).map(tarballName).sort();
        assert(
            JSON.stringify(produced) === JSON.stringify(expected),
            "exactly five tarballs",
            produced.join(", "),
        );

        const listings: Record<string, string[]> = {};
        for (const [name, path] of Object.entries(tarballs)) {
            const listed = run(["tar", "-tzf", path]);
            assert(listed.code === 0, `tar lists ${tarballName(name)}`, describe(listed));
            listings[name] = listed.stdout.split("\n").filter((line) => line.length > 0);
        }
        for (const [name, rule] of Object.entries(TARBALL_RULES)) {
            assertEntries(name, listings[name] ?? [], rule.ships, rule.omits);
        }
        const payloadEntries = [...(listings[PAYLOAD_PACKAGE] ?? [])].sort();
        assert(
            JSON.stringify(payloadEntries) === JSON.stringify(PAYLOAD_TARBALL),
            `${PAYLOAD_PACKAGE} ships exactly the payload files`,
            payloadEntries.join(", "),
        );
        assertEntries("all tarballs", Object.values(listings).flat(), [], FORBIDDEN_EVERYWHERE);

        for (const [name, path] of Object.entries(tarballs)) {
            const dest = join(extractedDir, name.slice("@eidnara/".length));
            mkdirSync(dest, { recursive: true });
            const extract = run(["tar", "-xzf", path, "-C", dest], { timeoutMs: 300_000 });
            assert(extract.code === 0, `extract ${name}`, describe(extract));
        }
        const cliEntry = readFileSync(
            join(extractedDir, "cli", "package", "dist", "index.js"),
            "utf8",
        );
        assert(
            cliEntry.startsWith("#!/usr/bin/env node"),
            "@eidnara/cli dist/index.js starts with a node shebang",
        );
        const tokenHits = scanPredecessorTokens(extractedDir);
        assert(
            tokenHits.length === 0,
            "no predecessor tokens in extracted tarballs",
            `\n${tokenHits.join("\n")}`,
        );

        const fileDeps = Object.fromEntries(
            Object.entries(tarballs).map(([name, path]) => [name, `file:${path}`]),
        );
        // The Pi extension declares its host packages as optional peers because Pi provides them at load time; the smoke project stands in for Pi, so it installs the pinned versions the package develops against.
        const piPeers = readPiPeerVersions(rootDir);
        const manifest = {
            name: "eidnara-smoke",
            private: true,
            type: "module",
            dependencies: { ...fileDeps, ...piPeers },
            overrides: {
                "@eidnara/shm-native": fileDeps["@eidnara/shm-native"],
                [PAYLOAD_PACKAGE]: fileDeps[PAYLOAD_PACKAGE],
            },
        };
        writeFileSync(join(project, "package.json"), `${JSON.stringify(manifest, null, 2)}\n`);
        const install = run(["bun", "install"], { cwd: project, timeoutMs: 600_000 });
        assert(install.code === 0, "bun install from tarballs", describe(install));
        const noisy = `${install.stdout}\n${install.stderr}`
            .split("\n")
            .filter((line) => line.includes("@eidnara/"))
            .filter((line) => /warn/i.test(line) || line.includes("registry.npmjs.org"));
        assert(
            noisy.length === 0,
            "bun install neither warns about nor fetches @eidnara/* from the registry",
            noisy.join("\n"),
        );
        const lockPackages = parseBunLock(join(project, "bun.lock"));
        for (const name of Object.keys(PACKAGE_DIRS)) {
            const resolution = String(lockPackages[name]?.[0] ?? "");
            console.log(`      ${name} -> ${resolution}`);
            assert(
                resolution.includes(".tgz"),
                `${name} resolves to a tarball in bun.lock`,
                resolution,
            );
        }
        const installedPayload = join(project, "node_modules", PAYLOAD_PACKAGE);
        for (const rel of PAYLOAD_FILES) {
            assert(existsSync(join(installedPayload, rel)), `installed ${PAYLOAD_PACKAGE}/${rel}`);
        }
        const launcherMode =
            statSync(join(installedPayload, "payload/bin/eidnara-host")).mode & 0o777;
        assert(launcherMode === 0o755, "installed launcher mode is 0755", launcherMode.toString(8));

        const imports = run(["bun", "-e", IMPORT_PROBE], { cwd: project });
        assert(
            imports.code === 0,
            `bun imports the installed plugins (${imports.stdout.trim()})`,
            describe(imports),
        );

        const version = run(["node", cli, "--version"], { cwd: project });
        assert(
            version.code === 0 && version.stdout.trim() === VERSION,
            `eidnara --version prints ${VERSION}`,
            describe(version),
        );
        const start = daemon(cli, project, "start", START_TIMEOUT_MS);
        started = start.result.code === 0;
        assertDaemon("start", start, 0, {
            schema: "eidnara.daemon/v1",
            command: "start",
            ok: true,
            state: "running",
        });
        const connection = join(scratchEnv().XDG_DATA_HOME, "eidnara", "run", "connection.json");
        assert(existsSync(connection), `${relative(tmpRoot, connection)} exists`);
        assertDaemon("status", daemon(cli, project, "status"), 0, {
            ok: true,
            state: "running",
            command: "status",
        });
        const stop = daemon(cli, project, "stop");
        stopped = stop.result.code === 0;
        assertDaemon("stop", stop, 0, { ok: true, state: "stopped", command: "stop" });
        assertDaemon("status", daemon(cli, project, "status"), 1, {
            ok: false,
            state: "stopped",
            reason: "not_running",
        });
    } finally {
        if (started && !stopped) run(["node", cli, "daemon", "stop", "--json"], { cwd: project });
        if (keep) console.log(`kept ${tmpRoot}`);
        else rmSync(tmpRoot, { recursive: true, force: true });
    }
}

try {
    main();
    console.log("tarball smoke: ok");
} catch (error) {
    if (!(error instanceof SmokeFailure)) {
        failures.push(error instanceof Error ? (error.stack ?? error.message) : String(error));
    }
    console.error(`tarball smoke: ${failures.length} check(s) failed`);
    for (const failure of failures) console.error(`  - ${failure}`);
    process.exit(1);
}
