import { describe, expect, test } from "bun:test";
import hostRelease from "../../../../release/host-release.json";
import { MANAGED_SUBTREE, STORAGE_SUBDIRECTORY } from "./host-release-layout";

describe("host release layout mirror", () => {
    test("MANAGED_SUBTREE equals layout.managed_subtree", () => {
        expect(MANAGED_SUBTREE).toBe(hostRelease.layout.managed_subtree);
    });

    test("STORAGE_SUBDIRECTORY equals layout.storage_subdirectory", () => {
        expect(STORAGE_SUBDIRECTORY).toBe(hostRelease.layout.storage_subdirectory);
    });
});
