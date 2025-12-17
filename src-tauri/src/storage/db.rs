use rusqlite::Connection;
// use std::path::Path; // Unused
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
            
        // Always ensure schema exists!
        create_tables(&connection)?;
        
        if !db_exists {
            // Only verify or notify if needed, but creates happened above
            println!("Successfully initialized database schema!");
        }
        Ok(connection)
    } else {
        anyhow::bail!("Failed to determine project directories")
    }
}

// Private helper to ensure tables exist
fn create_tables(conn: &Connection) -> anyhow::Result<()> {
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

    seed_defaults(conn)?;

    Ok(())
}

fn seed_defaults(conn: &Connection) -> anyhow::Result<()> {
    // 1. Ensure 'Me' Peer exists
    conn.execute(
        "INSERT OR IGNORE INTO peers (id, alias, last_seen, public_key) 
         VALUES (?1, ?2, ?3, ?4)",
        (
            "Me", 
            "Me (You)", 
            0, 
            Vec::new() // Dummy empty key for self
        ),
    )?;

    // 2. Ensure 'self' Chat exists
    conn.execute(
        "INSERT OR IGNORE INTO chats (id, name, is_group, encryption_key) 
         VALUES (?1, ?2, ?3, ?4)",
        (
            "self", 
            "Note to Self", 
            0, 
            Vec::new() // Dummy empty key for self chat
        ),
    )?;

    // 3. Ensure joined_at for 'Me' in 'self' chat
    conn.execute(
        "INSERT OR IGNORE INTO chat_peers (chat_id, peer_id, role, joined_at)
         VALUES (?1, ?2, ?3, ?4)",
        (
            "self",
            "Me",
            "admin",
            0
        )
    )?;

    Ok(())
}

// --- 3. Database Operations ---

pub fn insert_message(conn: &Connection, msg: &Message) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO messages (id, chat_id, peer_id, timestamp, content_type, text_content, file_hash)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        (
            &msg.id,
            &msg.chat_id,
            &msg.peer_id,
            &msg.timestamp,
            &msg.content_type,
            &msg.text_content,
            &msg.file_hash,
        ),
    )?;
    Ok(())
}

pub fn get_messages(conn: &Connection, chat_id: &str) -> anyhow::Result<Vec<Message>> {
    let mut stmt = conn.prepare(
        "SELECT id, chat_id, peer_id, timestamp, content_type, text_content, file_hash 
         FROM messages 
         WHERE chat_id = ?1 
         ORDER BY timestamp ASC"
    )?;
    
    let msg_iter = stmt.query_map([chat_id], |row| {
        Ok(Message {
            id: row.get(0)?,
            chat_id: row.get(1)?,
            peer_id: row.get(2)?,
            timestamp: row.get(3)?,
            content_type: row.get(4)?,
            text_content: row.get(5)?,
            file_hash: row.get(6)?,
        })
    })?;

    let mut messages = Vec::new();
    for msg in msg_iter {
        messages.push(msg?);
    }
    Ok(messages)
}