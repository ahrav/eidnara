import { describe, expect, it } from "bun:test";

import { HostCallError } from "@eidnara/opencode/shared/host-client/errors";

import { classifyPass } from "./pass-outcome";

describe("scale report pass outcome", () => {
    it("completes an accepted pass", () => {
        expect(classifyPass({ ok: true })).toEqual({
            outcome: "completed",
            refusal: null,
        });
    });

    it("censors a deadline after a possible send", () => {
        const deadline = Object.assign(new Error("module transport deadline expired"), {
            code: "ETIMEDOUT",
        });
        expect(classifyPass({ ok: false, error: deadline })).toEqual({
            outcome: "censored",
            refusal: null,
        });
    });

    it("names a daemon application error a daemon error", () => {
        const error = new HostCallError(
            "terminal",
            "the daemon speaks another transform revision",
            "transform_revision_unsupported",
        );
        expect(classifyPass({ ok: false, error })).toEqual({
            outcome: "refused",
            refusal: "daemon_error",
        });
    });

    it("names a failure before the daemon answered a transport error", () => {
        for (const error of [
            new HostCallError("outcome_unknown", "write failed", "write_failed"),
            new HostCallError("not_sent", "shared-memory channel closed"),
            new Error("socket closed"),
        ]) {
            expect(classifyPass({ ok: false, error })).toEqual({
                outcome: "refused",
                refusal: "transport_error",
            });
        }
    });

    it("names a pass the plugin declined without a thrown error declined", () => {
        expect(classifyPass({ ok: false })).toEqual({ outcome: "refused", refusal: "declined" });
    });
});
