import { describe, expect, it } from "bun:test";
import { sanitizeDiagnosticEndpoint } from "./redaction";

describe("sanitizeDiagnosticEndpoint", () => {
    it("strips query strings and userinfo from valid URLs", () => {
        const sanitized = sanitizeDiagnosticEndpoint(
            "https://user:pass@example.com/v1/embeddings?api_key=secret#frag",
        );
        expect(sanitized).toBe("https://example.com/v1/embeddings");
    });

    it("strips query strings and userinfo from invalid-scheme display values", () => {
        const sanitized = sanitizeDiagnosticEndpoint("ftp://user:pass@example.com/v1?token=secret");
        expect(sanitized).toBe("ftp://example.com/v1");
    });

    it("strips userinfo that URL parsing would read as an opaque scheme", () => {
        expect(sanitizeDiagnosticEndpoint("alice:hunter2@example.com/v1")).toBe("example.com/v1");
        expect(sanitizeDiagnosticEndpoint("alice:hunter2@example.com/v1?k=secret#frag")).toBe(
            "example.com/v1",
        );
        expect(sanitizeDiagnosticEndpoint("user:pass@example.com")).toBe("example.com");
    });

    it("strips userinfo and query from scheme-relative endpoints", () => {
        expect(sanitizeDiagnosticEndpoint("//alice:hunter2@example.com/v1?token=x")).toBe(
            "//example.com/v1",
        );
        expect(sanitizeDiagnosticEndpoint("//example.com/v1?token=x")).toBe("//example.com/v1");
    });

    it("keeps an @ that is part of the path", () => {
        expect(sanitizeDiagnosticEndpoint("https://example.com/v1/@scope/pkg")).toBe(
            "https://example.com/v1/@scope/pkg",
        );
    });
});
