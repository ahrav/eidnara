# original-budget-stops-sql

- `crates/daemon/tests/query_route.rs:74` cancels or sleeps past the deadline
  at each route phase, including `Phase::Lexical`, and asserts the request ends
  at that phase with `Cancelled` or `Deadline`.
- `crates/retrieval/tests/lexical_retrieval.rs:1376` and `:2018` interrupt the
  engine during counting, ranking, and a common scan.
