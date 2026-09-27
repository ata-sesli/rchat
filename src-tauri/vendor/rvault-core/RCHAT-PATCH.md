# RVault dependency patch

Copied from ata-sesli/rvault revision
`7dda2de45ac31eccb17dad7d1a4f1ebdd893e99c`, crates/rvault-core (MIT OR Apache-2.0).

The sole functional patch makes the SQLite-backed storage module/vault field and
rusqlite dependency optional behind a default-enabled `storage` feature. RChat
disables that feature: it uses the unchanged cryptography, keystore and session
implementation, not RVault's database. This prevents two SQLite implementations
from being linked together with Zova. Remove this copy when an upstream revision
provides the same feature split.
