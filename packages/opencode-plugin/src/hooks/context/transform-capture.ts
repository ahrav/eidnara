import { createHash } from "node:crypto";
import { types } from "node:util";

const TRANSFORM_CAPTURE_MAX_PASSES = 64;
const TRANSFORM_CAPTURE_MAX_BYTES = 64 * 1024 * 1024;
const MAX_REFERENCEABLE_DEPTH = 256;
/** Root arrays, the root tape, and the capture record itself. */
const ROOT_CAPTURE_BYTES = 256;
/** One retained tape slot; a string field also charges its UTF-16 units in case the host drops it. */
const TAPE_SLOT_BYTES = 8;
/** Members and snapshots arrays hold one slot each per root message. */
const ROOT_MEMBER_BYTES = 16;
/** Widest JSON scalar token: an f64 rendered with full precision and exponent. */
const SCALAR_WIRE_BYTES = 24;

type SnapshotField = string | number | boolean | symbol | null;
const ARRAY = Symbol("array");
const END_ARRAY = Symbol("end_array");
const OBJECT = Symbol("object");
const END_OBJECT = Symbol("end_object");
const EXTRA_KEY = Symbol("extra_key");
const ARRAY_METHODS = new Set(Object.getOwnPropertyNames(Array.prototype));
ARRAY_METHODS.delete("length");
ARRAY_METHODS.add("then");
const ARRAY_SYMBOLS = Object.getOwnPropertySymbols(Array.prototype).concat(
    Symbol.isConcatSpreadable,
);

export interface MessageContentSnapshot {
    fields: SnapshotField[];
    /** The walk's spend on a captured member. */
    bytes?: number;
}

/**
 * A SHA-256 chain over the tapes of a run of leading messages, so a later pass can recognize the
 * run unchanged without retaining its strings. Symbols other than the tape markers are kept for
 * identity.
 */
export interface HistoryDigest {
    readonly count: number;
    readonly digest: string;
    readonly symbols: readonly symbol[];
    /** The walks' total spend on the run; walks over identical content spend exactly this. */
    readonly bytes: number;
}

export interface ReferenceableRejection {
    reason:
        | "proxy"
        | "accessor"
        | "prototype"
        | "prototype_accessor"
        | "boxed_primitive"
        | "cycle"
        | "depth"
        | "sparse_array"
        | "undefined_element"
        | "nonfinite_number"
        | "bigint"
        | "function"
        | "symbol"
        | "undefined"
        | "not_array"
        | "to_json"
        | "extra_property";
    path: string;
}

/** A bounded walk or reservation failed; the transform owner treats this as a local decline. */
export class CaptureBudgetExceeded extends Error {
    constructor(detail: string) {
        super(`transform capture byte budget exceeded: ${detail}`);
    }
}

// Object.defineProperty bypasses inherited numeric setters on membership and output arrays.
function defineSlot<T>(array: T[], index: number, value: T): void {
    Object.defineProperty(array, index, {
        value,
        writable: true,
        enumerable: true,
        configurable: true,
    });
}

/** Own data property read that cannot run an accessor or proxy trap. */
export function readOwnDataProperty(value: unknown, key: PropertyKey): unknown {
    if (value === null || typeof value !== "object" || types.isProxy(value)) return undefined;
    const descriptor = Object.getOwnPropertyDescriptor(value, key);
    return descriptor && Object.hasOwn(descriptor, "value") ? descriptor.value : undefined;
}

/** The id the daemon's decoder names a message by: `info.id`, else the top-level `id`. Each hop is an own data read, so a planted proxy, revoked proxy, or accessor reads as no id and none of its traps or getters runs. */
export function messageId(message: unknown): string | undefined {
    const nested = readOwnDataProperty(readOwnDataProperty(message, "info"), "id");
    if (typeof nested === "string") return nested;
    const top = readOwnDataProperty(message, "id");
    return typeof top === "string" ? top : undefined;
}

/** 32-bit FNV-1a over UTF-16 code units. */
export function fnv1a32(text: string): number {
    let hash = 0x811c9dc5;
    for (let index = 0; index < text.length; index += 1)
        hash = Math.imul(hash ^ text.charCodeAt(index), 0x01000193);
    return hash >>> 0;
}

export function filterMayHold(filter: Uint32Array, id: string): boolean {
    const hash = fnv1a32(id);
    let low = 0;
    let high = filter.length;
    while (low < high) {
        const middle = (low + high) >>> 1;
        if ((filter[middle] as number) < hash) low = middle + 1;
        else high = middle;
    }
    return low < filter.length && filter[low] === hash;
}

/** Copies `host[start..end)` through indexed own data descriptors; a hole or accessor slot yields `undefined`. */
export function copyWindow(
    host: readonly unknown[],
    start: number,
    end: number,
): unknown[] | undefined {
    const window: unknown[] = [];
    for (let index = start; index < end; index += 1) {
        const slot = Object.getOwnPropertyDescriptor(host, index);
        if (!slot || !Object.hasOwn(slot, "value")) return undefined;
        defineSlot(window, index - start, slot.value);
    }
    return window;
}

// Source values are arrays, plain objects, strings, numbers and booleans; a read that misses an
// own property, or any method call on a primitive, resolves through one of these prototypes.
const BUILTIN_PROTOTYPES: readonly (readonly [string, object])[] = [
    ["Array", Array.prototype],
    ["Object", Object.prototype],
    ["String", String.prototype],
    ["Number", Number.prototype],
    ["Boolean", Boolean.prototype],
];

/**
 * The root array is the one value that crosses back to the host as a return value, so a `then`
 * anywhere on its chain would be assimilated by the host's promise machinery; `in` observes an
 * inherited accessor without invoking it. Reads of absent optional fields and out-of-range indexes
 * fall through to the built-in prototypes, so an accessor installed there is refused the same way.
 */
export function rootArrayRejection(value: unknown): ReferenceableRejection | undefined {
    if (types.isProxy(value)) return { reason: "proxy", path: "" };
    if (!Array.isArray(value)) return { reason: "not_array", path: "" };
    if (
        Object.getPrototypeOf(value) !== Array.prototype ||
        Object.getPrototypeOf(Array.prototype) !== Object.prototype ||
        Object.getPrototypeOf(Object.prototype) !== null
    )
        return { reason: "prototype", path: "" };
    if ("then" in value) return { reason: "extra_property", path: "/then" };
    // Indexed loops: `for...of` would read `Array.prototype[Symbol.iterator]` before it is inspected.
    for (let p = 0; p < BUILTIN_PROTOTYPES.length; p += 1) {
        const name = BUILTIN_PROTOTYPES[p]![0];
        const prototype = BUILTIN_PROTOTYPES[p]![1];
        const keys = Reflect.ownKeys(prototype);
        for (let k = 0; k < keys.length; k += 1) {
            const key = keys[k]!;
            const slot = Object.getOwnPropertyDescriptor(prototype, key);
            if (key !== "__proto__" && slot && !Object.hasOwn(slot, "value"))
                return { reason: "prototype_accessor", path: `${name}.prototype/${String(key)}` };
        }
    }
    return undefined;
}

/** `JSON_ESCAPE_CANDIDATE` matches every UTF-16 unit that can increase `jsonEscapeGrowth`. */
const JSON_ESCAPE_CANDIDATE = /["\\]|[^\x20-\ud7ff\ue000-\uffff]/;

/** `jsonEscapeGrowth` returns the additional bytes JSON escaping requires above two bytes per UTF-16 unit. */
function jsonEscapeGrowth(value: string): number {
    if (!JSON_ESCAPE_CANDIDATE.test(value)) return 0;
    let growth = 0;
    for (let index = 0; index < value.length; index += 1) {
        const unit = value.charCodeAt(index);
        if (unit === 0x22 || unit === 0x5c || (unit >= 0x08 && unit <= 0x0d && unit !== 0x0b)) {
            growth += 2;
        } else if (unit < 0x20) {
            growth += 10;
        } else if (unit >= 0xd800 && unit <= 0xdbff) {
            const next = value.charCodeAt(index + 1);
            if (next >= 0xdc00 && next <= 0xdfff) index += 1;
            else growth += 10;
        } else if (unit >= 0xdc00 && unit <= 0xdfff) {
            growth += 10;
        }
    }
    return growth;
}

function childPath(path: string, child: string | number | undefined): string {
    return child === undefined ? path : `${path}/${child}`;
}

class SourceRejected extends Error {
    constructor(
        readonly reason: ReferenceableRejection["reason"],
        readonly path: string,
    ) {
        super(`unsupported source: ${reason} at ${path}`);
    }
}
const CONTENT_CHANGED = Symbol("content_changed");

/**
 * The charge covers retained tape slots and the strings they can keep alive, not engine
 * enumeration, transient descriptors, or RSS. Own-name inspection includes hidden accessors; a
 * field-name allowlist would drift from consumers.
 */
class ReferenceableWalk {
    bytes = 0;
    /** Upper bound on the canonical JSON length of the walked values, in two-byte units for strings. */
    wireBytes = 0;
    private field?: (value: SnapshotField) => void;
    private readonly ancestors = new Set<object>();
    /**
     * Built-in prototype state the walk checks once: own descriptor reads run no source code, so
     * the state holds for the rest of a synchronous walk.
     */
    private arrayChain?: boolean;
    private arrayPrototypeToJsonChecked = false;
    private objectPrototypeToJsonChecked = false;

    /** Only inspection reads `wireBytes`; capture and recheck walks skip the per-unit escape scan. */
    constructor(
        private readonly maxBytes = TRANSFORM_CAPTURE_MAX_BYTES,
        private readonly estimateWire = true,
    ) {}

    spend(bytes: number): void {
        if (!Number.isSafeInteger(bytes) || bytes < 0 || bytes > this.maxBytes - this.bytes) {
            throw new CaptureBudgetExceeded("source and snapshot walk");
        }
        this.bytes += bytes;
    }

    /** `wireBytes` counts only tokens JSON serialization emits; tape-only markers pass zero. */
    private emit(value: SnapshotField, wireBytes: number): void {
        // A tape slot keeps its string, or a symbol's description, alive after the host drops it.
        if (typeof value === "string") {
            this.spend(TAPE_SLOT_BYTES + value.length * 2);
            if (wireBytes > 0 && this.estimateWire) wireBytes += jsonEscapeGrowth(value);
        } else
            this.spend(
                TAPE_SLOT_BYTES +
                    (typeof value === "symbol" ? (value.description?.length ?? 0) * 2 : 0),
            );
        if (wireBytes !== 0) this.wire(wireBytes);
        this.field?.(value);
    }

    /** Records a number or boolean that JSON serialization never emits. */
    private scalar(value: number | boolean): void {
        this.spend(TAPE_SLOT_BYTES);
        this.field?.(value);
    }

    private wire(bytes: number): void {
        const total = this.wireBytes + bytes;
        if (!Number.isSafeInteger(total)) throw new CaptureBudgetExceeded("wire estimate");
        this.wireBytes = total;
    }

    /** Walks `value` at `path`, or at `path/key` when `key` is given; the joined path is built only when needed. */
    walk(value: unknown, path = "", key?: string): void {
        const type = typeof value;
        if (value === null || type !== "object") {
            if (type === "number" && !Number.isFinite(value))
                throw new SourceRejected("nonfinite_number", childPath(path, key));
            if (value !== null && type !== "string" && type !== "number" && type !== "boolean")
                throw new SourceRejected(
                    type as ReferenceableRejection["reason"],
                    childPath(path, key),
                );
            this.emit(
                value as SnapshotField,
                type === "string" ? (value as string).length * 2 + 4 : SCALAR_WIRE_BYTES,
            );
            return;
        }
        this.entries(value as object, childPath(path, key));
    }

    members(messages: unknown, visit: (slot: PropertyDescriptor, index: number) => void): number {
        const rejection = rootArrayRejection(messages);
        if (rejection) throw new SourceRejected(rejection.reason, rejection.path);
        this.spend(ROOT_CAPTURE_BYTES);
        return this.entries(messages as unknown[], "", (key, slot) => {
            this.spend(ROOT_MEMBER_BYTES);
            visit(slot, Number(key));
        });
    }

    /** Hands each tape field to `sink` instead of recording it. */
    stream(operation: () => void, sink: (value: SnapshotField) => void): void {
        const outer = this.field;
        this.field = sink;
        try {
            operation();
        } finally {
            this.field = outer;
        }
    }

    recordOrCompare(
        operation: () => void,
        expected?: MessageContentSnapshot,
    ): MessageContentSnapshot {
        // Private tapes have no inherited setters. Returned tapes regain the ordinary array API.
        const fields: SnapshotField[] = expected?.fields ?? Object.setPrototypeOf([], null);
        let index = 0;
        const outer = this.field;
        this.field = (value) => {
            if (expected) {
                if (index >= fields.length || !Object.is(value, fields[index]))
                    throw CONTENT_CHANGED;
            } else fields[index] = value;
            index += 1;
        };
        try {
            operation();
            if (expected && index !== fields.length) throw CONTENT_CHANGED;
            return expected ?? { fields: Object.setPrototypeOf(fields, Array.prototype) };
        } finally {
            this.field = outer;
        }
    }

    private attributes(slot: PropertyDescriptor): void {
        this.scalar(slot.enumerable === true);
        this.scalar(slot.writable === true);
        this.scalar(slot.configurable === true);
    }

    /**
     * Every descriptor read spends one slot so the byte budget also bounds traversal work. A
     * rejection names `path`, or `path/child` when `child` is given.
     */
    private data(
        value: object,
        key: PropertyKey,
        path: string,
        child?: string | number,
    ): PropertyDescriptor {
        this.spend(TAPE_SLOT_BYTES);
        const slot = Object.getOwnPropertyDescriptor(value, key);
        if (!slot) throw new SourceRejected("sparse_array", childPath(path, child));
        if (!Object.hasOwn(slot, "value"))
            throw new SourceRejected("accessor", childPath(path, child));
        return slot;
    }

    /** JSON looks up `toJSON` even when it is hidden or inherited. */
    private rejectToJson(holder: object, path: string): void {
        const hook = Object.getOwnPropertyDescriptor(holder, "toJSON");
        if (hook && (!Object.hasOwn(hook, "value") || hook.value !== undefined)) {
            const reason = !Object.hasOwn(hook, "value")
                ? "accessor"
                : types.isProxy(hook.value)
                  ? "proxy"
                  : typeof hook.value === "function"
                    ? "function"
                    : "to_json";
            throw new SourceRejected(reason, `${path}/toJSON`);
        }
    }

    /**
     * Proxy rejection precedes prototype and descriptor reads; source iterators are never used.
     * Without `visit`, each counted entry is walked at `path/key`.
     */
    private entries(
        value: object,
        path: string,
        visit?: (key: string, slot: PropertyDescriptor) => void,
    ): number {
        if (types.isProxy(value)) throw new SourceRejected("proxy", path);
        // JSON serializes a boxed primitive by its internal slot, which no own property records.
        if (types.isBoxedPrimitive(value)) throw new SourceRejected("boxed_primitive", path);
        const array = Array.isArray(value);
        const prototype = Object.getPrototypeOf(value);
        if (
            (array
                ? prototype !== Array.prototype
                : prototype !== Object.prototype && prototype !== null) ||
            !(this.arrayChain ??= Object.getPrototypeOf(Array.prototype) === Object.prototype)
        )
            throw new SourceRejected("prototype", path);
        if (this.ancestors.size > MAX_REFERENCEABLE_DEPTH) throw new SourceRejected("depth", path);
        if (this.ancestors.has(value)) throw new SourceRejected("cycle", path);
        // The holder chain is the value, then `Array.prototype` for an array, then `Object.prototype` unless the prototype is null.
        this.rejectToJson(value, path);
        if (array && !this.arrayPrototypeToJsonChecked) {
            this.rejectToJson(Array.prototype, path);
            this.arrayPrototypeToJsonChecked = true;
        }
        if (prototype !== null && !this.objectPrototypeToJsonChecked) {
            this.rejectToJson(Object.prototype, path);
            this.objectPrototypeToJsonChecked = true;
        }
        const lengthSlot = array ? this.data(value, "length", path) : undefined;
        const length: number = lengthSlot?.value ?? 0;
        const symbols = Object.getOwnPropertySymbols(value);
        if (array) {
            // The declared length is charged before any element read so a huge sparse length fails early.
            this.spend(length);
            for (let index = 0; index < symbols.length; index += 1)
                if (ARRAY_SYMBOLS.includes(symbols[index] as symbol))
                    throw new SourceRejected("extra_property", path);
        }
        // Opening and closing brackets; each element or key pays its own separator.
        this.emit(array ? ARRAY : OBJECT, 1);
        // A null prototype changes what an absent optional field reads as, so the tape records it.
        if (!array) this.scalar(prototype === null);
        if (lengthSlot) {
            this.scalar(length);
            this.attributes(lengthSlot);
        }
        this.ancestors.add(value);
        let count = 0;
        try {
            for (const key of symbols) {
                const slot = this.data(value, key, path);
                this.emit(EXTRA_KEY, 0);
                this.emit(key, 0);
                this.attributes(slot);
                this.walk(slot.value, path);
            }
            for (const key of Object.getOwnPropertyNames(value)) {
                if (array && key === "length") continue;
                if (array && ARRAY_METHODS.has(key))
                    throw new SourceRejected("extra_property", path);
                const slot = this.data(value, key, path, key);
                // Own names list integer indexes ascending, so an index key that is not `count` leaves a hole at `count`.
                if (array && key !== String(count)) {
                    const index = Number(key);
                    if (Number.isInteger(index) && index >= 0 && String(index) === key)
                        this.data(value, String(count), path, count);
                    this.emit(EXTRA_KEY, 0);
                    this.emit(key, 0);
                    this.attributes(slot);
                    this.walk(slot.value, path, key);
                    continue;
                }
                if (slot.value === undefined) {
                    if (array) throw new SourceRejected("undefined_element", `${path}/${key}`);
                    continue;
                }
                // Object keys render quoted with a colon; array elements pay only their comma.
                if (array) this.wire(1);
                else this.emit(key, key.length * 2 + 6);
                this.attributes(slot);
                if (visit) visit(key, slot);
                else this.walk(slot.value, path, key);
                count += 1;
            }
            if (array && count !== length) this.data(value, String(count), path, count);
            this.emit(array ? END_ARRAY : END_OBJECT, 1);
            this.scalar(count);
            return count;
        } finally {
            this.ancestors.delete(value);
        }
    }
}

export interface ReferenceableInspection {
    ok: true;
    /** Per-message upper bounds for JSON escaping, not retained heap charges. */
    messageWireBytes: number[];
    estimatedBytes: number;
}

/**
 * Byte-limit exhaustion throws CaptureBudgetExceeded. The first `skip` members are left to a
 * digest-verified capture: only their root slots are inspected, and their wire bounds read zero.
 * Without `estimateWire`, the wire bounds leave out string escapes; `estimatedBytes` is the same.
 */
export function inspectReferenceableMessages(
    messages: unknown,
    maxBytes = TRANSFORM_CAPTURE_MAX_BYTES,
    skip = 0,
    estimateWire = true,
): ReferenceableInspection | { ok: false; rejection: ReferenceableRejection } {
    const walker = new ReferenceableWalk(maxBytes, estimateWire);
    const messageWireBytes: number[] = [];
    try {
        walker.members(messages, (slot, index) => {
            if (index < skip) {
                defineSlot(messageWireBytes, index, 0);
                return;
            }
            const wireBefore = walker.wireBytes;
            walker.walk(slot.value, `/${index}`);
            defineSlot(messageWireBytes, index, walker.wireBytes - wireBefore);
        });
    } catch (error) {
        if (error instanceof SourceRejected)
            return { ok: false, rejection: { reason: error.reason, path: error.path } };
        throw error;
    }
    return { ok: true, estimatedBytes: walker.bytes, messageWireBytes };
}

export function snapshotFieldsEqual(
    left: MessageContentSnapshot,
    right: MessageContentSnapshot,
): boolean {
    if (left.fields.length !== right.fields.length) return false;
    for (let index = 0; index < left.fields.length; index += 1) {
        if (!Object.is(left.fields[index], right.fields[index])) return false;
    }
    return true;
}

/** Long strings are hashed in slices of this many UTF-16 units. */
const HASH_CHUNK_UNITS = 1 << 16;
/** Below this length a string stays in the pending units, which saves a hash update per key. */
const UTF8_HASH_MIN_UNITS = 256;
/** Pending token units are flushed to the hash once this many are buffered. */
const PENDING_HASH_UNITS = 1 << 12;

interface Sha256 {
    update(data: string | Uint8Array, encoding?: "utf8" | "utf16le"): void;
    copy(): Sha256;
    digest(encoding: "base64"): string;
}
const BunCryptoHasher = (
    globalThis as { Bun?: { CryptoHasher?: new (algorithm: "sha256") => Sha256 } }
).Bun?.CryptoHasher;
const sha256 = (): Sha256 =>
    BunCryptoHasher ? new BunCryptoHasher("sha256") : createHash("sha256");

/**
 * Streams member tapes into one SHA-256 chain. Every token is self-delimiting: a string carries
 * its UTF-16 length, a number ends at `;`, and a member ends at `|`. The text is hashed as
 * UTF-16 code units, which keeps lone surrogates and makes the digest independent of chunking.
 * A well-formed string of [`UTF8_HASH_MIN_UNITS`, `HASH_CHUNK_UNITS`) units is hashed as UTF-8
 * after a `u` token instead: UTF-8 is injective on well-formed text, and the UTF-16 length still
 * ends it. Symbols other than the tape markers cannot be hashed by identity, so they are kept in order.
 */
class TapeHasher {
    private readonly hash = sha256();
    private readonly pending = new Uint16Array(PENDING_HASH_UNITS);
    private pendingUnits = 0;
    private readonly symbols: symbol[] = Object.setPrototypeOf([], null);
    private count = 0;
    private bytes = 0;

    readonly push = (value: SnapshotField): void => {
        if (typeof value === "string") {
            const length = value.length;
            if (length < UTF8_HASH_MIN_UNITS) {
                this.token(0x73, length, 0x3a);
                this.append(value);
            } else if (
                length < HASH_CHUNK_UNITS &&
                (value as string & { isWellFormed(): boolean }).isWellFormed()
            ) {
                this.token(0x75, length, 0x3a);
                this.flush();
                this.hash.update(value, "utf8");
            } else {
                this.token(0x73, length, 0x3a);
                this.flush();
                for (let start = 0; start < length; start += HASH_CHUNK_UNITS)
                    this.hash.update(value.slice(start, start + HASH_CHUNK_UNITS), "utf16le");
            }
        } else if (typeof value === "number") {
            if (Object.is(value, -0)) this.unit(0x2d);
            else this.token(0x6e, value, 0x3b);
        } else if (typeof value === "boolean") this.unit(value ? 0x74 : 0x66);
        else if (value === null) this.unit(0x7a);
        else if (value === ARRAY) this.unit(0x5b);
        else if (value === END_ARRAY) this.unit(0x5d);
        else if (value === OBJECT) this.unit(0x7b);
        else if (value === END_OBJECT) this.unit(0x7d);
        else if (value === EXTRA_KEY) this.unit(0x2b);
        else {
            this.unit(0x79);
            this.symbols[this.symbols.length] = value;
        }
    };

    /** Closes one member whose walk spent `bytes`. */
    end(bytes: number): void {
        this.unit(0x7c);
        this.count += 1;
        this.bytes += bytes;
    }

    private unit(code: number): void {
        if (this.pendingUnits === PENDING_HASH_UNITS) this.flush();
        this.pending[this.pendingUnits++] = code;
    }

    /** The `tag` unit, the decimal text of `value`, then the `close` unit. */
    private token(tag: number, value: number, close: number): void {
        this.unit(tag);
        this.append(String(value));
        this.unit(close);
    }

    private append(text: string): void {
        if (this.pendingUnits + text.length > PENDING_HASH_UNITS) this.flush();
        for (let index = 0; index < text.length; index += 1)
            this.pending[this.pendingUnits++] = text.charCodeAt(index);
    }

    private flush(): void {
        if (this.pendingUnits === 0) return;
        this.hash.update(new Uint8Array(this.pending.buffer, 0, this.pendingUnits * 2));
        this.pendingUnits = 0;
    }

    add(snapshot: TapedMember): void {
        for (let index = 0; index < snapshot.fields.length; index += 1)
            this.push(snapshot.fields[index] as SnapshotField);
        this.end(snapshot.bytes);
    }

    /** The chain so far; hashing may continue afterwards. */
    digest(): HistoryDigest {
        this.flush();
        const symbols: symbol[] = [];
        for (let index = 0; index < this.symbols.length; index += 1)
            defineSlot(symbols, index, this.symbols[index] as symbol);
        return {
            count: this.count,
            digest: this.hash.copy().digest("base64"),
            symbols,
            bytes: this.bytes,
        };
    }
}

interface TapedMember {
    fields: readonly SnapshotField[];
    bytes: number;
}

export function historyDigestsEqual(left: HistoryDigest, right: HistoryDigest): boolean {
    if (left === right) return true;
    if (
        left.count !== right.count ||
        left.digest !== right.digest ||
        left.bytes !== right.bytes ||
        left.symbols.length !== right.symbols.length
    )
        return false;
    for (let index = 0; index < left.symbols.length; index += 1) {
        if (left.symbols[index] !== right.symbols[index]) return false;
    }
    return true;
}

/**
 * Walks the members of a digested prefix under the prefix's recorded spend, so a changed prefix
 * costs no more than the original. Each walk is standalone; only its spend and hash are kept.
 */
class PrefixVerifier {
    private readonly walker: ReferenceableWalk;
    private readonly hasher = new TapeHasher();

    constructor(readonly expected: HistoryDigest) {
        this.walker = new ReferenceableWalk(expected.bytes, false);
    }

    /** Throws SourceRejected or CaptureBudgetExceeded when the member cannot match. */
    walk(value: unknown, index: number): void {
        const before = this.walker.bytes;
        this.walker.stream(() => this.walker.walk(value, `/${index}`), this.hasher.push);
        this.hasher.end(this.walker.bytes - before);
    }

    matches(): boolean {
        return historyDigestsEqual(this.hasher.digest(), this.expected);
    }

    continueHashing(): TapeHasher {
        return this.hasher;
    }
}

export interface CapturedMessages {
    members: readonly unknown[];
    /** Members covered by `verified` have no tape; their recheck walks the digest. */
    snapshots: readonly (MessageContentSnapshot | undefined)[];
    rootSnapshot: MessageContentSnapshot;
    /** The prior digest the leading members matched, when the capture was given one. */
    verified?: HistoryDigest;
}

export interface CapturedHistory extends CapturedMessages {
    /** `history` excludes the final member; `terminal` hashes it alone. */
    history: HistoryDigest;
    terminal?: HistoryDigest;
    /** The first taped member alone, compared against a prior pass's terminal. */
    boundary?: HistoryDigest;
}

const PREFIX_CHANGED = Symbol("prefix_changed");

/**
 * Captures `messages` within the lease's headroom, then reserves the walk's spend, which equals
 * the charge an inspection without wire estimates reports. The walk and the reservation form one
 * synchronous section, so no other reservation runs while the capture is unreserved. `undefined`
 * when the lease is cancelled, a value is not referenceable, or the headroom is short; nothing is
 * then reserved. `unchanged` is a capture `capturedMessagesUnchanged` confirmed for its values
 * with no source code run since; a member among those values takes its snapshot and spend.
 */
export function captureReserved(
    messages: unknown,
    lease: CaptureLease,
    unchanged?: { values: readonly unknown[]; capture: CapturedMessages },
): { capture: CapturedMessages; bytes: number } | undefined {
    if (lease.signal.aborted) return undefined;
    const walker = new ReferenceableWalk(
        Math.min(lease.remainingBytes, TRANSFORM_CAPTURE_MAX_BYTES),
        false,
    );
    const reuse = new Map<unknown, MessageContentSnapshot | undefined>();
    for (let index = 0; unchanged && index < unchanged.values.length; index += 1)
        reuse.set(unchanged.values[index], unchanged.capture.snapshots[index]);
    let capture: CapturedMessages;
    try {
        capture = walkMembers(walker, messages, undefined, reuse);
    } catch (error) {
        if (error instanceof SourceRejected || error instanceof CaptureBudgetExceeded)
            return undefined;
        throw error;
    }
    return lease.reserve(walker.bytes) ? { capture, bytes: walker.bytes } : undefined;
}

/**
 * With a `prefix`, capture charges only the root and members after the prefix; it verifies
 * leading members instead of retaining them. A mismatch, or no member after the prefix, returns
 * `undefined`.
 */
export function captureHistory(messages: unknown, lease: CaptureLease): CapturedHistory;
export function captureHistory(
    messages: unknown,
    lease: CaptureLease,
    prefix: HistoryDigest,
): CapturedHistory | undefined;
export function captureHistory(
    messages: unknown,
    lease: CaptureLease,
    prefix?: HistoryDigest,
): CapturedHistory | undefined {
    const verifier = prefix && new PrefixVerifier(prefix);
    const taped: TapedMember[] = [];
    let captured: CapturedMessages;
    try {
        captured = walkCapture(messages, lease, { taped, verifier });
    } catch (error) {
        if (error === PREFIX_CHANGED) return undefined;
        throw error;
    }
    const hasher = verifier?.continueHashing() ?? new TapeHasher();
    for (let index = 0; index < taped.length - 1; index += 1)
        hasher.add(taped[index] as TapedMember);
    const single = (member: TapedMember | undefined): HistoryDigest | undefined => {
        if (!member) return undefined;
        const alone = new TapeHasher();
        alone.add(member);
        return alone.digest();
    };
    const boundary = single(taped[0]);
    return {
        ...captured,
        verified: prefix,
        history: hasher.digest(),
        terminal: taped.length > 1 ? single(taped[taped.length - 1]) : boundary,
        boundary,
    };
}

/** `digest.verifier` validates leading members; a mismatch throws `PREFIX_CHANGED`. */
function walkCapture(
    messages: unknown,
    lease: CaptureLease,
    digest: { taped: TapedMember[]; verifier?: PrefixVerifier },
): CapturedMessages {
    if (lease.signal.aborted || lease.chargedBytes < ROOT_CAPTURE_BYTES)
        throw new CaptureBudgetExceeded("capture requires a live reservation");
    return walkMembers(
        new ReferenceableWalk(Math.min(lease.chargedBytes, TRANSFORM_CAPTURE_MAX_BYTES), false),
        messages,
        digest,
    );
}

function walkMembers(
    walker: ReferenceableWalk,
    messages: unknown,
    digest?: { taped: TapedMember[]; verifier?: PrefixVerifier },
    reuse?: ReadonlyMap<unknown, MessageContentSnapshot | undefined>,
): CapturedMessages {
    const verifier = digest?.verifier;
    const verifiedCount = verifier?.expected.count ?? 0;
    const members: unknown[] = [];
    const snapshots: (MessageContentSnapshot | undefined)[] = [];
    const rootSnapshot = walker.recordOrCompare(() => {
        const count = walker.members(messages, (slot, index) => {
            if (verifier && index < verifiedCount) {
                try {
                    verifier.walk(slot.value, index);
                } catch (error) {
                    if (error instanceof SourceRejected || error instanceof CaptureBudgetExceeded)
                        throw PREFIX_CHANGED;
                    throw error;
                }
                defineSlot(snapshots, index, undefined);
            } else {
                if (index === verifiedCount && verifier && !verifier.matches())
                    throw PREFIX_CHANGED;
                let snapshot = reuse?.get(slot.value);
                if (snapshot?.bytes !== undefined) walker.spend(snapshot.bytes);
                else {
                    const before = walker.bytes;
                    snapshot = walker.recordOrCompare(() => walker.walk(slot.value, `/${index}`));
                    snapshot.bytes = walker.bytes - before;
                }
                defineSlot(snapshots, index, snapshot);
                if (digest)
                    defineSlot(digest.taped, digest.taped.length, {
                        fields: snapshot.fields,
                        bytes: snapshot.bytes,
                    });
            }
            defineSlot(members, index, slot.value);
        });
        if (verifier && count <= verifiedCount) throw PREFIX_CHANGED;
    });
    return { members, snapshots, rootSnapshot };
}

/** Membership is checked through own descriptors; an accessor or inherited slot cannot match. */
export function capturedMessagesUnchanged(live: unknown, captured: CapturedMessages): boolean {
    try {
        const walker = new ReferenceableWalk(TRANSFORM_CAPTURE_MAX_BYTES, false);
        const verifier = captured.verified && new PrefixVerifier(captured.verified);
        walker.recordOrCompare(() => {
            const count = walker.members(live, (slot, index) => {
                if (
                    index >= captured.members.length ||
                    !Object.is(slot.value, captured.members[index])
                )
                    throw CONTENT_CHANGED;
                if (verifier && index < verifier.expected.count) {
                    verifier.walk(slot.value, index);
                    return;
                }
                // Bounds precede indexing so a short record cannot read an inherited slot.
                const snapshot =
                    index < captured.snapshots.length ? captured.snapshots[index] : undefined;
                if (!snapshot) throw CONTENT_CHANGED;
                walker.recordOrCompare(() => walker.walk(slot.value, `/${index}`), snapshot);
            });
            if (count !== captured.members.length) throw CONTENT_CHANGED;
            if (verifier && !verifier.matches()) throw CONTENT_CHANGED;
        }, captured.rootSnapshot);
        return true;
    } catch (error) {
        if (
            error === CONTENT_CHANGED ||
            error instanceof SourceRejected ||
            error instanceof CaptureBudgetExceeded
        )
            return false;
        throw error;
    }
}

export interface CaptureLease {
    readonly signal: AbortSignal;
    readonly chargedBytes: number;
    /** Owner-wide headroom shared by every lease, not this lease's own remainder; zero once released. */
    readonly remainingBytes: number;
    reserve(bytes: number): boolean;
    /** Returns every byte this lease holds to the owner and keeps the lease live; the caller must hold no charged capture at the call. */
    refund(): void;
    requestCancel(reason: string): void;
    release(): void;
}

/** TransformCaptureAdmission permits one live lease per session. */
export class TransformCaptureAdmission {
    private readonly leases = new Map<string, CaptureLease>();
    private chargedBytesTotal = 0;
    constructor(
        private readonly limits = {
            maxPasses: TRANSFORM_CAPTURE_MAX_PASSES,
            maxBytes: TRANSFORM_CAPTURE_MAX_BYTES,
        },
    ) {}

    get activePasses(): number {
        return this.leases.size;
    }
    get chargedBytes(): number {
        return this.chargedBytesTotal;
    }
    get remainingBytes(): number {
        return this.limits.maxBytes - this.chargedBytesTotal;
    }

    admit(
        sessionId: string,
    ): { lease: CaptureLease } | { declined: "session_busy" | "pass_count" } {
        const held = this.leases.get(sessionId);
        if (held) {
            held.requestCancel(
                `transform pass for session ${sessionId} superseded by a newer call`,
            );
            return { declined: "session_busy" };
        }
        if (this.leases.size >= this.limits.maxPasses) return { declined: "pass_count" };
        const controller = new AbortController();
        const owner = this;
        let bytes = 0;
        let released = false;
        const lease: CaptureLease = {
            signal: controller.signal,
            get chargedBytes() {
                return bytes;
            },
            get remainingBytes() {
                return released ? 0 : owner.remainingBytes;
            },
            reserve(additional) {
                if (
                    released ||
                    !Number.isSafeInteger(additional) ||
                    additional < 0 ||
                    additional > owner.remainingBytes
                )
                    return false;
                owner.chargedBytesTotal += additional;
                bytes += additional;
                return true;
            },
            refund() {
                owner.chargedBytesTotal -= bytes;
                bytes = 0;
            },
            requestCancel(reason) {
                if (!controller.signal.aborted) controller.abort(new Error(reason));
            },
            release() {
                if (released) return;
                released = true;
                owner.chargedBytesTotal -= bytes;
                bytes = 0;
                owner.leases.delete(sessionId);
            },
        };
        this.leases.set(sessionId, lease);
        return { lease };
    }

    requestCancel(sessionId: string, reason: string): void {
        this.leases.get(sessionId)?.requestCancel(reason);
    }
}

/** Module-wide default owner; tests can inject a separate instance. */
export const defaultTransformCaptureAdmission = new TransformCaptureAdmission();
