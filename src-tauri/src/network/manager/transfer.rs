use super::*;
use crate::network::direct_message::{ChunkInfo, DirectMessageKind, DirectMessageRequest};
use base64::Engine;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use tauri::Manager;

const TRANSFER_WORKER_POOL_SIZE: usize = 2;
const TRANSFER_QUEUE_CAPACITY: usize = 512;
const QUEUE_PRESSURE_THRESHOLD: usize = 32;
const TRANSFER_STATE_STALE_TTL: std::time::Duration = std::time::Duration::from_secs(600);

#[derive(Debug, Clone)]
pub(super) struct TransferState {
    pub manifest_persisted: bool,
    pub buffered_chunks: Vec<(String, String)>,
    pub completion_emitted: bool,
    pub updated_at: std::time::Instant,
}

impl Default for TransferState {
    fn default() -> Self {
        Self {
            manifest_persisted: false,
            buffered_chunks: Vec::new(),
            completion_emitted: false,
            updated_at: std::time::Instant::now(),
        }
    }
}

#[derive(Debug)]
pub(super) enum TransferTask {
    BuildFileMetadataResponse {
        peer: PeerId,
        request_id: String,
        file_hash: String,
    },
    BuildChunkResponse {
        peer: PeerId,
        request_id: String,
        file_hash: Option<String>,
        chunk_hash: String,
    },
    PersistChunkManifest {
        file_hash: String,
        chunks: Vec<ChunkInfo>,
    },
    StoreChunkAndCheckComplete {
        file_hash: String,
        chunk_hash: String,
        chunk_b64: String,
    },
    Shutdown,
}

#[derive(Debug)]
pub(super) enum TransferResult {
    SendDirectRequest {
        peer: PeerId,
        request: DirectMessageRequest,
    },
    ManifestPersisted {
        file_hash: String,
    },
    ChunkStored {
        file_hash: String,
        chunk_hash: String,
        chunk_size: usize,
        is_complete: bool,
    },
}

pub(super) fn start_transfer_workers(
    app_handle: AppHandle,
) -> (
    tokio::sync::mpsc::Sender<TransferTask>,
    Receiver<TransferResult>,
    Arc<AtomicBool>,
    Arc<AtomicBool>,
    Arc<AtomicUsize>,
    Arc<AtomicUsize>,
    Vec<tauri::async_runtime::JoinHandle<()>>,
) {
    let (task_tx, task_rx) = tokio::sync::mpsc::channel(TRANSFER_QUEUE_CAPACITY);
    let (result_tx, result_rx) = tokio::sync::mpsc::channel(TRANSFER_QUEUE_CAPACITY);

    let shared_task_rx = Arc::new(tokio::sync::Mutex::new(task_rx));
    let shutdown = Arc::new(AtomicBool::new(false));
    let accepting_tasks = Arc::new(AtomicBool::new(true));
    let pending_tasks = Arc::new(AtomicUsize::new(0));
    let inflight_tasks = Arc::new(AtomicUsize::new(0));
    let mut worker_handles = Vec::with_capacity(TRANSFER_WORKER_POOL_SIZE);

    for worker_id in 0..TRANSFER_WORKER_POOL_SIZE {
        let app_handle = app_handle.clone();
        let task_rx = shared_task_rx.clone();
        let result_tx = result_tx.clone();
        let pending_tasks = pending_tasks.clone();
        let inflight_tasks = inflight_tasks.clone();
        let shutdown = shutdown.clone();

        let handle = tauri::async_runtime::spawn(async move {
            loop {
                let task = {
                    let mut rx = task_rx.lock().await;
                    rx.recv().await
                };

                let Some(task) = task else {
                    break;
                };

                if matches!(task, TransferTask::Shutdown) {
                    break;
                }

                pending_tasks.fetch_sub(1, Ordering::SeqCst);
                inflight_tasks.fetch_add(1, Ordering::SeqCst);

                let app_handle_for_work = app_handle.clone();
                let result = tauri::async_runtime::spawn_blocking(move || {
                    process_transfer_task(&app_handle_for_work, task)
                })
                .await;

                match result {
                    Ok(Ok(Some(result_msg))) => {
                        if result_tx.send(result_msg).await.is_err() {
                            eprintln!("[ChunkTransfer] worker-{} result channel closed", worker_id);
                            break;
                        }
                    }
                    Ok(Ok(None)) => {}
                    Ok(Err(err)) => {
                        eprintln!("[ChunkTransfer] worker-{} task failed: {}", worker_id, err);
                    }
                    Err(join_err) => {
                        eprintln!(
                            "[ChunkTransfer] worker-{} join error: {}",
                            worker_id, join_err
                        );
                    }
                }

                inflight_tasks.fetch_sub(1, Ordering::SeqCst);
            }

            println!("[ChunkTransfer] worker-{} stopped", worker_id);
            shutdown.store(true, Ordering::SeqCst);
        });

        worker_handles.push(handle);
    }

    (
        task_tx,
        result_rx,
        shutdown,
        accepting_tasks,
        pending_tasks,
        inflight_tasks,
        worker_handles,
    )
}

fn process_transfer_task(
    app_handle: &AppHandle,
    task: TransferTask,
) -> Result<Option<TransferResult>, String> {
    match task {
        TransferTask::BuildFileMetadataResponse {
            peer,
            request_id,
            file_hash,
        } => {
            let chunks = with_db_conn(app_handle, |conn| load_chunk_manifest(conn, &file_hash))?;

            println!("[ChunkTransfer] 📋 Returning {} chunks", chunks.len());

            let response_req = DirectMessageRequest {
                id: format!("meta-resp-{}", request_id),
                sender_id: String::new(),
                msg_type: DirectMessageKind::FileMetadataResponse,
                text_content: None,
                file_hash: Some(file_hash),
                timestamp: unix_timestamp_secs(),
                chunk_hash: None,
                chunk_data: None,
                chunk_list: Some(chunks),
                sender_alias: None,
            };

            Ok(Some(TransferResult::SendDirectRequest {
                peer,
                request: response_req,
            }))
        }
        TransferTask::BuildChunkResponse {
            peer,
            request_id,
            file_hash,
            chunk_hash,
        } => {
            let chunk_path = chunks_dir().join(&chunk_hash);
            let chunk_data = match std::fs::read(&chunk_path) {
                Ok(data) => data,
                Err(err) => {
                    eprintln!(
                        "[ChunkTransfer] ❌ Chunk not found {} at {:?}: {}",
                        chunk_hash, chunk_path, err
                    );
                    return Ok(None);
                }
            };

            let chunk_b64 = base64::engine::general_purpose::STANDARD.encode(&chunk_data);

            println!(
                "[ChunkTransfer] 📦 Prepared chunk {} ({} bytes)",
                chunk_hash,
                chunk_data.len()
            );

            let response_req = DirectMessageRequest {
                id: format!("chunk-resp-{}", request_id),
                sender_id: String::new(),
                msg_type: DirectMessageKind::ChunkResponse,
                text_content: None,
                file_hash,
                timestamp: unix_timestamp_secs(),
                chunk_hash: Some(chunk_hash),
                chunk_data: Some(chunk_b64),
                chunk_list: None,
                sender_alias: None,
            };

            Ok(Some(TransferResult::SendDirectRequest {
                peer,
                request: response_req,
            }))
        }
        TransferTask::PersistChunkManifest { file_hash, chunks } => {
            with_db_conn(app_handle, |conn| {
                persist_chunk_manifest(conn, &file_hash, &chunks)
            })?;
            Ok(Some(TransferResult::ManifestPersisted { file_hash }))
        }
        TransferTask::StoreChunkAndCheckComplete {
            file_hash,
            chunk_hash,
            chunk_b64,
        } => {
            let chunk_data = base64::engine::general_purpose::STANDARD
                .decode(chunk_b64)
                .map_err(|e| format!("Failed to decode chunk data: {}", e))?;

            let chunk_size = store_chunk_file(&chunks_dir(), &chunk_hash, &chunk_data)?;
            let is_complete = with_db_conn(app_handle, |conn| {
                evaluate_file_completion(conn, &chunks_dir(), &file_hash)
            })?;

            Ok(Some(TransferResult::ChunkStored {
                file_hash,
                chunk_hash,
                chunk_size,
                is_complete,
            }))
        }
        TransferTask::Shutdown => Ok(None),
    }
}

fn with_db_conn<T>(
    app_handle: &AppHandle,
    op: impl FnOnce(&rusqlite::Connection) -> Result<T, String>,
) -> Result<T, String> {
    let state = app_handle.state::<crate::AppState>();
    let conn = state
        .db_conn
        .lock()
        .map_err(|e| format!("db lock poisoned: {}", e))?;
    op(&conn)
}

fn chunks_dir() -> PathBuf {
    directories::ProjectDirs::from("io.github", "ata-sesli", "RChat")
        .map(|p| p.data_dir().join("chunks"))
        .unwrap_or_else(|| PathBuf::from("chunks"))
}

fn unix_timestamp_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn load_chunk_manifest(
    conn: &rusqlite::Connection,
    file_hash: &str,
) -> Result<Vec<ChunkInfo>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT chunk_hash, chunk_order, chunk_size FROM file_chunks WHERE file_hash = ?1 ORDER BY chunk_order",
        )
        .map_err(|e| format!("prepare manifest query failed: {}", e))?;

    let rows = stmt
        .query_map([file_hash], |row| {
            Ok(ChunkInfo {
                chunk_hash: row.get(0)?,
                chunk_order: row.get(1)?,
                chunk_size: row.get(2)?,
            })
        })
        .map_err(|e| format!("manifest query failed: {}", e))?;

    let mut chunks = Vec::new();
    for row in rows {
        chunks.push(row.map_err(|e| format!("manifest row decode failed: {}", e))?);
    }
    Ok(chunks)
}

fn persist_chunk_manifest(
    conn: &rusqlite::Connection,
    file_hash: &str,
    chunks: &[ChunkInfo],
) -> Result<(), String> {
    for chunk_info in chunks {
        conn.execute(
            "INSERT OR IGNORE INTO file_chunks (file_hash, chunk_order, chunk_hash, chunk_size) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                file_hash,
                chunk_info.chunk_order,
                chunk_info.chunk_hash,
                chunk_info.chunk_size
            ],
        )
        .map_err(|e| format!("persist chunk manifest failed: {}", e))?;
    }
    Ok(())
}

fn store_chunk_file(
    chunks_dir: &Path,
    chunk_hash: &str,
    chunk_data: &[u8],
) -> Result<usize, String> {
    std::fs::create_dir_all(chunks_dir)
        .map_err(|e| format!("failed to create chunk dir {:?}: {}", chunks_dir, e))?;

    let chunk_path = chunks_dir.join(chunk_hash);
    std::fs::write(&chunk_path, chunk_data)
        .map_err(|e| format!("failed to write chunk {}: {}", chunk_hash, e))?;

    Ok(chunk_data.len())
}

fn evaluate_file_completion(
    conn: &rusqlite::Connection,
    chunks_dir: &Path,
    file_hash: &str,
) -> Result<bool, String> {
    let expected: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM file_chunks WHERE file_hash = ?1",
            [file_hash],
            |row| row.get(0),
        )
        .map_err(|e| format!("expected chunk count query failed: {}", e))?;

    let mut received = 0i64;
    let mut stmt = conn
        .prepare("SELECT chunk_hash FROM file_chunks WHERE file_hash = ?1")
        .map_err(|e| format!("prepare received chunk query failed: {}", e))?;
    let rows = stmt
        .query_map([file_hash], |row| row.get::<_, String>(0))
        .map_err(|e| format!("received chunk query failed: {}", e))?;

    for hash_result in rows {
        let hash = hash_result.map_err(|e| format!("received chunk decode failed: {}", e))?;
        if chunks_dir.join(hash).exists() {
            received += 1;
        }
    }

    println!("[ChunkTransfer] Progress: {}/{} chunks", received, expected);

    if received == expected && expected > 0 {
        conn.execute(
            "UPDATE files SET is_complete = 1 WHERE file_hash = ?1",
            [file_hash],
        )
        .map_err(|e| format!("file completion update failed: {}", e))?;
        println!("[ChunkTransfer] ✅ File {} complete!", file_hash);
        Ok(true)
    } else {
        Ok(false)
    }
}

impl NetworkManager {
    fn touch_transfer_state(&mut self, file_hash: &str) -> &mut TransferState {
        let state = self
            .transfer_states
            .entry(file_hash.to_string())
            .or_default();
        state.updated_at = std::time::Instant::now();
        state
    }

    pub(super) fn cleanup_stale_transfer_states(&mut self) {
        let now = std::time::Instant::now();
        self.transfer_states
            .retain(|_, state| now.duration_since(state.updated_at) < TRANSFER_STATE_STALE_TTL);
    }

    async fn enqueue_transfer_task(
        &mut self,
        task: TransferTask,
        context: &str,
    ) -> Result<(), String> {
        if !self.transfer_accepting_tasks.load(Ordering::SeqCst) {
            return Err(format!("Transfer queue stopped in {}", context));
        }

        let remaining = self.transfer_task_tx.capacity();
        if remaining <= QUEUE_PRESSURE_THRESHOLD {
            println!(
                "[ChunkTransfer] ⚠️ Queue pressure in {}: {} slots remaining",
                context, remaining
            );
        }

        self.transfer_pending_tasks.fetch_add(1, Ordering::SeqCst);
        if let Err(e) = self.transfer_task_tx.send(task).await {
            self.transfer_pending_tasks.fetch_sub(1, Ordering::SeqCst);
            return Err(format!("Failed to enqueue task in {}: {}", context, e));
        }

        Ok(())
    }

    pub(super) async fn handle_transfer_result(&mut self, result: TransferResult) {
        match result {
            TransferResult::SendDirectRequest { peer, mut request } => {
                request.sender_id = self.swarm.local_peer_id().to_string();
                self.swarm
                    .behaviour_mut()
                    .direct_message
                    .send_request(&peer, request);
            }
            TransferResult::ManifestPersisted { file_hash } => {
                let buffered = {
                    let state = self.touch_transfer_state(&file_hash);
                    state.manifest_persisted = true;
                    std::mem::take(&mut state.buffered_chunks)
                };

                for (chunk_hash, chunk_b64) in buffered {
                    let _ = self
                        .enqueue_transfer_task(
                            TransferTask::StoreChunkAndCheckComplete {
                                file_hash: file_hash.clone(),
                                chunk_hash,
                                chunk_b64,
                            },
                            "flush_buffered_chunk",
                        )
                        .await;
                }
            }
            TransferResult::ChunkStored {
                file_hash,
                chunk_hash,
                chunk_size,
                is_complete,
            } => {
                println!(
                    "[ChunkTransfer] 💾 Stored chunk {} ({} bytes)",
                    chunk_hash, chunk_size
                );
                let state = self.touch_transfer_state(&file_hash);
                if is_complete && !state.completion_emitted {
                    state.completion_emitted = true;
                    let _ = self.app_handle.emit(
                        "file-transfer-complete",
                        serde_json::json!({ "file_hash": file_hash }),
                    );
                    self.transfer_states.remove(&file_hash);
                }
            }
        }
    }

    pub(super) async fn handle_file_metadata_request(
        &mut self,
        peer: PeerId,
        request: &DirectMessageRequest,
    ) {
        if let Some(ref file_hash) = request.file_hash {
            println!("[ChunkTransfer] 📋 Metadata request for: {}", file_hash);
            if let Err(e) = self
                .enqueue_transfer_task(
                    TransferTask::BuildFileMetadataResponse {
                        peer,
                        request_id: request.id.clone(),
                        file_hash: file_hash.clone(),
                    },
                    "file_metadata_request",
                )
                .await
            {
                eprintln!("[ChunkTransfer] ❌ {}", e);
            }
        }
    }

    pub(super) async fn handle_chunk_request(
        &mut self,
        peer: PeerId,
        request: &DirectMessageRequest,
    ) {
        if let Some(ref chunk_hash) = request.chunk_hash {
            println!("[ChunkTransfer] 📦 Chunk request for: {}", chunk_hash);
            if let Err(e) = self
                .enqueue_transfer_task(
                    TransferTask::BuildChunkResponse {
                        peer,
                        request_id: request.id.clone(),
                        file_hash: request.file_hash.clone(),
                        chunk_hash: chunk_hash.clone(),
                    },
                    "chunk_request",
                )
                .await
            {
                eprintln!("[ChunkTransfer] ❌ {}", e);
            }
        }
    }

    pub(super) async fn handle_file_metadata_response(
        &mut self,
        peer: PeerId,
        request: &DirectMessageRequest,
    ) {
        if let (Some(ref file_hash), Some(ref chunks)) = (&request.file_hash, &request.chunk_list) {
            println!(
                "[ChunkTransfer] 📋 Received {} chunks for {}",
                chunks.len(),
                file_hash
            );

            {
                let state = self.touch_transfer_state(file_hash);
                state.manifest_persisted = false;
                state.completion_emitted = false;
            }

            if let Err(e) = self
                .enqueue_transfer_task(
                    TransferTask::PersistChunkManifest {
                        file_hash: file_hash.clone(),
                        chunks: chunks.clone(),
                    },
                    "file_metadata_response",
                )
                .await
            {
                eprintln!("[ChunkTransfer] ❌ {}", e);
                return;
            }

            for chunk_info in chunks {
                let chunk_req = DirectMessageRequest {
                    id: format!("chunk-req-{}-{}", file_hash, chunk_info.chunk_order),
                    sender_id: self.swarm.local_peer_id().to_string(),
                    msg_type: DirectMessageKind::ChunkRequest,
                    text_content: None,
                    file_hash: Some(file_hash.clone()),
                    timestamp: unix_timestamp_secs(),
                    chunk_hash: Some(chunk_info.chunk_hash.clone()),
                    chunk_data: None,
                    chunk_list: None,
                    sender_alias: None,
                };

                self.swarm
                    .behaviour_mut()
                    .direct_message
                    .send_request(&peer, chunk_req);

                println!(
                    "[ChunkTransfer] 📤 Requested chunk {}/{}",
                    chunk_info.chunk_order + 1,
                    chunks.len()
                );
            }
        }
    }

    pub(super) async fn handle_chunk_response(&mut self, request: &DirectMessageRequest) {
        if let (Some(ref file_hash), Some(ref chunk_hash), Some(ref chunk_b64)) =
            (&request.file_hash, &request.chunk_hash, &request.chunk_data)
        {
            let state = self.touch_transfer_state(file_hash);
            if !state.manifest_persisted {
                state
                    .buffered_chunks
                    .push((chunk_hash.clone(), chunk_b64.clone()));
                return;
            }

            if let Err(e) = self
                .enqueue_transfer_task(
                    TransferTask::StoreChunkAndCheckComplete {
                        file_hash: file_hash.clone(),
                        chunk_hash: chunk_hash.clone(),
                        chunk_b64: chunk_b64.clone(),
                    },
                    "chunk_response",
                )
                .await
            {
                eprintln!("[ChunkTransfer] ❌ {}", e);
            }
        }
    }

    pub(super) fn shutdown_transfer_workers_gracefully(&mut self, timeout: std::time::Duration) {
        self.transfer_accepting_tasks.store(false, Ordering::SeqCst);

        let worker_count = self.transfer_worker_handles.len();
        let deadline = std::time::Instant::now() + timeout;

        for _ in 0..worker_count {
            loop {
                match self.transfer_task_tx.try_send(TransferTask::Shutdown) {
                    Ok(_) => break,
                    Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                        while self.transfer_result_rx.try_recv().is_ok() {}
                        if std::time::Instant::now() >= deadline {
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => break,
                }
            }
        }

        while std::time::Instant::now() < deadline {
            while self.transfer_result_rx.try_recv().is_ok() {}
            if self.transfer_pending_tasks.load(Ordering::SeqCst) == 0
                && self.transfer_inflight_tasks.load(Ordering::SeqCst) == 0
                && self
                    .transfer_worker_handles
                    .iter()
                    .all(|h| h.inner().is_finished())
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        for handle in self.transfer_worker_handles.drain(..) {
            if !handle.inner().is_finished() {
                handle.abort();
            }
        }

        while self.transfer_result_rx.try_recv().is_ok() {}
        self.transfer_worker_shutdown.store(true, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_transfer_tables(conn: &rusqlite::Connection) {
        conn.execute(
            "CREATE TABLE IF NOT EXISTS files (
                file_hash TEXT PRIMARY KEY,
                file_name TEXT,
                mime_type TEXT,
                size_bytes INTEGER,
                is_complete INTEGER DEFAULT 0
            )",
            [],
        )
        .expect("create files");

        conn.execute(
            "CREATE TABLE IF NOT EXISTS file_chunks (
                file_hash TEXT,
                chunk_order INTEGER,
                chunk_hash TEXT,
                chunk_size INTEGER,
                PRIMARY KEY(file_hash, chunk_order)
            )",
            [],
        )
        .expect("create file_chunks");
    }

    #[test]
    fn persist_and_load_chunk_manifest_roundtrip() {
        let conn = rusqlite::Connection::open_in_memory().expect("open memory db");
        setup_transfer_tables(&conn);

        let chunks = vec![
            ChunkInfo {
                chunk_hash: "c1".to_string(),
                chunk_order: 0,
                chunk_size: 10,
            },
            ChunkInfo {
                chunk_hash: "c2".to_string(),
                chunk_order: 1,
                chunk_size: 12,
            },
        ];

        persist_chunk_manifest(&conn, "file-a", &chunks).expect("persist manifest");
        let loaded = load_chunk_manifest(&conn, "file-a").expect("load manifest");

        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].chunk_hash, "c1");
        assert_eq!(loaded[1].chunk_hash, "c2");
    }

    #[test]
    fn evaluate_completion_requires_all_chunks() {
        let conn = rusqlite::Connection::open_in_memory().expect("open memory db");
        setup_transfer_tables(&conn);

        conn.execute(
            "INSERT INTO files (file_hash, file_name, mime_type, size_bytes, is_complete) VALUES (?1, ?2, ?3, ?4, 0)",
            rusqlite::params!["file-b", "f", "application/octet-stream", 22_i64],
        )
        .expect("insert file");

        let chunks = vec![
            ChunkInfo {
                chunk_hash: "ca".to_string(),
                chunk_order: 0,
                chunk_size: 10,
            },
            ChunkInfo {
                chunk_hash: "cb".to_string(),
                chunk_order: 1,
                chunk_size: 12,
            },
        ];

        persist_chunk_manifest(&conn, "file-b", &chunks).expect("persist manifest");

        let temp = tempfile::tempdir().expect("tempdir");
        let chunks_path = temp.path().join("chunks");

        store_chunk_file(&chunks_path, "ca", b"1234567890").expect("write chunk a");
        let complete =
            evaluate_file_completion(&conn, &chunks_path, "file-b").expect("completion check a");
        assert!(!complete);

        store_chunk_file(&chunks_path, "cb", b"123456789012").expect("write chunk b");
        let complete =
            evaluate_file_completion(&conn, &chunks_path, "file-b").expect("completion check b");
        assert!(complete);

        let is_complete: i64 = conn
            .query_row(
                "SELECT is_complete FROM files WHERE file_hash = ?1",
                ["file-b"],
                |row| row.get(0),
            )
            .expect("query file completion");
        assert_eq!(is_complete, 1);
    }

    #[test]
    fn transfer_state_buffers_until_manifest() {
        let mut state = TransferState::default();
        assert!(!state.manifest_persisted);
        state.buffered_chunks.push(("h1".into(), "d1".into()));
        state.buffered_chunks.push(("h2".into(), "d2".into()));

        state.manifest_persisted = true;
        let flushed = std::mem::take(&mut state.buffered_chunks);

        assert_eq!(flushed.len(), 2);
        assert_eq!(flushed[0].0, "h1");
        assert_eq!(flushed[1].0, "h2");
    }
}
