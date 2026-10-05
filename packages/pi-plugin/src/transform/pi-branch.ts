/**
 * A per-session index of the entries on Pi's current branch, root first, kept by walking
 * `parentId` links from the leaf back to the last entry already indexed. Growth of the branch
 * visits only the new entries; a navigation away from the indexed leaf visits the entries from
 * the new leaf back to the branch point. With an empty index, the walk follows `parentId` links
 * until it reaches the root or an unavailable entry.
 */

import type { SessionEntry } from "@earendil-works/pi-coding-agent";

export interface PiBranchReader {
    getLeafId(): string | null | undefined;
    getEntry(id: string): SessionEntry | undefined;
}

export class PiBranchIndex {
    private ids: string[] = [];
    private readonly positions = new Map<string, number>();
    /** Indexes of compaction entries in `ids`, ascending. */
    private compactions: number[] = [];

    /** Returns the count of entries the walk read. */
    sync(reader: PiBranchReader): number {
        const leaf = reader.getLeafId() ?? undefined;
        if (leaf !== undefined && this.ids.at(-1) === leaf) return 0;
        const fresh: SessionEntry[] = [];
        let cursor = leaf;
        while (cursor !== undefined && !this.positions.has(cursor)) {
            const entry = reader.getEntry(cursor);
            if (!entry) break;
            fresh.push(entry);
            cursor = entry.parentId ?? undefined;
        }
        const meet = cursor === undefined ? -1 : (this.positions.get(cursor) ?? -1);
        this.truncate(meet + 1);
        for (let index = fresh.length - 1; index >= 0; index -= 1) {
            const entry = fresh[index] as SessionEntry;
            this.positions.set(entry.id, this.ids.length);
            if (entry.type === "compaction") this.compactions.push(this.ids.length);
            this.ids.push(entry.id);
        }
        return fresh.length;
    }

    get length(): number {
        return this.ids.length;
    }

    idAt(index: number): string | undefined {
        return this.ids[index];
    }

    indexOf(id: string): number | undefined {
        return this.positions.get(id);
    }

    latestCompaction(): number | undefined {
        return this.compactions.at(-1);
    }

    private truncate(length: number): void {
        for (let index = length; index < this.ids.length; index += 1)
            this.positions.delete(this.ids[index] as string);
        this.ids.length = length;
        while ((this.compactions.at(-1) ?? -1) >= length) this.compactions.pop();
    }
}
