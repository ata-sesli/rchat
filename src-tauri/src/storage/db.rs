use rusqlite::Connection;
// use std::path::Path; // Unused
use anyhow::Context;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

// --- 1. Rust Structs (Data Models) ---

#[derive(Debug, Serialize, Deserialize)]
pub struct Peer {
    pub id: String,
    pub alias: String,
    pub last_seen: i64, // Unix Timestamp
    pub public_key: Vec<u8>,
    pub method: String, // "local", "gist", "manual", etc.
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

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Message {
    pub id: String,
    pub chat_id: String,
    pub peer_id: String,
    pub timestamp: i64,
    pub content_type: String, // 'text', 'photo', 'video', 'document', 'voice'
    pub text_content: Option<String>,
    pub file_hash: Option<String>,
    pub status: String, // 'pending', 'delivered', 'read'
    pub content_metadata: Option<String>, // JSON: {"width": 1920, "height": 1080, ...}
    pub sender_alias: Option<String>, // Sender's display name
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

#[derive(Debug, Serialize, Deserialize)]
pub struct Envelope {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChatAssignment {
    pub chat_id: String,
    pub envelope_id: String,
}

// --- 2. Database Initialization ---
pub fn connect_to_db() -> anyhow::Result<Connection> {
    if let Some(project_dirs) = ProjectDirs::from("io.github", "ata-sesli", "RChat") {
        let project_dirs = project_dirs.data_dir();
        let database_dir = project_dirs.join("databases");
        std::fs::create_dir_all(&database_dir).context("Failed to create database directory")?;
        let final_path = database_dir.join("rchat.sqlite");
        let db_exists = final_path.exists();
        let connection =
            Connection::open(&final_path).context("Failed to open database connection")?;

        // Always ensure schema exists!
        create_tables(&connection)?;

        // Enable Foreign Keys explicitly (SQLite default is OFF)
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .context("Failed to enable foreign keys")?;

        // Set busy timeout to 5 seconds to avoid 'database is locked' errors
        connection
            .pragma_update(None, "busy_timeout", 5000)
            .context("Failed to set busy timeout")?;

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
             public_key BLOB NOT NULL,
             method TEXT NOT NULL DEFAULT 'unknown'
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

    // SEED: Ensure 'Me' user exists
    let me_exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM peers WHERE id = ?1)",
            ["Me"],
            |row| row.get(0),
        )
        .unwrap_or(false);

    if !me_exists {
        println!("Seeding default 'Me' user...");
        conn.execute(
            "INSERT INTO peers (id, alias, last_seen, public_key, method) VALUES (?1, ?2, ?3, ?4, ?5)",
            ("Me", "Me", 0, vec![0u8; 32], "self"), // method = "self" for the user's own entry
        )?;
    }

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
             status TEXT NOT NULL DEFAULT 'pending',
             FOREIGN KEY (chat_id) REFERENCES chats(id),
             FOREIGN KEY (peer_id) REFERENCES peers(id),
             FOREIGN KEY (file_hash) REFERENCES files(file_hash)
         )",
        [],
    )?;

    // Migration: Add status column if it doesn't exist
    let _ = conn.execute(
        "ALTER TABLE messages ADD COLUMN status TEXT NOT NULL DEFAULT 'pending'",
        [],
    );

    // Migration: Add content_metadata column for cached computed attributes (width, height, duration, etc.)
    let _ = conn.execute(
        "ALTER TABLE messages ADD COLUMN content_metadata TEXT",
        [],
    );

    // Migration: Add sender_alias column for display name from messages
    let _ = conn.execute(
        "ALTER TABLE messages ADD COLUMN sender_alias TEXT",
        [],
    );

    // 7. Envelopes
    // 7. Envelopes
    conn.execute(
        "CREATE TABLE IF NOT EXISTS envelopes (
                id TEXT NOT NULL PRIMARY KEY,
                name TEXT NOT NULL,
                icon TEXT
            )",
        [],
    )?;

    // Attempt to add 'icon' column if it doesn't exist (Migration for existing DBs)
    let _ = conn.execute("ALTER TABLE envelopes ADD COLUMN icon TEXT", []);

    // 8. Chat Envelopes (Assignments)
    conn.execute(
        "CREATE TABLE IF NOT EXISTS chat_envelopes (
                chat_id TEXT NOT NULL PRIMARY KEY,
                envelope_id TEXT NOT NULL,
                FOREIGN KEY (envelope_id) REFERENCES envelopes(id) ON DELETE CASCADE
            )",
        [],
    )?;

    // 9. Known Devices table removed - using peers table instead

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

    // known_devices index removed - table no longer exists

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
            Vec::new(), // Dummy empty key for self
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
            Vec::new(), // Dummy empty key for self chat
        ),
    )?;

    // 3. Ensure joined_at for 'Me' in 'self' chat
    conn.execute(
        "INSERT OR IGNORE INTO chat_peers (chat_id, peer_id, role, joined_at)
         VALUES (?1, ?2, ?3, ?4)",
        ("self", "Me", "admin", 0),
    )?;

    Ok(())
}

// --- Peer Functions ---

/// Add a new peer to the database (used after handshake)
pub fn add_peer(
    conn: &Connection,
    peer_id: &str,
    alias: Option<&str>,
    public_key: Option<&[u8]>,
    method: &str, // "local", "gist", "manual"
) -> anyhow::Result<()> {
    let alias = alias.unwrap_or(peer_id);
    let public_key = public_key.unwrap_or(&[0u8; 32]);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    conn.execute(
        "INSERT INTO peers (id, alias, last_seen, public_key, method)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(id) DO UPDATE SET
             last_seen = ?3,
             alias = COALESCE(?2, alias)",
        (peer_id, alias, now, public_key, method),
    )?;
    Ok(())
}

/// Get all peers from database
pub fn get_all_peers(conn: &Connection) -> anyhow::Result<Vec<Peer>> {
    // Put "Me" first (method='self'), then sort others by last_seen DESC
    let mut stmt = conn.prepare(
        "SELECT id, alias, last_seen, public_key, method FROM peers 
         ORDER BY CASE WHEN id = 'Me' THEN 0 ELSE 1 END, last_seen DESC",
    )?;

    let rows = stmt.query_map([], |row| {
        Ok(Peer {
            id: row.get(0)?,
            alias: row.get(1)?,
            last_seen: row.get(2)?,
            public_key: row.get(3)?,
            method: row.get(4)?,
        })
    })?;

    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

/// Check if a peer_id exists in the peers table
pub fn is_peer(conn: &Connection, peer_id: &str) -> bool {
    conn.query_row("SELECT 1 FROM peers WHERE id = ?1", [peer_id], |_| Ok(()))
        .is_ok()
}

/// Check if a chat exists for a given chat_id
pub fn chat_exists(conn: &Connection, chat_id: &str) -> bool {
    conn.query_row("SELECT 1 FROM chats WHERE id = ?1", [chat_id], |_| Ok(()))
        .is_ok()
}

/// Create a new chat
pub fn create_chat(
    conn: &Connection,
    chat_id: &str,
    name: &str,
    is_group: bool,
) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO chats (id, name, is_group, encryption_key) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(id) DO NOTHING",
        (chat_id, name, if is_group { 1 } else { 0 }, vec![0u8; 32]),
    )?;
    Ok(())
}

/// Delete a peer and their related chat/messages
pub fn delete_peer(conn: &Connection, peer_id: &str) -> anyhow::Result<()> {
    // 1. Delete Messages
    conn.execute(
        "DELETE FROM messages WHERE peer_id = ?1 OR chat_id = ?1",
        [peer_id],
    )?;
    // 2. Delete Chat (if 1:1)
    conn.execute("DELETE FROM chats WHERE id = ?1", [peer_id])?;
    // 3. Delete Peer
    conn.execute("DELETE FROM peers WHERE id = ?1", [peer_id])?;
    Ok(())
}

// --- 3. Database Operations ---

pub fn insert_message(conn: &Connection, msg: &Message) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO messages (id, chat_id, peer_id, timestamp, content_type, text_content, file_hash, status, content_metadata, sender_alias)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        (
            &msg.id,
            &msg.chat_id,
            &msg.peer_id,
            &msg.timestamp,
            &msg.content_type,
            &msg.text_content,
            &msg.file_hash,
            &msg.status,
            &msg.content_metadata,
            &msg.sender_alias,
        ),
    )?;
    Ok(())
}

/// Update the cached content_metadata for a message (computed attributes like width, height, duration)
pub fn update_content_metadata(conn: &Connection, msg_id: &str, metadata_json: &str) -> anyhow::Result<()> {
    conn.execute(
        "UPDATE messages SET content_metadata = ?1 WHERE id = ?2",
        [metadata_json, msg_id],
    )?;
    Ok(())
}

pub fn get_messages(conn: &Connection, chat_id: &str) -> anyhow::Result<Vec<Message>> {
    let mut stmt = conn.prepare(
        "SELECT id, chat_id, peer_id, timestamp, content_type, text_content, file_hash, COALESCE(status, 'delivered') as status, content_metadata, sender_alias
         FROM messages 
         WHERE chat_id = ?1 
         ORDER BY timestamp ASC",
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
            status: row.get(7)?,
            content_metadata: row.get(8)?,
            sender_alias: row.get(9)?,
        })
    })?;

    let mut messages = Vec::new();
    for msg in msg_iter {
        messages.push(msg?);
    }
    Ok(messages)
}

/// Get the latest sender_alias for each peer from their messages
pub fn get_peer_aliases(conn: &Connection) -> anyhow::Result<std::collections::HashMap<String, String>> {
    let mut stmt = conn.prepare(
        "SELECT chat_id, sender_alias
         FROM messages
         WHERE sender_alias IS NOT NULL AND sender_alias != ''
           AND peer_id != 'Me'
         GROUP BY chat_id
         HAVING MAX(timestamp)"
    )?;

    let mut aliases = std::collections::HashMap::new();
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;

    for row in rows {
        if let Ok((chat_id, alias)) = row {
            aliases.insert(chat_id, alias);
        }
    }
    Ok(aliases)
}

/// Update message status (pending -> delivered -> read)
pub fn update_message_status(conn: &Connection, msg_id: &str, status: &str) -> anyhow::Result<()> {
    conn.execute(
        "UPDATE messages SET status = ?1 WHERE id = ?2",
        [status, msg_id],
    )?;
    Ok(())
}

/// Mark all messages in a chat as read for a given sender
pub fn mark_messages_read(
    conn: &Connection,
    chat_id: &str,
    sender_id: &str,
) -> anyhow::Result<Vec<String>> {
    // Get IDs of messages that will be marked as read
    let mut stmt = conn.prepare(
        "SELECT id FROM messages WHERE chat_id = ?1 AND peer_id = ?2 AND status != 'read'",
    )?;
    let ids: Vec<String> = stmt
        .query_map([chat_id, sender_id], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();

    // Update them
    conn.execute(
        "UPDATE messages SET status = 'read' WHERE chat_id = ?1 AND peer_id = ?2 AND status != 'read'",
        [chat_id, sender_id],
    )?;
    Ok(ids)
}

/// Get unread message count for each chat
pub fn get_unread_counts(
    conn: &Connection,
    my_peer_id: &str,
) -> anyhow::Result<std::collections::HashMap<String, i64>> {
    let mut stmt = conn.prepare(
        "SELECT chat_id, COUNT(*) as count
         FROM messages 
         WHERE peer_id != ?1 AND status != 'read'
         GROUP BY chat_id",
    )?;

    let mut counts = std::collections::HashMap::new();
    let rows = stmt.query_map([my_peer_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;

    for row in rows {
        let (chat_id, count) = row?;
        counts.insert(chat_id, count);
    }
    Ok(counts)
}

/// Get latest message timestamp for each chat (for sorting by recency)
pub fn get_chat_latest_times(
    conn: &Connection,
) -> anyhow::Result<std::collections::HashMap<String, i64>> {
    let mut stmt = conn.prepare(
        "SELECT chat_id, MAX(timestamp) as latest_time
         FROM messages
         GROUP BY chat_id",
    )?;

    let mut result = std::collections::HashMap::new();
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;

    for row in rows {
        let (chat_id, latest_time) = row?;
        result.insert(chat_id, latest_time);
    }

    Ok(result)
}

// --- Envelope Operations ---

pub fn create_envelope(
    conn: &Connection,
    id: &str,
    name: &str,
    icon: Option<&str>,
) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO envelopes (id, name, icon) VALUES (?1, ?2, ?3)",
        (id, name, icon),
    )?;
    Ok(())
}

pub fn update_envelope(
    conn: &Connection,
    id: &str,
    name: &str,
    icon: Option<&str>,
) -> anyhow::Result<()> {
    conn.execute(
        "UPDATE envelopes SET name = ?1, icon = ?2 WHERE id = ?3",
        (name, icon, id),
    )?;
    Ok(())
}

pub fn delete_envelope(conn: &Connection, id: &str) -> anyhow::Result<()> {
    let count = conn.execute("DELETE FROM envelopes WHERE id = ?1", (id,))?;

    if count == 0 {
        return Err(anyhow::anyhow!(
            "Envelope with id '{}' not found or not deleted",
            id
        ));
    }

    Ok(())
}

pub fn get_envelopes(conn: &Connection) -> anyhow::Result<Vec<Envelope>> {
    let mut stmt = conn.prepare("SELECT id, name, icon FROM envelopes")?;
    let rows = stmt.query_map([], |row| {
        Ok(Envelope {
            id: row.get(0)?,
            name: row.get(1)?,
            icon: row.get(2)?,
        })
    })?;

    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

pub fn assign_chat_to_envelope(
    conn: &Connection,
    chat_id: &str,
    envelope_id: Option<&str>,
) -> anyhow::Result<()> {
    // If envelope_id is None, remove assignment (move to root)
    if let Some(env_id) = envelope_id {
        conn.execute(
            "INSERT OR REPLACE INTO chat_envelopes (chat_id, envelope_id) VALUES (?1, ?2)",
            (chat_id, env_id),
        )?;
    } else {
        conn.execute("DELETE FROM chat_envelopes WHERE chat_id = ?1", (chat_id,))?;
    }
    Ok(())
}

pub fn get_chat_assignments(conn: &Connection) -> anyhow::Result<Vec<ChatAssignment>> {
    let mut stmt = conn.prepare("SELECT chat_id, envelope_id FROM chat_envelopes")?;
    let rows = stmt.query_map([], |row| {
        Ok(ChatAssignment {
            chat_id: row.get(0)?,
            envelope_id: row.get(1)?,
        })
    })?;

    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}
