import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { log } from "../../shared/logger";
import { NativeCaptureError, type NativeCaptureExecutor } from "../../shared/memory-capture";
import { normalizeSDKResponse } from "../../shared/normalize-sdk-response";
import { withTimeout } from "../../shared/with-timeout";
import { createChildSession, deleteChildSession } from "./child-session-spawn";
import type { EidnaraDeps } from "./hook";

const AGENT = "eidnara-memory-capture";
const CLEANUP_MS = 10_000;
const MAX_IN_FLIGHT = 16;
/** Distinct (model, system prompt, output cap) projects kept warm; the least recently used is disposed beyond this. */
const MAX_PROJECTS = 4;

/** One private OpenCode project serves every capture with the same model, system prompt, and output cap. */
interface PrivateProject {
    directory: string;
    /** Settles once the directory is a real root with its authored config; rejects when setup failed. */
    ready: Promise<void>;
    /** Set once OpenCode has started an instance for the directory, so eviction knows to dispose it. */
    started: boolean;
    inFlight: number;
    lastUsed: number;
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

async function nativeLimits(
    client: EidnaraDeps["client"],
    providerID: string,
    modelID: string,
    maxOutputTokens: number,
): Promise<{ contextLimit: number; outputLimit: number }> {
    let contextLimit = 1_048_576;
    let outputLimit = maxOutputTokens;
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
        const limit = info?.providers?.find((provider) => provider.id === providerID)?.models?.[
            modelID
        ]?.limit;
        const nativeContext = limit?.context;
        const nativeOutput = limit?.output;
        if (
            typeof nativeContext === "number" &&
            Number.isSafeInteger(nativeContext) &&
            nativeContext > 0
        )
            contextLimit = nativeContext;
        if (
            typeof nativeOutput === "number" &&
            Number.isSafeInteger(nativeOutput) &&
            nativeOutput > 0
        )
            outputLimit = Math.min(outputLimit, nativeOutput);
    } catch {
        // Metadata is advisory; the native provider still enforces its actual limits.
    }
    outputLimit = Math.max(1, Math.min(outputLimit, Math.floor(contextLimit / 4)));
    return { contextLimit, outputLimit };
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
    const { contextLimit, outputLimit } = await nativeLimits(
        client,
        providerID,
        modelID,
        work.maxOutputTokens,
    );
    // A real root prevents native config discovery from walking into an outer checkout.
    execFileSync("git", ["init", "--quiet", "--template=", directory], {
        env: {
            PATH: process.env.PATH,
            HOME: directory,
            GIT_CONFIG_NOSYSTEM: "1",
            GIT_CONFIG_GLOBAL: "/dev/null",
        },
        stdio: "ignore",
    });
    mkdirSync(join(directory, ".opencode"), { mode: 0o700 });
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
            provider: {
                [providerID]: {
                    models: {
                        [modelID]: { limit: { context: contextLimit, output: outputLimit } },
                    },
                },
            },
        })
            .replaceAll("{env:", "\\u007benv:")
            .replaceAll("{file:", "\\u007bfile:"),
        { mode: 0o600 },
    );
}

/** Disposes the project's instance before its directory goes away; failures are reported, not fatal. */
async function evictProject(
    client: EidnaraDeps["client"],
    key: string,
    project: PrivateProject,
): Promise<void> {
    state.byKey.delete(key);
    const failures: string[] = [];
    if (project.started)
        await withTimeout(
            Promise.resolve(
                client.instance.dispose({ query: { directory: project.directory } } as never),
            ),
            CLEANUP_MS,
            "capture instance cleanup timed out",
        ).catch((error) => {
            failures.push(`dispose: ${describeFailure(error)}`);
        });
    removeDirectory(project.directory, failures);
    if (failures.length > 0)
        log.warn("[eidnara] native memory capture cleanup incomplete", failures);
}

function evictIdleProjects(client: EidnaraDeps["client"]): void {
    while (state.byKey.size > MAX_PROJECTS) {
        let victim: [string, PrivateProject] | undefined;
        for (const entry of state.byKey)
            if (entry[1].inFlight === 0 && (!victim || entry[1].lastUsed < victim[1].lastUsed))
                victim = entry;
        if (!victim) return;
        void evictProject(client, victim[0], victim[1]);
    }
}

function projectFor(
    client: EidnaraDeps["client"],
    work: { model: string; system: string; maxOutputTokens: number },
): PrivateProject {
    const key = `${work.model}\0${createHash("sha256").update(work.system).digest("hex")}\0${work.maxOutputTokens}`;
    const existing = state.byKey.get(key);
    if (existing) return existing;
    const directory = realpathSync(mkdtempSync(join(tmpdir(), "eidnara-capture-")));
    state.projects.add(directory);
    const project: PrivateProject = {
        directory,
        ready: Promise.resolve(),
        started: false,
        inFlight: 0,
        lastUsed: Date.now(),
    };
    project.ready = prepareProject(client, directory, work).catch((error) => {
        state.byKey.delete(key);
        removeDirectory(directory, []);
        throw error;
    });
    state.byKey.set(key, project);
    return project;
}

/** Every project this process still owns is disposed and removed. */
export async function disposeNativeCaptureProjects(client: EidnaraDeps["client"]): Promise<void> {
    await Promise.all([...state.byKey].map(([key, project]) => evictProject(client, key, project)));
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
        const project = projectFor(client, work);
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
            } catch {
                throw new NativeCaptureError("provider_unavailable");
            }
            if (signal.aborted) throw new NativeCaptureError("cancelled");
            project.started = true;
            const created = normalizeSDKResponse(
                await createChildSession({
                    client,
                    title: `eidnara-memory-capture`,
                    directory,
                    denyTools: true,
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
                            agent: AGENT,
                            model: { providerID, modelID },
                            system: work.system,
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
            if (result.info.finish !== "stop" && result.info.finish !== "end_turn")
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
            failure =
                error instanceof NativeCaptureError
                    ? error
                    : new NativeCaptureError(signal.aborted ? "cancelled" : "model_failed");
        } finally {
            signal.removeEventListener("abort", abort);
            if (session)
                await deleteChildSession(client, session, undefined, directory).catch((error) => {
                    cleanupFailures.push(`delete: ${describeFailure(error)}`);
                });
            project.inFlight -= 1;
            state.inFlight -= 1;
            // Cleanup failures never change the capture outcome; the answer is already final.
            if (cleanupFailures.length > 0)
                log.warn("[eidnara] native memory capture cleanup incomplete", cleanupFailures);
        }
        if (failure) throw failure;
        if (!answer) throw new NativeCaptureError("model_failed");
        return answer;
    };
}
