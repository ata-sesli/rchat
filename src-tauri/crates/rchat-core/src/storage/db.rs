use rusqlite::{Connection, OptionalExtension};
// use std::path::Path; // Unused
use anyhow::Context;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

// --- 1. Rust Structs (Data Models) ---

#[derive(Debug, Serialize, Deserialize)]
pub struct Peer {
    pub id: String,
    pub alias: String,
    pub last_seen: i64, // Unix Timestamp
    pub public_key: Vec<u8>,
    pub method: String, // "local", "gist", "manual", etc.
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Message {
    pub id: String,
    pub chat_id: String,
    pub peer_id: String,
    pub timestamp: i64,
    pub content_type: String, // 'text', 'photo', 'video', 'document', 'audio'
    pub text_content: Option<String>,
    pub file_hash: Option<String>,
    pub status: String,                   // 'pending', 'delivered', 'read'
    pub content_metadata: Option<String>, // JSON: {"width": 1920, "height": 1080, ...}
    pub sender_alias: Option<String>,     // Sender's display name
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

#[derive(Debug, Serialize, Deserialize)]
pub struct Sticker {
    pub file_hash: String,
    pub name: Option<String>,
    pub created_at: i64,
    pub size_bytes: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChatListItem {
    pub id: String,
    pub name: String,
    pub is_group: bool,
    pub image_hash: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GroupInviteRow {
    pub invite_id: String,
    pub group_id: String,
    pub group_name: String,
    pub inviter_peer_id: String,
    pub invitee_peer_id: String,
    pub status: String,
    pub payload_json: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GroupFileSource {
    pub group_id: String,
    pub file_hash: String,
    pub peer_id: String,
    pub updated_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct GroupMemberRow {
    pub peer_id: String,
    pub display_name: String,
    pub role: String,
    pub membership_state: String,
    pub invited_by: Option<String>,
    pub last_event_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct GroupMessageReceiptRow {
    pub message_id: String,
    pub group_id: String,
    pub peer_id: String,
    pub display_name: String,
    pub status: String,
    pub timestamp: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq, Eq)]
pub struct GroupPendingRecordSummary {
    pub count: i64,
    pub oldest_received_at: Option<i64>,
    pub record_types: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ChatConnectionStats {
    pub first_connected_at: Option<i64>,
    pub last_connected_at: Option<i64>,
    pub reconnect_count: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ChatContentBreakdown {
    pub text: i64,
    pub sticker: i64,
    pub image: i64,
    pub video: i64,
    pub audio: i64,
    pub document: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ChatMessageStats {
    pub sent_total: i64,
    pub received_total: i64,
    pub sent: ChatContentBreakdown,
    pub received: ChatContentBreakdown,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatFileRow {
    pub message_id: String,
    pub timestamp: i64,
    pub content_type: String,
    pub file_hash: String,
    pub file_name: Option<String>,
    pub size_bytes: Option<i64>,
    pub mime_type: Option<String>,
    pub sender: String,
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
pub(crate) fn create_tables(conn: &Connection) -> anyhow::Result<()> {
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
             encryption_key BLOB NOT NULL,
             image_hash TEXT
         )",
        [],
    )?;
    let _ = conn.execute("ALTER TABLE chats ADD COLUMN image_hash TEXT", []);

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

    let _ = conn.execute(
        "ALTER TABLE chat_peers ADD COLUMN membership_state TEXT NOT NULL DEFAULT 'joined'",
        [],
    );
    let _ = conn.execute("ALTER TABLE chat_peers ADD COLUMN invited_by TEXT", []);
    let _ = conn.execute("ALTER TABLE chat_peers ADD COLUMN last_event_id TEXT", []);

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

    // 5b. Stickers (local sticker library registry)
    conn.execute(
        "CREATE TABLE IF NOT EXISTS stickers (
             file_hash TEXT NOT NULL PRIMARY KEY,
             name TEXT,
             created_at INTEGER NOT NULL,
             source TEXT NOT NULL DEFAULT 'local',
             FOREIGN KEY (file_hash) REFERENCES files(file_hash) ON DELETE CASCADE
         )",
        [],
    )?;

    // 5c. Per-chat durable connection stats
    conn.execute(
        "CREATE TABLE IF NOT EXISTS chat_connection_stats (
             chat_id TEXT NOT NULL PRIMARY KEY,
             first_connected_at INTEGER,
             last_connected_at INTEGER,
             reconnect_count INTEGER NOT NULL DEFAULT 0
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
    let _ = conn.execute("ALTER TABLE messages ADD COLUMN content_metadata TEXT", []);

    // Migration: Add sender_alias column for display name from messages
    let _ = conn.execute("ALTER TABLE messages ADD COLUMN sender_alias TEXT", []);

    conn.execute(
        "CREATE TABLE IF NOT EXISTS group_records (
             id TEXT NOT NULL PRIMARY KEY,
             group_id TEXT NOT NULL,
             record_type TEXT NOT NULL,
             author_peer_id TEXT NOT NULL,
             timestamp INTEGER NOT NULL,
             payload_json TEXT NOT NULL,
             public_key_b64 TEXT NOT NULL,
             signature_b64 TEXT NOT NULL,
             verified INTEGER NOT NULL DEFAULT 0,
             pending INTEGER NOT NULL DEFAULT 0,
             received_at INTEGER NOT NULL
         )",
        [],
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS group_invites (
             invite_id TEXT NOT NULL PRIMARY KEY,
             group_id TEXT NOT NULL,
             group_name TEXT NOT NULL,
             inviter_peer_id TEXT NOT NULL,
             invitee_peer_id TEXT NOT NULL,
             status TEXT NOT NULL,
             payload_json TEXT NOT NULL,
             created_at INTEGER NOT NULL,
             updated_at INTEGER NOT NULL
         )",
        [],
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS group_message_receipts (
             message_id TEXT NOT NULL,
             group_id TEXT NOT NULL,
             peer_id TEXT NOT NULL,
             status TEXT NOT NULL,
             timestamp INTEGER NOT NULL,
             PRIMARY KEY (message_id, peer_id)
         )",
        [],
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS group_file_sources (
             group_id TEXT NOT NULL,
             file_hash TEXT NOT NULL,
             peer_id TEXT NOT NULL,
             updated_at INTEGER NOT NULL,
             PRIMARY KEY (group_id, file_hash, peer_id)
         )",
        [],
    )?;

    // Migration: hard-cut legacy voice content type to canonical audio
    let _ = conn.execute(
        "UPDATE messages SET content_type = 'audio' WHERE content_type = 'voice'",
        [],
    );

    // Migration: Add source column to stickers table if missing
    let _ = conn.execute(
        "ALTER TABLE stickers ADD COLUMN source TEXT NOT NULL DEFAULT 'local'",
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

    // Speed up sticker list ordering
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_stickers_created_at ON stickers(created_at DESC)",
        [],
    )?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_chat_connection_stats_last_connected
         ON chat_connection_stats(last_connected_at DESC)",
        [],
    )?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_group_records_group_ts
         ON group_records(group_id, timestamp)",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_group_invites_status
         ON group_invites(status)",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_group_file_sources_lookup
         ON group_file_sources(group_id, file_hash)",
        [],
    )?;

    // known_devices index removed - table no longer exists

    // Hard cutover: remove legacy accidental "General" chat data.
    remove_legacy_general_data(conn)?;

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

fn remove_legacy_general_data(conn: &Connection) -> anyhow::Result<()> {
    conn.execute(
        "DELETE FROM messages WHERE chat_id = 'General' OR peer_id = 'General'",
        [],
    )?;
    conn.execute(
        "DELETE FROM chat_peers WHERE chat_id = 'General' OR peer_id = 'General'",
        [],
    )?;
    conn.execute("DELETE FROM chat_envelopes WHERE chat_id = 'General'", [])?;
    conn.execute("DELETE FROM chats WHERE id = 'General'", [])?;
    conn.execute("DELETE FROM peers WHERE id = 'General'", [])?;
    Ok(())
}

fn merge_chat_connection_stats(
    tx: &rusqlite::Transaction<'_>,
    from_chat_id: &str,
    to_chat_id: &str,
) -> anyhow::Result<()> {
    let from_stats = get_chat_connection_stats(tx, from_chat_id)?;
    let to_stats = get_chat_connection_stats(tx, to_chat_id)?;

    let first_connected_at = match (from_stats.first_connected_at, to_stats.first_connected_at) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };
    let last_connected_at = match (from_stats.last_connected_at, to_stats.last_connected_at) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };
    let reconnect_count = from_stats
        .reconnect_count
        .saturating_add(to_stats.reconnect_count);

    tx.execute(
        "INSERT INTO chat_connection_stats (chat_id, first_connected_at, last_connected_at, reconnect_count)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(chat_id) DO UPDATE SET
             first_connected_at = excluded.first_connected_at,
             last_connected_at = excluded.last_connected_at,
             reconnect_count = excluded.reconnect_count",
        rusqlite::params![
            to_chat_id,
            first_connected_at,
            last_connected_at,
            reconnect_count
        ],
    )?;

    if from_chat_id != to_chat_id {
        tx.execute(
            "DELETE FROM chat_connection_stats WHERE chat_id = ?1",
            [from_chat_id],
        )?;
    }

    Ok(())
}

fn migrate_chat_id_references(
    tx: &rusqlite::Transaction<'_>,
    old_chat_id: &str,
    new_chat_id: &str,
) -> anyhow::Result<()> {
    if old_chat_id == new_chat_id {
        return Ok(());
    }

    let old_chat_row = tx
        .query_row(
            "SELECT name, is_group, encryption_key FROM chats WHERE id = ?1",
            [old_chat_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            },
        )
        .optional()?;

    let Some((old_name, old_is_group, old_encryption_key)) = old_chat_row else {
        return Ok(());
    };

    let new_chat_exists = chat_exists(tx, new_chat_id);
    if !new_chat_exists {
        tx.execute(
            "INSERT INTO chats (id, name, is_group, encryption_key) VALUES (?1, ?2, ?3, ?4)",
            (
                new_chat_id,
                old_name,
                old_is_group,
                old_encryption_key.clone(),
            ),
        )?;
    }

    tx.execute(
        "UPDATE messages SET chat_id = ?1 WHERE chat_id = ?2",
        (new_chat_id, old_chat_id),
    )?;

    tx.execute(
        "INSERT OR IGNORE INTO chat_peers (chat_id, peer_id, role, joined_at)
         SELECT ?1, peer_id, role, joined_at
         FROM chat_peers
         WHERE chat_id = ?2",
        (new_chat_id, old_chat_id),
    )?;
    tx.execute("DELETE FROM chat_peers WHERE chat_id = ?1", [old_chat_id])?;

    let old_envelope = tx
        .query_row(
            "SELECT envelope_id FROM chat_envelopes WHERE chat_id = ?1",
            [old_chat_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let new_envelope_exists = tx
        .query_row(
            "SELECT 1 FROM chat_envelopes WHERE chat_id = ?1",
            [new_chat_id],
            |_| Ok(()),
        )
        .is_ok();
    if let Some(envelope_id) = old_envelope {
        if !new_envelope_exists {
            tx.execute(
                "INSERT OR REPLACE INTO chat_envelopes (chat_id, envelope_id) VALUES (?1, ?2)",
                (new_chat_id, envelope_id),
            )?;
        }
        tx.execute(
            "DELETE FROM chat_envelopes WHERE chat_id = ?1",
            [old_chat_id],
        )?;
    }

    merge_chat_connection_stats(tx, old_chat_id, new_chat_id)?;
    tx.execute("DELETE FROM chats WHERE id = ?1", [old_chat_id])?;
    Ok(())
}

fn migrate_peer_id_reference(
    tx: &rusqlite::Transaction<'_>,
    old_peer_id: &str,
    new_peer_id: &str,
) -> anyhow::Result<()> {
    if old_peer_id == new_peer_id {
        return Ok(());
    }

    let old_peer = tx
        .query_row(
            "SELECT alias, last_seen, public_key, method FROM peers WHERE id = ?1",
            [old_peer_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()?;
    let Some((alias, last_seen, public_key, method)) = old_peer else {
        return Ok(());
    };

    if !is_peer(tx, new_peer_id) {
        tx.execute(
            "INSERT INTO peers (id, alias, last_seen, public_key, method) VALUES (?1, ?2, ?3, ?4, ?5)",
            (new_peer_id, alias, last_seen, public_key, method),
        )?;
    }

    tx.execute(
        "UPDATE messages SET peer_id = ?1 WHERE peer_id = ?2",
        (new_peer_id, old_peer_id),
    )?;

    tx.execute(
        "INSERT OR IGNORE INTO chat_peers (chat_id, peer_id, role, joined_at)
         SELECT chat_id, ?1, role, joined_at
         FROM chat_peers
         WHERE peer_id = ?2",
        (new_peer_id, old_peer_id),
    )?;
    tx.execute("DELETE FROM chat_peers WHERE peer_id = ?1", [old_peer_id])?;
    tx.execute("DELETE FROM peers WHERE id = ?1", [old_peer_id])?;
    Ok(())
}

fn migrate_legacy_github_chat_id_inner(
    tx: &rusqlite::Transaction<'_>,
    github_username: &str,
    peer_id: &str,
) -> anyhow::Result<()> {
    let old_chat_id = format!("gh:{}", github_username);
    let new_chat_id = crate::chat_identity::build_github_chat_id(github_username, peer_id);

    migrate_chat_id_references(tx, &old_chat_id, &new_chat_id)?;
    migrate_peer_id_reference(tx, &old_chat_id, &new_chat_id)?;

    Ok(())
}

pub fn migrate_legacy_github_chat_ids(
    conn: &mut Connection,
    github_peer_mapping: &std::collections::HashMap<String, String>,
) -> anyhow::Result<()> {
    if github_peer_mapping.is_empty() {
        return Ok(());
    }

    let tx = conn.transaction()?;
    for (github_username, peer_id) in github_peer_mapping {
        migrate_legacy_github_chat_id_inner(&tx, github_username, peer_id)?;
    }
    tx.commit()?;
    Ok(())
}

pub fn find_existing_local_chat_id_for_peer(
    conn: &Connection,
    peer_id: &str,
) -> anyhow::Result<Option<String>> {
    let mut stmt = conn.prepare(
        "SELECT id
         FROM chats
         WHERE is_group = 0
           AND id LIKE ?1
         ORDER BY id ASC
         LIMIT 1",
    )?;
    stmt.query_row([format!("lh:%-{}", peer_id)], |row| row.get(0))
        .optional()
        .map_err(Into::into)
}

pub fn find_existing_github_chat_id_for_peer(
    conn: &Connection,
    peer_id: &str,
) -> anyhow::Result<Option<String>> {
    let mut stmt = conn.prepare(
        "SELECT id
         FROM chats
         WHERE is_group = 0
           AND id LIKE ?1
         ORDER BY id ASC
         LIMIT 1",
    )?;
    stmt.query_row([format!("gh:%-{}", peer_id)], |row| row.get(0))
        .optional()
        .map_err(Into::into)
}

pub fn find_existing_direct_chat_id_for_peer(
    conn: &Connection,
    peer_id: &str,
) -> anyhow::Result<Option<String>> {
    if let Some(gh) = find_existing_github_chat_id_for_peer(conn, peer_id)? {
        return Ok(Some(gh));
    }
    if let Some(lh) = find_existing_local_chat_id_for_peer(conn, peer_id)? {
        return Ok(Some(lh));
    }
    if chat_exists(conn, peer_id) {
        return Ok(Some(peer_id.to_string()));
    }
    Ok(None)
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

/// Update the user-facing alias for an existing peer.
pub fn update_peer_alias(conn: &Connection, peer_id: &str, alias: &str) -> anyhow::Result<bool> {
    let alias = alias.trim();
    if alias.is_empty() {
        return Err(anyhow::anyhow!("Peer alias cannot be empty"));
    }
    let updated = conn.execute(
        "UPDATE peers SET alias = ?2 WHERE id = ?1",
        (peer_id, alias),
    )?;
    Ok(updated > 0)
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

pub fn upsert_chat(
    conn: &Connection,
    chat_id: &str,
    name: &str,
    is_group: bool,
) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO chats (id, name, is_group, encryption_key) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(id) DO UPDATE SET
             name = excluded.name,
             is_group = excluded.is_group",
        (chat_id, name, if is_group { 1 } else { 0 }, vec![0u8; 32]),
    )?;
    Ok(())
}

pub fn update_chat_image_hash(
    conn: &Connection,
    chat_id: &str,
    image_hash: Option<&str>,
) -> anyhow::Result<()> {
    conn.execute(
        "UPDATE chats SET image_hash = ?2 WHERE id = ?1",
        rusqlite::params![chat_id, image_hash],
    )?;
    Ok(())
}

pub fn add_chat_member(
    conn: &Connection,
    chat_id: &str,
    peer_id: &str,
    role: &str,
) -> anyhow::Result<()> {
    let joined_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    conn.execute(
        "INSERT OR IGNORE INTO chat_peers (chat_id, peer_id, role, joined_at)
         VALUES (?1, ?2, ?3, ?4)",
        (chat_id, peer_id, role, joined_at),
    )?;
    Ok(())
}

pub fn upsert_chat_member_state(
    conn: &Connection,
    chat_id: &str,
    peer_id: &str,
    role: &str,
    membership_state: &str,
    invited_by: Option<&str>,
    last_event_id: Option<&str>,
) -> anyhow::Result<()> {
    let joined_at = unix_now();
    conn.execute(
        "INSERT INTO chat_peers
             (chat_id, peer_id, role, joined_at, membership_state, invited_by, last_event_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(chat_id, peer_id) DO UPDATE SET
             role = excluded.role,
             membership_state = excluded.membership_state,
             invited_by = COALESCE(excluded.invited_by, chat_peers.invited_by),
             last_event_id = COALESCE(excluded.last_event_id, chat_peers.last_event_id)",
        (
            chat_id,
            peer_id,
            role,
            joined_at,
            membership_state,
            invited_by,
            last_event_id,
        ),
    )?;
    Ok(())
}

pub fn remove_chat_member(conn: &Connection, chat_id: &str, peer_id: &str) -> anyhow::Result<()> {
    conn.execute(
        "DELETE FROM chat_peers WHERE chat_id = ?1 AND peer_id = ?2",
        (chat_id, peer_id),
    )?;
    Ok(())
}

pub fn revoke_group_invites(conn: &Connection, group_id: &str) -> anyhow::Result<()> {
    conn.execute(
        "UPDATE group_invites SET status = 'revoked', updated_at = ?2 WHERE group_id = ?1 AND status IN ('sent', 'pending', 'ready')",
        rusqlite::params![group_id, unix_now()],
    )?;
    Ok(())
}

pub fn get_open_group_invitee_peer_ids(
    conn: &Connection,
    group_id: &str,
) -> anyhow::Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT invitee_peer_id FROM group_invites WHERE group_id = ?1 AND status IN ('sent', 'pending', 'ready') ORDER BY invitee_peer_id",
    )?;
    let rows = stmt.query_map([group_id], |row| row.get::<_, String>(0))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn get_group_invitee_peer_ids_for_sync(
    conn: &Connection,
    group_id: &str,
) -> anyhow::Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT invitee_peer_id FROM group_invites
         WHERE group_id = ?1 AND status IN ('sent', 'pending', 'revoked')
         ORDER BY invitee_peer_id",
    )?;
    let rows = stmt.query_map([group_id], |row| row.get::<_, String>(0))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn delete_group_chat(conn: &Connection, chat_id: &str) -> anyhow::Result<()> {
    conn.execute("DELETE FROM messages WHERE chat_id = ?1", [chat_id])?;
    conn.execute("DELETE FROM chat_envelopes WHERE chat_id = ?1", [chat_id])?;
    conn.execute("DELETE FROM chat_peers WHERE chat_id = ?1", [chat_id])?;
    conn.execute(
        "DELETE FROM chats WHERE id = ?1 AND is_group = 1",
        [chat_id],
    )?;
    Ok(())
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn insert_group_record(
    conn: &Connection,
    record: &crate::network::gossip::SignedGroupRecord,
    verified: bool,
    pending: bool,
) -> anyhow::Result<bool> {
    let payload_json = serde_json::to_string(record)?;
    let inserted = conn.execute(
        "INSERT OR IGNORE INTO group_records
         (id, group_id, record_type, author_peer_id, timestamp, payload_json, public_key_b64, signature_b64, verified, pending, received_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        (
            record.id(),
            record.group_id(),
            record.body().kind(),
            record.author_peer_id(),
            record.timestamp(),
            payload_json,
            &record.public_key_b64,
            &record.signature_b64,
            if verified { 1 } else { 0 },
            if pending { 1 } else { 0 },
            unix_now(),
        ),
    )?;
    Ok(inserted > 0)
}

pub fn replace_group_record(
    conn: &Connection,
    record: &crate::network::gossip::SignedGroupRecord,
    verified: bool,
    pending: bool,
) -> anyhow::Result<()> {
    let payload_json = serde_json::to_string(record)?;
    conn.execute(
        "UPDATE group_records SET
             group_id = ?2,
             record_type = ?3,
             author_peer_id = ?4,
             timestamp = ?5,
             payload_json = ?6,
             public_key_b64 = ?7,
             signature_b64 = ?8,
             verified = ?9,
             pending = ?10,
             received_at = ?11
         WHERE id = ?1",
        (
            record.id(),
            record.group_id(),
            record.body().kind(),
            record.author_peer_id(),
            record.timestamp(),
            payload_json,
            &record.public_key_b64,
            &record.signature_b64,
            if verified { 1 } else { 0 },
            if pending { 1 } else { 0 },
            unix_now(),
        ),
    )?;
    Ok(())
}

pub fn group_record_exists(conn: &Connection, record_id: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM group_records WHERE id = ?1",
        [record_id],
        |_| Ok(()),
    )
    .is_ok()
}

pub fn get_group_record(
    conn: &Connection,
    record_id: &str,
) -> anyhow::Result<Option<crate::network::gossip::SignedGroupRecord>> {
    conn.query_row(
        "SELECT payload_json FROM group_records WHERE id = ?1",
        [record_id],
        |row| row.get::<_, String>(0),
    )
    .optional()?
    .map(|json| serde_json::from_str(&json).map_err(Into::into))
    .transpose()
}

pub fn get_group_records_for_sync(
    conn: &Connection,
    group_id: &str,
    cursor: Option<&crate::network::gossip::GroupSyncCursor>,
    wanted_record_ids: &[String],
    limit: usize,
) -> anyhow::Result<(
    Vec<crate::network::gossip::SignedGroupRecord>,
    Option<crate::network::gossip::GroupSyncCursor>,
    bool,
)> {
    let mut stmt = conn.prepare(
        "SELECT payload_json
         FROM group_records
         WHERE group_id = ?1 AND verified = 1 AND pending = 0",
    )?;
    let rows = stmt.query_map([group_id], |row| row.get::<_, String>(0))?;
    let mut records = rows
        .map(|row| serde_json::from_str(&row?).map_err(Into::into))
        .collect::<anyhow::Result<Vec<crate::network::gossip::SignedGroupRecord>>>()?;
    records.sort_by_key(crate::network::gossip::GroupSyncCursor::from_record);

    let limit = limit.max(1);
    let wanted = wanted_record_ids
        .iter()
        .take(crate::network::gossip::MAX_GROUP_SYNC_WANTED_IDS)
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    let wanted_records = records
        .iter()
        .filter(|record| wanted.contains(record.id()))
        .collect::<Vec<_>>();

    // Explicit dependency requests are their own phase. Mixing an out-of-order
    // dependency into a cursor page would make the next page repeat that record
    // (or skip records between it and the cursor).
    if !wanted_records.is_empty() {
        let sequential_remaining = records
            .iter()
            .filter(|record| {
                cursor.is_none_or(|cursor| {
                    crate::network::gossip::GroupSyncCursor::from_record(record) > *cursor
                }) && !wanted.contains(record.id())
            })
            .count();
        return Ok((
            wanted_records
                .into_iter()
                .take(limit)
                .map(|record| (*record).clone())
                .collect(),
            cursor.cloned(),
            sequential_remaining > 0,
        ));
    }

    let sequential = records
        .iter()
        .filter(|record| {
            cursor.is_none_or(|cursor| {
                crate::network::gossip::GroupSyncCursor::from_record(record) > *cursor
            })
        })
        .collect::<Vec<_>>();
    let sequential_sent = limit.min(sequential.len());
    let out = sequential
        .iter()
        .take(sequential_sent)
        .map(|record| (*record).clone())
        .collect::<Vec<_>>();
    let next_cursor = sequential_sent
        .checked_sub(1)
        .and_then(|index| sequential.get(index))
        .map(|record| crate::network::gossip::GroupSyncCursor::from_record(record))
        .or_else(|| cursor.cloned());
    let has_more = sequential_sent < sequential.len();

    Ok((out, next_cursor, has_more))
}

pub fn get_all_group_records(
    conn: &Connection,
    group_id: &str,
) -> anyhow::Result<Vec<crate::network::gossip::SignedGroupRecord>> {
    let mut stmt = conn.prepare(
        "SELECT payload_json
         FROM group_records
         WHERE group_id = ?1 AND verified = 1
         ORDER BY id ASC",
    )?;
    let rows = stmt.query_map([group_id], |row| row.get::<_, String>(0))?;
    rows.map(|row| serde_json::from_str(&row?).map_err(Into::into))
        .collect()
}

pub fn get_group_record_ids(conn: &Connection, group_id: &str) -> anyhow::Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT id FROM group_records WHERE group_id = ?1 AND verified = 1 ORDER BY timestamp ASC",
    )?;
    let rows = stmt.query_map([group_id], |row| row.get::<_, String>(0))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

pub fn get_group_pending_record_summary(
    conn: &Connection,
    group_id: &str,
) -> anyhow::Result<GroupPendingRecordSummary> {
    let (count, oldest_received_at) = conn.query_row(
        "SELECT COUNT(*), MIN(received_at)
         FROM group_records
         WHERE group_id = ?1 AND pending = 1",
        [group_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let mut stmt = conn.prepare(
        "SELECT DISTINCT record_type
         FROM group_records
         WHERE group_id = ?1 AND pending = 1
         ORDER BY record_type ASC",
    )?;
    let rows = stmt.query_map([group_id], |row| row.get::<_, String>(0))?;
    let mut record_types = Vec::new();
    for row in rows {
        record_types.push(row?);
    }
    Ok(GroupPendingRecordSummary {
        count,
        oldest_received_at,
        record_types,
    })
}

pub fn upsert_group_invite(
    conn: &Connection,
    invite: &crate::network::gossip::GroupInvitePayload,
    status: &str,
) -> anyhow::Result<()> {
    let payload_json = serde_json::to_string(invite)?;
    let now = unix_now();
    conn.execute(
        "INSERT INTO group_invites
         (invite_id, group_id, group_name, inviter_peer_id, invitee_peer_id, status, payload_json, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(invite_id) DO UPDATE SET
             status = CASE
                 WHEN group_invites.status IN ('accepted', 'rejected', 'revoked', 'ready')
                     THEN group_invites.status
                 ELSE excluded.status
             END,
             payload_json = excluded.payload_json,
             updated_at = excluded.updated_at",
        (
            &invite.invite_id,
            &invite.group_id,
            &invite.group_name,
            &invite.inviter_peer_id,
            &invite.invitee_peer_id,
            status,
            payload_json,
            invite.created_at,
            now,
        ),
    )?;
    Ok(())
}

pub fn update_group_invite_status(
    conn: &Connection,
    invite_id: &str,
    status: &str,
) -> anyhow::Result<()> {
    conn.execute(
        "UPDATE group_invites SET status = ?1, updated_at = ?2 WHERE invite_id = ?3",
        (status, unix_now(), invite_id),
    )?;
    Ok(())
}

pub fn get_group_invite_status(
    conn: &Connection,
    invite_id: &str,
) -> anyhow::Result<Option<String>> {
    conn.query_row(
        "SELECT status FROM group_invites WHERE invite_id = ?1",
        [invite_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

pub fn get_group_invites_for_group(
    conn: &Connection,
    group_id: &str,
) -> anyhow::Result<Vec<GroupInviteRow>> {
    let mut statement = conn.prepare(
        "SELECT invite_id, group_id, group_name, inviter_peer_id, invitee_peer_id,
                status, payload_json, created_at, updated_at
         FROM group_invites
         WHERE group_id = ?1 AND status IN ('sent', 'pending', 'ready')
         ORDER BY created_at ASC, invite_id ASC",
    )?;
    let rows = statement.query_map([group_id], |row| {
        Ok(GroupInviteRow {
            invite_id: row.get(0)?,
            group_id: row.get(1)?,
            group_name: row.get(2)?,
            inviter_peer_id: row.get(3)?,
            invitee_peer_id: row.get(4)?,
            status: row.get(5)?,
            payload_json: row.get(6)?,
            created_at: row.get(7)?,
            updated_at: row.get(8)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn get_open_group_invites_for_peer(
    conn: &Connection,
    invitee_peer_id: &str,
) -> anyhow::Result<Vec<GroupInviteRow>> {
    let mut statement = conn.prepare(
        "SELECT invite_id, group_id, group_name, inviter_peer_id, invitee_peer_id,
                status, payload_json, created_at, updated_at
         FROM group_invites
         WHERE invitee_peer_id = ?1 AND status IN ('pending', 'ready')
         ORDER BY created_at ASC, invite_id ASC",
    )?;
    let rows = statement.query_map([invitee_peer_id], |row| {
        Ok(GroupInviteRow {
            invite_id: row.get(0)?,
            group_id: row.get(1)?,
            group_name: row.get(2)?,
            inviter_peer_id: row.get(3)?,
            invitee_peer_id: row.get(4)?,
            status: row.get(5)?,
            payload_json: row.get(6)?,
            created_at: row.get(7)?,
            updated_at: row.get(8)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn get_group_invite_payload(
    conn: &Connection,
    invite_id: &str,
) -> anyhow::Result<Option<crate::network::gossip::GroupInvitePayload>> {
    conn.query_row(
        "SELECT payload_json FROM group_invites WHERE invite_id = ?1",
        [invite_id],
        |row| row.get::<_, String>(0),
    )
    .optional()?
    .map(|json| serde_json::from_str(&json).map_err(Into::into))
    .transpose()
}

pub fn upsert_group_message_receipt(
    conn: &Connection,
    group_id: &str,
    message_id: &str,
    peer_id: &str,
    status: &str,
    timestamp: i64,
) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO group_message_receipts (message_id, group_id, peer_id, status, timestamp)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(message_id, peer_id) DO UPDATE SET
             status = CASE
                 WHEN group_message_receipts.status = 'read' AND excluded.status = 'delivered'
                 THEN group_message_receipts.status
                 ELSE excluded.status
             END,
             timestamp = CASE
                 WHEN group_message_receipts.status = 'read' AND excluded.status = 'delivered'
                 THEN group_message_receipts.timestamp
                 ELSE excluded.timestamp
             END",
        (message_id, group_id, peer_id, status, timestamp),
    )?;
    Ok(())
}

pub fn get_group_message_receipts(
    conn: &Connection,
    group_id: &str,
    message_id: &str,
) -> anyhow::Result<Vec<GroupMessageReceiptRow>> {
    let mut stmt = conn.prepare(
        "SELECT r.message_id,
                r.group_id,
                r.peer_id,
                COALESCE(NULLIF(p.alias, ''), r.peer_id),
                r.status,
                r.timestamp
         FROM group_message_receipts r
         LEFT JOIN peers p ON p.id = r.peer_id
         WHERE r.group_id = ?1 AND r.message_id = ?2
         ORDER BY r.timestamp ASC, r.peer_id ASC",
    )?;
    let rows = stmt.query_map((group_id, message_id), |row| {
        Ok(GroupMessageReceiptRow {
            message_id: row.get(0)?,
            group_id: row.get(1)?,
            peer_id: row.get(2)?,
            display_name: row.get(3)?,
            status: row.get(4)?,
            timestamp: row.get(5)?,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

pub fn upsert_group_file_source(
    conn: &Connection,
    group_id: &str,
    file_hash: &str,
    peer_id: &str,
) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO group_file_sources (group_id, file_hash, peer_id, updated_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(group_id, file_hash, peer_id) DO UPDATE SET updated_at = excluded.updated_at",
        (group_id, file_hash, peer_id, unix_now()),
    )?;
    Ok(())
}

pub fn get_group_file_sources(
    conn: &Connection,
    group_id: &str,
    file_hash: &str,
) -> anyhow::Result<Vec<GroupFileSource>> {
    let mut stmt = conn.prepare(
        "SELECT group_id, file_hash, peer_id, updated_at
         FROM group_file_sources
         WHERE group_id = ?1 AND file_hash = ?2
         ORDER BY updated_at DESC",
    )?;
    let rows = stmt.query_map((group_id, file_hash), |row| {
        Ok(GroupFileSource {
            group_id: row.get(0)?,
            file_hash: row.get(1)?,
            peer_id: row.get(2)?,
            updated_at: row.get(3)?,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

pub fn get_group_ids_for_file_hash(
    conn: &Connection,
    file_hash: &str,
) -> anyhow::Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT m.chat_id
         FROM messages m
         INNER JOIN chats c ON c.id = m.chat_id
         WHERE m.file_hash = ?1 AND c.is_group = 1",
    )?;
    let rows = stmt.query_map([file_hash], |row| row.get::<_, String>(0))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

pub fn get_joined_group_chat_ids(
    conn: &Connection,
    my_peer_id: &str,
) -> anyhow::Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT c.id
         FROM chats c
         INNER JOIN chat_peers cp ON cp.chat_id = c.id
         WHERE c.is_group = 1 AND cp.peer_id = ?1",
    )?;
    let rows = stmt.query_map([my_peer_id], |row| row.get::<_, String>(0))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

pub fn get_group_member_peer_ids(
    conn: &Connection,
    group_id: &str,
) -> anyhow::Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT peer_id FROM chat_peers WHERE chat_id = ?1 AND peer_id != 'Me'",
    )?;
    let rows = stmt.query_map([group_id], |row| row.get::<_, String>(0))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

pub fn get_group_roster(conn: &Connection, group_id: &str) -> anyhow::Result<Vec<GroupMemberRow>> {
    let mut stmt = conn.prepare(
        "SELECT cp.peer_id,
                COALESCE(NULLIF(p.alias, ''), cp.peer_id),
                cp.role,
                cp.membership_state,
                cp.invited_by,
                cp.last_event_id
         FROM chat_peers cp
         LEFT JOIN peers p ON p.id = cp.peer_id
         WHERE cp.chat_id = ?1
         ORDER BY CASE cp.membership_state
                      WHEN 'joined' THEN 0
                      WHEN 'invited' THEN 1
                      ELSE 2
                  END,
                  CASE cp.role WHEN 'admin' THEN 0 ELSE 1 END,
                  COALESCE(NULLIF(p.alias, ''), cp.peer_id) COLLATE NOCASE ASC,
                  cp.peer_id ASC",
    )?;
    let rows = stmt.query_map([group_id], |row| {
        Ok(GroupMemberRow {
            peer_id: row.get(0)?,
            display_name: row.get(1)?,
            role: row.get(2)?,
            membership_state: row.get(3)?,
            invited_by: row.get(4)?,
            last_event_id: row.get(5)?,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

pub fn get_chat_list(conn: &Connection) -> anyhow::Result<Vec<ChatListItem>> {
    let mut items = Vec::new();
    let mut seen_ids: HashSet<String> = HashSet::new();

    let mut stmt = conn.prepare(
        "SELECT id, name, is_group, image_hash
         FROM chats",
    )?;
    let chat_rows = stmt.query_map([], |row| {
        Ok(ChatListItem {
            id: row.get(0)?,
            name: row.get(1)?,
            is_group: row.get::<_, i64>(2)? != 0,
            image_hash: row.get(3)?,
        })
    })?;

    for row in chat_rows {
        let item = row?;
        seen_ids.insert(item.id.clone());
        items.push(item);
    }

    // Include known peers without chat rows as direct chats.
    let mut peer_stmt = conn.prepare(
        "SELECT id, alias
         FROM peers
         WHERE id != 'Me' AND method != 'group'",
    )?;
    let peer_rows = peer_stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in peer_rows {
        let (peer_id, alias) = row?;
        let has_scoped_direct_chat = seen_ids.iter().any(|id| {
            (id.starts_with("gh:") || id.starts_with("lh:"))
                && id.ends_with(&format!("-{}", peer_id))
        });
        if !seen_ids.contains(&peer_id) && !has_scoped_direct_chat {
            items.push(ChatListItem {
                id: peer_id.clone(),
                name: alias,
                is_group: false,
                image_hash: None,
            });
            seen_ids.insert(peer_id);
        }
    }

    // Ensure self chat exists in list.
    if !seen_ids.contains("self") {
        items.push(ChatListItem {
            id: "self".to_string(),
            name: "Note to Self".to_string(),
            is_group: false,
            image_hash: None,
        });
    }

    Ok(items)
}

pub fn get_chat_name(conn: &Connection, chat_id: &str) -> anyhow::Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT name FROM chats WHERE id = ?1 LIMIT 1")?;
    let mut rows = stmt.query([chat_id])?;
    if let Some(row) = rows.next()? {
        return Ok(Some(row.get(0)?));
    }
    Ok(None)
}

pub fn get_group_image_hash(conn: &Connection, group_id: &str) -> anyhow::Result<Option<String>> {
    conn.query_row(
        "SELECT image_hash FROM chats WHERE id = ?1 AND is_group = 1",
        [group_id],
        |row| row.get::<_, Option<String>>(0),
    )
    .optional()
    .map(Option::flatten)
    .map_err(Into::into)
}

pub fn get_peer_alias(conn: &Connection, peer_id: &str) -> anyhow::Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT alias FROM peers WHERE id = ?1 LIMIT 1")?;
    let mut rows = stmt.query([peer_id])?;
    if let Some(row) = rows.next()? {
        return Ok(Some(row.get(0)?));
    }
    Ok(None)
}

pub fn record_chat_connection_established(
    conn: &Connection,
    chat_id: &str,
    connected_at: i64,
) -> anyhow::Result<()> {
    let existing = get_chat_connection_stats(conn, chat_id)?;
    match existing.first_connected_at {
        None => {
            conn.execute(
                "INSERT INTO chat_connection_stats (chat_id, first_connected_at, last_connected_at, reconnect_count)
                 VALUES (?1, ?2, ?3, 0)
                 ON CONFLICT(chat_id) DO UPDATE SET
                    first_connected_at = COALESCE(chat_connection_stats.first_connected_at, excluded.first_connected_at),
                    last_connected_at = excluded.last_connected_at,
                    reconnect_count = chat_connection_stats.reconnect_count",
                (chat_id, connected_at, connected_at),
            )?;
        }
        Some(_) => {
            conn.execute(
                "UPDATE chat_connection_stats
                 SET last_connected_at = ?2, reconnect_count = reconnect_count + 1
                 WHERE chat_id = ?1",
                (chat_id, connected_at),
            )?;
        }
    }

    Ok(())
}

pub fn get_chat_connection_stats(
    conn: &Connection,
    chat_id: &str,
) -> anyhow::Result<ChatConnectionStats> {
    let mut stmt = conn.prepare(
        "SELECT first_connected_at, last_connected_at, reconnect_count
         FROM chat_connection_stats
         WHERE chat_id = ?1",
    )?;
    let mut rows = stmt.query([chat_id])?;
    if let Some(row) = rows.next()? {
        return Ok(ChatConnectionStats {
            first_connected_at: row.get(0)?,
            last_connected_at: row.get(1)?,
            reconnect_count: row.get::<_, i64>(2)?,
        });
    }

    Ok(ChatConnectionStats::default())
}

/// Delete a peer and their related chat/messages
pub fn delete_peer(conn: &Connection, peer_id: &str) -> anyhow::Result<()> {
    conn.execute("DELETE FROM chat_peers WHERE peer_id = ?1", [peer_id])?;
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
pub fn update_content_metadata(
    conn: &Connection,
    msg_id: &str,
    metadata_json: &str,
) -> anyhow::Result<()> {
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
pub fn get_peer_aliases(
    conn: &Connection,
) -> anyhow::Result<std::collections::HashMap<String, String>> {
    let mut stmt = conn.prepare(
        "SELECT chat_id, sender_alias
         FROM messages
         WHERE sender_alias IS NOT NULL AND sender_alias != ''
           AND peer_id != 'Me'
         GROUP BY chat_id
         HAVING MAX(timestamp)",
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

pub fn mark_group_messages_read(conn: &Connection, chat_id: &str) -> anyhow::Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT id FROM messages WHERE chat_id = ?1 AND peer_id != 'Me' AND status != 'read'",
    )?;
    let ids: Vec<String> = stmt
        .query_map([chat_id], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();

    conn.execute(
        "UPDATE messages SET status = 'read' WHERE chat_id = ?1 AND peer_id != 'Me' AND status != 'read'",
        [chat_id],
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

pub fn get_chat_message_stats(
    conn: &Connection,
    chat_id: &str,
) -> anyhow::Result<ChatMessageStats> {
    let mut stmt = conn.prepare(
        "SELECT
            SUM(CASE WHEN peer_id = 'Me' THEN 1 ELSE 0 END) AS sent_total,
            SUM(CASE WHEN peer_id != 'Me' THEN 1 ELSE 0 END) AS received_total,
            SUM(CASE WHEN peer_id = 'Me' AND content_type = 'text' THEN 1 ELSE 0 END) AS sent_text,
            SUM(CASE WHEN peer_id = 'Me' AND content_type = 'sticker' THEN 1 ELSE 0 END) AS sent_sticker,
            SUM(CASE WHEN peer_id = 'Me' AND (content_type = 'image' OR content_type = 'photo') THEN 1 ELSE 0 END) AS sent_image,
            SUM(CASE WHEN peer_id = 'Me' AND content_type = 'video' THEN 1 ELSE 0 END) AS sent_video,
            SUM(CASE WHEN peer_id = 'Me' AND content_type = 'audio' THEN 1 ELSE 0 END) AS sent_audio,
            SUM(CASE WHEN peer_id = 'Me' AND content_type = 'document' THEN 1 ELSE 0 END) AS sent_document,
            SUM(CASE WHEN peer_id != 'Me' AND content_type = 'text' THEN 1 ELSE 0 END) AS recv_text,
            SUM(CASE WHEN peer_id != 'Me' AND content_type = 'sticker' THEN 1 ELSE 0 END) AS recv_sticker,
            SUM(CASE WHEN peer_id != 'Me' AND (content_type = 'image' OR content_type = 'photo') THEN 1 ELSE 0 END) AS recv_image,
            SUM(CASE WHEN peer_id != 'Me' AND content_type = 'video' THEN 1 ELSE 0 END) AS recv_video,
            SUM(CASE WHEN peer_id != 'Me' AND content_type = 'audio' THEN 1 ELSE 0 END) AS recv_audio,
            SUM(CASE WHEN peer_id != 'Me' AND content_type = 'document' THEN 1 ELSE 0 END) AS recv_document
         FROM messages
         WHERE chat_id = ?1",
    )?;

    let stats = stmt.query_row([chat_id], |row| {
        let sent_total = row.get::<_, Option<i64>>(0)?.unwrap_or(0);
        let received_total = row.get::<_, Option<i64>>(1)?.unwrap_or(0);
        Ok(ChatMessageStats {
            sent_total,
            received_total,
            sent: ChatContentBreakdown {
                text: row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                sticker: row.get::<_, Option<i64>>(3)?.unwrap_or(0),
                image: row.get::<_, Option<i64>>(4)?.unwrap_or(0),
                video: row.get::<_, Option<i64>>(5)?.unwrap_or(0),
                audio: row.get::<_, Option<i64>>(6)?.unwrap_or(0),
                document: row.get::<_, Option<i64>>(7)?.unwrap_or(0),
            },
            received: ChatContentBreakdown {
                text: row.get::<_, Option<i64>>(8)?.unwrap_or(0),
                sticker: row.get::<_, Option<i64>>(9)?.unwrap_or(0),
                image: row.get::<_, Option<i64>>(10)?.unwrap_or(0),
                video: row.get::<_, Option<i64>>(11)?.unwrap_or(0),
                audio: row.get::<_, Option<i64>>(12)?.unwrap_or(0),
                document: row.get::<_, Option<i64>>(13)?.unwrap_or(0),
            },
        })
    })?;

    Ok(stats)
}

pub fn list_chat_files(
    conn: &Connection,
    chat_id: &str,
    filter: &str,
    limit: i64,
    offset: i64,
) -> anyhow::Result<Vec<ChatFileRow>> {
    let safe_limit = limit.clamp(1, 200);
    let safe_offset = offset.max(0);
    let filter_lower = filter.to_lowercase();

    let mut stmt = conn.prepare(
        "SELECT
            m.id,
            m.timestamp,
            m.content_type,
            m.file_hash,
            COALESCE(f.file_name, m.text_content) AS file_name,
            f.size_bytes,
            f.mime_type,
            m.peer_id
         FROM messages m
         LEFT JOIN files f ON f.file_hash = m.file_hash
         WHERE m.chat_id = ?1
           AND m.file_hash IS NOT NULL
           AND (
               ?2 = 'all'
               OR (?2 = 'image' AND (m.content_type = 'image' OR m.content_type = 'photo'))
               OR m.content_type = ?2
           )
         ORDER BY m.timestamp DESC
         LIMIT ?3 OFFSET ?4",
    )?;

    let rows = stmt.query_map(
        rusqlite::params![chat_id, filter_lower, safe_limit, safe_offset],
        |row| {
            Ok(ChatFileRow {
                message_id: row.get(0)?,
                timestamp: row.get(1)?,
                content_type: row.get(2)?,
                file_hash: row.get(3)?,
                file_name: row.get(4)?,
                size_bytes: row.get(5)?,
                mime_type: row.get(6)?,
                sender: row.get(7)?,
            })
        },
    )?;

    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
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

pub fn sticker_exists(conn: &Connection, file_hash: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM stickers WHERE file_hash = ?1",
        [file_hash],
        |_| Ok(()),
    )
    .is_ok()
}

pub fn upsert_sticker(
    conn: &Connection,
    file_hash: &str,
    name: Option<&str>,
    source: &str,
) -> anyhow::Result<bool> {
    let already_exists = sticker_exists(conn, file_hash);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    conn.execute(
        "INSERT INTO stickers (file_hash, name, created_at, source)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(file_hash) DO UPDATE SET
            name = COALESCE(excluded.name, stickers.name),
            source = stickers.source",
        (file_hash, name, now, source),
    )?;

    Ok(!already_exists)
}

pub fn list_stickers(conn: &Connection) -> anyhow::Result<Vec<Sticker>> {
    let mut stmt = conn.prepare(
        "SELECT s.file_hash, s.name, s.created_at, COALESCE(f.size_bytes, 0) as size_bytes
         FROM stickers s
         LEFT JOIN files f ON f.file_hash = s.file_hash
         ORDER BY s.created_at DESC",
    )?;

    let rows = stmt.query_map([], |row| {
        Ok(Sticker {
            file_hash: row.get(0)?,
            name: row.get(1)?,
            created_at: row.get(2)?,
            size_bytes: row.get(3)?,
        })
    })?;

    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

pub fn delete_sticker(conn: &Connection, file_hash: &str) -> anyhow::Result<()> {
    let deleted = conn.execute("DELETE FROM stickers WHERE file_hash = ?1", [file_hash])?;
    if deleted == 0 {
        return Err(anyhow::anyhow!("Sticker not found: {}", file_hash));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::gossip::{GroupRecordBody, SignedGroupRecord};
    use libp2p::identity;

    fn signed_group_record(
        keypair: &identity::Keypair,
        group_id: &str,
        id: &str,
        timestamp: i64,
    ) -> SignedGroupRecord {
        SignedGroupRecord::new(
            keypair,
            group_id.to_string(),
            id.to_string(),
            timestamp,
            Vec::new(),
            1,
            GroupRecordBody::Head {
                heads: vec![id.to_string()],
            },
        )
        .expect("signed group record")
    }

    #[test]
    fn legacy_general_rows_are_removed() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        create_tables(&conn).expect("schema");

        conn.execute(
            "INSERT OR REPLACE INTO peers (id, alias, last_seen, public_key, method) VALUES ('General', 'General', 0, ?1, 'legacy')",
            [vec![0u8; 32]],
        )
        .expect("insert peer");
        conn.execute(
            "INSERT OR REPLACE INTO chats (id, name, is_group, encryption_key) VALUES ('General', 'General', 0, ?1)",
            [vec![0u8; 32]],
        )
        .expect("insert chat");
        conn.execute(
            "INSERT OR REPLACE INTO messages (id, chat_id, peer_id, timestamp, content_type, text_content, file_hash, status) VALUES ('m1', 'General', 'General', 1, 'text', 'hello', NULL, 'delivered')",
            [],
        )
        .expect("insert message");

        conn.execute(
            "INSERT OR REPLACE INTO envelopes (id, name, icon) VALUES ('env1', 'Env', NULL)",
            [],
        )
        .expect("insert envelope");
        conn.execute(
            "INSERT OR REPLACE INTO chat_envelopes (chat_id, envelope_id) VALUES ('General', 'env1')",
            [],
        )
        .expect("insert chat envelope");

        remove_legacy_general_data(&conn).expect("cleanup");

        let chat_exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM chats WHERE id='General')",
                [],
                |row| row.get(0),
            )
            .expect("check chat");
        let msg_exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM messages WHERE chat_id='General' OR peer_id='General')",
                [],
                |row| row.get(0),
            )
            .expect("check messages");
        assert!(!chat_exists);
        assert!(!msg_exists);
    }

    #[test]
    fn open_group_invites_are_scoped_to_the_invitee() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        create_tables(&conn).expect("schema");

        for (invite_id, invitee_peer_id, status) in [
            ("pending-local", "peer-local", "pending"),
            ("ready-local", "peer-local", "ready"),
            ("sent-local", "peer-local", "sent"),
            ("pending-other", "peer-other", "pending"),
        ] {
            conn.execute(
                "INSERT INTO group_invites
                 (invite_id, group_id, group_name, inviter_peer_id, invitee_peer_id,
                  status, payload_json, created_at, updated_at)
                 VALUES (?1, 'group:test', 'Test', 'peer-admin', ?2, ?3, '{}', 1, 1)",
                rusqlite::params![invite_id, invitee_peer_id, status],
            )
            .expect("insert invite");
        }

        let invites = get_open_group_invites_for_peer(&conn, "peer-local")
            .expect("query open local invites");
        assert_eq!(
            invites
                .iter()
                .map(|invite| invite.invite_id.as_str())
                .collect::<Vec<_>>(),
            vec!["pending-local", "ready-local"]
        );
    }

    #[test]
    fn connection_stats_increment_only_after_first_connect() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        create_tables(&conn).expect("schema");

        record_chat_connection_established(&conn, "peer-a", 10).expect("first connect");
        let first = get_chat_connection_stats(&conn, "peer-a").expect("read first");
        assert_eq!(first.first_connected_at, Some(10));
        assert_eq!(first.last_connected_at, Some(10));
        assert_eq!(first.reconnect_count, 0);

        record_chat_connection_established(&conn, "peer-a", 20).expect("reconnect");
        let second = get_chat_connection_stats(&conn, "peer-a").expect("read second");
        assert_eq!(second.first_connected_at, Some(10));
        assert_eq!(second.last_connected_at, Some(20));
        assert_eq!(second.reconnect_count, 1);
    }

    #[test]
    fn group_sync_pages_canonically_and_prioritizes_wanted_dependencies() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        create_tables(&conn).expect("schema");
        let keypair = identity::Keypair::generate_ed25519();
        let group_id = "group:test-sync-window";
        let first = signed_group_record(&keypair, group_id, "record-1", 1);
        let second = signed_group_record(&keypair, group_id, "record-2", 2);
        let third = signed_group_record(&keypair, group_id, "record-3", 3);
        let pending = signed_group_record(&keypair, group_id, "pending-record", 4);
        insert_group_record(&conn, &first, true, false).expect("insert first");
        insert_group_record(&conn, &second, true, false).expect("insert second");
        insert_group_record(&conn, &third, true, false).expect("insert third");
        insert_group_record(&conn, &pending, true, true).expect("insert pending");

        let wanted = vec![pending.id().to_string(), third.id().to_string()];
        let (records, next_cursor, has_more) =
            get_group_records_for_sync(&conn, group_id, None, &wanted, 2).expect("sync records");

        assert_eq!(
            records.iter().map(|record| record.id()).collect::<Vec<_>>(),
            vec![third.id()]
        );
        assert_eq!(next_cursor, None);
        assert!(has_more);

        let (records, next_cursor, has_more) =
            get_group_records_for_sync(&conn, group_id, next_cursor.as_ref(), &[], 2)
                .expect("next sync page");
        assert_eq!(
            records.iter().map(|record| record.id()).collect::<Vec<_>>(),
            vec![first.id(), second.id()]
        );
        assert_eq!(
            next_cursor,
            Some(crate::network::gossip::GroupSyncCursor::from_record(&second))
        );
        assert!(has_more);

        let (records, next_cursor, has_more) =
            get_group_records_for_sync(&conn, group_id, next_cursor.as_ref(), &[], 2)
                .expect("final sync page");
        assert_eq!(records.iter().map(|record| record.id()).collect::<Vec<_>>(), vec![third.id()]);
        assert_eq!(
            next_cursor,
            Some(crate::network::gossip::GroupSyncCursor::from_record(&third))
        );
        assert!(!has_more);
    }

    #[test]
    fn group_sync_pages_are_bounded_deduplicated_and_resumable() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        create_tables(&conn).expect("schema");
        let keypair = identity::Keypair::generate_ed25519();
        let group_id = "group:550e8400-e29b-41d4-a716-446655440000";
        for counter in 1..=300 {
            let record = signed_group_record(
                &keypair,
                group_id,
                &format!("record-{counter}"),
                counter,
            );
            insert_group_record(&conn, &record, true, false).expect("insert record");
        }

        let mut cursor = None;
        let mut seen = HashSet::new();
        let mut pages = 0usize;
        loop {
            let (records, next_cursor, has_more) =
                get_group_records_for_sync(&conn, group_id, cursor.as_ref(), &[], 64)
                    .expect("sync page");
            assert!(records.len() <= 64);
            assert!(records
                .iter()
                .all(|record| seen.insert(record.id().to_string())));
            pages += 1;
            assert!(pages <= 5);
            if !has_more {
                assert_eq!(records.len(), if pages == 5 { 44 } else { 0 });
                break;
            }
            let next_cursor = next_cursor.expect("next cursor");
            if let Some(previous) = cursor.as_ref() {
                assert!(next_cursor > *previous);
            }
            cursor = Some(next_cursor);
        }
        assert_eq!(seen.len(), 300);
    }

    #[test]
    fn group_sync_does_not_advance_cursor_past_unsent_sequential_records() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        create_tables(&conn).expect("schema");
        let keypair = identity::Keypair::generate_ed25519();
        let group_id = "group:550e8400-e29b-41d4-a716-446655440000";
        let first = signed_group_record(&keypair, group_id, "record-1", 1);
        let second = signed_group_record(&keypair, group_id, "record-2", 2);
        let third = signed_group_record(&keypair, group_id, "record-3", 3);
        insert_group_record(&conn, &first, true, false).expect("insert first");
        insert_group_record(&conn, &second, true, false).expect("insert second");
        insert_group_record(&conn, &third, true, false).expect("insert third");

        let wanted = vec![second.id().to_string(), third.id().to_string()];
        let (records, next_cursor, has_more) =
            get_group_records_for_sync(&conn, group_id, None, &wanted, 2).expect("wanted page");
        assert_eq!(
            records.iter().map(|record| record.id()).collect::<Vec<_>>(),
            vec![second.id(), third.id()]
        );
        assert_eq!(next_cursor, None);
        assert!(has_more);

        let (records, _, _) =
            get_group_records_for_sync(&conn, group_id, next_cursor.as_ref(), &[], 1)
                .expect("first sequential page");
        assert_eq!(records[0].id(), first.id());
    }

    #[test]
    fn group_inspection_queries_return_ordered_roster_receipts_and_pending_summary() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        create_tables(&conn).expect("schema");
        let group_id = "group:inspection";
        upsert_chat(&conn, group_id, "Inspection", true).expect("group chat");
        update_chat_image_hash(&conn, group_id, Some("group-image")).expect("group image");
        add_peer(&conn, "peer-admin", Some("Admin"), None, "group").expect("admin peer");
        add_peer(&conn, "peer-member", Some("Member"), None, "group").expect("member peer");
        add_peer(&conn, "peer-invited", Some("Invitee"), None, "group").expect("invited peer");
        upsert_chat_member_state(
            &conn,
            group_id,
            "peer-invited",
            "member",
            "invited",
            Some("peer-admin"),
            Some("invite-record"),
        )
        .expect("invited member");
        upsert_chat_member_state(
            &conn,
            group_id,
            "peer-member",
            "member",
            "joined",
            None,
            Some("join-record"),
        )
        .expect("joined member");
        upsert_chat_member_state(
            &conn,
            group_id,
            "peer-admin",
            "admin",
            "joined",
            None,
            Some("create-record"),
        )
        .expect("admin member");

        upsert_group_message_receipt(
            &conn,
            group_id,
            "message-1",
            "peer-member",
            "delivered",
            20,
        )
        .expect("member receipt");
        upsert_group_message_receipt(
            &conn,
            group_id,
            "message-1",
            "peer-admin",
            "read",
            10,
        )
        .expect("admin receipt");
        conn.execute(
            "INSERT INTO group_records
             (id, group_id, record_type, author_peer_id, timestamp, payload_json, public_key_b64, signature_b64, verified, pending, received_at)
             VALUES ('pending-message', ?1, 'message', 'peer-member', 1, '{}', '', '', 0, 1, 40),
                    ('pending-receipt', ?1, 'receipt', 'peer-member', 2, '{}', '', '', 0, 1, 30)",
            [group_id],
        )
        .expect("pending records");

        let roster = get_group_roster(&conn, group_id).expect("roster");
        assert_eq!(
            roster
                .iter()
                .map(|member| member.peer_id.as_str())
                .collect::<Vec<_>>(),
            vec!["peer-admin", "peer-member", "peer-invited"]
        );
        assert_eq!(roster[0].display_name, "Admin");
        assert_eq!(roster[0].role, "admin");
        assert_eq!(roster[2].membership_state, "invited");
        assert_eq!(roster[2].invited_by.as_deref(), Some("peer-admin"));
        assert_eq!(get_group_image_hash(&conn, group_id).expect("group image"), Some("group-image".to_string()));

        let receipts = get_group_message_receipts(&conn, group_id, "message-1")
            .expect("message receipts");
        assert_eq!(
            receipts
                .iter()
                .map(|receipt| receipt.peer_id.as_str())
                .collect::<Vec<_>>(),
            vec!["peer-admin", "peer-member"]
        );
        assert_eq!(receipts[0].display_name, "Admin");
        assert_eq!(receipts[0].status, "read");

        let pending = get_group_pending_record_summary(&conn, group_id).expect("pending summary");
        assert_eq!(pending.count, 2);
        assert_eq!(pending.oldest_received_at, Some(30));
        assert_eq!(pending.record_types, vec!["message", "receipt"]);
    }

    #[test]
    fn group_inspection_queries_return_empty_rows_for_unknown_group() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        create_tables(&conn).expect("schema");
        upsert_chat(&conn, "group:no-image", "No Image", true).expect("group chat");

        assert!(get_group_roster(&conn, "group:missing")
            .expect("roster")
            .is_empty());
        assert!(get_group_message_receipts(&conn, "group:missing", "message")
            .expect("receipts")
            .is_empty());
        assert_eq!(
            get_group_image_hash(&conn, "group:missing").expect("image"),
            None
        );
        assert_eq!(
            get_group_image_hash(&conn, "group:no-image").expect("empty image"),
            None
        );
        assert_eq!(
            get_group_pending_record_summary(&conn, "group:missing").expect("pending"),
            GroupPendingRecordSummary::default()
        );
    }

    #[test]
    fn chat_list_does_not_promote_group_only_peers_to_direct_chats() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        create_tables(&conn).expect("schema");
        add_peer(&conn, "peer-local", Some("Local Peer"), None, "local").expect("local peer");
        add_peer(&conn, "peer-group", Some("Group Peer"), None, "group").expect("group peer");

        let chats = get_chat_list(&conn).expect("chat list");
        let ids: Vec<String> = chats.into_iter().map(|chat| chat.id).collect();

        assert!(ids.contains(&"peer-local".to_string()));
        assert!(!ids.contains(&"peer-group".to_string()));
    }

    #[test]
    fn migrates_legacy_github_chat_id_to_canonical_format() {
        let mut conn = Connection::open_in_memory().expect("in-memory db");
        create_tables(&conn).expect("schema");

        let legacy_chat_id = "gh:professional-tester";
        let peer_id = "12D3KooWLk1GoEB3MbHbRLHTxXrvNGSxC2UALaCuKAgKuYXkXazU";
        let canonical_chat_id =
            crate::chat_identity::build_github_chat_id("professional-tester", peer_id);

        add_peer(
            &conn,
            legacy_chat_id,
            Some("professional-tester"),
            None,
            "github",
        )
        .expect("legacy peer");
        create_chat(&conn, legacy_chat_id, "professional-tester", false).expect("legacy chat");

        let msg = Message {
            id: "msg-1".to_string(),
            chat_id: legacy_chat_id.to_string(),
            peer_id: "Me".to_string(),
            timestamp: 1,
            content_type: "text".to_string(),
            text_content: Some("hello".to_string()),
            file_hash: None,
            status: "delivered".to_string(),
            content_metadata: None,
            sender_alias: None,
        };
        insert_message(&conn, &msg).expect("legacy message");

        let mapping = std::collections::HashMap::from([(
            "professional-tester".to_string(),
            peer_id.to_string(),
        )]);
        migrate_legacy_github_chat_ids(&mut conn, &mapping).expect("migration");

        assert!(!chat_exists(&conn, legacy_chat_id));
        assert!(chat_exists(&conn, &canonical_chat_id));
        assert!(is_peer(&conn, &canonical_chat_id));
        let migrated_messages = get_messages(&conn, &canonical_chat_id).expect("messages");
        assert_eq!(migrated_messages.len(), 1);
        assert_eq!(migrated_messages[0].id, "msg-1");
    }
}
