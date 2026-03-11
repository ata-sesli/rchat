use anyhow::{anyhow, Result};
use libp2p::PeerId;
use local_ip_address::local_ip;
use serde::{Deserialize, Serialize};
use std::any::Any;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;
use tokio::sync::mpsc;
use zeroconf::prelude::*;
use zeroconf::{BrowserEvent, MdnsBrowser, MdnsService, ServiceType, TxtRecord};

static MDNS_INITIALIZED: AtomicBool = AtomicBool::new(false);
/// When true, use fast requery interval (5s) - for active discovery mode
static FAST_DISCOVERY: AtomicBool = AtomicBool::new(false);

/// Enable fast discovery mode (called when Add Person modal opens)
pub fn enable_fast_discovery() {
    FAST_DISCOVERY.store(true, Ordering::SeqCst);
    println!("[mDNS] ⚡ Fast discovery mode enabled (5s interval)");
}

/// Disable fast discovery mode (called when Add Person modal closes)
pub fn disable_fast_discovery() {
    FAST_DISCOVERY.store(false, Ordering::SeqCst);
    println!("[mDNS] 🐢 Normal discovery mode (30s interval)");
}

/// Get current requery interval based on discovery mode
fn get_requery_interval() -> Duration {
    if FAST_DISCOVERY.load(Ordering::SeqCst) {
        Duration::from_secs(5)
    } else {
        Duration::from_secs(30)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MdnsPeer {
    pub peer_id: String,
    pub addresses: Vec<String>,
    pub device_name: Option<String>,
    pub alias: Option<String>, // User's display name from TXT record
}

pub struct MdnsServiceHandle {
    shutdown: Arc<AtomicBool>,
    registration_thread: Option<JoinHandle<()>>,
    browser_thread: Option<JoinHandle<()>>,
}

impl MdnsServiceHandle {
    pub fn stop(&mut self) {
        if self.registration_thread.is_none() && self.browser_thread.is_none() {
            return;
        }

        self.shutdown.store(true, Ordering::SeqCst);

        if let Some(handle) = self.registration_thread.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.browser_thread.take() {
            let _ = handle.join();
        }

        MDNS_INITIALIZED.store(false, Ordering::SeqCst);
        println!("[mDNS] 🧹 Service threads stopped");
    }
}

impl Drop for MdnsServiceHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Start mDNS service - always advertises and browses at startup
pub fn start_mdns_service(
    peer_id: PeerId,
    port: u16,
    sender: mpsc::Sender<MdnsPeer>,
    user_alias: Option<String>, // User's alias from settings
) -> Result<MdnsServiceHandle> {
    if MDNS_INITIALIZED.swap(true, Ordering::SeqCst) {
        return Err(anyhow!("mDNS already initialized"));
    }

    let instance_name = peer_id.to_string();
    let local_ip = local_ip()
        .map(|ip| ip.to_string())
        .unwrap_or_else(|_| "unknown".to_string());

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
        "[mDNS] 📡 Starting service: {} (hostname: {}, IP: {}) on port {}",
        instance_name, valid_hostname, local_ip, port
    );

    let shutdown = Arc::new(AtomicBool::new(false));

    // Spawn registration thread (advertising)
    let instance_name_reg = instance_name.clone();
    let valid_hostname_reg = valid_hostname.clone();
    let alias_reg = user_alias.clone();
    let reg_shutdown = shutdown.clone();
    let registration_thread = std::thread::spawn(move || {
        if let Err(e) = run_service_registration(
            instance_name_reg,
            valid_hostname_reg,
            port,
            alias_reg,
            reg_shutdown,
        ) {
            eprintln!("[mDNS] Registration error: {}", e);
        }
    });

    // Spawn browser thread (discovery)
    let my_peer_id = instance_name;
    let browser_shutdown = shutdown.clone();
    let browser_thread = std::thread::spawn(move || {
        if let Err(e) = run_service_browser(sender, my_peer_id, browser_shutdown) {
            eprintln!("[mDNS] Browser error: {}", e);
        }
    });

    Ok(MdnsServiceHandle {
        shutdown,
        registration_thread: Some(registration_thread),
        browser_thread: Some(browser_thread),
    })
}

fn run_service_registration(
    instance_name: String,
    hostname: String,
    port: u16,
    user_alias: Option<String>,
    shutdown: Arc<AtomicBool>,
) -> Result<()> {
    let service_type = ServiceType::new("rchat", "udp")
        .map_err(|e| anyhow::anyhow!("Invalid service type: {:?}", e))?;

    let mut service = MdnsService::new(service_type, port);

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

    // Add user alias if set
    if let Some(alias) = &user_alias {
        txt_record
            .insert("alias", alias)
            .map_err(|e| anyhow::anyhow!("Failed to insert alias TXT record: {:?}", e))?;
    }

    service.set_name(&hostname);
    service.set_txt_record(txt_record);
    service.set_registered_callback(Box::new(on_service_registered));

    let event_loop = service
        .register()
        .map_err(|e| anyhow::anyhow!("Failed to register service: {:?}", e))?;

    println!("[mDNS] ✅ Service registered, polling...");

    while !shutdown.load(Ordering::SeqCst) {
        if let Err(e) = event_loop.poll(Duration::from_secs(1)) {
            eprintln!("[mDNS] Poll error: {:?}", e);
        }
    }

    println!("[mDNS] Registration loop stopped");
    Ok(())
}

fn on_service_registered(
    result: zeroconf::Result<zeroconf::ServiceRegistration>,
    _context: Option<Arc<dyn Any + Send + Sync>>,
) {
    match result {
        Ok(registration) => {
            println!("[mDNS] ✅ Registered: {}", registration.name());
        }
        Err(e) => {
            eprintln!("[mDNS] Registration failed: {:?}", e);
        }
    }
}

fn run_service_browser(
    sender: mpsc::Sender<MdnsPeer>,
    my_peer_id: String,
    shutdown: Arc<AtomicBool>,
) -> Result<()> {
    let service_type = ServiceType::new("rchat", "udp")
        .map_err(|e| anyhow::anyhow!("Invalid service type: {:?}", e))?;

    let sender = Arc::new(std::sync::Mutex::new(sender));
    let my_peer_id = Arc::new(my_peer_id);

    println!("[mDNS] Started browsing for _rchat._udp...");

    while !shutdown.load(Ordering::SeqCst) {
        let mut browser = MdnsBrowser::new(service_type.clone());

        let sender_clone = sender.clone();
        let my_peer_id_clone = my_peer_id.clone();

        browser.set_service_callback(Box::new(move |result, _context| {
            handle_browser_event(result, &sender_clone, &my_peer_id_clone);
        }));

        match browser.browse_services() {
            Ok(event_loop) => {
                let start = std::time::Instant::now();
                let requery_interval = get_requery_interval();

                while start.elapsed() < requery_interval && !shutdown.load(Ordering::SeqCst) {
                    if let Err(e) = event_loop.poll(Duration::from_secs(1)) {
                        eprintln!("[mDNS] Browse poll error: {:?}", e);
                    }
                }

                if !shutdown.load(Ordering::SeqCst) {
                    println!("[mDNS] 🔄 Re-querying mDNS services...");
                }
            }
            Err(e) => {
                eprintln!("[mDNS] Failed to start browsing: {:?}", e);
                for _ in 0..5 {
                    if shutdown.load(Ordering::SeqCst) {
                        break;
                    }
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        }
    }

    println!("[mDNS] Browser loop stopped");
    Ok(())
}

fn handle_browser_event(
    result: zeroconf::Result<BrowserEvent>,
    sender: &Arc<std::sync::Mutex<mpsc::Sender<MdnsPeer>>>,
    my_peer_id: &Arc<String>,
) {
    match result {
        Ok(BrowserEvent::Add(discovery)) => {
            let mut addr = discovery.address().to_string();
            let device_name = discovery.name().to_string();
            let port = discovery.port();

            // If address is 0.0.0.0, try to resolve hostname
            if addr == "0.0.0.0" {
                let hostname = discovery.host_name();
                if !hostname.is_empty() {
                    // Try DNS resolution of the hostname
                    if let Ok(ips) =
                        std::net::ToSocketAddrs::to_socket_addrs(&format!("{}:{}", hostname, port))
                    {
                        for socket_addr in ips {
                            if socket_addr.ip().is_ipv4() && !socket_addr.ip().is_loopback() {
                                addr = socket_addr.ip().to_string();
                                println!("[mDNS] 🔍 Resolved {} -> {}", hostname, addr);
                                break;
                            }
                        }
                    }
                }
            }

            // Extract peer_id from TXT record first for self-check
            let txt = discovery.txt();
            let discovered_peer_id = txt
                .as_ref()
                .and_then(|t| t.get("peer_id"))
                .unwrap_or_else(|| device_name.clone());

            // Skip self - don't even log it
            if discovered_peer_id == **my_peer_id {
                return;
            }

            println!("[mDNS] 🔍 Discovered: {} at {}:{}", device_name, addr, port);

            let discovered_alias = txt.as_ref().and_then(|t| t.get("alias"));

            let multiaddr = format!("/ip4/{}/udp/{}/quic-v1", addr, port);

            let peer = MdnsPeer {
                peer_id: discovered_peer_id,
                addresses: vec![multiaddr],
                device_name: Some(device_name),
                alias: discovered_alias,
            };

            if let Ok(sender) = sender.lock() {
                if let Err(e) = sender.blocking_send(peer) {
                    eprintln!("[mDNS] Failed to send peer: {}", e);
                }
            }
        }
        Ok(BrowserEvent::Remove(removal)) => {
            println!("[mDNS] ❌ Service removed: {}", removal.name());
        }
        Err(e) => {
            eprintln!("[mDNS] Browser event error: {:?}", e);
        }
    }
}
