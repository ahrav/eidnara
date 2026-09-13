# System lens: wildcard

HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.
Scope: [source register](../source-register.md). This pass follows all eleven
other system-model passes.

The byte cap has two paths. Bodies at most 1 MiB bypass the cap probe, but
larger bodies call `probe_request` before the meter exists
(`crates/daemon/src/lib.rs:12153-12166,16144-16161`). `ProbeKey` asks serde to
deserialize a string at `:15837-15860`. Not retaining the key is different
from avoiding unescape scratch. The sub-cap allocation test does not settle
the larger-body window. Preserve that as a hidden-peak investigation lead,
not a reproduced budget violation.

Canonical serialization creates span metadata and a second output buffer at
`crates/daemon/src/served_json.rs:121-144`. Removing envelope trees does not
make projection allocation-free. Its transient workspace needs attribution.

Unique additions: include above-facade-cap escaped keys in the complete
resource interval; distinguish canonical output bytes from serializer scratch.
