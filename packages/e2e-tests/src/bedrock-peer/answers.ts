export type Caller =
    | "history_summarizer"
    | "memory_classifier"
    | "memory_capture"
    | "context_researcher"
    | "conversation";

export interface ConverseRequest {
    system: string;
    /** `messages` contains every message's text in order, separated by newlines. */
    messages: string;
    lastUser: string;
}

export const CAPTURED_FACT = "The build listens on port 4242.";
export const RESEARCHER_ANSWER = "Project memory: the build listens on port 4242.";
export const CONVERSATION_ANSWER = "bedrock peer answer";

export function callerOf(request: ConverseRequest): Caller {
    // OpenCode titles every new session, a ModelExecution run's included, from its first prompt.
    if (request.system.includes("You are a title generator")) return "conversation";
    if (
        request.system.includes("Extract durable project memory") &&
        request.lastUser.includes('"existing_memories"')
    ) {
        return "memory_capture";
    }
    if (
        request.system.includes("You are a memory classifier") &&
        request.lastUser.includes("<pool>")
    ) {
        return "memory_classifier";
    }
    if (request.system.includes("You are ContextResearcher")) return "context_researcher";
    if (request.messages.includes("<new_messages>")) return "history_summarizer";
    return "conversation";
}

export function answer(caller: Caller, request: ConverseRequest): string {
    switch (caller) {
        case "history_summarizer":
            return scriptedSummary(request.messages) ?? CONVERSATION_ANSWER;
        case "memory_classifier":
            return classifyAnswer(request.messages);
        case "memory_capture":
            return captureAnswer(request.lastUser);
        case "context_researcher":
            return RESEARCHER_ANSWER;
        case "conversation":
            return CONVERSATION_ANSWER;
    }
}

const ALIAS_OPEN = "\u00ab";
const ALIAS_CLOSE = "\u00bb";
const SUMMARY_CHUNK = 5;

function presentedLine(line: string): [number, number, string] | undefined {
    const match = /^\[(\d+)(?:-(\d+))?\] [^:]*: (.*)$/.exec(line);
    if (!match) return undefined;
    const start = Number(match[1]);
    const end = match[2] === undefined ? start : Number(match[2]);
    const text = (match[3] as string)
        .split(/\s+/)
        .map((token) => {
            if (!token.startsWith(ALIAS_OPEN)) return token;
            const close = token.indexOf(ALIAS_CLOSE);
            return close < 0 ? token : token.slice(close + 1);
        })
        .filter((token) => token.length > 0)
        .join(" ");
    return [start, end, text];
}

const escapeXml = (text: string): string =>
    text.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;");

export function scriptedSummary(prompt: string): string | undefined {
    const open = prompt.indexOf("<new_messages>");
    const close = prompt.indexOf("</new_messages>", open);
    if (open < 0 || close < 0) return undefined;
    const lines: Array<[number, number, string]> = [];
    for (const line of prompt.slice(open + "<new_messages>".length, close).split("\n")) {
        const last = lines.at(-1);
        const presented = presentedLine(line);
        if (presented && (last === undefined || presented[0] === last[1] + 1)) {
            lines.push(presented);
        } else if (last && line.trim().length > 0) {
            last[2] += ` ${line.trim()}`;
        }
    }
    if (lines.length === 0) return undefined;
    let segments = "";
    for (let at = 0; at < lines.length; at += SUMMARY_CHUNK) {
        const group = lines.slice(at, at + SUMMARY_CHUNK);
        const start = (group[0] as [number, number, string])[0];
        const end = (group.at(-1) as [number, number, string])[1];
        const text = escapeXml(group.map(([, , text]) => text).join("; "));
        segments += `<history_segment start="${start}" end="${end}" title="messages ${start} to ${end}" episode_type="feature" importance="50"><p1>${text}</p1><p2>${text}</p2><p3>messages ${start} to ${end}</p3><p4 /></history_segment>`;
    }
    const next = (lines.at(-1) as [number, number, string])[1] + 1;
    return `<output><history_segments>${segments}</history_segments><meta><unprocessed_from>${next}</unprocessed_from></meta></output>`;
}

export function classifyAnswer(prompt: string): string {
    const ids = [...prompt.matchAll(/<memory id="([^"]*)"/g)].map((match) => match[1] as string);
    const entries = ids
        .map((id) => `<memory id="${id}" importance="60" scope="project" shareable="false"/>`)
        .join("");
    return `<classify>${entries}</classify>`;
}

interface CaptureSource {
    id: string;
    role: string;
    text: string;
}

export function captureAnswer(prompt: string): string {
    let sources: CaptureSource[] = [];
    try {
        const parsed = JSON.parse(prompt.slice(prompt.indexOf("{"))) as { messages?: unknown };
        if (Array.isArray(parsed.messages)) sources = parsed.messages as CaptureSource[];
    } catch {
        sources = [];
    }
    return JSON.stringify({
        version: 1,
        decisions: sources.map((source) => ({
            message_id: source.id,
            memories:
                source.role === "user" && source.text.includes(CAPTURED_FACT)
                    ? [{ category: "CONFIG_VALUES", content: CAPTURED_FACT, quote: CAPTURED_FACT }]
                    : [],
        })),
    });
}
