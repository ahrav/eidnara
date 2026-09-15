//! Every probe is one quoted atom bound as an SQL value, so user text can only ever be a phrase: `OR`, `NEAR`, `:`, `*`, `^`, and `-` inside it are never FTS operators.
//! FTS5 escapes quotes inside quoted strings by doubling them.
//!
//! Duplicate atoms stay duplicate probes because ranking, not compilation, reduces each occurrence to its best probe; permuting or repeating probes must leave that ranking unchanged.
//! Probes compile from atoms and never from parts; parts widen what the index matches, not what a request asks for.

use rusqlite::ToSql;
use rusqlite::types::ToSqlOutput;

use super::Analysis;

/// One quoted atom to bind as the right-hand side of `MATCH ?`; the text is reachable only through [`ToSql`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Probe(String);

impl ToSql for Probe {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        self.0.to_sql()
    }
}

/// One probe per atom in request order; an empty analysis yields no probe, so nothing can be bound for it.
pub fn compile(analysis: &Analysis) -> Vec<Probe> {
    analysis
        .atoms()
        .map(|atom| Probe(quote_atom(atom)))
        .collect()
}

fn quote_atom(atom: &str) -> String {
    format!("\"{}\"", atom.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::quote_atom;

    #[test]
    fn quoting_doubles_internal_quotes() {
        assert_eq!(quote_atom("a\"b"), "\"a\"\"b\"");
        assert_eq!(quote_atom("\""), "\"\"\"\"");
        assert_eq!(quote_atom(""), "\"\"");
        assert_eq!(quote_atom("OR"), "\"OR\"");
    }
}
