import { execFileSync } from "node:child_process";
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
const CAPTURE_PROJECTS = Symbol.for("eidnara.opencode.native-capture-projects.v1");
const CAPTURE_IN_FLIGHT = Symbol.for("eidnara.opencode.native-capture-in-flight.v1");
const globals = globalThis as typeof globalThis & {
    [CAPTURE_PROJECTS]?: Set<string>;
    [CAPTURE_IN_FLIGHT]?: Set<string>;
};
/** Private project directories that exist on disk; hooks must not capture from them. */
const captureProjects = (globals[CAPTURE_PROJECTS] ??= new Set<string>());
/** Directories with a capture still running; the admission bound counts these alone. */
const captureInFlight = (globals[CAPTURE_IN_FLIGHT] ??= new Set<string>());

/** This process alone registers owned private projects; repository configuration
 * cannot grant itself this guard or suppress normal capture. */
export function isNativeCaptureProject(directory: string): boolean {
    try {
        return captureProjects.has(realpathSync(directory));
    } catch {
        return false;
    }
}

function describeFailure(error: unknown): string {
    return error instanceof Error ? error.message : String(error);
}

/** A private project inherits native user-level providers/auth, never the source
 * repository's provider overrides. The only project config is authored here. */
export function openCodeMemoryCaptureExecutor(
    client: EidnaraDeps["client"],
): NativeCaptureExecutor {
    return async (work, signal) => {
        const separator = work.model.indexOf("/");
        const providerID = work.model.slice(0, separator);
        const modelID = work.model.slice(separator + 1);
        if (captureInFlight.size >= MAX_IN_FLIGHT)
            throw new NativeCaptureError("provider_unavailable");
        const directory = realpathSync(mkdtempSync(join(tmpdir(), "eidnara-capture-")));
        captureProjects.add(directory);
        captureInFlight.add(directory);
        let session: string | undefined;
        let instanceStarted = false;
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
            let contextLimit = 1_048_576;
            let outputLimit = work.maxOutputTokens;
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
                            models?: Record<
                                string,
                                { limit?: { context?: number; output?: number } }
                            >;
                        }>;
                    } | null,
                    { preferResponseOnMissingData: true },
                );
                const limit = info?.providers?.find((provider) => provider.id === providerID)
                    ?.models?.[modelID]?.limit;
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
            if (signal.aborted) throw new NativeCaptureError("cancelled");
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
                                [modelID]: {
                                    limit: { context: contextLimit, output: outputLimit },
                                },
                            },
                        },
                    },
                })
                    .replaceAll("{env:", "\\u007benv:")
                    .replaceAll("{file:", "\\u007bfile:"),
                { mode: 0o600 },
            );
            instanceStarted = true;
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
            // Dispose only this private instance; its watchers must stop before the directory goes away.
            if (instanceStarted)
                await withTimeout(
                    Promise.resolve(client.instance.dispose({ query: { directory } } as never)),
                    CLEANUP_MS,
                    "capture instance cleanup timed out",
                ).catch((error) => {
                    cleanupFailures.push(`dispose: ${describeFailure(error)}`);
                });
            captureInFlight.delete(directory);
            // The guard outlives a directory that cannot be removed; `isNativeCaptureProject`
            // resolves through `realpathSync`, so a removed directory needs no entry.
            try {
                rmSync(directory, { recursive: true, force: true });
                captureProjects.delete(directory);
            } catch (error) {
                cleanupFailures.push(`remove: ${describeFailure(error)}`);
            }
            // Cleanup failures never change the capture outcome; the answer is already final.
            if (cleanupFailures.length > 0)
                log.warn("[eidnara] native memory capture cleanup incomplete", cleanupFailures);
        }
        if (failure) throw failure;
        if (!answer) throw new NativeCaptureError("model_failed");
        return answer;
    };
}
