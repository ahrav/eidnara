/**
 * A session filter value no real session id can equal, so every
 * session-tagged record is dropped and only untagged records remain. Used when
 * session discovery failed and the user declined to include every session.
 */
export const EXCLUDE_SESSION_RECORDS = "__exclude-session-records__";

const RECORD_START_PATTERN = /^\[\d{4}-\d{2}-\d{2}T[^\]]+\] /;

/** Lines before the first timestamped line are the remainder of a record cut by a bounded tail read and are dropped. */
export function filterLogRecords(
    lines: string[],
    keepRecord: (firstLine: string) => boolean,
): string[] {
    let keep = false;
    return lines.filter((line) => {
        if (RECORD_START_PATTERN.test(line)) keep = keepRecord(line);
        return keep;
    });
}
