use crate::{
    storage::{
        config::FriendConfig,
        db::{self, Peer},
    },
    AppState,
};
use anyhow::Result;
use std::collections::HashMap;

pub fn get_trusted_peers(app_state: &AppState) -> Result<Vec<String>> {
    let conn = app_state.db_conn.lock().map_err(|error| {
        anyhow::anyhow!("failed to lock database while listing trusted peers: {error}")
    })?;
    Ok(db::get_all_peers(&conn)?
        .into_iter()
        .map(|peer| peer.id)
        .collect())
}

pub fn add_trusted_peer(
    app_state: &AppState,
    peer_id: String,
    alias: Option<String>,
) -> Result<()> {
    let peer_id = peer_id.trim().to_string();
    if peer_id.is_empty() {
        return Err(anyhow::anyhow!("Peer ID is required"));
    }
    let conn = app_state.db_conn.lock().map_err(|error| {
        anyhow::anyhow!("failed to lock database while adding peer: {error}")
    })?;
    db::add_peer(&conn, &peer_id, alias.as_deref(), None, "manual")
}

pub fn rename_peer(app_state: &AppState, peer_id: &str, alias: String) -> Result<()> {
    let conn = app_state.db_conn.lock().map_err(|error| {
        anyhow::anyhow!("failed to lock database while renaming peer: {error}")
    })?;
    if !db::update_peer_alias(&conn, peer_id, &alias)? {
        return Err(anyhow::anyhow!("Peer not found: {peer_id}"));
    }
    Ok(())
}

pub fn delete_peer(app_state: &AppState, peer_id: &str) -> Result<()> {
    let conn = app_state.db_conn.lock().map_err(|error| {
        anyhow::anyhow!("failed to lock database while deleting peer: {error}")
    })?;
    db::delete_peer(&conn, peer_id)
}

pub fn get_peer_aliases(app_state: &AppState) -> Result<HashMap<String, String>> {
    let conn = app_state.db_conn.lock().map_err(|error| {
        anyhow::anyhow!("failed to lock database while loading peer aliases: {error}")
    })?;
    db::get_peer_aliases(&conn)
}

pub fn get_all_peer_rows(app_state: &AppState) -> Result<Vec<Peer>> {
    let conn = app_state.db_conn.lock().map_err(|error| {
        anyhow::anyhow!("failed to lock database while loading peers: {error}")
    })?;
    db::get_all_peers(&conn)
}

pub async fn get_friends(app_state: &AppState) -> Result<Vec<FriendConfig>> {
    let mgr = app_state.config_manager.lock().await;
    Ok(mgr.load().await?.user.friends)
}

pub async fn add_friend(
    app_state: &AppState,
    username: String,
    x25519_key: Option<String>,
    ed25519_key: Option<String>,
) -> Result<()> {
    let username = username.trim().to_string();
    if username.is_empty() {
        return Err(anyhow::anyhow!("Friend username is required"));
    }

    let mgr = app_state.config_manager.lock().await;
    let mut config = mgr.load().await?;
    if !config
        .user
        .friends
        .iter()
        .any(|friend| friend.username == username)
    {
        config.user.friends.push(FriendConfig {
            username,
            alias: None,
            x25519_pubkey: x25519_key,
            ed25519_pubkey: ed25519_key,
            leaf_index: 0,
            encrypted_leaf_key: None,
            nonce: None,
        });
        mgr.save(&config).await?;
    }
    Ok(())
}

pub async fn remove_friend(app_state: &AppState, username: &str) -> Result<()> {
    let mgr = app_state.config_manager.lock().await;
    let mut config = mgr.load().await?;
    config.user.friends.retain(|friend| friend.username != username);
    mgr.save(&config).await?;
    Ok(())
}

pub async fn get_pinned_peers(app_state: &AppState) -> Result<Vec<String>> {
    let mgr = app_state.config_manager.lock().await;
    Ok(mgr.load().await?.user.pinned_peers)
}

pub async fn toggle_pin_peer(app_state: &AppState, username: String) -> Result<bool> {
    let mgr = app_state.config_manager.lock().await;
    let mut config = mgr.load().await?;
    let mut is_pinned = false;
    if let Some(pos) = config
        .user
        .pinned_peers
        .iter()
        .position(|peer| peer == &username)
    {
        config.user.pinned_peers.remove(pos);
    } else {
        config.user.pinned_peers.push(username);
        is_pinned = true;
    }
    mgr.save(&config).await?;
    Ok(is_pinned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::profile::test_app_state;

    #[tokio::test]
    async fn friend_add_remove_does_not_duplicate_entries() {
        let (_temp, app_state) = test_app_state().await;

        add_friend(&app_state, "ada".to_string(), Some("x".to_string()), None)
            .await
            .expect("add friend");
        add_friend(&app_state, "ada".to_string(), Some("x".to_string()), None)
            .await
            .expect("duplicate add");

        let friends = get_friends(&app_state).await.expect("friends");
        assert_eq!(friends.len(), 1);
        assert_eq!(friends[0].username, "ada");

        remove_friend(&app_state, "ada").await.expect("remove");
        assert!(get_friends(&app_state).await.expect("friends").is_empty());
    }

    #[tokio::test]
    async fn trusted_peer_add_rename_and_blank_alias_are_validated() {
        let (_temp, app_state) = test_app_state().await;
        {
            let conn = app_state.db_conn.lock().expect("db");
            db::add_peer(&conn, "peer-1", Some("Peer"), None, "manual").expect("insert peer");
            assert!(db::update_peer_alias(&conn, "peer-1", "Renamed").expect("rename"));
            let renamed = db::get_all_peers(&conn)
                .expect("peers")
                .into_iter()
                .find(|peer| peer.id == "peer-1")
                .expect("renamed peer");
            assert_eq!(renamed.alias, "Renamed");
            assert!(db::update_peer_alias(&conn, "peer-1", "  ").is_err());
            assert!(!db::update_peer_alias(&conn, "missing", "Name").expect("missing"));
        }
        rename_peer(&app_state, "peer-1", "  ".to_string()).expect_err("blank alias");
    }

    #[tokio::test]
    async fn adding_existing_peer_without_alias_preserves_existing_alias() {
        let (_temp, app_state) = test_app_state().await;
        add_trusted_peer(&app_state, "peer-1".to_string(), Some("User Alias".to_string()))
            .expect("initial add");
        add_trusted_peer(&app_state, "peer-1".to_string(), None).expect("duplicate add");
        let peers = get_all_peer_rows(&app_state).expect("peers");
        assert_eq!(
            peers.iter().find(|peer| peer.id == "peer-1").expect("peer").alias,
            "User Alias"
        );
    }

    #[tokio::test]
    async fn trusted_peers_list_and_delete_use_database_peers() {
        let (_temp, app_state) = test_app_state().await;
        {
            let conn = app_state.db_conn.lock().expect("db");
            db::add_peer(&conn, "peer-1", Some("Peer"), None, "manual").expect("insert peer");
        }

        assert!(get_trusted_peers(&app_state)
            .expect("trusted")
            .contains(&"peer-1".to_string()));

        delete_peer(&app_state, "peer-1").expect("delete");
        assert!(!get_trusted_peers(&app_state)
            .expect("trusted")
            .contains(&"peer-1".to_string()));
    }
}
