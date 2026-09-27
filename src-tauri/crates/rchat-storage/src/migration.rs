use crate::{Connection, Error, Result};
use std::{fs, path::Path};
use zova::SharedDatabase;

fn io(error: std::io::Error) -> Error {
    Error::InvalidValue(error.to_string())
}

/// Copy forward into a staging database. Never modify or delete the legacy source.
/// A failed staging file is deliberately retained and requires explicit recovery.
pub fn open_rchat(
    directory: &Path,
    initialize: impl FnOnce(&Connection) -> Result<()>,
) -> Result<Connection> {
    fs::create_dir_all(directory).map_err(io)?;
    let destination = directory.join("rchat.zova");
    let staging = directory.join("rchat.importing.zova");
    let legacy = directory.join("rchat.sqlite");
    if destination.exists() {
        let db = Connection::open(&destination)?;
        let version: i64 = db.query_row(
            "SELECT version FROM rchat_storage_migration WHERE id=1",
            [],
            |r| r.get(0),
        )?;
        if version != 1 {
            return Err(Error::InvalidValue(
                "unsupported RChat storage version".into(),
            ));
        }
        return Ok(db);
    }
    let lock_path = directory.join("rchat-migration.lock");
    let lock = fs::OpenOptions::new().write(true).create_new(true).open(&lock_path)
        .map_err(|e| Error::InvalidValue(format!("cannot reserve migration (close other RChat processes; see storage recovery documentation): {e}")))?;
    struct Lock<'a>(&'a Path, Option<fs::File>);
    impl Drop for Lock<'_> {
        fn drop(&mut self) {
            drop(self.1.take());
            let _ = fs::remove_file(self.0);
        }
    }
    let _lock = Lock(&lock_path, Some(lock));
    if staging.exists() || destination.exists() {
        return Err(Error::InvalidValue(
            "migration staging/destination already exists; explicit recovery required".into(),
        ));
    }
    if legacy.exists() {
        SharedDatabase::convert_sqlite_to_zova(&legacy, &staging)?;
    }
    let db = Connection::open(&staging)?;
    initialize(&db)?;
    let integrity: String = db.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    if integrity != "ok" {
        return Err(Error::InvalidValue(format!(
            "migration integrity check: {integrity}"
        )));
    }
    if db
        .prepare("PRAGMA foreign_key_check")?
        .query([])?
        .next()?
        .is_some()
    {
        return Err(Error::InvalidValue(
            "migration foreign-key check failed".into(),
        ));
    }
    db.execute_batch("CREATE TABLE rchat_storage_migration(id INTEGER PRIMARY KEY CHECK(id=1), version INTEGER NOT NULL); INSERT INTO rchat_storage_migration VALUES(1,1);")?;
    // Checkpoint before renaming: no committed data may remain in a staging WAL.
    let busy: i64 = db.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get(0))?;
    if busy != 0 {
        return Err(Error::InvalidValue(
            "migration WAL checkpoint is busy".into(),
        ));
    }
    drop(db);
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&staging)
        .map_err(io)?
        .sync_all()
        .map_err(io)?;
    fs::rename(&staging, &destination).map_err(io)?;
    #[cfg(unix)]
    fs::File::open(directory)
        .map_err(io)?
        .sync_all()
        .map_err(io)?;
    Connection::open(destination)
}
