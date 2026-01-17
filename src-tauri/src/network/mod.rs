mod behaviour;
pub mod direct_message;
pub mod discovery;
pub mod gist;
pub mod hks;
pub mod invite;
mod manager;
pub mod mdns;
pub mod stun;
use anyhow::Result;
use libp2p::{identity, PeerId, SwarmBuilder};
use tauri::{AppHandle, Manager};
use tokio::sync::mpsc;

use crate::network::behaviour::RChatBehaviour;
use crate::network::manager::NetworkManager;

fn configure_noise(
    keypair: &libp2p::identity::Keypair,
) -> Result<libp2p::noise::Config, libp2p::noise::Error> {
    libp2p::noise::Config::new(keypair)
}

pub async fn init(app_handle: AppHandle) -> Result<()> {
    println!("[Backend] network::init starting...");

    // Load or generate keypair (persistent across restarts)
    let local_key = {
        use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
        use tauri::Manager;

        let state = app_handle.state::<crate::AppState>();
        let config_manager = state.config_manager.lock().await;
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
    println!("[Backend] Local Peer ID: {local_peer_id}");

    println!("[Backend] Building swarm...");
    let mut swarm = SwarmBuilder::with_existing_identity(local_key.clone())
        .with_tokio()
        .with_tcp(libp2p::tcp::Config::default(), configure_noise, || {
            libp2p::yamux::Config::default()
        })?
        .with_quic()
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
    
    println!("[Backend] Using TCP port {} and UDP port {} for both IPv4 and IPv6", tcp_port, udp_port);
    
    // Do STUN discovery (socket closes after discovery)
    let stun_result = stun::discover_on_port(udp_port).await;
    let stun_external_port = stun_result.external_port;
    let stun_public_ip = stun_result.ipv4.map(|a| a.ip().to_string());
    
    if let Some(ext_port) = stun_external_port {
        println!("[Backend] STUN external port: {} (local: {})", ext_port, udp_port);
    }
    
    // Bind QUIC to the SAME port (socket was closed after STUN discovery)
    // On most NATs, binding to the same local port gets the same external mapping
    swarm.listen_on(format!("/ip6/::/udp/{}/quic-v1", udp_port).parse()?)?;
    swarm.listen_on(format!("/ip6/::/tcp/{}", tcp_port).parse()?)?;
    swarm.listen_on(format!("/ip4/0.0.0.0/udp/{}/quic-v1", udp_port).parse()?)?;
    swarm.listen_on(format!("/ip4/0.0.0.0/tcp/{}", tcp_port).parse()?)?;
    
    println!("[Backend] Swarm listeners started (QUIC on port {}, TCP on port {})", udp_port, tcp_port);
    
    // NOTE: STUN socket closed, QUIC now owns the port
    // On most NATs, QUIC will get the same external port mapping
    // If the invite is used quickly, this should work
    // TODO: If NAT mapping expires, we'd need bidirectional punching

    let (ctx, crx) = mpsc::channel(32);

    // Store the sender in app state (with STUN results)
    let network_state = crate::NetworkState {
        sender: tokio::sync::Mutex::new(ctx),
        listening_addresses: tokio::sync::Mutex::new(vec![]),
        public_address_v6: tokio::sync::Mutex::new(None),
        public_address_v4: tokio::sync::Mutex::new(stun_public_ip),
        stun_external_port: tokio::sync::Mutex::new(stun_external_port),
    };
    app_handle.manage(network_state);

    // 1. Create Discovery Channel
    let (disc_tx, disc_rx) = mpsc::channel(20);

    // 2. Spawn Discovery Task
    println!("[Backend] Spawning discovery task...");
    let discovery_handle = app_handle.clone();
    tauri::async_runtime::spawn(async move {
        println!("[Backend] Discovery task running");
        crate::network::discovery::discover_peers(disc_tx, discovery_handle).await;
    });

    // 3. Create mDNS-SD Channel
    let (mdns_tx, mdns_rx) = mpsc::channel(20);

    // Initialize the P2P Swarm
    // This starts the infinite loop in manager.rs
    println!("[Backend] Spawning NetworkManager loop...");
    tauri::async_runtime::spawn(async move {
        println!("[Backend] NetworkManager starting");
        // Move the 'swarm' and 'app_handle' into this thread
        let manager = NetworkManager::new(swarm, crx, disc_rx, mdns_rx, mdns_tx, app_handle);

        // Run the infinite loop
        manager.run().await;
    });
    Ok(())
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
