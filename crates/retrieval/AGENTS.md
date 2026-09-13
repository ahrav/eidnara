# Retrieval

The search projection uses schema version 4. Rebuild incompatible databases;
do not migrate them. Keep `baseline.sql`, `SCHEMA_VERSION`, and
`docs/properties/search-projection/projection-schema.md` consistent.

Keep mutations pure. The daemon owns physical cleanup and must release the
receipt transaction before acknowledging the old kernel consumer.
