use super::*;
use crate::network::direct_message::DirectMessageRequest;
use crate::network::gossip::GroupMessageEnvelope;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

const PERSISTENCE_WORKER_POOL_SIZE: usize = 2;
const PERSISTENCE_QUEUE_CAPACITY: usize = 512;
const QUEUE_PRESSURE_THRESHOLD: usize = 32;

pub(super) enum PersistenceTask {
    PersistIncomingDirectMessage {
        request: DirectMessageRequest,
        chat_id: String,
        db_msg: crate::storage::db::Message,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    PersistIncomingGroupMessage {
        envelope: GroupMessageEnvelope,
        db_msg: crate::storage::db::Message,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    UpdateDeliveredStatus {
        msg_id: String,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    UpdateReadStatuses {
        msg_ids: Vec<String>,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    Shutdown,
}

pub(super) fn start_persistence_workers(
    app_handle: AppHandle,
) -> (
    tokio::sync::mpsc::Sender<PersistenceTask>,
    Arc<AtomicBool>,
    Arc<AtomicBool>,
    Arc<AtomicUsize>,
    Arc<AtomicUsize>,
    Vec<tauri::async_runtime::JoinHandle<()>>,
) {
    let (task_tx, task_rx) = tokio::sync::mpsc::channel(PERSISTENCE_QUEUE_CAPACITY);

    let shared_task_rx = Arc::new(tokio::sync::Mutex::new(task_rx));
    let shutdown = Arc::new(AtomicBool::new(false));
    let accepting_tasks = Arc::new(AtomicBool::new(true));
    let pending_tasks = Arc::new(AtomicUsize::new(0));
    let inflight_tasks = Arc::new(AtomicUsize::new(0));
    let mut worker_handles = Vec::with_capacity(PERSISTENCE_WORKER_POOL_SIZE);

    for worker_id in 0..PERSISTENCE_WORKER_POOL_SIZE {
        let app_handle = app_handle.clone();
        let task_rx = shared_task_rx.clone();
        let shutdown = shutdown.clone();
        let pending_tasks = pending_tasks.clone();
        let inflight_tasks = inflight_tasks.clone();

        let handle = tauri::async_runtime::spawn(async move {
            loop {
                let task = {
                    let mut rx = task_rx.lock().await;
                    rx.recv().await
                };

                let Some(task) = task else {
                    break;
                };

                if matches!(task, PersistenceTask::Shutdown) {
                    break;
                }

                pending_tasks.fetch_sub(1, Ordering::SeqCst);
                inflight_tasks.fetch_add(1, Ordering::SeqCst);

                match task {
                    PersistenceTask::PersistIncomingDirectMessage {
                        request,
                        chat_id,
                        db_msg,
                        reply,
                    } => {
                        let app_handle_for_work = app_handle.clone();
                        let result = tauri::async_runtime::spawn_blocking(move || {
                            persist_incoming_direct_message(
                                &app_handle_for_work,
                                &request,
                                &chat_id,
                                &db_msg,
                            )
                        })
                        .await
                        .map_err(|e| e.to_string())
                        .and_then(|r| r);
                        let _ = reply.send(result);
                    }
                    PersistenceTask::PersistIncomingGroupMessage {
                        envelope,
                        db_msg,
                        reply,
                    } => {
                        let app_handle_for_work = app_handle.clone();
                        let result = tauri::async_runtime::spawn_blocking(move || {
                            persist_incoming_group_message(&app_handle_for_work, &envelope, &db_msg)
                        })
                        .await
                        .map_err(|e| e.to_string())
                        .and_then(|r| r);
                        let _ = reply.send(result);
                    }
                    PersistenceTask::UpdateDeliveredStatus { msg_id, reply } => {
                        let app_handle_for_work = app_handle.clone();
                        let result = tauri::async_runtime::spawn_blocking(move || {
                            with_db_conn(&app_handle_for_work, |conn| {
                                crate::storage::db::update_message_status(
                                    &conn,
                                    &msg_id,
                                    "delivered",
                                )
                                .map_err(|e| e.to_string())
                            })
                        })
                        .await
                        .map_err(|e| e.to_string())
                        .and_then(|r| r);
                        let _ = reply.send(result);
                    }
                    PersistenceTask::UpdateReadStatuses { msg_ids, reply } => {
                        let app_handle_for_work = app_handle.clone();
                        let result = tauri::async_runtime::spawn_blocking(move || {
                            with_db_conn(&app_handle_for_work, |conn| {
                                for msg_id in msg_ids {
                                    crate::storage::db::update_message_status(
                                        &conn, &msg_id, "read",
                                    )
                                    .map_err(|e| e.to_string())?;
                                }
                                Ok(())
                            })
                        })
                        .await
                        .map_err(|e| e.to_string())
                        .and_then(|r| r);
                        let _ = reply.send(result);
                    }
                    PersistenceTask::Shutdown => unreachable!(),
                }

                inflight_tasks.fetch_sub(1, Ordering::SeqCst);
            }

            println!("[Persistence] worker-{} stopped", worker_id);
            shutdown.store(true, Ordering::SeqCst);
        });

        worker_handles.push(handle);
    }

    (
        task_tx,
        shutdown,
        accepting_tasks,
        pending_tasks,
        inflight_tasks,
        worker_handles,
    )
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

fn persist_incoming_direct_message(
    app_handle: &AppHandle,
    request: &DirectMessageRequest,
    chat_id: &str,
    db_msg: &crate::storage::db::Message,
) -> Result<(), String> {
    with_db_conn(app_handle, |conn| {
        let sender_name = request
            .sender_alias
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| {
                crate::chat_identity::extract_name_from_chat_id(chat_id)
                    .filter(|name| !name.trim().is_empty())
            })
            .unwrap_or_else(|| "peer".to_string());

        let peer_exists = crate::storage::db::is_peer(conn, &request.sender_id);
        if !peer_exists {
            crate::storage::db::add_peer(
                conn,
                &request.sender_id,
                Some(&sender_name),
                None,
                "direct",
            )
                .map_err(|e| e.to_string())?;
        }

        let chat_exists = crate::storage::db::chat_exists(conn, chat_id);
        if !chat_exists {
            crate::storage::db::create_chat(conn, chat_id, &sender_name, false)
                .map_err(|e| e.to_string())?;
        } else if let Ok(existing_name) = crate::storage::db::get_chat_name(conn, chat_id) {
            if existing_name
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty() && *name != chat_id)
                .is_none()
            {
                let _ = crate::storage::db::upsert_chat(conn, chat_id, &sender_name, false);
            }
        }

        if request.msg_type.needs_file_transfer() {
            if let Some(ref file_hash) = request.file_hash {
                let file_exists: bool = conn
                    .query_row(
                        "SELECT 1 FROM files WHERE file_hash = ?1",
                        [file_hash],
                        |_| Ok(true),
                    )
                    .unwrap_or(false);

                if !file_exists {
                    conn.execute(
                        "INSERT INTO files (file_hash, file_name, mime_type, size_bytes, is_complete) VALUES (?1, NULL, 'application/octet-stream', 0, 0)",
                        [file_hash],
                    )
                    .map_err(|e| e.to_string())?;
                }
            }
        }

        crate::storage::db::insert_message(conn, db_msg).map_err(|e| e.to_string())
    })
}

fn persist_incoming_group_message(
    app_handle: &AppHandle,
    envelope: &GroupMessageEnvelope,
    db_msg: &crate::storage::db::Message,
) -> Result<(), String> {
    with_db_conn(app_handle, |conn| {
        if !crate::storage::db::is_peer(conn, &envelope.sender_id) {
            crate::storage::db::add_peer(conn, &envelope.sender_id, None, None, "group")
                .map_err(|e| e.to_string())?;
        }

        let group_name = crate::chat_kind::default_group_name(&envelope.group_id);
        crate::storage::db::upsert_chat(conn, &envelope.group_id, &group_name, true)
            .map_err(|e| e.to_string())?;
        crate::storage::db::add_chat_member(conn, &envelope.group_id, "Me", "member")
            .map_err(|e| e.to_string())?;
        crate::storage::db::add_chat_member(
            conn,
            &envelope.group_id,
            &envelope.sender_id,
            "member",
        )
        .map_err(|e| e.to_string())?;

        if envelope.content_type.needs_file_transfer() {
            if let Some(ref file_hash) = envelope.file_hash {
                let file_exists: bool = conn
                    .query_row(
                        "SELECT 1 FROM files WHERE file_hash = ?1",
                        [file_hash],
                        |_| Ok(true),
                    )
                    .unwrap_or(false);
                if !file_exists {
                    conn.execute(
                        "INSERT INTO files (file_hash, file_name, mime_type, size_bytes, is_complete) VALUES (?1, NULL, 'application/octet-stream', 0, 0)",
                        [file_hash],
                    )
                    .map_err(|e| e.to_string())?;
                }
            }
        }

        crate::storage::db::insert_message(conn, db_msg).map_err(|e| e.to_string())
    })
}

impl NetworkManager {
    async fn enqueue_persistence_task(
        &mut self,
        task: PersistenceTask,
        context: &str,
    ) -> Result<(), String> {
        if !self.persistence_accepting_tasks.load(Ordering::SeqCst) {
            return Err(format!("Persistence queue stopped in {}", context));
        }

        let remaining = self.persistence_task_tx.capacity();
        if remaining <= QUEUE_PRESSURE_THRESHOLD {
            println!(
                "[Persistence] ⚠️ Queue pressure in {}: {} slots remaining",
                context, remaining
            );
        }

        self.persistence_pending_tasks
            .fetch_add(1, Ordering::SeqCst);
        if let Err(e) = self.persistence_task_tx.send(task).await {
            self.persistence_pending_tasks
                .fetch_sub(1, Ordering::SeqCst);
            return Err(format!(
                "Failed to enqueue persistence task in {}: {}",
                context, e
            ));
        }

        Ok(())
    }

    pub(super) async fn persist_incoming_dm_message(
        &mut self,
        request: &DirectMessageRequest,
        chat_id: String,
        db_msg: crate::storage::db::Message,
    ) -> Result<(), String> {
        let (tx, rx) = tokio::sync::oneshot::channel();

        self.enqueue_persistence_task(
            PersistenceTask::PersistIncomingDirectMessage {
                request: request.clone(),
                chat_id,
                db_msg,
                reply: tx,
            },
            "persist_incoming_dm_message",
        )
        .await?;

        rx.await
            .map_err(|_| "Persistence worker dropped DM response".to_string())?
    }

    pub(super) async fn persist_incoming_group_message(
        &mut self,
        envelope: &GroupMessageEnvelope,
        db_msg: crate::storage::db::Message,
    ) -> Result<(), String> {
        let (tx, rx) = tokio::sync::oneshot::channel();

        self.enqueue_persistence_task(
            PersistenceTask::PersistIncomingGroupMessage {
                envelope: envelope.clone(),
                db_msg,
                reply: tx,
            },
            "persist_incoming_group_message",
        )
        .await?;

        rx.await
            .map_err(|_| "Persistence worker dropped group response".to_string())?
    }

    pub(super) async fn persist_delivered_status(&mut self, msg_id: String) -> Result<(), String> {
        let (tx, rx) = tokio::sync::oneshot::channel();

        self.enqueue_persistence_task(
            PersistenceTask::UpdateDeliveredStatus { msg_id, reply: tx },
            "persist_delivered_status",
        )
        .await?;

        rx.await
            .map_err(|_| "Persistence worker dropped delivered-status response".to_string())?
    }

    pub(super) async fn persist_read_statuses(
        &mut self,
        msg_ids: Vec<String>,
    ) -> Result<(), String> {
        let (tx, rx) = tokio::sync::oneshot::channel();

        self.enqueue_persistence_task(
            PersistenceTask::UpdateReadStatuses { msg_ids, reply: tx },
            "persist_read_statuses",
        )
        .await?;

        rx.await
            .map_err(|_| "Persistence worker dropped read-status response".to_string())?
    }

    pub(super) fn shutdown_persistence_workers_gracefully(&mut self, timeout: std::time::Duration) {
        self.persistence_accepting_tasks
            .store(false, Ordering::SeqCst);

        let worker_count = self.persistence_worker_handles.len();
        let deadline = std::time::Instant::now() + timeout;

        for _ in 0..worker_count {
            loop {
                match self.persistence_task_tx.try_send(PersistenceTask::Shutdown) {
                    Ok(_) => break,
                    Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
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
            if self.persistence_pending_tasks.load(Ordering::SeqCst) == 0
                && self.persistence_inflight_tasks.load(Ordering::SeqCst) == 0
            {
                if self
                    .persistence_worker_handles
                    .iter()
                    .all(|h| h.inner().is_finished())
                {
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        for handle in self.persistence_worker_handles.drain(..) {
            if !handle.inner().is_finished() {
                handle.abort();
            }
        }

        self.persistence_worker_shutdown
            .store(true, Ordering::SeqCst);
    }
}
