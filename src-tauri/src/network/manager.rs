use crate::network::behaviour::{RChatBehaviour, RChatBehaviourEvent};
use futures::StreamExt;
use libp2p::{swarm::SwarmEvent, Multiaddr, PeerId, Swarm};
use serde::Serialize;
use std::collections::HashMap;
use tauri::async_runtime::Receiver;
use tauri::AppHandle;
use tauri::Emitter;

#[derive(Clone, Serialize)]
pub struct LocalPeer {
    pub peer_id: String,
    pub addresses: Vec<String>,
}

pub struct NetworkManager {
    // The P2P Node itself
    swarm: Swarm<RChatBehaviour>,
    // The channel to receive commands FROM the UI
    crx: Receiver<String>,
    // The handle to send events TO the UI
    app_handle: AppHandle,
    disc_rx: Receiver<Multiaddr>,
    // Channel for mDNS-SD discovery
    mdns_rx: Receiver<crate::network::mdns::MdnsPeer>,
    // Sender to pass to mDNS service when starting it
    mdns_tx: tokio::sync::mpsc::Sender<crate::network::mdns::MdnsPeer>,
    // Flag to ensure we only start mDNS once
    mdns_started: bool,
    // Track local peers discovered via mDNS
    local_peers: HashMap<PeerId, Vec<Multiaddr>>,
}

impl NetworkManager {
    pub fn new(
        swarm: Swarm<RChatBehaviour>,
        crx: Receiver<String>,
        disc_rx: Receiver<Multiaddr>,
        mdns_rx: Receiver<crate::network::mdns::MdnsPeer>,
        mdns_tx: tokio::sync::mpsc::Sender<crate::network::mdns::MdnsPeer>,
        app_handle: AppHandle,
    ) -> Self {
        Self {
            swarm,
            crx,
            disc_rx,
            mdns_rx,
            mdns_tx,
            mdns_started: false,
            app_handle,
            local_peers: HashMap::new(),
        }
    }
    pub async fn run(mut self: Self) {
        println!("🛜 Network Manager: Running!");

        // Publish every 5 minutes
        let mut publish_interval = tokio::time::interval(std::time::Duration::from_secs(300));
        // Heartbeat every 60 seconds
        // Heartbeat every 10 seconds (Checking connectivity)
        let mut heartbeat_interval = tokio::time::interval(std::time::Duration::from_secs(10));

        loop {
            tokio::select! {
                _ = publish_interval.tick() => {
                    self.publish_listeners().await;
                }
                _ = heartbeat_interval.tick() => {
                    let peer_count = self.local_peers.len();
                    println!("[Network Debug] Heartbeat: Swarm active. Peer count: {}. Listening...", peer_count);
                }
                Some(cmd) = self.crx.recv() => {
                    self.handle_ui_command(cmd);
                }
                Some(addr) = self.disc_rx.recv() => {
                    // Start dialing the peer found from Gist
                    println!("Using Gist Peer: {}", addr);
                    let _ = self.swarm.dial(addr);
                }
                Some(peer) = self.mdns_rx.recv() => {
                    self.handle_mdns_peer(peer);
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

    pub fn handle_ui_command(&mut self, msg_content: String) {
        println!("UI Command Received: {}", msg_content);
        // 1. Define the Topic (Like a TV Channel)
        let topic = libp2p::gossipsub::IdentTopic::new("global-chat");

        // 2. Publish to the Swarm
        // We access the 'gossipsub' field we defined in behaviour.rs
        let result = self
            .swarm
            .behaviour_mut()
            .gossipsub
            .publish(topic, msg_content.as_bytes());

        match result {
            Ok(msg_id) => println!("Published Message ID: {:?}", msg_id),
            Err(e) => eprintln!("Publish Error: {:?}", e),
        }
    }

    /// Get current list of local peers (for Tauri command)
    pub fn get_local_peers(&self) -> Vec<LocalPeer> {
        self.local_peers
            .iter()
            .map(|(peer_id, addrs)| LocalPeer {
                peer_id: peer_id.to_string(),
                addresses: addrs.iter().map(|a| a.to_string()).collect(),
            })
            .collect()
    }

    pub async fn handle_swarm_event(&mut self, event: SwarmEvent<RChatBehaviourEvent>) {
        match event {
            // CASE A: One of our Behaviours (Gossip, mDNS) triggered an event
            SwarmEvent::Behaviour(behaviour_event) => {
                match behaviour_event {
                    // 1. Gossipsub Event: We received a message!
                    RChatBehaviourEvent::Gossipsub(libp2p::gossipsub::Event::Message {
                        message,
                        ..
                    }) => {
                        let text = String::from_utf8_lossy(&message.data).to_string();
                        let sender = message
                            .source
                            .map(|p| p.to_string())
                            .unwrap_or("Unknown".into());

                        println!("Network: Received '{}' from {}", text, sender);

                        // 1. Persist to DB
                        use tauri::Manager;
                        let state = self.app_handle.state::<crate::AppState>();

                        let timestamp = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs() as i64;

                        let id_suffix: u32 = rand::random();
                        let msg_id = format!("{}-{}", timestamp, id_suffix);

                        let db_msg = crate::storage::db::Message {
                            id: msg_id.clone(),
                            chat_id: sender.clone(), // Chat with the sender
                            peer_id: sender.clone(), // Message is FROM sender
                            timestamp,
                            content_type: "text".to_string(),
                            text_content: Some(text.clone()),
                            file_hash: None,
                        };

                        if let Ok(conn) = state.db_conn.lock() {
                            if let Err(e) = crate::storage::db::insert_message(&conn, &db_msg) {
                                eprintln!("Failed to save incoming message: {}", e);
                            } else {
                                println!("Message saved to DB.");
                            }
                        }

                        // 2. Emit event to Frontend
                        // We emit the same structure as DB Message so frontend can just use it
                        let _ = self.app_handle.emit("message-received", db_msg);
                    }

                    other => {
                        eprintln!(
                            "[Event Debug] Unhandled behaviour event: {:?}",
                            std::any::type_name_of_val(&other)
                        );
                    }
                }
            }
            // CASE B: Connection Status Changes
            SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                println!("[Swarm] Connected to {}", peer_id);
            }
            SwarmEvent::ConnectionClosed { peer_id, .. } => {
                println!("[Swarm] Disconnected from {}", peer_id);
            }
            // CASE C: New Listener Address (expected at startup)
            SwarmEvent::NewListenAddr { address, .. } => {
                println!("[Swarm] Listening on: {}", address);

                // If we haven't started mDNS yet, and this is a TCP address, start it!
                // If we haven't started mDNS yet, and this is a TCP address, start it!
                if !self.mdns_started {
                    if let Some(port) = crate::network::get_port_from_multiaddr(&address) {
                        if port != 0 {
                            println!(
                                "[NetworkManager] Found valid listen port: {}, starting mDNS...",
                                port
                            );
                            let peer_id = *self.swarm.local_peer_id();

                            // User requested to ignore is_online for now
                            // let mgr = state.config_manager.blocking_lock();
                            // is_online = cfg...
                            let is_online = true;

                            // We pass a clone of the sender
                            if let Err(e) = crate::network::mdns::start_mdns_service(
                                peer_id,
                                port,
                                is_online,
                                self.mdns_tx.clone(),
                            ) {
                                eprintln!("[NetworkManager] Failed to start mDNS: {}", e);
                            } else {
                                self.mdns_started = true;
                            }
                        }
                    }
                }
            }
            // CASE D: Incoming connection attempts
            SwarmEvent::IncomingConnection {
                local_addr,
                send_back_addr,
                ..
            } => {
                println!(
                    "[Swarm] Incoming connection from {} to {}",
                    send_back_addr, local_addr
                );
            }
            // CASE E: Dialing (outgoing connection attempt)
            SwarmEvent::Dialing { peer_id, .. } => {
                if let Some(peer) = peer_id {
                    println!("[Swarm] Dialing peer: {}", peer);
                }
            }
            // Catch-all for anything else
            other => {
                eprintln!(
                    "[Swarm Debug] Other event: {:?}",
                    std::any::type_name_of_val(&other)
                );
            }
        }
    }

    fn handle_mdns_peer(&mut self, peer: crate::network::mdns::MdnsPeer) {
        println!("[NetworkManager] Received mDNS peer: {}", peer.peer_id);

        // Parse peer ID
        let peer_id_res = peer.peer_id.parse::<PeerId>();
        match peer_id_res {
            Ok(peer_id) => {
                // 1. Add to known peers
                for addr_str in peer.addresses {
                    if let Ok(addr) = addr_str.parse::<Multiaddr>() {
                        println!("[NetworkManager] Dialing mDNS peer {} at {}", peer_id, addr);

                        // 2. Explicitly Dial
                        if let Err(e) = self.swarm.dial(addr.clone()) {
                            eprintln!("[NetworkManager] Dial failed: {}", e);
                        }

                        // 3. Track it
                        self.local_peers
                            .entry(peer_id)
                            .or_insert_with(Vec::new)
                            .push(addr);

                        // 4. Add to Gossipsub
                        self.swarm
                            .behaviour_mut()
                            .gossipsub
                            .add_explicit_peer(&peer_id);
                    }
                }

                // 5. Emit event to UI
                let peer_info = LocalPeer {
                    peer_id: peer.peer_id.clone(),
                    addresses: self
                        .local_peers
                        .get(&peer_id)
                        .map(|a| a.iter().map(|m| m.to_string()).collect())
                        .unwrap_or_default(),
                };
                let _ = self.app_handle.emit("local-peer-discovered", peer_info);
            }
            Err(e) => {
                eprintln!("[NetworkManager] Invalid Peer ID from mDNS: {}", e);
            }
        }
    }
}
