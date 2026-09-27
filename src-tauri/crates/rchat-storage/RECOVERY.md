# Zova storage and recovery

RChat pins Zova 1.1.0 (format 11). SQL remains SQLite-compatible, but all runtime
SQL and attachment bytes use Zova; no rusqlite/native SQLite handles are shared.
The cryptography-only RVault dependency has a documented local feature patch to
avoid linking its unused bundled SQLite implementation.

## First launch

Quit every old GUI/TUI process before upgrading. Back up the entire RChat data
directory, including `databases/rchat.sqlite`, any `-wal`/`-shm` sidecars, and
`chunks/`. Never copy only an active SQLite main file and discard its WAL.

Default GUI/TUI storage stays in its historical platform ProjectDirs data path.
Explicitly isolated TUI profiles use their own storage root. Startup reserves
`databases/rchat-migration.lock`, converts SQLite into `rchat.importing.zova`,
imports media, validates SQL integrity/foreign keys, writes a cutover marker,
checkpoints the WAL, closes and syncs the file, then renames it to `rchat.zova`.
The converter snapshots committed WAL rows. It cannot prevent an old binary
from writing after that snapshot: closing old processes is mandatory.

Legacy files are never deleted automatically. Allow free space for both database
copies, imported media, and migration WAL. Missing/corrupt chunks remain incomplete
and retryable. Valid partial chunks and original transport manifests are retained.
SQL and Zova object mutations cannot share a transaction in 1.1.0: bytes are
verified/staged first and SQL references committed afterward. A failed metadata
transaction may leave deduplicated unreferenced bytes; it cannot publish a dangling
complete attachment. Garbage collection is not introduced by this change.

## Failed or interrupted migration

Do not rename SQLite to `.zova`, edit Zova private tables, or delete the legacy
source. Stop all RChat processes and save the whole directory before recovery.

If there is **no final `rchat.zova`**, the original SQLite/chunks remain the source
of truth. After resolving the reported disk/permission/data problem, move the
staging database **and its WAL/SHM sidecars** to a backup directory and remove the
stale migration lock only after confirming no process owns it. Relaunch to restart
the copy-forward import. A failed staging file is never automatically selected.

If **`rchat.zova` exists**, startup requires its RChat cutover marker and supported
version. An unmarked/unsupported destination is an error, not permission to select
the older SQLite file. Preserve all files and investigate. Once new writes have
reached Zova, the legacy SQLite copy is a historical backup, **not a lossless
rollback**. Restore a verified whole Zova backup with all applications closed;
never merge databases by file copying. The storage API's `backup_to` creates and
verifies a Zova snapshot containing SQL and object bytes.

## Build and verification

Zova's published Rust packages compile generated C with Clang and the platform
SDK/linker. Zig and system SQLite development headers are not needed. `sqlite3`
CLI is a test-only prerequisite for independent legacy/WAL fixtures (included by
Linux dependency installers, supplied by macOS). Doctor checks it explicitly.

`cargo nextest run --manifest-path src-tauri/Cargo.toml -p rchat-storage` covers
bindings, panic/rollback, live WAL conversion, failed cutover, and verified object
backup. Core tests cover existing groups/archives and media import/transfer.
CI includes Linux tests, macOS storage tests and a Windows storage-only job;
the latter does not assert full Windows GUI/TUI support. No performance improvement
is claimed by this migration. Real multi-peer and clean-machine rollout checks
remain separate from these deterministic tests.
