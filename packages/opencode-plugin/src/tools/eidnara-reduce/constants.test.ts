import { describe, expect, it } from "bun:test";
import { EIDNARA_REDUCE_DESCRIPTION } from "./constants";

describe("eidnara-reduce constants", () => {
    //#given
    describe("EIDNARA_REDUCE_DESCRIPTION", () => {
        //#then
        it("frames reduction as deferred discard, not immediate delete", () => {
            expect(EIDNARA_REDUCE_DESCRIPTION).toContain("discardable");
            expect(EIDNARA_REDUCE_DESCRIPTION).toContain("NOT an immediate delete");
            expect(EIDNARA_REDUCE_DESCRIPTION).toContain("DONE with");
            expect(EIDNARA_REDUCE_DESCRIPTION).not.toContain("gone forever");
            expect(EIDNARA_REDUCE_DESCRIPTION).not.toContain("Remove entirely");
        });
    });
});
