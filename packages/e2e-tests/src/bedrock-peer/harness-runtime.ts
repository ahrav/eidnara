import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, realpathSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import type { BedrockCredentials } from "./sigv4";

const REPO_ROOT = join(import.meta.dir, "../../../..");
const OPENCODE_MANIFEST = join(
    REPO_ROOT,
    "release/harness-closures/opencode-linux-x64-1.18.22.json",
);
const PI_MANIFEST = join(REPO_ROOT, "release/harness-closures/pi-linux-x64-node-24.18.0.json");
const REDIRECT_EXTENSION = "extensions/bedrock-redirect.mjs";

interface ClosureNode {
    path: string;
    source_root: string;
    source_path: string;
    kind: string;
    mode: number;
    size_bytes: number;
    sha256: string;
    dependencies: Array<{ path: string; kind: string }>;
}

interface ClosureManifest {
    source_roots: string[];
    executable: string | null;
    interpreter: string | null;
    entrypoint: string | null;
    extensions: string[];
    nodes: ClosureNode[];
    version: string;
    package: string;
    [field: string]: unknown;
}

export interface HarnessRuntimeSources {
    /** The directory holding the pinned OpenCode binary as `opencode.exe`. */
    opencodeRoot: string;
    /** The directory holding the pinned Node binary as `node`. */
    nodeRoot: string;
}

const readManifest = (path: string): ClosureManifest =>
    JSON.parse(readFileSync(path, "utf8")) as ClosureManifest;

const sha256File = (path: string): string =>
    createHash("sha256").update(readFileSync(path)).digest("hex");

function onPath(command: string): string | undefined {
    const found = spawnSync("sh", ["-c", `command -v ${command}`], { encoding: "utf8" });
    const path = found.stdout.trim();
    return found.status === 0 && path.length > 0 ? realpathSync(path) : undefined;
}

function pinnedNode(manifest: ClosureManifest, path: string): ClosureNode {
    const node = manifest.nodes.find((candidate) => candidate.path === path);
    if (!node) throw new Error(`release manifest lacks ${path}`);
    return node;
}

/**
 * Finds the pinned OpenCode and Node binaries on PATH, each checked against the hash its
 * release manifest pins. A missing or different binary is a skip reason.
 */
export function detectHarnessRuntimeSources():
    | { ok: true; sources: HarnessRuntimeSources }
    | { ok: false; reason: string } {
    const opencode = readManifest(OPENCODE_MANIFEST);
    const pi = readManifest(PI_MANIFEST);
    const opencodePath = onPath("opencode");
    const nodePath = onPath("node");
    if (!onPath("npm")) return { ok: false, reason: "npm is not on PATH" };
    if (!opencodePath) return { ok: false, reason: "opencode is not on PATH" };
    if (!nodePath) return { ok: false, reason: "node is not on PATH" };
    const opencodeNode = pinnedNode(opencode, opencode.executable as string);
    const opencodeBinary = join(dirname(opencodePath), opencodeNode.source_path);
    if (!existsSync(opencodeBinary) || sha256File(opencodeBinary) !== opencodeNode.sha256) {
        return { ok: false, reason: `opencode on PATH is not the pinned ${opencode.version}` };
    }
    const nodeNode = pinnedNode(pi, pi.interpreter as string);
    if (sha256File(nodePath) !== nodeNode.sha256) {
        return { ok: false, reason: "node on PATH is not the Node runtime the Pi closure pins" };
    }
    return {
        ok: true,
        sources: { opencodeRoot: dirname(opencodeBinary), nodeRoot: dirname(nodePath) },
    };
}

/**
 * An npm install of the pinned Pi package, cached across runs under the system temp directory.
 * The package ships a shrinkwrap, so the install reproduces the release closure's files.
 */
export function ensurePiInstall(): string {
    const pi = readManifest(PI_MANIFEST);
    const root = join(tmpdir(), `eidnara-e2e-pi-install-${pi.version}`);
    const entrypoint = pinnedNode(pi, pi.entrypoint as string);
    const cli = join(root, entrypoint.source_path);
    if (existsSync(cli) && sha256File(cli) === entrypoint.sha256) return root;
    mkdirSync(root, { recursive: true, mode: 0o700 });
    const installed = spawnSync(
        "npm",
        [
            "install",
            "--no-save",
            "--ignore-scripts",
            "--no-audit",
            "--no-fund",
            "--omit=dev",
            "--prefix",
            root,
            `${pi.package}@${pi.version}`,
        ],
        { encoding: "utf8", timeout: 300_000 },
    );
    if (installed.status !== 0 || !existsSync(cli) || sha256File(cli) !== entrypoint.sha256) {
        throw new Error(`npm install of ${pi.package}@${pi.version} failed: ${installed.stderr}`);
    }
    return root;
}

/**
 * Writes a `--harness-runtime` file for `direct_host_fixture`: both release closures verbatim,
 * except that the Pi closure gains one provider extension pointing `amazon-bedrock` at the
 * peer's HTTP/2 listener and `anthropic` at the sentinel. OpenCode's inline config names the
 * same two endpoints. The backends see only `credentials` as envelope rows.
 */
export function writeHarnessRuntime(args: {
    dir: string;
    sources: HarnessRuntimeSources;
    piInstall: string;
    opencodeBaseUrl: string;
    piBaseUrl: string;
    /** `anthropicSentinelUrl` receives any Anthropic request either closure makes. */
    anthropicSentinelUrl: string;
    credentials: BedrockCredentials;
}): string {
    const redirectRoot = join(args.dir, "redirect");
    mkdirSync(redirectRoot, { recursive: true, mode: 0o700 });
    const extensionFile = join(redirectRoot, "bedrock-redirect.mjs");
    writeFileSync(
        extensionFile,
        `export default function (pi) {\n    pi.registerProvider("amazon-bedrock", { baseUrl: ${JSON.stringify(args.piBaseUrl)} });\n    pi.registerProvider("anthropic", { baseUrl: ${JSON.stringify(args.anthropicSentinelUrl)} });\n}\n`,
        { mode: 0o600 },
    );
    const extensionBytes = readFileSync(extensionFile);
    const pi = readManifest(PI_MANIFEST);
    pi.source_roots = [...pi.source_roots, "redirect"].sort();
    pi.extensions = [REDIRECT_EXTENSION];
    pi.nodes = [
        ...pi.nodes,
        {
            path: REDIRECT_EXTENSION,
            source_root: "redirect",
            source_path: "bedrock-redirect.mjs",
            kind: "extension",
            mode: 0o600,
            size_bytes: extensionBytes.length,
            sha256: createHash("sha256").update(extensionBytes).digest("hex"),
            dependencies: [],
        },
    ].sort((left, right) => (left.path < right.path ? -1 : left.path > right.path ? 1 : 0));
    const credentials: Array<[string, string]> = [
        ["AWS_ACCESS_KEY_ID", args.credentials.accessKeyId],
        ["AWS_SECRET_ACCESS_KEY", args.credentials.secretAccessKey],
        ["AWS_REGION", args.credentials.region],
    ];
    if (args.credentials.sessionToken !== undefined) {
        credentials.push(["AWS_SESSION_TOKEN", args.credentials.sessionToken]);
    }
    const path = join(args.dir, "harness-runtime.json");
    writeFileSync(
        path,
        JSON.stringify({
            opencode: {
                manifest: readManifest(OPENCODE_MANIFEST),
                source_roots: { runtime: args.sources.opencodeRoot },
            },
            pi: {
                manifest: pi,
                source_roots: {
                    "pi-install": args.piInstall,
                    redirect: redirectRoot,
                    runtime: args.sources.nodeRoot,
                },
            },
            opencode_provider_base_urls: {
                "amazon-bedrock": args.opencodeBaseUrl,
                anthropic: args.anthropicSentinelUrl,
            },
            credentials,
        }),
        { mode: 0o600 },
    );
    return path;
}
