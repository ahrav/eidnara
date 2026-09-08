export interface DumpMessageInfo {
    id?: string;
    /** The `message.time_created` column, which orders the session together with `id`. */
    timeCreated?: number;
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
