/**
 * The Pi runner creates an isolated XDG/Pi home, locates the Pi CLI, and writes the configuration
 * files a Pi process needs to load the built Eidnara extension against the mock provider.
 */

import {
    existsSync,
    mkdirSync,
    readdirSync,
    realpathSync,
    symlinkSync,
    writeFileSync,
} from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { userEidnaraConfigPath } from "../opencode-runner/spawn";

export const REPO_ROOT = resolve(import.meta.dir, "../../../..");
export const PI_PLUGIN_ROOT = join(REPO_ROOT, "packages/pi-plugin");
export const PI_PLUGIN_ENTRY = join(PI_PLUGIN_ROOT, "dist", "index.js");
export const PI_PLUGIN_BUILD_HINT = `${PI_PLUGIN_ENTRY} is missing. Run: bun run --cwd packages/pi-plugin build`;

/**
 * `@earendil-works/pi-coding-agent` is a devDependency of `packages/pi-plugin`, so resolution starts
 * from that package's `node_modules`; Bun's isolated layout keeps the real files under the root
 * `node_modules/.bun`, which is the fallback when the workspace symlink is absent.
 */
const piPluginRequire = createRequire(join(PI_PLUGIN_ROOT, "package.json"));

function compareSemver(a: string, b: string): number {
    const left = a.split(".").map((part) => Number(part));
    const right = b.split(".").map((part) => Number(part));
    for (let i = 0; i < Math.max(left.length, right.length); i++) {
        const diff = (left[i] ?? 0) - (right[i] ?? 0);
        if (diff !== 0) return diff;
    }
    return 0;
}

function resolvePiPackageJson(): string | null {
    try {
        return piPluginRequire.resolve("@earendil-works/pi-coding-agent/package.json");
    } catch {
        const bunModules = join(REPO_ROOT, "node_modules/.bun");
        if (!existsSync(bunModules)) return null;
        const prefix = "@earendil-works+pi-coding-agent@";
        const candidates = readdirSync(bunModules, { withFileTypes: true })
            .filter((entry) => entry.isDirectory() && entry.name.startsWith(prefix))
            .map((entry) => {
                const version = entry.name.slice(prefix.length).split("+")[0] ?? "0.0.0";
                return { name: entry.name, version };
            })
            .sort((a, b) => compareSemver(b.version, a.version));
        const best = candidates[0];
        if (best === undefined) return null;
        const candidate = join(
            bunModules,
            best.name,
            "node_modules/@earendil-works/pi-coding-agent/package.json",
        );
        return existsSync(candidate) ? candidate : null;
    }
}

/** `null` when `@earendil-works/pi-coding-agent` is not installed; `detectPiPrereqs` reports it. */
export const PI_PACKAGE_JSON = resolvePiPackageJson();
export const PI_CLI =
    PI_PACKAGE_JSON === null ? null : join(dirname(PI_PACKAGE_JSON), "dist/cli.js");

export interface PiPrereqs {
    ok: boolean;
    skipReason?: string;
}

/**
 * Pi's CLI is a Node ESM entrypoint (`engines.node`), so the runner spawns it with `node` rather
 * than the Bun test process; the built extension is required because Pi loads `dist/index.js`.
 */
export function detectPiPrereqs(): PiPrereqs {
    const missing: string[] = [];
    if (PI_CLI === null) {
        missing.push(
            "@earendil-works/pi-coding-agent is not installed under packages/pi-plugin/node_modules (run bun install)",
        );
    }
    if (Bun.which("node") === null) missing.push("node is not on PATH");
    if (!existsSync(PI_PLUGIN_ENTRY)) missing.push(PI_PLUGIN_BUILD_HINT);
    return missing.length === 0 ? { ok: true } : { ok: false, skipReason: missing.join("; ") };
}

export interface PiIsolatedEnv {
    baseDir: string;
    configDir: string;
    dataDir: string;
    cacheDir: string;
    workdir: string;
    agentDir: string;
    pluginDir: string;
}

export interface PiRunResult {
    sessionId: string | null;
    events: Array<Record<string, unknown>>;
    stdout: string;
    stderr: string;
    exitCode: number | null;
    signalCode: NodeJS.Signals | null;
}

export interface PiRunnerOptions {
    mockProviderURL: string;
    env?: PiIsolatedEnv;
    eidnaraConfig?: Record<string, unknown>;
    piSettingsExtra?: Record<string, unknown>;
    modelContextLimit?: number;
}

export function createPiIsolatedEnv(): PiIsolatedEnv {
    const unique = `pi-e2e-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
    const baseDirRaw = join(tmpdir(), unique);
    mkdirSync(baseDirRaw, { recursive: true });
    const baseDir = realpathSync(baseDirRaw);
    const configDir = join(baseDir, "config");
    const dataDir = join(baseDir, "data");
    const cacheDir = join(baseDir, "cache");
    const workdir = join(baseDir, "work");
    const agentDir = join(baseDir, ".pi", "agent");
    const pluginDir = join(agentDir, "extensions", "eidnara-pi");
    for (const d of [
        configDir,
        dataDir,
        cacheDir,
        workdir,
        agentDir,
        join(agentDir, "extensions"),
    ]) {
        mkdirSync(d, { recursive: true });
    }

    // Real paths prevent `/var` and `/private/var` identity drift on macOS.
    return {
        baseDir: realpathSync(baseDir),
        configDir: realpathSync(configDir),
        dataDir: realpathSync(dataDir),
        cacheDir: realpathSync(cacheDir),
        workdir: realpathSync(workdir),
        agentDir: realpathSync(agentDir),
        pluginDir,
    };
}

/**
 * Pi requires each `settings.packages` entry to be a package directory whose `package.json`
 * declares `pi.extensions`; `packages/pi-plugin` is that directory, so the isolated home links to it.
 */
export function ensurePluginAvailable(env: PiIsolatedEnv): void {
    if (!existsSync(PI_PLUGIN_ENTRY)) throw new Error(PI_PLUGIN_BUILD_HINT);
    if (!existsSync(env.pluginDir)) symlinkSync(PI_PLUGIN_ROOT, env.pluginDir, "dir");
}

/**
 * Writes Pi `settings.json` and `models.json` under the Pi agent directory and the user-tier
 * Eidnara config at the path the extension's own resolver yields under the child's
 * `XDG_CONFIG_HOME`. The Eidnara defaults use small thresholds so tests reach the execute path quickly.
 */
export function writeConfigs(env: PiIsolatedEnv, opts: PiRunnerOptions): void {
    ensurePluginAvailable(env);

    const settings = {
        packages: [env.pluginDir],
        defaultProvider: "anthropic",
        defaultModel: "claude-haiku-4-5",
        enabledModels: ["anthropic/claude-haiku-4-5"],
        compaction: { enabled: false },
        retry: { enabled: false },
        quietStartup: true,
        enableInstallTelemetry: false,
        ...(opts.piSettingsExtra ?? {}),
    };
    writeFileSync(join(env.agentDir, "settings.json"), JSON.stringify(settings, null, 2));

    const models = {
        providers: {
            anthropic: {
                baseUrl: opts.mockProviderURL,
                apiKey: "test-key-not-real",
                modelOverrides: {
                    "claude-haiku-4-5": {
                        contextWindow: opts.modelContextLimit ?? 200000,
                        maxTokens: 8192,
                        reasoning: false,
                    },
                },
            },
        },
    };
    writeFileSync(join(env.agentDir, "models.json"), JSON.stringify(models, null, 2));

    const eidnara = {
        $schema: "https://raw.githubusercontent.com/ahrav/eidnara/main/assets/eidnara.schema.json",
        enabled: true,
        protected_tags: 1,
        execute_threshold_percentage: 40,
        history_budget_percentage: 0.15,
        memory: {
            enabled: true,
            auto_promote: false,
            auto_search: { enabled: false },
            git_commit_indexing: { enabled: false },
        },
        sidekick: { disable: true },
        ...(opts.eidnaraConfig ?? {}),
    };
    const userConfigPath = userEidnaraConfigPath(env);
    mkdirSync(dirname(userConfigPath), { recursive: true });
    writeFileSync(userConfigPath, JSON.stringify(eidnara, null, 2));
}

export function childEnv(env: PiIsolatedEnv): Record<string, string> {
    const result: Record<string, string> = {};
    for (const [key, value] of Object.entries(process.env)) {
        if (value === undefined) continue;
        if (key === "NODE_ENV") continue;
        result[key] = value;
    }
    result.PI_CODING_AGENT_DIR = env.agentDir;
    result.HOME = env.baseDir;
    result.XDG_CONFIG_HOME = env.configDir;
    result.XDG_DATA_HOME = env.dataDir;
    result.XDG_CACHE_HOME = env.cacheDir;
    result.ANTHROPIC_API_KEY = "test-key-not-real";
    result.PI_OFFLINE = "1";
    result.PI_SKIP_VERSION_CHECK = "1";
    return result;
}
