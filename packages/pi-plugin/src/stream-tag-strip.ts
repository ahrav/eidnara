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

/** StreamTagStripper removes `§N§` tags containing 1 to 19 ASCII digits, and the mangled `§N">` form with an optional `§` or `§N§` after it, across delta boundaries. It holds a trailing prefix of either form until another delta resolves it or `flush()` ends the text, and passes every other `§` through, so `§ 4` and `§42 of the Act` stream unchanged. With `leading` enabled, opening tags consume their surrounding whitespace. Released text only grows. */
export class StreamTagStripper {
    #released = "";
    #held = "";
    #leading: boolean;
    #strippedOpeningTag = false;

    constructor(leading: boolean) {
        this.#leading = leading;
    }

    get released(): string {
        return this.#released;
    }

    push(delta: string): string {
        const input = this.#held + delta;
        this.#held = "";
        return this.#release(this.#scan(input, false));
    }

    flush(): string {
        const input = this.#held;
        this.#held = "";
        const out = this.#scan(input, true);
        this.#leading = false;
        return this.#release(out);
    }

    #release(out: string): string {
        this.#released += out;
        return out;
    }

    #scan(input: string, final: boolean): string {
        let out = "";
        let index = 0;
        while (index < input.length) {
            if (this.#leading) {
                const start = index;
                while (isWhitespace(input[index])) index += 1;
                if (index === input.length) {
                    if (!final) this.#held = input.slice(start);
                    else if (!this.#strippedOpeningTag) out += input.slice(start);
                    break;
                }
                const end = tagEnd(input, index, final);
                if (end === "open") {
                    this.#held = input.slice(start);
                    break;
                }
                if (end !== undefined) {
                    this.#strippedOpeningTag = true;
                    index = end;
                    continue;
                }
                this.#leading = false;
                if (!this.#strippedOpeningTag) out += input.slice(start, index);
                continue;
            }
            const section = input.indexOf(SECTION, index);
            if (section === -1) {
                out += input.slice(index);
                break;
            }
            out += input.slice(index, section);
            const end = tagEnd(input, section, final);
            if (end === "open") {
                this.#held = input.slice(section);
                break;
            }
            if (end === undefined) {
                out += SECTION;
                index = section + 1;
            } else {
                index = end;
            }
        }
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

interface PartStream {
    raw: string;
    stripper: StreamTagStripper;
    deltas: StreamTagStripper;
}

/** Stream continuity requires the provider to reuse each part object as its text grows. */
const streams = new WeakMap<object, PartStream>();

/** Content arrays already stripped as whole messages, so `message_end`, which Pi hands the same message object, strips them once. */
const wholeContents = new WeakSet<object>();

export function isStrippedWholeContent(content: unknown): boolean {
    return typeof content === "object" && content !== null && wholeContents.has(content);
}

/** Rewrites one Pi `message_start` or `message_update` event in place so its listeners render assistant text with `\u00a7N\u00a7` tags removed. An event without an `assistantMessageEvent` carries a whole message, which is stripped as complete text. Every text part, the content array, and an object-valued `partial` are replaced by snapshots, because the provider keeps appending to its own parts while later handlers and listeners run. `text_delta` updates receive newly released text in `delta`, and `text_end` updates receive the released part text in `content`. A trailing tag prefix is released at `text_end` into `content` and the message, after the last `text_delta`. Each `text_delta` delta passes through a stripper of its own in event order, since Pi can hand listeners an update after the provider has appended later text to the part. */
export function stripStreamedTags(event: {
    message: unknown;
    assistantMessageEvent?: unknown;
}): void {
    const message = event.message as { role?: unknown; content?: unknown } | null;
    if (message === null || typeof message !== "object") return;
    if (message.role !== "assistant" || !Array.isArray(message.content)) return;
    const update = event.assistantMessageEvent as
        | {
              type?: unknown;
              contentIndex?: unknown;
              delta?: unknown;
              content?: unknown;
              partial?: unknown;
          }
        | undefined;
    const ended = update?.type === "text_end" ? update.contentIndex : undefined;
    let firstText = true;
    let delta: string | undefined;
    const content = message.content.map((part: unknown, index: number) => {
        if (!isTextPart(part)) return part;
        const leading = firstText;
        firstText = false;
        if (update === undefined) return { ...part, text: stripTags(part.text, leading) };
        let stream = streams.get(part);
        if (stream === undefined || !part.text.startsWith(stream.raw)) {
            stream = {
                raw: "",
                stripper: new StreamTagStripper(leading),
                deltas: new StreamTagStripper(leading),
            };
            streams.set(part, stream);
        }
        stream.stripper.push(part.text.slice(stream.raw.length));
        stream.raw = part.text;
        if (index === ended) stream.stripper.flush();
        if (index === update?.contentIndex && typeof update.delta === "string") {
            delta = stream.deltas.push(update.delta);
        }
        return { ...part, text: stream.stripper.released };
    });
    message.content = content;
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
