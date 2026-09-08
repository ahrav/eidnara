export interface DumpMessageInfo {
    id?: string;
    role?: string;
    sessionID?: string;
    error?: MsgError;
    [key: string]: unknown;
}

export interface MsgError {
    name: string;
    data: unknown[];
}
export interface DumpMessage {
    info: DumpMessageInfo;
    parts: unknown[];
}
