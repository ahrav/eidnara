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
});
