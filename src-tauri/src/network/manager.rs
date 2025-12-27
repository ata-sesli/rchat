use crate::network::behaviour::{RChatBehaviour, RChatBehaviourEvent};
use futures::StreamExt;
use libp2p::{swarm::SwarmEvent, Multiaddr, PeerId, Swarm};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use tauri::async_runtime::Receiver;
use tauri::AppHandle;
use tauri::Emitter;

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

        // Handle connection request command
        if msg_content.starts_with("REQUEST_CONNECTION:") {
            if let Some(peer_id_str) = msg_content.strip_prefix("REQUEST_CONNECTION:") {
                self.handle_connection_request(peer_id_str);
                return;
            }
        }

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
        if self.incoming_requests.contains(&peer_id) {
            println!("[Handshake] 🤝 Mutual handshake complete with {}!", peer_id);
            self.complete_handshake(peer_id);
            return;
        }

        // Otherwise, add to our pending requests and send request message
        self.pending_requests.insert(peer_id);
        println!("[Handshake] ⏳ Waiting for {} to accept...", peer_id);

        // Emit waiting state to frontend
        let _ = self.app_handle.emit("connection-waiting", peer_id_str);

        // Send connection request to peer via gossipsub
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

        // Add to known_devices database
        use tauri::Manager;
        let state = self.app_handle.state::<crate::AppState>();
        if let Ok(conn) = state.db_conn.lock() {
            if let Err(e) = crate::storage::db::save_known_device(
                &conn,
                &peer_id.to_string(),
                None, // device_name - can be updated later
            ) {
                eprintln!("[Handshake] Failed to save known device: {}", e);
            } else {
                println!("[Handshake] ✅ {} saved to known_devices!", peer_id);
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
                                };

                                if let Ok(conn) = state.db_conn.lock() {
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
                                    // Build request: 0x01 + file_hash bytes
                                    let mut request = vec![0x01u8];
                                    request.extend_from_slice(file_hash.as_bytes());

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
                                crate::storage::db::is_known_device(
                                    &conn,
                                    &sender_peer_id.to_string(),
                                )
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
                        use libp2p::request_response::{Event, Message};
                        match event {
                            Event::Message { peer, message, .. } => {
                                match message {
                                    Message::Request {
                                        request, channel, ..
                                    } => {
                                        // Request is Vec<u8> - first byte is command, rest is payload
                                        if request.len() > 1 && request[0] == 0x01 {
                                            // 0x01 = Request image by hash
                                            let file_hash =
                                                String::from_utf8_lossy(&request[1..]).to_string();
                                            println!("[File Transfer] 📥 Received request for image: {} from {}", file_hash, peer);

                                            use tauri::Manager;
                                            let state = self.app_handle.state::<crate::AppState>();

                                            let response = if let Ok(conn) = state.db_conn.lock() {
                                                match crate::storage::object::load(
                                                    &conn, &file_hash, None,
                                                ) {
                                                    Ok(data) => {
                                                        println!("[File Transfer] 📤 Sending {} bytes to {}", data.len(), peer);
                                                        data
                                                    }
                                                    Err(e) => {
                                                        eprintln!("[File Transfer] Failed to load image: {}", e);
                                                        vec![]
                                                    }
                                                }
                                            } else {
                                                vec![]
                                            };

                                            let _ = self
                                                .swarm
                                                .behaviour_mut()
                                                .direct_message
                                                .send_response(channel, response);
                                        }
                                    }
                                    Message::Response {
                                        request_id,
                                        response,
                                    } => {
                                        println!("[File Transfer] 📦 Received response for request {:?}: {} bytes", request_id, response.len());

                                        if !response.is_empty() {
                                            use tauri::Manager;
                                            let state = self.app_handle.state::<crate::AppState>();

                                            let result = {
                                                let conn = state.db_conn.lock().ok();
                                                if let Some(conn) = conn {
                                                    crate::storage::object::create(
                                                        &conn,
                                                        &response,
                                                        None,
                                                        Some("image/png"),
                                                        None,
                                                    )
                                                    .ok()
                                                } else {
                                                    None
                                                }
                                            };

                                            if let Some(file_hash) = result {
                                                println!(
                                                    "[File Transfer] ✅ Stored received image: {}",
                                                    file_hash
                                                );
                                                let _ = self.app_handle.emit(
                                                    "image-received",
                                                    serde_json::json!({
                                                        "file_hash": file_hash,
                                                    }),
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                            Event::OutboundFailure { peer, error, .. } => {
                                eprintln!(
                                    "[File Transfer] Outbound failure to {}: {:?}",
                                    peer, error
                                );
                            }
                            Event::InboundFailure { peer, error, .. } => {
                                eprintln!(
                                    "[File Transfer] Inbound failure from {}: {:?}",
                                    peer, error
                                );
                            }
                            _ => {}
                        }
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

                            // Always advertise + browse at startup
                            if let Err(e) = crate::network::mdns::start_mdns_service(
                                peer_id,
                                port,
                                self.mdns_tx.clone(),
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
