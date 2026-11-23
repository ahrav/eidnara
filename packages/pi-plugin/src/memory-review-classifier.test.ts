import { describe, expect, it } from "bun:test";
import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import { ReviewClassifierError } from "@eidnara/opencode/shared/memory-review-classifier";
import { piHasReviewClassifier, piReviewClassifierExecutor } from "./memory-review-classifier";

const WORK = {
    lease: "0123456789abcdef0123456789abcdef",
    classifier: "typesafe/jev-latest",
    state: { memory: { text: "User stated: Deploy from the release branch." } },
    questions: { changes: { type: "bool" } },
    maxDurationMs: 10_000,
};

function context(registry: unknown): ExtensionContext {
    return { modelRegistry: registry } as unknown as ExtensionContext;
}

/** A registry with the classifier surface; `classify` answers with `result`. */
function registry(result: unknown, calls: unknown[] = []) {
    return {
        getModelOfType: (type: string, provider: string, id: string) =>
            type === "classifier" && provider === "typesafe" && id === "jev-latest"
                ? { provider, id }
                : undefined,
        classify: async (model: unknown, request: unknown) => {
            calls.push({ model, request });
            return result;
        },
    };
}

describe("piHasReviewClassifier", () => {
    it("is true only for a registry that can call the named classifier", () => {
        expect(piHasReviewClassifier(context(registry({})), "typesafe/jev-latest")).toBe(true);
        expect(piHasReviewClassifier(context(registry({})), "typesafe/other")).toBe(false);
        expect(piHasReviewClassifier(context(registry({})), "jev-latest")).toBe(false);
        // A Pi from before the classifier surface has a registry without it.
        expect(
            piHasReviewClassifier(context({ find: () => undefined }), "typesafe/jev-latest"),
        ).toBe(false);
        expect(piHasReviewClassifier(context(undefined), "typesafe/jev-latest")).toBe(false);
        // A classifier the registry lists without a credential leaves the chat model to decide.
        for (const [configured, expected] of [
            [true, true],
            [false, false],
        ] as const) {
            const listed = { ...registry({}), hasConfiguredAuth: () => configured };
            expect(piHasReviewClassifier(context(listed), "typesafe/jev-latest")).toBe(expected);
        }
    });
});

describe("piReviewClassifierExecutor", () => {
    it("sends the state and questions to the named classifier and returns its answers", async () => {
        const calls: unknown[] = [];
        const answers = { changes: { type: "bool", probability: 0.04 } };
        const execute = piReviewClassifierExecutor(
            context(registry({ stopReason: "stop", answers }, calls)),
        );
        expect(await execute(WORK, new AbortController().signal)).toEqual(answers);
        expect(calls).toEqual([
            {
                model: { provider: "typesafe", id: "jev-latest" },
                request: { state: WORK.state, questions: WORK.questions },
            },
        ]);
    });

    it("reports a classifier that is absent, failed, or aborted as a closed code", async () => {
        const failing = async (registryValue: unknown, signal = new AbortController().signal) => {
            try {
                await piReviewClassifierExecutor(context(registryValue))(WORK, signal);
            } catch (error) {
                return error instanceof ReviewClassifierError ? error.code : "unexpected";
            }
            return "no error";
        };
        expect(await failing({ find: () => undefined })).toBe("provider_unavailable");
        expect(
            await failing(registry({ stopReason: "error", errorMessage: "401 key rejected" })),
        ).toBe("provider_unavailable");
        expect(await failing(registry({ stopReason: "stop" }))).toBe("provider_unavailable");
        expect(await failing(registry({ stopReason: "aborted" }))).toBe("cancelled");
        const aborted = new AbortController();
        aborted.abort();
        expect(await failing(registry({ stopReason: "stop", answers: {} }), aborted.signal)).toBe(
            "cancelled",
        );
    });
});
