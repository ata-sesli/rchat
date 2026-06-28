# RChat Complete Zova Migration Checklist

> Branch: `codex/zova-migration-checklist`
>
> Latest Zova checked: `zova = "0.17.0"` and `zova-sys = "0.17.0"`
>
> Checked on: 2026-06-27
>
> Verification used: `cargo search zova --limit 5`, `cargo info zova`, `cargo info zova-sys`, and docs.rs crate page.
>
> Source links: [zova 0.17.0 on crates.io](https://crates.io/crates/zova/0.17.0), [zova-sys 0.17.0 on crates.io](https://crates.io/crates/zova-sys/0.17.0), [zova 0.17.0 on docs.rs](https://docs.rs/crate/zova/0.17.0)

## Goal

Move RChat's local persistence fully onto Zova. Records, media objects, chunk transfer state, stickers, envelopes, maintenance, backup, restore, compacting, and future vector search should use one `.zova` database boundary instead of RChat directly owning `rusqlite::Connection`, `rchat.sqlite`, and an external `chunks/` directory.

## Migration Rule

- [ ] Do the implementation on a separate branch.
- [ ] Use `codex/zova-migration-checklist` for this checklist branch.
- [ ] Create a new implementation branch before code migration work, for example `codex/zova-storage-migration`.
- [ ] Do not mix the migration with unrelated frontend, voice, networking, or UI changes.
- [ ] Keep each storage phase reviewable and independently testable.
- [ ] Keep old `rchat.sqlite` and `chunks/` as rollback inputs during the first Zova-backed release.

## Zova 0.17.0 Intake

- [ ] Add the published crate dependency to `src-tauri/Cargo.toml`:
  - `zova = "0.17.0"`
- [ ] Do not depend on `zova-sys` directly unless RChat needs raw C ABI access.
- [ ] Confirm `cargo info zova` remains at `0.17.0` before implementation starts.
- [ ] Confirm `cargo info zova-sys` remains at `0.17.0` before implementation starts.
- [ ] Record Zova crate metadata in the migration PR:
  - crate: `zova`
  - version: `0.17.0`
  - license: `MIT`
  - Rust version: `1.79`
  - documentation: `https://docs.rs/zova`
  - repository: `https://github.com/atasesli/zova`
- [ ] Document native build requirements:
  - Rust toolchain compatible with Rust `1.79+`
  - Zig `0.16.0+`
  - working C compiler/linker
- [ ] Verify RChat release packaging includes what `zova-sys` needs to build Zova's native C ABI.
- [ ] Update `src-tauri/Cargo.lock`.
- [ ] Run `cargo check --manifest-path src-tauri/Cargo.toml` immediately after adding the crate.
- [ ] Decide whether release CI should install Zig explicitly or rely on an existing toolchain image.

## Current RChat Storage Inventory

- [ ] Inventory all direct `rusqlite` imports and call sites before editing.
- [ ] Inventory all filesystem chunk reads/writes before editing.
- [ ] Inventory all places that assume `rchat.sqlite` is the database file.
- [ ] Inventory all places that assume a top-level app data `chunks/` directory exists.
- [ ] Capture the current schema from `src-tauri/src/storage/db.rs`.
- [ ] Capture current app-data paths in `src-tauri/src/storage/db.rs`, `src-tauri/src/storage/object.rs`, and startup code.
- [ ] Capture current storage entry points used by commands:
  - chat commands
  - media commands
  - envelope commands
  - auth/setup commands
  - invite commands
  - peer profile commands
  - network persistence
  - chunk transfer logic
  - theme storage
- [ ] Confirm whether theme storage should move into Zova or remain vault/config-backed.

## Target Storage Architecture

- [ ] Create one Zova-backed storage boundary, for example `src-tauri/src/storage/zova_store.rs`.
- [ ] Introduce an `RChatStore` type that owns or wraps `zova::SharedDatabase`.
- [ ] Replace `AppState.db_conn: Mutex<rusqlite::Connection>` with an app-level store handle.
- [ ] Keep command code from preparing raw Zova statements directly.
- [ ] Route persistence through behavior-named repository methods, for example:
  - `create_peer`
  - `list_chats`
  - `insert_message`
  - `mark_message_delivered`
  - `store_media_object`
  - `load_media_object`
  - `list_object_manifest`
  - `put_received_chunk`
  - `assemble_received_object`
  - `assign_chat_to_envelope`
  - `record_connection_stat`
- [ ] Use `zova::SharedDatabase` for normal Tauri command access.
- [ ] Use `SharedDatabase::transaction` or `transaction_immediate` for multi-step writes.
- [ ] Use `SharedDatabase::with_exclusive` for multi-call units that must not interleave.
- [ ] Set a busy timeout intentionally; match the old SQLite `5000 ms` unless a test proves a better value.
- [ ] Keep frontend command names and payload shapes unchanged during the first migration pass.

## Zova File Layout

- [ ] Store the primary app database as `databases/rchat.zova`.
- [ ] Keep old `databases/rchat.sqlite` untouched during migration.
- [ ] Keep old `chunks/` untouched during migration.
- [ ] Use Zova user SQL tables for RChat records.
- [ ] Use Zova objects for media bytes and transferred file content.
- [ ] Store Zova object IDs as raw 32-byte BLOBs in new user tables where possible.
- [ ] Preserve hex-string compatibility where frontend/backend APIs expose `file_hash`.
- [ ] Add helpers for:
  - hex string to `zova::ObjectId`
  - `zova::ObjectId` to hex string
  - SHA-256 verification against legacy `file_hash`
- [ ] Do not query or mutate private `_zova_*` tables from RChat.
- [ ] Add an app-owned metadata table for RChat storage schema versioning.

## Schema Design

- [ ] Create a Zova schema initializer for RChat user tables.
- [ ] Keep RChat schema version separate from Zova's private format version.
- [ ] Add `app_meta` or equivalent:
  - `schema_version`
  - `created_at`
  - `last_migrated_from`
  - `last_successful_backup_at`
- [ ] Recreate current RChat tables as user SQL tables:
  - `peers`
  - `chats`
  - `chat_peers`
  - `messages`
  - `files`
  - `stickers`
  - `envelopes`
  - `chat_envelopes`
  - `chat_connection_stats`
- [ ] Replace or adapt `file_chunks`:
  - remove as authoritative chunk storage
  - keep only if needed as compatibility metadata
  - prefer Zova object manifests for real chunk data
- [ ] Add transfer state tables if restart-resumable transfers are desired:
  - `transfers`
  - `transfer_chunks`
- [ ] Recreate all existing indexes.
- [ ] Recreate foreign key behavior where it still matches app semantics.
- [ ] Test that no RChat-owned table starts with `_zova_`.
- [ ] Test fresh database initialization.
- [ ] Test opening an existing initialized `.zova` file.

## Record Repository Port

- [ ] Port peer CRUD from `storage::db` into `RChatStore`.
- [ ] Port chat CRUD into `RChatStore`.
- [ ] Port chat membership operations into `RChatStore`.
- [ ] Port message insert/query/update/read-receipt operations into `RChatStore`.
- [ ] Port unread count queries into `RChatStore`.
- [ ] Port latest-message-time queries into `RChatStore`.
- [ ] Port file metadata queries into `RChatStore`.
- [ ] Port sticker registry operations into `RChatStore`.
- [ ] Port envelope CRUD into `RChatStore`.
- [ ] Port chat-envelope assignment operations into `RChatStore`.
- [ ] Port connection stats into `RChatStore`.
- [ ] Port any remaining command-level SQL into repository methods.
- [ ] Keep `self -> Me` and scoped chat ID behavior unchanged.
- [ ] Keep existing serde payloads unchanged.

## Media Object Storage

- [ ] Replace the current FastCDC filesystem object layer with Zova object APIs.
- [ ] Make media writes use `put_object` for small in-memory data.
- [ ] Make large media writes use `ObjectWriter`.
- [ ] Make media reads use `get_object` where full bytes are needed.
- [ ] Make previews and partial-serving paths use `read_object_range`.
- [ ] Replace `chunks/` as the authoritative chunk store.
- [ ] Keep `chunks/` read-only as migration input until verification passes.
- [ ] Store file metadata in app SQL tables while bytes live as Zova objects.
- [ ] Verify duplicate media stores deduplicate to the same object ID.
- [ ] Verify each media command can still return bytes by frontend-facing `file_hash`.
- [ ] Verify stickers continue to resolve through file metadata and object IDs.

## File Transfer And Chunk Exchange

- [ ] Replace outbound chunk manifest loading with Zova object manifests.
- [ ] Replace outbound chunk reads with `get_object_chunk`.
- [ ] Replace inbound chunk writes with `put_object_chunk`.
- [ ] Replace file completion logic with `assemble_object_from_chunks`.
- [ ] Keep transfer progress, sender peer, retry state, and UI progress in RChat user SQL tables.
- [ ] Verify corrupted chunks are rejected by hash validation.
- [ ] Verify loose chunks can be assembled after all chunks arrive.
- [ ] Decide whether partial inbound transfers survive restart.
- [ ] If partial transfers survive restart, add tests for restart and resume.
- [ ] If partial transfers do not survive restart, clear transfer state safely on startup and document it.

## Existing Data Migration

- [ ] Add startup detection:
  - if `rchat.zova` exists, open it
  - if only `rchat.sqlite` exists, run or offer migration
  - if neither exists, create fresh `rchat.zova`
- [ ] Create a timestamped backup of `rchat.sqlite` before migration.
- [ ] Preserve old `chunks/` unchanged before migration.
- [ ] Decide whether to use Zova's SQLite conversion as the first step.
- [ ] If using conversion:
  - convert `rchat.sqlite` to `rchat.zova`
  - run schema upgrade inside `.zova`
  - import old filesystem chunks into Zova objects
- [ ] If not using conversion:
  - create fresh `rchat.zova`
  - copy rows table-by-table
  - import old filesystem chunks into Zova objects
- [ ] Verify every imported object's full SHA-256 equals the legacy `file_hash`.
- [ ] Verify every message with a media `file_hash` resolves to a readable object.
- [ ] Log migration counts:
  - peers
  - chats
  - chat memberships
  - messages
  - files
  - stickers
  - envelopes
  - connection stats
  - imported objects
  - skipped duplicate objects
  - rejected/corrupt chunks
- [ ] Make migration idempotent:
  - fail safely if a completed migration already exists
  - resume deliberately or restart cleanly after a failed migration
  - never silently mix partial migrated state with live use
- [ ] Never delete `rchat.sqlite` or `chunks/` automatically in the first Zova-backed release.

## Operational Safety

- [ ] Add backend maintenance commands:
  - `backup_zova_database`
  - `compact_zova_database`
  - `restore_zova_backup`
  - `check_zova_database`
- [ ] Use `SharedDatabase::backup_to` for online backups.
- [ ] Use `SharedDatabase::compact_to` for space-reclaiming copies.
- [ ] Use `zova::restore_backup` only into a new destination file.
- [ ] Keep default verification enabled for backup, compact, and restore.
- [ ] Do not replace the live database file while Zova handles are open.
- [ ] Implement restore as an app-level controlled sequence:
  - stop protected app/network state
  - close/drop store handles
  - move current `rchat.zova` aside
  - move restored file into place
  - reopen store
  - validate schema and data
  - restart protected app/network state
- [ ] Add a manual CLI recovery document using:
  - `zova backup`
  - `zova compact`
  - `zova restore`
  - `zova check --deep`
- [ ] Add tests that backup/compact/restore preserve records, media objects, and vectors once vectors are introduced.

## Packaging And CI

- [ ] Add Zig `0.16.0+` to developer setup documentation.
- [ ] Add a clear preflight error when Zig is missing or too old.
- [ ] Update macOS packaging checks for the Zova native build.
- [ ] Update Linux package/build docs for the Zova native build.
- [ ] Update CI to run:
  - `cargo check --manifest-path src-tauri/Cargo.toml`
  - `cargo test --manifest-path src-tauri/Cargo.toml storage`
  - `cargo test --manifest-path src-tauri/Cargo.toml media`
  - `cargo test --manifest-path src-tauri/Cargo.toml chat`
  - `cargo test --manifest-path src-tauri/Cargo.toml network::manager`
  - `cargo test --manifest-path src-tauri/Cargo.toml`
  - `pnpm check`
- [ ] Verify release scripts still work with the native Zova build.
- [ ] Verify clean clone builds without relying on local `/Users/.../zova` checkout.

## Tests

- [ ] Add tests for Zova schema initialization.
- [ ] Add tests for opening an existing `rchat.zova`.
- [ ] Add tests for reserved `_zova_*` table protection.
- [ ] Add tests for peer CRUD.
- [ ] Add tests for chat CRUD.
- [ ] Add tests for chat membership operations.
- [ ] Add tests for message insert/query/status/read behavior.
- [ ] Add tests for unread count behavior.
- [ ] Add tests for envelope CRUD and chat assignment.
- [ ] Add tests for connection stats.
- [ ] Add tests for media object store/load/range-read behavior.
- [ ] Add tests for duplicate object deduplication.
- [ ] Add tests for sticker storage and retrieval.
- [ ] Add tests for outbound object manifest generation.
- [ ] Add tests for inbound loose chunk ingest.
- [ ] Add tests for corrupted chunk rejection.
- [ ] Add tests for object assembly from chunks.
- [ ] Add tests for migration from fixture `rchat.sqlite` plus fixture `chunks/`.
- [ ] Add tests for failed migration recovery.
- [ ] Add tests for backup/compact/restore.
- [ ] Add regression tests for frontend command payload compatibility.
- [ ] Remove or rewrite tests that directly depend on `rusqlite::Connection`.

## Search And Future Vectors

- [ ] Keep semantic search out of the first storage migration unless all record/object tests are green.
- [ ] Add vector schema only as dormant infrastructure if it does not affect existing behavior.
- [ ] Plan a later vector collection for message text chunks.
- [ ] Plan a later vector collection for attachment/document extracted text.
- [ ] Keep vector IDs application-owned.
- [ ] Join vector IDs back to messages/files through RChat user SQL tables.

## Documentation Updates

- [ ] Update `README.md` storage sections from SQLite + `chunks/` to Zova `.zova`.
- [ ] Explain that Zova uses SQLite internally, but RChat no longer owns a raw `rusqlite` connection.
- [ ] Update the architecture diagram to show `rchat.zova`.
- [ ] Explain backup, compact, restore, and check flows.
- [ ] Document Zig `0.16.0+`, Rust `1.79+`, and C compiler/linker requirements.
- [ ] Document migration behavior and rollback files.
- [ ] Document troubleshooting:
  - missing Zig
  - failed native build
  - failed migration
  - restore from backup
  - read-only validation

## Implementation Exit Criteria

- [ ] RChat starts from a fresh profile and creates `rchat.zova`.
- [ ] RChat migrates an existing `rchat.sqlite` plus `chunks/` fixture.
- [ ] Existing chats, messages, stickers, envelopes, and files survive migration.
- [ ] Sending and receiving text messages works.
- [ ] Sending and receiving media messages works.
- [ ] File transfer chunk exchange works through Zova manifests/chunks.
- [ ] Voice calls still work; storage migration does not affect live media.
- [ ] Frontend API payloads remain compatible.
- [ ] `rchat.sqlite` and `chunks/` are not deleted automatically.
- [ ] Backup, compact, restore, and check are available from backend tests or commands.
- [ ] Full backend tests pass.
- [ ] `pnpm check` passes.
- [ ] Release packaging check passes.
