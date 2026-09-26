//! Shared admission and cancellation for all inbound live-media protocols.

use libp2p::PeerId;
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::watch;

pub(crate) const GLOBAL_LIMIT: usize = 12;
pub(crate) const PER_PEER_LIMIT: usize = 4;
const HEADER_DEADLINE: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum MediaKind {
    Voice,
    Video,
    Broadcast,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Session {
    peer: PeerId,
    id: String,
    generation: u64,
}

#[derive(Debug, Default)]
struct State {
    sessions: HashMap<MediaKind, Session>,
    total: usize,
    peers: HashMap<PeerId, usize>,
    active: HashSet<(MediaKind, u64)>,
    generation: u64,
    closed: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct MediaAdmission {
    inner: Arc<Mutex<State>>,
    changed: watch::Sender<u64>,
}

impl Default for MediaAdmission {
    fn default() -> Self {
        Self {
            inner: Default::default(),
            changed: watch::channel(0).0,
        }
    }
}

impl MediaAdmission {
    pub fn set(&self, kind: MediaKind, session: Option<(PeerId, String)>) {
        let mut state = self.inner.lock().unwrap();
        if state.closed || state.sessions.get(&kind).map(|s| (s.peer, s.id.clone())) == session {
            return;
        }
        state.generation = state.generation.wrapping_add(1);
        let generation = state.generation;
        if let Some((peer, id)) = session {
            state.sessions.insert(
                kind,
                Session {
                    peer,
                    id,
                    generation,
                },
            );
        } else {
            state.sessions.remove(&kind);
        }
        drop(state);
        self.changed.send_modify(|v| *v = v.wrapping_add(1));
    }

    pub fn shutdown(&self) {
        let mut state = self.inner.lock().unwrap();
        state.closed = true;
        state.sessions.clear();
        drop(state);
        self.changed.send_modify(|v| *v = v.wrapping_add(1));
    }

    pub fn admit(&self, kind: MediaKind, peer: PeerId) -> Option<ReaderPermit> {
        let mut state = self.inner.lock().unwrap();
        let session = state.sessions.get(&kind)?.clone();
        if state.closed
            || session.peer != peer
            || state.total >= GLOBAL_LIMIT
            || state.peers.get(&peer).copied().unwrap_or(0) >= PER_PEER_LIMIT
        {
            return None;
        }
        state.total += 1;
        *state.peers.entry(peer).or_default() += 1;
        Some(ReaderPermit {
            pool: self.clone(),
            kind,
            session,
            active: false,
            admitted_at: tokio::time::Instant::now(),
        })
    }
}

#[derive(Debug)]
pub struct ReaderPermit {
    pool: MediaAdmission,
    kind: MediaKind,
    session: Session,
    active: bool,
    admitted_at: tokio::time::Instant,
}

impl ReaderPermit {
    fn current(&self) -> bool {
        let state = self.pool.inner.lock().unwrap();
        !state.closed && state.sessions.get(&self.kind) == Some(&self.session)
    }

    pub fn authorize(&mut self, id: &str) -> bool {
        let mut state = self.pool.inner.lock().unwrap();
        if state.closed
            || self.session.id != id
            || state.sessions.get(&self.kind) != Some(&self.session)
            || !state.active.insert((self.kind, self.session.generation))
        {
            return false;
        }
        self.active = true;
        true
    }

    pub async fn cancelled(&self) {
        let mut changed = self.pool.changed.subscribe();
        while self.current() {
            if changed.changed().await.is_err() {
                break;
            }
        }
    }

    pub async fn header<T>(&self, read: impl Future<Output = std::io::Result<T>>) -> Option<T> {
        let deadline = self.admitted_at + HEADER_DEADLINE;
        // timeout_at polls a ready future before its timer. Do not grant an
        // already-expired queued stream a fresh chance merely because it buffered data.
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::select! {
            biased;
            _ = self.cancelled() => None,
            result = tokio::time::timeout_at(deadline, read) => result.ok().and_then(Result::ok),
        }
    }

    // No inactivity timeout after authorization: muted calls/cameras may be
    // silent indefinitely. Session revocation cancels reads AND event sends.
    pub async fn run<T>(&self, reader: impl Future<Output = T>) -> Option<T> {
        tokio::select! { biased; _ = self.cancelled() => None, result = reader => Some(result) }
    }
}

impl Drop for ReaderPermit {
    fn drop(&mut self) {
        let mut state = self.pool.inner.lock().unwrap();
        state.total -= 1;
        if let Some(count) = state.peers.get_mut(&self.session.peer) {
            *count -= 1;
            if *count == 0 {
                state.peers.remove(&self.session.peer);
            }
        }
        if self.active {
            state.active.remove(&(self.kind, self.session.generation));
        }
    }
}

pub(crate) fn spawn_readers<F, Fut>(
    mut incoming: super::voice_stream::IncomingStreams,
    reader: F,
) -> tokio::task::JoinHandle<()>
where
    F: Fn(PeerId, libp2p::swarm::Stream, ReaderPermit) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        let mut readers = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                biased;
                _ = readers.join_next(), if !readers.is_empty() => {},
                incoming = incoming.recv() => match incoming {
                    Some((peer, stream, permit)) => { readers.spawn(reader(peer, stream, permit)); }
                    None => break,
                }
            }
        }
        // JoinSet drop aborts every child; no detached reader tasks.
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shared_budget_covers_all_media_and_releases_on_drop() {
        let pool = MediaAdmission::default();
        let mut permits = Vec::new();
        for kind in [MediaKind::Voice, MediaKind::Video, MediaKind::Broadcast] {
            let peer = PeerId::random();
            pool.set(kind, Some((peer, "session".into())));
            for _ in 0..PER_PEER_LIMIT {
                permits.push(pool.admit(kind, peer).unwrap());
            }
        }
        assert_eq!(permits.len(), GLOBAL_LIMIT);
        let replacement_peer = PeerId::random();
        pool.set(
            MediaKind::Voice,
            Some((replacement_peer, "replacement".into())),
        );
        assert!(pool.admit(MediaKind::Voice, replacement_peer).is_none());
        permits.pop();
        assert!(pool.admit(MediaKind::Voice, replacement_peer).is_some());
        drop(permits);
        assert_eq!(pool.inner.lock().unwrap().total, 0);
    }

    #[tokio::test]
    async fn header_deadline_includes_time_waiting_for_the_reader() {
        let pool = MediaAdmission::default();
        let peer = PeerId::random();
        pool.set(MediaKind::Voice, Some((peer, "call".into())));
        let mut permit = pool.admit(MediaKind::Voice, peer).unwrap();
        permit.admitted_at -= HEADER_DEADLINE;
        assert!(permit
            .header(async { Ok("already buffered") })
            .await
            .is_none());
        assert!(permit
            .header(std::future::pending::<std::io::Result<String>>())
            .await
            .is_none());
    }

    #[tokio::test]
    async fn muted_reader_stays_live_until_session_replaced_even_with_same_id() {
        let pool = MediaAdmission::default();
        let peer = PeerId::random();
        pool.set(MediaKind::Voice, Some((peer, "call".into())));
        let mut permit = pool.admit(MediaKind::Voice, peer).unwrap();
        assert!(permit.authorize("call"));
        let reader = permit.run(std::future::pending::<()>());
        tokio::pin!(reader);
        assert!(tokio::time::timeout(Duration::from_millis(10), &mut reader)
            .await
            .is_err());
        pool.set(MediaKind::Voice, None);
        pool.set(MediaKind::Voice, Some((peer, "call".into())));
        assert!(tokio::time::timeout(Duration::from_secs(1), &mut reader)
            .await
            .unwrap()
            .is_none());
        let mut replacement = pool.admit(MediaKind::Voice, peer).unwrap();
        assert!(replacement.authorize("call"));
    }

    #[tokio::test]
    async fn shutdown_cancels_blocked_event_delivery() {
        let pool = MediaAdmission::default();
        let peer = PeerId::random();
        pool.set(MediaKind::Voice, Some((peer, "call".into())));
        let mut permit = pool.admit(MediaKind::Voice, peer).unwrap();
        assert!(permit.authorize("call"));
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        tx.send(()).await.unwrap();
        let reader = permit.run(tx.send(()));
        tokio::pin!(reader);
        assert!(tokio::time::timeout(Duration::from_millis(10), &mut reader)
            .await
            .is_err());
        pool.shutdown();
        assert!(reader.await.is_none());
    }

    #[tokio::test]
    async fn admission_bounds_pending_and_active_readers_and_revokes_sessions() {
        let pool = MediaAdmission::default();
        let peer = libp2p::PeerId::random();
        assert!(pool.admit(MediaKind::Voice, peer).is_none());
        pool.set(MediaKind::Voice, Some((peer, "call".into())));
        let mut permits: Vec<_> = (0..PER_PEER_LIMIT)
            .map(|_| pool.admit(MediaKind::Voice, peer).unwrap())
            .collect();
        assert!(pool.admit(MediaKind::Voice, peer).is_none());
        assert!(!permits[0].authorize("invalid"));
        assert!(permits[0].authorize("call"));
        assert!(!permits[1].authorize("call"));
        pool.set(MediaKind::Voice, None);
        permits[0].cancelled().await;
        assert!(!permits[1].authorize("call"));
        drop(permits);
        assert_eq!(pool.inner.lock().unwrap().total, 0);
        pool.set(MediaKind::Voice, Some((peer, "call".into())));
        let permit = pool.admit(MediaKind::Voice, peer).unwrap();
        pool.shutdown();
        permit.cancelled().await;
        assert!(pool.admit(MediaKind::Voice, peer).is_none());
    }
}
