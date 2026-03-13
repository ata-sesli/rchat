use crate::network::behaviour::{RChatBehaviour, RChatBehaviourEvent};
use crate::network::command::NetworkCommand;
use crate::network::gossip::GroupMessageEnvelope;
use futures::StreamExt;
use libp2p::{swarm::SwarmEvent, Multiaddr, PeerId, Swarm};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use tauri::async_runtime::Receiver;
use tauri::{AppHandle, Emitter, Manager};

mod persistence;
mod punching;
mod run_loop;
mod swarm_events;
mod transfer;
mod ui_commands;
#[path = "../../live/video/manager.rs"]
mod video_call;
#[path = "../../live/voice/manager.rs"]
mod voice_call;

#[cfg(test)]
mod tests;

const NAT_KEEPALIVE_ADDR: &str = "/ip4/1.1.1.1/udp/9/quic-v1";

#[derive(Clone, Serialize)]
pub struct LocalPeer {
    pub peer_id: String,
    pub addresses: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveCallPhase {
    OutgoingRinging,
    IncomingRinging,
    Active,
}

#[derive(Clone)]
struct ActiveCall {
    call_id: String,
    kind: crate::app_state::CallKind,
    peer_chat_id: String,
    remote_peer_id: PeerId,
    phase: ActiveCallPhase,
    ring_deadline: Option<std::time::Instant>,
    ring_expires_at: Option<i64>,
    started_at: Option<i64>,
    muted: bool,
    camera_enabled: bool,
}

#[derive(Debug, Default, Clone, Copy)]
struct PeerTransportState {
    quic_connections: usize,
    tcp_connections: usize,
}

#[derive(Debug, Default, Clone)]
struct PeerTransportRegistry {
    by_peer: HashMap<PeerId, PeerTransportState>,
}

impl PeerTransportRegistry {
    fn is_quic_addr(addr: &Multiaddr) -> bool {
        let raw = addr.to_string();
        raw.contains("/quic-v1") || raw.contains("/quic/")
    }

    fn is_tcp_addr(addr: &Multiaddr) -> bool {
        addr.to_string().contains("/tcp/")
    }

    fn record_connected(&mut self, peer_id: PeerId, remote_addr: &Multiaddr) {
        let state = self.by_peer.entry(peer_id).or_default();
        if Self::is_quic_addr(remote_addr) {
            state.quic_connections = state.quic_connections.saturating_add(1);
        } else if Self::is_tcp_addr(remote_addr) {
            state.tcp_connections = state.tcp_connections.saturating_add(1);
        }
    }

    fn record_disconnected(&mut self, peer_id: PeerId, remote_addr: &Multiaddr) -> bool {
        let Some(state) = self.by_peer.get_mut(&peer_id) else {
            return false;
        };
        let had_quic = state.quic_connections > 0;

        if Self::is_quic_addr(remote_addr) {
            state.quic_connections = state.quic_connections.saturating_sub(1);
        } else if Self::is_tcp_addr(remote_addr) {
            state.tcp_connections = state.tcp_connections.saturating_sub(1);
        }

        let has_quic = state.quic_connections > 0;
        if state.quic_connections == 0 && state.tcp_connections == 0 {
            self.by_peer.remove(&peer_id);
        }
        had_quic && !has_quic
    }

    fn has_quic(&self, peer_id: &PeerId) -> bool {
        self.by_peer
            .get(peer_id)
            .map(|state| state.quic_connections > 0)
            .unwrap_or(false)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OutgoingDialSource {
    NatKeepalive,
    Mdns,
    Gist,
    Punch,
    Unknown,
}

impl OutgoingDialSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::NatKeepalive => "nat_keepalive",
            Self::Mdns => "mdns",
            Self::Gist => "gist",
            Self::Punch => "punch",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone)]
struct RecentDial {
    source: OutgoingDialSource,
    at: std::time::Instant,
}

fn extract_candidate_multiaddr_from_error_debug(error_debug: &str) -> Option<String> {
    let start = error_debug.find("/ip")?;
    let tail = &error_debug[start..];
    let end = tail
        .find(',')
        .or_else(|| tail.find(')'))
        .unwrap_or(tail.len());
    let candidate = tail[..end].trim();
    if candidate.is_empty() {
        return None;
    }
    candidate.parse::<Multiaddr>().ok().map(|_| candidate.to_string())
}

fn classify_outgoing_error_source(
    error_debug: &str,
    candidate_addr: Option<&str>,
    recent_dials: &HashMap<String, RecentDial>,
    peer_present: bool,
    peer_known_mdns: bool,
    peer_inflight_mdns: bool,
    now: std::time::Instant,
) -> OutgoingDialSource {
    if error_debug.contains(NAT_KEEPALIVE_ADDR) {
        return OutgoingDialSource::NatKeepalive;
    }

    if let Some(addr) = candidate_addr {
        if let Some(recent) = recent_dials.get(addr) {
            if now.duration_since(recent.at) <= std::time::Duration::from_secs(30) {
                return recent.source;
            }
        }
    }

    if peer_present && (peer_known_mdns || peer_inflight_mdns) {
        return OutgoingDialSource::Mdns;
    }

    OutgoingDialSource::Unknown
}

pub struct NetworkManager {
    // The P2P Node itself
    swarm: Swarm<RChatBehaviour>,
    // The channel to receive commands FROM the UI
    crx: Receiver<NetworkCommand>,
    // The handle to send events TO the UI
    app_handle: AppHandle,
    disc_rx: Receiver<Multiaddr>,
    // Channel for mDNS-SD discovery
    mdns_rx: Receiver<crate::network::mdns::MdnsPeer>,
    // Sender to pass to mDNS service when starting it
    mdns_tx: tokio::sync::mpsc::Sender<crate::network::mdns::MdnsPeer>,
    // Flag to ensure we only start mDNS once
    mdns_started: bool,
    // Lifecycle handle for mDNS service threads.
    mdns_handle: Option<crate::network::mdns::MdnsServiceHandle>,
    // Track local peers discovered via mDNS
    local_peers: HashMap<PeerId, Vec<Multiaddr>>,
    // Per-peer in-flight mDNS dial timestamps.
    mdns_dial_inflight: HashMap<PeerId, std::time::Instant>,
    // Per-peer next-allowed mDNS dial instant (debounce + backoff).
    mdns_backoff_until: HashMap<PeerId, std::time::Instant>,
    // Per-peer consecutive mDNS dial failures.
    mdns_dial_failures: HashMap<PeerId, u32>,
    // Recent dial origins keyed by multiaddr string for error attribution.
    recent_dials: HashMap<String, RecentDial>,
    // Track our outgoing connection requests (peers we pressed Connect on)
    pending_requests: HashSet<PeerId>,
    // Track incoming connection requests from others
    incoming_requests: HashSet<PeerId>,
    // Pending GitHub mappings: multiaddr → (inviter_username, my_username) for connection events
    pending_github_mappings: HashMap<String, (String, String)>,
    // Pending shadow polls: invitee_username → (password, my_username, created_at)
    // Used to poll invitee's Gist for shadow invite (bidirectional hole punch)
    pending_shadow_polls: HashMap<String, (String, String, u64)>,
    // Active punch targets: target_name → (Multiaddr, start_time)
    // Continuous 500ms punching for 30 seconds
    active_punch_targets: HashMap<String, (Multiaddr, std::time::Instant)>,
    // Joined group IDs we are currently subscribed to
    subscribed_group_ids: HashSet<String>,
    // Fast lookup cache: GitHub username -> PeerId string
    peer_id_by_github: HashMap<String, String>,
    // Reverse lookup cache: PeerId string -> GitHub username
    github_by_peer_id: HashMap<String, String>,
    // Temporary chat routing cache: temp chat id -> peer id
    temp_peer_by_chat_id: HashMap<String, String>,
    // Reverse temporary routing cache: peer id -> temp chat id
    temp_chat_by_peer_id: HashMap<String, String>,
    // Connection transport capability registry per peer.
    peer_transport_registry: PeerTransportRegistry,
    // Transfer per-file ordering/emit state.
    transfer_states: HashMap<String, transfer::TransferState>,
    // Transfer worker queue sender.
    transfer_task_tx: tokio::sync::mpsc::Sender<transfer::TransferTask>,
    // Transfer worker queue result receiver.
    transfer_result_rx: Receiver<transfer::TransferResult>,
    // Graceful shutdown signal for transfer workers.
    transfer_worker_shutdown: Arc<AtomicBool>,
    // Whether transfer queue accepts new tasks.
    transfer_accepting_tasks: Arc<AtomicBool>,
    // Transfer queue counters.
    transfer_pending_tasks: Arc<AtomicUsize>,
    transfer_inflight_tasks: Arc<AtomicUsize>,
    // Worker handles owned by manager for lifecycle control.
    transfer_worker_handles: Vec<tauri::async_runtime::JoinHandle<()>>,
    // Persistence worker queue sender.
    persistence_task_tx: tokio::sync::mpsc::Sender<persistence::PersistenceTask>,
    // Graceful shutdown signal for persistence workers.
    persistence_worker_shutdown: Arc<AtomicBool>,
    // Whether persistence queue accepts new tasks.
    persistence_accepting_tasks: Arc<AtomicBool>,
    // Persistence queue counters.
    persistence_pending_tasks: Arc<AtomicUsize>,
    persistence_inflight_tasks: Arc<AtomicUsize>,
    // Worker handles owned by manager for lifecycle control.
    persistence_worker_handles: Vec<tauri::async_runtime::JoinHandle<()>>,
    // Current DM call runtime state (single-call invariant across voice+video).
    active_call: Option<ActiveCall>,
    // Backend audio engine for current active call.
    voice_audio_engine: Option<crate::live::voice::voice::VoiceAudioEngine>,
    // Captured local PCM16 frames from audio engine.
    voice_capture_rx: Option<tokio::sync::mpsc::UnboundedReceiver<Vec<i16>>>,
    // Sequence number for outgoing voice frames.
    voice_next_seq: u32,
    // Sequence-aware jitter buffer for inbound voice frames.
    voice_jitter_buffer: crate::live::voice::jitter::VoiceJitterBuffer,
}

fn build_incoming_dm_db_message(
    request: &crate::network::direct_message::DirectMessageRequest,
    chat_id: String,
) -> crate::storage::db::Message {
    use crate::network::direct_message::DirectMessageKind;

    let text_content = match request.msg_type {
        DirectMessageKind::Text => request.text_content.clone(),
        DirectMessageKind::Image => None,
        DirectMessageKind::Sticker => None,
        DirectMessageKind::Document => Some(
            request
                .text_content
                .clone()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| "document".to_string()),
        ),
        DirectMessageKind::Video => Some(
            request
                .text_content
                .clone()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| "video".to_string()),
        ),
        DirectMessageKind::Audio => Some(
            request
                .text_content
                .clone()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| "audio".to_string()),
        ),
        _ => request.text_content.clone(),
    };

    let file_hash = match request.msg_type {
        DirectMessageKind::Text => None,
        _ => request.file_hash.clone(),
    };

    crate::storage::db::Message {
        id: request.id.clone(),
        chat_id,
        peer_id: request.sender_id.clone(),
        timestamp: request.timestamp,
        content_type: request.msg_type.as_str().to_string(),
        text_content,
        file_hash,
        status: "delivered".to_string(),
        content_metadata: None,
        sender_alias: request.sender_alias.clone(),
    }
}

fn build_incoming_group_db_message(envelope: &GroupMessageEnvelope) -> crate::storage::db::Message {
    let text_content = match envelope.content_type {
        crate::network::gossip::GroupContentType::Text => envelope.text_content.clone(),
        crate::network::gossip::GroupContentType::Image => None,
        crate::network::gossip::GroupContentType::Sticker => None,
        crate::network::gossip::GroupContentType::Document => Some(
            envelope
                .text_content
                .clone()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| "document".to_string()),
        ),
        crate::network::gossip::GroupContentType::Video => Some(
            envelope
                .text_content
                .clone()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| "video".to_string()),
        ),
        crate::network::gossip::GroupContentType::Audio => Some(
            envelope
                .text_content
                .clone()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| "audio".to_string()),
        ),
    };

    let file_hash = match envelope.content_type {
        crate::network::gossip::GroupContentType::Text => None,
        _ => envelope.file_hash.clone(),
    };

    crate::storage::db::Message {
        id: envelope.id.clone(),
        chat_id: envelope.group_id.clone(),
        peer_id: envelope.sender_id.clone(),
        timestamp: envelope.timestamp,
        content_type: envelope.content_type.as_str().to_string(),
        text_content,
        file_hash,
        status: "delivered".to_string(),
        content_metadata: None,
        sender_alias: envelope.sender_alias.clone(),
    }
}

impl NetworkManager {
    const MDNS_DIAL_DEBOUNCE: std::time::Duration = std::time::Duration::from_secs(2);
    const MDNS_DIAL_INFLIGHT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(12);
    const MDNS_DIAL_MAX_BACKOFF: std::time::Duration = std::time::Duration::from_secs(30);
    const RECENT_DIAL_TTL: std::time::Duration = std::time::Duration::from_secs(30);

    pub fn new(
        swarm: Swarm<RChatBehaviour>,
        crx: Receiver<NetworkCommand>,
        disc_rx: Receiver<Multiaddr>,
        mdns_rx: Receiver<crate::network::mdns::MdnsPeer>,
        mdns_tx: tokio::sync::mpsc::Sender<crate::network::mdns::MdnsPeer>,
        app_handle: AppHandle,
    ) -> Self {
        let (
            transfer_task_tx,
            transfer_result_rx,
            transfer_worker_shutdown,
            transfer_accepting_tasks,
            transfer_pending_tasks,
            transfer_inflight_tasks,
            transfer_worker_handles,
        ) = transfer::start_transfer_workers(app_handle.clone());
        let (
            persistence_task_tx,
            persistence_worker_shutdown,
            persistence_accepting_tasks,
            persistence_pending_tasks,
            persistence_inflight_tasks,
            persistence_worker_handles,
        ) = persistence::start_persistence_workers(app_handle.clone());

        Self {
            swarm,
            crx,
            disc_rx,
            mdns_rx,
            mdns_tx,
            mdns_started: false,
            mdns_handle: None,
            app_handle,
            local_peers: HashMap::new(),
            mdns_dial_inflight: HashMap::new(),
            mdns_backoff_until: HashMap::new(),
            mdns_dial_failures: HashMap::new(),
            recent_dials: HashMap::new(),
            pending_requests: HashSet::new(),
            incoming_requests: HashSet::new(),
            pending_github_mappings: HashMap::new(),
            pending_shadow_polls: HashMap::new(),
            active_punch_targets: HashMap::new(),
            subscribed_group_ids: HashSet::new(),
            peer_id_by_github: HashMap::new(),
            github_by_peer_id: HashMap::new(),
            temp_peer_by_chat_id: HashMap::new(),
            temp_chat_by_peer_id: HashMap::new(),
            peer_transport_registry: PeerTransportRegistry::default(),
            transfer_states: HashMap::new(),
            transfer_task_tx,
            transfer_result_rx,
            transfer_worker_shutdown,
            transfer_accepting_tasks,
            transfer_pending_tasks,
            transfer_inflight_tasks,
            transfer_worker_handles,
            persistence_task_tx,
            persistence_worker_shutdown,
            persistence_accepting_tasks,
            persistence_pending_tasks,
            persistence_inflight_tasks,
            persistence_worker_handles,
            active_call: None,
            voice_audio_engine: None,
            voice_capture_rx: None,
            voice_next_seq: 0,
            voice_jitter_buffer: crate::live::voice::jitter::VoiceJitterBuffer::new(),
        }
    }

    fn prune_stale_mdns_dials(&mut self, now: std::time::Instant) {
        self.mdns_dial_inflight
            .retain(|_, started| now.duration_since(*started) <= Self::MDNS_DIAL_INFLIGHT_TIMEOUT);
        self.mdns_backoff_until.retain(|_, until| *until > now);
        self.recent_dials
            .retain(|_, recent| now.duration_since(recent.at) <= Self::RECENT_DIAL_TTL);
    }

    pub(super) fn record_outgoing_dial(&mut self, addr: &Multiaddr, source: OutgoingDialSource) {
        let now = std::time::Instant::now();
        self.recent_dials.insert(
            addr.to_string(),
            RecentDial {
                source,
                at: now,
            },
        );
        self.prune_stale_mdns_dials(now);
    }

    pub(super) fn classify_outgoing_error(
        &mut self,
        peer_id: Option<PeerId>,
        error_debug: &str,
    ) -> (OutgoingDialSource, Option<String>) {
        let now = std::time::Instant::now();
        self.prune_stale_mdns_dials(now);
        let candidate_addr = extract_candidate_multiaddr_from_error_debug(error_debug);
        let (peer_present, peer_known_mdns, peer_inflight_mdns) = if let Some(peer) = peer_id {
            (
                true,
                self.local_peers.contains_key(&peer),
                self.mdns_dial_inflight.contains_key(&peer),
            )
        } else {
            (false, false, false)
        };
        let source = classify_outgoing_error_source(
            error_debug,
            candidate_addr.as_deref(),
            &self.recent_dials,
            peer_present,
            peer_known_mdns,
            peer_inflight_mdns,
            now,
        );
        (source, candidate_addr)
    }

    pub(super) fn log_mdns_dial_skip(&mut self, peer_id: PeerId) {
        let now = std::time::Instant::now();
        self.prune_stale_mdns_dials(now);

        if self.swarm.is_connected(&peer_id) {
            println!("[mDNS] Dial skipped for {}: already connected", peer_id);
            return;
        }
        if let Some(started) = self.mdns_dial_inflight.get(&peer_id) {
            let elapsed_ms = now.duration_since(*started).as_millis();
            println!(
                "[mDNS] Dial skipped for {}: in-flight ({}ms elapsed)",
                peer_id, elapsed_ms
            );
            return;
        }
        if let Some(until) = self.mdns_backoff_until.get(&peer_id) {
            if *until > now {
                let remaining = until.duration_since(now).as_secs_f32();
                let attempts = self.mdns_dial_failures.get(&peer_id).copied().unwrap_or(0);
                println!(
                    "[mDNS] Dial skipped for {}: backoff active (attempt {}, retry in {:.1}s)",
                    peer_id, attempts, remaining
                );
            }
        }
    }

    pub(super) fn can_start_mdns_dial(&mut self, peer_id: PeerId) -> bool {
        let now = std::time::Instant::now();
        self.prune_stale_mdns_dials(now);

        if self.swarm.is_connected(&peer_id) {
            return false;
        }
        if self.mdns_dial_inflight.contains_key(&peer_id) {
            return false;
        }
        if let Some(until) = self.mdns_backoff_until.get(&peer_id) {
            if *until > now {
                return false;
            }
        }
        true
    }

    pub(super) fn note_mdns_dial_started(&mut self, peer_id: PeerId) {
        let now = std::time::Instant::now();
        self.mdns_dial_inflight.insert(peer_id, now);
        self.mdns_backoff_until
            .insert(peer_id, now + Self::MDNS_DIAL_DEBOUNCE);
    }

    pub(super) fn note_mdns_dial_success(&mut self, peer_id: PeerId) {
        self.mdns_dial_inflight.remove(&peer_id);
        self.mdns_backoff_until.remove(&peer_id);
        self.mdns_dial_failures.remove(&peer_id);
    }

    pub(super) fn note_mdns_dial_failure(&mut self, peer_id: PeerId) {
        let now = std::time::Instant::now();
        self.mdns_dial_inflight.remove(&peer_id);
        let attempts = self.mdns_dial_failures.entry(peer_id).or_insert(0);
        *attempts = attempts.saturating_add(1);
        let pow = std::cmp::min(*attempts, 5);
        let secs = 1u64 << pow;
        let backoff = std::cmp::min(
            std::time::Duration::from_secs(secs),
            Self::MDNS_DIAL_MAX_BACKOFF,
        );
        self.mdns_backoff_until.insert(peer_id, now + backoff);
        println!(
            "[mDNS] Dial failure recorded for {}: attempt {}, next retry in {:.1}s",
            peer_id,
            *attempts,
            backoff.as_secs_f32()
        );
    }

    pub(super) fn cache_peer_mapping(&mut self, github_username: &str, peer_id: &str) {
        self.peer_id_by_github
            .insert(github_username.to_string(), peer_id.to_string());
        self.github_by_peer_id
            .insert(peer_id.to_string(), github_username.to_string());
    }

    pub(super) async fn refresh_peer_mapping_cache(&mut self) {
        let mut next_peer_id_by_github: HashMap<String, String> = HashMap::new();
        let mut next_github_by_peer_id: HashMap<String, String> = HashMap::new();

        let state = self.app_handle.state::<crate::AppState>();
        let mgr = state.config_manager.lock().await;
        if let Ok(config) = mgr.load().await {
            for (gh_user, peer_id) in config.user.github_peer_mapping {
                next_peer_id_by_github.insert(gh_user.clone(), peer_id.clone());
                next_github_by_peer_id.insert(peer_id, gh_user);
            }
        }

        self.peer_id_by_github = next_peer_id_by_github;
        self.github_by_peer_id = next_github_by_peer_id;
    }

    pub(super) async fn resolve_peer_id(
        &mut self,
        target_peer_id: &str,
        context: &str,
    ) -> Option<PeerId> {
        let actual_peer_id_str =
            if let Some(mapped_peer_id) = self.temp_peer_by_chat_id.get(target_peer_id) {
                mapped_peer_id.clone()
            } else if let Some(github_username) = target_peer_id.strip_prefix("gh:") {
                if !self.peer_id_by_github.contains_key(github_username) {
                    self.refresh_peer_mapping_cache().await;
                }

                if let Some(peer_id_string) = self.peer_id_by_github.get(github_username) {
                    println!(
                        "[{}] 🔄 Resolved GitHub user {} to PeerId {}",
                        context, github_username, peer_id_string
                    );
                    peer_id_string.clone()
                } else {
                    eprintln!(
                        "[{}] ❌ No PeerId mapping found for GitHub user {}. Message queued.",
                        context, github_username
                    );
                    return None;
                }
            } else {
                target_peer_id.to_string()
            };

        match actual_peer_id_str.parse::<PeerId>() {
            Ok(p) => Some(p),
            Err(e) => {
                eprintln!(
                    "[{}] ❌ Invalid peer_id: {} ({})",
                    context, actual_peer_id_str, e
                );
                None
            }
        }
    }

    pub(super) async fn resolve_chat_id_for_sender(&mut self, sender_peer_id: &str) -> String {
        if let Some(temp_chat_id) = self.temp_chat_by_peer_id.get(sender_peer_id) {
            return temp_chat_id.clone();
        }

        if let Some(gh_user) = self.github_by_peer_id.get(sender_peer_id) {
            return format!("gh:{}", gh_user);
        }

        self.refresh_peer_mapping_cache().await;
        if let Some(gh_user) = self.github_by_peer_id.get(sender_peer_id) {
            return format!("gh:{}", gh_user);
        }

        sender_peer_id.to_string()
    }

    pub(super) fn cache_temporary_mapping(&mut self, chat_id: &str, peer_id: &str) {
        self.temp_peer_by_chat_id
            .insert(chat_id.to_string(), peer_id.to_string());
        self.temp_chat_by_peer_id
            .insert(peer_id.to_string(), chat_id.to_string());
    }

    pub(super) fn remove_temporary_by_chat_id(&mut self, chat_id: &str) {
        if let Some(peer_id) = self.temp_peer_by_chat_id.remove(chat_id) {
            self.temp_chat_by_peer_id.remove(&peer_id);
        }
    }

    pub(super) fn remove_temporary_by_peer_id(&mut self, peer_id: &str) -> Option<String> {
        let chat_id = self.temp_chat_by_peer_id.remove(peer_id)?;
        self.temp_peer_by_chat_id.remove(&chat_id);
        Some(chat_id)
    }

    pub(super) async fn mark_connected_chat_id(&mut self, chat_id: String) {
        let state = self.app_handle.state::<crate::NetworkState>();
        let mut connected = state.connected_chat_ids.lock().await;
        connected.insert(chat_id);
    }

    pub(super) async fn unmark_connected_chat_id(&mut self, chat_id: &str) {
        let state = self.app_handle.state::<crate::NetworkState>();
        let mut connected = state.connected_chat_ids.lock().await;
        connected.remove(chat_id);
    }

    pub(super) async fn note_chat_connection_established(
        &mut self,
        chat_id: &str,
        remote_addr: &str,
        connected_at: i64,
    ) -> bool {
        let state = self.app_handle.state::<crate::NetworkState>();
        let mut runtime = state.chat_connections.lock().await;
        let entry = runtime.entry(chat_id.to_string()).or_default();
        let was_connected = entry.connected;
        entry.connected = true;
        entry.remote_addr = Some(remote_addr.to_string());
        entry.last_connected_at = Some(connected_at);
        if !was_connected {
            entry.connected_since = Some(connected_at);
        }
        !was_connected
    }

    pub(super) async fn note_chat_connection_closed(&mut self, chat_id: &str) {
        let state = self.app_handle.state::<crate::NetworkState>();
        let mut runtime = state.chat_connections.lock().await;
        let entry = runtime.entry(chat_id.to_string()).or_default();
        entry.connected = false;
        entry.connected_since = None;
    }

    pub(super) async fn set_voice_call_state(
        &mut self,
        mut next: crate::app_state::VoiceCallState,
        reason: Option<String>,
    ) {
        if reason.is_some() {
            next.reason = reason;
        }
        let state = self.app_handle.state::<crate::NetworkState>();
        {
            let mut shared = state.voice_call_state.lock().await;
            *shared = next.clone();
        }
        let _ = self.app_handle.emit("voice-call-state-updated", next);
    }

    pub(super) fn note_peer_transport_connected(&mut self, peer_id: PeerId, remote_addr: &Multiaddr) {
        self.peer_transport_registry
            .record_connected(peer_id, remote_addr);
    }

    pub(super) fn note_peer_transport_disconnected(
        &mut self,
        peer_id: PeerId,
        remote_addr: &Multiaddr,
    ) -> bool {
        self.peer_transport_registry
            .record_disconnected(peer_id, remote_addr)
    }

    pub(super) fn peer_has_quic_path(&self, peer_id: &PeerId) -> bool {
        self.peer_transport_registry.has_quic(peer_id)
    }

    pub(super) fn current_connectivity_settings(&self) -> crate::storage::config::ConnectivitySettings {
        let state = self.app_handle.state::<crate::NetworkState>();
        let settings = match state.connectivity.try_lock() {
            Ok(settings) => settings.clone(),
            Err(_) => crate::storage::config::ConnectivitySettings::default(),
        };
        settings
    }

    pub(super) fn is_mdns_enabled(&self) -> bool {
        self.current_connectivity_settings().mdns_enabled
    }

    pub(super) fn is_github_sync_enabled(&self) -> bool {
        self.current_connectivity_settings().github_sync_enabled
    }

    pub(super) fn is_nat_keepalive_enabled(&self) -> bool {
        self.current_connectivity_settings().nat_keepalive_enabled
    }

    pub(super) fn is_punch_assist_enabled(&self) -> bool {
        self.current_connectivity_settings().punch_assist_enabled
    }
}

impl Drop for NetworkManager {
    fn drop(&mut self) {
        self.shutdown_transfer_workers_gracefully(std::time::Duration::from_secs(5));
        self.shutdown_persistence_workers_gracefully(std::time::Duration::from_secs(5));

        if let Some(mut mdns_handle) = self.mdns_handle.take() {
            mdns_handle.stop();
        }
    }
}
