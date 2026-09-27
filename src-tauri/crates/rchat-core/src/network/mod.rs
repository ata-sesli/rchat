mod behaviour;
pub mod command;
pub mod direct_message;
pub mod discovery;
pub mod endpoint;
#[cfg(test)]
mod endpoint_tests;
pub mod gist;
pub mod gossip;
pub mod hks;
pub mod invite;
mod manager;
pub mod mdns;
pub(crate) mod media_admission;
pub(crate) mod voice_stream;
use anyhow::Result;
use libp2p::{identity, PeerId, SwarmBuilder};
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::events::SharedCoreEventSink;
use crate::network::behaviour::RChatBehaviour;
use crate::network::manager::NetworkManager;

fn configure_noise(
    keypair: &libp2p::identity::Keypair,
) -> Result<libp2p::noise::Config, libp2p::noise::Error> {
    libp2p::noise::Config::new(keypair)
}

pub async fn start(
    app_state: crate::AppState,
    event_sink: SharedCoreEventSink,
) -> Result<crate::NetworkState> {
    println!("[Backend] network::init starting...");

    // Load or generate keypair (persistent across restarts)
    let local_key = {
        use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};

        let config_manager = app_state.config_manager.lock().await;
        let mut config = config_manager.load().await.unwrap_or_default();

        if let Some(ref key_b64) = config.user.libp2p_keypair {
            // Load existing keypair (saved as protobuf-encoded)
            if let Ok(key_bytes) = BASE64.decode(key_b64) {
                if let Ok(keypair) = identity::Keypair::from_protobuf_encoding(&key_bytes) {
                    println!("[Backend] Loaded existing keypair from config");
                    keypair
                } else {
                    // Invalid keypair format, generate new one
                    let new_key = identity::Keypair::generate_ed25519();
                    let key_bytes = new_key.to_protobuf_encoding().expect("keypair encoding");
                    config.user.libp2p_keypair = Some(BASE64.encode(&key_bytes));
                    let _ = config_manager.save(&config).await;
                    println!("[Backend] Generated new keypair (old format invalid)");
                    new_key
                }
            } else {
                // Decode failed, generate new one
                let new_key = identity::Keypair::generate_ed25519();
                let key_bytes = new_key.to_protobuf_encoding().expect("keypair encoding");
                config.user.libp2p_keypair = Some(BASE64.encode(&key_bytes));
                let _ = config_manager.save(&config).await;
                println!("[Backend] Generated new keypair (decode failed)");
                new_key
            }
        } else {
            // No keypair exists, generate and save
            let new_key = identity::Keypair::generate_ed25519();
            let key_bytes = new_key.to_protobuf_encoding().expect("keypair encoding");
            config.user.libp2p_keypair = Some(BASE64.encode(&key_bytes));
            let _ = config_manager.save(&config).await;
            println!("[Backend] Generated and saved new keypair");
            new_key
        }
    };

    let local_peer_id = PeerId::from_public_key(&local_key.public());
    if let Ok(mut cached_peer_id) = app_state.local_peer_id.write() {
        *cached_peer_id = Some(local_peer_id.to_string());
    }
    println!("[Backend] Local Peer ID: {local_peer_id}");

    println!("[Backend] Building swarm...");
    let public_endpoint = endpoint::EndpointObserver::default();
    let runtime = public_endpoint.runtime();
    let mut swarm = SwarmBuilder::with_existing_identity(local_key.clone())
        .with_tokio()
        .with_tcp(libp2p::tcp::Config::default(), configure_noise, || {
            libp2p::yamux::Config::default()
        })?
        .with_quic_config(|mut config| {
            config.runtime = Some(runtime);
            config
        })
        .with_dns()?
        .with_relay_client(configure_noise, || libp2p::yamux::Config::default())?
        .with_behaviour(|key, relay_client| RChatBehaviour::new(key.clone(), relay_client))?
        .with_swarm_config(|c| c.with_idle_connection_timeout(std::time::Duration::from_secs(60)))
        .build();

    println!("[Backend] Swarm built. Listening...");

    // Get a random available port first, then use it for both IPv4 and IPv6
    // This ensures mDNS advertises a port that works for both protocols
    let tcp_port = {
        let socket = std::net::TcpListener::bind("0.0.0.0:0")?;
        socket.local_addr()?.port()
    };
    let udp_port = {
        let socket = std::net::UdpSocket::bind("0.0.0.0:0")?;
        socket.local_addr()?.port()
    };

    println!(
        "[Backend] Using TCP port {} and UDP port {} for both IPv4 and IPv6",
        tcp_port, udp_port
    );

    // Observe STUN on these live QUIC sockets; never close and rebind the port.
    swarm.listen_on(format!("/ip6/::/udp/{}/quic-v1", udp_port).parse()?)?;
    swarm.listen_on(format!("/ip6/::/tcp/{}", tcp_port).parse()?)?;
    swarm.listen_on(format!("/ip4/0.0.0.0/udp/{}/quic-v1", udp_port).parse()?)?;
    swarm.listen_on(format!("/ip4/0.0.0.0/tcp/{}", tcp_port).parse()?)?;

    println!(
        "[Backend] Swarm listeners started (QUIC on port {}, TCP on port {})",
        udp_port, tcp_port
    );

    let (ctx, crx) = mpsc::channel(32);
    let connectivity_settings = {
        let mgr = app_state.config_manager.lock().await;
        mgr.load()
            .await
            .map(|c| c.user.connectivity.with_derived_mode())
            .unwrap_or_default()
    };

    // Store the sender in app state (with STUN results)
    let network_state = crate::NetworkState {
        sender: Arc::new(tokio::sync::Mutex::new(ctx)),
        local_peer_id: Arc::new(tokio::sync::Mutex::new(Some(local_peer_id.to_string()))),
        listening_addresses: Arc::new(tokio::sync::Mutex::new(vec![])),
        public_endpoint,
        temporary_state: Arc::new(tokio::sync::Mutex::new(
            crate::app_state::TemporaryRuntimeState::default(),
        )),
        connected_chat_ids: Arc::new(tokio::sync::Mutex::new(std::collections::HashSet::new())),
        chat_connections: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
        voice_call_state: Arc::new(tokio::sync::Mutex::new(
            crate::app_state::VoiceCallState::default(),
        )),
        broadcast_state: Arc::new(tokio::sync::Mutex::new(
            crate::app_state::BroadcastState::default(),
        )),
        connectivity: Arc::new(tokio::sync::Mutex::new(connectivity_settings)),
    };

    // 1. Create Discovery Channel
    let (disc_tx, disc_rx) = mpsc::channel(20);

    // 2. Spawn Discovery Task
    println!("[Backend] Spawning discovery task...");
    let discovery_state = app_state.clone();
    tokio::spawn(async move {
        println!("[Backend] Discovery task running");
        crate::network::discovery::discover_peers(disc_tx, discovery_state).await;
    });

    // 3. Create mDNS-SD Channel
    let (mdns_tx, mdns_rx) = mpsc::channel(20);

    // Initialize the P2P Swarm
    // This starts the infinite loop in manager.rs
    println!("[Backend] Spawning NetworkManager loop...");
    let manager_network_state = network_state.clone();
    tokio::spawn(async move {
        println!("[Backend] NetworkManager starting");
        let manager = NetworkManager::new(
            swarm,
            crx,
            disc_rx,
            mdns_rx,
            mdns_tx,
            app_state,
            manager_network_state,
            event_sink,
        );

        // Run the infinite loop
        manager.run().await;
    });
    Ok(network_state)
}

fn get_port_from_multiaddr(addr: &libp2p::Multiaddr) -> Option<u16> {
    use libp2p::multiaddr::Protocol;
    for proto in addr.iter() {
        if let Protocol::Tcp(port) = proto {
            return Some(port);
        }
        if let Protocol::Udp(port) = proto {
            return Some(port);
        }
    }
    None
}
