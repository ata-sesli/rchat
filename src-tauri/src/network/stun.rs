//! STUN Client for NAT Traversal (IPv6-First)
//!
//! Uses public STUN servers to discover our public IP address.
//! Tries IPv6 first, falls back to IPv4.

use std::net::{SocketAddr, UdpSocket, Ipv4Addr, Ipv6Addr, IpAddr};
use std::time::Duration;

/// Public STUN servers (most support both IPv4 and IPv6)
const STUN_SERVERS: &[&str] = &[
    "stun.l.google.com:19302",
    "stun1.l.google.com:19302", 
    "stun2.l.google.com:19302",
    "stun.services.mozilla.com:3478",
    "stun.nextcloud.com:3478",
];

// STUN constants
const STUN_BINDING_RESPONSE: u16 = 0x0101;
const STUN_MAGIC_COOKIE: u32 = 0x2112A442;
const ATTR_XOR_MAPPED_ADDRESS: u16 = 0x0020;
const ATTR_MAPPED_ADDRESS: u16 = 0x0001;

/// Result of STUN discovery
#[derive(Debug, Clone)]
pub struct StunResult {
    pub ipv6: Option<SocketAddr>,
    pub ipv4: Option<SocketAddr>,
}

impl StunResult {
    /// Get the best address (IPv6 preferred)
    pub fn best(&self) -> Option<SocketAddr> {
        self.ipv6.or(self.ipv4)
    }
}

/// Discover public addresses using STUN (IPv6 first)
pub async fn discover_public_addresses() -> StunResult {
    let mut result = StunResult { ipv6: None, ipv4: None };
    
    // Try to get both IPv6 and IPv4 addresses
    for server in STUN_SERVERS {
        // Resolve all addresses for the server
        let addrs: Vec<SocketAddr> = match tokio::net::lookup_host(server).await {
            Ok(iter) => iter.collect(),
            Err(_) => continue,
        };
        
        // Try IPv6 first if we don't have one yet
        if result.ipv6.is_none() {
            if let Some(v6_server) = addrs.iter().find(|a| a.is_ipv6()) {
                if let Ok(addr) = query_stun_v6(*v6_server).await {
                    println!("[STUN] ✅ IPv6 discovered: {} (from {})", addr, server);
                    result.ipv6 = Some(addr);
                }
            }
        }
        
        // Try IPv4 if we don't have one yet
        if result.ipv4.is_none() {
            if let Some(v4_server) = addrs.iter().find(|a| a.is_ipv4()) {
                if let Ok(addr) = query_stun_v4(*v4_server).await {
                    println!("[STUN] ✅ IPv4 discovered: {} (from {})", addr, server);
                    result.ipv4 = Some(addr);
                }
            }
        }
        
        // Stop if we have both
        if result.ipv6.is_some() && result.ipv4.is_some() {
            break;
        }
    }
    
    if result.ipv6.is_none() && result.ipv4.is_none() {
        eprintln!("[STUN] ❌ No public address discovered");
    }
    
    result
}

/// Query STUN server via IPv6
async fn query_stun_v6(server: SocketAddr) -> Result<SocketAddr, String> {
    let socket = UdpSocket::bind("[::]:0")
        .map_err(|e| format!("Failed to bind IPv6: {}", e))?;
    socket.set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|e| format!("Failed to set timeout: {}", e))?;
    
    query_stun_raw(&socket, server)
}

/// Query STUN server via IPv4
async fn query_stun_v4(server: SocketAddr) -> Result<SocketAddr, String> {
    let socket = UdpSocket::bind("0.0.0.0:0")
        .map_err(|e| format!("Failed to bind IPv4: {}", e))?;
    socket.set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|e| format!("Failed to set timeout: {}", e))?;
    
    query_stun_raw(&socket, server)
}

/// Raw STUN query
fn query_stun_raw(socket: &UdpSocket, server: SocketAddr) -> Result<SocketAddr, String> {
    // Build STUN Binding Request
    let mut request = [0u8; 20];
    request[0] = 0x00; request[1] = 0x01; // Binding Request
    request[2] = 0x00; request[3] = 0x00; // Length: 0
    request[4] = 0x21; request[5] = 0x12; request[6] = 0xA4; request[7] = 0x42; // Magic cookie
    for i in 8..20 { request[i] = rand::random(); } // Transaction ID
    
    socket.send_to(&request, server)
        .map_err(|e| format!("Send failed: {}", e))?;
    
    let mut buf = [0u8; 1024];
    let (len, _) = socket.recv_from(&mut buf)
        .map_err(|e| format!("Recv failed: {}", e))?;
    
    if len < 20 {
        return Err("Response too short".to_string());
    }
    
    let msg_type = u16::from_be_bytes([buf[0], buf[1]]);
    if msg_type != STUN_BINDING_RESPONSE {
        return Err(format!("Bad message type: 0x{:04x}", msg_type));
    }
    
    // Parse attributes
    let msg_len = u16::from_be_bytes([buf[2], buf[3]]) as usize;
    let mut offset = 20;
    
    while offset + 4 <= 20 + msg_len && offset + 4 <= len {
        let attr_type = u16::from_be_bytes([buf[offset], buf[offset + 1]]);
        let attr_len = u16::from_be_bytes([buf[offset + 2], buf[offset + 3]]) as usize;
        
        if offset + 4 + attr_len > len { break; }
        let data = &buf[offset + 4..offset + 4 + attr_len];
        
        match attr_type {
            ATTR_XOR_MAPPED_ADDRESS => {
                let family = data[1];
                if family == 0x01 && attr_len >= 8 { // IPv4
                    let port = u16::from_be_bytes([data[2], data[3]]) ^ ((STUN_MAGIC_COOKIE >> 16) as u16);
                    let ip = u32::from_be_bytes([data[4], data[5], data[6], data[7]]) ^ STUN_MAGIC_COOKIE;
                    return Ok(SocketAddr::new(Ipv4Addr::from(ip).into(), port));
                } else if family == 0x02 && attr_len >= 20 { // IPv6
                    let port = u16::from_be_bytes([data[2], data[3]]) ^ ((STUN_MAGIC_COOKIE >> 16) as u16);
                    // XOR with magic cookie + transaction ID
                    let mut ip_bytes = [0u8; 16];
                    let xor_bytes = &buf[4..20]; // magic + txid
                    for i in 0..16 {
                        ip_bytes[i] = data[4 + i] ^ xor_bytes[i];
                    }
                    return Ok(SocketAddr::new(Ipv6Addr::from(ip_bytes).into(), port));
                }
            }
            ATTR_MAPPED_ADDRESS => {
                let family = data[1];
                if family == 0x01 && attr_len >= 8 { // IPv4
                    let port = u16::from_be_bytes([data[2], data[3]]);
                    let ip = Ipv4Addr::new(data[4], data[5], data[6], data[7]);
                    return Ok(SocketAddr::new(ip.into(), port));
                } else if family == 0x02 && attr_len >= 20 { // IPv6
                    let port = u16::from_be_bytes([data[2], data[3]]);
                    let mut ip_bytes = [0u8; 16];
                    ip_bytes.copy_from_slice(&data[4..20]);
                    return Ok(SocketAddr::new(Ipv6Addr::from(ip_bytes).into(), port));
                }
            }
            _ => {}
        }
        
        offset += 4 + ((attr_len + 3) & !3);
    }
    
    Err("No mapped address found".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_stun_discovery() {
        let result = discover_public_addresses().await;
        println!("IPv6: {:?}, IPv4: {:?}", result.ipv6, result.ipv4);
        assert!(result.best().is_some());
    }
}
