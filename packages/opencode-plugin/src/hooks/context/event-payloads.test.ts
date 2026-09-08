import { describe, expect, it } from "bun:test";

import { getSessionCreatedInfo } from "./event-payloads";

describe("getSessionCreatedInfo", () => {
    it("#given a root session without parentID #when resolved #then returns the info with parentID undefined", () => {
        const info = getSessionCreatedInfo({
            info: { id: "ses_root", projectID: "p", directory: "/w", title: "Root" },
        });

        expect(info).toEqual({
            id: "ses_root",
            parentID: undefined,
            providerID: undefined,
            modelID: undefined,
            title: "Root",
        });
    });

    it("#given a child session with parentID #when resolved #then carries parentID and model fields", () => {
        const info = getSessionCreatedInfo({
            info: {
                id: "ses_child",
                parentID: "ses_root",
                providerID: "anthropic",
                modelID: "claude",
                title: "eidnara-compiler",
            },
        });

        expect(info).toEqual({
            id: "ses_child",
            parentID: "ses_root",
            providerID: "anthropic",
            modelID: "claude",
            title: "eidnara-compiler",
        });
    });

    it("#given info without a string id #when resolved #then returns null", () => {
        expect(getSessionCreatedInfo({ info: { parentID: "ses_root" } })).toBeNull();
        expect(getSessionCreatedInfo({ info: { id: 42 } })).toBeNull();
    });

    it("#given properties without an info record #when resolved #then returns null", () => {
        expect(getSessionCreatedInfo(undefined)).toBeNull();
        expect(getSessionCreatedInfo({})).toBeNull();
        expect(getSessionCreatedInfo({ info: "ses_root" })).toBeNull();
    });
});
