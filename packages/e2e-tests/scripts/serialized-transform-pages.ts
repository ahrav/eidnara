import { createHash } from "node:crypto";
import {
    SERIALIZED_TRANSFORM_SESSION,
    serializedTransformCorpus,
} from "@eidnara/opencode/hooks/context/__tests__/serialized-transform-corpus";
import { buildPagedModuleTransformPayloads } from "@eidnara/opencode/hooks/context/module-wire";
import { serializedJsonText } from "@eidnara/opencode/shared/host-client/serialized-json-body";

const cases = serializedTransformCorpus().map((fixture) => {
    const originalText = JSON.stringify(fixture.body);
    try {
        const pages = buildPagedModuleTransformPayloads(fixture.body).map(({ page, bytes }) => {
            const text = serializedJsonText(page);
            return { text, bytes, sha256: createHash("sha256").update(text, "utf8").digest("hex") };
        });
        if (fixture.pagerRefuses) throw new Error(`${fixture.name}: pager must refuse`);
        return {
            name: fixture.name,
            originalText,
            hostRefuses: fixture.hostRefuses ?? false,
            pageCount: fixture.pageCount,
            firstPageBytes: fixture.firstPageBytes,
            lastPageBytes: fixture.lastPageBytes,
            pages,
        };
    } catch (error) {
        if (
            !fixture.pagerRefuses ||
            !(error instanceof Error) ||
            error.message !== "module transform scalar tail exceeds the 512 KiB page limit"
        ) {
            throw error;
        }
        return { name: fixture.name, originalText, pagerRefused: true, pages: [] };
    }
});
console.log(JSON.stringify({ session: SERIALIZED_TRANSFORM_SESSION, cases }));
