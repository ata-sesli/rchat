//! Object storage with FastCDC content-defined chunking.
//!
//! This module provides functions to store, load, and delete objects (files)
//! using content-defined chunking for deduplication.

use anyhow::{Context, Result};
use directories::ProjectDirs;
use fastcdc::v2020::FastCDC;

use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;

// Chunk size parameters (in bytes)
const MIN_CHUNK_SIZE: u32 = 2 * 1024; // 2 KB
const AVG_CHUNK_SIZE: u32 = 8 * 1024; // 8 KB
const MAX_CHUNK_SIZE: u32 = 64 * 1024; // 64 KB

/// Get the chunks directory path.
fn get_chunks_dir(root_dir: Option<PathBuf>) -> Result<PathBuf> {
    let base_dir = if let Some(d) = root_dir {
        d
    } else {
        let project_dirs = ProjectDirs::from("io.github", "ata-sesli", "RChat")
            .context("Failed to determine project directories")?;
        project_dirs.data_dir().to_path_buf()
    };

    let chunks_dir = base_dir.join("chunks");
    fs::create_dir_all(&chunks_dir).context("Failed to create chunks directory")?;
    Ok(chunks_dir)
}

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
    root_dir: Option<PathBuf>,
) -> Result<String> {
    let file_hash = sha256_hex(data);
    let size_bytes = data.len() as i64;

    // Check if file already exists
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM files WHERE file_hash = ?1)",
        [&file_hash],
        |row| row.get(0),
    )?;

    if exists {
        return Ok(file_hash);
    }

    let chunks_dir = get_chunks_dir(root_dir)?;

    // Chunk the data using FastCDC
    let chunker = FastCDC::new(data, MIN_CHUNK_SIZE, AVG_CHUNK_SIZE, MAX_CHUNK_SIZE);
    let mut chunk_order: i64 = 0;
    let mut chunk_records: Vec<(String, i64, i64)> = Vec::new(); // (chunk_hash, chunk_order, chunk_size)

    for chunk in chunker {
        let chunk_data = &data[chunk.offset..chunk.offset + chunk.length];
        let chunk_hash = sha256_hex(chunk_data);
        let chunk_size = chunk.length as i64;

        // Store chunk to disk if it doesn't exist (deduplication)
        let chunk_path = chunks_dir.join(&chunk_hash);
        if !chunk_path.exists() {
            fs::write(&chunk_path, chunk_data)
                .with_context(|| format!("Failed to write chunk {}", chunk_hash))?;
        }

        chunk_records.push((chunk_hash, chunk_order, chunk_size));
        chunk_order += 1;
    }

    // Begin transaction
    let tx = conn.unchecked_transaction()?;

    // Insert into files table
    tx.execute(
        "INSERT INTO files (file_hash, file_name, mime_type, size_bytes, is_complete) VALUES (?1, ?2, ?3, ?4, 1)",
        (
            &file_hash,
            file_name,
            mime_type,
            size_bytes,
        ),
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
pub fn load(conn: &Connection, file_hash: &str, root_dir: Option<PathBuf>) -> Result<Vec<u8>> {
    // Verify file exists
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM files WHERE file_hash = ?1)",
        [file_hash],
        |row| row.get(0),
    )?;

    if !exists {
        anyhow::bail!("File not found: {}", file_hash);
    }

    let chunks_dir = get_chunks_dir(root_dir)?;

    // Get chunks in order
    let mut stmt = conn.prepare(
        "SELECT chunk_hash FROM file_chunks WHERE file_hash = ?1 ORDER BY chunk_order ASC",
    )?;

    let chunk_hashes: Vec<String> = stmt
        .query_map([file_hash], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;

    // Read and concatenate chunks
    let mut result = Vec::new();
    for chunk_hash in chunk_hashes {
        let chunk_path = chunks_dir.join(&chunk_hash);
        let chunk_data = fs::read(&chunk_path)
            .with_context(|| format!("Failed to read chunk {}", chunk_hash))?;
        result.extend_from_slice(&chunk_data);
    }

    Ok(result)
}

/// Delete an object (file) from the database.
///
/// Note: Chunks are NOT deleted from disk to avoid race conditions with deduplication.
/// A separate garbage collection process can clean up orphaned chunks.
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
    use rusqlite::Connection;
    use tempfile::tempdir;

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
