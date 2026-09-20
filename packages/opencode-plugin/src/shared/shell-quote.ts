/** POSIX single-quoting: an embedded `'` closes, escapes, and reopens the quote. */
export function shellQuote(value: string): string {
    return `'${value.replaceAll("'", "'\\''")}'`;
}
