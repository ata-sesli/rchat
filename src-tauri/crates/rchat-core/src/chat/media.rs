use crate::{
    app_state::{AppState, NetworkState},
    chat::{direct, group, temporary},
    chat_kind::{self, ChatKind},
    network::{
        command::{DirectMediaKind, NetworkCommand},
        gossip::{GroupContentType, GroupMessageEnvelope},
    },
    storage,
};
use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaKind {
    Image,
    Document,
    Video,
    Audio,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SentMediaResult {
    pub msg_id: String,
    pub file_hash: String,
    pub file_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedAttachment {
    pub file_hash: String,
    pub file_name: Option<String>,
    pub mime_type: String,
    pub bytes: Vec<u8>,
}

pub async fn send_file_from_path(
    app_state: &AppState,
    net_state: &NetworkState,
    chat_id: &str,
    kind: MediaKind,
    file_path: impl AsRef<Path>,
) -> Result<SentMediaResult> {
    let file_path = file_path.as_ref();
    let bytes = fs::read(file_path)
        .with_context(|| format!("Failed to read {}: {}", kind.noun(), file_path.display()))?;
    let file_name = file_path
        .file_name()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| kind.default_file_name().to_string());
    let mime_type = kind.detect_mime(file_path)?;
    let file_hash = {
        let conn = app_state
            .db_conn
            .lock()
            .map_err(|error| anyhow!("database lock failed: {error}"))?;
        storage::object::create(
            &conn,
            &bytes,
            Some(&file_name),
            Some(mime_type),
            None,
        )
        .with_context(|| format!("Failed to store {}", kind.noun()))?
    };

    send_stored_media(
        app_state,
        net_state,
        chat_id,
        StoredMedia {
            content_type: kind.content_type(),
            group_type: kind.group_type(),
            direct_type: kind.direct_type(),
            file_hash,
            file_name: kind.result_file_name(file_name),
            size_bytes: Some(bytes.len()),
            text_content: kind.text_content_name(file_path),
            direct_file_name: kind.direct_file_name(file_path),
        },
    )
    .await
}

pub async fn send_sticker(
    app_state: &AppState,
    net_state: &NetworkState,
    chat_id: &str,
    file_hash: &str,
) -> Result<SentMediaResult> {
    let file_name = {
        let conn = app_state
            .db_conn
            .lock()
            .map_err(|error| anyhow!("database lock failed: {error}"))?;
        if !storage::db::sticker_exists(&conn, file_hash) {
            return Err(anyhow!("Sticker not found in local library"));
        }
        let file_exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM files WHERE file_hash = ?1)",
                [file_hash],
                |row| row.get(0),
            )
            .context("Failed to check sticker file")?;
        if !file_exists {
            return Err(anyhow!("Sticker file is missing from local storage"));
        }
        conn.query_row(
            "SELECT file_name FROM files WHERE file_hash = ?1",
            [file_hash],
            |row| row.get(0),
        )
        .ok()
    };

    send_stored_media(
        app_state,
        net_state,
        chat_id,
        StoredMedia {
            content_type: "sticker",
            group_type: GroupContentType::Sticker,
            direct_type: DirectMediaKind::Sticker,
            file_hash: file_hash.to_string(),
            file_name,
            size_bytes: None,
            text_content: None,
            direct_file_name: None,
        },
    )
    .await
}

pub fn load_attachment_bytes(app_state: &AppState, file_hash: &str) -> Result<LoadedAttachment> {
    let conn = app_state
        .db_conn
        .lock()
        .map_err(|error| anyhow!("database lock failed: {error}"))?;
    let bytes = storage::object::load(&conn, file_hash, None)
        .with_context(|| format!("Failed to load attachment {file_hash}"))?;
    let (file_name, mime_type): (Option<String>, Option<String>) = conn
        .query_row(
            "SELECT file_name, mime_type FROM files WHERE file_hash = ?1",
            [file_hash],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap_or((None, None));
    Ok(LoadedAttachment {
        file_hash: file_hash.to_string(),
        file_name,
        mime_type: mime_type.unwrap_or_else(|| "application/octet-stream".to_string()),
        bytes,
    })
}

pub fn save_attachment_to_path(
    app_state: &AppState,
    file_hash: &str,
    target_path: impl AsRef<Path>,
) -> Result<()> {
    let loaded = load_attachment_bytes(app_state, file_hash)?;
    fs::write(target_path.as_ref(), loaded.bytes)
        .with_context(|| format!("Failed to save attachment to {}", target_path.as_ref().display()))
}

pub fn load_attachment_data_url(app_state: &AppState, file_hash: &str) -> Result<String> {
    let loaded = load_attachment_bytes(app_state, file_hash)?;
    Ok(data_url(&loaded.mime_type, &loaded.bytes))
}

pub fn load_image_data_url(app_state: &AppState, file_hash: &str) -> Result<String> {
    let loaded = load_attachment_bytes(app_state, file_hash)?;
    let mime_type = if loaded.mime_type.starts_with("image/") {
        loaded.mime_type
    } else {
        detect_image_mime_from_bytes(&loaded.bytes)
            .unwrap_or("image/png")
            .to_string()
    };
    update_file_mime_type(app_state, file_hash, &mime_type);
    Ok(data_url(&mime_type, &loaded.bytes))
}

pub fn load_image_path_data_url(file_path: impl AsRef<Path>) -> Result<String> {
    let file_path = file_path.as_ref();
    let data = fs::read(file_path)
        .with_context(|| format!("Failed to read image file: {}", file_path.display()))?;
    let mime_type = match file_path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .as_deref()
    {
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("png") => "image/png",
        _ => "image/png",
    };
    Ok(data_url(mime_type, &data))
}

pub fn load_video_data_url(app_state: &AppState, file_hash: &str) -> Result<String> {
    let loaded = load_attachment_bytes(app_state, file_hash)?;
    let mime_type = if loaded.mime_type.starts_with("video/") {
        loaded.mime_type
    } else {
        "video/mp4".to_string()
    };
    Ok(data_url(&mime_type, &loaded.bytes))
}

pub fn load_audio_data_url(app_state: &AppState, file_hash: &str) -> Result<String> {
    let loaded = load_attachment_bytes(app_state, file_hash)?;
    let mime_type = if loaded.mime_type.starts_with("audio/") {
        loaded.mime_type
    } else {
        detect_audio_mime_from_bytes(&loaded.bytes)
            .unwrap_or("audio/webm")
            .to_string()
    };
    update_file_mime_type(app_state, file_hash, &mime_type);
    Ok(data_url(&mime_type, &loaded.bytes))
}

fn data_url(mime_type: &str, data: &[u8]) -> String {
    format!("data:{};base64,{}", mime_type, STANDARD.encode(data))
}

fn update_file_mime_type(app_state: &AppState, file_hash: &str, mime_type: &str) {
    if let Ok(conn) = app_state.db_conn.lock() {
        let _ = conn.execute(
            "UPDATE files SET mime_type = ?2 WHERE file_hash = ?1",
            rusqlite::params![file_hash, mime_type],
        );
    }
}

pub async fn retry_direct_attachment_fetch(
    app_state: &AppState,
    net_state: &NetworkState,
    chat_id: &str,
    file_hash: &str,
) -> Result<()> {
    if file_hash.trim().is_empty() {
        return Err(anyhow!("file hash is empty"));
    }
    let canonical_chat_id = direct::canonical_direct_or_self_chat_id(app_state, chat_id).await?;
    match chat_kind::parse_chat_kind(&canonical_chat_id) {
        ChatKind::Direct | ChatKind::TemporaryDirect => {
            let target_peer_id = direct::resolve_peer_id_for_chat(&canonical_chat_id)
                .unwrap_or_else(|| canonical_chat_id.clone());
            let tx = net_state.sender.lock().await;
            tx.send(NetworkCommand::RequestDirectFileMetadata {
                target_peer_id,
                file_hash: file_hash.to_string(),
            })
            .await
            .map_err(|error| anyhow!("network command channel is closed: {error}"))?;
            Ok(())
        }
        ChatKind::SelfChat => Err(anyhow!("self chat attachments are already local")),
        ChatKind::Group => Err(anyhow!(
            "durable group attachment retry is not supported in rchat-tui yet"
        )),
        ChatKind::TemporaryGroup => {
            let session =
                temporary::validate_temp_group_session(net_state, &canonical_chat_id).await?;
            let target_peer_id = session
                .peer_id
                .ok_or_else(|| anyhow!("temporary group peer is not connected yet"))?;
            let tx = net_state.sender.lock().await;
            tx.send(NetworkCommand::RequestDirectFileMetadata {
                target_peer_id,
                file_hash: file_hash.to_string(),
            })
            .await
            .map_err(|error| anyhow!("network command channel is closed: {error}"))?;
            Ok(())
        }
        ChatKind::Archived => Err(anyhow!("archived chats are read-only")),
    }
}

struct StoredMedia {
    content_type: &'static str,
    group_type: GroupContentType,
    direct_type: DirectMediaKind,
    file_hash: String,
    file_name: Option<String>,
    size_bytes: Option<usize>,
    text_content: Option<String>,
    direct_file_name: Option<String>,
}

async fn send_stored_media(
    app_state: &AppState,
    net_state: &NetworkState,
    chat_id: &str,
    media: StoredMedia,
) -> Result<SentMediaResult> {
    let canonical_chat_id = direct::canonical_direct_or_self_chat_id(app_state, chat_id).await?;
    let chat_kind = chat_kind::parse_chat_kind(&canonical_chat_id);

    if matches!(chat_kind, ChatKind::Archived) {
        return Err(anyhow!("Archived chats are read-only"));
    }

    if matches!(chat_kind, ChatKind::Group) {
        let msg_id = group::send_group_media_reference(
            app_state,
            net_state,
            canonical_chat_id,
            media.group_type,
            media.file_hash.clone(),
            media.file_name.clone(),
            None,
        )
        .await?;
        return Ok(SentMediaResult {
            msg_id,
            file_hash: media.file_hash,
            file_name: media.file_name,
        });
    }

    let timestamp = direct::now_unix_timestamp();
    let msg_id = format!("{}-{}", timestamp, rand::random::<u32>());
    let is_temporary = matches!(
        chat_kind,
        ChatKind::TemporaryDirect | ChatKind::TemporaryGroup
    );
    let db_chat_id = if matches!(chat_kind, ChatKind::SelfChat) {
        "self".to_string()
    } else {
        canonical_chat_id.clone()
    };
    let status = media_outgoing_status(chat_kind)?;
    let message = storage::db::Message {
        id: msg_id.clone(),
        chat_id: db_chat_id.clone(),
        peer_id: "Me".to_string(),
        timestamp,
        content_type: media.content_type.to_string(),
        text_content: media.text_content.clone(),
        file_hash: Some(media.file_hash.clone()),
        status: status.to_string(),
        content_metadata: media
            .size_bytes
            .map(|size| format!("{{\"size_bytes\":{size}}}")),
        sender_alias: None,
    };

    // Enqueue the network command before storing the message so a closed
    // command channel cannot leave a phantom `delivered` message behind.
    if !matches!(chat_kind, ChatKind::SelfChat) {
        if matches!(chat_kind, ChatKind::TemporaryGroup) {
            temporary::validate_temp_group_session(net_state, &canonical_chat_id).await?;
        }
        let tx = net_state.sender.lock().await;
        match chat_kind {
            ChatKind::Direct | ChatKind::TemporaryDirect => {
                let target_peer_id = direct::resolve_peer_id_for_chat(&canonical_chat_id)
                    .unwrap_or_else(|| canonical_chat_id.clone());
                tx.send(NetworkCommand::SendDirectMedia {
                    kind: media.direct_type,
                    target_peer_id,
                    file_hash: media.file_hash.clone(),
                    file_name: media.direct_file_name.clone(),
                    msg_id: msg_id.clone(),
                    timestamp,
                })
                .await
                .map_err(|error| anyhow!("network command channel is closed: {error}"))?;
            }
            ChatKind::Group | ChatKind::TemporaryGroup => {
                let envelope = GroupMessageEnvelope {
                    id: msg_id.clone(),
                    group_id: canonical_chat_id.clone(),
                    sender_id: "Me".to_string(),
                    sender_alias: None,
                    timestamp,
                    content_type: media.group_type,
                    text_content: media.text_content.clone(),
                    file_hash: Some(media.file_hash.clone()),
                    protocol_version: None,
                    signed_record_id: None,
                };
                tx.send(NetworkCommand::PublishGroup { envelope })
                    .await
                    .map_err(|error| anyhow!("network command channel is closed: {error}"))?;
            }
            ChatKind::SelfChat | ChatKind::Archived => {}
        }
    }

    if is_temporary {
        let mut temp_state = net_state.temporary_state.lock().await;
        temp_state
            .messages
            .entry(db_chat_id.clone())
            .or_default()
            .push(message);
    } else {
        let resolved_peer_id = direct::resolve_peer_id_for_chat(&canonical_chat_id)
            .unwrap_or_else(|| canonical_chat_id.clone());
        let conn = app_state
            .db_conn
            .lock()
            .map_err(|error| anyhow!("database lock failed: {error}"))?;
        if matches!(chat_kind, ChatKind::Direct) {
            direct::ensure_direct_chat_rows(&conn, &canonical_chat_id, &resolved_peer_id)?;
        } else if matches!(chat_kind, ChatKind::Group | ChatKind::TemporaryGroup) {
            ensure_group_chat_rows(&conn, &canonical_chat_id)?;
        }
        storage::db::insert_message(&conn, &message)?;
    }

    Ok(SentMediaResult {
        msg_id,
        file_hash: media.file_hash,
        file_name: media.file_name,
    })
}

fn ensure_group_chat_rows(conn: &rusqlite::Connection, chat_id: &str) -> Result<()> {
    if !storage::db::chat_exists(conn, chat_id) {
        storage::db::upsert_chat(
            conn,
            chat_id,
            &chat_kind::default_group_name(chat_id),
            true,
        )?;
    }
    storage::db::add_chat_member(conn, chat_id, "Me", "member")?;
    Ok(())
}

fn media_outgoing_status(kind: ChatKind) -> Result<&'static str> {
    match kind {
        ChatKind::Archived => Err(anyhow!("Archived chats are read-only")),
        other => Ok(direct::outgoing_status(other)),
    }
}

impl MediaKind {
    fn content_type(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Document => "document",
            Self::Video => "video",
            Self::Audio => "audio",
        }
    }

    fn noun(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Document => "document",
            Self::Video => "video",
            Self::Audio => "audio",
        }
    }

    fn default_file_name(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Document => "document",
            Self::Video => "video.mp4",
            Self::Audio => "audio",
        }
    }

    fn direct_type(self) -> DirectMediaKind {
        match self {
            Self::Image => DirectMediaKind::Image,
            Self::Document => DirectMediaKind::Document,
            Self::Video => DirectMediaKind::Video,
            Self::Audio => DirectMediaKind::Audio,
        }
    }

    fn group_type(self) -> GroupContentType {
        match self {
            Self::Image => GroupContentType::Image,
            Self::Document => GroupContentType::Document,
            Self::Video => GroupContentType::Video,
            Self::Audio => GroupContentType::Audio,
        }
    }

    fn result_file_name(self, file_name: String) -> Option<String> {
        Some(file_name)
    }

    fn text_content_name(self, path: &Path) -> Option<String> {
        match self {
            Self::Image => None,
            Self::Document | Self::Video | Self::Audio => path
                .file_name()
                .and_then(|name| name.to_str())
                .map(ToOwned::to_owned)
                .or_else(|| Some(self.default_file_name().to_string())),
        }
    }

    fn direct_file_name(self, path: &Path) -> Option<String> {
        match self {
            Self::Image => None,
            Self::Document | Self::Video | Self::Audio => self.text_content_name(path),
        }
    }

    fn detect_mime(self, path: &Path) -> Result<&'static str> {
        let ext = path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.to_ascii_lowercase());
        Ok(match self {
            Self::Image => match ext.as_deref() {
                Some("jpg" | "jpeg") => "image/jpeg",
                Some("png") => "image/png",
                Some("gif") => "image/gif",
                Some("webp") => "image/webp",
                _ => "image/png",
            },
            Self::Document => match ext.as_deref() {
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
            },
            Self::Video => match ext.as_deref() {
                Some("mp4") => "video/mp4",
                Some("webm") => "video/webm",
                Some("mov") => "video/quicktime",
                Some("avi") => "video/x-msvideo",
                Some("mkv") => "video/x-matroska",
                _ => "video/mp4",
            },
            Self::Audio => detect_audio_mime(path).ok_or_else(|| {
                anyhow!("Unsupported audio format. Allowed: mp3, m4a, wav, ogg, webm, opus")
            })?,
        })
    }
}

fn detect_audio_mime(path: &Path) -> Option<&'static str> {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .as_deref()
    {
        Some("mp3") => Some("audio/mpeg"),
        Some("m4a") => Some("audio/mp4"),
        Some("wav") => Some("audio/wav"),
        Some("ogg") => Some("audio/ogg"),
        Some("webm") => Some("audio/webm"),
        Some("opus") => Some("audio/opus"),
        _ => None,
    }
}

pub fn detect_image_mime_from_bytes(data: &[u8]) -> Option<&'static str> {
    match image::guess_format(data).ok()? {
        image::ImageFormat::Png => Some("image/png"),
        image::ImageFormat::Jpeg => Some("image/jpeg"),
        image::ImageFormat::Gif => Some("image/gif"),
        image::ImageFormat::WebP => Some("image/webp"),
        _ => None,
    }
}

pub fn detect_audio_mime_from_bytes(data: &[u8]) -> Option<&'static str> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn now_unix_secs() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0)
    }

    #[test]
    fn audio_mime_accepts_supported_extensions() {
        assert_eq!(detect_audio_mime(Path::new("clip.mp3")), Some("audio/mpeg"));
        assert_eq!(detect_audio_mime(Path::new("clip.m4a")), Some("audio/mp4"));
        assert_eq!(detect_audio_mime(Path::new("clip.wav")), Some("audio/wav"));
        assert_eq!(detect_audio_mime(Path::new("clip.ogg")), Some("audio/ogg"));
        assert_eq!(detect_audio_mime(Path::new("clip.webm")), Some("audio/webm"));
        assert_eq!(detect_audio_mime(Path::new("clip.opus")), Some("audio/opus"));
    }

    #[test]
    fn audio_mime_rejects_unsupported_extensions() {
        assert_eq!(detect_audio_mime(Path::new("clip.aac")), None);
        assert_eq!(detect_audio_mime(Path::new("clip")), None);
    }

    #[tokio::test]
    async fn load_and_save_attachment_round_trips_bytes_and_metadata() {
        let (temp, app_state) = crate::settings::profile::test_app_state().await;
        let file_hash = {
            let conn = app_state.db_conn.lock().expect("db");
            storage::object::create(
                &conn,
                b"hello attachment",
                Some("hello.txt"),
                Some("text/plain"),
                None,
            )
            .expect("object stored")
        };

        let loaded = load_attachment_bytes(&app_state, &file_hash).expect("loaded");

        assert_eq!(loaded.bytes, b"hello attachment");
        assert_eq!(loaded.file_name.as_deref(), Some("hello.txt"));
        assert_eq!(loaded.mime_type, "text/plain");

        let target = temp.path().join("saved.txt");
        save_attachment_to_path(&app_state, &file_hash, &target).expect("saved");
        assert_eq!(std::fs::read(target).expect("saved bytes"), b"hello attachment");
    }

    #[tokio::test]
    async fn temporary_group_media_send_stores_in_temp_state_and_publishes() {
        use crate::app_state::{TemporaryChatKind, TemporaryChatSession};
        use crate::network::command::NetworkCommand;
        use crate::network::gossip::GroupContentType;

        let (_temp, app_state) = crate::testing::test_app_state().await;
        let (net_state, mut rx) = crate::testing::test_network_state();
        let chat_id = crate::chat_kind::generate_temp_group_chat_id();
        {
            let mut temp_state = net_state.temporary_state.lock().await;
            temp_state.chats.insert(
                chat_id.clone(),
                TemporaryChatSession {
                    chat_id: chat_id.clone(),
                    name: "Design Crew".to_string(),
                    kind: TemporaryChatKind::Group,
                    expires_at: now_unix_secs() + 3600,
                    peer_id: Some("12D3KooWAKrRudfV7S7XK418Jg4c8SvCkcnjwjhoATAQ1J6NAw86".to_string()),
                    archived: false,
                },
            );
        }
        let image_path = std::env::temp_dir().join("rchat-media-test.png");
        std::fs::write(&image_path, b"fake png bytes").expect("write image");

        let result = send_file_from_path(
            &app_state,
            &net_state,
            &chat_id,
            MediaKind::Image,
            &image_path,
        )
        .await
        .expect("send media");
        let _ = std::fs::remove_file(&image_path);

        let messages = net_state
            .temporary_state
            .lock()
            .await
            .messages
            .get(&chat_id)
            .cloned()
            .unwrap_or_default();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].id, result.msg_id);
        assert_eq!(messages[0].content_type, "image");
        assert_eq!(messages[0].file_hash.as_deref(), Some(result.file_hash.as_str()));
        assert_eq!(messages[0].status, "delivered");

        match rx.recv().await.expect("command") {
            NetworkCommand::PublishGroup { envelope } => {
                assert_eq!(envelope.group_id, chat_id);
                assert_eq!(envelope.sender_id, "Me");
                assert_eq!(envelope.content_type, GroupContentType::Image);
                assert_eq!(envelope.file_hash.as_deref(), Some(result.file_hash.as_str()));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[tokio::test]
    async fn temporary_group_media_send_failure_leaves_history_unchanged() {
        use crate::app_state::{TemporaryChatKind, TemporaryChatSession};

        let (_temp, app_state) = crate::testing::test_app_state().await;
        let (net_state, rx) = crate::testing::test_network_state();
        let chat_id = crate::chat_kind::generate_temp_group_chat_id();
        {
            let mut temp_state = net_state.temporary_state.lock().await;
            temp_state.chats.insert(
                chat_id.clone(),
                TemporaryChatSession {
                    chat_id: chat_id.clone(),
                    name: "Design Crew".to_string(),
                    kind: TemporaryChatKind::Group,
                    expires_at: now_unix_secs() + 3600,
                    peer_id: Some("12D3KooWAKrRudfV7S7XK418Jg4c8SvCkcnjwjhoATAQ1J6NAw86".to_string()),
                    archived: false,
                },
            );
        }
        drop(rx);
        let image_path = std::env::temp_dir().join("rchat-media-test.png");
        std::fs::write(&image_path, b"fake png bytes").expect("write image");

        let result = send_file_from_path(
            &app_state,
            &net_state,
            &chat_id,
            MediaKind::Image,
            &image_path,
        )
        .await;
        let _ = std::fs::remove_file(&image_path);
        assert!(result.is_err());

        let messages = net_state
            .temporary_state
            .lock()
            .await
            .messages
            .get(&chat_id)
            .cloned()
            .unwrap_or_default();
        assert!(
            messages.is_empty(),
            "failed media send must not leave a phantom delivered message"
        );
    }

    #[tokio::test]
    async fn data_url_helpers_return_expected_mime_prefixes() {
        let (_temp, app_state) = crate::settings::profile::test_app_state().await;
        let image_hash = {
            let conn = app_state.db_conn.lock().expect("db");
            storage::object::create(
                &conn,
                b"not a real image but stored as png",
                Some("image.png"),
                Some("image/png"),
                None,
            )
            .expect("image stored")
        };
        let audio_hash = {
            let conn = app_state.db_conn.lock().expect("db");
            storage::object::create(
                &conn,
                b"ID3 fake mp3",
                Some("clip.mp3"),
                Some("audio/mpeg"),
                None,
            )
            .expect("audio stored")
        };

        assert!(load_image_data_url(&app_state, &image_hash)
            .expect("image data url")
            .starts_with("data:image/png;base64,"));
        assert!(load_audio_data_url(&app_state, &audio_hash)
            .expect("audio data url")
            .starts_with("data:audio/mpeg;base64,"));
    }
}
