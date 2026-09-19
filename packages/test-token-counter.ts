import Tokenizer from "ai-tokenizer";
import * as claudeEncoding from "ai-tokenizer/encoding/claude";

const tokenizer = new Tokenizer(claudeEncoding);

export function deterministicTestTokenCount(text: string): number {
    return tokenizer.encode(text, "all").length;
}
