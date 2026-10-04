import {
    type FauxResponseStep,
    fauxAssistantMessage,
    registerFauxProvider,
} from "@earendil-works/pi-ai/compat";
import {
    type AgentSession,
    AuthStorage,
    createAgentSession,
    DefaultResourceLoader,
    type ExtensionFactory,
    ModelRegistry,
    SessionManager,
    SettingsManager,
} from "@earendil-works/pi-coding-agent";

export interface TestAgentSession {
    session: AgentSession;
    authStorage: AuthStorage;
    sessionManager: SessionManager;
    requests: { messages: unknown[] }[];
    respond(steps: FauxResponseStep[]): void;
    dispose(): Promise<void>;
}

export function openSessionFile(path: string, cwd: string): SessionManager {
    return SessionManager.open(path, undefined, cwd);
}

export async function createTestAgentSession(args: {
    cwd: string;
    extensionFactories: ExtensionFactory[];
    sessionManager?: SessionManager;
    contextWindow?: number;
    settings?: Record<string, unknown>;
    /** Tool names to enable; none when unset. */
    tools?: string[];
}): Promise<TestAgentSession> {
    const faux = registerFauxProvider({
        provider: "faux",
        models: [
            { id: "faux-model", contextWindow: args.contextWindow ?? 200_000, maxTokens: 8_192 },
        ],
    });
    const requests: { messages: unknown[] }[] = [];
    const script: FauxResponseStep[] = [];
    const answer: FauxResponseStep = (context) => {
        requests.push({ messages: structuredClone(context.messages) });
        const next = script.shift();
        if (typeof next === "function")
            return next(context, undefined, faux.state, faux.getModel());
        return next ?? fauxAssistantMessage("ok");
    };
    faux.setResponses(Array.from({ length: 256 }, () => answer));
    const settingsManager = SettingsManager.inMemory({
        compaction: { enabled: false },
        ...args.settings,
    });
    const authStorage = AuthStorage.inMemory();
    authStorage.setRuntimeApiKey("faux", "test-key");
    const sessionManager = args.sessionManager ?? SessionManager.inMemory(args.cwd);
    const resourceLoader = new DefaultResourceLoader({
        cwd: args.cwd,
        agentDir: args.cwd,
        settingsManager,
        extensionFactories: args.extensionFactories,
        noSkills: true,
        noPromptTemplates: true,
        noThemes: true,
        noContextFiles: true,
    });
    await resourceLoader.reload();
    const { session } = await createAgentSession({
        cwd: args.cwd,
        agentDir: args.cwd,
        authStorage,
        modelRegistry: ModelRegistry.inMemory(authStorage),
        model: faux.getModel(),
        settingsManager,
        sessionManager,
        resourceLoader,
        ...(args.tools ? { tools: args.tools } : { noTools: "all" as const }),
    });
    await session.bindExtensions({});
    return {
        session,
        authStorage,
        sessionManager,
        requests,
        respond: (steps) => script.push(...steps),
        async dispose() {
            await session.extensionRunner?.emit({ type: "session_shutdown", reason: "quit" });
            session.dispose();
            faux.unregister();
        },
    };
}
