use anyhow::Result;
use libp2p::PeerId;
use local_ip_address::local_ip;
use serde::{Deserialize, Serialize};
use std::any::Any;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use zeroconf::prelude::*;
use zeroconf::{BrowserEvent, MdnsBrowser, MdnsService, ServiceType, TxtRecord};

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
    let instance_name = peer_id.to_string();

    // Get local IP for logging
    let local_ip = local_ip()
        .map(|ip| ip.to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    // Get hostname for proper DNS - fall back to a PeerID-derived name if invalid
    let raw_hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "rchat-host".to_string());

    // DNS hostnames must start with a letter
    let valid_hostname = if raw_hostname
        .chars()
        .next()
        .map(|c| c.is_ascii_digit())
        .unwrap_or(true)
    {
        format!(
            "rchat-{}",
            &instance_name[..std::cmp::min(12, instance_name.len())]
        )
    } else {
        raw_hostname.chars().take(32).collect()
    };

    println!(
        "[mDNS-Zeroconf] Advertising as: {} (hostname: {}, IP: {}) on port {}",
        instance_name, valid_hostname, local_ip, port
    );

    // Clone values for the registration thread
    let instance_name_reg = instance_name.clone();
    let valid_hostname_reg = valid_hostname.clone();

    // Spawn registration thread
    std::thread::spawn(move || {
        if let Err(e) = run_service_registration(instance_name_reg, valid_hostname_reg, port) {
            eprintln!("[mDNS-Zeroconf] Registration error: {}", e);
        }
    });

    // Spawn browser thread
    let my_peer_id = instance_name.clone();
    std::thread::spawn(move || {
        if let Err(e) = run_service_browser(sender, my_peer_id) {
            eprintln!("[mDNS-Zeroconf] Browser error: {}", e);
        }
    });

    Ok(())
}

fn run_service_registration(instance_name: String, hostname: String, port: u16) -> Result<()> {
    let service_type = ServiceType::new("rchat", "tcp")
        .map_err(|e| anyhow::anyhow!("Invalid service type: {:?}", e))?;

    let mut service = MdnsService::new(service_type, port);

    // Create TXT record with peer_id
    let mut txt_record = TxtRecord::new();
    txt_record
        .insert("peer_id", &instance_name)
        .map_err(|e| anyhow::anyhow!("Failed to insert TXT record: {:?}", e))?;
    txt_record
        .insert("version", "1.0.0")
        .map_err(|e| anyhow::anyhow!("Failed to insert TXT record: {:?}", e))?;
    txt_record
        .insert("protocol", "rchat/1.0")
        .map_err(|e| anyhow::anyhow!("Failed to insert TXT record: {:?}", e))?;

    service.set_name(&hostname);
    service.set_txt_record(txt_record);
    service.set_registered_callback(Box::new(on_service_registered));

    let event_loop = service
        .register()
        .map_err(|e| anyhow::anyhow!("Failed to register service: {:?}", e))?;

    println!("[mDNS-Zeroconf] ✅ Service registered, polling...");

    // Keep polling to maintain the service
    loop {
        if let Err(e) = event_loop.poll(Duration::from_secs(1)) {
            eprintln!("[mDNS-Zeroconf] Poll error: {:?}", e);
        }
    }
}

fn on_service_registered(
    result: zeroconf::Result<zeroconf::ServiceRegistration>,
    _context: Option<Arc<dyn Any + Send + Sync>>,
) {
    match result {
        Ok(registration) => {
            println!("[mDNS-Zeroconf] ✅ Registered: {}", registration.name());
        }
        Err(e) => {
            eprintln!("[mDNS-Zeroconf] Registration failed: {:?}", e);
        }
    }
}

fn run_service_browser(sender: mpsc::Sender<MdnsPeer>, my_peer_id: String) -> Result<()> {
    let service_type = ServiceType::new("rchat", "tcp")
        .map_err(|e| anyhow::anyhow!("Invalid service type: {:?}", e))?;

    let mut browser = MdnsBrowser::new(service_type);

    // Wrap sender and my_peer_id in Arc for callback
    let sender = Arc::new(std::sync::Mutex::new(sender));
    let my_peer_id = Arc::new(my_peer_id);

    let sender_clone = sender.clone();
    let my_peer_id_clone = my_peer_id.clone();

    browser.set_service_callback(Box::new(move |result, _context| {
        handle_browser_event(result, &sender_clone, &my_peer_id_clone);
    }));

    let event_loop = browser
        .browse_services()
        .map_err(|e| anyhow::anyhow!("Failed to start browsing: {:?}", e))?;

    println!("[mDNS-Zeroconf] Started browsing for _rchat._tcp...");

    // Keep polling to receive events
    loop {
        if let Err(e) = event_loop.poll(Duration::from_secs(1)) {
            eprintln!("[mDNS-Zeroconf] Browse poll error: {:?}", e);
        }
    }
}

fn handle_browser_event(
    result: zeroconf::Result<BrowserEvent>,
    sender: &Arc<std::sync::Mutex<mpsc::Sender<MdnsPeer>>>,
    my_peer_id: &Arc<String>,
) {
    match result {
        Ok(BrowserEvent::Add(discovery)) => {
            let addr = discovery.address();
            println!(
                "[mDNS-Zeroconf] 🔍 Discovered: {} at {}:{}",
                discovery.name(),
                addr,
                discovery.port()
            );

            // Extract peer_id from TXT record
            let txt = discovery.txt();
            let discovered_peer_id = txt
                .as_ref()
                .and_then(|t| t.get("peer_id"))
                .unwrap_or_else(|| discovery.name().to_string());

            // Skip self
            if discovered_peer_id == **my_peer_id {
                println!("[mDNS-Zeroconf] Skipping self");
                return;
            }

            // Build address - address() returns &String directly
            let port = discovery.port();
            let multiaddr = format!("/ip4/{}/tcp/{}", addr, port);

            let peer = MdnsPeer {
                peer_id: discovered_peer_id,
                addresses: vec![multiaddr],
            };

            // Send to manager
            if let Ok(sender) = sender.lock() {
                if let Err(e) = sender.blocking_send(peer) {
                    eprintln!("[mDNS-Zeroconf] Failed to send peer: {}", e);
                }
            }
        }
        Ok(BrowserEvent::Remove(removal)) => {
            println!("[mDNS-Zeroconf] ❌ Service removed: {}", removal.name());
        }
        Err(e) => {
            eprintln!("[mDNS-Zeroconf] Browser event error: {:?}", e);
        }
    }
}
