import { describe, expect, it } from "bun:test";
import type { ChildProcess } from "node:child_process";
import { EventEmitter } from "node:events";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import type { HermeticHostStack } from "../rust-runner/hermetic-host";
import { __spawnOpencodeTest, type IsolatedEnv, userEidnaraConfigPath } from "./spawn";

class FakeChild extends EventEmitter {
    readonly pid = 42;
    exitCode: number | null = null;
    signalCode: NodeJS.Signals | null = null;
    readonly signals: NodeJS.Signals[] = [];

    kill(signal: NodeJS.Signals = "SIGTERM"): boolean {
        this.signals.push(signal);
        if (signal === "SIGKILL") {
            queueMicrotask(() => {
                this.signalCode = signal;
                this.emit("exit", null, signal);
            });
        }
        return true;
    }
}

function childProcess(fake: FakeChild): ChildProcess {
    return fake as unknown as ChildProcess;
}

describe("opencode child lifecycle", () => {
    it("merges contributed providers beside the generated mock provider", () => {
        const root = mkdtempSync(join(tmpdir(), "opencode-provider-merge-"));
        const env: IsolatedEnv = {
            configDir: join(root, "config"),
            dataDir: join(root, "data"),
            cacheDir: join(root, "cache"),
            workdir: join(root, "work"),
        };
        try {
            for (const dir of Object.values(env)) mkdirSync(dir, { recursive: true });
            __spawnOpencodeTest.writeConfigs(env, "http://127.0.0.1:4321", {
                mockProviderURL: "http://127.0.0.1:4321",
                openCodeConfigExtra: {
                    provider: {
                        anthropic: {
                            api: "@ai-sdk/anthropic",
                            env: ["ANTHROPIC_API_KEY"],
                            models: {
                                "claude-sonnet-4-5-20250929": {
                                    limit: { context: 32_000 },
                                },
                            },
                        },
                        "mock-anthropic": { name: "must-not-replace-generated-mock" },
                    },
                },
            });

            const config = JSON.parse(
                readFileSync(join(env.configDir, "opencode.json"), "utf8"),
            ) as {
                provider: Record<
                    string,
                    {
                        name?: string;
                        options?: { baseURL?: string };
                        models?: Record<string, unknown>;
                    }
                >;
            };
            expect(Object.keys(config.provider).sort()).toEqual(["anthropic", "mock-anthropic"]);
            expect(config.provider["mock-anthropic"]?.name).toBe("Mock Anthropic");
            expect(config.provider["mock-anthropic"]?.options?.baseURL).toBe(
                "http://127.0.0.1:4321",
            );
            expect(config.provider.anthropic?.models).toHaveProperty("claude-sonnet-4-5-20250929");
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });

    it("rejects inline credentials in contributed provider config before writing", () => {
        const root = mkdtempSync(join(tmpdir(), "opencode-provider-secret-"));
        const env: IsolatedEnv = {
            configDir: join(root, "config"),
            dataDir: join(root, "data"),
            cacheDir: join(root, "cache"),
            workdir: join(root, "work"),
        };
        try {
            for (const dir of Object.values(env)) mkdirSync(dir, { recursive: true });
            expect(() =>
                __spawnOpencodeTest.writeConfigs(env, "http://127.0.0.1:4321", {
                    mockProviderURL: "http://127.0.0.1:4321",
                    openCodeConfigExtra: {
                        provider: {
                            anthropic: {
                                options: { apiKey: "sk-live-must-stay-in-extra-env" },
                            },
                        },
                    },
                }),
            ).toThrow(
                /credential-shaped key: openCodeConfigExtra\.provider\.anthropic\.options\.apiKey/,
            );
            expect(existsSync(join(env.configDir, "opencode.json"))).toBe(false);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });

    it("rejects compound credential key shapes while allowing count-like keys", () => {
        const root = mkdtempSync(join(tmpdir(), "opencode-provider-shapes-"));
        const env: IsolatedEnv = {
            configDir: join(root, "config"),
            dataDir: join(root, "data"),
            cacheDir: join(root, "cache"),
            workdir: join(root, "work"),
        };
        const write = (options: Record<string, unknown>) => () =>
            __spawnOpencodeTest.writeConfigs(env, "http://127.0.0.1:4321", {
                mockProviderURL: "http://127.0.0.1:4321",
                openCodeConfigExtra: {
                    provider: { anthropic: { options } },
                },
            });
        try {
            for (const dir of Object.values(env)) mkdirSync(dir, { recursive: true });
            expect(write({ headers: { "x-api-key": "sk-live" } })).toThrow(
                /credential-shaped key: openCodeConfigExtra\.provider\.anthropic\.options\.headers\.x-api-key/,
            );
            expect(write({ accessToken: "sk-live" })).toThrow(/accessToken/);
            expect(write({ clientSecret: "sk-live" })).toThrow(/clientSecret/);
            expect(write({ secret_access_key: "sk-live" })).toThrow(/secret_access_key/);
            expect(write({ maxTokens: 4096, baseURL: "http://127.0.0.1:1" })).not.toThrow();

            // Everything in the extra config reaches the same serve config, so a
            // credential outside `provider` is refused as well.
            expect(() =>
                __spawnOpencodeTest.writeConfigs(env, "http://127.0.0.1:4321", {
                    mockProviderURL: "http://127.0.0.1:4321",
                    openCodeConfigExtra: {
                        mcp: { docs: { headers: { Authorization: "Bearer sk-live" } } },
                    },
                }),
            ).toThrow(
                /credential-shaped key: openCodeConfigExtra\.mcp\.docs\.headers\.Authorization/,
            );

            // Counting keys keep working: `token` alone is not a credential word.
            expect(write({ maxTokens: 4096, promptTokens: 12, tokenBudget: 7 })).not.toThrow();

            // The sibling config channels are written to disk beside opencode.json,
            // and confusing them for `openCodeConfigExtra` is easy.
            expect(() =>
                __spawnOpencodeTest.writeConfigs(env, "http://127.0.0.1:4321", {
                    mockProviderURL: "http://127.0.0.1:4321",
                    eidnaraConfig: { hook: { apiKey: "sk-live" } },
                }),
            ).toThrow(/credential-shaped key: eidnaraConfig\.hook\.apiKey/);
            expect(() =>
                __spawnOpencodeTest.writeConfigs(env, "http://127.0.0.1:4321", {
                    mockProviderURL: "http://127.0.0.1:4321",
                    projectEidnaraConfig: { hook: { token: "Bearer sk-live-abcdefghij" } },
                }),
            ).toThrow(/projectEidnaraConfig\.hook\.token/);

            // An ordinary header value still passes.
            expect(() =>
                __spawnOpencodeTest.writeConfigs(env, "http://127.0.0.1:4321", {
                    mockProviderURL: "http://127.0.0.1:4321",
                    openCodeConfigExtra: {
                        mcp: { docs: { headers: { "x-trace": "run-42" } } },
                    },
                }),
            ).not.toThrow();

            // `toJSON()` returns `{}`, but object spread copies `hook`. The credential scan and config write must inspect the same representation.
            // Bun's `JSON.stringify` consults only an enumerable `toJSON`, so every hook below is defined enumerable.
            for (const channel of [
                "eidnaraConfig",
                "projectEidnaraConfig",
                "openCodeConfigExtra",
            ] as const) {
                const hidden: Record<string, unknown> = {
                    hook: { apiKey: "sk-ant-abcdefghijklmnopqrstuv" },
                };
                Object.defineProperty(hidden, "toJSON", {
                    value: () => ({}),
                    enumerable: true,
                });
                __spawnOpencodeTest.writeConfigs(env, "http://127.0.0.1:4321", {
                    mockProviderURL: "http://127.0.0.1:4321",
                    [channel]: hidden,
                });
                for (const file of [
                    join(env.configDir, "opencode.json"),
                    userEidnaraConfigPath(env),
                    join(env.workdir, ".eidnara", "eidnara.jsonc"),
                ]) {
                    if (!existsSync(file)) continue;
                    expect(readFileSync(file, "utf8")).not.toContain("sk-ant-");
                }
            }

            // A scalar JSON value has no configuration fields to merge; its original properties are ignored.
            const scalar: Record<string, unknown> = { hook: { apiKey: "sk-live" } };
            Object.defineProperty(scalar, "toJSON", { value: () => "opaque", enumerable: true });
            expect(() =>
                __spawnOpencodeTest.writeConfigs(env, "http://127.0.0.1:4321", {
                    mockProviderURL: "http://127.0.0.1:4321",
                    eidnaraConfig: scalar,
                }),
            ).toThrow(/eidnaraConfig must serialize to a JSON object/);

            // The user config loader expands `{env:NAME}`, so a placeholder naming a sensitive
            // variable is the token that reaches disk under a credential-shaped key.
            expect(() =>
                __spawnOpencodeTest.writeConfigs(env, "http://127.0.0.1:4321", {
                    mockProviderURL: "http://127.0.0.1:4321",
                    eidnaraConfig: {
                        hook: { api_key: "{env:HOOK_API_KEY}" },
                    },
                }),
            ).not.toThrow();
            expect(readFileSync(userEidnaraConfigPath(env), "utf8")).toContain(
                "{env:HOOK_API_KEY}",
            );

            // Only a whole, named placeholder: a credential must not ride along behind one,
            // and an empty or malformed name resolves to nothing.
            for (const value of [
                "{env:HOOK_API_KEY} sk-ant-abcdefghijklmnopqrstuv",
                "sk-ant-abcdefghijklmnopqrstuv",
                "{env:}",
                "{env:9NOPE}",
                "prefix{env:HOOK_API_KEY}",
                // A name the sensitive-key rule does not recognize is no approved channel.
                "{env:FOO}",
                "{env:HOOK_KEY}",
            ]) {
                expect(() =>
                    __spawnOpencodeTest.writeConfigs(env, "http://127.0.0.1:4321", {
                        mockProviderURL: "http://127.0.0.1:4321",
                        eidnaraConfig: { hook: { api_key: value } },
                    }),
                ).toThrow(/eidnaraConfig\.hook\.api_key/);
            }

            // `typeof [] === "object"`, so an array would spread into the provider map as
            // numeric keys rather than being ignored.
            __spawnOpencodeTest.writeConfigs(env, "http://127.0.0.1:4321", {
                mockProviderURL: "http://127.0.0.1:4321",
                openCodeConfigExtra: { provider: ["bogus"] },
            });
            const providerKeys = Object.keys(
                (
                    JSON.parse(readFileSync(join(env.configDir, "opencode.json"), "utf8")) as {
                        provider?: Record<string, unknown>;
                    }
                ).provider ?? {},
            );
            expect(providerKeys).not.toContain("0");

            // A `toJSON()` hook shares a reference with `extraEnv` and runs during
            // canonicalization, so the loopback rule has to read the post-hook environment.
            const smuggled: Record<string, string> = {};
            const mutating: Record<string, unknown> = { compaction: { auto: false } };
            Object.defineProperty(mutating, "toJSON", {
                value: () => {
                    smuggled.ANTHROPIC_API_KEY = "sk-ant-abcdefghijklmnopqrstuv";
                    return { compaction: { auto: false } };
                },
                enumerable: true,
            });
            const smugglingOpts = {
                mockProviderURL: "http://127.0.0.1:4321",
                extraEnv: smuggled,
                openCodeConfigExtra: mutating,
            };
            expect(() =>
                __spawnOpencodeTest.assertSecretsBoundToLoopback(smugglingOpts, "0.0.0.0"),
            ).not.toThrow();
            const postHook = {
                ...__spawnOpencodeTest.canonicalizeSpawnConfigs(smugglingOpts),
                extraEnv: { ...(smugglingOpts.extraEnv ?? {}) },
            };
            expect(() =>
                __spawnOpencodeTest.assertSecretsBoundToLoopback(postHook, "0.0.0.0"),
            ).toThrow(/refusing to bind the unauthenticated serve API/);

            // An ordinary query still passes.
            expect(() =>
                __spawnOpencodeTest.writeConfigs(env, "http://127.0.0.1:4321/v1?maxTokens=4096", {
                    mockProviderURL: "http://127.0.0.1:4321/v1?maxTokens=4096",
                }),
            ).not.toThrow();
            expect(() =>
                __spawnOpencodeTest.writeConfigs(env, "http://127.0.0.1:4321", {
                    mockProviderURL: "http://127.0.0.1:4321",
                }),
            ).not.toThrow();
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });

    it("loads only the built plugin bundle and writes rust consent beside the connection file", () => {
        const repoRoot = resolve(import.meta.dir, "../../../..");
        const dist = join(repoRoot, "packages/opencode-plugin/dist/index.js");
        const root = mkdtempSync(join(tmpdir(), "opencode-plugin-entry-"));
        const env: IsolatedEnv = {
            configDir: join(root, "config"),
            dataDir: join(root, "data"),
            cacheDir: join(root, "cache"),
            workdir: join(root, "work"),
        };
        try {
            for (const dir of Object.values(env)) mkdirSync(dir, { recursive: true });
            const connectionFile = join(env.dataDir, "connection.json");
            const write = () =>
                __spawnOpencodeTest.writeConfigs(env, "http://127.0.0.1:1", {
                    mockProviderURL: "http://127.0.0.1:1",
                    userHostConnectionFile: connectionFile,
                    projectEidnaraConfig: { protected_tags: 1 },
                });
            if (!existsSync(dist)) {
                expect(write).toThrow(/plugin bundle missing/);
                return;
            }
            write();
            const config = JSON.parse(
                readFileSync(join(env.configDir, "opencode.json"), "utf8"),
            ) as { plugin: string[] };
            expect(config.plugin).toEqual([`file://${dist}`]);

            const user = JSON.parse(readFileSync(userEidnaraConfigPath(env), "utf8")) as {
                transform_mode?: string;
                subc?: { connection_file?: string };
            };
            expect(user.transform_mode).toBe("rust");
            expect(user.subc?.connection_file).toBe(connectionFile);

            const project = JSON.parse(
                readFileSync(join(env.workdir, ".eidnara", "eidnara.jsonc"), "utf8"),
            ) as { transform_mode?: string; protected_tags?: number };
            expect(project.transform_mode).toBe("rust");
            expect(project.protected_tags).toBe(1);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });

    it("rejects startup on child spawn error", async () => {
        const child = new FakeChild();
        const startup = __spawnOpencodeTest.rejectOnSpawnError(childProcess(child));
        child.emit("error", new Error("spawn opencode ENOENT"));
        expect(String(await startup.catch((error: unknown) => error))).toContain(
            "spawn opencode ENOENT",
        );
    });

    it("withholds parent OpenCode control variables and the Broca-child guard from the child", () => {
        const { isInheritableEnvKey } = __spawnOpencodeTest;
        for (const key of [
            "EIDNARA_BROCA_CHILD",
            "OPENCODE_DB",
            "OPENCODE_CONFIG",
            "OPENCODE_CONFIG_CONTENT",
            "EIDNARA_MODULE_ID",
            "EIDNARA_LAUNCH_NONCE",
            "OPENCODE_SERVER_PASSWORD",
            "NODE_ENV",
        ]) {
            expect(isInheritableEnvKey(key)).toBe(false);
        }
        for (const key of ["PATH", "HOME", "OPENCODE_DISABLE_AUTOUPDATE"]) {
            expect(isInheritableEnvKey(key)).toBe(true);
        }
    });

    it("escalates a SIGTERM-ignoring child and waits for exit", async () => {
        const child = new FakeChild();
        let exitObserved = false;
        child.once("exit", () => {
            exitObserved = true;
        });

        await __spawnOpencodeTest.stopChild(childProcess(child), 5);

        expect(child.signals).toEqual(["SIGTERM", "SIGKILL"]);
        expect(exitObserved).toBe(true);
        expect(child.signalCode).toBe("SIGKILL");
    });

    it("refuses a credential a serialization hook adds after the first environment check", async () => {
        const root = mkdtempSync(join(tmpdir(), "opencode-spawn-smuggle-"));
        const env: IsolatedEnv = {
            configDir: join(root, "config"),
            dataDir: join(root, "data"),
            cacheDir: join(root, "cache"),
            workdir: join(root, "work"),
        };
        for (const dir of Object.values(env)) mkdirSync(dir, { recursive: true });
        const host = {
            connectionFile: join(env.dataDir, "host-connection.json"),
            async stop(): Promise<void> {},
        } as HermeticHostStack;
        // The hook mutates the same `extraEnv` object the first check already inspected.
        // Bun's `JSON.stringify` consults only an enumerable `toJSON`.
        const extraEnv: Record<string, string> = {};
        const mutating: Record<string, unknown> = { compaction: { auto: false } };
        Object.defineProperty(mutating, "toJSON", {
            value: () => {
                extraEnv.ANTHROPIC_API_KEY = "sk-ant-abcdefghijklmnopqrstuv";
                return { compaction: { auto: false } };
            },
            enumerable: true,
        });
        const previousMode = process.env.EIDNARA_E2E_MODE;
        process.env.EIDNARA_E2E_MODE = "rust";

        try {
            const error = await __spawnOpencodeTest
                .spawnOpencodeWithProvision(
                    {
                        mockProviderURL: "http://127.0.0.1:1",
                        port: 1,
                        extraEnv,
                        openCodeConfigExtra: mutating,
                    },
                    async () => ({ env, connectionFile: host.connectionFile, host }),
                )
                .catch((failure: unknown) => failure);

            expect(String(error)).toContain("refusing to bind the unauthenticated serve API");
            expect(String(error)).toContain("ANTHROPIC_API_KEY");
        } finally {
            if (previousMode === undefined) delete process.env.EIDNARA_E2E_MODE;
            else process.env.EIDNARA_E2E_MODE = previousMode;
            rmSync(root, { recursive: true, force: true });
        }
    });

    it("never provisions the Rust fixture when config serialization fails", async () => {
        const root = mkdtempSync(join(tmpdir(), "opencode-spawn-rollback-"));
        const env: IsolatedEnv = {
            configDir: join(root, "config"),
            dataDir: join(root, "data"),
            cacheDir: join(root, "cache"),
            workdir: join(root, "work"),
        };
        for (const dir of Object.values(env)) mkdirSync(dir, { recursive: true });
        const fixtureState = join(env.dataDir, "fixture-state");
        writeFileSync(fixtureState, "running");

        let stopCalls = 0;
        let provisionCalls = 0;
        const host = {
            connectionFile: join(env.dataDir, "host-connection.json"),
            async stop(): Promise<void> {
                stopCalls++;
                rmSync(root, { recursive: true, force: true });
            },
        } as HermeticHostStack;
        const cyclic: Record<string, unknown> = {};
        cyclic.self = cyclic;
        const previousMode = process.env.EIDNARA_E2E_MODE;
        process.env.EIDNARA_E2E_MODE = "rust";

        try {
            const error = await __spawnOpencodeTest
                .spawnOpencodeWithProvision(
                    {
                        mockProviderURL: "http://127.0.0.1:1",
                        port: 1,
                        openCodeConfigExtra: cyclic,
                    },
                    async () => {
                        provisionCalls += 1;
                        return { env, connectionFile: host.connectionFile, host };
                    },
                )
                .catch((failure: unknown) => failure);

            expect(String(error)).toContain("cyclic structures");
            // The config is canonicalized before provisioning, so there is no fixture to
            // stop: a rejected spawn does not create resources it then has to tear down.
            expect(provisionCalls).toBe(0);
            expect(stopCalls).toBe(0);
            expect(existsSync(fixtureState)).toBe(true);
        } finally {
            if (previousMode === undefined) delete process.env.EIDNARA_E2E_MODE;
            else process.env.EIDNARA_E2E_MODE = previousMode;
            rmSync(root, { recursive: true, force: true });
        }
    });
});
