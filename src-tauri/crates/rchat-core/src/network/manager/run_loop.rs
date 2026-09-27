use super::*;

impl NetworkManager {
    pub async fn run(mut self: Self) {
        println!("🛜 Network Manager: Running!");
        self.refresh_peer_mapping_cache().await;
        self.refresh_trusted_peer_registry().await;

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
            let group_ids = {
                let state = &self.app_state;
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

        // Refresh the live transport asynchronously; never bind a second STUN socket.
        let mut endpoint_interval = tokio::time::interval(std::time::Duration::from_secs(5));

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
        // Voice-call tick: ring timeout + outgoing frame pump.
        let mut voice_call_tick = tokio::time::interval(std::time::Duration::from_millis(20));
        // Video-call tick: native camera frame pump + stream lifecycle + diagnostics/adaptation.
        let mut video_call_tick = tokio::time::interval(std::time::Duration::from_millis(15));
        // Broadcast tick: ring timeout lifecycle + native screen-capture pump.
        let mut broadcast_tick = tokio::time::interval(std::time::Duration::from_millis(33));
        // Ensure mDNS runtime reflects current connectivity settings.
        let mut mdns_reconcile_interval = tokio::time::interval(std::time::Duration::from_secs(2));

        loop {
            tokio::select! {
                _ = endpoint_interval.tick() => {
                    self.sync_public_endpoint().await;
                    self.start_endpoint_refresh();
                }
                result = async {
                    match self.endpoint_refresh_task.as_mut() {
                        Some(task) => task.await,
                        None => std::future::pending().await,
                    }
                } => {
                    self.endpoint_refresh_task.take();
                    if let Ok(Err(error)) = result { eprintln!("[STUN] {error}"); }
                    self.finish_endpoint_refresh().await;
                }
                _ = publish_interval.tick() => {
                    self.publish_listeners().await;
                }
                _ = async {
                    match self.endpoint_publication_task.as_mut() {
                        Some(task) => { let _ = task.await; }
                        None => std::future::pending().await,
                    }
                } => {
                    self.endpoint_publication_task.take();
                    self.finish_endpoint_publication().await;
                }
                _ = heartbeat_interval.tick() => {
                    let connected_count = self.swarm.connected_peers().count();
                    let discovered_count = self.local_peers.len();
                    println!(
                        "[Network Debug] Heartbeat: Swarm active. Connected: {}, discovered: {}. Listening...",
                        connected_count, discovered_count
                    );
                }
                _ = nat_keepalive_interval.tick() => {
                    // Dial a dummy address to send outbound UDP and keep NAT mapping alive
                    // The dial will fail, but the outbound packet is enough for NAT
                    if self.is_nat_keepalive_enabled() {
                        println!("[NAT] KeepAlive sent to 1.1.1.1");
                        self.record_outgoing_dial(&nat_keepalive_addr, OutgoingDialSource::NatKeepalive);
                        let _ = self.swarm.dial(nat_keepalive_addr.clone());
                    }
                }
                _ = shadow_poll_interval.tick() => {
                    // Poll for shadow invites from invitees
                    if self.is_punch_assist_enabled() {
                        self.poll_shadow_invites().await;
                    }
                }
                _ = punch_interval.tick() => {
                    // Continuously punch all active targets
                    if self.is_punch_assist_enabled() {
                        self.punch_active_targets().await;
                    }
                }
                _ = transfer_cleanup_interval.tick() => {
                    self.cleanup_stale_transfer_states();
                }
                _ = voice_call_tick.tick() => {
                    self.tick_voice_call().await;
                }
                Some(event) = self.voice_stream_event_rx.recv() => {
                    self.handle_voice_stream_event(event).await;
                }
                _ = video_call_tick.tick() => {
                    self.tick_video_call().await;
                }
                Some(event) = self.video_encode_event_rx.recv() => {
                    self.handle_outbound_video_encode_event(event);
                }
                Some(event) = self.video_stream_event_rx.recv() => {
                    self.handle_video_stream_event(event).await;
                }
                _ = broadcast_tick.tick() => {
                    self.tick_broadcast().await;
                }
                Some(event) = self.screen_broadcast_worker_event_rx.recv() => {
                    self.handle_screen_broadcast_worker_event(event).await;
                }
                Some(event) = self.screen_broadcast_stream_event_rx.recv() => {
                    self.handle_screen_broadcast_stream_event(event).await;
                }
                _ = mdns_reconcile_interval.tick() => {
                    self.reconcile_mdns_runtime();
                }
                Some(cmd) = self.crx.recv() => {
                    self.dispatch_command(cmd).await;
                }
                Some(addr) = self.disc_rx.recv() => {
                    // Start dialing the peer found from Gist
                    println!("Using Gist Peer: {}", addr);
                    self.record_outgoing_dial(&addr, OutgoingDialSource::Gist);
                    let _ = self.swarm.dial(addr);
                }
                Some(peer) = self.mdns_rx.recv() => {
                    self.handle_mdns_peer(peer).await;
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
    pub(super) async fn publish_listeners(&mut self) {
        if !self.is_github_sync_enabled() {
            return;
        }

        let local: Vec<String> = self.swarm.listeners().map(|l| l.to_string()).collect();
        let listeners = crate::network::endpoint::advertised_addresses(
            &self.network_state.public_endpoint.observation(),
            &local,
            std::time::Instant::now(),
        );

        self.start_endpoint_publication(listeners);
    }
}
