import { describe, expect, it } from "bun:test";
import { estimateTokens as estimateTokensFromNative } from "@eidnara/shm-native";
import { estimateTokens as estimateTokensFromHook } from "../hooks/context/read-session-formatting";
import { estimateTokens } from "./token-estimator";

describe("token estimator", () => {
    it("is the native binding across every public re-export", () => {
        expect(estimateTokens).toBe(estimateTokensFromNative);
        expect(estimateTokensFromHook).toBe(estimateTokensFromNative);
    });
});
