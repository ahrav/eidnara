import { beforeEach, describe, expect, it } from "bun:test";
import {
    clearHookInitFailure,
    FAIL_CLOSED_DOCTOR_COMMAND,
    getLastHookInitFailure,
    recordHookInitFailure,
} from "./fail-closed-block";

describe("hook init failure record", () => {
    beforeEach(() => {
        clearHookInitFailure();
    });

    it("names the CLI doctor command", () => {
        expect(FAIL_CLOSED_DOCTOR_COMMAND).toBe("eidnara doctor");
    });

    it("keeps the last recorded failure until it is cleared", () => {
        expect(getLastHookInitFailure()).toBeNull();
        recordHookInitFailure({ type: "no_project" });
        expect(getLastHookInitFailure()).toEqual({ type: "no_project" });
        clearHookInitFailure();
        expect(getLastHookInitFailure()).toBeNull();
    });
});
