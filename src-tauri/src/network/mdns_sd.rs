use anyhow::Result;
use libp2p::PeerId;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::mpsc;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MdnsPeer {
    pub peer_id: String,
    pub addresses: Vec<String>,
}

pub fn start_mdns_service(
    peer_id: PeerId,
    port: u16,
    is_online: bool, // We can respect the online switch here
    sender: mpsc::Sender<MdnsPeer>,
) -> Result<()> {
    if !is_online {
        println!("[mDNS-SD] Offline mode: Not starting mDNS service.");
        return Ok(());
    }

    let mdns = ServiceDaemon::new()?;
    let service_type = "_rchat._tcp.local.";
    let instance_name = peer_id.to_string();

    // 1. Advertise Service
    let ip = "0.0.0.0"; // Allow mDNS to pick interfaces
    let host_name = format!("{}.local.", instance_name);

    let mut properties = HashMap::new();
    properties.insert("version".to_string(), "1.0.0".to_string());

    let service_info = ServiceInfo::new(
        service_type,
        &instance_name,
        &host_name,
        ip,
        port,
        properties,
    )?
    .enable_addr_auto();

    mdns.register(service_info)?;
    println!(
        "[mDNS-SD] Registered service: {} at port {}",
        instance_name, port
    );

    // 2. Browse for Peers
    let receiver = mdns.browse(service_type)?;

    // Spawn a thread to handle events (mdns-sd uses std::sync::mpsc)
    std::thread::spawn(move || {
        while let Ok(event) = receiver.recv() {
            match event {
                ServiceEvent::ServiceResolved(info) => {
                    let discovered_peer_id =
                        info.get_fullname().split('.').next().unwrap_or("unknown");

                    // Ignore self
                    if discovered_peer_id.contains(&instance_name) {
                        continue;
                    }

                    println!("[mDNS-SD] Resolved peer: {}", discovered_peer_id);

                    let addresses: Vec<String> = info
                        .get_addresses()
                        .iter()
                        .map(|ip| format!("/ip4/{}/tcp/{}", ip, info.get_port()))
                        .collect();

                    let peer = MdnsPeer {
                        peer_id: discovered_peer_id.to_string(),
                        addresses,
                    };

                    // Send to manager
                    if let Err(e) = sender.blocking_send(peer) {
                        eprintln!("[mDNS-SD] Failed to send peer to manager: {}", e);
                        break;
                    }
                }
                _ => {} // Ignore other events for now
            }
        }
    });

    Ok(())
}
