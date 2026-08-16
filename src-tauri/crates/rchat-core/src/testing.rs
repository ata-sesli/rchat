//! Test-support helpers shared by `rchat-core` and downstream crates.
//!
//! These build isolated `AppState`/`NetworkState` values with an in-memory
//! database and a live command channel so tests can exercise the chat send,
//! history, and read paths without touching real user data.

use crate::app_state::{
    BroadcastState, ChatConnectionRuntime, NetworkState, TemporaryRuntimeState, VoiceCallState,
};
use crate::network::command::NetworkCommand;
use crate::storage::config::{ConfigManager, ConnectivitySettings};
use crate::AppState;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

/// Build an `AppState` backed by a temp directory and an in-memory database
/// with the full schema applied. Returns the `TempDir` so the caller can keep
/// it alive for the duration of the test.
pub async fn test_app_state() -> (tempfile::TempDir, AppState) {
    let temp = tempfile::tempdir().expect("tempdir");
    let app_dir = temp.path().join("rchat-data");
    std::fs::create_dir_all(&app_dir).expect("app dir");
    let mut manager = ConfigManager::new(app_dir.clone());
    manager.init("password").await.expect("init config");
    let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
    crate::storage::db::create_tables(&conn).expect("schema");

    (
        temp,
        AppState {
            config_manager: Arc::new(Mutex::new(manager)),
            db_conn: Arc::new(std::sync::Mutex::new(conn)),
            app_dir: PathBuf::from(app_dir),
        },
    )
}

/// Build a `NetworkState` with an empty temporary runtime and a live command
/// channel. The returned receiver lets tests assert on emitted commands.
pub fn test_network_state() -> (NetworkState, mpsc::Receiver<NetworkCommand>) {
    let (tx, rx) = mpsc::channel(8);
    (
        NetworkState {
            sender: Arc::new(Mutex::new(tx)),
            local_peer_id: Arc::new(Mutex::new(Some(
                "12D3KooWLk1GoEB3MbHbRLHTxXrvNGSxC2UALaCuKAgKuYXkXazU".to_string(),
            ))),
            listening_addresses: Arc::new(Mutex::new(vec![
                "/ip4/127.0.0.1/udp/5000/quic-v1".to_string(),
            ])),
            public_address_v6: Arc::new(Mutex::new(None)),
            public_address_v4: Arc::new(Mutex::new(None)),
            stun_external_port: Arc::new(Mutex::new(None)),
            temporary_state: Arc::new(Mutex::new(TemporaryRuntimeState::default())),
            connected_chat_ids: Arc::new(Mutex::new(HashSet::new())),
            chat_connections: Arc::new(Mutex::new(
                HashMap::<String, ChatConnectionRuntime>::new(),
            )),
            voice_call_state: Arc::new(Mutex::new(VoiceCallState::default())),
            broadcast_state: Arc::new(Mutex::new(BroadcastState::default())),
            connectivity: Arc::new(Mutex::new(ConnectivitySettings::default())),
        },
        rx,
    )
}
