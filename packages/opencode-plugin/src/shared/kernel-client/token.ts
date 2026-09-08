import type { MutationToken, ReadRow } from "./wire";

/** The token operations `KernelClient` performs. `TokenCache` holds them directly; a caller may hand the client a view whose methods resolve to whichever cache currently belongs with the client's transport. Writes and reads carry the transport's `connectionIdentity` so a store can refuse to hand a body tokens minted under a connection the transport no longer represents. commentlint: allow(JUDGE) */
export interface TokenStore {
    remember(
        projectRoot: string,
        rows: readonly ReadRow[],
        knownAsOf: number,
        connectionIdentity?: string,
    ): void;
    rememberTokens(
        projectRoot: string,
        tokens: readonly MutationToken[],
        knownAsOf: number,
        connectionIdentity?: string,
    ): void;
    get(
        projectRoot: string,
        objectId: string,
        connectionIdentity?: string,
    ): MutationToken | undefined;
    knownAsOfFor(projectRoot: string): number | undefined;
    dropProject(projectRoot: string): void;
    size(projectRoot: string): number;
}

/**
 * Mutation tokens keyed by `(project_root, object_id)`. A token is the
 * `known_as_of` the object was last read at; `kernel.commit` rejects a token
 * once any change event for that object lands past it, so the cache holds the
 * newest value seen per object and never invents one. A project's tokens are
 * also bound to the connection identity they were written under: a write or
 * read that names a different identity first drops the project, because a
 * position in one daemon's sequence means nothing to its successor.
 */
export class TokenCache implements TokenStore {
    private readonly tokens = new Map<string, Map<string, number>>();
    private readonly knownAsOf = new Map<string, number>();
    private readonly identity = new Map<string, string>();

    private bucket(projectRoot: string): Map<string, number> {
        let bucket = this.tokens.get(projectRoot);
        if (!bucket) {
            bucket = new Map();
            this.tokens.set(projectRoot, bucket);
        }
        return bucket;
    }

    /** Drops the project when `connectionIdentity` names a connection other than the one its tokens came from; an access without an identity leaves the binding alone. commentlint: allow(JUDGE) */
    private fence(projectRoot: string, connectionIdentity: string | undefined): void {
        if (connectionIdentity === undefined) return;
        const bound = this.identity.get(projectRoot);
        if (bound !== undefined && bound !== connectionIdentity) this.dropProject(projectRoot);
        this.identity.set(projectRoot, connectionIdentity);
    }

    /** Mints a token per row and advances the project's `known_as_of`. */
    remember(
        projectRoot: string,
        rows: readonly ReadRow[],
        knownAsOf: number,
        connectionIdentity?: string,
    ): void {
        this.fence(projectRoot, connectionIdentity);
        const bucket = this.bucket(projectRoot);
        for (const row of rows) this.rememberToken(bucket, row.token);
        this.advance(projectRoot, knownAsOf);
    }

    /** Commit receipts hand back tokens at the commit's sequence. */
    rememberTokens(
        projectRoot: string,
        tokens: readonly MutationToken[],
        knownAsOf: number,
        connectionIdentity?: string,
    ): void {
        this.fence(projectRoot, connectionIdentity);
        const bucket = this.bucket(projectRoot);
        for (const token of tokens) this.rememberToken(bucket, token);
        this.advance(projectRoot, knownAsOf);
    }

    private rememberToken(bucket: Map<string, number>, token: MutationToken): void {
        const existing = bucket.get(token.object_id);
        if (existing === undefined || token.known_as_of > existing) {
            bucket.set(token.object_id, token.known_as_of);
        }
    }

    private advance(projectRoot: string, knownAsOf: number): void {
        const existing = this.knownAsOf.get(projectRoot);
        if (existing === undefined || knownAsOf > existing) {
            this.knownAsOf.set(projectRoot, knownAsOf);
        }
    }

    get(
        projectRoot: string,
        objectId: string,
        connectionIdentity?: string,
    ): MutationToken | undefined {
        this.fence(projectRoot, connectionIdentity);
        const knownAsOf = this.tokens.get(projectRoot)?.get(objectId);
        return knownAsOf === undefined
            ? undefined
            : { object_id: objectId, known_as_of: knownAsOf };
    }

    /** The newest snapshot position read for the project, if any. */
    knownAsOfFor(projectRoot: string): number | undefined {
        return this.knownAsOf.get(projectRoot);
    }

    dropProject(projectRoot: string): void {
        this.tokens.delete(projectRoot);
        this.knownAsOf.delete(projectRoot);
        this.identity.delete(projectRoot);
    }

    size(projectRoot: string): number {
        return this.tokens.get(projectRoot)?.size ?? 0;
    }
}
