/**
 * Callers compare `expectedSignature` with `authorization.signature` to verify a request
 * against a known secret.
 */

import { createHash, createHmac } from "node:crypto";

export interface SignedRequest {
    method: string;
    /** The request path exactly as it arrived, before any decoding. */
    path: string;
    /** Header values by lowercase name; `host` carries `:authority` for an HTTP/2 request. */
    headers: Record<string, string>;
    body: Uint8Array;
}

export interface Authorization {
    accessKeyId: string;
    date: string;
    region: string;
    service: string;
    signedHeaders: string[];
    signature: string;
}

const AUTHORIZATION =
    /^AWS4-HMAC-SHA256 Credential=([^/]+)\/(\d{8})\/([a-z0-9-]+)\/([a-z0-9-]+)\/aws4_request, ?SignedHeaders=([a-z0-9;:-]+), ?Signature=([0-9a-f]{64})$/;

export function parseAuthorization(value: string | undefined): Authorization | undefined {
    const match = value === undefined ? null : AUTHORIZATION.exec(value);
    if (!match) return undefined;
    const [, accessKeyId, date, region, service, signed, signature] = match as unknown as string[];
    return {
        accessKeyId: accessKeyId as string,
        date: date as string,
        region: region as string,
        service: service as string,
        signedHeaders: (signed as string).split(";"),
        signature: signature as string,
    };
}

/** RFC 3986 encoding of every byte outside the unreserved set, as SigV4 encodes a path segment. */
function uriEncode(segment: string): string {
    return [...Buffer.from(segment, "utf8")]
        .map((byte) => {
            const char = String.fromCharCode(byte);
            return /[A-Za-z0-9\-_.~]/.test(char)
                ? char
                : `%${byte.toString(16).toUpperCase().padStart(2, "0")}`;
        })
        .join("");
}

const sha256Hex = (data: Uint8Array | string): string =>
    createHash("sha256").update(data).digest("hex");
const hmac = (key: Uint8Array | string, data: string): Buffer =>
    createHmac("sha256", key).update(data, "utf8").digest();

/** `canonicalRequest` re-encodes the received path to match non-S3 SigV4 canonicalization. */
export function canonicalRequest(request: SignedRequest, signedHeaders: string[]): string {
    const [path, query = ""] = request.path.split("?", 2) as [string, string?];
    const canonicalUri = path.split("/").map(uriEncode).join("/");
    const canonicalQuery = query
        .split("&")
        .filter((pair) => pair.length > 0)
        .sort()
        .join("&");
    const canonicalHeaders = signedHeaders
        .map((name) => `${name}:${(request.headers[name] ?? "").trim().replace(/\s+/g, " ")}\n`)
        .join("");
    const payloadHash = request.headers["x-amz-content-sha256"] ?? sha256Hex(request.body);
    return [
        request.method,
        canonicalUri,
        canonicalQuery,
        canonicalHeaders,
        signedHeaders.join(";"),
        payloadHash,
    ].join("\n");
}

export function expectedSignature(
    request: SignedRequest,
    authorization: Authorization,
    secretAccessKey: string,
): string {
    const amzDate = request.headers["x-amz-date"] ?? "";
    const scope = `${authorization.date}/${authorization.region}/${authorization.service}/aws4_request`;
    const toSign = [
        "AWS4-HMAC-SHA256",
        amzDate,
        scope,
        sha256Hex(canonicalRequest(request, authorization.signedHeaders)),
    ].join("\n");
    let key: Buffer = hmac(`AWS4${secretAccessKey}`, authorization.date);
    for (const part of [authorization.region, authorization.service, "aws4_request"]) {
        key = hmac(key, part);
    }
    return hmac(key, toSign).toString("hex");
}

export interface BedrockCredentials {
    accessKeyId: string;
    secretAccessKey: string;
    sessionToken?: string;
    region: string;
}

/** The scope names `credentials`, the signature covers the host, the body digest, and any session token, and it matches under the secret. */
export function verifySigned(
    request: SignedRequest,
    authorization: Authorization,
    credentials: BedrockCredentials,
): boolean {
    const payloadHash = request.headers["x-amz-content-sha256"];
    const signed = new Set(authorization.signedHeaders);
    return (
        authorization.accessKeyId === credentials.accessKeyId &&
        authorization.region === credentials.region &&
        authorization.service === "bedrock" &&
        (signed.has("host") || signed.has(":authority")) &&
        (payloadHash === undefined || payloadHash === sha256Hex(request.body)) &&
        (credentials.sessionToken === undefined ||
            (signed.has("x-amz-security-token") &&
                request.headers["x-amz-security-token"] === credentials.sessionToken)) &&
        expectedSignature(request, authorization, credentials.secretAccessKey) ===
            authorization.signature
    );
}
