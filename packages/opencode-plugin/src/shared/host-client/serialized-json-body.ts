import { isRecord } from "../record-type-guard";

const SERIALIZED_JSON = Symbol("serialized JSON body");

/** Serialized text is authoritative; this type exposes a shallow-readonly inspection view.
 * Root fields are frozen; nested edits do not rewrite the stored text. */
export type SerializedJsonBody = Readonly<Record<string, unknown>> & {
    readonly [SERIALIZED_JSON]: string;
};

/** Nested values are shared with `value`; only root fields are copied, so nothing is parsed. */
export function serializeJsonBody(value: Record<string, unknown>): SerializedJsonBody {
    const text = JSON.stringify(value);
    if (text === undefined) throw new TypeError("request body is not JSON serializable");
    const body: Record<string, unknown> = { ...value };
    Object.defineProperty(body, SERIALIZED_JSON, { value: text });
    return Object.freeze(body) as SerializedJsonBody;
}

export function serializedJsonText(body: SerializedJsonBody): string;
export function serializedJsonText(body: unknown): string | undefined;
export function serializedJsonText(body: unknown): string | undefined {
    return isRecord(body) && Object.hasOwn(body, SERIALIZED_JSON)
        ? (body as SerializedJsonBody)[SERIALIZED_JSON]
        : undefined;
}
