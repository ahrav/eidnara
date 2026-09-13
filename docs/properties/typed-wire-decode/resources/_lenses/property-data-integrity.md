# Property lens: data integrity

HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.
Scope: [source register](../source-register.md).

Payload Values remain data, even when envelope Values disappear. Recursive
capacity accounting is visible at
`crates/daemon/src/retained_size.rs:96-134,157-232`. Canonical text is a
separate allocation at `crates/daemon/src/wire.rs:269-270,731-736`.
Removing a duplicated envelope term must not remove these terms.

Candidate: `retained-accounting-follows-typed-ownership`. Check structural
ownership independently of the estimator, with populated payload variants.
Byte/hash equality itself belongs to the sibling identity work; this part
keeps it as a prerequisite for calling the measured operation equivalent.
