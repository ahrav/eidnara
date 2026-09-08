import { describe, expect, test } from "bun:test";
import Tokenizer from "ai-tokenizer";
import * as claude from "ai-tokenizer/encoding/claude";
import { concatSessionMessages } from "./run-context-concat";
import type { DumpMessage } from "./types";

const tokenizer = new Tokenizer(claude);

function text(role: string, body: string): DumpMessage {
    return { info: { role }, parts: [{ type: "text", text: body }] };
}

function toolsOnly(count: number): DumpMessage {
    const parts = Array.from({ length: count }, () => ({ type: "tool" }));
    return { info: { role: "assistant" }, parts };
}

function empty(role: string): DumpMessage {
    return { info: { role }, parts: [] };
}

describe("concatSessionMessages", () => {
    test("totalTokens is the count of the joined output and never exceeds the budget", () => {
        const messages = Array.from({ length: 40 }, (_, i) =>
            text(i % 2 === 0 ? "user" : "assistant", `message ${i} lorem ipsum dolor sit amet`),
        );
        const renderedLines = messages.map(
            (_, i) =>
                `[${i}] ${i % 2 === 0 ? "User" : "Assistant"}: message ${i} lorem ipsum dolor sit amet`,
        );
        const perLineSum = renderedLines.reduce((sum, line) => sum + tokenizer.count(line), 0);
        const joinedAll = tokenizer.count(renderedLines.join("\n"));
        // Separators make the joined count larger than the sum.
        expect(joinedAll).toBeGreaterThan(perLineSum);

        for (const budget of [perLineSum - 5, perLineSum, perLineSum + 5, joinedAll]) {
            const page = concatSessionMessages(messages, budget);
            expect(page.totalTokens).toBe(tokenizer.count(page.output));
            expect(page.totalTokens).toBeLessThanOrEqual(budget);
        }

        const full = concatSessionMessages(messages, joinedAll);
        expect(full.messagesWithContent).toBe(messages.length);
        expect(full.hasMore).toBe(false);
    });

    test("a rejected mid-page tool summary does not advance endIndex past the tool run", () => {
        const messages = [
            text("user", "first question about the code"),
            toolsOnly(2),
            toolsOnly(1),
            text("assistant", "here is the answer"),
        ];
        const firstLine = "[0] User: first question about the code";
        // Budget admits the first line but not the tool summary that follows it.
        const budget = tokenizer.count(firstLine);

        const page = concatSessionMessages(messages, budget);
        expect(page.output).toBe(firstLine);
        expect(page.endIndex).toBe(0);
        expect(page.messagesWithContent).toBe(1);
        expect(page.hasMore).toBe(true);

        // Resuming at endIndex + 1 re-reads the tool run instead of skipping it.
        const next = concatSessionMessages(messages, 1000, page.endIndex + 1);
        expect(next.output).toBe("[1] Assistant: 3 tool calls\n[3] Assistant: here is the answer");
        expect(next.messagesWithContent).toBe(3);
        expect(next.hasMore).toBe(false);
    });

    test("a rejected trailing tool summary leaves hasMore true", () => {
        const messages = [text("user", "first question about the code"), toolsOnly(2)];
        const firstLine = "[0] User: first question about the code";
        const budget = tokenizer.count(firstLine);

        const page = concatSessionMessages(messages, budget);
        expect(page.output).toBe(firstLine);
        expect(page.endIndex).toBe(0);
        expect(page.hasMore).toBe(true);
    });

    test("an admitted tool run counts its messages and advances endIndex to its last message", () => {
        const messages = [toolsOnly(1), toolsOnly(2), text("user", "next")];
        const page = concatSessionMessages(messages, 1000);
        expect(page.output).toBe("[0] Assistant: 3 tool calls\n[2] User: next");
        expect(page.messagesWithContent).toBe(3);
        expect(page.endIndex).toBe(2);
        expect(page.hasMore).toBe(false);
    });

    test("messages without text or tool parts are consumed so hasMore reaches false", () => {
        const messages = [text("user", "hello"), empty("user"), empty("system")];
        const page = concatSessionMessages(messages, 1000);
        expect(page.output).toBe("[0] User: hello");
        expect(page.endIndex).toBe(2);
        expect(page.hasMore).toBe(false);
    });

    test("a first message that exceeds the budget throws instead of reporting it consumed", () => {
        const big = text("user", "word ".repeat(200));
        expect(() => concatSessionMessages([big], 10)).toThrow(
            /Message 0 needs \d+ tokens on its own, which exceeds the budget of 10/,
        );
        // Same message reached by resuming after an admitted first page.
        const messages = [text("user", "short"), big];
        const first = concatSessionMessages(messages, 10);
        expect(first.endIndex).toBe(0);
        expect(first.hasMore).toBe(true);
        expect(() => concatSessionMessages(messages, 10, first.endIndex + 1)).toThrow(
            /Message 1 needs/,
        );
    });

    test("a first tool run whose summary exceeds the budget throws", () => {
        expect(() => concatSessionMessages([toolsOnly(3)], 1)).toThrow(/Message 0 needs/);
        expect(() => concatSessionMessages([toolsOnly(3), text("user", "x")], 1)).toThrow(
            /Message 0 needs/,
        );
    });

    test("an offset past the end reports no progress and no more pages", () => {
        const messages = [text("user", "hello")];
        const page = concatSessionMessages(messages, 1000, 1);
        expect(page.output).toBe("");
        expect(page.startIndex).toBe(1);
        expect(page.endIndex).toBe(0);
        expect(page.messagesWithContent).toBe(0);
        expect(page.hasMore).toBe(false);
    });
});
