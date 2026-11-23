import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import {
    ReviewClassifierError,
    type ReviewClassifierExecutor,
} from "@eidnara/opencode/shared/memory-review-classifier";

/**
 * The classifier surface of Pi's model registry, as Pi 1.6.0 has it. The Pi this package is
 * built against predates it, so the plugin reads it structurally and treats a registry without
 * it as a harness that has no classifier.
 */
interface ClassifierRegistry {
    getModelOfType(type: "classifier", provider: string, id: string): unknown;
    /** Whether the user configured a credential for the model's provider. */
    hasConfiguredAuth(model: unknown): boolean;
    classify(
        model: unknown,
        context: { state: Record<string, unknown>; questions: Record<string, unknown> },
        options?: { signal?: AbortSignal },
    ): Promise<{
        answers?: Record<string, unknown>;
        stopReason?: string;
        errorMessage?: string;
    }>;
}

function classifierRegistry(ctx: ExtensionContext): ClassifierRegistry | undefined {
    const registry = ctx.modelRegistry as unknown as Partial<ClassifierRegistry> | undefined;
    return typeof registry?.classify === "function" &&
        typeof registry.getModelOfType === "function" &&
        typeof registry.hasConfiguredAuth === "function"
        ? (registry as ClassifierRegistry)
        : undefined;
}

/** Whether this Pi's registry lists the classifier `classifier` (`provider/model`) and holds a credential the user configured for it. */
export function piHasReviewClassifier(ctx: ExtensionContext, classifier: string): boolean {
    const registry = classifierRegistry(ctx);
    const separator = classifier.indexOf("/");
    if (!registry || separator <= 0) return false;
    try {
        const model = registry.getModelOfType(
            "classifier",
            classifier.slice(0, separator),
            classifier.slice(separator + 1),
        );
        return Boolean(model) && registry.hasConfiguredAuth(model);
    } catch {
        return false;
    }
}

/**
 * Runs one classifier request inside the active harness, which resolves the model and its
 * credential. The answers go back to the daemon as Pi returned them; a failure crosses as one
 * content-free code.
 */
export function piReviewClassifierExecutor(ctx: ExtensionContext): ReviewClassifierExecutor {
    return async (work, signal) => {
        const registry = classifierRegistry(ctx);
        const separator = work.classifier.indexOf("/");
        const model = registry?.getModelOfType(
            "classifier",
            work.classifier.slice(0, separator),
            work.classifier.slice(separator + 1),
        );
        if (!registry || !model) throw new ReviewClassifierError("provider_unavailable");
        const result = await registry.classify(
            model,
            { state: work.state, questions: work.questions },
            { signal },
        );
        if (signal.aborted || result.stopReason === "aborted")
            throw new ReviewClassifierError("cancelled");
        if (result.stopReason !== "stop" || !result.answers)
            throw new ReviewClassifierError("provider_unavailable");
        return result.answers;
    };
}
