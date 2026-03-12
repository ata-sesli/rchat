//! STUN client for NAT traversal.
//! Discovers the IPv4 public endpoint for the exact UDP port used by QUIC.

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::time::Duration;

/// Public STUN servers (most support both IPv4 and IPv6).
const STUN_SERVERS: &[&str] = &[
    "stun.l.google.com:19302",
    "stun1.l.google.com:19302",
    "stun2.l.google.com:19302",
    "stun.services.mozilla.com:3478",
    "stun.nextcloud.com:3478",
];

const STUN_BINDING_RESPONSE: u16 = 0x0101;
const STUN_MAGIC_COOKIE: u32 = 0x2112A442;
const ATTR_XOR_MAPPED_ADDRESS: u16 = 0x0020;
const ATTR_MAPPED_ADDRESS: u16 = 0x0001;

#[derive(Debug, Clone)]
pub struct StunResult {
    pub ipv6: Option<SocketAddr>,
    pub ipv4: Option<SocketAddr>,
    pub external_port: Option<u16>,
}

/// Discover public address using a specific local UDP port.
/// This must match the QUIC listener port for reliable NAT hole punching.
pub async fn discover_on_port(local_port: u16) -> StunResult {
    let mut result = StunResult {
        ipv6: None,
        ipv4: None,
        external_port: None,
    };

    println!("[STUN] 🔍 Discovering on local port {}...", local_port);

    let socket = match UdpSocket::bind(format!("0.0.0.0:{}", local_port)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[STUN] ❌ Failed to bind to port {}: {}", local_port, e);
            return result;
        }
    };
    socket.set_read_timeout(Some(Duration::from_secs(2))).ok();

    for server in STUN_SERVERS {
        let addrs: Vec<SocketAddr> = match tokio::net::lookup_host(server).await {
            Ok(iter) => iter.collect(),
            Err(_) => continue,
        };

        if let Some(v4_server) = addrs.iter().find(|a| a.is_ipv4()) {
            if let Ok(addr) = query_stun_raw(&socket, *v4_server) {
                println!("[STUN] ✅ External address: {} (from {})", addr, server);
                result.ipv4 = Some(addr);
                result.external_port = Some(addr.port());
                break;
            }
        }
    }

    if result.ipv4.is_none() {
        eprintln!(
            "[STUN] ❌ No external address discovered on port {}",
            local_port
        );
    }

    result
}

fn query_stun_raw(socket: &UdpSocket, server: SocketAddr) -> Result<SocketAddr, String> {
    // Build STUN binding request
    let mut request = [0u8; 20];
    request[0] = 0x00;
    request[1] = 0x01; // Binding Request
    request[2] = 0x00;
    request[3] = 0x00; // Length: 0
    request[4] = 0x21;
    request[5] = 0x12;
    request[6] = 0xA4;
    request[7] = 0x42; // Magic cookie
    for byte in &mut request[8..20] {
        *byte = rand::random(); // Transaction ID
    }

    socket
        .send_to(&request, server)
        .map_err(|e| format!("Send failed: {}", e))?;

    let mut buf = [0u8; 1024];
    let (len, _) = socket
        .recv_from(&mut buf)
        .map_err(|e| format!("Recv failed: {}", e))?;

    if len < 20 {
        return Err("Response too short".to_string());
    }

    let msg_type = u16::from_be_bytes([buf[0], buf[1]]);
    if msg_type != STUN_BINDING_RESPONSE {
        return Err(format!("Bad message type: 0x{:04x}", msg_type));
    }

    let msg_len = u16::from_be_bytes([buf[2], buf[3]]) as usize;
    let mut offset = 20;

    while offset + 4 <= 20 + msg_len && offset + 4 <= len {
        let attr_type = u16::from_be_bytes([buf[offset], buf[offset + 1]]);
        let attr_len = u16::from_be_bytes([buf[offset + 2], buf[offset + 3]]) as usize;

        if offset + 4 + attr_len > len {
            break;
        }
        let data = &buf[offset + 4..offset + 4 + attr_len];

        match attr_type {
            ATTR_XOR_MAPPED_ADDRESS => {
                let family = data[1];
                if family == 0x01 && attr_len >= 8 {
                    // IPv4
                    let port =
                        u16::from_be_bytes([data[2], data[3]]) ^ ((STUN_MAGIC_COOKIE >> 16) as u16);
                    let ip = u32::from_be_bytes([data[4], data[5], data[6], data[7]])
                        ^ STUN_MAGIC_COOKIE;
                    return Ok(SocketAddr::new(Ipv4Addr::from(ip).into(), port));
                }
                if family == 0x02 && attr_len >= 20 {
                    // IPv6
                    let port =
                        u16::from_be_bytes([data[2], data[3]]) ^ ((STUN_MAGIC_COOKIE >> 16) as u16);
                    let mut ip_bytes = [0u8; 16];
                    let xor_bytes = &buf[4..20]; // magic cookie + txid
                    for i in 0..16 {
                        ip_bytes[i] = data[4 + i] ^ xor_bytes[i];
                    }
                    return Ok(SocketAddr::new(Ipv6Addr::from(ip_bytes).into(), port));
                }
            }
            ATTR_MAPPED_ADDRESS => {
                let family = data[1];
                if family == 0x01 && attr_len >= 8 {
                    // IPv4
                    let port = u16::from_be_bytes([data[2], data[3]]);
                    let ip = Ipv4Addr::new(data[4], data[5], data[6], data[7]);
                    return Ok(SocketAddr::new(ip.into(), port));
                }
                if family == 0x02 && attr_len >= 20 {
                    // IPv6
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
    #[ignore = "requires external network reachability"]
    async fn test_stun_discovery_on_port() {
        let result = discover_on_port(0).await;
        assert!(result.ipv4.is_some() || result.ipv6.is_some());
    }
}
