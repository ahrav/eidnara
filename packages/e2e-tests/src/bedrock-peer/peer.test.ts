import { describe, expect, it } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { CAPTURED_FACT, callerOf, captureAnswer, classifyAnswer, scriptedSummary } from "./answers";
import { decodeMessages, encodeEvent } from "./eventstream";
import { completedWrapupRounds, isWrapupResult } from "./scenario";
import { BedrockPeer } from "./server";
import { canonicalRequest, expectedSignature, parseAuthorization, verifySigned } from "./sigv4";

const VECTOR = JSON.parse(
    readFileSync(
        join(import.meta.dir, "../../../../crates/daemon/tests/fixtures/sigv4/bedrock-colon.json"),
        "utf8",
    ),
) as Record<string, unknown> & {
    headers: Array<[string, string]>;
    body: string;
    authorization: string;
};

describe("bedrock peer", () => {
    it("verifies the botocore Bedrock vector, the canonical request included", () => {
        const body = Buffer.from(VECTOR.body);
        const authorization = parseAuthorization(VECTOR.authorization);
        expect(authorization?.accessKeyId).toBe("AKIDEXAMPLE");
        const request = {
            method: "POST",
            path: VECTOR.wire_path as string,
            headers: {
                ...Object.fromEntries(VECTOR.headers),
                host: VECTOR.host as string,
                "x-amz-content-sha256": new Bun.CryptoHasher("sha256").update(body).digest("hex"),
                "x-amz-date": "20150830T123600Z",
                "x-amz-security-token": VECTOR.session_token as string,
            },
            body,
        };
        if (!authorization) throw new Error("vector authorization did not parse");
        expect(canonicalRequest(request, authorization.signedHeaders)).toBe(
            VECTOR.canonical_request as string,
        );
        expect(expectedSignature(request, authorization, VECTOR.secret_access_key as string)).toBe(
            VECTOR.signature as string,
        );
        const credentials = {
            accessKeyId: "AKIDEXAMPLE",
            secretAccessKey: VECTOR.secret_access_key as string,
            sessionToken: VECTOR.session_token as string,
            region: "us-east-1",
        };
        expect(verifySigned(request, authorization, credentials)).toBe(true);
        expect(
            verifySigned(
                { ...request, body: Buffer.from(`${VECTOR.body} `) },
                authorization,
                credentials,
            ),
        ).toBe(false);
        expect(
            verifySigned(request, authorization, { ...credentials, sessionToken: "another" }),
        ).toBe(false);
        const unsignedToken = {
            ...authorization,
            signedHeaders: authorization.signedHeaders.filter(
                (name) => name !== "x-amz-security-token",
            ),
        };
        expect(verifySigned(request, unsignedToken, credentials)).toBe(false);
        expect(expectedSignature(request, authorization, "another secret")).not.toBe(
            VECTOR.signature as string,
        );
    });

    it("frames events with both checksums and string headers", () => {
        const frames = Buffer.concat([
            encodeEvent("messageStart", { role: "assistant" }),
            encodeEvent("contentBlockDelta", { contentBlockIndex: 0, delta: { text: "é ok" } }),
        ]);
        const decoded = decodeMessages(frames);
        expect(decoded.map((message) => message.headers[":event-type"])).toEqual([
            "messageStart",
            "contentBlockDelta",
        ]);
        expect(JSON.parse(decoded[1]?.payload.toString("utf8") ?? "")).toEqual({
            contentBlockIndex: 0,
            delta: { text: "é ok" },
        });
        const corrupt = Buffer.from(frames);
        corrupt[20] = (corrupt[20] as number) ^ 0xff;
        expect(() => decodeMessages(corrupt)).toThrow("checksum");
    });

    it("answers each caller in the shape its validator reads", () => {
        expect(
            callerOf({
                system: "You are a memory classifier for",
                messages: "<pool>",
                lastUser: "<pool>",
            }),
        ).toBe("memory_classifier");
        expect(
            classifyAnswer(
                '<pool><memory id="m1" kind="fact">\n<body>\nx\n</body>\n</memory>\n<memory id="m2" kind="fact">',
            ),
        ).toBe(
            '<classify><memory id="m1" importance="60" scope="project" shareable="false"/><memory id="m2" importance="60" scope="project" shareable="false"/></classify>',
        );
        expect(
            callerOf({ system: "Extract durable project memory", messages: "hi", lastUser: "hi" }),
        ).toBe("conversation");
        const capture = JSON.parse(
            captureAnswer(
                JSON.stringify({
                    messages: [
                        { id: "u1", role: "user", text: `note: ${CAPTURED_FACT}` },
                        { id: "a1", role: "assistant", text: CAPTURED_FACT },
                    ],
                    existing_memories: [],
                }),
            ),
        );
        expect(capture.decisions).toEqual([
            {
                message_id: "u1",
                memories: [
                    { category: "CONFIG_VALUES", content: CAPTURED_FACT, quote: CAPTURED_FACT },
                ],
            },
            { message_id: "a1", memories: [] },
        ]);
        expect(
            scriptedSummary("<new_messages>\n[1] U: hello «a1»world\n[2] A: hi\n</new_messages>"),
        ).toBe(
            '<output><history_segments><history_segment start="1" end="2" title="messages 1 to 2" episode_type="feature" importance="50"><p1>hello world; hi</p1><p2>hello world; hi</p2><p3>messages 1 to 2</p3><p4 /></history_segment></history_segments><meta><unprocessed_from>3</unprocessed_from></meta></output>',
        );
    });

    it("counts only a completed wrapup's rounds", () => {
        expect(completedWrapupRounds("## Eidnara Wrapup\n\ncompacted 4 messages (2 rounds)")).toBe(
            2,
        );
        expect(completedWrapupRounds("## Eidnara Wrapup  compacted 1 message (1 round)")).toBe(1);
        expect(
            completedWrapupRounds("## Eidnara Wrapup — Partial\n\nstopped (1 round)"),
        ).toBeUndefined();
        expect(completedWrapupRounds("## Eidnara Wrapup\n\nStarting wrapup…")).toBeUndefined();
        expect(completedWrapupRounds("## Eidnara Wrapup\n\nNothing to compact.")).toBeUndefined();
        expect(isWrapupResult("## Eidnara Wrapup  Starting wrapup…")).toBe(false);
        expect(isWrapupResult("## Eidnara Wrapup — Failed\n\nx")).toBe(true);
    });

    it("refuses an unsigned request and records it", async () => {
        const peer = new BedrockPeer({
            accessKeyId: "AKID",
            secretAccessKey: "secret",
            region: "us-east-1",
        });
        await peer.start();
        try {
            const response = await fetch(`${peer.http1Url}/model/m%3A0/converse-stream`, {
                method: "POST",
                headers: { "content-type": "application/json", "x-api-key": "k" },
                body: "{}",
            });
            expect(response.status).toBe(403);
            expect(peer.requests[0]?.signatureValid).toBe(false);
            expect(peer.requests[0]?.modelId).toBe("m:0");
            expect(peer.requests[0]?.headerNames).toContain("x-api-key");
            expect(peer.callers()).toEqual([]);
        } finally {
            await peer.stop();
        }
    });
});
