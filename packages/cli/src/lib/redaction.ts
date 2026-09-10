import { sanitizeDiagnosticText } from "@eidnara/opencode/shared/redaction";

/**
 * Userinfo is stripped before URL parsing because the parser cannot expose it in two cases.
 * `new URL("alice:hunter2@example.com/v1")` reads `alice` as an opaque scheme with empty
 * `username` and `password`. `new URL("//alice:hunter2@example.com/v1")` throws without a base.
 *
 * The strip removes text through the last `@` before a path, query, or fragment, so passwords containing whitespace or raw `@` cannot reach the parse-failure fallback.
 */
export function sanitizeDiagnosticEndpoint(value: string): string {
    const trimmed = value.trim();
    if (!trimmed) return sanitizeDiagnosticText(value);
    const withoutUserInfo = trimmed.replace(/^((?:[a-z][a-z0-9+.-]*:)?\/\/+|\/*)[^/?#]*@/i, "$1");
    try {
        const url = new URL(withoutUserInfo);
        url.username = "";
        url.password = "";
        url.search = "";
        url.hash = "";
        return sanitizeDiagnosticText(url.toString());
    } catch {
        return sanitizeDiagnosticText(withoutUserInfo.replace(/[?#].*$/, ""));
    }
}
