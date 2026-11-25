import { describe, expect, it } from "bun:test";
import { AuthStorage, type ExtensionContext, ModelRegistry } from "@earendil-works/pi-coding-agent";
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
        hasConfiguredAuth: () => true,
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
        // A registry that cannot report a configured credential is one without a classifier.
        const { hasConfiguredAuth: _unused, ...unreported } = registry({});
        expect(piHasReviewClassifier(context(unreported), "typesafe/jev-latest")).toBe(false);
        // A classifier the registry lists without a credential leaves the chat model to decide.
        const listed = { ...registry({}), hasConfiguredAuth: () => false };
        expect(piHasReviewClassifier(context(listed), "typesafe/jev-latest")).toBe(false);
    });

    // `docs/memory-reviewer-operations.md` ("Stage 1 by a typed classifier") states that the Pi this package pins and CI tests has a registry without the classifier surface. A pin that moves to a Pi with the surface changes this case, that sentence, and the peer range together.
    it("finds no classifier in the registry the pinned Pi builds", async () => {
        const pinned = ModelRegistry.inMemory(AuthStorage.inMemory()) as unknown as Record<
            string,
            unknown
        >;
        expect(typeof pinned.hasConfiguredAuth).toBe("function");
        expect(typeof pinned.getModelOfType).toBe("undefined");
        expect(typeof pinned.classify).toBe("undefined");
        expect(piHasReviewClassifier(context(pinned), "typesafe/jev-latest")).toBe(false);
        const call = piReviewClassifierExecutor(context(pinned))(
            WORK,
            new AbortController().signal,
        );
        await expect(call).rejects.toBeInstanceOf(ReviewClassifierError);
        await expect(call).rejects.toMatchObject({ code: "provider_unavailable" });
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

    it("reports cancelled, without a classifier call, once the classifier's credential is gone", async () => {
        const calls: unknown[] = [];
        const answering = registry({ stopReason: "stop", answers: {} }, calls);
        let configured = true;
        const execute = piReviewClassifierExecutor(
            context({ ...answering, hasConfiguredAuth: () => configured }),
        );
        expect(await execute(WORK, new AbortController().signal)).toEqual({});
        expect(calls).toHaveLength(1);
        configured = false;
        const refused = await execute(WORK, new AbortController().signal).catch((error) => error);
        expect(refused).toBeInstanceOf(ReviewClassifierError);
        expect((refused as ReviewClassifierError).code).toBe("cancelled");
        expect(calls).toHaveLength(1);
    });
});
