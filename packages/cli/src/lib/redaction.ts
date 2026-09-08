import { sanitizeDiagnosticText } from "@eidnara/opencode/shared/redaction";

/**
 * Userinfo is stripped before URL parsing. `new URL("alice:hunter2@example.com/v1")` reads
 * `alice` as an opaque scheme, so its `username` and `password` are empty and clearing them
 * would leave the credentials in place.
 */
export function sanitizeDiagnosticEndpoint(value: string): string {
    const trimmed = value.trim();
    if (!trimmed) return sanitizeDiagnosticText(value);
    const withoutUserInfo = trimmed.replace(/^([a-z][a-z0-9+.-]*:\/\/)?[^/?#@\s]*@/i, "$1");
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
