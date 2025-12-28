mod behaviour;
mod discovery;
pub mod gist;
pub mod hks;
mod manager;
pub mod mdns; // New module
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
            // Load existing keypair
            if let Ok(key_bytes) = BASE64.decode(key_b64) {
                if let Ok(keypair) = identity::Keypair::ed25519_from_bytes(key_bytes.clone()) {
                    println!("[Backend] Loaded existing keypair from config");
                    keypair
                } else {
                    // Invalid keypair, generate new one
                    let new_key = identity::Keypair::generate_ed25519();
                    let key_bytes = new_key.to_protobuf_encoding().expect("keypair encoding");
                    config.user.libp2p_keypair = Some(BASE64.encode(&key_bytes));
                    let _ = config_manager.save(&config).await;
                    println!("[Backend] Generated new keypair (old was invalid)");
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
        .with_behaviour(|key| RChatBehaviour::new(key.clone()))?
        .with_swarm_config(|c| c.with_idle_connection_timeout(std::time::Duration::from_secs(60)))
        .build();

    println!("[Backend] Swarm built. Listening...");
    // Listen on all interfaces (Random Port)
    swarm.listen_on("/ip4/0.0.0.0/udp/0/quic-v1".parse()?)?;
    swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;
    println!("[Backend] Swarm listeners started.");

    let (ctx, crx) = mpsc::channel(32);

    // Store the sender in app state
    let network_state = crate::NetworkState {
        sender: tokio::sync::Mutex::new(ctx),
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
