// The SINGLEFILE variant embeds the WASM in the JavaScript module.
// The embedded WASM survives bundling into `dist/index.js`.
// The default variant loads `emscripten-module.wasm` with `new URL(..., import.meta.url)`.
// `new URL(..., import.meta.url)` resolves the sibling WASM path to `dist/emscripten-module.wasm`.
// The build emits no `dist/emscripten-module.wasm`, so the default variant fails with `ENOENT`.
// The capability API requires the ASYNCIFY variant because the sandbox installs asynchronous host functions.
//
// These two modules are imported LAZILY inside getAsyncModule() (below), not at
// the top of this file. The singlefile variant inlines ~2.6MB of base64 WASM into
// the bundle; a top-level import forced the JS engine to parse that blob on every
// plugin load — and on every subagent child spawn — adding hundreds of ms.
// Deferring the import to the first smart-note evaluation splits the variant
// into its own chunk that stays out of the cold-start parse. The type-only import
// below is erased at build time and pulls in no runtime code.
import type {
    QuickJSAsyncContext,
    QuickJSAsyncWASMModule,
    QuickJSHandle,
} from "quickjs-emscripten";

import type { SmartNoteCapabilityApi, SmartNoteCapabilityFactory } from "./capabilities";
import { isSmartNoteNetworkError, type SmartNoteCheckResult, smartNoteAbortError } from "./types";

/**
 * The reusable WASM module requires ~1 MB of compilation.
 * Each check creates a disposable context from the shared module.
 * The process-wide module promise creates the WASM module once rather than per check.
 *
 * Dynamic imports load the QuickJS variant and runtime on the first smart-note check.
 * Dynamic imports defer parsing the large QuickJS modules until a smart-note check runs.
 */
let asyncModulePromise: Promise<QuickJSAsyncWASMModule> | null = null;
function getAsyncModule(): Promise<QuickJSAsyncWASMModule> {
    if (asyncModulePromise) return asyncModulePromise;
    const promise = (async () => {
        const [{ default: singlefileAsyncifyVariant }, { newQuickJSAsyncWASMModuleFromVariant }] =
            await Promise.all([
                import("@jitl/quickjs-singlefile-cjs-release-asyncify"),
                import("quickjs-emscripten"),
            ]);
        return newQuickJSAsyncWASMModuleFromVariant(singlefileAsyncifyVariant);
    })();
    // A cached rejection would fail every later check without retrying initialization.
    promise.catch(() => {
        if (asyncModulePromise === promise) asyncModulePromise = null;
    });
    asyncModulePromise = promise;
    return promise;
}

/**
 * The process-wide chain serializes sandbox runs.
 *
 * Each asyncify WASM module instance has one suspension stack.
 * Awaiting a host capability unwinds and parks the WASM stack.
 * A second `evalCodeAsync` suspension before the first resumes corrupts the shared asyncify stack.
 * A later continuation can resume against a disposed context.
 * Resuming against a disposed context surfaces as `QuickJSUseAfterFree: Lifetime not alive`.
 *
 * withSandboxLock permits at most one suspended eval at a time.
 * Both `sandboxRunChain` handlers advance the chain, so a rejected run does not block later callers.
 */
let sandboxRunChain: Promise<unknown> = Promise.resolve();
function withSandboxLock<T>(fn: () => Promise<T>): Promise<T> {
    const run = sandboxRunChain.then(fn, fn);
    sandboxRunChain = run.then(
        () => undefined,
        () => undefined,
    );
    return run;
}

/**
 * The runner installs every capability the API exposes and enforces no manifest.
 * The caller gates a compiled check against its manifest before invoking the runner.
 */
export interface RunCompiledSmartNoteCheckOptions {
    compiledCheck: string;
    capabilities?: SmartNoteCapabilityApi;
    capabilityFactory?: SmartNoteCapabilityFactory;
    signal?: AbortSignal;
    timeoutMs?: number;
    heapLimitBytes?: number;
    stackLimitBytes?: number;
}

export interface RunCompiledSmartNoteCheckSuccess {
    ok: true;
    result: SmartNoteCheckResult;
}

export interface RunCompiledSmartNoteCheckFailure {
    ok: false;
    cancelled: false;
    error: string;
    network: boolean;
}

export interface RunCompiledSmartNoteCheckCancelled {
    ok: false;
    cancelled: true;
    error: string;
    network: false;
}

export type RunCompiledSmartNoteCheckResult =
    | RunCompiledSmartNoteCheckSuccess
    | RunCompiledSmartNoteCheckFailure
    | RunCompiledSmartNoteCheckCancelled;

const DEFAULT_TIMEOUT_MS = 2_000;
const DEFAULT_HEAP_LIMIT_BYTES = 8 * 1024 * 1024;
const DEFAULT_STACK_LIMIT_BYTES = 512 * 1024;
const MAX_COMPILED_CHECK_BYTES = 64 * 1024;
const MAX_SANDBOX_ERROR_CHARS = 2 * 1024;

// Capabilities that outlive VM interruption must observe signal.
// A tarpit request can keep the shared QuickJS module suspended past the sandbox budget.
// A suspended request blocks the next caller on the process-wide lock.
// The factory path receives the signal; the direct `capabilities` path cannot.
// `installCapabilityObject` races each host call against the signal.
function resolveCapabilitiesForRun(
    options: RunCompiledSmartNoteCheckOptions,
    signal: AbortSignal,
): SmartNoteCapabilityApi {
    if (options.capabilityFactory) {
        return options.capabilityFactory(signal);
    }
    if (options.capabilities) {
        return options.capabilities;
    }
    throw new Error("smart-note check requires capabilities");
}

function throwIfRunAborted(signal: AbortSignal): void {
    if (signal.aborted) {
        throw signal.reason ?? new Error("smart-note check aborted");
    }
}

export async function runCompiledSmartNoteCheck(
    options: RunCompiledSmartNoteCheckOptions,
): Promise<RunCompiledSmartNoteCheckResult> {
    if (options.signal?.aborted) return cancelledResult(options.signal.reason);
    if (Buffer.byteLength(options.compiledCheck, "utf8") > MAX_COMPILED_CHECK_BYTES) {
        return failureResult("compiled check exceeds 64 KiB", false);
    }
    // A non-finite deadline never interrupts a synchronous guest loop, and the timer cannot run while
    // that loop holds the thread; the run would wedge the process-wide lock.
    for (const [name, value] of [
        ["timeoutMs", options.timeoutMs],
        ["heapLimitBytes", options.heapLimitBytes],
        ["stackLimitBytes", options.stackLimitBytes],
    ] as const) {
        if (value !== undefined && !(Number.isFinite(value) && value > 0)) {
            return failureResult(`${name} must be a positive finite number`, false);
        }
    }
    // The lock initializes each check's timeout and host-capability controller.
    // A queued check's timeout starts after it acquires the lock.
    return withSandboxLock(() => runCompiledSmartNoteCheckLocked(options));
}

async function runCompiledSmartNoteCheckLocked(
    options: RunCompiledSmartNoteCheckOptions,
): Promise<RunCompiledSmartNoteCheckResult> {
    if (options.signal?.aborted) return cancelledResult(options.signal.reason);
    const timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS;
    const controller = new AbortController();
    let externallyCancelled = false;
    let executionTimedOut = false;
    const externalAbort = () => {
        externallyCancelled = true;
        controller.abort(options.signal?.reason);
    };
    options.signal?.addEventListener("abort", externalAbort, { once: true });
    const timer = setTimeout(() => {
        executionTimedOut = true;
        controller.abort(new Error("smart-note check timed out"));
    }, timeoutMs);
    try {
        throwIfRunAborted(controller.signal);
        const capabilities = resolveCapabilitiesForRun(options, controller.signal);
        // The timer cannot fire while a synchronous guest loop holds the thread, so the interrupt
        // predicate is the only stop for that loop; a monotonic clock keeps a wall-clock step from
        // stretching the budget.
        const deadline = performance.now() + timeoutMs;
        const quickjs = await getAsyncModule();
        throwIfRunAborted(controller.signal);
        const context = quickjs.newContext();
        try {
            context.runtime.setMemoryLimit(options.heapLimitBytes ?? DEFAULT_HEAP_LIMIT_BYTES);
            context.runtime.setMaxStackSize(options.stackLimitBytes ?? DEFAULT_STACK_LIMIT_BYTES);
            context.runtime.setInterruptHandler(
                () => controller.signal.aborted || performance.now() > deadline,
            );
            installCapabilityObject(context, capabilities, controller.signal);
            disableAmbientDynamicCode(context);
            const result = await evalCheck(context, options.compiledCheck);
            // A guest `try/catch` around a host call can swallow the abort and return a value.
            if (controller.signal.aborted) throw smartNoteAbortError(controller.signal);
            const checkResult = result as { met?: unknown } | null;
            if (!checkResult || typeof checkResult.met !== "boolean") {
                return failureResult("check() must return { met: boolean }", false);
            }
            return { ok: true, result: { met: checkResult.met } };
        } finally {
            context.dispose();
        }
    } catch (error) {
        if (externallyCancelled && !executionTimedOut) return cancelledResult(error);
        return failureResult(formatSandboxError(error), isSmartNoteNetworkError(error));
    } finally {
        clearTimeout(timer);
        options.signal?.removeEventListener("abort", externalAbort);
    }
}

function failureResult(error: string, network: boolean): RunCompiledSmartNoteCheckFailure {
    return { ok: false, cancelled: false, error: truncate(error), network };
}

function cancelledResult(reason: unknown): RunCompiledSmartNoteCheckCancelled {
    return {
        ok: false,
        cancelled: true,
        error: truncate(reason instanceof Error ? reason.message : String(reason ?? "cancelled")),
        network: false,
    };
}

function formatSandboxError(error: unknown): string {
    return error instanceof Error ? `${error.name}: ${error.message}` : String(error);
}

function truncate(value: string): string {
    return value.slice(0, MAX_SANDBOX_ERROR_CHARS);
}

function installCapabilityObject(
    context: QuickJSAsyncContext,
    cap: SmartNoteCapabilityApi,
    signal: AbortSignal,
): void {
    const capObject = context.newObject();
    try {
        installAsyncStringFunction(context, capObject, "__readFile", async (arg) => {
            const value = await raceWithAbort(cap.readFile(arg), signal);
            return value === null ? null : value;
        });
        installAsyncStringFunction(context, capObject, "__httpGet", async (arg) =>
            JSON.stringify(await raceWithAbort(cap.httpGet(arg), signal)),
        );
        installAsyncNoArgFunction(context, capObject, "__gitHeadSha", () =>
            raceWithAbort(cap.gitHeadSha(), signal),
        );
        installAsyncNoArgFunction(context, capObject, "__gitTag", () =>
            raceWithAbort(cap.gitTag(), signal),
        );
        installAsyncStringFunction(context, capObject, "__gitLog", async (arg) => {
            const opts = arg
                ? (JSON.parse(arg) as { maxCount?: number; path?: string; since?: string })
                : undefined;
            return JSON.stringify(await raceWithAbort(cap.gitLog(opts), signal));
        });
        context.setProp(context.global, "__eidnaraHostCap", capObject);
    } finally {
        capObject.dispose();
    }
}

// A host call that never settles would hold the asyncify suspension past the run budget.
// Rejecting on abort resumes the guest with a network-class error; the orphaned promise is dropped.
function raceWithAbort<T>(promise: Promise<T>, signal: AbortSignal): Promise<T> {
    if (signal.aborted) return Promise.reject(smartNoteAbortError(signal));
    let onAbort: (() => void) | undefined;
    const abort = new Promise<never>((_, reject) => {
        onAbort = () => reject(smartNoteAbortError(signal));
        signal.addEventListener("abort", onAbort, { once: true });
    });
    return Promise.race([promise, abort]).finally(() => {
        if (onAbort) signal.removeEventListener("abort", onAbort);
    });
}

function installAsyncStringFunction(
    context: QuickJSAsyncContext,
    target: QuickJSHandle,
    name: string,
    fn: (arg: string) => Promise<string | null>,
): void {
    const handle = context.newAsyncifiedFunction(name, async (argHandle) => {
        const arg = context.getString(argHandle);
        const value = await fn(arg);
        return value === null ? context.null : context.newString(value);
    });
    handle.consume((fnHandle) => context.setProp(target, name, fnHandle));
}

function installAsyncNoArgFunction(
    context: QuickJSAsyncContext,
    target: QuickJSHandle,
    name: string,
    fn: () => Promise<string | null>,
): void {
    const handle = context.newAsyncifiedFunction(name, async () => {
        const value = await fn();
        return value === null ? context.null : context.newString(value);
    });
    handle.consume((fnHandle) => context.setProp(target, name, fnHandle));
}

// Function literals expose dynamic-code constructors through their prototype chains.
// Poisoning all four prototypes' `constructor` closes `(function () {}).constructor("...")()`.
//
// A check must evaluate identically on every run with the same capability inputs. Reading the clock or
// the PRNG is the only way a check can flip without an external signal. `Date.now`, `Date()`, and
// `new Date()` throw; `Date.parse`, `Date.UTC`, and `new Date(value)` stay available for `authorDate`
// arithmetic. The native constructor is unreachable: the replacement owns `Date.prototype.constructor`.
const SANDBOX_PRELUDE = `
for (const fn of [function () {}, async function () {}, function* () {}, async function* () {}]) {
  Object.defineProperty(Object.getPrototypeOf(fn), "constructor", {
    value: undefined, writable: false, enumerable: false, configurable: false,
  });
}
{
  const frozen = Object.freeze;
  const poison = (name) => frozen(function () {
    throw new TypeError(name + " is nondeterministic and disabled in smart-note checks");
  });
  const nativeDate = globalThis.Date;
  const guardedDate = function Date(...args) {
    if (new.target === undefined || args.length === 0) poison("Date()")();
    return Reflect.construct(nativeDate, args, new.target);
  };
  guardedDate.prototype = nativeDate.prototype;
  guardedDate.parse = nativeDate.parse;
  guardedDate.UTC = nativeDate.UTC;
  guardedDate.now = poison("Date.now");
  Object.defineProperty(nativeDate.prototype, "constructor", {
    value: guardedDate, writable: false, enumerable: false, configurable: false,
  });
  Object.defineProperty(globalThis, "Date", {
    value: frozen(guardedDate), writable: false, enumerable: false, configurable: false,
  });
  Object.defineProperty(Math, "random", {
    value: poison("Math.random"), writable: false, enumerable: false, configurable: false,
  });
}`;

function disableAmbientDynamicCode(context: QuickJSAsyncContext): void {
    context.setProp(context.global, "eval", context.undefined);
    context.setProp(context.global, "Function", context.undefined);
    context
        .unwrapResult(context.evalCode(SANDBOX_PRELUDE, "sandbox-prelude.js", { type: "global" }))
        .dispose();
}

async function evalCheck(context: QuickJSAsyncContext, compiledCheck: string): Promise<unknown> {
    const wrapped = `
"use strict";
const module = { exports: {} };
const exports = module.exports;
const __mcCap = (() => {
  const hostCap = __eidnaraHostCap;
  delete globalThis.__eidnaraHostCap;
  if (Object.prototype.hasOwnProperty.call(globalThis, "__eidnaraHostCap")) {
    globalThis.__eidnaraHostCap = undefined;
  }
  return Object.freeze({
    readFile(path) { return hostCap.__readFile(String(path)); },
    httpGet(url) { return JSON.parse(hostCap.__httpGet(String(url))); },
    gitHeadSha() { return hostCap.__gitHeadSha(); },
    gitTag() { return hostCap.__gitTag(); },
    gitLog(opts) { return JSON.parse(hostCap.__gitLog(JSON.stringify(opts || {}))); },
  });
})();
${compiledCheck}
const __check = typeof check === "function" ? check : module.exports.check;
if (typeof __check !== "function") throw new Error("compiled check must define check(cap)");
const __result = __check(__mcCap);
if (!__result || typeof __result.met !== "boolean") throw new Error("check() must return { met: boolean }");
JSON.stringify({ met: __result.met });`;
    const evalResult = await context.evalCodeAsync(wrapped, "smart-note-check.js", {
        type: "global",
    });
    const resultHandle = context.unwrapResult(evalResult);
    try {
        return JSON.parse(context.getString(resultHandle));
    } finally {
        resultHandle.dispose();
    }
}
