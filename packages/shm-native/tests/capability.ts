import assert from "node:assert/strict";
import {
        activeNativeChannels,
        DESCRIPTOR_SCHEMA_VERSION,
        NativeChannel,
        nativeArtifactIdentity,
        probeCapabilities,
        QUALIFIED_TEST_PROFILE,
        supportsNativePlatform,
} from "../index.ts";

assert.equal(supportsNativePlatform("linux", "x64"), true);
assert.equal(supportsNativePlatform("darwin", "x64"), false);
assert.equal(supportsNativePlatform("darwin", "arm64"), false);
assert.equal(activeNativeChannels(), 0);
const capability = probeCapabilities();
const claimedTarget = process.env.EIDNARA_SHM_NATIVE_CLAIMED_TARGET === "1";
// The record names the artifact and runtime that produced it, so a pass or a recorded
// limitation is attributable to one build at one layout on one runtime.
const artifact = nativeArtifactIdentity();
const runtime = {
        name: process.release.name,
        version: process.version,
        bun: (globalThis as { Bun?: { version: string } }).Bun?.version ?? null,
};
if (claimedTarget) {
        assert.ok(artifact, "claimed native target loaded no addon");
        assert.equal(artifact.buildProfile, "release");
        assert.equal(artifact.buildTarget, "linux-x86_64");
        assert.equal(artifact.descriptorSchemaVersion, DESCRIPTOR_SCHEMA_VERSION);
        assert.equal(artifact.qualifiedTestProfile, QUALIFIED_TEST_PROFILE);
}
// A claimed target must at least load its addon on every runtime. Full availability is a
// separate question: Bun 1.3.14 has no `markAsUntransferable`, so the probe stops there.
if (claimedTarget) {
        assert.notEqual(
                capability.reason,
                "addon_unavailable",
                `claimed native target failed to load its addon: ${capability.reason}`,
        );
}
if (typeof (globalThis as { Bun?: unknown }).Bun === "undefined") {
        assert.equal(capability.available, false);
        if (claimedTarget) {
                assert.equal(capability.reason, "node_detachment_unavailable");
        } else {
                assert.ok(
                        capability.reason === "node_detachment_unavailable" ||
                                capability.reason === "addon_unavailable",
                        `unexpected Node reason: ${capability.reason}`,
                );
        }
}
assert.equal(
        activeNativeChannels(),
        0,
        "capability probe created a shared candidate",
);

if (capability.available) {
        const pair = NativeChannel.createTestPair();
        assert.equal(activeNativeChannels(), 2);
        pair.first.close();
        pair.second.close();
        assert.equal(activeNativeChannels(), 0);
        console.log(
                JSON.stringify({
                        capabilityOutcome: "ACTIVATED",
                        runtime,
                        artifact,
                }),
        );
} else {
        assert.throws(
                () => NativeChannel.createTestPair(),
                /shared-memory native addon|shared-memory native startup failed/,
        );
        assert.equal(activeNativeChannels(), 0);
        console.log(
                JSON.stringify({
                        capabilityOutcome: "TERMINAL_STARTUP_FAILURE",
                        runtime,
                        artifact,
                        reason: capability.reason,
                }),
        );
}
