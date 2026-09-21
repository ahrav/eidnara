import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { log } from "../../shared/logger";
import { NativeCaptureError, type NativeCaptureExecutor } from "../../shared/memory-capture";
import { normalizeSDKResponse } from "../../shared/normalize-sdk-response";
import { withTimeout } from "../../shared/with-timeout";
import { createChildSession, deleteChildSession } from "./child-session-spawn";
import type { EidnaraDeps } from "./hook";

const AGENT = "eidnara-memory-capture";
const CLEANUP_MS = 10_000;
const MAX_IN_FLIGHT = 16;
/** `other` and `unknown` indicate an omitted `finish_reason`; the part checks determine
 * whether the response text is usable. */
const FAILED_FINISH_REASONS: ReadonlySet<string> = new Set([
    "tool-calls",
    "content-filter",
    "error",
]);
/** Distinct (model, system prompt, output cap) projects kept warm; the least recently used is disposed beyond this. */
const MAX_PROJECTS = 4;

/** One private OpenCode project serves every capture with the same model, system prompt, and output cap. */
interface PrivateProject {
    key: string;
    directory: string;
    /** Settles once the directory is a real root with its authored config; rejects when setup failed. */
    ready: Promise<void>;
    /** Set once OpenCode has started an instance for the directory, so eviction knows to dispose it. */
    started: boolean;
    inFlight: number;
    lastUsed: number;
    /** Set when disposal found the project busy; the last capture to finish evicts it. */
    retired: boolean;
}

interface CaptureState {
    /** Private project directories that exist on disk; hooks must not capture from them. */
    projects: Set<string>;
    byKey: Map<string, PrivateProject>;
    inFlight: number;
}

const CAPTURE_STATE = Symbol.for("eidnara.opencode.native-capture-state.v2");
const globals = globalThis as typeof globalThis & { [CAPTURE_STATE]?: CaptureState };
const state = (globals[CAPTURE_STATE] ??= (() => {
    const created: CaptureState = { projects: new Set(), byKey: new Map(), inFlight: 0 };
    // Private directories outlive individual captures; the process end is their last owner.
    process.once("exit", () => {
        for (const directory of created.projects) {
            try {
                rmSync(directory, { recursive: true, force: true });
            } catch {
                // A directory the process cannot remove at exit is left for the tmpdir owner.
            }
        }
    });
    return created;
})());

/** This process alone registers owned private projects; repository configuration
 * cannot grant itself this guard or suppress normal capture. */
export function isNativeCaptureProject(directory: string): boolean {
    try {
        return state.projects.has(realpathSync(directory));
    } catch {
        return false;
    }
}

function describeFailure(error: unknown): string {
    return error instanceof Error ? error.message : String(error);
}

/** OpenCode records missing/expired credentials and provider transport failures on the
 * answer; they say nothing about the model's output, so they must not spend the daemon's
 * model-failure allowance. Non-retryable HTTP errors other than auth are the model's. */
function isProviderUnavailable(error: unknown): boolean {
    const failure = error as {
        name?: unknown;
        data?: { statusCode?: unknown; isRetryable?: unknown };
    } | null;
    if (!failure || typeof failure !== "object") return false;
    if (failure.name === "ProviderAuthError") return true;
    if (failure.name !== "APIError") return false;
    const status = failure.data?.statusCode;
    return failure.data?.isRetryable === true || status === 401 || status === 403;
}

/** Removes the directory; the guard outlives a directory that cannot be removed, and
 * `isNativeCaptureProject` resolves through `realpathSync`, so a removed one needs no entry. */
function removeDirectory(directory: string, failures: string[]): void {
    try {
        rmSync(directory, { recursive: true, force: true });
        state.projects.delete(directory);
    } catch (error) {
        failures.push(`remove: ${describeFailure(error)}`);
    }
}

/** The model's native limits from OpenCode's provider metadata, or `undefined` when they are
 * unknown: metadata timed out, listed no such model, or carried no usable numbers. An unknown
 * limit is never replaced by a guess, since an override above the real output cap would make
 * the provider reject every request. */
async function nativeLimits(
    client: EidnaraDeps["client"],
    providerID: string,
    modelID: string,
    maxOutputTokens: number,
): Promise<{ contextLimit: number; outputLimit: number } | undefined> {
    let limit: { context?: number; output?: number } | undefined;
    try {
        const response = await withTimeout(
            Promise.resolve(client.config.providers()),
            2000,
            "native model metadata timed out",
        );
        const info = normalizeSDKResponse(
            response,
            null as {
                providers?: Array<{
                    id?: string;
                    models?: Record<string, { limit?: { context?: number; output?: number } }>;
                }>;
            } | null,
            { preferResponseOnMissingData: true },
        );
        limit = info?.providers?.find((provider) => provider.id === providerID)?.models?.[modelID]
            ?.limit;
    } catch {
        // Metadata is advisory; without it the private project keeps OpenCode's own model limits.
    }
    const usable = (value: unknown): value is number =>
        typeof value === "number" && Number.isSafeInteger(value) && value > 0;
    if (!usable(limit?.context) || !usable(limit?.output)) return undefined;
    const contextLimit = limit.context;
    const outputLimit = Math.max(
        1,
        Math.min(maxOutputTokens, limit.output, Math.floor(contextLimit / 4)),
    );
    return { contextLimit, outputLimit };
}

const OPENCODE_CONFIG_NAMES = ["opencode.json", "opencode.jsonc", ".opencode"];

/** Makes the private directory a project root. With `git` on PATH that is a fresh repository,
 * which stops OpenCode's config discovery at the directory. Without `git`, OpenCode files the
 * directory under its global project and walks discovery up to `/`, so the fallback only
 * accepts a root with no OpenCode config anywhere above it. */
function isolateRoot(directory: string): void {
    try {
        execFileSync("git", ["init", "--quiet", "--template=", directory], {
            env: {
                PATH: process.env.PATH,
                HOME: directory,
                GIT_CONFIG_NOSYSTEM: "1",
                GIT_CONFIG_GLOBAL: "/dev/null",
            },
            stdio: "ignore",
        });
        return;
    } catch (error) {
        if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
    }
    for (let parent = dirname(directory); ; parent = dirname(parent)) {
        for (const name of OPENCODE_CONFIG_NAMES)
            if (existsSync(join(parent, name)))
                throw new Error(
                    `git is unavailable and ${join(parent, name)} would apply to the private project`,
                );
        if (parent === dirname(parent)) return;
    }
}

/** A private project inherits native user-level providers/auth, never the source
 * repository's provider overrides. The only project config is authored here. */
async function prepareProject(
    client: EidnaraDeps["client"],
    directory: string,
    work: { model: string; system: string; maxOutputTokens: number },
): Promise<void> {
    const separator = work.model.indexOf("/");
    const providerID = work.model.slice(0, separator);
    const modelID = work.model.slice(separator + 1);
    const limits = await nativeLimits(client, providerID, modelID, work.maxOutputTokens);
    // A real root prevents native config discovery from walking into an outer checkout.
    isolateRoot(directory);
    // The authored config lives at the root; a `.opencode` directory would make OpenCode's
    // config loader install `@opencode-ai/plugin` into it from the npm registry. The output cap
    // is written only when the native limits are known; a guess above the real cap would make
    // the provider reject every request, while OpenCode's own limits already bound the model.
    writeFileSync(
        join(directory, "opencode.json"),
        JSON.stringify({
            compaction: { auto: false, prune: false },
            agent: {
                [AGENT]: {
                    mode: "subagent",
                    hidden: true,
                    steps: 2,
                    prompt: work.system,
                    tools: { "*": false },
                    permission: { "*": "deny" },
                },
            },
            ...(limits
                ? {
                      provider: {
                          [providerID]: {
                              models: {
                                  [modelID]: {
                                      limit: {
                                          context: limits.contextLimit,
                                          output: limits.outputLimit,
                                      },
                                  },
                              },
                          },
                      },
                  }
                : {}),
        })
            .replaceAll("{env:", "\\u007benv:")
            .replaceAll("{file:", "\\u007bfile:"),
        { mode: 0o600 },
    );
}

/** Disposes the project's instance before its directory goes away; failures are reported, not fatal.
 * An instance that could not be disposed keeps its directory and recursion guard until process exit. */
async function evictProject(client: EidnaraDeps["client"], project: PrivateProject): Promise<void> {
    if (state.byKey.get(project.key) === project) state.byKey.delete(project.key);
    const failures: string[] = [];
    if (project.started)
        await withTimeout(
            // `.then` keeps a synchronous throw from a disposed SDK client inside this catch.
            Promise.resolve().then(() =>
                client.instance.dispose({ query: { directory: project.directory } } as never),
            ),
            CLEANUP_MS,
            "capture instance cleanup timed out",
        ).catch((error) => {
            failures.push(`dispose: ${describeFailure(error)}`);
        });
    if (failures.length === 0) removeDirectory(project.directory, failures);
    if (failures.length > 0)
        log.warn("[eidnara] native memory capture cleanup incomplete", failures);
}

function evictIdleProjects(client: EidnaraDeps["client"]): void {
    while (state.byKey.size > MAX_PROJECTS) {
        let victim: PrivateProject | undefined;
        for (const project of state.byKey.values())
            if (project.inFlight === 0 && (!victim || project.lastUsed < victim.lastUsed))
                victim = project;
        if (!victim) return;
        void evictProject(client, victim);
    }
}

function projectFor(
    client: EidnaraDeps["client"],
    work: { model: string; system: string; maxOutputTokens: number },
): PrivateProject {
    const key = `${work.model}\0${createHash("sha256").update(work.system).digest("hex")}\0${work.maxOutputTokens}`;
    const existing = state.byKey.get(key);
    if (existing) return existing;
    const directory = realpathSync(mkdtempSync(join(privateRootParent(), "eidnara-capture-")));
    state.projects.add(directory);
    const project: PrivateProject = {
        key,
        directory,
        ready: Promise.resolve(),
        started: false,
        inFlight: 0,
        lastUsed: Date.now(),
        retired: false,
    };
    project.ready = prepareProject(client, directory, work).catch((error) => {
        if (state.byKey.get(key) === project) state.byKey.delete(key);
        removeDirectory(directory, []);
        throw error;
    });
    state.byKey.set(key, project);
    return project;
}

/** Where private project roots are allocated; tests point it at a directory that cannot hold one. */
let privateRootParent: () => string = tmpdir;

export const __nativeCaptureTest = {
    isolateRoot,
    setPrivateRootParent(parent: (() => string) | undefined): void {
        privateRootParent = parent ?? tmpdir;
    },
};

/** Busy projects remain available until their in-flight captures finish. */
export async function disposeNativeCaptureProjects(client: EidnaraDeps["client"]): Promise<void> {
    const projects = [...state.byKey.values()];
    state.byKey.clear();
    await Promise.all(
        projects.map((project) => {
            project.retired = true;
            return project.inFlight === 0 ? evictProject(client, project) : Promise.resolve();
        }),
    );
}

/** Captures run in a warm private project; only the session is per capture. */
export function openCodeMemoryCaptureExecutor(
    client: EidnaraDeps["client"],
): NativeCaptureExecutor {
    return async (work, signal) => {
        const separator = work.model.indexOf("/");
        const providerID = work.model.slice(0, separator);
        const modelID = work.model.slice(separator + 1);
        if (state.inFlight >= MAX_IN_FLIGHT) throw new NativeCaptureError("provider_unavailable");
        let project: PrivateProject;
        try {
            project = projectFor(client, work);
        } catch (error) {
            // Allocating the private root precedes any model call; its failure is not the model's.
            log.warn("[eidnara] native memory capture project unavailable", describeFailure(error));
            throw new NativeCaptureError("provider_unavailable");
        }
        const directory = project.directory;
        state.inFlight += 1;
        project.inFlight += 1;
        project.lastUsed = Date.now();
        // A project in use is never the victim, so the one just claimed survives.
        evictIdleProjects(client);
        let session: string | undefined;
        const cleanupFailures: string[] = [];
        let answer: { model: string; text: string } | undefined;
        let failure: NativeCaptureError | undefined;
        const abortSession = async () => {
            if (session)
                await withTimeout(
                    Promise.resolve(
                        client.session.abort({
                            path: { id: session },
                            query: { directory },
                        } as never),
                    ),
                    CLEANUP_MS,
                    "capture cancellation timed out",
                );
        };
        const abort = () => {
            void abortSession().catch((error) => {
                log.warn("[eidnara] native memory capture abort failed", describeFailure(error));
            });
        };
        signal.addEventListener("abort", abort, { once: true });
        try {
            try {
                await project.ready;
            } catch (error) {
                log.warn(
                    "[eidnara] native memory capture project unavailable",
                    describeFailure(error),
                );
                throw new NativeCaptureError("provider_unavailable");
            }
            if (signal.aborted) throw new NativeCaptureError("cancelled");
            project.started = true;
            // Session creation precedes any model call; its failure says nothing about the model.
            const created = normalizeSDKResponse(
                await createChildSession({
                    client,
                    title: `eidnara-memory-capture`,
                    directory,
                    denyTools: true,
                }).catch((error) => {
                    log.warn(
                        "[eidnara] native memory capture session create failed",
                        describeFailure(error),
                    );
                    throw new NativeCaptureError("provider_unavailable");
                }),
                null as { id?: string } | null,
                { preferResponseOnMissingData: true },
            );
            if (typeof created?.id !== "string" || !created.id)
                throw new NativeCaptureError("provider_unavailable");
            session = created.id;
            if (signal.aborted) throw new NativeCaptureError("cancelled");
            const response = await withTimeout(
                Promise.resolve(
                    client.session.prompt({
                        path: { id: session },
                        query: { directory },
                        signal,
                        body: {
                            // The agent's configured prompt already carries `work.system`; a body
                            // `system` would append the same instructions a second time.
                            agent: AGENT,
                            model: { providerID, modelID },
                            parts: [{ type: "text", text: work.prompt }],
                        },
                    } as never),
                ),
                work.maxDurationMs,
                "native capture timed out",
            );
            const result = normalizeSDKResponse(
                response,
                null as {
                    info?: {
                        error?: unknown;
                        modelID?: string;
                        providerID?: string;
                        finish?: string;
                    };
                    parts?: Array<{ type?: string; text?: string }>;
                } | null,
                { preferResponseOnMissingData: true },
            );
            if (isProviderUnavailable(result?.info?.error))
                throw new NativeCaptureError("provider_unavailable");
            if (
                !result ||
                result.info?.error ||
                result.info?.modelID !== modelID ||
                result.info?.providerID !== providerID ||
                !Array.isArray(result.parts) ||
                result.parts.some((part) => part.type === "tool")
            ) {
                throw new NativeCaptureError("model_failed");
            }
            if (result.info.finish === "length") throw new NativeCaptureError("output_limit");
            if (FAILED_FINISH_REASONS.has(result.info.finish ?? ""))
                throw new NativeCaptureError("model_failed");
            const text = result.parts
                .filter((part) => part.type === "text")
                .map((part) => part.text ?? "")
                .join("\n");
            if (Buffer.byteLength(text) > work.maxOutputBytes)
                throw new NativeCaptureError("output_limit");
            answer = { model: work.model, text };
        } catch (error) {
            await abortSession().catch((abortError) => {
                cleanupFailures.push(`abort: ${describeFailure(abortError)}`);
            });
            // A caller's abort outranks whatever OpenCode recorded for the interrupted prompt.
            failure = signal.aborted
                ? new NativeCaptureError("cancelled")
                : error instanceof NativeCaptureError
                  ? error
                  : new NativeCaptureError("model_failed");
        } finally {
            signal.removeEventListener("abort", abort);
            if (session)
                await deleteChildSession(client, session, undefined, directory).catch((error) => {
                    cleanupFailures.push(`delete: ${describeFailure(error)}`);
                });
            project.inFlight -= 1;
            state.inFlight -= 1;
            // Projects that were all busy at admission become evictable only now.
            evictIdleProjects(client);
            // Cleanup failures never change the capture outcome; the answer is already final.
            if (cleanupFailures.length > 0)
                log.warn("[eidnara] native memory capture cleanup incomplete", cleanupFailures);
            if (project.retired && project.inFlight === 0) await evictProject(client, project);
        }
        if (failure) throw failure;
        if (!answer) throw new NativeCaptureError("model_failed");
        return answer;
    };
}
