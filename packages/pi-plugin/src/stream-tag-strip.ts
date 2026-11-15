const SECTION = "\u00a7";
const MAX_TAG_DIGITS = 19;

function isDigit(char: string | undefined): boolean {
    return char !== undefined && char >= "0" && char <= "9";
}

function isWhitespace(char: string | undefined): boolean {
    return char !== undefined && /\s/u.test(char);
}

/** The index after the `§N§` tag or mangled `§N">` tag at `at`, `"open"` while more text could change the answer, or `undefined` when `at` starts no tag. With `final`, the text has ended, so the longest tag complete at the end counts. */
function tagEnd(input: string, at: number, final: boolean): number | "open" | undefined {
    const open = (complete: number | undefined) => (final ? complete : "open");
    if (input[at] !== SECTION) return undefined;
    const digits = at + 1;
    let cursor = digits;
    while (isDigit(input[cursor])) {
        cursor += 1;
        if (cursor - digits > MAX_TAG_DIGITS) return undefined;
    }
    if (cursor === input.length) return open(undefined);
    if (cursor === digits) return undefined;
    if (input[cursor] === SECTION) return cursor + 1;
    if (input[cursor] !== '"') return undefined;
    cursor += 1;
    if (cursor === input.length) return open(undefined);
    if (input[cursor] !== ">") return undefined;
    cursor += 1;
    if (cursor === input.length) return open(cursor);
    if (input[cursor] !== SECTION) return cursor;
    cursor += 1;
    const closed = cursor;
    while (isDigit(input[cursor])) {
        cursor += 1;
        if (cursor - closed > MAX_TAG_DIGITS) return closed;
    }
    if (cursor === input.length) return open(closed);
    return cursor > closed && input[cursor] === SECTION ? cursor + 1 : closed;
}

interface ScanState {
    leading: boolean;
    strippedOpeningTag: boolean;
}

/** The released ranges of the latest `scan` as `[from, to)` pairs of its input, in order and with adjacent ranges merged: `kept[0..keptLength)`. The array only grows, so a scan allocates only when it records more ranges than any earlier scan. */
const kept: number[] = [];
let keptLength = 0;

function keep(from: number, to: number): void {
    if (to <= from) return;
    if (keptLength > 0 && kept[keptLength - 1] === from) {
        kept[keptLength - 1] = to;
        return;
    }
    kept[keptLength] = from;
    kept[keptLength + 1] = to;
    keptLength += 2;
}

/** The text of the latest scan's released ranges in `input`. */
function keptText(input: string): string {
    let out = "";
    for (let i = 0; i < keptLength; i += 2) out += input.slice(kept[i], kept[i + 1]);
    return out;
}

/** Resolves `input` from `index` under the stripping rule, records each released range in `kept`, and returns the index where held text begins (`input.length` when nothing is held). The scan reads only `input[index..]`, so a caller can rescan its held text in place once more text follows it. */
function scan(input: string, index: number, final: boolean, state: ScanState): number {
    keptLength = 0;
    while (index < input.length) {
        if (state.leading) {
            const start = index;
            while (isWhitespace(input[index])) index += 1;
            if (index === input.length) {
                if (!final) return start;
                if (!state.strippedOpeningTag) keep(start, index);
                return index;
            }
            const end = tagEnd(input, index, final);
            if (end === "open") return start;
            if (end !== undefined) {
                state.strippedOpeningTag = true;
                index = end;
                continue;
            }
            state.leading = false;
            if (!state.strippedOpeningTag) keep(start, index);
            continue;
        }
        const section = input.indexOf(SECTION, index);
        if (section === -1) {
            keep(index, input.length);
            return input.length;
        }
        const end = tagEnd(input, section, final);
        if (end === "open") {
            keep(index, section);
            return section;
        }
        if (end === undefined) {
            keep(index, section + 1);
            index = section + 1;
        } else {
            keep(index, section);
            index = end;
        }
    }
    return index;
}

/** A stream fed text in pieces, where `held` waits for the next piece. */
interface DeltaStream {
    held: string;
    state: ScanState;
}

function pushPiece(stream: DeltaStream, piece: string, final: boolean): string {
    const input = stream.held + piece;
    const held = scan(input, 0, final, stream.state);
    stream.held = input.slice(held);
    if (final) stream.state.leading = false;
    return keptText(input);
}

/** StreamTagStripper removes `§N§` tags containing 1 to 19 ASCII digits, and the mangled `§N">` form with an optional `§` or `§N§` after it, across delta boundaries. It holds a trailing prefix of either form until another delta resolves it or `flush()` ends the text, and passes every other `§` through, so `§ 4` and `§42 of the Act` stream unchanged. With `leading` enabled, opening tags consume their surrounding whitespace. Released text only grows. */
export class StreamTagStripper {
    #released = "";
    #stream: DeltaStream;

    constructor(leading: boolean) {
        this.#stream = { held: "", state: { leading, strippedOpeningTag: false } };
    }

    get released(): string {
        return this.#released;
    }

    push(delta: string): string {
        const out = pushPiece(this.#stream, delta, false);
        this.#released += out;
        return out;
    }

    flush(): string {
        const out = pushPiece(this.#stream, "", true);
        this.#released += out;
        return out;
    }
}

export function stripTags(text: string, leading: boolean): string {
    const stripper = new StreamTagStripper(leading);
    stripper.push(text);
    stripper.flush();
    return stripper.released;
}

export interface TextPart {
    type: "text";
    text: string;
}

export function isTextPart(part: unknown): part is TextPart {
    return (
        part !== null &&
        typeof part === "object" &&
        (part as { type?: unknown }).type === "text" &&
        typeof (part as { text?: unknown }).text === "string"
    );
}

/** One streamed text part. The part's text so far is `raw`, and `raw[heldFrom..]` waits for more text. While `view` holds, the released text is `raw[runStart, runEnd)`, with removed text confined to either side. Releasing text after a removal that follows that slice clears `view`; subsequent changes append their newly released text to `released`. `deltas` strips the update deltas in event order. While `deltas` is `undefined`, every delta has matched the part's growth exactly and the part remains open, so the deltas release the same text as the part. */
interface PartStream {
    raw: string;
    heldFrom: number;
    state: ScanState;
    view: boolean;
    runStart: number;
    runEnd: number;
    released: string;
    deltas: DeltaStream | undefined;
}

function openPartStream(leading: boolean): PartStream {
    return {
        raw: "",
        heldFrom: 0,
        state: { leading, strippedOpeningTag: false },
        view: true,
        runStart: 0,
        runEnd: 0,
        released: "",
        deltas: undefined,
    };
}

/** Scans the part text from the held index and leaves the newly released ranges in `kept`. With `final`, the part has ended and its held text resolves. */
function advance(stream: PartStream, text: string, final: boolean): void {
    stream.raw = text;
    stream.heldFrom = scan(text, stream.heldFrom, final, stream.state);
    if (final) stream.state.leading = false;
    if (keptLength === 0) return;
    if (!stream.view) {
        stream.released += keptText(text);
        return;
    }
    let i = 0;
    for (; i < keptLength; i += 2) {
        const from = kept[i];
        if (from !== stream.runEnd) {
            if (stream.runEnd > stream.runStart) break;
            stream.runStart = from;
        }
        stream.runEnd = kept[i + 1];
    }
    let released = text.slice(stream.runStart, stream.runEnd);
    if (i < keptLength) {
        stream.view = false;
        for (; i < keptLength; i += 2) released += text.slice(kept[i], kept[i + 1]);
    }
    stream.released = released;
}

/** Releases the part's growth `piece` whole. The caller guarantees that nothing was held, the growth holds no section sign, the stream is past its leading text, and in view the growth continues the run. */
function extendRun(stream: PartStream, text: string, piece: string): void {
    stream.raw = text;
    stream.heldFrom = text.length;
    if (stream.view) {
        stream.runEnd = text.length;
        stream.released = text.slice(stream.runStart);
    } else stream.released += piece;
}

/** Stream continuity requires the provider to reuse each part object as its text grows. */
const streams = new WeakMap<object, PartStream>();

/** Content arrays already stripped as whole messages, so `message_end`, which Pi hands the same message object, strips them once. */
const wholeContents = new WeakSet<object>();

export function isStrippedWholeContent(content: unknown): boolean {
    return typeof content === "object" && content !== null && wholeContents.has(content);
}

type StreamUpdate = {
    type?: unknown;
    contentIndex?: unknown;
    delta?: unknown;
    content?: unknown;
    partial?: unknown;
};

/** The released delta of the update that the innermost running `stripStreamedTags` call strips. `streamPart` sets it for the targeted part, and each call restores the enclosing call's value before it returns or throws. */
let releasedDelta: string | undefined;

/** The delta stream copies the shared part state before the part's next change. */
function separateDeltas(stream: PartStream): DeltaStream {
    const deltas = { held: stream.raw.slice(stream.heldFrom), state: { ...stream.state } };
    stream.deltas = deltas;
    return deltas;
}

/** Advances one text part for an update and returns its released text. For the part the update targets, it also sets `releasedDelta` to the released delta. */
function streamPart(
    part: TextPart,
    index: number,
    leading: boolean,
    update: StreamUpdate | null,
): string {
    const text = part.text;
    let stream = streams.get(part);
    if (stream === undefined || text.slice(0, stream.raw.length) !== stream.raw) {
        stream = openPartStream(leading);
        streams.set(part, stream);
    }
    const piece =
        index === update?.contentIndex && typeof update.delta === "string"
            ? update.delta
            : undefined;
    const grew = text !== stream.raw;
    const ends = update?.type === "text_end" && index === update.contentIndex;
    let deltas = stream.deltas;
    if (deltas === undefined) {
        const before = stream.raw.length;
        if (
            !ends &&
            (grew
                ? piece !== undefined &&
                  text.length === before + piece.length &&
                  text.startsWith(piece, before)
                : piece === undefined || piece === "")
        ) {
            if (!grew) {
                if (piece !== undefined) releasedDelta = "";
            } else if (
                piece !== undefined &&
                stream.heldFrom === before &&
                (!stream.view || stream.runEnd === before) &&
                !stream.state.leading &&
                !piece.includes(SECTION)
            ) {
                extendRun(stream, text, piece);
                releasedDelta = piece;
            } else {
                advance(stream, text, false);
                releasedDelta = keptText(text);
            }
            return stream.released;
        }
        deltas = separateDeltas(stream);
    }
    if (grew) advance(stream, text, false);
    if (ends) advance(stream, text, true);
    if (piece !== undefined) releasedDelta = pushPiece(deltas, piece, false);
    return stream.released;
}

function snapshotContent(parts: unknown[], update: StreamUpdate | undefined): unknown[] {
    let firstText = true;
    return parts.map((part: unknown, index: number) => {
        if (!isTextPart(part)) return part;
        const leading = firstText;
        firstText = false;
        if (update === undefined) return { ...part, text: stripTags(part.text, leading) };
        const text = streamPart(part, index, leading, update);
        return { ...part, text };
    });
}

/** Rewrites one Pi `message_start` or `message_update` event in place so its listeners render assistant text with `\u00a7N\u00a7` tags removed. An event without an `assistantMessageEvent` carries a whole message, which is stripped as complete text. Every text part, the content array, and an object-valued `partial` are replaced by snapshots, because the provider keeps appending to its own parts while later handlers and listeners run. `text_delta` updates receive newly released text in `delta`, and `text_end` updates receive the released part text in `content`. A trailing tag prefix is released at `text_end` into `content` and the message, after the last `text_delta`. Each `text_delta` delta is stripped in event order, since Pi can hand listeners an update after the provider has appended later text to the part. While every delta has matched the part's growth exactly, the delta is the text the part released; from the first update that differs, the deltas pass through a stripper of their own. */
export function stripStreamedTags(event: {
    message: unknown;
    assistantMessageEvent?: unknown;
}): void {
    const message = event.message as { role?: unknown; content?: unknown } | null;
    if (message === null || typeof message !== "object") return;
    if (message.role !== "assistant" || !Array.isArray(message.content)) return;
    const update = event.assistantMessageEvent as StreamUpdate | undefined;
    const outer = releasedDelta;
    releasedDelta = undefined;
    let delta: string | undefined;
    let content: unknown[];
    try {
        content = snapshotContent(message.content, update);
        message.content = content;
        delta = releasedDelta;
    } finally {
        releasedDelta = outer;
    }
    if (update === undefined) {
        wholeContents.add(content);
        return;
    }
    if (update.type === "text_delta" && delta !== undefined) update.delta = delta;
    if (update.type === "text_end" && typeof update.contentIndex === "number") {
        const part = content[update.contentIndex];
        if (isTextPart(part)) update.content = part.text;
    }
    if (update.partial !== null && typeof update.partial === "object") {
        update.partial = { ...update.partial, content };
    }
}
