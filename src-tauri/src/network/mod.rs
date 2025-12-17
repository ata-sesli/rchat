mod behaviour;
mod manager;
mod discovery;
pub mod hks;
pub mod gist;
use libp2p::{PeerId, SwarmBuilder, identity};
use tauri::{AppHandle,Manager};
use tokio::sync::mpsc;
use anyhow::Result;

use crate::network::manager::NetworkManager;
use crate::network::behaviour::RChatBehaviour;

fn configure_noise(keypair: &libp2p::identity::Keypair) -> Result<libp2p::noise::Config, libp2p::noise::Error> {
    libp2p::noise::Config::new(keypair)
}

pub async fn init (app_handle: AppHandle) -> Result<()> {
    println!("[Backend] network::init starting...");
    let local_key = identity::Keypair::generate_ed25519();

    let local_peer_id = PeerId::from_public_key(&local_key.public());
    println!("[Backend] Local Peer ID: {local_peer_id}");

    println!("[Backend] Building swarm...");
    let mut swarm = SwarmBuilder::with_existing_identity(local_key.clone())
        .with_tokio()
        .with_tcp(libp2p::tcp::Config::default(),
        configure_noise,
        || {libp2p::yamux::Config::default()})?
        .with_quic()
        .with_behaviour(|key|{
            RChatBehaviour::new(key.clone())
        })?
        .with_swarm_config(|c| c.with_idle_connection_timeout(std::time::Duration::from_secs(60)))
        .build();
    
    println!("[Backend] Swarm built. Listening...");
    // Listen on all interfaces (Random Port)
    swarm.listen_on("/ip4/0.0.0.0/udp/0/quic-v1".parse()?)?;
    swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;
    println!("[Backend] Swarm listeners started.");

    let (ctx,crx) = mpsc::channel(32);
    
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

    // Initialize the P2P Swarm
    // This starts the infinite loop in manager.rs
    println!("[Backend] Spawning NetworkManager loop...");
    tauri::async_runtime::spawn(async move {
        println!("[Backend] NetworkManager starting");
        // Move the 'swarm' and 'app_handle' into this thread
        let manager = NetworkManager::new(swarm, crx, disc_rx, app_handle);
        
        // Run the infinite loop
        manager.run().await;
    });
    Ok(())
}