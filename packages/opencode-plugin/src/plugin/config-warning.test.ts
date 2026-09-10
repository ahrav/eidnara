import { afterEach, describe, expect, it, mock } from "bun:test";
import { __ignoredNotificationTest } from "../hooks/context/send-session-notification";
import { __resetNotificationStateForTests } from "../shared/rpc-notifications";
import { createConfigWarningDelivery, formatConfigWarning } from "./config-warning";

const WARNING = formatConfigWarning(["cache_ttl must be a duration"]);

/** A rejecting `prompt` yields the `failed` disposition; the default-title `skipped` path waits on a production backoff and is covered by `safe-notification-target.test.ts`. */
function client(options: { title: string; sessions?: string[]; promptRejects?: boolean }) {
    const prompt = mock(async () => {
        if (options.promptRejects) throw new Error("prompt unavailable");
        return {};
    });
    const get = mock(async () => ({ title: options.title }));
    const messages = mock(async () => [
        {
            info: {
                role: "assistant",
                agent: "builder",
                model: { providerID: "anthropic", modelID: "claude-fable" },
            },
        },
    ]);
    const list = mock(async () => (options.sessions ?? []).map((id) => ({ id })));
    return { client: { session: { prompt, get, messages, list } }, prompt, get, list };
}

describe("createConfigWarningDelivery", () => {
    afterEach(() => {
        __ignoredNotificationTest.reset();
        __resetNotificationStateForTests();
    });

    it("stays pending when no session exists and delivers to the first prompted session", async () => {
        __ignoredNotificationTest.setMidTurnDetector(() => false);
        const fake = client({ title: "Investigating the flaky cache" });
        const delivery = createConfigWarningDelivery(fake.client, WARNING);

        await delivery.deliverToFirstSession();
        expect(fake.list).toHaveBeenCalledTimes(1);
        expect(fake.prompt).not.toHaveBeenCalled();
        expect(delivery.pending).toBe(true);

        await delivery.deliverTo("ses_first_prompt");
        expect(fake.prompt).toHaveBeenCalledTimes(1);
        expect((fake.prompt.mock.calls[0]?.[0] as { path: { id: string } }).path.id).toBe(
            "ses_first_prompt",
        );
        expect(delivery.pending).toBe(false);

        await delivery.deliverTo("ses_second_prompt");
        expect(fake.prompt).toHaveBeenCalledTimes(1);
    });

    it("delivers once to the first listed session and ignores later prompts", async () => {
        __ignoredNotificationTest.setMidTurnDetector(() => false);
        const fake = client({
            title: "Investigating the flaky cache",
            sessions: ["ses_a", "ses_b"],
        });
        const delivery = createConfigWarningDelivery(fake.client, WARNING);

        await delivery.deliverToFirstSession();
        expect(fake.prompt).toHaveBeenCalledTimes(1);
        expect((fake.prompt.mock.calls[0]?.[0] as { path: { id: string } }).path.id).toBe("ses_a");

        await delivery.deliverTo("ses_b");
        expect(fake.prompt).toHaveBeenCalledTimes(1);
        expect(delivery.pending).toBe(false);
    });

    it("keeps the warning pending after a failed delivery and retries on the next prompt", async () => {
        __ignoredNotificationTest.setMidTurnDetector(() => false);
        const fake = client({ title: "Investigating the flaky cache", promptRejects: true });
        const delivery = createConfigWarningDelivery(fake.client, WARNING);

        await delivery.deliverTo("ses_flaky");
        expect(fake.prompt).toHaveBeenCalledTimes(1);
        expect(delivery.pending).toBe(true);

        fake.prompt.mockImplementation(async () => ({}));
        await delivery.deliverTo("ses_flaky");
        expect(fake.prompt).toHaveBeenCalledTimes(2);
        expect(delivery.pending).toBe(false);
    });

    it("shares one in-flight attempt across concurrent callers", async () => {
        __ignoredNotificationTest.setMidTurnDetector(() => false);
        const fake = client({ title: "Investigating the flaky cache", sessions: ["ses_a"] });
        const delivery = createConfigWarningDelivery(fake.client, WARNING);

        await Promise.all([delivery.deliverToFirstSession(), delivery.deliverTo("ses_b")]);
        expect(fake.prompt).toHaveBeenCalledTimes(1);
        expect(delivery.pending).toBe(false);
    });

    it("stops retrying after the attempt budget is spent", async () => {
        __ignoredNotificationTest.setMidTurnDetector(() => false);
        const fake = client({ title: "Investigating the flaky cache", promptRejects: true });
        const delivery = createConfigWarningDelivery(fake.client, WARNING);

        for (let attempt = 0; attempt < 10; attempt += 1) {
            await delivery.deliverTo("ses_flaky");
        }
        expect(fake.prompt).toHaveBeenCalledTimes(10);
        expect(delivery.pending).toBe(false);

        await delivery.deliverTo("ses_flaky");
        expect(fake.prompt).toHaveBeenCalledTimes(10);
    });
});
