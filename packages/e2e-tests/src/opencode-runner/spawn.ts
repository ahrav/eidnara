/**
 * Spawns `opencode serve` against an isolated config, data, and cache root so a run never touches
 * the user's own OpenCode state, and writes the plugin, provider, and Eidnara configs it reads.
 */

import { type ChildProcess, spawn } from "node:child_process";
import { existsSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { resolveEidnaraUserConfigPath } from "@eidnara/opencode/config/config-paths";
import { isSecretKey } from "@eidnara/opencode/shared/redaction";
import { waitForChildExit } from "../process-exit";
import {
    buildDirectHostFixture,
    detectRustModePrereqs,
    HermeticHostStack,
} from "../rust-runner/hermetic-host";
import { isSensitiveEnvKey } from "../secret-env-keys";

const REPO_ROOT = resolve(import.meta.dir, "../../../..");
/** OpenCode loads the built bundle: loading `src/index.ts` can delay startup enough to exhaust readiness polling on slow CI. */
const PLUGIN_DIST_ENTRY = join(REPO_ROOT, "packages/opencode-plugin/dist/index.js");

/**
 * Resolved at spawn time so a bundle built after this module was imported is selected.
 * A missing bundle is refused here rather than surfacing as an opaque plugin-load failure inside `opencode serve`.
 */
export function pluginEntryPath(): string {
    if (!existsSync(PLUGIN_DIST_ENTRY)) {
        throw new Error(
            `plugin bundle missing at ${PLUGIN_DIST_ENTRY}; run \`bun run --cwd packages/opencode-plugin build\``,
        );
    }
    return PLUGIN_DIST_ENTRY;
}

export interface IsolatedEnv {
    configDir: string;
    dataDir: string;
    cacheDir: string;
    workdir: string;
}

/**
 * `ServeHostname` permits only addresses reachable at `http://127.0.0.1:${port}`.
 *
 * Otherwise readiness polling times out because the fixed client URL cannot reach the listener.
 * Adding another address requires deriving the client URL from it, including IPv6 brackets.
 */
export type ServeHostname = "0.0.0.0" | "127.0.0.1";

export interface SpawnedOpencode {
    url: string;
    port: number;
    env: IsolatedEnv;
    kill: () => Promise<void>;
    stdout: () => string;
    stderr: () => string;
    /** Direct host fixture provisioned when the caller supplied no connection file. */
    hostStack?: HermeticHostStack;
}

export interface SpawnOptions {
    mockProviderURL: string;
    /** Port for opencode serve. Default: random available */
    port?: number;
    eidnaraConfig?: Record<string, unknown>;
    /** Extra opencode.json provider/model config, merged with defaults. */
    openCodeConfigExtra?: Record<string, unknown>;
    /** Override the mock model's context token limit. Default 200000. */
    modelContextLimit?: number;
    /** Reuse an isolated env so direct host starts before OpenCode and survives serve restarts. */
    existingEnv?: IsolatedEnv;
    /**
     * User-tier host connection file. When set, the user config carries `subc.connection_file`
     * and `transform_mode: "rust"`, and the project config selects `transform_mode: "rust"`.
     */
    userHostConnectionFile?: string;
    /** `projectEidnaraConfig` is written to `<workdir>/.eidnara/eidnara.jsonc` when set. */
    projectEidnaraConfig?: Record<string, unknown>;
    /**
     * extraEnv overrides inherited environment variables.
     */
    extraEnv?: Record<string, string>;
    /**
     * hostname defaults to "0.0.0.0".
     * Bind to `0.0.0.0` so readiness polling can reach the server.
     * The serve HTTP API is unauthenticated.
     * Spawns with real child-environment credentials must use "127.0.0.1".
     * Using "127.0.0.1" keeps the unauthenticated API off non-loopback interfaces.
     */
    hostname?: ServeHostname;
    /**
     * allowSecretEnvOffLoopback permits non-loopback serving only for fake fixture credentials.
     */
    allowSecretEnvOffLoopback?: boolean;
}

async function pickFreePort(): Promise<number> {
    const server = Bun.serve({ port: 0, fetch: () => new Response() });
    const port: number = server.port ?? 0;
    server.stop(true);
    if (!port) throw new Error("could not allocate a free port");
    return port;
}

/**
 * The direct host needs dataDir before OpenCode starts to publish its connection file.
 * Reusing the environment preserves opencode.db and the module store across serve restarts.
 */
export function createIsolatedEnv(): IsolatedEnv {
    const unique = `opencode-e2e-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
    const base = join(tmpdir(), unique);
    const configDir = join(base, "config");
    const dataDir = join(base, "data");
    const cacheDir = join(base, "cache");
    const workdir = join(base, "work");
    for (const d of [configDir, dataDir, cacheDir, workdir]) {
        mkdirSync(d, { recursive: true });
    }
    return { configDir, dataDir, cacheDir, workdir };
}

/**
 * The child runs with `XDG_CONFIG_HOME=env.configDir`, so the plugin's own path resolver yields the
 * user-tier file the loader reads; a hand-built path would drift from it silently and the loader
 * would fall back to ts with only a warning.
 */
export function userEidnaraConfigPath(env: IsolatedEnv): string {
    const previous = process.env.XDG_CONFIG_HOME;
    process.env.XDG_CONFIG_HOME = env.configDir;
    try {
        return resolveEidnaraUserConfigPath();
    } finally {
        if (previous === undefined) delete process.env.XDG_CONFIG_HOME;
        else process.env.XDG_CONFIG_HOME = previous;
    }
}

/**
 * Writes `opencode.json`, the user-tier `eidnara.jsonc`, and the project `.eidnara/eidnara.jsonc`.
 * The eidnara defaults use small thresholds so tests reach the transform's execute path quickly.
 */
function writeConfigs(env: IsolatedEnv, mockProviderURL: string, opts: SpawnOptions): void {
    const pluginSpec = `file://${pluginEntryPath()}`;
    /** Every caller-supplied config channel is written to disk beside the others, and all three are `Record<string, unknown>` — an easy mix-up — so each is guarded rather than only the one an unauthenticated serve reads. */
    const extra = canonicalConfig(opts.openCodeConfigExtra, "openCodeConfigExtra") ?? {};
    const eidnaraConfig = canonicalConfig(opts.eidnaraConfig, "eidnaraConfig");
    const projectEidnaraConfig = canonicalConfig(opts.projectEidnaraConfig, "projectEidnaraConfig");
    const contributedProviders = extra.provider;
    const extraWithoutProvider = { ...extra };
    delete extraWithoutProvider.provider;

    const opencodeConfig: Record<string, unknown> = {
        $schema: "https://opencode.ai/config.json",
        plugin: [pluginSpec],
        // `autoupdate: false` disables telemetry-style checks that make network requests.
        autoupdate: false,
        // OpenCode enables compaction by default; Eidnara disables its conflict detector when compaction is enabled.
        // When compaction is enabled, Eidnara disables its conflict detector and the plugin does nothing.
        compaction: { auto: false, prune: false },
        provider: {
            ...(contributedProviders &&
            typeof contributedProviders === "object" &&
            !Array.isArray(contributedProviders)
                ? contributedProviders
                : {}),
            "mock-anthropic": {
                api: "@ai-sdk/anthropic",
                name: "Mock Anthropic",
                npm: "@ai-sdk/anthropic",
                env: [],
                options: {
                    apiKey: "test-key-not-real",
                    baseURL: mockProviderURL,
                },
                models: {
                    "mock-sonnet": {
                        id: "mock-sonnet",
                        name: "Mock Sonnet",
                        cost: { input: 0, output: 0 },
                        limit: { context: opts.modelContextLimit ?? 200000, output: 8192 },
                        // The mock advertises image and PDF input so OpenCode preserves inline file parts.
                        // OpenCode replaces inline file parts for unsupported inputs with text error messages.
                        // The mock mirrors Sonnet's image and PDF input capabilities.
                        modalities: {
                            input: ["text", "image", "pdf"],
                            output: ["text"],
                        },
                        options: {},
                    },
                },
            },
        },
        ...extraWithoutProvider,
    };

    const eidnara: Record<string, unknown> = {
        $schema: "https://raw.githubusercontent.com/ahrav/eidnara/main/assets/eidnara.schema.json",
        execute_threshold_percentage: 40,
        history_budget_percentage: 0.15,
        sidekick: { disable: true },
        ...(eidnaraConfig ?? {}),
    };
    if (opts.userHostConnectionFile) {
        // The config loader activates rust only with user-tier consent: a user-tier
        // `transform_mode: "rust"` or a user-tier `subc.connection_file`. Both are written so
        // the project selection below cannot be downgraded to ts by the consent check.
        Object.assign(eidnara, {
            transform_mode: "rust",
            subc: { connection_file: opts.userHostConnectionFile },
        });
    }

    writeFileSync(join(env.configDir, "opencode.json"), JSON.stringify(opencodeConfig, null, 2));

    //
    const userConfigPath = userEidnaraConfigPath(env);
    mkdirSync(dirname(userConfigPath), { recursive: true });
    writeFileSync(userConfigPath, JSON.stringify(eidnara, null, 2));

    const projectConfig: Record<string, unknown> | undefined = opts.userHostConnectionFile
        ? { ...(projectEidnaraConfig ?? {}), transform_mode: "rust" }
        : projectEidnaraConfig;
    if (projectConfig) {
        const projectConfigDir = join(env.workdir, ".eidnara");
        mkdirSync(projectConfigDir, { recursive: true });
        writeFileSync(
            join(projectConfigDir, "eidnara.jsonc"),
            JSON.stringify(
                {
                    $schema:
                        "https://raw.githubusercontent.com/ahrav/eidnara/main/assets/eidnara.schema.json",
                    ...projectConfig,
                },
                null,
                2,
            ),
        );
    }
}

/**
 * Every caller-supplied config is serialized once, before the credential scan and before any resource is provisioned, so a `toJSON()` hook cannot present one configuration to the scan and write another to disk.
 */
function canonicalizeSpawnConfigs(opts: SpawnOptions): SpawnOptions {
    /** Copied before any `toJSON()` runs, and returned so the child is given the same map that was validated: a hook that replaces `extraEnv` rather than mutating it would otherwise have its replacement forwarded while validation read the map it displaced. */
    const extraEnv = opts.extraEnv === undefined ? undefined : { ...opts.extraEnv };
    return {
        ...opts,
        extraEnv,
        openCodeConfigExtra: canonicalConfig(opts.openCodeConfigExtra, "openCodeConfigExtra"),
        eidnaraConfig: canonicalConfig(opts.eidnaraConfig, "eidnaraConfig"),
        projectEidnaraConfig: canonicalConfig(opts.projectEidnaraConfig, "projectEidnaraConfig"),
    };
}

/**
 * Serialize before validation so `toJSON()` transformations cannot bypass credential checks.
 * Cyclic input causes `JSON.stringify` to throw before credential validation.
 */
function canonicalConfig(
    value: Record<string, unknown> | undefined,
    label: string,
): Record<string, unknown> | undefined {
    if (value === undefined) return undefined;
    /** A spread copies own enumerable fields whatever `toJSON()` reported, so the scan and the write must read one representation; `writeConfigs` assembles every file from this return value. */
    const serialized = JSON.parse(JSON.stringify(value)) as unknown;
    /** A `toJSON()` returning a non-object leaves no fields to spread, and treating it as a config would write the scalar's own properties instead. */
    if (serialized === null || typeof serialized !== "object" || Array.isArray(serialized)) {
        throw new Error(`${label} must serialize to a JSON object`);
    }
    const canonical = serialized as Record<string, unknown>;
    assertConfigHasNoCredentials(canonical, label);
    return canonical;
}

/**
 * The user config loader expands `{env:NAME}` before the plugin reads a value, so a placeholder under a credential-shaped key is the one way a config may name a credential the child resolves from its environment.
 * Anchored at both ends and restricted to an environment variable name, so a credential cannot ride along after the placeholder and an empty name is refused.
 * The captured name must be sensitive by `isSensitiveEnvKey`: `isInheritableEnvKey` drops ambient sensitive names, so `extraEnv` stays the only channel that can deliver the resolved value and `assertSecretsBoundToLoopback` keeps covering it.
 */
const ENV_PLACEHOLDER = /^\{env:\s*([A-Za-z_][A-Za-z0-9_]*)\s*\}$/;

/**
 * `assertConfigHasNoCredentials` refuses a credential-shaped key name anywhere in a config channel.
 * The diagnostic names the key path and never the value, so a refusal never puts a secret in a log.
 *
 * Key shape is the only rule: a credential under an innocuous name still reaches `opencode.json`,
 * which leaves `extraEnv` the only channel governed by shape rather than by recognition.
 */
function assertConfigHasNoCredentials(value: unknown, label: string): void {
    const seen = new WeakSet<object>();
    const visit = (current: unknown, path: string): void => {
        if (current === null || typeof current !== "object" || seen.has(current)) return;
        seen.add(current);
        for (const [key, child] of Object.entries(current)) {
            const childPath = `${path}.${key}`;
            /** A placeholder is not a credential: what reaches disk is the token, and the value it stands for is resolved from the environment after the file is read. Only a name the sensitive-key rule recognizes is an approved channel; any other placeholder falls through to the key rule. */
            const placeholder = typeof child === "string" ? ENV_PLACEHOLDER.exec(child) : null;
            if (placeholder !== null && isSensitiveEnvKey(placeholder[1] as string)) continue;
            if (!Array.isArray(current) && isSecretKey(key)) {
                throw new Error(
                    `config contains credential-shaped key: ${childPath}; ` +
                        "pass credentials through extraEnv",
                );
            }
            visit(child, childPath);
        }
    };
    visit(value, label);
}

/**
 * Bun limits fetch to about five minutes even when AbortSignal.timeout is longer; bound each attempt so retries can honor the overall deadline.
 * Each fetch attempt uses a timeout so a hung fetch cannot consume the overall retry deadline.
 */
// OpenCode readiness allows up to 300 seconds for first-run initialization in CI.
// GitHub-hosted runners can delay OpenCode readiness while the server initializes plugins and first-run state.
// OpenCode initializes its SQLite store on first run against a fresh XDG_DATA_HOME.
async function waitForReady(
    url: string,
    timeoutMs = 300_000,
    cancellation?: AbortSignal,
): Promise<void> {
    const deadline = Date.now() + timeoutMs;
    const FETCH_TIMEOUT_MS = 2_000;
    let lastFetchErr: unknown = null;
    let fetchAttempts = 0;

    while (Date.now() < deadline) {
        if (cancellation?.aborted) throw new Error("opencode readiness cancelled");
        try {
            fetchAttempts++;
            const timeout = AbortSignal.timeout(FETCH_TIMEOUT_MS);
            const res = await fetch(`${url}/doc`, {
                method: "GET",
                signal: cancellation ? AbortSignal.any([timeout, cancellation]) : timeout,
            });
            if (res.ok || res.status === 404 || res.status === 401) {
                // A 2xx, 401, or 404 response confirms that the server is reachable.
                return;
            }
        } catch (err) {
            if (cancellation?.aborted) throw new Error("opencode readiness cancelled");
            lastFetchErr = err;
        }
        await Bun.sleep(200);
    }
    throw new Error(
        `opencode serve did not become ready in ${timeoutMs}ms.\n` +
            `  url=${url}/doc\n` +
            `  fetchAttempts=${fetchAttempts}\n` +
            `  fetchLastErr=${String(lastFetchErr)}`,
    );
}

interface RustSpawnResources {
    env: IsolatedEnv;
    connectionFile: string;
    host: HermeticHostStack;
}

/**
 * The readiness wait rejects when the child fails to spawn or exits before readiness.
 * A child can start and then die without emitting `error`; handling `exit` prevents the startup race from waiting for its timeout.
 */
function rejectOnSpawnError(child: ChildProcess, cancellation?: AbortSignal): Promise<never> {
    return new Promise((_, rejectSpawn) => {
        const detach = (): void => {
            child.off("error", onError);
            child.off("exit", onExit);
        };
        const onError = (error: Error): void => {
            detach();
            cancellation?.removeEventListener("abort", onAbort);
            rejectSpawn(error);
        };
        const onExit = (code: number | null, signal: NodeJS.Signals | null): void => {
            detach();
            cancellation?.removeEventListener("abort", onAbort);
            rejectSpawn(
                new Error(
                    `opencode serve exited before readiness (code=${code}, signal=${signal})`,
                ),
            );
        };
        const onAbort = (): void => {
            detach();
        };
        if (cancellation?.aborted) return;
        child.once("error", onError);
        child.once("exit", onExit);
        cancellation?.addEventListener("abort", onAbort, { once: true });
    });
}

async function stopChild(child: ChildProcess, timeoutMs = 3_000): Promise<void> {
    if (child.exitCode !== null || child.signalCode !== null || child.pid === undefined) return;

    const exitedAfterTerm = waitForChildExit(child, timeoutMs);
    child.kill("SIGTERM");
    if (await exitedAfterTerm) return;

    const exitedAfterKill = waitForChildExit(child, timeoutMs);
    child.kill("SIGKILL");
    if (!(await exitedAfterKill)) {
        throw new Error("opencode serve did not exit after SIGKILL");
    }
}

/** The direct host is provisioned before OpenCode so OpenCode can publish its connection file. */
async function provisionRustMode(): Promise<RustSpawnResources> {
    const prereqs = detectRustModePrereqs();
    if (!prereqs.ok) {
        throw new Error(
            `EIDNARA_E2E_MODE=rust prerequisite failure: ${prereqs.skipReason ?? "unknown prerequisite"}`,
        );
    }
    const fixtureBin = await buildDirectHostFixture();
    const env = createIsolatedEnv();
    try {
        const host = await HermeticHostStack.start({ dataDir: env.dataDir, fixtureBin });
        return { env, connectionFile: host.connectionFile, host };
    } catch (error) {
        // A surviving `dataDir` records failed teardown because it contains the leaked fixture's PID file.
        // The next run uses the leaked fixture's PID file as its only handle on that process.
        // Preserving `dataDir` lets the next run reclaim the leaked process from its PID file.
        if (!existsSync(env.dataDir)) {
            try {
                rmSync(dirname(env.dataDir), { recursive: true, force: true });
            } catch {
                // Preserving a leaked `dataDir` does not mask the startup failure.
            }
        }
        throw new Error(
            `EIDNARA_E2E_MODE=rust failed to start direct host fixture: ${String(error)}`,
        );
    }
}

/**
 * The explicit environment list prevents the child from inheriting runner variables.
 * port.
 */
function isInheritableEnvKey(key: string): boolean {
    // Tests run unsecured on a random localhost port; inherited auth would require Basic headers that SDK requests do not set.
    if (key === "OPENCODE_SERVER_PASSWORD" || key === "OPENCODE_SERVER_USERNAME") return false;
    // Exclude `NODE_ENV` because Bun sets it to `test`, which silences the plugin logger.
    // Exclude `NODE_ENV` so the subprocess writes diagnostic logs.
    if (key === "NODE_ENV") return false;
    // The harness clears `EIDNARA_MODULE_ID` and `EIDNARA_LAUNCH_NONCE` because an inherited supervisor identity makes the plugin send a nonce the hermetic host rejects.
    // The hermetic host rejects an inherited supervisor identity whose nonce does not match a supervised launch.
    if (key === "EIDNARA_MODULE_ID" || key === "EIDNARA_LAUNCH_NONCE") return false;
    // `EIDNARA_BROCA_CHILD=1` makes the bundled plugin return before installing any hook, so an inherited value would run the suite without the Rust transform.
    if (key === "EIDNARA_BROCA_CHILD") return false;
    // These point OpenCode at a database or configuration outside the isolated `env`; the child must read and mutate only the state this harness provisions.
    if (key === "OPENCODE_DB" || key === "OPENCODE_CONFIG" || key === "OPENCODE_CONFIG_CONTENT") {
        return false;
    }
    // Ambient secrets are never forwarded.
    // Ambient secrets would be exposed through the unauthenticated API to any process that reaches the port.
    // The child uses the mock provider; credentialed spawns pass credentials through `extraEnv`.
    // Dropping ambient secrets leaves `extraEnv` as the only caller-secret channel, so `assertSecretsBoundToLoopback` covers all caller secrets.
    if (isSensitiveEnvKey(key)) return false;
    return true;
}

/**
 * The spawn path rejects caller-supplied secrets on non-loopback interfaces.
 *
 * The spawned environment's default `ANTHROPIC_API_KEY` is a fixture value.
 * Checking the assembled environment would reject every default spawn because it contains the fake `ANTHROPIC_API_KEY`.
 *
 * `assertSecretsBoundToLoopback` does not inspect credentials embedded in `openCodeConfigExtra`.
 * `writeConfigs` applies `assertConfigHasNoCredentials` to that channel instead.
 *
 * `Pick<SpawnOptions>` permits forwarding `extraEnv` without `hostname`.
 * Omitting `hostname` uses the all-interfaces default.
 * `extraEnv` secrets reach the unauthenticated serve API when `hostname` falls back to `0.0.0.0`.
 *
 * `assertSecretsBoundToLoopback` runs before provisioning to avoid creating Rust resources for rejected spawns.
 * `isSensitiveEnvKey` matches names because fake credentials cannot be distinguished from real credentials by value.
 * `allowSecretEnvOffLoopback` permits explicitly waived sensitive environment variables off loopback.
 * `assertSafeExtraEnv` rejects sensitive environment variables with the same predicate.
 */
function assertSecretsBoundToLoopback(resolvedOpts: SpawnOptions, hostname: ServeHostname): void {
    if (hostname === "127.0.0.1" || resolvedOpts.allowSecretEnvOffLoopback) return;
    const secretKeys = Object.keys(resolvedOpts.extraEnv ?? {}).filter(isSensitiveEnvKey);
    if (secretKeys.length === 0) return;
    throw new Error(
        `refusing to bind the unauthenticated serve API to ${hostname} while extraEnv carries ` +
            `${secretKeys.join(", ")}. Pass hostname: "127.0.0.1" to keep the API on loopback, ` +
            `or allowSecretEnvOffLoopback: true if the value is a fake fixture credential.`,
    );
}

async function spawnOpencodeWithProvision(
    opts: SpawnOptions,
    provision: () => Promise<RustSpawnResources>,
): Promise<SpawnedOpencode> {
    const hostname = opts.hostname ?? "0.0.0.0";
    assertSecretsBoundToLoopback(opts, hostname);
    /** Canonicalized and scanned before provisioning, for the same reason the loopback gate runs first: a rejected spawn must not have created a hermetic Rust stack to tear down. `canonicalizeSpawnConfigs` snapshots `extraEnv` ahead of any `toJSON()` and returns that snapshot, so the map the scan read is the map the child is given — re-reading `opts.extraEnv` later would forward whatever a hook left behind, and a hook that replaces the map is never seen by the scan at all. A hook serializes config; it does not get a say in the child's environment. */
    const canonicalOpts: SpawnOptions = canonicalizeSpawnConfigs(opts);
    /** The gate reads both maps merged. A hook cannot reach the child, but adding a sensitive name is still an attempt worth refusing rather than silently dropping, and this gate is the one that reads the hostname. */
    assertSecretsBoundToLoopback(
        {
            ...canonicalOpts,
            extraEnv: { ...(canonicalOpts.extraEnv ?? {}), ...(opts.extraEnv ?? {}) },
        },
        hostname,
    );

    // `EIDNARA_E2E_MODE` selects provisioning at this shared spawn path; `rust` is its only value.
    // A caller that already owns a direct host passes its connection file and skips provisioning.
    const mode = process.env.EIDNARA_E2E_MODE;
    if (mode !== undefined && mode !== "rust") {
        throw new Error(`EIDNARA_E2E_MODE=${mode} is unsupported; the only accepted value is rust`);
    }
    const resources = mode === "rust" && !opts.userHostConnectionFile ? await provision() : null;

    let child: ChildProcess | undefined;
    let cleanupPromise: Promise<void> | undefined;
    const cleanup = (): Promise<void> => {
        cleanupPromise ??= (async () => {
            let cleanupError: unknown;
            if (child) {
                try {
                    await stopChild(child);
                } catch (error) {
                    cleanupError = error;
                }
            }
            try {
                await resources?.host.stop();
            } catch (error) {
                cleanupError ??= error;
            }
            if (cleanupError !== undefined) throw cleanupError;
        })();
        return cleanupPromise;
    };

    let stdoutBuf = "";
    let stderrBuf = "";
    try {
        const resolvedOpts: SpawnOptions = resources
            ? {
                  ...canonicalOpts,
                  existingEnv: resources.env,
                  userHostConnectionFile: resources.connectionFile,
              }
            : canonicalOpts;

        const env = resolvedOpts.existingEnv ?? createIsolatedEnv();
        const port = resolvedOpts.port ?? (await pickFreePort());

        writeConfigs(env, resolvedOpts.mockProviderURL, resolvedOpts);

        const childEnv: Record<string, string> = {};
        for (const [key, value] of Object.entries(process.env)) {
            if (value === undefined) continue;
            if (!isInheritableEnvKey(key)) continue;
            childEnv[key] = value;
        }
        childEnv.OPENCODE_CONFIG_DIR = env.configDir;
        childEnv.XDG_CONFIG_HOME = env.configDir;
        childEnv.XDG_DATA_HOME = env.dataDir;
        childEnv.XDG_CACHE_HOME = env.cacheDir;
        childEnv.ANTHROPIC_API_KEY = "test-key-not-real";
        for (const [key, value] of Object.entries(resolvedOpts.extraEnv ?? {})) {
            childEnv[key] = value;
        }

        // Sensitive `extraEnv` requires `hostname: "127.0.0.1"` unless `allowSecretEnvOffLoopback` is true.
        child = spawn("opencode", ["serve", "--port", String(port), "--hostname", hostname], {
            cwd: env.workdir,
            env: childEnv,
            stdio: ["ignore", "pipe", "pipe"],
        });

        child.stdout?.on("data", (chunk: Buffer) => {
            stdoutBuf += chunk.toString();
        });
        child.stderr?.on("data", (chunk: Buffer) => {
            stderrBuf += chunk.toString();
        });

        const url = `http://127.0.0.1:${port}`;
        const startup = new AbortController();
        try {
            await Promise.race([
                waitForReady(url, 300_000, startup.signal),
                rejectOnSpawnError(child, startup.signal),
            ]);
        } finally {
            startup.abort();
        }

        return {
            url,
            port,
            env,
            stdout: () => stdoutBuf,
            stderr: () => stderrBuf,
            hostStack: resources?.host,
            kill: cleanup,
        };
    } catch (error) {
        let cleanupError: unknown;
        try {
            await cleanup();
        } catch (failure) {
            cleanupError = failure;
        }
        throw new Error(
            `opencode serve failed to start.\n--- stdout ---\n${stdoutBuf}\n--- stderr ---\n${stderrBuf}\n\n${String(error)}` +
                (cleanupError === undefined ? "" : `\ncleanup failed: ${String(cleanupError)}`),
        );
    }
}

export function spawnOpencode(opts: SpawnOptions): Promise<SpawnedOpencode> {
    return spawnOpencodeWithProvision(opts, provisionRustMode);
}

export const __spawnOpencodeTest = {
    assertSecretsBoundToLoopback,
    canonicalizeSpawnConfigs,
    isInheritableEnvKey,
    rejectOnSpawnError,
    stopChild,
    spawnOpencodeWithProvision,
    writeConfigs,
};
