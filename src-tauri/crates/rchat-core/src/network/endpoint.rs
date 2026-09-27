//! Public endpoint observations belong to a live QUIC socket, not a local port number.
use quinn::{AsyncUdpSocket, Runtime, UdpPoller};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::{Duration, Instant};
use std::{
    fmt,
    future::Future,
    io::{self, IoSliceMut},
    pin::Pin,
    sync::{Arc, Mutex, Weak},
    task::{Context, Poll},
};

const SERVERS: &[&str] = &["stun.l.google.com:19302", "stun1.l.google.com:19302"];
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const RETRY_COOLDOWN: Duration = Duration::from_secs(5);

#[derive(Debug, Default, Clone)]
pub struct EndpointObserver(Arc<ObserverInner>);

#[derive(Debug, Default)]
struct ObserverInner {
    sockets: Mutex<Vec<Weak<ObservedSocket>>>,
    state: Mutex<ObservationState>,
    refresh: tokio::sync::Mutex<()>,
}

#[derive(Debug, Default)]
struct ObservationState {
    observation: Observation,
    generation: u64,
    attempted_at: Option<Instant>,
}

impl EndpointObserver {
    pub(crate) fn runtime(&self) -> Arc<dyn Runtime> {
        Arc::new(ObservedRuntime(self.clone()))
    }

    pub fn observation(&self) -> Observation {
        self.0.state.lock().unwrap().observation.clone()
    }

    pub(crate) fn needs_refresh(&self) -> bool {
        let state = self.0.state.lock().unwrap();
        state.observation.fresh(Instant::now()).is_none()
            && state
                .attempted_at
                .is_none_or(|at| at.elapsed() >= RETRY_COOLDOWN)
            && self.0.sockets.lock().unwrap().iter().any(|s| {
                s.upgrade()
                    .is_some_and(|s| s.local_addr().is_ok_and(|a| a.is_ipv4()))
            })
    }

    pub(crate) fn invalidate(&self) {
        let mut state = self.0.state.lock().unwrap();
        state.generation = state.generation.wrapping_add(1);
        state.observation.invalidate();
        state.attempted_at = None;
    }

    /// Coalesce concurrent callers. Failure preserves last-known for diagnostics only.
    pub async fn refresh(&self) -> Result<SocketAddr, String> {
        let _guard = self.0.refresh.lock().await;
        if let Some(addr) = self.observation().fresh(Instant::now()) {
            return Ok(addr);
        }
        if !self.needs_refresh() {
            return Err("No active socket or endpoint refresh in cooldown".into());
        }
        let mut servers = Vec::new();
        for name in SERVERS {
            if let Ok(Ok(addrs)) =
                tokio::time::timeout(PROBE_TIMEOUT, tokio::net::lookup_host(*name)).await
            {
                if let Some(addr) = addrs.into_iter().find(SocketAddr::is_ipv4) {
                    servers.push(addr);
                }
            }
        }
        self.probe(&servers, false).await
    }

    #[cfg(test)]
    pub(crate) async fn refresh_from(
        &self,
        servers: &[SocketAddr],
        force: bool,
    ) -> Result<SocketAddr, String> {
        let _guard = self.0.refresh.lock().await;
        self.probe(servers, force).await
    }

    async fn probe(&self, servers: &[SocketAddr], force: bool) -> Result<SocketAddr, String> {
        let generation = {
            let mut state = self.0.state.lock().unwrap();
            let now = Instant::now();
            if !force {
                if let Some(addr) = state.observation.fresh(now) {
                    return Ok(addr);
                }
                if state
                    .attempted_at
                    .is_some_and(|at| now.duration_since(at) < RETRY_COOLDOWN)
                {
                    return Err(
                        "Public endpoint refresh is cooling down after a failed attempt".into(),
                    );
                }
            }
            state.attempted_at = Some(now);
            state.observation.invalidate();
            state.generation
        };
        let socket = {
            let mut sockets = self.0.sockets.lock().unwrap();
            sockets.retain(|socket| socket.strong_count() > 0);
            sockets
                .iter()
                .filter_map(Weak::upgrade)
                .find(|s| s.local_addr().is_ok_and(|a| a.is_ipv4()))
        }
        .ok_or("No active IPv4 QUIC socket")?;
        for server in servers.iter().take(2) {
            if let Ok(Ok(address)) =
                tokio::time::timeout(PROBE_TIMEOUT, socket.query(*server)).await
            {
                if !address.is_ipv4() {
                    continue;
                }
                let mut state = self.0.state.lock().unwrap();
                if state.generation != generation {
                    return Err("Network changed during endpoint refresh".into());
                }
                state.observation.record(address, Instant::now());
                return Ok(address);
            }
        }
        Err("Could not verify the public endpoint of the active QUIC socket; using local listeners only".into())
    }
}

#[derive(Debug)]
struct ObservedRuntime(EndpointObserver);

impl Runtime for ObservedRuntime {
    fn new_timer(&self, at: Instant) -> Pin<Box<dyn quinn::AsyncTimer>> {
        quinn::TokioRuntime.new_timer(at)
    }
    fn spawn(&self, future: Pin<Box<dyn Future<Output = ()> + Send>>) {
        quinn::TokioRuntime.spawn(future);
    }
    fn wrap_udp_socket(&self, socket: std::net::UdpSocket) -> io::Result<Arc<dyn AsyncUdpSocket>> {
        let inner = quinn::TokioRuntime.wrap_udp_socket(socket)?;
        let socket = Arc::new(ObservedSocket {
            inner,
            pending: Mutex::new(None),
        });
        let mut sockets = self.0 .0.sockets.lock().unwrap();
        sockets.retain(|socket| socket.strong_count() > 0);
        sockets.push(Arc::downgrade(&socket));
        Ok(socket)
    }
}

struct Pending {
    server: SocketAddr,
    transaction: [u8; 12],
    reply: tokio::sync::oneshot::Sender<SocketAddr>,
}

struct ObservedSocket {
    inner: Arc<dyn AsyncUdpSocket>,
    pending: Mutex<Option<Pending>>,
}

impl fmt::Debug for ObservedSocket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ObservedSocket")
            .field("local_addr", &self.local_addr())
            .finish()
    }
}

struct PendingGuard<'a>(&'a ObservedSocket);
impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        self.0.pending.lock().unwrap().take();
    }
}

impl ObservedSocket {
    async fn query(&self, server: SocketAddr) -> io::Result<SocketAddr> {
        let transaction: [u8; 12] = rand::random();
        let mut request = [0; 20];
        request[1] = 1;
        request[4..8].copy_from_slice(&COOKIE.to_be_bytes());
        request[8..20].copy_from_slice(&transaction);
        let (reply, receive) = tokio::sync::oneshot::channel();
        *self.pending.lock().unwrap() = Some(Pending {
            server,
            transaction,
            reply,
        });
        let _guard = PendingGuard(self);
        let transmit = quinn::udp::Transmit {
            destination: server,
            contents: &request,
            ecn: None,
            segment_size: None,
            src_ip: None,
        };
        let mut poller = self.inner.clone().create_io_poller();
        futures::future::poll_fn(|cx| match self.inner.try_send(&transmit) {
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                match poller.as_mut().poll_writable(cx) {
                    Poll::Ready(Ok(())) => {
                        cx.waker().wake_by_ref();
                        Poll::Pending
                    }
                    other => other,
                }
            }
            result => Poll::Ready(result),
        })
        .await?;
        receive
            .await
            .map_err(|_| io::Error::other("STUN observation cancelled"))
    }
}

impl AsyncUdpSocket for ObservedSocket {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        self.inner.clone().create_io_poller()
    }
    fn try_send(&self, transmit: &quinn::udp::Transmit) -> io::Result<()> {
        self.inner.try_send(transmit)
    }
    fn poll_recv(
        &self,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [quinn::udp::RecvMeta],
    ) -> Poll<io::Result<usize>> {
        let count = match self.inner.poll_recv(cx, bufs, meta) {
            Poll::Ready(Ok(n)) => n,
            other => return other,
        };
        for i in 0..count {
            // A STUN response is one datagram. Do not consume coalesced QUIC data.
            if meta[i].len != meta[i].stride {
                continue;
            }
            let mut pending = self.pending.lock().unwrap();
            if let Some(probe) = pending.as_ref().filter(|p| p.server == meta[i].addr) {
                if let Some(address) = parse_response(&bufs[i][..meta[i].len], &probe.transaction) {
                    let probe = pending.take().unwrap();
                    let _ = probe.reply.send(address);
                    meta[i].len = 0;
                }
            }
        }
        Poll::Ready(Ok(count))
    }
    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.local_addr()
    }
    fn max_transmit_segments(&self) -> usize {
        self.inner.max_transmit_segments()
    }
    fn max_receive_segments(&self) -> usize {
        self.inner.max_receive_segments()
    }
    fn may_fragment(&self) -> bool {
        self.inner.may_fragment()
    }
}

pub(crate) const MAX_OBSERVATION_AGE: Duration = Duration::from_secs(30);
const COOKIE: u32 = 0x2112a442;

#[derive(Debug, Default, Clone)]
pub struct Observation {
    pub last_known: Option<SocketAddr>,
    pub verified_at: Option<Instant>,
}

impl Observation {
    pub fn fresh(&self, now: Instant) -> Option<SocketAddr> {
        self.verified_at
            .filter(|at| now.saturating_duration_since(*at) < MAX_OBSERVATION_AGE)
            .and(self.last_known)
    }

    pub(crate) fn record(&mut self, endpoint: SocketAddr, now: Instant) {
        self.last_known = Some(endpoint);
        self.verified_at = Some(now);
    }

    pub(crate) fn invalidate(&mut self) {
        self.verified_at = None;
    }
}

/// RFC 5389 binding success response, tied to our unpredictable transaction ID.
/// The socket demultiplexer additionally checks the response source address.
pub(crate) fn parse_response(packet: &[u8], transaction: &[u8; 12]) -> Option<SocketAddr> {
    if packet.len() < 20
        || packet[..2] != [1, 1]
        || packet[4..8] != COOKIE.to_be_bytes()
        || packet[8..20] != *transaction
    {
        return None;
    }
    let length = u16::from_be_bytes([packet[2], packet[3]]) as usize;
    if !length.is_multiple_of(4) || length + 20 != packet.len() {
        return None;
    }
    let mut offset = 20;
    let mut mapped = None;
    while offset < packet.len() {
        let header = packet.get(offset..offset + 4)?;
        let kind = u16::from_be_bytes([header[0], header[1]]);
        let length = u16::from_be_bytes([header[2], header[3]]) as usize;
        let data = packet.get(offset + 4..offset + 4 + length)?;
        offset += 4 + length.div_ceil(4) * 4;
        if offset > packet.len() {
            return None;
        }
        if kind != 0x0020 && kind != 0x0001 {
            continue;
        }
        if data.len() < 4 || data[0] != 0 {
            return None;
        }
        let xor = kind == 0x0020;
        let port = u16::from_be_bytes([data[2], data[3]]) ^ if xor { 0x2112 } else { 0 };
        let ip = match (data[1], data.len()) {
            (1, 8) => {
                let bits =
                    u32::from_be_bytes(data[4..8].try_into().ok()?) ^ if xor { COOKIE } else { 0 };
                IpAddr::V4(Ipv4Addr::from(bits))
            }
            (2, 20) => {
                let mut bytes: [u8; 16] = data[4..20].try_into().ok()?;
                if xor {
                    for (byte, mask) in bytes.iter_mut().zip(&packet[4..20]) {
                        *byte ^= mask;
                    }
                }
                IpAddr::V6(Ipv6Addr::from(bytes))
            }
            _ => return None,
        };
        let local_only = match ip {
            IpAddr::V4(ip) => ip.is_private() || ip.is_link_local() || ip.is_broadcast(),
            IpAddr::V6(ip) => ip.is_unique_local() || ip.is_unicast_link_local(),
        };
        if port == 0 || local_only || ip.is_unspecified() || ip.is_loopback() || ip.is_multicast() {
            return None;
        }
        // Prefer XOR-MAPPED-ADDRESS when both attributes are supplied.
        if xor || mapped.is_none() {
            mapped = Some(SocketAddr::new(ip, port));
        }
    }
    mapped
}

pub fn multiaddr(endpoint: SocketAddr) -> String {
    format!(
        "/ip{}/{}/udp/{}/quic-v1",
        if endpoint.is_ipv4() { 4 } else { 6 },
        endpoint.ip(),
        endpoint.port()
    )
}

pub(crate) fn advertised_addresses(
    observation: &Observation,
    listeners: &[String],
    now: Instant,
) -> Vec<String> {
    use libp2p::multiaddr::Protocol;
    let mut addresses: Vec<String> = observation.fresh(now).map(multiaddr).into_iter().collect();
    for address in listeners {
        let Ok(parsed) = address.parse::<libp2p::Multiaddr>() else {
            continue;
        };
        let usable = parsed.iter().any(|p| match p {
            Protocol::Ip4(ip) => !ip.is_unspecified() && !ip.is_loopback() && !ip.is_multicast(),
            Protocol::Ip6(ip) => !ip.is_unspecified() && !ip.is_loopback() && !ip.is_multicast(),
            _ => false,
        });
        if usable && !addresses.contains(address) {
            addresses.push(address.clone());
        }
    }
    addresses
}

/// Shared GUI/TUI invite selector. Local addresses remain fallback candidates,
/// never fabricated STUN observations. Last-known public endpoints are not used.
pub async fn resolve_address(net: &crate::NetworkState) -> anyhow::Result<String> {
    if let Err(error) = net.public_endpoint.refresh().await {
        eprintln!("[STUN] {error}");
    }
    current_addresses(net)
        .await
        .into_iter()
        .next()
        .ok_or_else(|| {
            anyhow::anyhow!("No usable listening address available. Is the network started?")
        })
}

pub async fn current_addresses(net: &crate::NetworkState) -> Vec<String> {
    advertised_addresses(
        &net.public_endpoint.observation(),
        &net.listening_addresses.lock().await,
        Instant::now(),
    )
}
