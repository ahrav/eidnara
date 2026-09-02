-- Objects every Eidnara module store carries ahead of its own baseline.
--
-- `fence` holds the single writer-epoch row that every fenced write and every
-- open checks. `format_marker` holds the SHA-256 of the complete baseline text
-- (this file followed by the consumer's baseline), so a file's identity is
-- readable without re-deriving the schema.
CREATE TABLE fence (
    id INTEGER PRIMARY KEY CHECK (id = 0),
    epoch INTEGER NOT NULL CHECK (epoch >= 0)
);
CREATE TABLE format_marker (
    id INTEGER PRIMARY KEY CHECK (id = 0),
    baseline_sha256 TEXT NOT NULL CHECK (length(baseline_sha256) = 64)
);
