use crate::network::behaviour::{RChatBehaviour, RChatBehaviourEvent};
use futures::StreamExt;
use libp2p::{swarm::SwarmEvent, Multiaddr, PeerId, Swarm};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use tauri::async_runtime::Receiver;
use tauri::AppHandle;
use tauri::Emitter;
use tauri::Manager;

#[derive(Clone, Serialize)]
pub struct LocalPeer {
    pub peer_id: String,
    pub addresses: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConnectionRequest {
    pub from_peer_id: String,
    pub device_name: Option<String>,
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
    // Track our outgoing connection requests (peers we pressed Connect on)
    pending_requests: HashSet<PeerId>,
    // Track incoming connection requests from others
    incoming_requests: HashSet<PeerId>,
    // Pending GitHub mappings: multiaddr → (inviter_username, my_username) for connection events
    pending_github_mappings: HashMap<String, (String, String)>,
    // Pending shadow polls: invitee_username → (password, my_username, created_at)
    // Used to poll invitee's Gist for shadow invite (bidirectional hole punch)
    pending_shadow_polls: HashMap<String, (String, String, u64)>,
    // Active punch targets: target_name → (Multiaddr, start_time)
    // Continuous 500ms punching for 30 seconds
    active_punch_targets: HashMap<String, (Multiaddr, std::time::Instant)>,
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
            pending_requests: HashSet::new(),
            incoming_requests: HashSet::new(),
            pending_github_mappings: HashMap::new(),
            pending_shadow_polls: HashMap::new(),
            active_punch_targets: HashMap::new(),
        }
    }
    pub async fn run(mut self: Self) {
        println!("🛜 Network Manager: Running!");

        // Subscribe to the global-chat topic to receive messages
        let topic = libp2p::gossipsub::IdentTopic::new("global-chat");
        if let Err(e) = self.swarm.behaviour_mut().gossipsub.subscribe(&topic) {
            eprintln!("[Gossipsub] Failed to subscribe to global-chat: {:?}", e);
        } else {
            println!("[Gossipsub] ✅ Subscribed to global-chat topic");
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

        // Handle dial command (DIAL:multiaddr:inviter_username:my_username)
        if msg_content.starts_with("DIAL:") {
            let parts: Vec<&str> = msg_content.splitn(4, ':').collect();
            if parts.len() >= 3 {
                let multiaddr_str = parts[1];
                let inviter_username = parts[2].to_string();
                let my_username = if parts.len() >= 4 { Some(parts[3].to_string()) } else { None };
                
                println!("[DIAL] 📞 Dialing {} (inviter: {}, me: {:?})", multiaddr_str, inviter_username, my_username);
                
                if let Ok(addr) = multiaddr_str.parse::<Multiaddr>() {
                    // Store pending mapping for when connection succeeds
                    if let Some(my_user) = my_username {
                        self.pending_github_mappings.insert(
                            multiaddr_str.to_string(), 
                            (inviter_username.clone(), my_user)
                        );
                    }
                    
                    // Add to active punch targets for continuous punching
                    self.add_punch_target(&inviter_username, addr);
                } else {
                    eprintln!("[DIAL] ❌ Invalid multiaddr: {}", multiaddr_str);
                }
                return;
            }
        }
        
        // Handle START_PUNCH command (START_PUNCH:multiaddr:target_username)
        // Used by invitee to start punching after publishing shadow invite
        if msg_content.starts_with("START_PUNCH:") {
            let parts: Vec<&str> = msg_content.splitn(3, ':').collect();
            if parts.len() == 3 {
                let multiaddr_str = parts[1];
                let target_username = parts[2];
                
                println!("[PUNCH] 🥊 Starting punch to {} at {}", target_username, multiaddr_str);
                
                if let Ok(addr) = multiaddr_str.parse::<Multiaddr>() {
                    self.add_punch_target(target_username, addr);
                }
                return;
            }
        }

        // Handle connection request command
        if msg_content.starts_with("REQUEST_CONNECTION:") {
            if let Some(peer_id_str) = msg_content.strip_prefix("REQUEST_CONNECTION:") {
                self.handle_connection_request(peer_id_str);
                return;
            }
        }

        // Handle shadow poll registration (REGISTER_SHADOW:invitee:password:my_username)
        if msg_content.starts_with("REGISTER_SHADOW:") {
            let parts: Vec<&str> = msg_content.splitn(4, ':').collect();
            if parts.len() == 4 {
                let invitee = parts[1];
                let password = parts[2];
                let my_username = parts[3];
                self.register_shadow_poll(invitee, password, my_username);
                return;
            }
        }

        // Handle direct messages (DM:peer_id:msg_id:timestamp:alias:content)
        if msg_content.starts_with("DM:") {
            let parts: Vec<&str> = msg_content.splitn(6, ':').collect();
            if parts.len() >= 6 {
                let target_peer_id = parts[1];
                let msg_id = parts[2];
                let timestamp: i64 = parts[3].parse().unwrap_or(0);
                let sender_alias = parts[4];
                let content = parts[5];

                println!(
                    "[DM] 📤 Sending direct message to {} (alias: {}): {}",
                    target_peer_id, sender_alias, content
                );

                // Handle GitHub chat prefix (gh:username)
                let actual_peer_id_str = if target_peer_id.starts_with("gh:") {
                    let github_username = &target_peer_id[3..]; // Remove "gh:" prefix
                    
                    // Look up the actual PeerId from config's github_peer_mapping
                    let state = self.app_handle.state::<crate::AppState>();
                    let mapping = tauri::async_runtime::block_on(async {
                        let mgr = state.config_manager.lock().await;
                        if let Ok(config) = mgr.load().await {
                            config.user.github_peer_mapping.get(github_username).cloned()
                        } else {
                            None
                        }
                    });
                    
                    if let Some(peer_id_string) = mapping {
                        println!("[DM] 🔄 Resolved GitHub user {} to PeerId {}", github_username, peer_id_string);
                        peer_id_string
                    } else {
                        eprintln!("[DM] ❌ No PeerId mapping found for GitHub user {}. Message queued.", github_username);
                        // TODO: Queue message for later delivery when PeerId is discovered
                        return;
                    }
                } else {
                    target_peer_id.to_string()
                };

                // Find the peer in connected peers
                if let Ok(peer_id) = actual_peer_id_str.parse::<PeerId>() {
                    use crate::network::direct_message::DirectMessageRequest;
                    let request = DirectMessageRequest {
                        id: msg_id.to_string(),
                        sender_id: self.swarm.local_peer_id().to_string(),
                        msg_type: "text".to_string(),
                        text_content: Some(content.to_string()),
                        file_hash: None,
                        timestamp,
                        chunk_hash: None,
                        chunk_data: None,
                        chunk_list: None,
                        sender_alias: if sender_alias.is_empty() { None } else { Some(sender_alias.to_string()) },
                    };

                    self.swarm
                        .behaviour_mut()
                        .direct_message
                        .send_request(&peer_id, request);
                    println!("[DM] ✅ Request sent to {}", peer_id);
                } else {
                    eprintln!("[DM] ❌ Invalid peer_id: {}", actual_peer_id_str);
                }
                return;
            }
        }

        // Handle read receipts (READ_RECEIPT:peer_id:msg_id1,msg_id2,...)
        if msg_content.starts_with("READ_RECEIPT:") {
            let parts: Vec<&str> = msg_content.splitn(3, ':').collect();
            if parts.len() >= 3 {
                let target_peer_id = parts[1];
                let msg_ids = parts[2];

                println!(
                    "[READ_RECEIPT] 📤 Sending read receipt to {}",
                    target_peer_id
                );

                if let Ok(peer_id) = target_peer_id.parse::<PeerId>() {
                    use crate::network::direct_message::DirectMessageRequest;
                    let request = DirectMessageRequest {
                        id: format!(
                            "read-receipt-{}",
                            std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap()
                                .as_secs()
                        ),
                        sender_id: self.swarm.local_peer_id().to_string(),
                        msg_type: "read_receipt".to_string(),
                        text_content: Some(msg_ids.to_string()),
                        file_hash: None,
                        timestamp: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs() as i64,
                        chunk_hash: None,
                        chunk_data: None,
                        chunk_list: None,
                        sender_alias: None,
                    };

                    self.swarm
                        .behaviour_mut()
                        .direct_message
                        .send_request(&peer_id, request);
                    println!("[READ_RECEIPT] ✅ Sent to {}", peer_id);
                } else {
                    eprintln!("[READ_RECEIPT] ❌ Invalid peer_id: {}", target_peer_id);
                }
                return;
            }
        }

        // Handle image messages (__IMAGE_MSG__:file_hash:target_peer_id)
        if msg_content.starts_with("__IMAGE_MSG__:") {
            let parts: Vec<&str> = msg_content.splitn(3, ':').collect();
            if parts.len() >= 3 {
                let file_hash = parts[1];
                let target_peer_id = parts[2];

                if target_peer_id != "General" {
                    // 1v1 Chat: Use Request-Response (Direct Message)
                    println!("[Image] 📤 Sending image {} to {}", file_hash, target_peer_id);
                    
                    if let Ok(peer_id) = target_peer_id.parse::<PeerId>() {
                        use crate::network::direct_message::DirectMessageRequest;
                        let request = DirectMessageRequest {
                            id: format!(
                                "img-{}",
                                std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap()
                                    .as_secs()
                            ),
                            sender_id: self.swarm.local_peer_id().to_string(),
                            msg_type: "image".to_string(),
                            text_content: None,
                            file_hash: Some(file_hash.to_string()),
                            timestamp: std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap()
                                .as_secs() as i64,
                            chunk_hash: None,
                            chunk_data: None,
                            chunk_list: None,
                            sender_alias: None,
                        };

                        self.swarm
                            .behaviour_mut()
                            .direct_message
                            .send_request(&peer_id, request);
                        println!("[Image] ✅ Direct request sent to {}", peer_id);
                        return; // Done
                    } else {
                        eprintln!("[Image] ❌ Invalid peer_id: {}", target_peer_id);
                        return;
                    }
                }
                // If "General", fall through to Gossipsub below
            }
        }

        // Handle document messages (__DOCUMENT_MSG__:file_hash:target_peer_id:filename_b64)
        if msg_content.starts_with("__DOCUMENT_MSG__:") {
            let parts: Vec<&str> = msg_content.splitn(4, ':').collect();
            if parts.len() >= 4 {
                let file_hash = parts[1];
                let target_peer_id = parts[2];
                let filename_b64 = parts[3];

                // Decode filename from base64
                use base64::{engine::general_purpose::STANDARD, Engine as _};
                let file_name = STANDARD.decode(filename_b64)
                    .ok()
                    .and_then(|bytes| String::from_utf8(bytes).ok())
                    .unwrap_or_else(|| "document".to_string());

                if target_peer_id != "General" {
                    println!("[Document] 📤 Sending document {} ({}) to {}", file_hash, file_name, target_peer_id);
                    
                    if let Ok(peer_id) = target_peer_id.parse::<PeerId>() {
                        use crate::network::direct_message::DirectMessageRequest;
                        let request = DirectMessageRequest {
                            id: format!(
                                "doc-{}",
                                std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap()
                                    .as_secs()
                            ),
                            sender_id: self.swarm.local_peer_id().to_string(),
                            msg_type: "document".to_string(),
                            text_content: Some(file_name), // Filename in text_content
                            file_hash: Some(file_hash.to_string()),
                            timestamp: std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap()
                                .as_secs() as i64,
                            chunk_hash: None,
                            chunk_data: None,
                            chunk_list: None,
                            sender_alias: None,
                        };

                        self.swarm
                            .behaviour_mut()
                            .direct_message
                            .send_request(&peer_id, request);
                        println!("[Document] ✅ Direct request sent to {}", peer_id);
                        return;
                    } else {
                        eprintln!("[Document] ❌ Invalid peer_id: {}", target_peer_id);
                        return;
                    }
                }
            }
        }

        // Handle video messages (__VIDEO_MSG__:file_hash:target_peer_id:filename_b64)
        if msg_content.starts_with("__VIDEO_MSG__:") {
            let parts: Vec<&str> = msg_content.splitn(4, ':').collect();
            if parts.len() >= 4 {
                let file_hash = parts[1];
                let target_peer_id = parts[2];
                let filename_b64 = parts[3];

                // Decode filename from base64
                use base64::{engine::general_purpose::STANDARD, Engine as _};
                let file_name = STANDARD.decode(filename_b64)
                    .ok()
                    .and_then(|bytes| String::from_utf8(bytes).ok())
                    .unwrap_or_else(|| "video.mp4".to_string());

                if target_peer_id != "General" {
                    println!("[Video] 📤 Sending video {} ({}) to {}", file_hash, file_name, target_peer_id);
                    
                    if let Ok(peer_id) = target_peer_id.parse::<PeerId>() {
                        use crate::network::direct_message::DirectMessageRequest;
                        let request = DirectMessageRequest {
                            id: format!(
                                "vid-{}",
                                std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap()
                                    .as_secs()
                            ),
                            sender_id: self.swarm.local_peer_id().to_string(),
                            msg_type: "video".to_string(),
                            text_content: Some(file_name), // Filename in text_content
                            file_hash: Some(file_hash.to_string()),
                            timestamp: std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap()
                                .as_secs() as i64,
                            chunk_hash: None,
                            chunk_data: None,
                            chunk_list: None,
                            sender_alias: None,
                        };

                        self.swarm
                            .behaviour_mut()
                            .direct_message
                            .send_request(&peer_id, request);
                        println!("[Video] ✅ Direct request sent to {}", peer_id);
                        return;
                    } else {
                        eprintln!("[Video] ❌ Invalid peer_id: {}", target_peer_id);
                        return;
                    }
                }
            }
        }

        // 1. Define the Topic (Like a TV Channel)
        let topic = libp2p::gossipsub::IdentTopic::new("global-chat");

        // Check if we have any connected peers subscribed to this topic
        let mesh_peer_count = self
            .swarm
            .behaviour()
            .gossipsub
            .mesh_peers(&topic.hash())
            .count();

        println!("[Gossipsub] Mesh peers for topic: {}", mesh_peer_count);

        // 2. Publish to the Swarm
        // We access the 'gossipsub' field we defined in behaviour.rs
        let result = self
            .swarm
            .behaviour_mut()
            .gossipsub
            .publish(topic, msg_content.as_bytes());

        match result {
            Ok(msg_id) => println!("[Gossipsub] ✅ Published Message ID: {:?}", msg_id),
            Err(e) => eprintln!(
                "[Gossipsub] ❌ Publish Error: {:?}. Mesh peers count was: {}",
                e, mesh_peer_count
            ),
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

    /// Handle a connection request from UI (user pressed Connect on a peer)
    fn handle_connection_request(&mut self, peer_id_str: &str) {
        println!("[Handshake] User requested connection to: {}", peer_id_str);

        // Parse peer_id
        let peer_id = match peer_id_str.parse::<PeerId>() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("[Handshake] Invalid peer_id: {}", e);
                return;
            }
        };

        // Check if this peer already sent us a request (mutual handshake!)
        let already_requested_us = self.incoming_requests.contains(&peer_id);
        if already_requested_us {
            println!("[Handshake] 🤝 Mutual handshake complete with {}!", peer_id);
            self.complete_handshake(peer_id);
            // Don't return - still send our request so THEY can complete too
        } else {
            // Add to our pending requests
            self.pending_requests.insert(peer_id);
            println!("[Handshake] ⏳ Waiting for {} to accept...", peer_id);
            // Emit waiting state to frontend
            let _ = self.app_handle.emit("connection-waiting", peer_id_str);
        }

        // Always send connection request to peer via gossipsub
        let my_peer_id = self.swarm.local_peer_id().to_string();
        let request_msg = format!("__CONNECTION_REQUEST__:{}", my_peer_id);

        let topic = libp2p::gossipsub::IdentTopic::new("global-chat");
        let _ = self
            .swarm
            .behaviour_mut()
            .gossipsub
            .publish(topic, request_msg.as_bytes());
    }

    /// Handle incoming connection request from another peer
    fn handle_incoming_connection_request(&mut self, from_peer_id: PeerId) {
        println!(
            "[Handshake] Received connection request from: {}",
            from_peer_id
        );

        // Check if we already requested this peer (mutual handshake!)
        if self.pending_requests.contains(&from_peer_id) {
            println!(
                "[Handshake] 🤝 Mutual handshake complete with {}!",
                from_peer_id
            );
            self.complete_handshake(from_peer_id);
            return;
        }

        // Otherwise, store as incoming request
        self.incoming_requests.insert(from_peer_id);

        // Emit to frontend so they can see who's waiting
        let _ = self
            .app_handle
            .emit("connection-request-received", from_peer_id.to_string());
    }

    /// Complete the handshake - both sides have agreed
    fn complete_handshake(&mut self, peer_id: PeerId) {
        // Remove from both sets
        self.pending_requests.remove(&peer_id);
        self.incoming_requests.remove(&peer_id);

        // Add to peers database (source of truth for friends)
        use tauri::Manager;
        let state = self.app_handle.state::<crate::AppState>();
        if let Ok(conn) = state.db_conn.lock() {
            if let Err(e) = crate::storage::db::add_peer(
                &conn,
                &peer_id.to_string(),
                None,    // alias - can be updated later
                None,    // public_key - can be updated later
                "local", // method - discovered via mDNS
            ) {
                eprintln!("[Handshake] Failed to save peer: {}", e);
            } else {
                println!("[Handshake] ✅ {} saved to peers table!", peer_id);
            }
        }

        // Emit success to frontend
        let _ = self.app_handle.emit("peer-connected", peer_id.to_string());
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

                        // Check for connection request message
                        if text.starts_with("__CONNECTION_REQUEST__:") {
                            if let Some(from_peer_str) =
                                text.strip_prefix("__CONNECTION_REQUEST__:")
                            {
                                if let Ok(from_peer) = from_peer_str.parse::<PeerId>() {
                                    self.handle_incoming_connection_request(from_peer);
                                }
                            }
                            return; // Don't process as regular message
                        }

                        // Check for image message: __IMAGE_MSG__:<file_hash>:<from_peer_id>
                        if text.starts_with("__IMAGE_MSG__:") {
                            let parts: Vec<&str> = text
                                .strip_prefix("__IMAGE_MSG__:")
                                .unwrap()
                                .split(':')
                                .collect();
                            if parts.len() >= 2 {
                                let file_hash = parts[0];
                                // Note: For now, we just store the message reference
                                // The actual image data would need to be transferred via a separate protocol

                                use tauri::Manager;
                                let state = self.app_handle.state::<crate::AppState>();

                                let timestamp = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap()
                                    .as_secs()
                                    as i64;

                                let id_suffix: u32 = rand::random();
                                let msg_id = format!("{}-{}", timestamp, id_suffix);

                                let db_msg = crate::storage::db::Message {
                                    id: msg_id.clone(),
                                    chat_id: sender.clone(),
                                    peer_id: sender.clone(),
                                    timestamp,
                                    content_type: "image".to_string(),
                                    text_content: None,
                                    file_hash: Some(file_hash.to_string()),
                                    status: "delivered".to_string(), // Received = delivered
                                    content_metadata: None,
                                    sender_alias: None,
                                };

                                if let Ok(conn) = state.db_conn.lock() {
                                    // Ensure peer and chat exist (prevents FOREIGN KEY error)
                                    if !crate::storage::db::is_peer(&conn, &sender) {
                                        let _ = crate::storage::db::add_peer(
                                            &conn,
                                            &sender,
                                            None,
                                            None,
                                            "direct",
                                        );
                                    }
                                    if !crate::storage::db::chat_exists(&conn, &sender) {
                                        let _ = crate::storage::db::create_chat(
                                            &conn,
                                            &sender,
                                            &sender,
                                            false,
                                        );
                                    }
                                    
                                    if let Err(e) =
                                        crate::storage::db::insert_message(&conn, &db_msg)
                                    {
                                        eprintln!("Failed to save incoming image message: {}", e);
                                    } else {
                                        println!(
                                            "[Image] 📷 Received image message from {}: {}",
                                            sender, file_hash
                                        );
                                    }
                                }

                                // Request the actual image data from the sender
                                if let Some(sender_peer_id) = message.source {
                                    // Build file request using DirectMessageRequest
                                    use crate::network::direct_message::DirectMessageRequest;
                                    let timestamp = std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .unwrap()
                                        .as_secs()
                                        as i64;
                                    let id_suffix: u32 = rand::random();
                                    let request = DirectMessageRequest {
                                        id: format!("{}-{}", timestamp, id_suffix),
                                        sender_id: self.swarm.local_peer_id().to_string(),
                                        msg_type: "file_metadata_request".to_string(),
                                        text_content: None,
                                        file_hash: Some(file_hash.to_string()),
                                        timestamp,
                                        chunk_hash: None,
                                        chunk_data: None,
                                        chunk_list: None,
                                        sender_alias: None,
                                    };

                                    println!(
                                        "[File Transfer] 📡 Requesting image {} from {}",
                                        file_hash, sender_peer_id
                                    );
                                    self.swarm
                                        .behaviour_mut()
                                        .direct_message
                                        .send_request(&sender_peer_id, request);
                                }

                                // Emit event to frontend
                                let _ = self.app_handle.emit("message-received", db_msg);
                            }
                            return; // Don't process as regular message
                        }

                        // Check if sender is a known peer
                        // Block messages from unknown peers
                        if let Some(sender_peer_id) = message.source {
                            use tauri::Manager;
                            let state = self.app_handle.state::<crate::AppState>();
                            let is_known = if let Ok(conn) = state.db_conn.lock() {
                                crate::storage::db::is_peer(&conn, &sender_peer_id.to_string())
                            } else {
                                false
                            };

                            if !is_known {
                                println!(
                                    "[Security] ⛔ Blocked message from unknown peer: {}",
                                    sender_peer_id
                                );
                                // Disconnect from unknown peer
                                let _ = self.swarm.disconnect_peer_id(sender_peer_id);
                                return;
                            }
                        }

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
                            status: "delivered".to_string(), // Received = delivered
                            content_metadata: None,
                            sender_alias: None, // TODO: extract from DM format
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

                    // Handle request-response for file transfer
                    RChatBehaviourEvent::DirectMessage(event) => {
                        use crate::network::direct_message::{
                            DirectMessageRequest, DirectMessageResponse,
                        };
                        use libp2p::request_response::{Event, Message};

                        match event {
                            Event::Message { peer, message, .. } => {
                                match message {
                                    Message::Request {
                                        request, channel, ..
                                    } => {
                                        println!(
                                            "[DM] 📥 Received {} from {}",
                                            request.msg_type, peer
                                        );

                                        match request.msg_type.as_str() {
                                            "text" | "image" => {
                                                // Save incoming message to database
                                                use tauri::Manager;
                                                let state =
                                                    self.app_handle.state::<crate::AppState>();

                                                let db_msg = crate::storage::db::Message {
                                                    id: request.id.clone(),
                                                    chat_id: request.sender_id.clone(),
                                                    peer_id: request.sender_id.clone(),
                                                    timestamp: request.timestamp,
                                                    content_type: request.msg_type.clone(),
                                                    text_content: request.text_content.clone(),
                                                    file_hash: request.file_hash.clone(),
                                                    status: "delivered".to_string(),
                                                    content_metadata: None,
                                                    sender_alias: request.sender_alias.clone(),
                                                };

                                                if let Ok(conn) = state.db_conn.lock() {
                                                    // Ensure peer and chat exist (with detailed logging)
                                                    let peer_exists = crate::storage::db::is_peer(
                                                        &conn,
                                                        &request.sender_id,
                                                    );
                                                    if !peer_exists {
                                                        println!("[DM] Adding peer {} to database", request.sender_id);
                                                        if let Err(e) = crate::storage::db::add_peer(
                                                            &conn,
                                                            &request.sender_id,
                                                            None,
                                                            None,
                                                            "direct",
                                                        ) {
                                                            eprintln!("[DM] ❌ Failed to add peer: {}", e);
                                                        }
                                                    }
                                                    
                                                    let chat_exists = crate::storage::db::chat_exists(
                                                        &conn,
                                                        &request.sender_id,
                                                    );
                                                    if !chat_exists {
                                                        println!("[DM] Creating chat for {}", request.sender_id);
                                                        if let Err(e) = crate::storage::db::create_chat(
                                                            &conn,
                                                            &request.sender_id,
                                                            &request.sender_id,
                                                            false,
                                                        ) {
                                                            eprintln!("[DM] ❌ Failed to create chat: {}", e);
                                                        }
                                                    }

                                                    // For image messages, ensure file record exists (FK constraint)
                                                    if request.msg_type == "image" {
                                                        if let Some(ref file_hash) = request.file_hash {
                                                            // Insert placeholder if file doesn't exist yet
                                                            let file_exists: bool = conn.query_row(
                                                                "SELECT 1 FROM files WHERE file_hash = ?1",
                                                                [file_hash],
                                                                |_| Ok(true),
                                                            ).unwrap_or(false);
                                                            
                                                            if !file_exists {
                                                                println!("[DM] Creating file placeholder for {}", file_hash);
                                                                if let Err(e) = conn.execute(
                                                                    "INSERT INTO files (file_hash, file_name, mime_type, size_bytes, is_complete) VALUES (?1, NULL, 'image/unknown', 0, 0)",
                                                                    [file_hash],
                                                                ) {
                                                                    eprintln!("[DM] ❌ Failed to create file placeholder: {}", e);
                                                                }
                                                            }
                                                        }
                                                    }

                                                    if let Err(e) =
                                                        crate::storage::db::insert_message(
                                                            &conn, &db_msg,
                                                        )
                                                    {
                                                        eprintln!(
                                                            "[DM] ❌ Failed to save {} message (id={}, chat_id={}, peer_id={}, file_hash={:?}): {}",
                                                            request.msg_type,
                                                            request.id,
                                                            request.sender_id,
                                                            request.sender_id,
                                                            request.file_hash,
                                                            e
                                                        );
                                                    } else {
                                                        println!("[DM] ✅ Message saved");
                                                    }
                                                }

                                                // For image messages, request the file chunks from sender
                                                if request.msg_type == "image" {
                                                    if let Some(ref file_hash) = request.file_hash {
                                                        println!("[ChunkTransfer] 📤 Requesting metadata for {}", file_hash);
                                                        
                                                        let metadata_req = DirectMessageRequest {
                                                            id: format!("meta-req-{}", file_hash),
                                                            sender_id: self.swarm.local_peer_id().to_string(),
                                                            msg_type: "file_metadata_request".to_string(),
                                                            text_content: None,
                                                            file_hash: Some(file_hash.clone()),
                                                            timestamp: std::time::SystemTime::now()
                                                                .duration_since(std::time::UNIX_EPOCH)
                                                                .unwrap()
                                                                .as_secs() as i64,
                                                            chunk_hash: None,
                                                            chunk_data: None,
                                                            chunk_list: None,
                                                            sender_alias: None,
                                                        };
                                                        
                                                        self.swarm
                                                            .behaviour_mut()
                                                            .direct_message
                                                            .send_request(&peer, metadata_req);
                                                    }
                                                }

                                                // Emit event to frontend
                                                let _ = self.app_handle.emit(
                                                    "message-received",
                                                    serde_json::json!({
                                                        "chat_id": request.sender_id,
                                                        "peer_id": request.sender_id,
                                                        "msg_id": request.id,
                                                        "text_content": request.text_content,
                                                        "file_hash": request.file_hash,
                                                        "timestamp": request.timestamp,
                                                    }),
                                                );

                                                // Send "delivered" response
                                                let response = DirectMessageResponse {
                                                    msg_id: request.id.clone(),
                                                    status: "delivered".to_string(),
                                                    error: None,
                                                };
                                                let _ = self
                                                    .swarm
                                                    .behaviour_mut()
                                                    .direct_message
                                                    .send_response(channel, response);
                                            }
                                            "invite_handshake" => {
                                                // Invitee connected and sent their GitHub username
                                                // Create gh: chat and PeerId mapping for this inviter
                                                if let Some(invitee_github) = request.text_content.clone() {
                                                    let invitee_peer_id = request.sender_id.clone();
                                                    println!(
                                                        "[HANDSHAKE] 🤝 Received handshake from GitHub user: {} (PeerId: {})",
                                                        invitee_github, invitee_peer_id
                                                    );
                                                    
                                                    let chat_id = format!("gh:{}", invitee_github);
                                                    
                                                    // Store mapping and create chat
                                                    use tauri::Manager;
                                                    let state = self.app_handle.state::<crate::AppState>();
                                                    
                                                    // 1. Store PeerId mapping in config
                                                    {
                                                        let app_handle = self.app_handle.clone();
                                                        let gh_user = invitee_github.clone();
                                                        let peer_id_str = invitee_peer_id.clone();
                                                        tauri::async_runtime::spawn(async move {
                                                            let state = app_handle.state::<crate::AppState>();
                                                            let mgr = state.config_manager.lock().await;
                                                            if let Ok(mut config) = mgr.load().await {
                                                                config.user.github_peer_mapping.insert(gh_user.clone(), peer_id_str.clone());
                                                                if let Err(e) = mgr.save(&config).await {
                                                                    eprintln!("[HANDSHAKE] Failed to save mapping: {}", e);
                                                                } else {
                                                                    println!("[HANDSHAKE] ✅ Saved mapping: {} → {}", gh_user, peer_id_str);
                                                                }
                                                            }
                                                        });
                                                    }
                                                    
                                                    // 2. Create peer and chat in SQLite
                                                    if let Ok(conn) = state.db_conn.lock() {
                                                        // Add peer
                                                        if !crate::storage::db::is_peer(&conn, &chat_id) {
                                                            let _ = crate::storage::db::add_peer(
                                                                &conn,
                                                                &chat_id,
                                                                Some(&invitee_github),
                                                                None,
                                                                "github",
                                                            );
                                                        }
                                                        // Create chat
                                                        if !crate::storage::db::chat_exists(&conn, &chat_id) {
                                                            let _ = crate::storage::db::create_chat(
                                                                &conn,
                                                                &chat_id,
                                                                &invitee_github,
                                                                false,
                                                            );
                                                        }
                                                        println!("[HANDSHAKE] ✅ Created chat: {}", chat_id);
                                                    }
                                                    
                                                    // 3. Notify frontend about new chat
                                                    let _ = self.app_handle.emit("new-github-chat", serde_json::json!({
                                                        "chat_id": chat_id,
                                                        "github_username": invitee_github,
                                                        "peer_id": invitee_peer_id,
                                                    }));
                                                }
                                                
                                                // Send response
                                                let response = DirectMessageResponse {
                                                    msg_id: request.id.clone(),
                                                    status: "delivered".to_string(),
                                                    error: None,
                                                };
                                                let _ = self
                                                    .swarm
                                                    .behaviour_mut()
                                                    .direct_message
                                                    .send_response(channel, response);
                                            }
                                            "read_receipt" => {
                                                // text_content contains comma-separated message IDs
                                                if let Some(ref msg_ids_str) = request.text_content
                                                {
                                                    use tauri::Manager;

                                                    // Collect message IDs to update (as owned Strings)
                                                    let msg_ids: Vec<String> = msg_ids_str
                                                        .split(',')
                                                        .map(|s| s.trim().to_string())
                                                        .filter(|s| !s.is_empty())
                                                        .collect();

                                                    // Update DB (separate scope to release lock)
                                                    {
                                                        let state = self
                                                            .app_handle
                                                            .state::<crate::AppState>();
                                                        let lock_result = state.db_conn.lock();
                                                        if let Ok(conn) = lock_result {
                                                            for msg_id in &msg_ids {
                                                                let _ = crate::storage::db::update_message_status(&conn, msg_id, "read");
                                                                println!("[READ_RECEIPT] 📥 Marked {} as read", msg_id);
                                                            }
                                                            drop(conn); // Explicitly drop to release lock
                                                        }
                                                    }

                                                    // Emit events to frontend (after DB lock is released)
                                                    for msg_id in &msg_ids {
                                                        let _ = self.app_handle.emit(
                                                            "message-status-updated",
                                                            serde_json::json!({
                                                                "msg_id": msg_id,
                                                                "status": "read",
                                                            }),
                                                        );
                                                    }
                                                }
                                                // Send ack response
                                                let response = DirectMessageResponse {
                                                    msg_id: request.id.clone(),
                                                    status: "delivered".to_string(),
                                                    error: None,
                                                };
                                                let _ = self
                                                    .swarm
                                                    .behaviour_mut()
                                                    .direct_message
                                                    .send_response(channel, response);
                                            }
                                            "file_metadata_request" => {
                                                // Return list of chunks for requested file
                                                if let Some(ref file_hash) = request.file_hash {
                                                    println!(
                                                        "[ChunkTransfer] 📋 Metadata request for: {}",
                                                        file_hash
                                                    );
                                                    
                                                    use tauri::Manager;
                                                    let state = self.app_handle.state::<crate::AppState>();
                                                    
                                                    if let Ok(conn) = state.db_conn.lock() {
                                                        // Query chunks from database
                                                        let mut stmt = conn.prepare(
                                                            "SELECT chunk_hash, chunk_order, chunk_size FROM file_chunks WHERE file_hash = ?1 ORDER BY chunk_order"
                                                        ).ok();
                                                        
                                                        let chunks: Vec<crate::network::direct_message::ChunkInfo> = if let Some(ref mut s) = stmt {
                                                            s.query_map([file_hash], |row| {
                                                                Ok(crate::network::direct_message::ChunkInfo {
                                                                    chunk_hash: row.get(0)?,
                                                                    chunk_order: row.get(1)?,
                                                                    chunk_size: row.get(2)?,
                                                                })
                                                            }).ok()
                                                            .map(|rows| rows.filter_map(|r| r.ok()).collect())
                                                            .unwrap_or_default()
                                                        } else {
                                                            Vec::new()
                                                        };
                                                        
                                                        println!("[ChunkTransfer] 📋 Returning {} chunks", chunks.len());
                                                        
                                                        // Send metadata response via DirectMessageRequest
                                                        let response_req = DirectMessageRequest {
                                                            id: format!("meta-resp-{}", request.id),
                                                            sender_id: self.swarm.local_peer_id().to_string(),
                                                            msg_type: "file_metadata_response".to_string(),
                                                            text_content: None,
                                                            file_hash: Some(file_hash.clone()),
                                                            timestamp: std::time::SystemTime::now()
                                                                .duration_since(std::time::UNIX_EPOCH)
                                                                .unwrap()
                                                                .as_secs() as i64,
                                                            chunk_hash: None,
                                                            chunk_data: None,
                                                            chunk_list: Some(chunks),
                                                            sender_alias: None,
                                                        };
                                                        
                                                        // Send as new request back to peer
                                                        self.swarm
                                                            .behaviour_mut()
                                                            .direct_message
                                                            .send_request(&peer, response_req);
                                                    }
                                                    
                                                    // Acknowledge original request
                                                    let response = DirectMessageResponse {
                                                        msg_id: request.id.clone(),
                                                        status: "delivered".to_string(),
                                                        error: None,
                                                    };
                                                    let _ = self
                                                        .swarm
                                                        .behaviour_mut()
                                                        .direct_message
                                                        .send_response(channel, response);
                                                }
                                            }
                                            "chunk_request" => {
                                                // Return chunk data for requested chunk_hash
                                                if let Some(ref chunk_hash) = request.chunk_hash {
                                                    println!(
                                                        "[ChunkTransfer] 📦 Chunk request for: {}",
                                                        chunk_hash
                                                    );
                                                    
                                                    // Load chunk from disk
                                                    let chunks_dir = directories::ProjectDirs::from("io.github", "ata-sesli", "RChat")
                                                        .map(|p| p.data_dir().join("chunks"))
                                                        .unwrap_or_else(|| std::path::PathBuf::from("chunks"));
                                                    
                                                    let chunk_path = chunks_dir.join(chunk_hash);
                                                    
                                                    if let Ok(chunk_data) = std::fs::read(&chunk_path) {
                                                        let chunk_b64 = base64::Engine::encode(
                                                            &base64::engine::general_purpose::STANDARD,
                                                            &chunk_data
                                                        );
                                                        
                                                        println!("[ChunkTransfer] 📦 Sending chunk {} ({} bytes)", chunk_hash, chunk_data.len());
                                                        
                                                        // Send chunk data as new request
                                                        let response_req = DirectMessageRequest {
                                                            id: format!("chunk-resp-{}", request.id),
                                                            sender_id: self.swarm.local_peer_id().to_string(),
                                                            msg_type: "chunk_response".to_string(),
                                                            text_content: None,
                                                            file_hash: request.file_hash.clone(),
                                                            timestamp: std::time::SystemTime::now()
                                                                .duration_since(std::time::UNIX_EPOCH)
                                                                .unwrap()
                                                                .as_secs() as i64,
                                                            chunk_hash: Some(chunk_hash.clone()),
                                                            chunk_data: Some(chunk_b64),
                                                            chunk_list: None,
                                                            sender_alias: None,
                                                        };
                                                        
                                                        self.swarm
                                                            .behaviour_mut()
                                                            .direct_message
                                                            .send_request(&peer, response_req);
                                                    } else {
                                                        eprintln!("[ChunkTransfer] ❌ Chunk not found: {}", chunk_hash);
                                                    }
                                                    
                                                    // Acknowledge
                                                    let response = DirectMessageResponse {
                                                        msg_id: request.id.clone(),
                                                        status: "delivered".to_string(),
                                                        error: None,
                                                    };
                                                    let _ = self
                                                        .swarm
                                                        .behaviour_mut()
                                                        .direct_message
                                                        .send_response(channel, response);
                                                }
                                            }
                                            "file_metadata_response" => {
                                                // Received chunk list - request each chunk
                                                if let (Some(ref file_hash), Some(ref chunks)) = (&request.file_hash, &request.chunk_list) {
                                                    println!(
                                                        "[ChunkTransfer] 📋 Received {} chunks for {}",
                                                        chunks.len(), file_hash
                                                    );
                                                    
                                                    // Request each chunk
                                                    for chunk_info in chunks {
                                                        let chunk_req = DirectMessageRequest {
                                                            id: format!("chunk-req-{}-{}", file_hash, chunk_info.chunk_order),
                                                            sender_id: self.swarm.local_peer_id().to_string(),
                                                            msg_type: "chunk_request".to_string(),
                                                            text_content: None,
                                                            file_hash: Some(file_hash.clone()),
                                                            timestamp: std::time::SystemTime::now()
                                                                .duration_since(std::time::UNIX_EPOCH)
                                                                .unwrap()
                                                                .as_secs() as i64,
                                                            chunk_hash: Some(chunk_info.chunk_hash.clone()),
                                                            chunk_data: None,
                                                            chunk_list: None,
                                                            sender_alias: None,
                                                        };
                                                        
                                                        self.swarm
                                                            .behaviour_mut()
                                                            .direct_message
                                                            .send_request(&peer, chunk_req);
                                                        
                                                        println!("[ChunkTransfer] 📤 Requested chunk {}/{}", 
                                                            chunk_info.chunk_order + 1, chunks.len());
                                                    }
                                                    
                                                    // Store expected chunk count in DB for completion tracking
                                                    {
                                                        use tauri::Manager;
                                                        let state = self.app_handle.state::<crate::AppState>();
                                                        let lock_result = state.db_conn.lock();
                                                        if let Ok(conn) = lock_result {
                                                            // Insert chunk metadata records
                                                            for chunk_info in chunks {
                                                                let _ = conn.execute(
                                                                    "INSERT OR IGNORE INTO file_chunks (file_hash, chunk_order, chunk_hash, chunk_size) VALUES (?1, ?2, ?3, ?4)",
                                                                    rusqlite::params![
                                                                        file_hash,
                                                                        chunk_info.chunk_order,
                                                                        chunk_info.chunk_hash,
                                                                        chunk_info.chunk_size
                                                                    ]
                                                                );
                                                            }
                                                        }
                                                    }
                                                }
                                                
                                                // Acknowledge
                                                let response = DirectMessageResponse {
                                                    msg_id: request.id.clone(),
                                                    status: "delivered".to_string(),
                                                    error: None,
                                                };
                                                let _ = self
                                                    .swarm
                                                    .behaviour_mut()
                                                    .direct_message
                                                    .send_response(channel, response);
                                            }
                                            "chunk_response" => {
                                                // Received chunk data - save to disk
                                                if let (Some(ref file_hash), Some(ref chunk_hash), Some(ref chunk_b64)) = 
                                                    (&request.file_hash, &request.chunk_hash, &request.chunk_data) 
                                                {
                                                    use base64::Engine;
                                                    
                                                    if let Ok(chunk_data) = base64::engine::general_purpose::STANDARD.decode(chunk_b64) {
                                                        // Save chunk to disk
                                                        let chunks_dir = directories::ProjectDirs::from("io.github", "ata-sesli", "RChat")
                                                            .map(|p| p.data_dir().join("chunks"))
                                                            .unwrap_or_else(|| std::path::PathBuf::from("chunks"));
                                                        
                                                        let _ = std::fs::create_dir_all(&chunks_dir);
                                                        let chunk_path = chunks_dir.join(chunk_hash);
                                                        
                                                        if let Err(e) = std::fs::write(&chunk_path, &chunk_data) {
                                                            eprintln!("[ChunkTransfer] ❌ Failed to save chunk {}: {}", chunk_hash, e);
                                                        } else {
                                                            println!("[ChunkTransfer] 💾 Saved chunk {} ({} bytes)", chunk_hash, chunk_data.len());
                                                        }
                                                        
                                                        // Check if all chunks received
                                                        let file_complete = {
                                                            use tauri::Manager;
                                                            let state = self.app_handle.state::<crate::AppState>();
                                                            let mut is_complete = false;
                                                            if let Ok(conn) = state.db_conn.lock() {
                                                                // Count expected vs received chunks
                                                                let expected: i64 = conn.query_row(
                                                                    "SELECT COUNT(*) FROM file_chunks WHERE file_hash = ?1",
                                                                    [file_hash],
                                                                    |row| row.get(0)
                                                                ).unwrap_or(0);
                                                                
                                                                // Count chunks actually on disk
                                                                let mut received = 0i64;
                                                                if let Ok(mut stmt) = conn.prepare(
                                                                    "SELECT chunk_hash FROM file_chunks WHERE file_hash = ?1"
                                                                ) {
                                                                    if let Ok(rows) = stmt.query_map([file_hash], |row| row.get::<_, String>(0)) {
                                                                        for hash_result in rows {
                                                                            if let Ok(hash) = hash_result {
                                                                                if chunks_dir.join(&hash).exists() {
                                                                                    received += 1;
                                                                                }
                                                                            }
                                                                        }
                                                                    }
                                                                }
                                                                
                                                                println!("[ChunkTransfer] Progress: {}/{} chunks", received, expected);
                                                                
                                                                if received == expected && expected > 0 {
                                                                    // All chunks received! Mark file as complete
                                                                    let _ = conn.execute(
                                                                        "UPDATE files SET is_complete = 1 WHERE file_hash = ?1",
                                                                        [file_hash]
                                                                    );
                                                                    println!("[ChunkTransfer] ✅ File {} complete!", file_hash);
                                                                    is_complete = true;
                                                                }
                                                            }
                                                            is_complete
                                                        };
                                                        
                                                        if file_complete {
                                                            // Emit event to frontend (outside DB scope)
                                                            let _ = self.app_handle.emit(
                                                                "file-transfer-complete",
                                                                serde_json::json!({ "file_hash": file_hash })
                                                            );
                                                        }
                                                    } else {
                                                        eprintln!("[ChunkTransfer] ❌ Failed to decode chunk data");
                                                    }
                                                }
                                                
                                                // Acknowledge
                                                let response = DirectMessageResponse {
                                                    msg_id: request.id.clone(),
                                                    status: "delivered".to_string(),
                                                    error: None,
                                                };
                                                let _ = self
                                                    .swarm
                                                    .behaviour_mut()
                                                    .direct_message
                                                    .send_response(channel, response);
                                            }
                                            _ => {
                                                println!(
                                                    "[DM] Unknown message type: {}",
                                                    request.msg_type
                                                );
                                            }
                                        }
                                    }
                                    Message::Response {
                                        request_id,
                                        response,
                                    } => {
                                        println!(
                                            "[DM] 📦 Response for {:?}: {} for msg {}",
                                            request_id, response.status, response.msg_id
                                        );

                                        if response.status == "delivered" {
                                            // Update outgoing message status from "pending" to "delivered"
                                            use tauri::Manager;
                                            let state = self.app_handle.state::<crate::AppState>();
                                            if let Ok(conn) = state.db_conn.lock() {
                                                let _ = crate::storage::db::update_message_status(
                                                    &conn,
                                                    &response.msg_id,
                                                    "delivered",
                                                );
                                            }

                                            // Notify frontend
                                            let _ = self.app_handle.emit(
                                                "message-status-updated",
                                                serde_json::json!({
                                                    "msg_id": response.msg_id,
                                                    "status": "delivered",
                                                }),
                                            );
                                        }
                                    }
                                }
                            }
                            Event::OutboundFailure {
                                peer,
                                request_id,
                                error,
                                ..
                            } => {
                                eprintln!(
                                    "[DM] Outbound failure to {} for {:?}: {:?}",
                                    peer, request_id, error
                                );
                            }
                            Event::InboundFailure { peer, error, .. } => {
                                eprintln!("[DM] Inbound failure from {}: {:?}", peer, error);
                            }
                            _ => {}
                        }
                    }

                    // Handle expected events silently (no action needed)
                    RChatBehaviourEvent::Identify(_) => {
                        // Identify events are expected, no logging needed
                    }
                    RChatBehaviourEvent::Ping(_) => {
                        // Ping events are expected, no logging needed
                    }
                    RChatBehaviourEvent::Kademlia(_) => {
                        // Kademlia events are expected, no logging needed
                    }
                    
                    // Relay client events - for NAT traversal
                    RChatBehaviourEvent::RelayClient(event) => {
                        println!("[Relay] 📡 Event: {:?}", event);
                    }
                    
                    // DCUtR events - for hole punching
                    RChatBehaviourEvent::Dcutr(event) => {
                        println!("[DCUtR] 🔄 Event: {:?}", event);
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
            SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } => {
                println!("[Swarm] Connected to {}", peer_id);
                
                // Cleanup active punch targets
                let remote_addr = endpoint.get_remote_address();
                let mut to_remove = Vec::new();
                for (name, (addr, _)) in self.active_punch_targets.iter() {
                    // Check if the connected address matches the punch target address
                    // Since QUIC/UDP addresses might slightly vary (port mapping), check IP
                    let target_ip = addr.to_string().split('/').nth(2).unwrap_or("").to_string();
                    let connected_ip = remote_addr.to_string().split('/').nth(2).unwrap_or("").to_string();
                    
                    if !target_ip.is_empty() && target_ip == connected_ip {
                        to_remove.push(name.clone());
                    }
                }
                
                for name in to_remove {
                    self.remove_punch_target(&name);
                }
                
                // Check if this connection is from a GitHub invite dial
                // The endpoint contains the dialed address
                let remote_addr = remote_addr.to_string();
                
                // Check all pending mappings for partial match (IP portion)
                let mut matched_data = None;
                for (pending_addr, (inviter_user, my_user)) in self.pending_github_mappings.iter() {
                    // Match if the pending address contains same IP
                    if remote_addr.starts_with("/ip4/") && pending_addr.starts_with("/ip4/") {
                        // Extract IP from both
                        let pending_ip = pending_addr.split('/').nth(2);
                        let remote_ip = remote_addr.split('/').nth(2);
                        if pending_ip == remote_ip && pending_ip.is_some() {
                            matched_data = Some((pending_addr.clone(), inviter_user.clone(), my_user.clone()));
                            break;
                        }
                    }
                }
                
                if let Some((addr_key, inviter_github_user, my_username)) = matched_data {
                    self.pending_github_mappings.remove(&addr_key);
                    let peer_id_str = peer_id.to_string();
                    println!("[DIAL] ✅ GitHub user {} connected with PeerId {}", inviter_github_user, peer_id_str);
                    
                    // Store in config (inviter's PeerId)
                    let app_handle = self.app_handle.clone();
                    let gh_user = inviter_github_user.clone();
                    let peer_id_for_mapping = peer_id_str.clone();
                    tauri::async_runtime::spawn(async move {
                        let state = app_handle.state::<crate::AppState>();
                        let mgr = state.config_manager.lock().await;
                        if let Ok(mut config) = mgr.load().await {
                            config.user.github_peer_mapping.insert(gh_user.clone(), peer_id_for_mapping.clone());
                            if let Err(e) = mgr.save(&config).await {
                                eprintln!("[DIAL] Failed to save GitHub peer mapping: {}", e);
                            } else {
                                println!("[DIAL] ✅ Saved mapping: {} → {}", gh_user, peer_id_for_mapping);
                            }
                        }
                    });
                    
                    // Send handshake to inviter so they can create reverse mapping
                    // Use my_username from the stored tuple (no block_on needed)
                    println!("[HANDSHAKE] 🤝 Sending invite_handshake to {} with my username: {}", peer_id, my_username);
                    
                    use crate::network::direct_message::DirectMessageRequest;
                    let handshake = DirectMessageRequest {
                        id: format!("handshake-{}", std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs()),
                        sender_id: self.swarm.local_peer_id().to_string(),
                        msg_type: "invite_handshake".to_string(),
                        text_content: Some(my_username),
                        file_hash: None,
                        timestamp: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs() as i64,
                        chunk_hash: None,
                        chunk_data: None,
                        chunk_list: None,
                        sender_alias: None,
                    };
                    
                    self.swarm
                        .behaviour_mut()
                        .direct_message
                        .send_request(&peer_id, handshake);
                    println!("[HANDSHAKE] ✅ Handshake sent to {}", peer_id);
                }
            }
            SwarmEvent::ConnectionClosed {
                peer_id,
                num_established,
                ..
            } => {
                println!("[Swarm] Disconnected from {}", peer_id);

                // Only remove peer when ALL connections are closed
                if num_established == 0 {
                    // Remove from local_peers
                    if self.local_peers.remove(&peer_id).is_some() {
                        println!("[Swarm] Peer {} fully disconnected, notifying UI", peer_id);
                        // Emit event to frontend
                        let _ = self
                            .app_handle
                            .emit("local-peer-expired", peer_id.to_string());
                    }
                }
            }
            // CASE C: New Listener Address (expected at startup)
            SwarmEvent::NewListenAddr { address, .. } => {
                println!("[Swarm] Listening on: {}", address);

                // Store in NetworkState for invite creation (filter out localhost)
                let addr_str = address.to_string();
                if !addr_str.contains("127.0.0.1") && !addr_str.contains("::1") {
                    let app_handle = self.app_handle.clone();
                    let addr_clone = addr_str.clone();
                    tauri::async_runtime::spawn(async move {
                        let state = app_handle.state::<crate::NetworkState>();
                        let mut addrs = state.listening_addresses.lock().await;
                        if !addrs.contains(&addr_clone) {
                            addrs.push(addr_clone);
                        }
                    });
                }

                // If we haven't started mDNS yet, and this is a TCP address, start it!
                // IPv4 and IPv6 now share the same port, so any TCP address works
                if !self.mdns_started && address.to_string().contains("/tcp/") {
                    if let Some(port) = crate::network::get_port_from_multiaddr(&address) {
                        if port != 0 {
                            println!(
                                "[NetworkManager] Found TCP listen port: {}, starting mDNS...",
                                port
                            );
                            let peer_id = *self.swarm.local_peer_id();

                            // Get user alias from config (try_lock to avoid blocking in async context)
                            let user_alias = {
                                use tauri::Manager;
                                let state = self.app_handle.state::<crate::AppState>();
                                state
                                    .config_manager
                                    .try_lock()
                                    .ok()
                                    .and_then(|mgr| mgr.load_sync().ok())
                                    .and_then(|c| c.user.profile.alias.clone())
                            };

                            // Always advertise + browse at startup
                            if let Err(e) = crate::network::mdns::start_mdns_service(
                                peer_id,
                                port,
                                self.mdns_tx.clone(),
                                user_alias,
                            ) {
                                eprintln!("[NetworkManager] Failed to start mDNS: {}", e);
                            } else {
                                self.mdns_started = true;
                                println!("[NetworkManager] mDNS started (advertising + browsing)");
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
            // CASE F: Outgoing connection error (important for relay debugging)
            SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                eprintln!("[Swarm] ❌ Outgoing connection error to {:?}: {:?}", peer_id, error);
            }
            // CASE G: Listener error (relay listen failures)
            SwarmEvent::ListenerError { listener_id, error } => {
                eprintln!("[Swarm] ❌ Listener {:?} error: {:?}", listener_id, error);
            }
            // CASE H: Listener closed
            SwarmEvent::ListenerClosed { listener_id, reason, .. } => {
                eprintln!("[Swarm] Listener {:?} closed: {:?}", listener_id, reason);
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
                // Skip if already connected to this peer
                if self.swarm.is_connected(&peer_id) {
                    return; // Already connected, no need to dial
                }
                
                // 1. Add to known peers
                for addr_str in peer.addresses {
                    // Filter out invalid 0.0.0.0 addresses
                    if addr_str.contains("0.0.0.0") {
                        println!("[NetworkManager] ⚠️ Skipping invalid address: {}", addr_str);
                        continue;
                    }

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
    
    /// Register a pending shadow poll (called when creating an invite)
    /// REGISTER_SHADOW:invitee_username:password:my_username
    fn register_shadow_poll(&mut self, invitee: &str, password: &str, my_username: &str) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        
        self.pending_shadow_polls.insert(
            invitee.to_lowercase(),
            (password.to_string(), my_username.to_lowercase(), now)
        );
        println!("[Shadow] 📋 Registered poll for {}", invitee);
    }
    
    /// Poll for shadow invites from all pending invitees
    async fn poll_shadow_invites(&mut self) {
        use crate::network::gist;
        use crate::network::invite;
        
        // Skip if no pending polls
        if self.pending_shadow_polls.is_empty() {
            return;
        }
        
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        
        // Remove expired polls (2 minute TTL)
        self.pending_shadow_polls.retain(|_, (_, _, created)| now - *created < 120);
        
        // Clone keys to avoid borrow issues
        let invitees: Vec<String> = self.pending_shadow_polls.keys().cloned().collect();
        
        for invitee in invitees {
            let (password, my_username, _) = match self.pending_shadow_polls.get(&invitee) {
                Some(v) => v.clone(),
                None => continue,
            };
            
            // Fetch shadow invites from invitee's Gist
            match gist::get_friend_shadows(&invitee).await {
                Ok(shadows) => {
                    for shadow in shadows {
                        // Try to decrypt with our key
                        match invite::decrypt_shadow_invite(&shadow, &password, &my_username, &invitee) {
                            Ok(Some(payload)) => {
                                println!("[Shadow] 🎯 Found shadow from {}: {}", invitee, payload.invitee_address);
                                
                                // Add to active punch targets for continuous punching
                                if let Ok(addr) = payload.invitee_address.parse::<Multiaddr>() {
                                    self.add_punch_target(&invitee, addr);
                                }
                                
                                // Remove from pending shadow polls
                                self.pending_shadow_polls.remove(&invitee);
                            }
                            Ok(None) => {
                                // Wrong key or not for us, continue
                            }
                            Err(e) => {
                                eprintln!("[Shadow] Decrypt error: {}", e);
                            }
                        }
                    }
                }
                Err(e) => {
                    eprintln!("[Shadow] Failed to fetch shadows from {}: {}", invitee, e);
                }
            }
        }
    }
    
    /// Continuously punch all active targets (called every 500ms)
    fn punch_active_targets(&mut self) {
        if self.active_punch_targets.is_empty() {
            return;
        }
        
        let now = std::time::Instant::now();
        let timeout = std::time::Duration::from_secs(30);
        
        // Remove expired targets (older than 30 seconds)
        let expired: Vec<String> = self.active_punch_targets
            .iter()
            .filter(|(_, (_, start))| now.duration_since(*start) > timeout)
            .map(|(name, _)| name.clone())
            .collect();
        
        for name in expired {
            println!("[Punch] ⏰ Timeout for {}", name);
            self.active_punch_targets.remove(&name);
        }
        
        // Punch all remaining active targets
        for (name, (addr, start)) in &self.active_punch_targets {
            let attempt = (now.duration_since(*start).as_millis() / 500) + 1;
            let _ = self.swarm.dial(addr.clone());
            // Only log every 10th attempt to reduce spam
            if attempt % 10 == 1 || attempt <= 3 {
                println!("[Punch] 📤 {}/60 to {}", attempt.min(60), name);
            }
        }
    }
    
    /// Add a target to active punch list
    fn add_punch_target(&mut self, name: &str, addr: Multiaddr) {
        println!("[Punch] 🎯 Added target: {} -> {}", name, addr);
        self.active_punch_targets.insert(
            name.to_string(),
            (addr, std::time::Instant::now())
        );
    }
    
    /// Remove a target from active punch list (e.g., on connection success)
    fn remove_punch_target(&mut self, name: &str) -> bool {
        if self.active_punch_targets.remove(name).is_some() {
            println!("[Punch] 🎉 {} connected, removed from targets", name);
            true
        } else {
            false
        }
    }
}
