# `live-lease-file-replacement-is-reached`

- **Discovery:** coverage/vacuity evaluation of inode stability.
- **Primary evidence:** documented race at `lease-store-density.md:22-24`; both acquisition methods retain descriptor-bound locks after `open_lease_file` returns.
- **Coverage condition:** holder remains live on inode A while the pathname resolves to distinct inode B.
- **Why independent:** the state can occur without a second acquisition succeeding; it is the vulnerable precondition, not the safety violation.
- **Timing need:** replace after holder open and before drop.
- **Instrumentation:** holder inode and path inode identities are not exposed.
- **Open questions:** production actor capable of replacement is unknown.
