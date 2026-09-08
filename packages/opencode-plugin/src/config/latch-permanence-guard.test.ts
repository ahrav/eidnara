import { describe, expect, test } from "bun:test";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const REPOSITORY_ROOT = join(dirname(fileURLToPath(import.meta.url)), "../../../..");
const SOURCE_ROOTS = ["packages/opencode-plugin/src", "packages/pi-plugin/src"] as const;

type Classification = "VERDICT" | "DIAGNOSTIC" | "PUBLICATION";

type KnownSlot = {
    classification: Classification;
    reason: string;
};

/**
 * KNOWN_SLOTS classifies module-level mutable slots that can retain a capability, absence, or failure verdict.
 */
const KNOWN_SLOTS: Record<string, KnownSlot> = {
    "packages/opencode-plugin/src/features/context/compaction-marker.ts:cachedSchemaCompatible": {
        classification: "VERDICT",
        reason: "DEFECT: a transient PRAGMA/read failure is cached as incompatible until the writable DB is closed.",
    },
    "packages/opencode-plugin/src/features/context/smart-notes/sandbox-runner.ts:asyncModulePromise":
        {
            classification: "VERDICT",
            reason: "DEFECT: a rejected dynamic-import/WASM-init promise is retained for every later smart-note check.",
        },
    "packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.ts:ctxReduceRegisteredGlobally":
        {
            classification: "VERDICT",
            reason: "Correct by scope: tool registration is resolved once at plugin boot and cannot change while that instance runs.",
        },
    "packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.ts:availabilityBySession": {
        classification: "VERDICT",
        reason: "Correct by contract: the first persisted user message freezes that session's tool surface.",
    },
    "packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.ts:permissionDeniedBySession":
        {
            classification: "DIAGNOSTIC",
            reason: "Repeatedly assigned on each cache-busting permission read; later reads replace an earlier denial.",
        },
    "packages/opencode-plugin/src/shared/token-estimator.ts:tokenizerLoadAttempted": {
        classification: "VERDICT",
        reason: "Saved: gates only the synchronous bare-require path; preloadTokenizer's installed-package search still runs after a synchronous failure.",
    },
    "packages/opencode-plugin/src/shared/token-estimator.ts:tokenizerPreloadAttempted": {
        classification: "VERDICT",
        reason: "Correct by contract: preload is a one-shot warm; after it fails the process keeps the deterministic heuristic fallback until restart, as warnTokenizerFallback documents.",
    },
    "packages/opencode-plugin/src/shared/token-estimator.ts:tokenizerPoisoned": {
        classification: "VERDICT",
        reason: "Correct by contract: after an encode failure the process keeps the heuristic estimator until restart, so identical text never alternates between exact and approximate counts.",
    },
    "packages/opencode-plugin/src/hooks/context/module-transport.ts:stateSyncCapabilityCache": {
        classification: "VERDICT",
        reason: "Saved: invalidateStateSyncCapabilities runs on NEED_FULL_SYNC and connection invalidation before the next capability probe.",
    },
    "packages/opencode-plugin/src/plugin/conflict-warning-hook.ts:cachedDesktopStateByDir": {
        classification: "VERDICT",
        reason: "Correct by scope: its deciding startup-warning paths run once per plugin boot, so a later Desktop state-file write is not observed by design.",
    },
    "packages/opencode-plugin/src/shared/models-dev-cache.ts:authRewarmDone": {
        classification: "VERDICT",
        reason: "Saved: refreshModelLimitsAfterAuthOnce resets the latch when its warm fails.",
    },
    "packages/opencode-plugin/src/features/context/fail-closed-block.ts:lastHookInitFailure": {
        classification: "DIAGNOSTIC",
        reason: "Most-recent boot diagnostic: recordHookInitFailure overwrites it and clearHookInitFailure resets it.",
    },
    "packages/opencode-plugin/src/shared/token-estimator.ts:tokenizerLoadPromise": {
        classification: "PUBLICATION",
        reason: "In-flight handle only: finally clears it after the load settles, so it cannot retain a failure verdict.",
    },
    "packages/opencode-plugin/src/shared/exit-abort-registry.ts:listenerRegistered": {
        classification: "PUBLICATION",
        reason: "One-time process exit-listener installation, not a capability or failure verdict.",
    },
    "packages/opencode-plugin/src/shared/models-dev-cache.ts:apiCache": {
        classification: "PUBLICATION",
        reason: "Publishes last-known-good model metadata, not a failure verdict; refresh writes a later successful value.",
    },
    "packages/opencode-plugin/src/shared/models-dev-cache.ts:persistSeedLoaded": {
        classification: "VERDICT",
        reason: "Bounded: the persisted file is only a cold-start seed. A failed first read skips the seed for this process; the SDK warm path fills apiCache and rewrites the file, after which the latch is unreachable because apiCache is non-null.",
    },
};

function sourceFiles(directory: string): string[] {
    const files: string[] = [];
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
        const path = join(directory, entry.name);
        if (entry.isDirectory()) {
            files.push(...sourceFiles(path));
        } else if (entry.isFile() && path.endsWith(".ts") && !path.endsWith(".test.ts")) {
            files.push(path);
        }
    }
    return files;
}

function declaredOneShotSlots(source: string): string[] {
    const declarations =
        /^(?:export\s+)?let\s+([A-Za-z_$][\w$]*)\b[^\n]*(?:=\s*(?:null|false|true)|Promise<)/gm;
    const verdictName =
        /(?:attempted|availability|compatible|cooldown|disabled|failed|failure|latch|loaded|missing|permission|poisoned|promise|registered|unavailable)/i;
    const moduleSlots = [...source.matchAll(declarations)].map((match) => match[1]);
    // Provider-local permanent failures short-circuit later provider calls without re-probing.
    const providerLatches = [
        ...source.matchAll(/^\s*private\s+(permanentFailure)\s*=\s*false;/gm),
    ].map((match) => match[1]);
    return [...moduleSlots, ...providerLatches].filter(
        (name): name is string => name !== undefined && verdictName.test(name),
    );
}

describe("latch permanence classification guard", () => {
    test("classifies every production one-shot verdict-shaped slot", () => {
        const inlineTestBlocks: string[] = [];
        const discovered = new Set<string>();
        for (const root of SOURCE_ROOTS) {
            // A root that has not landed yet contributes no slots.
            if (!existsSync(join(REPOSITORY_ROOT, root))) continue;
            for (const file of sourceFiles(join(REPOSITORY_ROOT, root))) {
                const source = readFileSync(file, "utf8");
                if (/from\s+["']bun:test["']/.test(source)) {
                    inlineTestBlocks.push(relative(REPOSITORY_ROOT, file));
                }
                for (const slot of declaredOneShotSlots(source)) {
                    discovered.add(`${relative(REPOSITORY_ROOT, file)}:${slot}`);
                }
            }
        }

        expect(inlineTestBlocks).toEqual([]);
        const unclassified = [...discovered].filter((slot) => !(slot in KNOWN_SLOTS)).sort();
        expect(
            unclassified,
            `Classify each new one-shot verdict-shaped slot in KNOWN_SLOTS: ${unclassified.join(", ")}`,
        ).toEqual([]);
        expect(Object.keys(KNOWN_SLOTS).length).toBeGreaterThan(0);
    });
});
