use crate::network::behaviour::{RChatBehaviour, RChatBehaviourEvent};
use futures::StreamExt;
use libp2p::{swarm::SwarmEvent, Multiaddr, Swarm};
use tauri::async_runtime::Receiver;
use tauri::AppHandle;
use tauri::Emitter;

pub struct NetworkManager {
    // The P2P Node itself
    swarm: Swarm<RChatBehaviour>,
    // The channel to receive commands FROM the UI
    crx: Receiver<String>,
    // The handle to send events TO the UI
    app_handle: AppHandle,
    disc_rx: Receiver<Multiaddr>,
}
impl NetworkManager {
    pub fn new(
        swarm: Swarm<RChatBehaviour>,
        crx: Receiver<String>,
        disc_rx: Receiver<Multiaddr>,
        app_handle: AppHandle,
    ) -> Self {
        Self {
            swarm,
            crx,
            disc_rx,
            app_handle,
        }
    }
    pub async fn run(mut self: Self) {
        println!("🛜 Network Manager: Running!");

        // Publish every 5 minutes
        let mut publish_interval = tokio::time::interval(std::time::Duration::from_secs(300));

        loop {
            tokio::select! {
                _ = publish_interval.tick() => {
                    self.publish_listeners().await;
                }
                Some(cmd) = self.crx.recv() => {
                    self.handle_ui_command(cmd);
                }
                Some(addr) = self.disc_rx.recv() => {
                    // Start dialing the peer found from Gist
                    println!("Using Gist Peer: {}", addr);
                    let _ = self.swarm.dial(addr);
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
        let token = {
            let mgr = state.config_manager.lock().await;
            if let Ok(config) = mgr.load().await {
                config.system.github_token.clone()
            } else {
                None
            }
        };

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

                        // FIRE EVENT TO SVELTE
                        // The frontend listens for "p2p-message"
                        let _ = self.app_handle.emit("p2p-message", text);
                    }
                    // 2. mDNS Event: We found a neighbour on Wi-Fi!
                    RChatBehaviourEvent::Mdns(libp2p::mdns::Event::Discovered(list)) => {
                        for (peer_id, _multiaddr) in list {
                            println!("mDNS: Found peer {}", peer_id);
                            // Auto-connect for gossip
                            self.swarm
                                .behaviour_mut()
                                .gossipsub
                                .add_explicit_peer(&peer_id);
                        }
                    }
                    _ => {}
                }
            }
            // CASE B: Connection Status Changes
            SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                print!("Connected to {}", peer_id);
            }
            SwarmEvent::ConnectionClosed { peer_id, .. } => {
                println!("Disconnected from {}", peer_id);
            }
            _ => {}
        }
    }
}
