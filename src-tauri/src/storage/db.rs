use rusqlite::{Connection, Result};
use std::path::Path;
use anyhow::Context;
use serde::{Serialize, Deserialize};
use directories::ProjectDirs;

// --- 1. Rust Structs (Data Models) ---

#[derive(Debug, Serialize, Deserialize)]
pub struct Peer {
    pub id: String,
    pub alias: String,
    pub last_seen: i64, // Unix Timestamp
    pub public_key: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Chat {
    pub id: String,
    pub name: String,
    pub is_group: bool,
    pub encryption_key: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChatPeer {
    pub chat_id: String,
    pub peer_id: String,
    pub role: String, // 'admin', 'member'
    pub joined_at: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub chat_id: String,
    pub peer_id: String,
    pub timestamp: i64,
    pub content_type: String, // 'text', 'file'
    pub text_content: Option<String>,
    pub file_hash: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FileMetadata {
    pub file_hash: String,
    pub file_name: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub is_complete: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FileChunk {
    pub file_hash: String,
    pub chunk_order: i64,
    pub chunk_hash: String,
    pub chunk_size: i64,
}

// --- 2. Database Initialization ---
pub fn connect_to_db() -> anyhow::Result<Connection> {
    if let Some(project_dirs) = ProjectDirs::from("io.github", "ata-sesli", "RChat") {
        let project_dirs = project_dirs.data_dir();
        let database_dir = project_dirs.join("databases");
        std::fs::create_dir_all(&database_dir)
            .context("Failed to create database directory")?;
        let final_path = database_dir.join("rchat.sqlite");
        let db_exists = final_path.exists();
        let connection = Connection::open(&final_path)
            .context("Failed to open database connection")?;
        if db_exists {
            println!("Successfully opened existing database!");
        } else {
            println!("Successfully created new database!");
        }
        Ok(connection)
    } else {
        anyhow::bail!("Failed to determine project directories")
    }
}
pub fn init() -> anyhow::Result<Connection> {
    let conn = connect_to_db()?;

    // --- Critical Performance & Safety Settings ---
    // Enable Write-Ahead Logging for concurrency (Readers don't block Writers)
    conn.pragma_update(None, "journal_mode", "WAL")?;
    // Relax sync slightly for SSD health (optional, good for desktop apps)
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    // Enforce Foreign Key constraints (SQLite disables them by default!)
    conn.execute("PRAGMA foreign_keys = ON;", [])?;

    // --- Schema Creation ---
    
    // 1. Peers
    conn.execute(
        "CREATE TABLE IF NOT EXISTS peers (
            id TEXT NOT NULL PRIMARY KEY,
            alias TEXT NOT NULL,
            last_seen INTEGER,
            public_key BLOB NOT NULL
        )",
        [],
    )?;

    // 2. Chats
    conn.execute(
        "CREATE TABLE IF NOT EXISTS chats (
            id TEXT NOT NULL PRIMARY KEY,
            name TEXT NOT NULL,
            is_group INTEGER DEFAULT 0 NOT NULL,
            encryption_key BLOB NOT NULL
        )",
        [],
    )?;

    // 3. Chat Peers (Junction Table)
    conn.execute(
        "CREATE TABLE IF NOT EXISTS chat_peers (
            chat_id TEXT NOT NULL,
            peer_id TEXT NOT NULL,
            role TEXT DEFAULT 'member' NOT NULL,
            joined_at INTEGER NOT NULL,
            PRIMARY KEY (chat_id, peer_id),
            FOREIGN KEY (peer_id) REFERENCES peers(id),
            FOREIGN KEY (chat_id) REFERENCES chats(id)
        )",
        [],
    )?;

    // 4. Files
    conn.execute(
        "CREATE TABLE IF NOT EXISTS files (
            file_hash TEXT PRIMARY KEY,
            file_name TEXT,
            mime_type TEXT,
            size_bytes INTEGER,
            is_complete BOOLEAN DEFAULT 0
        )",
        [],
    )?;

    // 5. File Chunks
    conn.execute(
        "CREATE TABLE IF NOT EXISTS file_chunks (
            file_hash TEXT NOT NULL,
            chunk_order INTEGER NOT NULL,
            chunk_hash TEXT NOT NULL,
            chunk_size INTEGER NOT NULL,
            PRIMARY KEY (file_hash, chunk_order),
            FOREIGN KEY (file_hash) REFERENCES files(file_hash)
        )",
        [],
    )?;

    // 6. Messages
    conn.execute(
        "CREATE TABLE IF NOT EXISTS messages (
            id TEXT NOT NULL PRIMARY KEY,
            chat_id TEXT NOT NULL,
            peer_id TEXT NOT NULL,
            timestamp INTEGER NOT NULL,
            content_type TEXT NOT NULL,
            text_content TEXT,
            file_hash TEXT,
            FOREIGN KEY (chat_id) REFERENCES chats(id),
            FOREIGN KEY (peer_id) REFERENCES peers(id),
            FOREIGN KEY (file_hash) REFERENCES files(file_hash)
        )",
        [],
    )?;

    // --- Indexes (Crucial for Speed) ---
    
    // Speed up loading chat history (WHERE chat_id = ?)
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_messages_chat_id ON messages(chat_id)",
        [],
    )?;

    // Speed up sorting messages (ORDER BY timestamp)
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_messages_timestamp ON messages(timestamp)",
        [],
    )?;

    // Speed up finding chunks for a file (WHERE file_hash = ?)
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_file_chunks_file_hash ON file_chunks(file_hash)",
        [],
    )?;

    Ok(conn)
}