use super::*;

mod connections;
mod direct_message;
mod gossipsub;

impl NetworkManager {
    pub async fn handle_swarm_event(&mut self, event: SwarmEvent<RChatBehaviourEvent>) {
        match event {
            SwarmEvent::Behaviour(behaviour_event) => match behaviour_event {
                RChatBehaviourEvent::Gossipsub(libp2p::gossipsub::Event::Message {
                    message,
                    ..
                }) => {
                    self.handle_gossipsub_message(message).await;
                }
                RChatBehaviourEvent::DirectMessage(event) => {
                    self.handle_direct_message_event(event).await;
                }
                RChatBehaviourEvent::VoiceCall(event) => {
                    self.handle_voice_frame_event(event).await;
                }
                RChatBehaviourEvent::VideoCall(event) => {
                    self.handle_video_frame_event(event).await;
                }
                RChatBehaviourEvent::Identify(_) => {}
                RChatBehaviourEvent::Ping(_) => {}
                RChatBehaviourEvent::Kademlia(_) => {}
                RChatBehaviourEvent::RelayClient(event) => {
                    println!("[Relay] 📡 Event: {:?}", event);
                }
                RChatBehaviourEvent::Dcutr(event) => {
                    println!("[DCUtR] 🔄 Event: {:?}", event);
                }
                other => {
                    eprintln!(
                        "[Event Debug] Unhandled behaviour event: {:?}",
                        std::any::type_name_of_val(&other)
                    );
                }
            },
            SwarmEvent::ConnectionEstablished {
                peer_id, endpoint, ..
            } => {
                self.handle_connection_established(peer_id, endpoint).await;
            }
            SwarmEvent::ConnectionClosed {
                peer_id,
                num_established,
                endpoint,
                ..
            } => {
                self.handle_connection_closed(peer_id, num_established, endpoint)
                    .await;
            }
            SwarmEvent::NewListenAddr { address, .. } => {
                self.handle_new_listen_addr(address);
            }
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
            SwarmEvent::Dialing { peer_id, .. } => {
                if let Some(peer) = peer_id {
                    println!("[Swarm] Dialing peer: {}", peer);
                }
            }
            SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                eprintln!(
                    "[Swarm] ❌ Outgoing connection error to {:?}: {:?}",
                    peer_id, error
                );
            }
            SwarmEvent::ListenerError { listener_id, error } => {
                eprintln!("[Swarm] ❌ Listener {:?} error: {:?}", listener_id, error);
            }
            SwarmEvent::ListenerClosed {
                listener_id,
                reason,
                ..
            } => {
                eprintln!("[Swarm] Listener {:?} closed: {:?}", listener_id, reason);
            }
            other => {
                eprintln!(
                    "[Swarm Debug] Other event: {:?}",
                    std::any::type_name_of_val(&other)
                );
            }
        }
    }

    pub(super) fn handle_mdns_peer(&mut self, peer: crate::network::mdns::MdnsPeer) {
        if !self.is_mdns_enabled() {
            return;
        }

        println!("[NetworkManager] Received mDNS peer: {}", peer.peer_id);

        // Parse peer ID
        let peer_id_res = peer.peer_id.parse::<PeerId>();
        match peer_id_res {
            Ok(peer_id) => {
                // Skip if already connected to this peer
                if self.swarm.is_connected(&peer_id) {
                    // Still emit/update discovery so frontend local-scan can show connected peers
                    // even when they were connected before the modal/listener was opened.
                    for addr_str in peer.addresses {
                        if addr_str.contains("0.0.0.0") {
                            continue;
                        }
                        if let Ok(addr) = addr_str.parse::<Multiaddr>() {
                            let entry = self.local_peers.entry(peer_id).or_insert_with(Vec::new);
                            if !entry.iter().any(|existing| existing == &addr) {
                                entry.push(addr);
                            }
                        }
                    }

                    let peer_info = LocalPeer {
                        peer_id: peer.peer_id.clone(),
                        addresses: self
                            .local_peers
                            .get(&peer_id)
                            .map(|a| a.iter().map(|m| m.to_string()).collect())
                            .unwrap_or_default(),
                    };
                    let _ = self.app_handle.emit("local-peer-discovered", peer_info);
                    return;
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
                        let entry = self.local_peers.entry(peer_id).or_insert_with(Vec::new);
                        if !entry.iter().any(|existing| existing == &addr) {
                            entry.push(addr);
                        }

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
