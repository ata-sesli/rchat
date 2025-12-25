use anyhow::{Context, Result};
use libp2p::PeerId;
use local_ip_address::local_ip;
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
    _is_online: bool,
    sender: mpsc::Sender<MdnsPeer>,
) -> Result<()> {
    let mdns = ServiceDaemon::new().context("Failed to create mDNS daemon")?;
    let service_type = "_rchat._tcp.local.";
    let instance_name = peer_id.to_string();

    // 1. Advertise with REAL IP
    let local_ip = local_ip().context("Failed to get local IP")?.to_string();

    // Get hostname for proper DNS - but fall back to a PeerID-derived name if invalid
    let raw_hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "rchat-host".to_string());

    // DNS hostnames must start with a letter. If hostname starts with a digit (like an IP),
    // use a PeerID-derived hostname instead. Also limit length for DNS compatibility.
    let valid_hostname = if raw_hostname
        .chars()
        .next()
        .map(|c| c.is_ascii_digit())
        .unwrap_or(true)
    {
        // Hostname is invalid (starts with digit or empty), use "rchat-" + first 12 chars of PeerID
        format!(
            "rchat-{}",
            &instance_name[..std::cmp::min(12, instance_name.len())]
        )
    } else {
        // Hostname is valid, use it but limit length
        raw_hostname.chars().take(32).collect()
    };

    println!(
        "[mDNS-SD] Advertising as: {} (hostname: {}, IP: {}) on port {}",
        instance_name, valid_hostname, local_ip, port
    );

    let mut properties = HashMap::new();
    properties.insert("version".to_string(), "1.0.0".to_string());
    properties.insert("peer_id".to_string(), instance_name.clone());
    properties.insert("protocol".to_string(), "rchat/1.0".to_string());

    // Use peer_id as instance name for guaranteed uniqueness across network
    let service_info = ServiceInfo::new(
        service_type,
        &instance_name, // Use PeerID as instance name for uniqueness
        &format!("{}.local.", valid_hostname), // Use VALID hostname
        &local_ip,
        port,
        properties,
    )?
    .enable_addr_auto();

    mdns.register(service_info)
        .context("Failed to register service")?;

    println!("[mDNS-SD] ✅ Service registered: {}:{}", local_ip, port);

    // 2. Browse for Peers
    let receiver = mdns
        .browse(service_type)
        .context("Failed to start browsing")?;

    // Clone for the thread
    let thread_sender = sender.clone();
    let my_peer_id = instance_name.clone();

    std::thread::spawn(move || {
        println!("[mDNS-SD] Started browsing for {}...", service_type);

        while let Ok(event) = receiver.recv() {
            match event {
                ServiceEvent::ServiceFound(service, iface) => {
                    println!(
                        "[mDNS-SD] 🔍 ServiceFound: {} on interface {:?}",
                        service, iface
                    );
                    // mdns-sd should auto-resolve, but log it to confirm events arrive
                }

                ServiceEvent::ServiceResolved(info) => {
                    // Extract peer ID from TXT records
                    let txt_records: HashMap<String, String> = info
                        .get_properties()
                        .iter()
                        .map(|p| {
                            (
                                p.key().to_string(),
                                String::from_utf8_lossy(p.val().unwrap_or(&[])).to_string(),
                            )
                        })
                        .collect();

                    let discovered_peer_id =
                        txt_records.get("peer_id").cloned().unwrap_or_else(|| {
                            // Fallback: extract from instance name if no TXT record
                            info.get_fullname()
                                .split('.')
                                .next()
                                .unwrap_or("unknown")
                                .to_string()
                        });

                    // Skip self
                    if discovered_peer_id == my_peer_id {
                        continue;
                    }

                    // Get ALL IP addresses (IPv4 and IPv6)
                    let addresses: Vec<String> = info
                        .get_addresses()
                        .iter()
                        .filter(|ip| !ip.is_loopback()) // Skip 127.0.0.1 if possible
                        .flat_map(|ip| {
                            // Create both TCP and potential QUIC addresses
                            let mut addrs = vec![
                                format!("/ip4/{}/tcp/{}", ip, info.get_port()),
                                // format!("/ip6/{}/tcp/{}", ip, info.get_port()),
                            ];
                            // We focus on IPv4 for now as per previous config,
                            // but mdns-sd returns IpAddr which can be V4 or V6.
                            // The string format needs to match.

                            // If this is V6, the format /ip4/ is wrong.
                            if ip.is_ipv6() {
                                // For now, we mainly support IPv4 in RChatBehaviour?
                                // Let's just log it or add it as /ip6/
                                vec![format!("/ip6/{}/tcp/{}", ip, info.get_port())]
                            } else {
                                addrs
                            }
                        })
                        .collect();

                    if addresses.is_empty() {
                        // If only loopback was found, maybe we should use it if nothing else?
                        // But usually we want real IPs.
                        continue;
                    }

                    println!(
                        "[mDNS-SD] ✅ Resolved {} at {:?}",
                        discovered_peer_id, addresses
                    );

                    let peer = MdnsPeer {
                        peer_id: discovered_peer_id,
                        addresses,
                    };

                    // Send to manager
                    if let Err(e) = thread_sender.blocking_send(peer) {
                        eprintln!("[mDNS-SD] Failed to send peer to manager: {}", e);
                        break;
                    }
                }
                other_event => {
                    println!("[mDNS-SD Debug] Event: {:?}", other_event);
                }
            }
        }
        println!("[mDNS-SD] Browsing thread exiting");
    });

    Ok(())
}
