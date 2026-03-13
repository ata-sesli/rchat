use tauri::State;

use crate::chat_kind::{self, ChatKind};
use crate::network::command::{DirectMediaKind, NetworkCommand};
use crate::network::gossip::{GroupContentType, GroupMessageEnvelope};
use crate::storage;
use crate::{AppState, NetworkState};
use image::codecs::webp::WebPEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, ExtendedColorType};
use std::path::Path;

const MAX_STICKER_SIZE_BYTES: usize = 1_000_000; // 1 MB
const MIN_STICKER_SIDE_PX: u32 = 96;
const MAX_STICKER_SCALE_ATTEMPTS: u32 = 16;

#[derive(serde::Serialize)]
pub struct SentMediaResult {
    pub msg_id: String,
    pub file_hash: String,
    pub file_name: Option<String>,
}

#[derive(serde::Serialize)]
pub struct AddStickerResult {
    pub file_hash: String,
    pub name: String,
    pub converted: bool,
    pub already_exists: bool,
}

#[derive(serde::Serialize)]
pub struct StickerImportResult {
    pub file_path: String,
    pub file_hash: Option<String>,
    pub error: Option<String>,
}

#[derive(serde::Serialize)]
pub struct StickerBatchImportResult {
    pub success_count: usize,
    pub failure_count: usize,
    pub results: Vec<StickerImportResult>,
}

#[derive(Debug)]
struct PreparedSticker {
    file_name: String,
    file_data: Vec<u8>,
    converted: bool,
}

fn encode_webp_lossless(image: &DynamicImage) -> Result<Vec<u8>, String> {
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    let mut out = Vec::new();
    WebPEncoder::new_lossless(&mut out)
        .encode(&rgba, width, height, ExtendedColorType::Rgba8)
        .map_err(|e| format!("Failed to encode WebP: {}", e))?;
    Ok(out)
}

fn convert_to_webp_with_auto_downscale(image: DynamicImage) -> Result<Vec<u8>, String> {
    let mut current = image;
    for _ in 0..=MAX_STICKER_SCALE_ATTEMPTS {
        let encoded = encode_webp_lossless(&current)?;
        if encoded.len() <= MAX_STICKER_SIZE_BYTES {
            return Ok(encoded);
        }

        let width = current.width();
        let height = current.height();
        if width <= MIN_STICKER_SIDE_PX || height <= MIN_STICKER_SIDE_PX {
            break;
        }

        let next_w = ((width as f32) * 0.85).round() as u32;
        let next_h = ((height as f32) * 0.85).round() as u32;
        let next_w = next_w.max(MIN_STICKER_SIDE_PX);
        let next_h = next_h.max(MIN_STICKER_SIDE_PX);

        if next_w == width && next_h == height {
            break;
        }

        current = current.resize(next_w, next_h, FilterType::Lanczos3);
    }

    Err("Converted WebP sticker is still larger than 1MB after auto-compression".to_string())
}

fn sticker_name_from_path(file_path: &str) -> String {
    let stem = Path::new(file_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or("sticker");
    format!("{}.webp", stem)
}

fn prepare_sticker_for_import(file_path: &str) -> Result<PreparedSticker, String> {
    let input_data = std::fs::read(file_path)
        .map_err(|e| format!("Failed to read file '{}': {}", file_path, e))?;

    let ext = Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();

    let file_name = sticker_name_from_path(file_path);

    match ext.as_str() {
        "webp" => {
            if input_data.len() > MAX_STICKER_SIZE_BYTES {
                return Err("WebP sticker exceeds 1MB limit".to_string());
            }
            Ok(PreparedSticker {
                file_name,
                file_data: input_data,
                converted: false,
            })
        }
        "png" | "jpg" | "jpeg" => {
            let image = image::load_from_memory(&input_data)
                .map_err(|e| format!("Failed to decode image: {}", e))?;
            let converted = convert_to_webp_with_auto_downscale(image)?;
            Ok(PreparedSticker {
                file_name,
                file_data: converted,
                converted: true,
            })
        }
        _ => Err(
            "Unsupported sticker format. Use .webp directly or import .png/.jpg/.jpeg".to_string(),
        ),
    }
}

fn detect_audio_mime(file_path: &str) -> Option<&'static str> {
    match file_path
        .rsplit('.')
        .next()
        .map(|ext| ext.to_ascii_lowercase())
    {
        Some(ext) if ext == "mp3" => Some("audio/mpeg"),
        Some(ext) if ext == "m4a" => Some("audio/mp4"),
        Some(ext) if ext == "wav" => Some("audio/wav"),
        Some(ext) if ext == "ogg" => Some("audio/ogg"),
        Some(ext) if ext == "webm" => Some("audio/webm"),
        Some(ext) if ext == "opus" => Some("audio/opus"),
        _ => None,
    }
}

fn detect_image_mime_from_bytes(data: &[u8]) -> Option<&'static str> {
    match image::guess_format(data).ok()? {
        image::ImageFormat::Png => Some("image/png"),
        image::ImageFormat::Jpeg => Some("image/jpeg"),
        image::ImageFormat::Gif => Some("image/gif"),
        image::ImageFormat::WebP => Some("image/webp"),
        _ => None,
    }
}

fn detect_audio_mime_from_bytes(data: &[u8]) -> Option<&'static str> {
    if data.len() >= 12 {
        if &data[0..4] == b"RIFF" && &data[8..12] == b"WAVE" {
            return Some("audio/wav");
        }
        if &data[4..8] == b"ftyp" {
            return Some("audio/mp4");
        }
    }
    if data.len() >= 4 && &data[0..4] == b"OggS" {
        return Some("audio/ogg");
    }
    if data.len() >= 4 && data[0..4] == [0x1A, 0x45, 0xDF, 0xA3] {
        return Some("audio/webm");
    }
    if data.len() >= 3 && &data[0..3] == b"ID3" {
        return Some("audio/mpeg");
    }
    if data.len() >= 2 && data[0] == 0xFF && (data[1] & 0xE0) == 0xE0 {
        return Some("audio/mpeg");
    }
    None
}

fn outgoing_status_for_chat(chat_kind: ChatKind) -> Result<&'static str, String> {
    match chat_kind {
        ChatKind::SelfChat => Ok("read"),
        ChatKind::Direct | ChatKind::TemporaryDirect => Ok("pending"),
        ChatKind::Group | ChatKind::TemporaryGroup => Ok("delivered"),
        ChatKind::Archived => Err("Archived chats are read-only".to_string()),
    }
}

fn ensure_persisted_outgoing_chat(
    conn: &rusqlite::Connection,
    chat_kind: ChatKind,
    canonical_chat_id: &str,
) -> Result<(), String> {
    match chat_kind {
        ChatKind::Direct => {
            if !storage::db::is_peer(conn, canonical_chat_id) {
                storage::db::add_peer(
                    conn,
                    canonical_chat_id,
                    Some(&default_direct_chat_name(canonical_chat_id)),
                    None,
                    if canonical_chat_id.starts_with("gh:") {
                        "github"
                    } else {
                        "local"
                    },
                )
                .map_err(|e| e.to_string())?;
            }

            if !storage::db::chat_exists(conn, canonical_chat_id) {
                storage::db::create_chat(
                    conn,
                    canonical_chat_id,
                    &default_direct_chat_name(canonical_chat_id),
                    false,
                )
                    .map_err(|e| e.to_string())?;
            }
        }
        ChatKind::Group => {
            if !storage::db::chat_exists(conn, canonical_chat_id) {
                storage::db::upsert_chat(
                    conn,
                    canonical_chat_id,
                    &chat_kind::default_group_name(canonical_chat_id),
                    true,
                )
                .map_err(|e| e.to_string())?;
            }
            storage::db::add_chat_member(conn, canonical_chat_id, "Me", "member")
                .map_err(|e| e.to_string())?;
        }
        ChatKind::SelfChat
        | ChatKind::TemporaryDirect
        | ChatKind::TemporaryGroup
        | ChatKind::Archived => {}
    }

    Ok(())
}

fn default_direct_chat_name(chat_id: &str) -> String {
    crate::chat_identity::extract_name_from_chat_id(chat_id)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "peer".to_string())
}

async fn store_outgoing_temp_message(
    net_state: &State<'_, NetworkState>,
    chat_id: &str,
    msg: storage::db::Message,
) {
    let mut temp_state = net_state.temporary_state.lock().await;
    temp_state
        .messages
        .entry(chat_id.to_string())
        .or_default()
        .push(msg);
}

async fn canonical_direct_chat_id(
    app_state: &State<'_, AppState>,
    peer_id: &str,
) -> String {
    if !matches!(chat_kind::parse_chat_kind(peer_id), ChatKind::Direct) {
        return peer_id.to_string();
    }
    if peer_id.starts_with("gh:") || peer_id.starts_with("lh:") {
        return peer_id.to_string();
    }

    let mgr = app_state.config_manager.lock().await;
    let Ok(config) = mgr.load().await else {
        return crate::chat_identity::build_local_chat_id("peer", peer_id);
    };
    if let Some(mapped) =
        crate::chat_identity::github_chat_id_for_peer_id(peer_id, &config.user.github_peer_mapping)
    {
        return mapped;
    }

    let conn = match app_state.db_conn.lock() {
        Ok(conn) => conn,
        Err(_) => return crate::chat_identity::build_local_chat_id("peer", peer_id),
    };

    if let Ok(Some(existing_lh)) = storage::db::find_existing_local_chat_id_for_peer(&conn, peer_id)
    {
        return existing_lh;
    }

    let local_name = storage::db::get_peer_alias(&conn, peer_id)
        .ok()
        .flatten()
        .filter(|name| !name.trim().is_empty() && name != peer_id)
        .unwrap_or_else(|| "peer".to_string());
    crate::chat_identity::build_local_chat_id(&local_name, peer_id)
}

async fn resolve_direct_target_peer_id(
    _app_state: &State<'_, AppState>,
    chat_id: &str,
) -> String {
    crate::chat_identity::resolve_peer_id_for_direct_chat_id(chat_id)
        .unwrap_or_else(|| chat_id.to_string())
}

#[tauri::command]
pub async fn send_image_message(
    peer_id: String,
    file_path: String,
    app_state: State<'_, AppState>,
    net_state: State<'_, NetworkState>,
) -> Result<SentMediaResult, String> {
    println!(
        "[Backend] send_image_message: to {} from {}",
        peer_id, file_path
    );
    let canonical_peer_id = canonical_direct_chat_id(&app_state, &peer_id).await;

    let file_data = std::fs::read(&file_path).map_err(|e| format!("Failed to read file: {}", e))?;

    let mime_type = match std::path::Path::new(&file_path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
    {
        Some(ext) if ext == "jpg" || ext == "jpeg" => "image/jpeg",
        Some(ext) if ext == "png" => "image/png",
        Some(ext) if ext == "gif" => "image/gif",
        Some(ext) if ext == "webp" => "image/webp",
        _ => "image/png",
    };

    let file_name = std::path::Path::new(&file_path)
        .file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string());

    let file_hash = {
        let conn = app_state.db_conn.lock().map_err(|e| e.to_string())?;
        storage::object::create(
            &conn,
            &file_data,
            file_name.as_deref(),
            Some(mime_type),
            None,
        )
        .map_err(|e| format!("Failed to store image: {}", e))?
    };

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let id_suffix: u32 = rand::random();
    let msg_id = format!("{}-{}", timestamp, id_suffix);

    let chat_kind = chat_kind::parse_chat_kind(&canonical_peer_id);
    let is_temporary = matches!(
        chat_kind,
        ChatKind::TemporaryDirect | ChatKind::TemporaryGroup
    );
    let status = outgoing_status_for_chat(chat_kind)?;
    let chat_id = if matches!(chat_kind, ChatKind::SelfChat) {
        "self".to_string()
    } else {
        canonical_peer_id.clone()
    };
    let message = storage::db::Message {
        id: msg_id.clone(),
        chat_id: chat_id.clone(),
        peer_id: "Me".to_string(),
        timestamp,
        content_type: "image".to_string(),
        text_content: None,
        file_hash: Some(file_hash.clone()),
        status: status.to_string(),
        content_metadata: None,
        sender_alias: None,
    };

    if is_temporary {
        store_outgoing_temp_message(&net_state, &chat_id, message).await;
    } else {
        let conn = app_state.db_conn.lock().map_err(|e| e.to_string())?;
        ensure_persisted_outgoing_chat(&conn, chat_kind, &canonical_peer_id)?;
        if let Err(e) = storage::db::insert_message(&conn, &message) {
            eprintln!("[Backend] Failed to save image message: {}", e);
            return Err(e.to_string());
        }
    }

    if !matches!(chat_kind, ChatKind::SelfChat) {
        let direct_target_peer_id = resolve_direct_target_peer_id(&app_state, &canonical_peer_id).await;
        let tx = net_state.sender.lock().await;
        match chat_kind {
            ChatKind::SelfChat => {}
            ChatKind::Direct | ChatKind::TemporaryDirect => {
                tx.send(NetworkCommand::SendDirectMedia {
                    kind: DirectMediaKind::Image,
                    target_peer_id: direct_target_peer_id,
                    file_hash: file_hash.clone(),
                    file_name: None,
                    msg_id: msg_id.clone(),
                    timestamp,
                })
                .await
                .map_err(|e| e.to_string())?;
            }
            ChatKind::Group | ChatKind::TemporaryGroup => {
                let envelope = GroupMessageEnvelope {
                    id: msg_id.clone(),
                    group_id: canonical_peer_id.clone(),
                    sender_id: "Me".to_string(),
                    sender_alias: None,
                    timestamp,
                    content_type: GroupContentType::Image,
                    text_content: None,
                    file_hash: Some(file_hash.clone()),
                };
                tx.send(NetworkCommand::PublishGroup { envelope })
                    .await
                    .map_err(|e| e.to_string())?;
            }
            ChatKind::Archived => {}
        }
    }

    println!("[Backend] Image message sent: hash={}", file_hash);
    Ok(SentMediaResult {
        msg_id,
        file_hash,
        file_name,
    })
}

#[tauri::command]
pub async fn get_image_data(
    file_hash: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let conn = state.db_conn.lock().map_err(|e| e.to_string())?;

    let data = storage::object::load(&conn, &file_hash, None)
        .map_err(|e| format!("Failed to load image: {}", e))?;

    let stored_mime_type: String = conn
        .query_row(
            "SELECT COALESCE(mime_type, 'image/png') FROM files WHERE file_hash = ?1",
            [&file_hash],
            |row| row.get(0),
        )
        .unwrap_or_else(|_| "image/png".to_string());
    let resolved_mime_type = if stored_mime_type.starts_with("image/") {
        stored_mime_type.clone()
    } else {
        detect_image_mime_from_bytes(&data)
            .unwrap_or("image/png")
            .to_string()
    };
    if resolved_mime_type != stored_mime_type {
        let _ = conn.execute(
            "UPDATE files SET mime_type = ?2 WHERE file_hash = ?1",
            rusqlite::params![&file_hash, &resolved_mime_type],
        );
    }

    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let b64 = STANDARD.encode(&data);
    let data_url = format!("data:{};base64,{}", resolved_mime_type, b64);

    Ok(data_url)
}

#[tauri::command]
pub async fn get_image_from_path(file_path: String) -> Result<String, String> {
    let data =
        std::fs::read(&file_path).map_err(|e| format!("Failed to read image file: {}", e))?;

    let mime_type = if file_path.ends_with(".png") {
        "image/png"
    } else if file_path.ends_with(".jpg") || file_path.ends_with(".jpeg") {
        "image/jpeg"
    } else if file_path.ends_with(".gif") {
        "image/gif"
    } else if file_path.ends_with(".webp") {
        "image/webp"
    } else {
        "image/png"
    };

    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let b64 = STANDARD.encode(&data);
    let data_url = format!("data:{};base64,{}", mime_type, b64);

    Ok(data_url)
}

#[tauri::command]
pub async fn save_image_to_file(
    file_hash: String,
    target_path: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let conn = state.db_conn.lock().map_err(|e| e.to_string())?;

    let data = storage::object::load(&conn, &file_hash, None)
        .map_err(|e| format!("Failed to load image: {}", e))?;

    std::fs::write(&target_path, &data).map_err(|e| format!("Failed to save image: {}", e))?;

    println!("[Backend] Image saved to: {}", target_path);
    Ok(())
}

#[tauri::command]
pub async fn send_document_message(
    peer_id: String,
    file_path: String,
    app_state: State<'_, AppState>,
    net_state: State<'_, NetworkState>,
) -> Result<SentMediaResult, String> {
    println!("[Backend] Sending document to {}: {}", peer_id, file_path);
    let canonical_peer_id = canonical_direct_chat_id(&app_state, &peer_id).await;
    let chat_kind = chat_kind::parse_chat_kind(&canonical_peer_id);

    let file_data =
        std::fs::read(&file_path).map_err(|e| format!("Failed to read document: {}", e))?;

    let file_name = std::path::Path::new(&file_path)
        .file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "document".to_string());

    let mime_type = match file_path.rsplit('.').next() {
        Some("pdf") => "application/pdf",
        Some("doc") => "application/msword",
        Some("docx") => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        Some("txt") => "text/plain",
        Some("xls") => "application/vnd.ms-excel",
        Some("xlsx") => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        Some("ppt") => "application/vnd.ms-powerpoint",
        Some("pptx") => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        Some("csv") => "text/csv",
        _ => "application/octet-stream",
    };

    let file_hash = {
        let conn = app_state.db_conn.lock().map_err(|e| e.to_string())?;
        storage::object::create(&conn, &file_data, Some(&file_name), Some(mime_type), None)
            .map_err(|e| format!("Failed to store document: {}", e))?
    };

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let id_suffix: u32 = rand::random();
    let msg_id = format!("{}-{}", timestamp, id_suffix);

    let is_temporary = matches!(
        chat_kind,
        ChatKind::TemporaryDirect | ChatKind::TemporaryGroup
    );
    let status = outgoing_status_for_chat(chat_kind)?;
    let chat_id = if matches!(chat_kind, ChatKind::SelfChat) {
        "self".to_string()
    } else {
        canonical_peer_id.clone()
    };
    let message = storage::db::Message {
        id: msg_id.clone(),
        chat_id: chat_id.clone(),
        peer_id: "Me".to_string(),
        timestamp,
        content_type: "document".to_string(),
        text_content: Some(file_name.clone()),
        file_hash: Some(file_hash.clone()),
        status: status.to_string(),
        content_metadata: Some(format!("{{\"size_bytes\":{}}}", file_data.len())),
        sender_alias: None,
    };

    if is_temporary {
        store_outgoing_temp_message(&net_state, &chat_id, message).await;
    } else {
        let conn = app_state.db_conn.lock().map_err(|e| e.to_string())?;
        ensure_persisted_outgoing_chat(&conn, chat_kind, &canonical_peer_id)?;

        if let Err(e) = storage::db::insert_message(&conn, &message) {
            eprintln!("[Backend] Failed to save document message: {}", e);
            return Err(e.to_string());
        }
    }

    if !matches!(chat_kind, ChatKind::SelfChat) {
        let direct_target_peer_id = resolve_direct_target_peer_id(&app_state, &canonical_peer_id).await;
        let tx = net_state.sender.lock().await;
        match chat_kind {
            ChatKind::SelfChat => {}
            ChatKind::Direct | ChatKind::TemporaryDirect => {
                tx.send(NetworkCommand::SendDirectMedia {
                    kind: DirectMediaKind::Document,
                    target_peer_id: direct_target_peer_id,
                    file_hash: file_hash.clone(),
                    file_name: Some(file_name.clone()),
                    msg_id: msg_id.clone(),
                    timestamp,
                })
                .await
                .map_err(|e| e.to_string())?;
            }
            ChatKind::Group | ChatKind::TemporaryGroup => {
                let envelope = GroupMessageEnvelope {
                    id: msg_id.clone(),
                    group_id: canonical_peer_id.clone(),
                    sender_id: "Me".to_string(),
                    sender_alias: None,
                    timestamp,
                    content_type: GroupContentType::Document,
                    text_content: Some(file_name.clone()),
                    file_hash: Some(file_hash.clone()),
                };
                tx.send(NetworkCommand::PublishGroup { envelope })
                    .await
                    .map_err(|e| e.to_string())?;
            }
            ChatKind::Archived => {}
        }
    }

    println!(
        "[Backend] Document message sent: hash={}, name={}",
        file_hash, file_name
    );
    Ok(SentMediaResult {
        msg_id,
        file_hash,
        file_name: Some(file_name),
    })
}

#[tauri::command]
pub async fn save_document_to_file(
    file_hash: String,
    target_path: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let conn = state.db_conn.lock().map_err(|e| e.to_string())?;

    let data = storage::object::load(&conn, &file_hash, None)
        .map_err(|e| format!("Failed to load document: {}", e))?;

    std::fs::write(&target_path, &data).map_err(|e| format!("Failed to save document: {}", e))?;

    println!("[Backend] Document saved to: {}", target_path);
    Ok(())
}

#[tauri::command]
pub async fn send_video_message(
    peer_id: String,
    file_path: String,
    app_state: State<'_, AppState>,
    net_state: State<'_, NetworkState>,
) -> Result<SentMediaResult, String> {
    println!("[Backend] Sending video to {}: {}", peer_id, file_path);
    let canonical_peer_id = canonical_direct_chat_id(&app_state, &peer_id).await;
    let chat_kind = chat_kind::parse_chat_kind(&canonical_peer_id);

    let file_data =
        std::fs::read(&file_path).map_err(|e| format!("Failed to read video: {}", e))?;

    let file_name = std::path::Path::new(&file_path)
        .file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "video.mp4".to_string());

    let mime_type = match file_path.rsplit('.').next() {
        Some("mp4") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mov") => "video/quicktime",
        Some("avi") => "video/x-msvideo",
        Some("mkv") => "video/x-matroska",
        _ => "video/mp4",
    };

    let file_hash = {
        let conn = app_state.db_conn.lock().map_err(|e| e.to_string())?;
        storage::object::create(&conn, &file_data, Some(&file_name), Some(mime_type), None)
            .map_err(|e| format!("Failed to store video: {}", e))?
    };

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let id_suffix: u32 = rand::random();
    let msg_id = format!("{}-{}", timestamp, id_suffix);

    let is_temporary = matches!(
        chat_kind,
        ChatKind::TemporaryDirect | ChatKind::TemporaryGroup
    );
    let status = outgoing_status_for_chat(chat_kind)?;
    let chat_id = if matches!(chat_kind, ChatKind::SelfChat) {
        "self".to_string()
    } else {
        canonical_peer_id.clone()
    };
    let message = storage::db::Message {
        id: msg_id.clone(),
        chat_id: chat_id.clone(),
        peer_id: "Me".to_string(),
        timestamp,
        content_type: "video".to_string(),
        text_content: Some(file_name.clone()),
        file_hash: Some(file_hash.clone()),
        status: status.to_string(),
        content_metadata: Some(format!("{{\"size_bytes\":{}}}", file_data.len())),
        sender_alias: None,
    };

    if is_temporary {
        store_outgoing_temp_message(&net_state, &chat_id, message).await;
    } else {
        let conn = app_state.db_conn.lock().map_err(|e| e.to_string())?;
        ensure_persisted_outgoing_chat(&conn, chat_kind, &canonical_peer_id)?;

        if let Err(e) = storage::db::insert_message(&conn, &message) {
            eprintln!("[Backend] Failed to save video message: {}", e);
            return Err(e.to_string());
        }
    }

    if !matches!(chat_kind, ChatKind::SelfChat) {
        let direct_target_peer_id = resolve_direct_target_peer_id(&app_state, &canonical_peer_id).await;
        let tx = net_state.sender.lock().await;
        match chat_kind {
            ChatKind::SelfChat => {}
            ChatKind::Direct | ChatKind::TemporaryDirect => {
                tx.send(NetworkCommand::SendDirectMedia {
                    kind: DirectMediaKind::Video,
                    target_peer_id: direct_target_peer_id,
                    file_hash: file_hash.clone(),
                    file_name: Some(file_name.clone()),
                    msg_id: msg_id.clone(),
                    timestamp,
                })
                .await
                .map_err(|e| e.to_string())?;
            }
            ChatKind::Group | ChatKind::TemporaryGroup => {
                let envelope = GroupMessageEnvelope {
                    id: msg_id.clone(),
                    group_id: canonical_peer_id.clone(),
                    sender_id: "Me".to_string(),
                    sender_alias: None,
                    timestamp,
                    content_type: GroupContentType::Video,
                    text_content: Some(file_name.clone()),
                    file_hash: Some(file_hash.clone()),
                };
                tx.send(NetworkCommand::PublishGroup { envelope })
                    .await
                    .map_err(|e| e.to_string())?;
            }
            ChatKind::Archived => {}
        }
    }

    println!(
        "[Backend] Video message sent: hash={}, name={}",
        file_hash, file_name
    );
    Ok(SentMediaResult {
        msg_id,
        file_hash,
        file_name: Some(file_name),
    })
}

#[tauri::command]
pub async fn get_video_data(
    file_hash: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let conn = state.db_conn.lock().map_err(|e| e.to_string())?;

    let data = storage::object::load(&conn, &file_hash, None)
        .map_err(|e| format!("Failed to load video: {}", e))?;

    let mime_type: String = conn
        .query_row(
            "SELECT COALESCE(mime_type, 'video/mp4') FROM files WHERE file_hash = ?1",
            [&file_hash],
            |row| row.get(0),
        )
        .unwrap_or_else(|_| "video/mp4".to_string());

    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let b64 = STANDARD.encode(&data);
    let data_url = format!("data:{};base64,{}", mime_type, b64);

    Ok(data_url)
}

#[tauri::command]
pub async fn send_audio_message(
    peer_id: String,
    file_path: String,
    app_state: State<'_, AppState>,
    net_state: State<'_, NetworkState>,
) -> Result<SentMediaResult, String> {
    println!("[Backend] Sending audio to {}: {}", peer_id, file_path);
    let canonical_peer_id = canonical_direct_chat_id(&app_state, &peer_id).await;
    let chat_kind = chat_kind::parse_chat_kind(&canonical_peer_id);

    let file_data =
        std::fs::read(&file_path).map_err(|e| format!("Failed to read audio: {}", e))?;

    let file_name = std::path::Path::new(&file_path)
        .file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "audio".to_string());

    let mime_type = detect_audio_mime(&file_path).ok_or_else(|| {
        "Unsupported audio format. Allowed: mp3, m4a, wav, ogg, webm, opus".to_string()
    })?;

    let file_hash = {
        let conn = app_state.db_conn.lock().map_err(|e| e.to_string())?;
        storage::object::create(&conn, &file_data, Some(&file_name), Some(mime_type), None)
            .map_err(|e| format!("Failed to store audio: {}", e))?
    };

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let id_suffix: u32 = rand::random();
    let msg_id = format!("{}-{}", timestamp, id_suffix);

    let is_temporary = matches!(
        chat_kind,
        ChatKind::TemporaryDirect | ChatKind::TemporaryGroup
    );
    let status = outgoing_status_for_chat(chat_kind)?;
    let chat_id = if matches!(chat_kind, ChatKind::SelfChat) {
        "self".to_string()
    } else {
        canonical_peer_id.clone()
    };
    let message = storage::db::Message {
        id: msg_id.clone(),
        chat_id: chat_id.clone(),
        peer_id: "Me".to_string(),
        timestamp,
        content_type: "audio".to_string(),
        text_content: Some(file_name.clone()),
        file_hash: Some(file_hash.clone()),
        status: status.to_string(),
        content_metadata: Some(format!("{{\"size_bytes\":{}}}", file_data.len())),
        sender_alias: None,
    };

    if is_temporary {
        store_outgoing_temp_message(&net_state, &chat_id, message).await;
    } else {
        let conn = app_state.db_conn.lock().map_err(|e| e.to_string())?;
        ensure_persisted_outgoing_chat(&conn, chat_kind, &canonical_peer_id)?;

        if let Err(e) = storage::db::insert_message(&conn, &message) {
            eprintln!("[Backend] Failed to save audio message: {}", e);
            return Err(e.to_string());
        }
    }

    if !matches!(chat_kind, ChatKind::SelfChat) {
        let direct_target_peer_id = resolve_direct_target_peer_id(&app_state, &canonical_peer_id).await;
        let tx = net_state.sender.lock().await;
        match chat_kind {
            ChatKind::SelfChat => {}
            ChatKind::Direct | ChatKind::TemporaryDirect => {
                tx.send(NetworkCommand::SendDirectMedia {
                    kind: DirectMediaKind::Audio,
                    target_peer_id: direct_target_peer_id,
                    file_hash: file_hash.clone(),
                    file_name: Some(file_name.clone()),
                    msg_id: msg_id.clone(),
                    timestamp,
                })
                .await
                .map_err(|e| e.to_string())?;
            }
            ChatKind::Group | ChatKind::TemporaryGroup => {
                let envelope = GroupMessageEnvelope {
                    id: msg_id.clone(),
                    group_id: canonical_peer_id.clone(),
                    sender_id: "Me".to_string(),
                    sender_alias: None,
                    timestamp,
                    content_type: GroupContentType::Audio,
                    text_content: Some(file_name.clone()),
                    file_hash: Some(file_hash.clone()),
                };
                tx.send(NetworkCommand::PublishGroup { envelope })
                    .await
                    .map_err(|e| e.to_string())?;
            }
            ChatKind::Archived => {}
        }
    }

    println!(
        "[Backend] Audio message sent: hash={}, name={}",
        file_hash, file_name
    );
    Ok(SentMediaResult {
        msg_id,
        file_hash,
        file_name: Some(file_name),
    })
}

#[tauri::command]
pub async fn get_audio_data(
    file_hash: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let conn = state.db_conn.lock().map_err(|e| e.to_string())?;

    let data = storage::object::load(&conn, &file_hash, None)
        .map_err(|e| format!("Failed to load audio: {}", e))?;

    let stored_mime_type: String = conn
        .query_row(
            "SELECT COALESCE(mime_type, 'audio/mpeg') FROM files WHERE file_hash = ?1",
            [&file_hash],
            |row| row.get(0),
        )
        .unwrap_or_else(|_| "audio/mpeg".to_string());
    let resolved_mime_type = if stored_mime_type.starts_with("audio/") {
        stored_mime_type.clone()
    } else {
        detect_audio_mime_from_bytes(&data)
            .unwrap_or("audio/webm")
            .to_string()
    };
    if resolved_mime_type != stored_mime_type {
        let _ = conn.execute(
            "UPDATE files SET mime_type = ?2 WHERE file_hash = ?1",
            rusqlite::params![&file_hash, &resolved_mime_type],
        );
    }

    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let b64 = STANDARD.encode(&data);
    let data_url = format!("data:{};base64,{}", resolved_mime_type, b64);

    Ok(data_url)
}

#[tauri::command]
pub async fn save_audio_to_file(
    file_hash: String,
    target_path: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let conn = state.db_conn.lock().map_err(|e| e.to_string())?;

    let data = storage::object::load(&conn, &file_hash, None)
        .map_err(|e| format!("Failed to load audio: {}", e))?;

    std::fs::write(&target_path, &data).map_err(|e| format!("Failed to save audio: {}", e))?;

    println!("[Backend] Audio saved to: {}", target_path);
    Ok(())
}

#[tauri::command]
pub async fn list_stickers(
    state: State<'_, AppState>,
) -> Result<Vec<storage::db::Sticker>, String> {
    let conn = state.db_conn.lock().map_err(|e| e.to_string())?;
    storage::db::list_stickers(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn add_sticker(
    file_path: String,
    state: State<'_, AppState>,
) -> Result<AddStickerResult, String> {
    let prepared = prepare_sticker_for_import(&file_path)?;

    let conn = state.db_conn.lock().map_err(|e| e.to_string())?;
    let file_hash = storage::object::create(
        &conn,
        &prepared.file_data,
        Some(&prepared.file_name),
        Some("image/webp"),
        None,
    )
    .map_err(|e| format!("Failed to store sticker file: {}", e))?;

    let inserted =
        storage::db::upsert_sticker(&conn, &file_hash, Some(&prepared.file_name), "local")
            .map_err(|e| format!("Failed to register sticker: {}", e))?;

    Ok(AddStickerResult {
        file_hash,
        name: prepared.file_name,
        converted: prepared.converted,
        already_exists: !inserted,
    })
}

#[tauri::command]
pub async fn add_stickers_batch(
    file_paths: Vec<String>,
    state: State<'_, AppState>,
) -> Result<StickerBatchImportResult, String> {
    let mut results = Vec::with_capacity(file_paths.len());
    let mut success_count = 0usize;
    let mut failure_count = 0usize;

    for file_path in file_paths {
        match prepare_sticker_for_import(&file_path) {
            Ok(prepared) => {
                let item = {
                    let conn = state.db_conn.lock().map_err(|e| e.to_string())?;
                    match storage::object::create(
                        &conn,
                        &prepared.file_data,
                        Some(&prepared.file_name),
                        Some("image/webp"),
                        None,
                    ) {
                        Ok(file_hash) => {
                            match storage::db::upsert_sticker(
                                &conn,
                                &file_hash,
                                Some(&prepared.file_name),
                                "local",
                            ) {
                                Ok(_) => StickerImportResult {
                                    file_path: file_path.clone(),
                                    file_hash: Some(file_hash),
                                    error: None,
                                },
                                Err(e) => StickerImportResult {
                                    file_path: file_path.clone(),
                                    file_hash: None,
                                    error: Some(format!("Failed to register sticker: {}", e)),
                                },
                            }
                        }
                        Err(e) => StickerImportResult {
                            file_path: file_path.clone(),
                            file_hash: None,
                            error: Some(format!("Failed to store sticker file: {}", e)),
                        },
                    }
                };

                if item.error.is_none() {
                    success_count += 1;
                } else {
                    failure_count += 1;
                }
                results.push(item);
            }
            Err(e) => {
                failure_count += 1;
                results.push(StickerImportResult {
                    file_path,
                    file_hash: None,
                    error: Some(e),
                });
            }
        }
    }

    Ok(StickerBatchImportResult {
        success_count,
        failure_count,
        results,
    })
}

#[tauri::command]
pub async fn delete_sticker(file_hash: String, state: State<'_, AppState>) -> Result<(), String> {
    let conn = state.db_conn.lock().map_err(|e| e.to_string())?;
    storage::db::delete_sticker(&conn, &file_hash).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn save_sticker_from_message(
    file_hash: String,
    state: State<'_, AppState>,
) -> Result<AddStickerResult, String> {
    let conn = state.db_conn.lock().map_err(|e| e.to_string())?;

    let exists_in_files: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM files WHERE file_hash = ?1)",
            [&file_hash],
            |row| row.get(0),
        )
        .map_err(|e| format!("Failed to check sticker file: {}", e))?;

    if !exists_in_files {
        return Err("Sticker file is not available locally yet".to_string());
    }

    let name: String = conn
        .query_row(
            "SELECT COALESCE(file_name, ?2) FROM files WHERE file_hash = ?1",
            rusqlite::params![
                &file_hash,
                format!("sticker-{}.webp", &file_hash[..8.min(file_hash.len())])
            ],
            |row| row.get(0),
        )
        .unwrap_or_else(|_| format!("sticker-{}.webp", &file_hash[..8.min(file_hash.len())]));

    let inserted = storage::db::upsert_sticker(&conn, &file_hash, Some(&name), "received")
        .map_err(|e| format!("Failed to save sticker to library: {}", e))?;

    Ok(AddStickerResult {
        file_hash,
        name,
        converted: false,
        already_exists: !inserted,
    })
}

#[tauri::command]
pub async fn send_sticker_message(
    peer_id: String,
    file_hash: String,
    app_state: State<'_, AppState>,
    net_state: State<'_, NetworkState>,
) -> Result<SentMediaResult, String> {
    let canonical_peer_id = canonical_direct_chat_id(&app_state, &peer_id).await;
    let chat_kind = chat_kind::parse_chat_kind(&canonical_peer_id);

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let id_suffix: u32 = rand::random();
    let msg_id = format!("{}-{}", timestamp, id_suffix);
    let is_temporary = matches!(
        chat_kind,
        ChatKind::TemporaryDirect | ChatKind::TemporaryGroup
    );
    let status = outgoing_status_for_chat(chat_kind)?;

    let (file_name, chat_id) = {
        let conn = app_state.db_conn.lock().map_err(|e| e.to_string())?;

        if !storage::db::sticker_exists(&conn, &file_hash) {
            return Err("Sticker not found in local library".to_string());
        }

        let file_exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM files WHERE file_hash = ?1)",
                [&file_hash],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to check sticker file: {}", e))?;
        if !file_exists {
            return Err("Sticker file is missing from local storage".to_string());
        }

        let file_name: Option<String> = conn
            .query_row(
                "SELECT file_name FROM files WHERE file_hash = ?1",
                [&file_hash],
                |row| row.get(0),
            )
            .ok();

        let chat_id = if matches!(chat_kind, ChatKind::SelfChat) {
            "self".to_string()
        } else {
            canonical_peer_id.clone()
        };

        if !is_temporary {
            ensure_persisted_outgoing_chat(&conn, chat_kind, &canonical_peer_id)?;
        }

        (file_name, chat_id)
    };

    let message = storage::db::Message {
        id: msg_id.clone(),
        chat_id: chat_id.clone(),
        peer_id: "Me".to_string(),
        timestamp,
        content_type: "sticker".to_string(),
        text_content: None,
        file_hash: Some(file_hash.clone()),
        status: status.to_string(),
        content_metadata: None,
        sender_alias: None,
    };

    if is_temporary {
        store_outgoing_temp_message(&net_state, &chat_id, message).await;
    } else {
        let conn = app_state.db_conn.lock().map_err(|e| e.to_string())?;
        storage::db::insert_message(&conn, &message)
            .map_err(|e| format!("Failed to save sticker message: {}", e))?;
    }

    if !matches!(chat_kind, ChatKind::SelfChat) {
        let direct_target_peer_id = resolve_direct_target_peer_id(&app_state, &canonical_peer_id).await;
        let tx = net_state.sender.lock().await;
        match chat_kind {
            ChatKind::SelfChat => {}
            ChatKind::Direct | ChatKind::TemporaryDirect => {
                tx.send(NetworkCommand::SendDirectMedia {
                    kind: DirectMediaKind::Sticker,
                    target_peer_id: direct_target_peer_id,
                    file_hash: file_hash.clone(),
                    file_name: None,
                    msg_id: msg_id.clone(),
                    timestamp,
                })
                .await
                .map_err(|e| e.to_string())?;
            }
            ChatKind::Group | ChatKind::TemporaryGroup => {
                let envelope = GroupMessageEnvelope {
                    id: msg_id.clone(),
                    group_id: canonical_peer_id.clone(),
                    sender_id: "Me".to_string(),
                    sender_alias: None,
                    timestamp,
                    content_type: GroupContentType::Sticker,
                    text_content: None,
                    file_hash: Some(file_hash.clone()),
                };
                tx.send(NetworkCommand::PublishGroup { envelope })
                    .await
                    .map_err(|e| e.to_string())?;
            }
            ChatKind::Archived => {}
        }
    }

    Ok(SentMediaResult {
        msg_id,
        file_hash,
        file_name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn prepare_sticker_rejects_unsupported_format() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("sticker.txt");
        std::fs::write(&path, b"not-an-image").expect("write");

        let err =
            prepare_sticker_for_import(path.to_str().expect("path")).expect_err("expected error");
        assert!(err.contains("Unsupported sticker format"));
    }

    #[test]
    fn prepare_sticker_converts_png_to_webp() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("sample.png");

        let image = image::RgbaImage::from_pixel(256, 256, image::Rgba([255, 0, 0, 255]));
        image
            .save_with_format(&path, image::ImageFormat::Png)
            .expect("save png");

        let prepared =
            prepare_sticker_for_import(path.to_str().expect("path")).expect("prepare sticker");
        assert!(prepared.converted);
        assert!(prepared.file_name.ends_with(".webp"));
        assert!(prepared.file_data.len() <= MAX_STICKER_SIZE_BYTES);
        assert_eq!(&prepared.file_data[0..4], b"RIFF");
    }

    #[test]
    fn prepare_sticker_rejects_oversized_webp() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("too-big.webp");
        std::fs::write(&path, vec![0u8; MAX_STICKER_SIZE_BYTES + 1]).expect("write");

        let err =
            prepare_sticker_for_import(path.to_str().expect("path")).expect_err("expected error");
        assert!(err.contains("exceeds 1MB"));
    }

    #[test]
    fn detect_audio_mime_accepts_supported_extensions() {
        assert_eq!(detect_audio_mime("clip.mp3"), Some("audio/mpeg"));
        assert_eq!(detect_audio_mime("clip.m4a"), Some("audio/mp4"));
        assert_eq!(detect_audio_mime("clip.wav"), Some("audio/wav"));
        assert_eq!(detect_audio_mime("clip.ogg"), Some("audio/ogg"));
        assert_eq!(detect_audio_mime("clip.webm"), Some("audio/webm"));
        assert_eq!(detect_audio_mime("clip.opus"), Some("audio/opus"));
    }

    #[test]
    fn detect_audio_mime_rejects_unsupported_extension() {
        assert_eq!(detect_audio_mime("clip.aac"), None);
        assert_eq!(detect_audio_mime("clip"), None);
    }
}
