export interface SerializedTransformCase {
    name: string;
    body: Record<string, unknown>;
    pagerRefuses?: boolean;
    hostRefuses?: boolean;
    firstPageBytes?: number;
    lastPageBytes?: number;
    pageCount?: number;
}

const CAP = 524_288;
export const SERIALIZED_TRANSFORM_SESSION = "serialized-transform-corpus";
export const SERIALIZED_TRANSFORM_TEXT = 'café 😀\u2028\u0001\n"\\ literal \\ud800';

function message(ordinal: number, text: string, padding = ""): Record<string, unknown> {
    return {
        mid: `m${ordinal}`,
        ordinal,
        ck: {
            role: "user",
            content: [{ kind: { type: "text", text } }],
            meta: { harness_id: `m${ordinal}` },
        },
        padding,
    };
}

function body(messages: Record<string, unknown>[]): Record<string, unknown> {
    return {
        method: "transform",
        kind: "transform",
        v: 2,
        session_id: SERIALIZED_TRANSFORM_SESSION,
        serializer_profile: "owned-llmrunner",
        render_config: "serialized-transform-corpus",
        messages,
    };
}

function scalarBoundary(bytes: number, number: unknown = 1): Record<string, unknown> {
    const value = { ...body([message(1, SERIALIZED_TRANSFORM_TEXT)]), number, padding: "" };
    value.padding = "x".repeat(bytes - Buffer.byteLength(JSON.stringify(value)));
    return value;
}

function pageEnvelope(messages: Record<string, unknown>[], index: number): Record<string, unknown> {
    return {
        method: "transform",
        session_id: SERIALIZED_TRANSFORM_SESSION,
        transform_page_id: "0".repeat(36),
        transform_generation: 0,
        transform_page_index: index,
        transform_page_total: 2,
        transform_page_complete: index === 1,
        transform_page_digest: "0".repeat(64),
        messages,
    };
}

function pageBoundary(text: string, number = 1): Record<string, unknown> {
    const first = { ...message(1, text), number, padding: "" };
    const envelope = pageEnvelope([first], 0);
    first.padding = "x".repeat(CAP - Buffer.byteLength(JSON.stringify(envelope)));
    return body([first, message(2, "second message", "y".repeat(120_000))]);
}

function finalPageBoundary(number: number): Record<string, unknown> {
    const first = message(1, SERIALIZED_TRANSFORM_TEXT, "x".repeat(300_000));
    const second = message(2, "second message", "y".repeat(300_000));
    const { messages: _messages, ...scalars } = body([first, second]);
    const final = { ...pageEnvelope([second], 1), ...scalars, number, padding: "" };
    const padding = "z".repeat(CAP - Buffer.byteLength(JSON.stringify(final)));
    return { ...body([first, second]), number, padding };
}

export function serializedTransformCorpus(): SerializedTransformCase[] {
    const rawJson = (JSON as JSON & { rawJSON: (text: string) => unknown }).rawJSON;
    const numeric = pageBoundary(SERIALIZED_TRANSFORM_TEXT);
    numeric.numbers = [
        1.5,
        1e-7,
        0.000001,
        0.00001,
        1.0000000000000002,
        5e-324,
        1e21,
        1e23,
        2 ** 60,
        2 ** 63,
        -(2 ** 63),
        2 ** 64,
        -(2 ** 63) - 2048,
        1.7976931348623157e308,
        -2.2250738585072014e-308,
    ];
    const separate = pageBoundary(SERIALIZED_TRANSFORM_TEXT, 2 ** 64);
    (separate.messages as Record<string, unknown>[])[1] = message(2, "\ud800", "y".repeat(120_000));
    return [
        { name: "small-unicode", body: body([message(1, SERIALIZED_TRANSFORM_TEXT)]) },
        { name: "scalar-below", body: scalarBoundary(CAP - 1), firstPageBytes: CAP - 1 },
        { name: "scalar-exact", body: scalarBoundary(CAP), firstPageBytes: CAP },
        { name: "scalar-over", body: scalarBoundary(CAP + 1), pagerRefuses: true },
        {
            name: "scalar-number-over",
            body: scalarBoundary(CAP, 2 ** 64),
            firstPageBytes: CAP,
            pageCount: 1,
        },
        {
            name: "raw-exponent-over",
            body: scalarBoundary(CAP, rawJson("1e5")),
            firstPageBytes: CAP,
            pageCount: 1,
        },
        {
            name: "raw-negative-zero-over",
            body: scalarBoundary(CAP, rawJson("-0")),
            firstPageBytes: CAP,
            pageCount: 1,
        },
        {
            name: "page-exact",
            body: pageBoundary(SERIALIZED_TRANSFORM_TEXT),
            firstPageBytes: CAP,
            pageCount: 2,
        },
        { name: "page-number-growth", body: pageBoundary(SERIALIZED_TRANSFORM_TEXT, 2 ** 64) },
        { name: "numeric-scalar-tail", body: numeric, firstPageBytes: CAP },
        { name: "final-page-exact", body: finalPageBoundary(1), lastPageBytes: CAP, pageCount: 2 },
        { name: "final-page-number-growth", body: finalPageBoundary(2 ** 64), pageCount: 3 },
        {
            name: "continuation",
            body: body([message(1, SERIALIZED_TRANSFORM_TEXT, "x".repeat(CAP * 2))]),
        },
        { name: "lone-high-small", body: body([message(1, "\ud800")]), hostRefuses: true },
        { name: "lone-low-small", body: body([message(1, "\udc00")]), hostRefuses: true },
        { name: "lone-after-number-page", body: separate, hostRefuses: true, pageCount: 2 },
        {
            name: "lone-with-number-growth",
            body: pageBoundary("\ud800", 2 ** 64),
            hostRefuses: true,
            firstPageBytes: CAP,
        },
        {
            name: "lone-high-page",
            body: pageBoundary("\ud800"),
            hostRefuses: true,
            firstPageBytes: CAP,
        },
        {
            name: "lone-low-page",
            body: pageBoundary("\udc00"),
            hostRefuses: true,
            firstPageBytes: CAP,
        },
    ];
}
