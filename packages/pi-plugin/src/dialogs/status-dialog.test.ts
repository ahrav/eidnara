import { describe, expect, it } from "bun:test";
import { resolveProjectIdentity } from "@eidnara/opencode/features/context/project-identity";
import type { RustSessionStatus } from "@eidnara/opencode/plugin/rpc-handlers";
import {
    ANTI_MEMORY_CATEGORY,
    renderAntiMemoryContent,
} from "@eidnara/opencode/shared/kernel-client/anti-memory";
import { fakeContext, fakeKernelResolver } from "../__tests__/test-utils";
import { buildPiStatusDetail, showStatusDialog } from "./status-dialog";

const DAEMON_STATUS: RustSessionStatus = {
    usage: { current_total_input_tokens: 42_000, context_limit_tokens: 100_000 },
    compartment_count: 4,
    compartment_tokens: 23,
    pending_drop_count: 2,
    wrapup_active: true,
    tail_hygiene: {
        u: 65_100,
        t: 100_000,
        severity: 0.651,
        evaluable: true,
        generation_invalidated: false,
        baseline_generation: 7,
        computed_at_ms: 123,
    },
};

const fakePi = { getAllTools: () => [] } as never;

function deps(kernelClient = fakeKernelResolver().kernelClient) {
    return { kernelClient, projectIdentity: resolveProjectIdentity(process.cwd()) };
}

function reservedWindowContext(sessionId: string) {
    return {
        ...fakeContext(sessionId),
        model: { provider: "anthropic", id: "claude", contextWindow: 100_000, maxTokens: 20_000 },
        getContextUsage: () => ({ tokens: 50_000, percent: 50, contextWindow: 100_000 }),
        getSystemPrompt: () => "system prompt",
    };
}

/** A Pi UI whose `custom` renders the dialog once at `width` and records the lines. */
function renderingContext(sessionId: string, width: number, keepOpen = false) {
    const rendered: string[][] = [];
    let component: { render: (width: number) => string[]; dispose?: () => void } | undefined;
    const ctx = {
        ...fakeContext(sessionId),
        ui: {
            async custom(factory: unknown) {
                const makeComponent = factory as (
                    tui: { requestRender: () => void },
                    theme: {
                        fg: (_name: string, text: string) => string;
                        bold: (text: string) => string;
                    },
                    keybindings: unknown,
                    done: (value: undefined) => void,
                ) => { render: (width: number) => string[]; dispose?: () => void };
                component = makeComponent(
                    { requestRender: () => undefined },
                    { fg: (_name, text) => text, bold: (text) => text },
                    undefined,
                    () => undefined,
                );
                rendered.push(component.render(width));
                // Disposing clears the dialog's refresh interval; a `keepOpen` caller drives `refresh` and disposes itself.
                if (!keepOpen) component.dispose?.();
                return undefined;
            },
        },
        getSystemPrompt: () => "system prompt",
    };
    const refresh = async () => {
        const open = component as unknown as { refresh: () => Promise<void> };
        await open.refresh();
        rendered.push((component as { render: (width: number) => string[] }).render(width));
    };
    return {
        ctx,
        text: () => rendered.flat().join("\n"),
        reset: () => (rendered.length = 0),
        refresh,
        dispose: () => component?.dispose?.(),
    };
}

function daemonSource(initial: RustSessionStatus, read = async () => initial) {
    return { initial, read };
}

describe("Pi status dialog", () => {
    it("displays live usage against the output-reserved safe window without a daemon status", () => {
        const sessionId = "ses-status-reserved-window";
        const detail = buildPiStatusDetail(
            fakePi,
            reservedWindowContext(sessionId) as never,
            deps(),
            sessionId,
            fakeKernelResolver().kernel.snapshot("explicit_search"),
        );
        expect(detail.contextLimit).toBe(80_000);
        expect(detail.usagePercentage).toBe(62.5);
        expect(detail.inputTokens).toBe(50_000);
    });

    it("maps the daemon status onto usage, compartments, pending drops, hygiene, and historian", () => {
        const sessionId = "ses-status-daemon";
        const detail = buildPiStatusDetail(
            fakePi,
            reservedWindowContext(sessionId) as never,
            deps(),
            sessionId,
            fakeKernelResolver().kernel.snapshot("explicit_search"),
            DAEMON_STATUS,
        );
        expect(detail.inputTokens).toBe(42_000);
        expect(detail.contextLimit).toBe(100_000);
        expect(detail.usagePercentage).toBe(42);
        expect(detail.compartmentCount).toBe(4);
        expect(detail.compartmentTokens).toBe(23);
        expect(detail.pendingOpsCount).toBe(2);
        expect(detail.historianRunning).toBe(true);
        expect(detail.tailHygiene).toMatchObject({ u: 65_100, t: 100_000, evaluable: true });
        expect(detail.historyBlockTokens).toBe(23);
        // Compartments carry the daemon's count; the conversation bucket absorbs the remainder.
        expect(
            detail.systemPromptTokens +
                detail.compartmentTokens +
                detail.conversationTokens +
                detail.toolDefinitionTokens,
        ).toBe(42_000);
    });

    it("holds storage-only fields at their neutral value", () => {
        const sessionId = "ses-status-neutral";
        const detail = buildPiStatusDetail(
            fakePi,
            reservedWindowContext(sessionId) as never,
            deps(),
            sessionId,
            fakeKernelResolver().kernel.snapshot("explicit_search"),
            DAEMON_STATUS,
        );
        expect(detail).toMatchObject({
            memoryBlockCount: 0,
            sessionNoteCount: 0,
            readySmartNoteCount: 0,
            lastTransformError: null,
            isSubagent: false,
            activeTags: 0,
            droppedTags: 0,
            totalTags: 0,
            activeBytes: 0,
            factTokens: 0,
            memoryTokens: 0,
            docsTokens: 0,
            profileTokens: 0,
            toolCallTokens: 0,
            newWorkTokens: 0,
            totalInputTokens: 0,
        });
    });

    it("renders the daemon hygiene ratio and the window derivation", async () => {
        const sessionId = "ses-status-hygiene";
        const { ctx, text } = renderingContext(sessionId, 90);
        await showStatusDialog(fakePi, ctx as never, deps(), daemonSource(DAEMON_STATUS));
        expect(text()).toContain("Hygiene 65.1% · 65,100 / 100,000 tok");
        expect(text()).toContain("Conversation includes model Reasoning; hygiene excludes it");
        expect(text()).toContain("Counts: 4 compartments");
        expect(text()).toContain("Pending drops: 2");
        expect(text()).toContain("Historian: running");
        expect(text()).toContain("Window ");
        expect(text()).not.toContain("Context:");
    });

    it("re-reads the daemon status on each refresh and keeps the last answer when a read fails", async () => {
        const sessionId = "ses-status-refresh";
        const settled: RustSessionStatus = {
            ...DAEMON_STATUS,
            compartment_count: 5,
            pending_drop_count: 0,
            wrapup_active: false,
        };
        const answers: Array<() => Promise<RustSessionStatus>> = [
            async () => settled,
            async () => {
                throw new Error("socket closed");
            },
        ];
        const { ctx, text, reset, refresh, dispose } = renderingContext(sessionId, 90, true);
        try {
            await showStatusDialog(
                fakePi,
                ctx as never,
                deps(),
                daemonSource(DAEMON_STATUS, () => (answers.shift() ?? (async () => settled))()),
            );
            expect(text()).toContain("Historian: running");

            reset();
            await refresh();
            expect(text()).toContain("Counts: 5 compartments");
            expect(text()).toContain("Pending drops: 0");
            expect(text()).toContain("Historian: idle");

            reset();
            await refresh();
            expect(text()).toContain("Counts: 5 compartments");
            expect(text()).toContain("Historian: idle");
        } finally {
            dispose();
        }
    });

    it("reports the kernel state and row count instead of claim-lane counts", async () => {
        const sessionId = "ses-status-kernel";
        const fake = fakeKernelResolver();
        fake.kernel.seedDecision({
            object_id: `mem_${"1".repeat(32)}`,
            decision_kind: "NAMING",
            summary: "One memory.",
        });
        const { ctx, text, reset } = renderingContext(sessionId, 90);

        await showStatusDialog(fakePi, ctx as never, deps(fake.kernelClient));
        expect(text()).toContain("1 memories (0 injected, available)");
        expect(fake.transport.calls[0]?.body).toMatchObject({
            surface: "explicit_search",
            gated: true,
        });

        fake.transport.fileExists = false;
        reset();
        await showStatusDialog(fakePi, ctx as never, deps(fake.kernelClient));
        expect(text()).toContain("0 memories (0 injected, unavailable:daemon_absent)");
    });

    it("marks a truncated read's memory count as a lower bound", async () => {
        const sessionId = "ses-status-truncated";
        const fake = fakeKernelResolver();
        fake.kernel.seedDecision({
            object_id: `mem_${"2".repeat(32)}`,
            decision_kind: "NAMING",
            summary: "One memory.",
        });
        fake.kernel.readTruncated = true;
        const { ctx, text } = renderingContext(sessionId, 90);
        await showStatusDialog(fakePi, ctx as never, deps(fake.kernelClient));
        expect(text()).toContain("1+ memories (0 injected, available)");
    });

    it("an expired anti-memory stays out of the memory count", () => {
        const sessionId = "ses-status-expired-anti";
        const fake = fakeKernelResolver();
        fake.kernel.seedDecision({
            object_id: `mem_${"a".repeat(32)}`,
            decision_kind: "PROJECT_RULES",
            summary: "Always use Bun for builds",
        });
        for (const [idChar, expiresAt] of [
            ["b", 1],
            ["c", Date.now() + 60_000],
        ] as const) {
            fake.kernel.seedDecision({
                object_id: `mem_${idChar.repeat(32)}`,
                decision_kind: ANTI_MEMORY_CATEGORY,
                summary: renderAntiMemoryContent({
                    trigger: "asked to bypass the daemon",
                    rejectedStrategy: "write straight to the store",
                    rejectionReason: "the daemon owns commit ordering",
                    expiresAt,
                }),
            });
        }

        const detail = buildPiStatusDetail(
            fakePi,
            { ...fakeContext(sessionId), getSystemPrompt: () => "system prompt" } as never,
            deps(fake.kernelClient),
            sessionId,
            fake.kernel.snapshot("explicit_search"),
        );
        expect(detail.memoryCount).toBe(2);
    });
});
