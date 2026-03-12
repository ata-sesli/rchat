use super::*;

impl NetworkManager {
    pub async fn run(mut self: Self) {
        println!("🛜 Network Manager: Running!");
        self.refresh_peer_mapping_cache().await;

        let control_topic = crate::network::gossip::control_topic();
        if let Err(e) = self
            .swarm
            .behaviour_mut()
            .gossipsub
            .subscribe(&control_topic)
        {
            eprintln!(
                "[Gossipsub] Failed to subscribe to control topic {}: {:?}",
                crate::network::gossip::CONTROL_TOPIC,
                e
            );
        } else {
            println!(
                "[Gossipsub] ✅ Subscribed to control topic {}",
                crate::network::gossip::CONTROL_TOPIC
            );
        }

        // Subscribe to all previously joined group topics.
        {
            use tauri::Manager;
            let group_ids = {
                let state = self.app_handle.state::<crate::AppState>();
                let loaded = if let Ok(conn) = state.db_conn.lock() {
                    crate::storage::db::get_joined_group_chat_ids(&conn, "Me").unwrap_or_default()
                } else {
                    Vec::new()
                };
                loaded
            };

            for group_id in group_ids {
                if let Some(topic) = crate::network::gossip::topic_for_group_id(&group_id) {
                    if let Err(e) = self.swarm.behaviour_mut().gossipsub.subscribe(&topic) {
                        eprintln!("[Gossipsub] Failed to subscribe {}: {:?}", group_id, e);
                    } else {
                        self.subscribed_group_ids.insert(group_id);
                    }
                }
            }
        }

        // NOTE: STUN discovery is now done in mod.rs during init with the correct UDP port
        // This ensures the STUN external port matches the QUIC listener port

        // Publish every 5 minutes
        let mut publish_interval = tokio::time::interval(std::time::Duration::from_secs(300));
        // Heartbeat every 10 seconds (Checking connectivity)
        let mut heartbeat_interval = tokio::time::interval(std::time::Duration::from_secs(10));
        // NAT keepalive every 15 seconds - dial dummy address to keep port mapping alive
        let mut nat_keepalive_interval = tokio::time::interval(std::time::Duration::from_secs(15));
        // Dummy address for NAT keepalive (will fail, but outbound packet keeps NAT alive)
        let nat_keepalive_addr: Multiaddr = "/ip4/1.1.1.1/udp/9/quic-v1".parse().unwrap();
        // Shadow invite polling every 2 seconds - check invitees for their shadow invites
        let mut shadow_poll_interval = tokio::time::interval(std::time::Duration::from_secs(2));
        // Aggressive punch interval - 500ms for continuous hole punching
        let mut punch_interval = tokio::time::interval(std::time::Duration::from_millis(500));
        // Cleanup stale transfer states every minute.
        let mut transfer_cleanup_interval =
            tokio::time::interval(std::time::Duration::from_secs(60));

        loop {
            tokio::select! {
                _ = publish_interval.tick() => {
                    self.publish_listeners().await;
                }
                _ = heartbeat_interval.tick() => {
                    let peer_count = self.local_peers.len();
                    println!("[Network Debug] Heartbeat: Swarm active. Peer count: {}. Listening...", peer_count);
                }
                _ = nat_keepalive_interval.tick() => {
                    // Dial a dummy address to send outbound UDP and keep NAT mapping alive
                    // The dial will fail, but the outbound packet is enough for NAT
                    let _ = self.swarm.dial(nat_keepalive_addr.clone());
                    // Don't log every time to avoid spam, but occasionally log
                }
                _ = shadow_poll_interval.tick() => {
                    // Poll for shadow invites from invitees
                    self.poll_shadow_invites().await;
                }
                _ = punch_interval.tick() => {
                    // Continuously punch all active targets
                    self.punch_active_targets();
                }
                _ = transfer_cleanup_interval.tick() => {
                    self.cleanup_stale_transfer_states();
                }
                Some(cmd) = self.crx.recv() => {
                    self.dispatch_command(cmd).await;
                }
                Some(addr) = self.disc_rx.recv() => {
                    // Start dialing the peer found from Gist
                    println!("Using Gist Peer: {}", addr);
                    let _ = self.swarm.dial(addr);
                }
                Some(peer) = self.mdns_rx.recv() => {
                    self.handle_mdns_peer(peer);
                }
                Some(transfer_result) = self.transfer_result_rx.recv() => {
                    self.handle_transfer_result(transfer_result).await;
                }
                event = self.swarm.select_next_some() => {
                    self.handle_swarm_event(event).await;
                }
            }
        }
    }
    async fn publish_listeners(&mut self) {
        use tauri::Manager;
        let listeners: Vec<String> = self.swarm.listeners().map(|l| l.to_string()).collect();
        if listeners.is_empty() {
            return;
        }

        let state = self.app_handle.state::<crate::AppState>();
        let (token, is_online) = {
            let mgr = state.config_manager.lock().await;
            if let Ok(config) = mgr.load().await {
                (config.system.github_token.clone(), config.user.is_online)
            } else {
                (None, false)
            }
        };

        if !is_online {
            return;
        }

        if let Some(token) = token {
            println!("Publishing listeners to Gist...");
            if !listeners.is_empty() {
                if let Err(e) = crate::network::discovery::publish_peer_info(
                    &token,
                    listeners,
                    self.app_handle.clone(),
                )
                .await
                {
                    eprintln!("Failed to publish peer info: {}", e);
                }
            }
        }
    }
}
