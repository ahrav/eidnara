import { describe, expect, it } from "bun:test";
import { estimateTokens as estimateTokensFromNative } from "@eidnara/shm-native";
import { estimateTokens as estimateTokensFromHook } from "../hooks/context/read-session-formatting";
import { estimateTokens } from "./token-estimator";

describe("token estimator", () => {
    it("is the native binding across every public re-export", () => {
        for (const sample of [
            "",
            "plain words",
            "<EOT> literal special tokens",
            "multibyte — émoji 🎉 text",
            "x".repeat(5000),
        ]) {
            const expected = estimateTokensFromNative(sample);
            expect(estimateTokens(sample)).toBe(expected);
            expect(estimateTokensFromHook(sample)).toBe(expected);
        }
    });
});
