import { describe, expect, test } from "bun:test";
import hostRelease from "../../../../release/host-release.json";
import { MANAGED_SUBTREE, STORAGE_SUBDIRECTORY } from "./host-release-layout";

describe("host release layout mirror", () => {
    test("mirrors the managed subtree and storage subdirectory from host-release.json", () => {
        expect(MANAGED_SUBTREE).toBe(hostRelease.layout.managed_subtree);
        expect(STORAGE_SUBDIRECTORY).toBe(hostRelease.layout.storage_subdirectory);
    });
});
