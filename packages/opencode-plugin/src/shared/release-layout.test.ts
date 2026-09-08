import { describe, expect, test } from "bun:test";
import hostRelease from "../../../../release/host-release.json";
import { RELEASE_LAYOUT } from "./release-layout";

describe("release layout mirror", () => {
    test("equals the layout row of release/host-release.json", () => {
        expect({ ...RELEASE_LAYOUT }).toEqual(hostRelease.layout);
    });
});
