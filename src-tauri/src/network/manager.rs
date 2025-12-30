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

        // Subscribe to the global-chat topic to receive messages
        let topic = libp2p::gossipsub::IdentTopic::new("global-chat");
        if let Err(e) = self.swarm.behaviour_mut().gossipsub.subscribe(&topic) {
            eprintln!("[Gossipsub] Failed to subscribe to global-chat: {:?}", e);
        } else {
            println!("[Gossipsub] ✅ Subscribed to global-chat topic");
        }

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

        // Handle direct messages (DM:peer_id:msg_id:timestamp:content)
        if msg_content.starts_with("DM:") {
            let parts: Vec<&str> = msg_content.splitn(5, ':').collect();
            if parts.len() >= 5 {
                let target_peer_id = parts[1];
                let msg_id = parts[2];
                let timestamp: i64 = parts[3].parse().unwrap_or(0);
                let content = parts[4];

                println!(
                    "[DM] 📤 Sending direct message to {}: {}",
                    target_peer_id, content
                );

                // Find the peer in connected peers
                if let Ok(peer_id) = target_peer_id.parse::<PeerId>() {
                    use crate::network::direct_message::DirectMessageRequest;
                    let request = DirectMessageRequest {
                        id: msg_id.to_string(),
                        sender_id: self.swarm.local_peer_id().to_string(),
                        msg_type: "text".to_string(),
                        text_content: Some(content.to_string()),
                        file_hash: None,
                        timestamp,
                    };

                    self.swarm
                        .behaviour_mut()
                        .direct_message
                        .send_request(&peer_id, request);
                    println!("[DM] ✅ Request sent to {}", peer_id);
                } else {
                    eprintln!("[DM] ❌ Invalid peer_id: {}", target_peer_id);
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
                        text_content: Some(msg_ids.to_string()), // Message IDs that were read
                        file_hash: None,
                        timestamp: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs() as i64,
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
                                        msg_type: "file_request".to_string(),
                                        text_content: None,
                                        file_hash: Some(file_hash.to_string()),
                                        timestamp,
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
                                                };

                                                if let Ok(conn) = state.db_conn.lock() {
                                                    // Ensure peer and chat exist
                                                    if !crate::storage::db::is_peer(
                                                        &conn,
                                                        &request.sender_id,
                                                    ) {
                                                        let _ = crate::storage::db::add_peer(
                                                            &conn,
                                                            &request.sender_id,
                                                            None,
                                                            None,
                                                            "direct",
                                                        );
                                                    }
                                                    if !crate::storage::db::chat_exists(
                                                        &conn,
                                                        &request.sender_id,
                                                    ) {
                                                        let _ = crate::storage::db::create_chat(
                                                            &conn,
                                                            &request.sender_id,
                                                            &request.sender_id, // name = sender_id for 1:1 chats
                                                            false,
                                                        );
                                                    }

                                                    if let Err(e) =
                                                        crate::storage::db::insert_message(
                                                            &conn, &db_msg,
                                                        )
                                                    {
                                                        eprintln!(
                                                            "[DM] Failed to save message: {}",
                                                            e
                                                        );
                                                    } else {
                                                        println!("[DM] ✅ Message saved");
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
                                            "file_request" => {
                                                // Legacy file transfer support
                                                if let Some(ref file_hash) = request.file_hash {
                                                    println!(
                                                        "[DM] 📥 File request for: {}",
                                                        file_hash
                                                    );
                                                    // Note: File transfer via request-response needs separate handling
                                                    // For now just acknowledge
                                                    let response = DirectMessageResponse {
                                                        msg_id: request.id.clone(),
                                                        status: "error".to_string(),
                                                        error: Some(
                                                            "File transfer not yet implemented"
                                                                .to_string(),
                                                        ),
                                                    };
                                                    let _ = self
                                                        .swarm
                                                        .behaviour_mut()
                                                        .direct_message
                                                        .send_response(channel, response);
                                                }
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
}
