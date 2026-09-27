//! Object storage with FastCDC content-defined chunking.
//!
//! This module provides functions to store, load, and delete objects (files)
//! using content-defined chunking for deduplication.

use anyhow::{Context, Result};
use fastcdc::v2020::FastCDC;

use rchat_storage::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;

// Chunk size parameters (in bytes)
const MIN_CHUNK_SIZE: u32 = 2 * 1024; // 2 KB
const AVG_CHUNK_SIZE: u32 = 8 * 1024; // 8 KB
const MAX_CHUNK_SIZE: u32 = 64 * 1024; // 64 KB

/// Calculate SHA256 hash and return as hex string.
fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    hex::encode(result)
}

/// Store an object (file) using content-defined chunking.
///
/// Returns the file hash (SHA256 of the complete file).
pub fn create(
    conn: &Connection,
    data: &[u8],
    file_name: Option<&str>,
    mime_type: Option<&str>,
    _root_dir: Option<PathBuf>,
) -> Result<String> {
    let file_hash = sha256_hex(data);
    let size_bytes = data.len() as i64;

    // Chunk the data using FastCDC
    let chunker = FastCDC::new(data, MIN_CHUNK_SIZE, AVG_CHUNK_SIZE, MAX_CHUNK_SIZE);
    let mut chunk_order: i64 = 0;
    let mut chunk_records: Vec<(String, i64, i64)> = Vec::new(); // (chunk_hash, chunk_order, chunk_size)

    for chunk in chunker {
        let chunk_data = &data[chunk.offset..chunk.offset + chunk.length];
        let chunk_hash = sha256_hex(chunk_data);
        let chunk_size = chunk.length as i64;

        conn.put_chunk(&chunk_hash, chunk_data)?;

        chunk_records.push((chunk_hash, chunk_order, chunk_size));
        chunk_order += 1;
    }

    // Zova object writes cannot participate in a SQL transaction. Stage and verify
    // first; a metadata rollback can leave only unreferenced, reusable chunks.
    conn.assemble_object(
        &file_hash,
        &chunk_records
            .iter()
            .map(|(hash, _, size)| (hash.clone(), *size as u64))
            .collect::<Vec<_>>(),
    )?;

    // Begin transaction
    let tx = conn.unchecked_transaction()?;

    tx.execute("DELETE FROM file_chunks WHERE file_hash = ?1", [&file_hash])?;

    // Insert into files table, repairing incomplete placeholders created by inbound transfers.
    tx.execute(
        "INSERT INTO files (file_hash, file_name, mime_type, size_bytes, is_complete)
         VALUES (?1, ?2, ?3, ?4, 1)
         ON CONFLICT(file_hash) DO UPDATE SET
            file_name = COALESCE(excluded.file_name, files.file_name),
            mime_type = COALESCE(excluded.mime_type, files.mime_type),
            size_bytes = excluded.size_bytes,
            is_complete = 1",
        params![&file_hash, file_name, mime_type, size_bytes,],
    )?;

    // Insert into file_chunks table
    for (chunk_hash, order, size) in &chunk_records {
        tx.execute(
            "INSERT INTO file_chunks (file_hash, chunk_order, chunk_hash, chunk_size) VALUES (?1, ?2, ?3, ?4)",
            (&file_hash, order, chunk_hash, size),
        )?;
    }

    tx.commit()?;

    Ok(file_hash)
}

/// Load an object (file) by reassembling its chunks.
///
/// Returns the complete file data.
pub fn load(conn: &Connection, file_hash: &str, _root_dir: Option<PathBuf>) -> Result<Vec<u8>> {
    // Verify file exists
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM files WHERE file_hash = ?1 AND is_complete=1)",
        [file_hash],
        |row| row.get(0),
    )?;

    if !exists {
        anyhow::bail!("File not found: {}", file_hash);
    }

    conn.get_object(file_hash)
        .context("Media unavailable; retry the attachment")
}

/// Assemble only a complete, ordered, size- and hash-verified transport manifest.
pub(crate) fn assemble_received(conn: &Connection, file_hash: &str) -> Result<bool> {
    let size: i64 = conn.query_row(
        "SELECT size_bytes FROM files WHERE file_hash=?1",
        [file_hash],
        |r| r.get(0),
    )?;
    let mut statement = conn.prepare("SELECT chunk_order, chunk_hash, chunk_size FROM file_chunks WHERE file_hash=?1 ORDER BY chunk_order")?;
    let rows = statement.query_map([file_hash], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
        ))
    })?;
    let mut chunks = Vec::new();
    let mut total = 0i64;
    let mut hasher = Sha256::new();
    for row in rows {
        let (order, hash, length) = row?;
        if order != chunks.len() as i64
            || !(1..=i64::from(MAX_CHUNK_SIZE)).contains(&length)
            || hash.len() != 64
            || !hash.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Ok(false);
        }
        if !conn.has_chunk(&hash)? {
            return Ok(false);
        }
        let bytes = conn.get_chunk(&hash)?;
        if bytes.len() as i64 != length {
            return Ok(false);
        }
        total = total
            .checked_add(length)
            .context("manifest size overflow")?;
        hasher.update(&bytes);
        chunks.push((hash, length as u64));
    }
    if total != size || hex::encode(hasher.finalize()) != file_hash {
        return Ok(false);
    }
    conn.assemble_object(file_hash, &chunks)?;
    conn.execute(
        "UPDATE files SET is_complete=1 WHERE file_hash=?1",
        [file_hash],
    )?;
    Ok(true)
}

/// Import legacy files in bounded chunk-sized reads; never delete the originals.
pub(crate) fn import_legacy_chunks(conn: &Connection, root: &std::path::Path) -> Result<()> {
    use std::io::Read;
    // Keyset traversal finalizes each SQL statement before object writes. A live
    // read cursor also counts as a transaction to Zova's object API.
    let mut previous: Option<String> = None;
    while let Some(file) = conn.query_row(
        "SELECT file_hash FROM files WHERE ?1 IS NULL OR file_hash>?1 ORDER BY file_hash LIMIT 1",
        params![previous], |r| r.get::<_,String>(0)).optional()? {
        previous = Some(file.clone());
        conn.execute("UPDATE files SET is_complete=0 WHERE file_hash=?1", [&file])?;
        let mut previous_order: Option<i64> = None;
        while let Some((order,hash)) = conn.query_row(
            "SELECT chunk_order,chunk_hash FROM file_chunks WHERE file_hash=?1 AND (?2 IS NULL OR chunk_order>?2) ORDER BY chunk_order LIMIT 1",
            params![file,previous_order], |r| Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?))).optional()? {
            previous_order = Some(order);
            // A legacy database is input, not a trusted filesystem path.
            if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
                continue;
            }
            let path = root.join("chunks").join(&hash);
            let input = match fs::File::open(path) {
                Ok(file) => file,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            };
            let mut bytes = Vec::new();
            input
                .take(u64::from(MAX_CHUNK_SIZE) + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() > MAX_CHUNK_SIZE as usize || sha256_hex(&bytes) != hash {
                continue;
            }
            conn.put_chunk(&hash, &bytes)?;
        }
        assemble_received(conn, &file)?;
    }
    Ok(())
}

/// Delete an object (file) from the database.
///
/// Note: Zova bytes are retained to avoid invalidating other references.
/// A separate garbage collection process can clean up orphaned chunks.
#[cfg(test)]
pub fn delete(conn: &Connection, file_hash: &str) -> Result<()> {
    let tx = conn.unchecked_transaction()?;

    // Delete from file_chunks first (foreign key constraint)
    tx.execute("DELETE FROM file_chunks WHERE file_hash = ?1", [file_hash])?;

    // Delete from files
    let rows_deleted = tx.execute("DELETE FROM files WHERE file_hash = ?1", [file_hash])?;

    tx.commit()?;

    if rows_deleted == 0 {
        anyhow::bail!("File not found: {}", file_hash);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rchat_storage::Connection;
    use tempfile::tempdir;

    #[test]
    fn legacy_import_preserves_manifest_and_missing_media_is_retryable() {
        let conn = setup_test_db();
        let root = tempdir().unwrap();
        fs::create_dir(root.path().join("chunks")).unwrap();
        let data = b"legacy attachment";
        let hash = sha256_hex(data);
        let missing = sha256_hex(b"missing");
        let corrupt = sha256_hex(b"corrupt");
        for (id, size) in [(&hash, data.len()), (&missing, 7), (&corrupt, 7)] {
            conn.execute(
                "INSERT INTO files VALUES(?1,'file','application/octet-stream',?2,1)",
                params![id, size],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO file_chunks VALUES(?1,0,?1,?2)",
                params![id, size],
            )
            .unwrap();
        }
        fs::write(root.path().join("chunks").join(&hash), data).unwrap();
        fs::write(root.path().join("chunks").join(&corrupt), b"WRONG!!").unwrap();
        import_legacy_chunks(&conn, root.path()).unwrap();
        assert_eq!(load(&conn, &hash, None).unwrap(), data);
        assert!(load(&conn, &missing, None).is_err());
        assert!(load(&conn, &corrupt, None).is_err());
        assert_eq!(
            fs::read(root.path().join("chunks").join(&hash)).unwrap(),
            data
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM file_chunks", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            3
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM files WHERE is_complete=1", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap(),
            1
        );
    }

    fn setup_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();

        // Create tables
        conn.execute(
            "CREATE TABLE files (
                file_hash TEXT PRIMARY KEY,
                file_name TEXT,
                mime_type TEXT,
                size_bytes INTEGER,
                is_complete BOOLEAN DEFAULT 0
            )",
            [],
        )
        .unwrap();

        conn.execute(
            "CREATE TABLE file_chunks (
                file_hash TEXT NOT NULL,
                chunk_order INTEGER NOT NULL,
                chunk_hash TEXT NOT NULL,
                chunk_size INTEGER NOT NULL,
                PRIMARY KEY (file_hash, chunk_order),
                FOREIGN KEY (file_hash) REFERENCES files(file_hash)
            )",
            [],
        )
        .unwrap();

        conn
    }

    #[test]
    fn test_create_and_load() {
        let conn = setup_test_db();
        let temp = tempdir().unwrap();
        let root = Some(temp.path().to_path_buf());

        // Create test data (larger than chunk size to ensure multiple chunks)
        let test_data: Vec<u8> = (0..100_000).map(|i| (i % 256) as u8).collect();

        // Create object
        let file_hash = create(
            &conn,
            &test_data,
            Some("test.bin"),
            Some("application/octet-stream"),
            root.clone(),
        )
        .expect("Failed to create object");

        // Load object
        let loaded_data = load(&conn, &file_hash, root).expect("Failed to load object");

        // Verify
        assert_eq!(test_data, loaded_data);
    }

    #[test]
    fn test_deduplication() {
        let conn = setup_test_db();
        let temp = tempdir().unwrap();
        let root = Some(temp.path().to_path_buf());

        let test_data = b"Hello, World! This is a test file.".to_vec();

        // Create same object twice
        let hash1 = create(&conn, &test_data, Some("file1.txt"), None, root.clone()).unwrap();
        let hash2 = create(&conn, &test_data, Some("file2.txt"), None, root).unwrap();

        // Hashes should be identical
        assert_eq!(hash1, hash2);

        // Only one file record should exist
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))
            .unwrap();

        assert_eq!(count, 1);
    }

    #[test]
    fn create_repairs_incomplete_placeholder_row() {
        let conn = setup_test_db();
        let temp = tempdir().unwrap();
        let root = Some(temp.path().to_path_buf());
        let test_data: Vec<u8> = (0..20_000).map(|i| (i % 251) as u8).collect();
        let file_hash = sha256_hex(&test_data);

        conn.execute(
            "INSERT INTO files (file_hash, file_name, mime_type, size_bytes, is_complete)
             VALUES (?1, NULL, 'application/octet-stream', 0, 0)",
            [&file_hash],
        )
        .unwrap();

        let repaired_hash = create(
            &conn,
            &test_data,
            Some("image.png"),
            Some("image/png"),
            root.clone(),
        )
        .unwrap();

        assert_eq!(repaired_hash, file_hash);
        let chunk_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM file_chunks WHERE file_hash = ?1",
                [&file_hash],
                |row| row.get(0),
            )
            .unwrap();
        assert!(chunk_count > 0);
        let (size_bytes, is_complete, mime_type): (i64, bool, String) = conn
            .query_row(
                "SELECT size_bytes, is_complete, mime_type FROM files WHERE file_hash = ?1",
                [&file_hash],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(size_bytes, test_data.len() as i64);
        assert!(is_complete);
        assert_eq!(mime_type, "image/png");
        assert_eq!(load(&conn, &file_hash, root).unwrap(), test_data);
    }

    #[test]
    fn test_delete() {
        let conn = setup_test_db();
        let temp = tempdir().unwrap();
        let root = Some(temp.path().to_path_buf());

        let test_data = b"Data to be deleted".to_vec();

        let file_hash = create(&conn, &test_data, None, None, root.clone()).unwrap();

        // Verify exists
        assert!(load(&conn, &file_hash, root.clone()).is_ok());

        // Delete
        delete(&conn, &file_hash).unwrap();

        // Verify load fails
        assert!(load(&conn, &file_hash, root).is_err());
    }

    #[test]
    fn test_delete_nonexistent() {
        let conn = setup_test_db();

        // Deleting non-existent file should error
        assert!(delete(&conn, "nonexistent_hash").is_err());
    }
}
