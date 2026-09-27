use crate::{
    storage::db::{self, ChatAssignment, Envelope},
    AppState,
};
use anyhow::{anyhow, Context};
use rchat_storage::Connection;

pub fn create_envelope(
    state: &AppState,
    id: &str,
    name: &str,
    icon: Option<&str>,
) -> anyhow::Result<()> {
    with_db(state, |conn| create_envelope_with_conn(conn, id, name, icon))
}

pub fn update_envelope(
    state: &AppState,
    id: &str,
    name: &str,
    icon: Option<&str>,
) -> anyhow::Result<()> {
    with_db(state, |conn| update_envelope_with_conn(conn, id, name, icon))
}

pub fn delete_envelope(state: &AppState, id: &str) -> anyhow::Result<()> {
    with_db(state, |conn| db::delete_envelope(conn, id))
}

pub fn list_envelopes(state: &AppState) -> anyhow::Result<Vec<Envelope>> {
    with_db(state, list_envelopes_with_conn)
}

pub fn move_chat_to_envelope(
    state: &AppState,
    chat_id: &str,
    envelope_id: Option<&str>,
) -> anyhow::Result<()> {
    with_db(state, |conn| {
        move_chat_to_envelope_with_conn(conn, chat_id, envelope_id)
    })
}

pub fn list_assignments(state: &AppState) -> anyhow::Result<Vec<ChatAssignment>> {
    with_db(state, list_assignments_with_conn)
}

fn with_db<T>(
    state: &AppState,
    f: impl FnOnce(&Connection) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let conn = state
        .db_conn
        .lock()
        .map_err(|error| anyhow!("database lock poisoned: {error}"))?;
    f(&conn)
}

fn create_envelope_with_conn(
    conn: &Connection,
    id: &str,
    name: &str,
    icon: Option<&str>,
) -> anyhow::Result<()> {
    db::create_envelope(conn, id, name, icon).context("failed to create envelope")
}

fn update_envelope_with_conn(
    conn: &Connection,
    id: &str,
    name: &str,
    icon: Option<&str>,
) -> anyhow::Result<()> {
    db::update_envelope(conn, id, name, icon).context("failed to update envelope")
}

fn list_envelopes_with_conn(conn: &Connection) -> anyhow::Result<Vec<Envelope>> {
    db::get_envelopes(conn).context("failed to list envelopes")
}

fn move_chat_to_envelope_with_conn(
    conn: &Connection,
    chat_id: &str,
    envelope_id: Option<&str>,
) -> anyhow::Result<()> {
    db::assign_chat_to_envelope(conn, chat_id, envelope_id)
        .context("failed to move chat to envelope")
}

fn list_assignments_with_conn(conn: &Connection) -> anyhow::Result<Vec<ChatAssignment>> {
    db::get_chat_assignments(conn).context("failed to list envelope assignments")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope_test_db() -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory db");
        conn.execute(
            "CREATE TABLE envelopes (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                icon TEXT
            )",
            [],
        )
        .expect("create envelopes");
        conn.execute(
            "CREATE TABLE chat_envelopes (
                chat_id TEXT PRIMARY KEY,
                envelope_id TEXT NOT NULL
            )",
            [],
        )
        .expect("create chat envelopes");
        conn
    }

    #[test]
    fn envelope_service_round_trips_envelopes() {
        let conn = envelope_test_db();

        create_envelope_with_conn(&conn, "work", "Work", Some("briefcase")).unwrap();
        update_envelope_with_conn(&conn, "work", "Projects", Some("folder")).unwrap();

        let envelopes = list_envelopes_with_conn(&conn).unwrap();
        assert_eq!(envelopes.len(), 1);
        assert_eq!(envelopes[0].id, "work");
        assert_eq!(envelopes[0].name, "Projects");
        assert_eq!(envelopes[0].icon.as_deref(), Some("folder"));
    }

    #[test]
    fn envelope_service_moves_and_unassigns_chats() {
        let conn = envelope_test_db();

        create_envelope_with_conn(&conn, "work", "Work", None).unwrap();
        move_chat_to_envelope_with_conn(&conn, "chat-1", Some("work")).unwrap();

        let assignments = list_assignments_with_conn(&conn).unwrap();
        assert_eq!(assignments.len(), 1);
        assert_eq!(assignments[0].chat_id, "chat-1");
        assert_eq!(assignments[0].envelope_id, "work");

        move_chat_to_envelope_with_conn(&conn, "chat-1", None).unwrap();
        assert!(list_assignments_with_conn(&conn).unwrap().is_empty());
    }
}
