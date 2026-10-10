import { afterEach, describe, expect, it, mock } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { __ignoredNotificationTest } from "../hooks/context/send-session-notification";
import {
    __resetNotificationStateForTests,
    type RpcNotification,
    registerNotificationSink,
} from "../shared/rpc-notifications";
import { createStartNoticeDelivery } from "./start-notice-delivery";

const NOTICE =
    "Eidnara is off in this process: its daemon did not start (harness_unavailable). Run `eidnara doctor` for details.";

/** Desktop start notices use session messages. */
function desktopClient(sessions: string[]) {
    const prompt = mock(async () => ({}));
    const get = mock(async () => ({ title: "Investigating the flaky cache" }));
    const messages = mock(async () => [
        {
            info: {
                role: "assistant",
                agent: "builder",
                model: { providerID: "anthropic", modelID: "claude-fable" },
            },
        },
    ]);
    const list = mock(async () => sessions.map((id) => ({ id })));
    return { client: { session: { prompt, get, messages, list } }, prompt };
}

function promptedText(call: unknown): { sessionId: string; text: string; ignored: boolean } {
    const input = call as {
        path: { id: string };
        body: { parts: Array<{ text: string; ignored: boolean }> };
    };
    return {
        sessionId: input.path.id,
        text: input.body.parts[0]?.text ?? "",
        ignored: input.body.parts[0]?.ignored ?? false,
    };
}

describe("createStartNoticeDelivery", () => {
    afterEach(() => {
        __ignoredNotificationTest.reset();
        __resetNotificationStateForTests();
    });

    it("persists a notice as an ignored message in the first listed Desktop session", async () => {
        __ignoredNotificationTest.setMidTurnDetector(() => false);
        const fake = desktopClient(["ses_a"]);
        const delivery = createStartNoticeDelivery(fake.client);
        await delivery.announce(NOTICE);
        expect(fake.prompt).toHaveBeenCalledTimes(1);
        expect(promptedText(fake.prompt.mock.calls[0]?.[0])).toEqual({
            sessionId: "ses_a",
            text: `## ⚠️ Eidnara is off\n\n${NOTICE}`,
            ignored: true,
        });
        await delivery.deliverTo("ses_a");
        expect(fake.prompt).toHaveBeenCalledTimes(1);
    });

    it("shows a warning toast titled for the refusal on a connected Eidnara TUI", async () => {
        __ignoredNotificationTest.setMidTurnDetector(() => false);
        const sent: RpcNotification[] = [];
        const unregister = registerNotificationSink({
            sessionId: "ses_tui",
            protocol: 2,
            send: (notification) => sent.push(notification),
        });
        try {
            const fake = desktopClient(["ses_tui"]);
            await createStartNoticeDelivery(fake.client).announce(NOTICE);
            expect(fake.prompt).not.toHaveBeenCalled();
            expect(sent).toHaveLength(1);
            const payload = sent[0]?.payload as { title: string; variant: string; message: string };
            expect(payload.title).toBe("⚠️ Eidnara is off");
            expect(payload.variant).toBe("warning");
            expect(payload.message).toContain("did not start (harness_unavailable)");
        } finally {
            unregister();
        }
    });

    it("holds a notice until a session prompts and delivers each held notice once", async () => {
        __ignoredNotificationTest.setMidTurnDetector(() => false);
        const fake = desktopClient([]);
        const delivery = createStartNoticeDelivery(fake.client);
        await delivery.announce(NOTICE);
        await delivery.announce(`${NOTICE} (second)`);
        expect(fake.prompt).not.toHaveBeenCalled();
        await delivery.deliverTo("ses_first_prompt");
        expect(fake.prompt).toHaveBeenCalledTimes(2);
        expect(
            fake.prompt.mock.calls.map((call) => promptedText(call[0]).text.split("\n\n")[1]),
        ).toEqual([NOTICE, `${NOTICE} (second)`]);
        await delivery.deliverTo("ses_second_prompt");
        expect(fake.prompt).toHaveBeenCalledTimes(2);
    });
});

describe("plugin start notice wiring", () => {
    it("routes start refusals through the session notification path, as config warnings are", () => {
        const source = readFileSync(join(import.meta.dir, "..", "index.ts"), "utf8");
        expect(source).toContain("createStartNoticeDelivery(ctx.client)");
        expect(source).not.toContain("tui.showToast");
    });
});
