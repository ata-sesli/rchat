mod behaviour;
mod discovery;
pub mod gist;
pub mod hks;
mod manager;
pub mod mdns_sd; // New module
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
    let local_key = identity::Keypair::generate_ed25519();

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

    // 4. Start mDNS-SD Service
    // We need to wait for swarm to start listening to get the port,
    // but we configured it to listen on random port '0'.
    // We will assume that SwarmEvent::NewListenAddr will trigger later,
    // BUT we need to register the service now.
    // Ideally we should wait for the first NewListenAddr event.
    // For now, let's start the service but we need the actual port.
    // HACK: We can't know the port until the swarm starts.
    // So we will pass the mdns_tx to NetworkManager and let it start mDNS
    // when it receives NewListenAddr !!
    // BUT NetworkManager is consuming the channel, not producing.

    // BETTER APPROACH: Spawn mDNS task here, but delay registration?
    // OR: Just hardcode a port for now? No, we use random ports.

    // Correction: We can query the listener address immediately after `swarm.listen_on` returns!
    let mut listen_port = 0;
    for addr in swarm.listeners() {
        if let Some(p) = get_port_from_multiaddr(addr) {
            listen_port = p;
            break;
        }
    }
    println!("[Backend] Determined listen port: {}", listen_port);

    // Get online status
    let state = app_handle.state::<crate::AppState>();
    let is_online = {
        let mgr = state.config_manager.lock().await;
        mgr.load().await.map(|c| c.user.is_online).unwrap_or(false)
    };

    if let Err(e) = crate::network::mdns_sd::start_mdns_service(
        local_peer_id.clone(),
        listen_port,
        is_online,
        mdns_tx,
    ) {
        eprintln!("Failed to start mDNS-SD service: {}", e);
    }

    // Initialize the P2P Swarm
    // This starts the infinite loop in manager.rs
    println!("[Backend] Spawning NetworkManager loop...");
    tauri::async_runtime::spawn(async move {
        println!("[Backend] NetworkManager starting");
        // Move the 'swarm' and 'app_handle' into this thread
        let manager = NetworkManager::new(swarm, crx, disc_rx, mdns_rx, app_handle);

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
