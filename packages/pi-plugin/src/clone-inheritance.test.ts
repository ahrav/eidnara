/// <reference types="bun-types" />

import { describe, expect, it } from "bun:test";
import { resetKernelClientsForTest } from "@eidnara/opencode/hooks/context/kernel-transport";
import { KernelClient, TokenCache } from "@eidnara/opencode/shared/kernel-client";
import {
    FakeKernel,
    FakeKernelTransport,
} from "@eidnara/opencode/shared/kernel-client-testing/fake-kernel";
import { handlePiCloneSessionStart } from "./clone-inheritance";
import { createPiKernelClientResolver, resetPiKernelClientsForTest } from "./kernel-client-pi";

describe("Pi clone state inheritance", () => {
    it("gives a forked session no parent token and reads from tip first", async () => {
        resetKernelClientsForTest();
        resetPiKernelClientsForTest();
        const kernel = new FakeKernel();
        kernel.seedDecision({
            object_id: `mem_${"a".repeat(32)}`,
            decision_kind: "NAMING",
            summary: "Parent read this.",
        });
        const projectRoot = "/project";
        const parentTokens = new TokenCache();
        const parent = new KernelClient({
            transport: new FakeKernelTransport(kernel),
            enabled: true,
            sessionId: "source",
            projectRoot,
            tokens: parentTokens,
        });
        await parent.read({ surface: "explicit_search", gated: false });
        expect(parentTokens.size(projectRoot)).toBe(1);
        expect(parentTokens.knownAsOfFor(projectRoot)).toBe(1);

        await handlePiCloneSessionStart(
            { reason: "fork" },
            { sessionManager: { getSessionId: () => "clone" } },
            { writeLog: () => undefined },
        );

        const resolver = createPiKernelClientResolver(() => ({}));
        const clone = resolver({ sessionId: "clone", projectRoot });
        expect(clone.tokens).not.toBe(parentTokens);
        expect(clone.tokens.size(projectRoot)).toBe(0);
        expect(clone.tokens.knownAsOfFor(projectRoot)).toBeUndefined();

        const cloneTransport = new FakeKernelTransport(kernel);
        const cloneClient = new KernelClient({
            transport: cloneTransport,
            enabled: true,
            sessionId: "clone",
            projectRoot,
            tokens: clone.tokens,
        });
        await cloneClient.read({ surface: "explicit_search", gated: false });
        expect(cloneTransport.calls[0]?.body).toMatchObject({ as_of: null });
        expect(clone.tokens.size(projectRoot)).toBe(1);
        expect(parentTokens.size(projectRoot)).toBe(1);
        resetPiKernelClientsForTest();
    });

    it("fails open with one actionable structured log line", async () => {
        const messages: string[] = [];

        const result = await handlePiCloneSessionStart(
            { reason: "fork" },
            { sessionManager: {} },
            { writeLog: (message) => messages.push(message) },
        );

        expect(result).toBe(false);
        expect(messages).toHaveLength(1);
        expect(messages[0]).toContain("stage=resolve-destination");
    });
});
