/**
 * Opening lines of OpenCode's built-in prompts (title, summary, and compaction
 * agents); a system prompt that starts with one belongs to OpenCode, not to a
 * user agent.
 */
export const INTERNAL_OPENCODE_AGENT_SIGNATURES: readonly string[] = [
    "You are a title generator. You output ONLY a thread title.",
    "Summarize what was done in this conversation. Write like a pull request description.",
    "You are an anchored context summarization assistant for coding sessions.",
];

/** Opening lines of this plugin's hidden-agent prompts and of the daemon's compaction prompt. */
export const EIDNARA_INTERNAL_AGENT_SIGNATURES: readonly string[] = [
    "You are Historian — the hippocampus of a long-running coding agent.",
    // SMART_NOTE_COMPILER_SYSTEM_PROMPT
    "You are the Eidnara smart-note compiler for the memory system.",
    // SIDEKICK_SYSTEM_PROMPT
    "You are Sidekick, a focused memory-retrieval subagent for an AI coding assistant.",
];
