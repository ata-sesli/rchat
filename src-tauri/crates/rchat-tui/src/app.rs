use crate::{
    bridge::{TuiEvent, TuiEventSink},
    media::{
        decode_inline_media_preview, DecodedRgbaFrame, InlineMediaCache, InlineMediaKey,
        InlineMediaState, LatestFrameSlot, MediaViewerKey, ProtocolRequest, ProtocolRequestId,
        ProtocolResponse, ProtocolWorker, RemoteVideoFrameDecoder, ScreenFrameDecoder,
    },
    smoke::SmokeFrameGenerator,
    state::{
        db_chat_id, message_is_attachment, AppSessionPhase, AttachmentActionField,
        AttachmentFileEntry, AttachmentModalField, ComposerAction, ContextMenuAction,
        ContextMenuState, ContextMenuTarget, FocusPane, MediaViewerAction, MediaViewerKind,
        NewPersonField, NewPersonStep, SettingsField, SettingsPane, SettingsSection,
        StickerPickerMode, StickerPickerState, TuiAppState, TuiChat, TuiChatDetails,
        TuiEnvelope, TuiMessage, TuiSticker, TuiThemePreset,
    },
};
use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use crossterm::{
    cursor::MoveTo,
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event as CrosstermEvent, KeyCode, KeyEvent,
        KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, Clear as TerminalClear, ClearType, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};
use image::{DynamicImage, Luma};
use qrcode::QrCode;
use ratatui::{
    backend::{Backend, CrosstermBackend},
    layout::{Alignment, Constraint, Direction, Layout, Rect, Size},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
    Frame, Terminal,
};
use ratatui_image::{picker::ProtocolType, Image};
use rfd::FileDialog;
use rchat_core::{
    app_state::{
        BroadcastPhase, BroadcastState, CallKind, TemporaryChatKind, VoiceCallPhase, VoiceCallState,
    },
    chat::{
        details, direct, envelopes, group,
        media::{self as chat_media, MediaKind},
        temporary,
    },
    chat_identity,
    chat_kind::{self, ChatKind},
    events::{CoreEvent, VideoEncodedRemoteFrameEvent},
    live::broadcast::protocol::BroadcastFrameEvent,
    live::video::codec::{i420_to_rgba, VideoProfile, Vp8VideoDecoder, Vp8VideoEncoder},
    network::{command::NetworkCommand, mdns},
    oauth, runtime,
    settings::{
        connectivity as settings_connectivity, peers as settings_peers,
        profile as settings_profile, stickers as settings_stickers, theme as settings_theme,
    },
    storage,
    storage::config::ConnectivityMode,
    AppState, NetworkState,
};
use rchat_screen_capture::{ScreenCaptureConfig, ScreenCaptureProfile, ScreenCaptureSession};
#[cfg(unix)]
use std::os::fd::{AsRawFd, RawFd};
use std::{
    collections::{hash_map::DefaultHasher, HashMap, HashSet},
    fs::{self, OpenOptions},
    hash::{Hash, Hasher},
    io,
    io::Write,
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
    sync::{mpsc as std_mpsc, Arc},
    thread,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

const TUI_EVENT_BUFFER: usize = 512;
const DEFAULT_SMOKE_FPS: u32 = 10;
const DEFAULT_SMOKE_SECONDS: u64 = 20;
const DEFAULT_LOCAL_SCREEN_SECONDS: u64 = 30;
const SMOKE_WIDTH: u32 = 640;
const SMOKE_HEIGHT: u32 = 360;
const INLINE_MEDIA_CACHE_CAPACITY: usize = 24;
const INLINE_MEDIA_VISIBLE_CAP: usize = 3;
const INLINE_MEDIA_PREVIEW_HEIGHT: u16 = 10;
const INLINE_MEDIA_MIN_WIDTH: u16 = 18;
const PASSWORD_MASK_SYMBOL: &str = "•";
const NEW_PERSON_QR_WIDTH: u16 = 28;
const NEW_PERSON_QR_HEIGHT: u16 = 12;
const KITTY_DELETE_VISIBLE_PLACEMENTS: &[u8] = b"\x1b_Ga=d,q=2\x1b\\";

#[derive(Debug, Parser)]
#[command(name = "rchat-tui")]
#[command(about = "Terminal RChat client media spike")]
pub struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    MediaSmoke {
        #[arg(long, default_value_t = DEFAULT_SMOKE_FPS)]
        fps: u32,
        #[arg(long, default_value_t = DEFAULT_SMOKE_SECONDS)]
        seconds: u64,
    },
    LocalScreenSmoke {
        #[arg(long, value_parser = parse_screen_profile, default_value = "720p15")]
        profile: ScreenCaptureProfile,
        #[arg(long, value_enum, default_value = "vp8")]
        path: LocalScreenPath,
        #[arg(long, default_value_t = DEFAULT_LOCAL_SCREEN_SECONDS)]
        seconds: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum LocalScreenPath {
    Direct,
    Vp8,
}

impl LocalScreenPath {
    fn label(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Vp8 => "vp8",
        }
    }
}

fn parse_screen_profile(value: &str) -> std::result::Result<ScreenCaptureProfile, String> {
    ScreenCaptureProfile::from_label(value)
        .ok_or_else(|| "expected one of: 480p15, 480p30, 720p15, 720p30".to_string())
}

fn video_profile_for_screen_profile(profile: ScreenCaptureProfile) -> VideoProfile {
    match profile {
        ScreenCaptureProfile::P480F15 | ScreenCaptureProfile::P480F30 => VideoProfile::P480,
        ScreenCaptureProfile::P720F15 | ScreenCaptureProfile::P720F30 => VideoProfile::P720,
    }
}

fn now_unix_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn qr_dynamic_image(payload: &str) -> Result<DynamicImage> {
    let code = QrCode::new(payload.as_bytes()).context("failed to generate QR code")?;
    let image = code
        .render::<Luma<u8>>()
        .min_dimensions(256, 256)
        .quiet_zone(true)
        .build();
    Ok(DynamicImage::ImageLuma8(image))
}

fn decode_qr_payload_from_image(image: DynamicImage) -> Result<String> {
    let mut prepared = rqrr::PreparedImage::prepare(image.to_luma8());
    let grids = prepared.detect_grids();
    let Some(grid) = grids.into_iter().next() else {
        return Err(anyhow!("no QR code found in image"));
    };
    let (_meta, content) = grid
        .decode()
        .map_err(|error| anyhow!("failed to decode QR code: {error:?}"))?;
    Ok(content)
}

fn decode_qr_payload_from_image_path(path: &str) -> Result<String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("enter a QR image path first"));
    }
    let image = image::open(trimmed).with_context(|| format!("failed to open {}", trimmed))?;
    decode_qr_payload_from_image(image)
}

fn qr_inline_key(payload: &str, size: Size) -> InlineMediaKey {
    let mut hasher = DefaultHasher::new();
    payload.hash(&mut hasher);
    InlineMediaKey::new("new-person-qr", format!("{:016x}", hasher.finish()), size)
}

fn should_request_qr_protocol(
    kitty_available: bool,
    cache: &InlineMediaCache,
    payload: &str,
    size: Size,
) -> bool {
    kitty_available && !payload.is_empty() && cache.get(&qr_inline_key(payload, size)).is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_default_interactive_shell() {
        let cli = Cli::try_parse_from(["rchat-tui"]).unwrap();
        assert!(cli.command.is_none());
    }

    #[test]
    fn parses_palette_commands() {
        assert_eq!(
            parse_palette_command("refresh").unwrap(),
            PaletteCommand::Refresh
        );
        assert_eq!(
            parse_palette_command("connect peer-1").unwrap(),
            PaletteCommand::Connect {
                peer_id: "peer-1".to_string()
            }
        );
        assert_eq!(
            parse_palette_command("open peer-1").unwrap(),
            PaletteCommand::Open {
                chat_id: "peer-1".to_string()
            }
        );
        assert_eq!(
            parse_palette_command("details").unwrap(),
            PaletteCommand::Details { chat_id: None }
        );
        assert_eq!(
            parse_palette_command("info peer-1").unwrap(),
            PaletteCommand::Details {
                chat_id: Some("peer-1".to_string())
            }
        );
        assert_eq!(
            parse_palette_command("envelope create work Work Chats").unwrap(),
            PaletteCommand::EnvelopeCreate {
                id: "work".to_string(),
                name: "Work Chats".to_string()
            }
        );
        assert_eq!(
            parse_palette_command("envelope rename work Projects").unwrap(),
            PaletteCommand::EnvelopeRename {
                id: "work".to_string(),
                name: "Projects".to_string()
            }
        );
        assert_eq!(
            parse_palette_command("envelope delete work").unwrap(),
            PaletteCommand::EnvelopeDelete {
                id: "work".to_string()
            }
        );
        assert_eq!(
            parse_palette_command("move peer-1 work").unwrap(),
            PaletteCommand::MoveChat {
                chat_id: "peer-1".to_string(),
                envelope_id: Some("work".to_string())
            }
        );
        assert_eq!(
            parse_palette_command("move peer-1 root").unwrap(),
            PaletteCommand::MoveChat {
                chat_id: "peer-1".to_string(),
                envelope_id: None
            }
        );
        assert_eq!(
            parse_palette_command("attach image /tmp/a.png").unwrap(),
            PaletteCommand::Attach {
                kind: chat_media::MediaKind::Image,
                path: "/tmp/a.png".to_string()
            }
        );
        assert_eq!(
            parse_palette_command("attach document /tmp/report.pdf").unwrap(),
            PaletteCommand::Attach {
                kind: chat_media::MediaKind::Document,
                path: "/tmp/report.pdf".to_string()
            }
        );
        assert_eq!(
            parse_palette_command("attach").unwrap(),
            PaletteCommand::AttachModal
        );
        assert_eq!(
            parse_palette_command("sticker abc123").unwrap(),
            PaletteCommand::Sticker {
                file_hash: "abc123".to_string()
            }
        );
        assert_eq!(
            parse_palette_command("sticker").unwrap(),
            PaletteCommand::StickerPicker
        );
        assert_eq!(
            parse_palette_command("save abc123 /tmp/out.png").unwrap(),
            PaletteCommand::Save {
                file_hash: "abc123".to_string(),
                target_path: "/tmp/out.png".to_string()
            }
        );
        assert_eq!(
            parse_palette_command("copy-hash abc123").unwrap(),
            PaletteCommand::CopyHash {
                file_hash: "abc123".to_string()
            }
        );
        assert_eq!(
            parse_palette_command("view abc123").unwrap(),
            PaletteCommand::View {
                file_hash: "abc123".to_string()
            }
        );
        assert!(parse_palette_command("view").is_err());
        assert_eq!(
            parse_palette_command("retry abc123").unwrap(),
            PaletteCommand::Retry {
                file_hash: "abc123".to_string()
            }
        );
    }

    #[test]
    fn media_messages_get_human_labels() {
        let image = TuiMessage {
            id: "m1".to_string(),
            chat_id: "peer-1".to_string(),
            sender: "peer-1".to_string(),
            text: String::new(),
            timestamp: 1,
            status: "delivered".to_string(),
            content_type: "image".to_string(),
            file_hash: Some("abcdef1234567890".to_string()),
            content_metadata: None,
        };
        let video = TuiMessage {
            content_type: "video".to_string(),
            text: "clip.mp4".to_string(),
            content_metadata: Some("{\"size_bytes\":1536}".to_string()),
            ..image.clone()
        };

        assert_eq!(media_content_label(&image.content_type), "[image]");
        assert_eq!(media_display_name(&image), "image attachment");
        assert_eq!(
            short_hash(image.file_hash.as_deref().unwrap()),
            "abcdef123456..."
        );
        assert_eq!(media_content_label(&video.content_type), "[video]");
        assert_eq!(media_display_name(&video), "clip.mp4");
        assert_eq!(
            media_size_label(video.content_metadata.as_deref().unwrap()).as_deref(),
            Some("1.5 KB")
        );
    }

    #[test]
    fn inline_preview_only_applies_to_images_and_stickers_with_hashes() {
        let image = TuiMessage {
            id: "m1".to_string(),
            chat_id: "peer-1".to_string(),
            sender: "peer-1".to_string(),
            text: String::new(),
            timestamp: 1,
            status: "delivered".to_string(),
            content_type: "image".to_string(),
            file_hash: Some("hash-1".to_string()),
            content_metadata: None,
        };
        let sticker = TuiMessage {
            content_type: "sticker".to_string(),
            ..image.clone()
        };
        let image_without_hash = TuiMessage {
            file_hash: None,
            ..image.clone()
        };
        let video = TuiMessage {
            content_type: "video".to_string(),
            ..image.clone()
        };

        assert!(inline_preview_capable(&image));
        assert!(inline_preview_capable(&sticker));
        assert!(!inline_preview_capable(&image_without_hash));
        assert!(!inline_preview_capable(&video));
    }

    #[test]
    fn parses_local_screen_smoke_defaults() {
        let cli = Cli::try_parse_from(["rchat-tui", "local-screen-smoke"]).unwrap();

        let Some(Command::LocalScreenSmoke {
            profile,
            path,
            seconds,
        }) = cli.command
        else {
            panic!("expected local screen smoke command");
        };

        assert_eq!(profile, rchat_screen_capture::ScreenCaptureProfile::P720F15);
        assert_eq!(path, LocalScreenPath::Vp8);
        assert_eq!(seconds, 30);
    }

    #[test]
    fn parses_local_screen_smoke_options() {
        let cli = Cli::try_parse_from([
            "rchat-tui",
            "local-screen-smoke",
            "--profile",
            "480p30",
            "--path",
            "direct",
            "--seconds",
            "5",
        ])
        .unwrap();

        let Some(Command::LocalScreenSmoke {
            profile,
            path,
            seconds,
        }) = cli.command
        else {
            panic!("expected local screen smoke command");
        };

        assert_eq!(profile, rchat_screen_capture::ScreenCaptureProfile::P480F30);
        assert_eq!(path, LocalScreenPath::Direct);
        assert_eq!(seconds, 5);
    }

    #[test]
    fn auth_password_mask_preserves_length_without_revealing_text() {
        assert_eq!(mask_secret(""), "");
        assert_eq!(mask_secret("secret"), "••••••");
        assert!(!mask_secret("secret").contains('*'));
    }

    #[test]
    fn auth_git_hub_mode_uses_token_field() {
        let auth = AuthUiState::new(AuthMode::GitHubLogin);
        assert_eq!(auth.field, AuthField::Token);
        assert_eq!(auth.status, "Connect GitHub");
    }

    #[test]
    fn auth_github_token_field_is_masked() {
        assert!(auth_field_is_secret(
            AuthMode::GitHubLogin,
            AuthField::Token
        ));
    }

    #[test]
    fn auth_display_value_only_masks_secret_fields() {
        assert_eq!(auth_display_value("ghp_token", false), "ghp_token");
        assert_eq!(auth_display_value("secret", true), "••••••");
    }

    #[test]
    fn inline_previews_do_not_render_metadata_rows() {
        let message = media_message("sticker", Some("hash-1"));

        assert!(!inline_preview_renders_metadata(&message, true));
        assert!(inline_preview_renders_metadata(&message, false));
    }

    #[test]
    fn generated_qr_round_trips_through_decode_path() {
        let payload = "rchat://temp/example-payload";
        let image = qr_dynamic_image(payload).expect("qr image");

        let decoded = decode_qr_payload_from_image(image).expect("qr decodes");

        assert_eq!(decoded, payload);
    }

    #[test]
    fn non_qr_image_returns_readable_error() {
        let error = decode_qr_payload_from_image(DynamicImage::new_luma8(32, 32))
            .expect_err("blank image has no qr");

        assert!(error.to_string().contains("no QR"));
    }

    #[test]
    fn qr_protocol_requests_require_kitty_and_missing_cache_entry() {
        let key = qr_inline_key(
            "payload",
            Size::new(NEW_PERSON_QR_WIDTH, NEW_PERSON_QR_HEIGHT),
        );
        let mut cache = InlineMediaCache::new(4);

        assert!(should_request_qr_protocol(
            true,
            &cache,
            "payload",
            Size::new(NEW_PERSON_QR_WIDTH, NEW_PERSON_QR_HEIGHT)
        ));
        assert!(!should_request_qr_protocol(
            false,
            &cache,
            "payload",
            Size::new(NEW_PERSON_QR_WIDTH, NEW_PERSON_QR_HEIGHT)
        ));

        cache.insert_loading(key);
        assert!(!should_request_qr_protocol(
            true,
            &cache,
            "payload",
            Size::new(NEW_PERSON_QR_WIDTH, NEW_PERSON_QR_HEIGHT)
        ));
    }

    #[test]
    fn mouse_sidebar_hit_testing_maps_chat_rows_only() {
        let mut state = UiState::new(ProtocolType::Kitty, TuiEventSink::channel(4).0);
        state.app.replace_chats(vec![
            TuiChat {
                id: "chat-1".to_string(),
                name: "fedora".to_string(),
                latest_timestamp: 2,
                unread_count: 0,
            },
            TuiChat {
                id: "chat-2".to_string(),
                name: "mac".to_string(),
                latest_timestamp: 1,
                unread_count: 0,
            },
        ]);
        state
            .app
            .apply_local_peer_discovered(rchat_core::events::LocalPeerEvent {
                peer_id: "peer-1".to_string(),
                addresses: vec![],
            });
        let sidebar = Rect {
            x: 0,
            y: 3,
            width: 34,
            height: 20,
        };

        assert_eq!(
            sidebar_click_target(&state, sidebar, 2, 6),
            Some(MouseHitTarget::Chat(1))
        );
        assert_eq!(sidebar_click_target(&state, sidebar, 2, 9), None);
    }

    #[test]
    fn sidebar_rows_group_enveloped_chats_and_hit_test_visible_rows() {
        let mut state = UiState::new(ProtocolType::Kitty, TuiEventSink::channel(4).0);
        state.app.replace_chats(vec![
            TuiChat {
                id: "chat-root".to_string(),
                name: "Root".to_string(),
                latest_timestamp: 3,
                unread_count: 0,
            },
            TuiChat {
                id: "chat-work".to_string(),
                name: "Work".to_string(),
                latest_timestamp: 2,
                unread_count: 0,
            },
        ]);
        state.app.replace_envelopes(vec![TuiEnvelope {
            id: "env-work".to_string(),
            name: "Projects".to_string(),
            icon: Some("folder".to_string()),
        }]);
        state
            .app
            .replace_envelope_assignments(std::collections::HashMap::from([(
                "chat-work".to_string(),
                "env-work".to_string(),
            )]));

        assert_eq!(
            sidebar_rows(&state),
            vec![
                SidebarRow::Chat(0),
                SidebarRow::Envelope {
                    id: "env-work".to_string(),
                    label: "folder Projects".to_string(),
                },
                SidebarRow::Chat(1),
            ]
        );

        let sidebar = Rect {
            x: 0,
            y: 3,
            width: 34,
            height: 20,
        };
        assert_eq!(
            sidebar_click_target(&state, sidebar, 2, 5),
            Some(MouseHitTarget::Chat(0))
        );
        assert_eq!(
            sidebar_click_target(&state, sidebar, 2, 6),
            Some(MouseHitTarget::Envelope("env-work".to_string()))
        );
        assert_eq!(
            sidebar_click_target(&state, sidebar, 2, 7),
            Some(MouseHitTarget::Chat(1))
        );

        state.app.sidebar_search = "work".to_string();
        assert_eq!(
            sidebar_rows(&state),
            vec![
                SidebarRow::Envelope {
                    id: "env-work".to_string(),
                    label: "folder Projects".to_string(),
                },
                SidebarRow::Chat(1),
            ]
        );
    }

    #[test]
    fn sidebar_chat_line_marks_group_rows_without_presence() {
        let mut state = UiState::new(ProtocolType::Kitty, TuiEventSink::channel(4).0);
        state.app.replace_chats(vec![TuiChat {
            id: "group:550e8400-e29b-41d4-a716-446655440000".to_string(),
            name: "Design Crew".to_string(),
            latest_timestamp: 2,
            unread_count: 4,
        }]);
        let line = sidebar_chat_line(&state, 0, &Theme::rchat());

        let rendered = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(rendered.contains("Design Crew [group] (4)"));
        assert!(!rendered.contains("[offline]"));
    }

    #[test]
    fn sidebar_visible_rows_keep_selected_chat_in_view() {
        let mut state = UiState::new(ProtocolType::Kitty, TuiEventSink::channel(4).0);
        state.app.replace_chats(
            (0..12)
                .map(|index| TuiChat {
                    id: format!("chat-{index}"),
                    name: format!("Chat {index}"),
                    latest_timestamp: index,
                    unread_count: 0,
                })
                .collect(),
        );
        state.app.selected_chat_index = 10;

        let rows = visible_sidebar_rows(&mut state, 4);

        assert_eq!(rows.len(), 4);
        assert!(rows.contains(&SidebarRow::Chat(10)));
        assert!(state.app.sidebar_scroll_offset > 0);
    }

    #[test]
    fn media_viewer_copy_uses_move_image_wording() {
        let viewer = crate::state::MediaViewerState::new(
            "m1".to_string(),
            "hash-1".to_string(),
            "image.png".to_string(),
            "image".to_string(),
            None,
        );

        assert_eq!(media_viewer_view_label(&viewer), "zoom 100%  centered");
        assert!(media_viewer_shortcuts_label().contains("arrows move image when zoomed"));
        assert!(media_viewer_shortcuts_label().contains("0/R reset"));
        assert!(!media_viewer_shortcuts_label().contains("pan"));
    }

    #[test]
    fn new_person_shortcuts_prefer_arrow_navigation() {
        let label = new_person_shortcuts_label();

        assert!(label.contains("Up/Down"));
        assert!(!label.contains("Tab focus"));
    }

    #[test]
    fn footer_help_text_fits_terminal_width() {
        let state = UiState::new(ProtocolType::Kitty, TuiEventSink::channel(4).0);

        let compact = help_line_text(&state, 52);

        assert!(compact.chars().count() <= 52);
        assert!(compact.contains("n new"));
        assert!(compact.contains("? help"));
        assert!(!compact.contains("/ commands"));
        assert!(!compact.contains("Conversations -> Chat -> Message"));
    }

    #[test]
    fn composer_printable_keys_are_message_text_not_global_shortcuts() {
        assert_eq!(
            composer_printable_char(FocusPane::Composer, KeyCode::Char('/')),
            Some('/')
        );
        assert_eq!(
            composer_printable_char(FocusPane::Composer, KeyCode::Char('?')),
            Some('?')
        );
        assert_eq!(
            composer_printable_char(FocusPane::Composer, KeyCode::Char('n')),
            Some('n')
        );
        assert_eq!(
            composer_printable_char(FocusPane::Composer, KeyCode::Char('q')),
            Some('q')
        );
        assert_eq!(composer_printable_char(FocusPane::Chats, KeyCode::Char('/')), None);
    }

    #[test]
    fn help_overlay_copy_is_gui_first_not_raw_commands() {
        let text = help_overlay_text_lines().join("\n");

        assert!(text.contains("n: New Person"));
        assert!(text.contains("s: Settings"));
        assert!(!text.contains("command palette"));
        assert!(!text.contains("invite create"));
        assert!(!text.contains("invite redeem"));
        assert!(!text.contains("attach ["));
    }

    #[test]
    fn kitty_graphics_clear_sequence_deletes_visible_placements_silently() {
        assert_eq!(
            kitty_graphics_delete_visible_placements_sequence(),
            b"\x1b_Ga=d,q=2\x1b\\"
        );
    }

    #[test]
    fn invalidating_terminal_graphics_drops_cached_protocol_state() {
        let mut state = UiState::new(ProtocolType::Kitty, TuiEventSink::channel(4).0);
        let inline_key = InlineMediaKey::new("m1", "hash-1", Size::new(20, 8));
        let viewer_key = MediaViewerKey::new("hash-2", Size::new(80, 24), 100, 0, 0);
        state.inline_media_cache.insert_loading(inline_key.clone());
        state.last_protocol_seq = Some(12);
        state.viewer_protocol_key = Some(viewer_key);

        invalidate_terminal_graphics_protocols(&mut state);

        assert!(state.inline_media_cache.get(&inline_key).is_none());
        assert!(state.protocol.is_none());
        assert!(state.remote_video_protocol.is_none());
        assert!(state.viewer_protocol.is_none());
        assert!(state.viewer_protocol_key.is_none());
        assert!(state.last_protocol_seq.is_none());
    }

    #[test]
    fn requesting_new_viewer_protocol_keeps_current_protocol_visible() {
        let mut state = UiState::new(ProtocolType::Kitty, TuiEventSink::channel(4).0);
        let old_key = MediaViewerKey::new("hash-1", Size::new(80, 24), 100, 0, 0);
        let new_key = MediaViewerKey::new("hash-1", Size::new(80, 24), 125, 0, 0);
        let protocol = ratatui_image::picker::Picker::halfblocks()
            .new_protocol(
                DynamicImage::new_rgba8(10, 20),
                Size::new(1, 1),
                ratatui_image::Resize::Fit(None),
            )
            .expect("test protocol builds");
        state.viewer_protocol = Some(ProtocolResponse {
            id: ProtocolRequestId::Viewer(old_key.clone()),
            protocol,
        });
        state.viewer_protocol_key = Some(old_key);

        request_viewer_protocol_key(&mut state, new_key.clone());

        assert!(state.viewer_protocol.is_some());
        assert_eq!(state.viewer_protocol_key, Some(new_key));
    }

    #[test]
    fn external_terminal_clear_forces_next_frame_to_repaint_even_if_content_matches() {
        let backend = ratatui::backend::TestBackend::new(5, 1);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| {
                frame.render_widget(Paragraph::new("hello"), frame.area());
            })
            .unwrap();
        terminal.backend().assert_buffer_lines(["hello"]);

        terminal.backend_mut().clear().unwrap();
        terminal.backend().assert_buffer_lines(["     "]);

        force_full_redraw_after_external_clear(&mut terminal);
        terminal
            .draw(|frame| {
                frame.render_widget(Paragraph::new("hello"), frame.area());
            })
            .unwrap();

        terminal.backend().assert_buffer_lines(["hello"]);
    }

    #[test]
    fn footer_help_text_uses_modal_context() {
        let mut state = UiState::new(ProtocolType::Kitty, TuiEventSink::channel(4).0);
        state.app.open_new_person();

        let compact = help_line_text(&state, 44);

        assert!(compact.contains("New Person"));
        assert!(compact.contains("Up/Down move"));
        assert!(!compact.contains("n new"));
        assert!(compact.chars().count() <= 44);
    }

    #[test]
    fn background_kitty_media_is_suppressed_while_modal_overlay_is_open() {
        let mut state = UiState::new(ProtocolType::Kitty, TuiEventSink::channel(4).0);

        assert!(background_kitty_media_enabled(&state, true));

        state.app.open_new_person();

        assert!(!background_kitty_media_enabled(&state, true));
    }

    #[test]
    fn overlay_transitions_do_not_request_full_terminal_clear() {
        assert_eq!(
            graphics_transition_action(false, true, true),
            GraphicsTransitionAction {
                clear_kitty_graphics: true,
                invalidate_protocols: true,
                clear_terminal: false,
            }
        );
        assert_eq!(
            graphics_transition_action(true, false, true),
            GraphicsTransitionAction {
                clear_kitty_graphics: true,
                invalidate_protocols: true,
                clear_terminal: false,
            }
        );
        assert_eq!(
            graphics_transition_action(false, true, false),
            GraphicsTransitionAction {
                clear_kitty_graphics: false,
                invalidate_protocols: false,
                clear_terminal: false,
            }
        );
    }

    #[test]
    fn overlay_active_dims_background_theme_without_dimming_modal_theme() {
        let mut state = UiState::new(ProtocolType::Kitty, TuiEventSink::channel(4).0);
        let normal = Theme::rchat();

        assert_eq!(app_background_theme(&state), normal);
        assert_eq!(modal_overlay_theme(), normal);

        state.app.open_new_person();
        let dimmed = app_background_theme(&state);

        assert_ne!(dimmed, normal);
        assert_eq!(dimmed.bg, Color::Rgb(8, 10, 14));
        assert_eq!(modal_overlay_theme(), normal);
    }

    #[test]
    fn graphics_clear_generation_changes_when_modal_overlay_opens_or_closes() {
        let mut state = UiState::new(ProtocolType::Kitty, TuiEventSink::channel(4).0);
        let initial = graphics_clear_generation(&state);

        state.app.open_new_person();
        let with_modal = graphics_clear_generation(&state);

        state.app.close_new_person();
        let closed = graphics_clear_generation(&state);

        assert_ne!(initial, with_modal);
        assert_eq!(initial, closed);
    }

    #[test]
    fn group_chat_summary_preserves_activity_and_unread_counts() {
        let item = storage::db::ChatListItem {
            id: "group:550e8400-e29b-41d4-a716-446655440000".to_string(),
            name: "Design Crew".to_string(),
            is_group: true,
        };
        let latest_times = std::collections::HashMap::from([(item.id.clone(), 123)]);
        let unread_counts = std::collections::HashMap::from([(item.id.clone(), 2)]);

        let summary = group_summary_from_item(item, &latest_times, &unread_counts)
            .expect("group row should produce a summary");

        assert_eq!(summary.id, "group:550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(summary.name, "Design Crew");
        assert_eq!(summary.latest_timestamp, 123);
        assert_eq!(summary.unread_count, 2);
    }

    #[test]
    fn visible_message_plans_scroll_by_whole_messages() {
        let messages = vec![
            media_message("text", None),
            TuiMessage {
                id: "m2".to_string(),
                text: "middle".to_string(),
                ..media_message("text", None)
            },
            TuiMessage {
                id: "m3".to_string(),
                text: "newest".to_string(),
                ..media_message("text", None)
            },
        ];

        let plans = visible_message_plans(&messages, 80, 6, false, 1);

        assert_eq!(
            plans
                .iter()
                .map(|plan| plan.message.id.as_str())
                .collect::<Vec<_>>(),
            vec!["m1", "m2"]
        );
        assert!(plans.iter().all(|plan| plan.clip_top == 0));
        assert_eq!(plans.last().map(|plan| plan.visible_height), Some(3));
    }

    #[test]
    fn visible_message_plans_bottom_anchor_newest_messages() {
        let messages = vec![
            media_message("text", None),
            TuiMessage {
                id: "m2".to_string(),
                text: "middle".to_string(),
                ..media_message("text", None)
            },
            TuiMessage {
                id: "m3".to_string(),
                text: "newest".to_string(),
                ..media_message("text", None)
            },
        ];

        let plans = visible_message_plans(&messages, 80, 6, false, 0);

        assert_eq!(
            plans
                .iter()
                .map(|plan| plan.message.id.as_str())
                .collect::<Vec<_>>(),
            vec!["m2", "m3"]
        );
        assert_eq!(plans.first().map(|plan| plan.y_offset), Some(0));
        assert_eq!(plans.last().map(|plan| plan.y_offset), Some(3));
    }

    #[test]
    fn visible_message_plans_clip_older_preview_instead_of_leaving_top_blank() {
        let messages = vec![
            TuiMessage {
                id: "older".to_string(),
                ..media_message("image", Some("hash-older"))
            },
            TuiMessage {
                id: "middle".to_string(),
                ..media_message("image", Some("hash-middle"))
            },
            TuiMessage {
                id: "newest".to_string(),
                ..media_message("image", Some("hash-newest"))
            },
        ];

        let plans = visible_message_plans(&messages, 80, 15, true, 0);

        assert_eq!(
            plans
                .iter()
                .map(|plan| plan.message.id.as_str())
                .collect::<Vec<_>>(),
            vec!["middle", "newest"]
        );
        assert_eq!(plans.first().map(|plan| plan.y_offset), Some(0));
        assert_eq!(plans.first().map(|plan| plan.visible_height), Some(3));
        assert_eq!(plans.first().map(|plan| plan.clip_top), Some(9));
        assert_eq!(plans.last().map(|plan| plan.y_offset), Some(3));
        assert_eq!(plans.last().map(|plan| plan.visible_height), Some(12));
        assert_eq!(plans.last().map(|plan| plan.clip_top), Some(0));
    }

    fn synthetic_i420(width: u32, height: u32) -> Vec<u8> {
        let y_len = (width * height) as usize;
        let uv_len = y_len / 4;
        let mut data = vec![96_u8; y_len];
        data.extend(std::iter::repeat(128_u8).take(uv_len));
        data.extend(std::iter::repeat(128_u8).take(uv_len));
        data
    }

    fn media_message(content_type: &str, file_hash: Option<&str>) -> TuiMessage {
        TuiMessage {
            id: "m1".to_string(),
            chat_id: "peer-1".to_string(),
            sender: "Me".to_string(),
            text: String::new(),
            timestamp: 1,
            status: "read".to_string(),
            content_type: content_type.to_string(),
            file_hash: file_hash.map(ToOwned::to_owned),
            content_metadata: None,
        }
    }

    fn screen_frame(width: u32, height: u32) -> rchat_screen_capture::I420ScreenFrame {
        rchat_screen_capture::I420ScreenFrame {
            timestamp_us: 123,
            width,
            height,
            data: synthetic_i420(width, height),
        }
    }

    #[test]
    fn local_screen_direct_path_converts_i420_to_rgba() {
        let mut encoder = None;
        let mut decoder = None;

        let (frame, encoded_count) = local_screen_frame_to_rgba(
            LocalScreenPath::Direct,
            ScreenCaptureProfile::P480F15,
            &mut encoder,
            &mut decoder,
            7,
            screen_frame(16, 16),
        )
        .unwrap()
        .unwrap();

        assert_eq!(frame.seq, 7);
        assert_eq!(frame.width, 16);
        assert_eq!(frame.height, 16);
        assert_eq!(frame.rgba.len(), 16 * 16 * 4);
        assert_eq!(encoded_count, 0);
    }

    #[test]
    fn local_screen_vp8_path_round_trips_i420_to_rgba() {
        let mut encoder = Some(
            Vp8VideoEncoder::new_with_dimensions(
                video_profile_for_screen_profile(ScreenCaptureProfile::P480F15),
                16,
                16,
            )
            .unwrap(),
        );
        let mut decoder = Some(Vp8VideoDecoder::new().unwrap());

        let (frame, encoded_count) = local_screen_frame_to_rgba(
            LocalScreenPath::Vp8,
            ScreenCaptureProfile::P480F15,
            &mut encoder,
            &mut decoder,
            0,
            screen_frame(16, 16),
        )
        .unwrap()
        .unwrap();

        assert_eq!(frame.seq, 0);
        assert_eq!(frame.width, 16);
        assert_eq!(frame.height, 16);
        assert_eq!(frame.rgba.len(), 16 * 16 * 4);
        assert!(encoded_count > 0);
    }

    #[test]
    fn parses_voice_call_palette_commands() {
        assert_eq!(
            parse_palette_command("voice start peer-1").unwrap(),
            PaletteCommand::VoiceCallStart {
                peer_id: Some("peer-1".to_string())
            }
        );
        assert_eq!(
            parse_palette_command("voice accept").unwrap(),
            PaletteCommand::VoiceCallAccept { call_id: None }
        );
        assert_eq!(
            parse_palette_command("voice reject call-1").unwrap(),
            PaletteCommand::VoiceCallReject {
                call_id: Some("call-1".to_string())
            }
        );
        assert_eq!(
            parse_palette_command("voice end").unwrap(),
            PaletteCommand::VoiceCallEnd { call_id: None }
        );
        assert_eq!(
            parse_palette_command("voice mute off call-1").unwrap(),
            PaletteCommand::VoiceCallMute {
                call_id: Some("call-1".to_string()),
                muted: false
            }
        );
    }

    #[test]
    fn voice_call_status_labels_current_call_controls() {
        let mut state = UiState::new(ProtocolType::Halfblocks, TuiEventSink::channel(1).0);
        apply_voice_call_state(
            &mut state,
            VoiceCallState {
                phase: VoiceCallPhase::IncomingRinging,
                call_kind: Some(CallKind::Voice),
                call_id: Some("call-1".to_string()),
                peer_id: Some("peer-1".to_string()),
                started_at: None,
                ring_expires_at: None,
                muted: false,
                camera_enabled: true,
                reason: None,
            },
        );

        let status = voice_call_status_label(&state);

        assert_eq!(state.voice_call_state.call_id.as_deref(), Some("call-1"));
        assert_eq!(
            status,
            Some("voice incoming peer-1 | use incoming call controls".to_string())
        );
    }

    #[test]
    fn parses_video_call_palette_commands() {
        assert_eq!(
            parse_palette_command("video start peer-1").unwrap(),
            PaletteCommand::VideoCallStart {
                peer_id: Some("peer-1".to_string())
            }
        );
        assert_eq!(
            parse_palette_command("video accept").unwrap(),
            PaletteCommand::VideoCallAccept { call_id: None }
        );
        assert_eq!(
            parse_palette_command("video reject call-1").unwrap(),
            PaletteCommand::VideoCallReject {
                call_id: Some("call-1".to_string())
            }
        );
        assert_eq!(
            parse_palette_command("video end").unwrap(),
            PaletteCommand::VideoCallEnd { call_id: None }
        );
        assert_eq!(
            parse_palette_command("video mute on call-1").unwrap(),
            PaletteCommand::VideoCallMute {
                call_id: Some("call-1".to_string()),
                muted: true
            }
        );
        assert_eq!(
            parse_palette_command("video camera off call-1").unwrap(),
            PaletteCommand::VideoCallCamera {
                call_id: Some("call-1".to_string()),
                enabled: false
            }
        );
    }

    #[test]
    fn parses_screen_share_palette_commands_with_profiles() {
        assert_eq!(
            parse_palette_command("screen start").unwrap(),
            PaletteCommand::ScreenShareStart {
                chat_id: None,
                profile: ScreenCaptureProfile::P720F15
            }
        );
        assert_eq!(
            parse_palette_command("screen start 480p30 peer-1").unwrap(),
            PaletteCommand::ScreenShareStart {
                chat_id: Some("peer-1".to_string()),
                profile: ScreenCaptureProfile::P480F30
            }
        );
        assert_eq!(
            parse_palette_command("screen accept").unwrap(),
            PaletteCommand::ScreenShareAccept { session_id: None }
        );
        assert_eq!(
            parse_palette_command("screen reject session-1").unwrap(),
            PaletteCommand::ScreenShareReject {
                session_id: Some("session-1".to_string())
            }
        );
        assert_eq!(
            parse_palette_command("screen end").unwrap(),
            PaletteCommand::ScreenShareEnd { session_id: None }
        );
    }

    #[test]
    fn parses_group_invite_palette_commands() {
        assert_eq!(
            parse_palette_command("group-invite accept invite-1").unwrap(),
            PaletteCommand::GroupInviteAccept {
                invite_id: "invite-1".to_string()
            }
        );
        assert_eq!(
            parse_palette_command("group-invite reject invite-2").unwrap(),
            PaletteCommand::GroupInviteReject {
                invite_id: "invite-2".to_string()
            }
        );
        assert!(parse_palette_command("group-invite accept").is_err());
    }

    #[test]
    fn incoming_group_invite_status_is_gui_first() {
        let event = rchat_core::events::GroupInviteReceivedEvent {
            invite_id: "invite-1".to_string(),
            group_id: "group:550e8400-e29b-41d4-a716-446655440000".to_string(),
            group_name: "Design Crew".to_string(),
            inviter_peer_id: "peer-1".to_string(),
        };

        let status = group_invite_received_status(&event);

        assert!(status.contains("group invite Design Crew from peer-1"));
        assert!(status.contains("New Person"));
        assert!(!status.contains("/group-invite"));
    }

    #[test]
    fn screen_share_status_labels_current_broadcast_controls() {
        let mut state = UiState::new(ProtocolType::Halfblocks, TuiEventSink::channel(1).0);
        apply_broadcast_state(
            &mut state,
            &mut ScreenFrameDecoder::default(),
            BroadcastState {
                phase: BroadcastPhase::Active,
                session_id: Some("session-1".to_string()),
                peer_id: Some("peer-1".to_string()),
                started_at: Some(1_700_000_000),
                ring_expires_at: None,
                is_host: true,
                reason: None,
            },
        );

        assert_eq!(
            screen_share_status_label(&state),
            Some("sharing screen with peer-1 | e end".to_string())
        );
    }

    #[test]
    fn incoming_screen_share_prompt_summarizes_peer_and_actions() {
        let state = UiState {
            broadcast_state: BroadcastState {
                phase: BroadcastPhase::IncomingRinging,
                session_id: Some("session-1".to_string()),
                peer_id: Some("peer-1".to_string()),
                started_at: None,
                ring_expires_at: Some(1_700_000_030),
                is_host: false,
                reason: None,
            },
            ..UiState::new(ProtocolType::Halfblocks, TuiEventSink::channel(1).0)
        };

        let prompt = incoming_screen_share_prompt_summary(&state).expect("prompt visible");

        assert_eq!(prompt.title, "Incoming screen share");
        assert!(prompt.body.contains("peer-1"));
        assert!(prompt.actions.contains("a accept"));
        assert!(prompt.actions.contains("r reject"));
        assert!(!prompt.actions.contains("/ screen"));
    }

    #[test]
    fn video_call_status_labels_current_call_controls() {
        let mut state = UiState::new(ProtocolType::Halfblocks, TuiEventSink::channel(1).0);
        apply_voice_call_state(
            &mut state,
            VoiceCallState {
                phase: VoiceCallPhase::Active,
                call_kind: Some(CallKind::Video),
                call_id: Some("call-1".to_string()),
                peer_id: Some("peer-1".to_string()),
                started_at: Some(1_700_000_000),
                ring_expires_at: None,
                muted: true,
                camera_enabled: false,
                reason: None,
            },
        );

        let status = voice_call_status_label(&state);

        assert_eq!(
            status,
            Some("video active peer-1 muted camera off | Actions: Video ends".to_string())
        );
    }

    const REMOTE_PEER_ID: &str =
        "12D3KooWAKrRudfV7S7XK418Jg4c8SvCkcnjwjhoATAQ1J6NAw86";

    async fn temp_group_tui_state(
        chat_id: &str,
        messages: Vec<rchat_core::storage::db::Message>,
    ) -> (
        tempfile::TempDir,
        rchat_core::AppState,
        rchat_core::NetworkState,
        tokio::sync::mpsc::Receiver<NetworkCommand>,
        UiState,
    ) {
        let (temp, app_state) = rchat_core::testing::test_app_state().await;
        let (network_state, rx) = rchat_core::testing::test_network_state();
        {
            let mut temp_state = network_state.temporary_state.lock().await;
            temp_state.chats.insert(
                chat_id.to_string(),
                rchat_core::app_state::TemporaryChatSession {
                    chat_id: chat_id.to_string(),
                    name: "Design Crew".to_string(),
                    kind: rchat_core::app_state::TemporaryChatKind::Group,
                    expires_at: now_unix_secs() + 3600,
                    peer_id: Some(REMOTE_PEER_ID.to_string()),
                    members: Vec::new(),
                    member_op_winners: std::collections::HashMap::new(),
                    next_member_op_counter: 0,
                    archived: false,
                    pending_send_count: 0,
                },
            );
            if !messages.is_empty() {
                temp_state.messages.insert(chat_id.to_string(), messages);
            }
        }
        let mut state = UiState::new(ProtocolType::Kitty, TuiEventSink::channel(4).0);
        state.app.replace_chats(vec![TuiChat {
            id: chat_id.to_string(),
            name: "Design Crew".to_string(),
            latest_timestamp: 1_700_000_000,
            unread_count: 0,
        }]);
        (temp, app_state, network_state, rx, state)
    }

    fn now_unix_secs() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0)
    }

    fn temp_group_db_message(
        chat_id: &str,
        id: &str,
        text: &str,
    ) -> rchat_core::storage::db::Message {
        rchat_core::storage::db::Message {
            id: id.to_string(),
            chat_id: chat_id.to_string(),
            peer_id: REMOTE_PEER_ID.to_string(),
            timestamp: 1_700_000_000,
            content_type: "text".to_string(),
            text_content: Some(text.to_string()),
            file_hash: None,
            status: "delivered".to_string(),
            content_metadata: None,
            sender_alias: None,
        }
    }

    #[tokio::test]
    async fn temporary_group_activation_loads_history_and_removes_placeholder() {
        let chat_id = rchat_core::chat_kind::generate_temp_group_chat_id();
        let (_temp, app_state, network_state, _rx, mut state) = temp_group_tui_state(
            &chat_id,
            vec![
                temp_group_db_message(&chat_id, "m1", "first"),
                temp_group_db_message(&chat_id, "m2", "second"),
            ],
        )
        .await;

        open_chat_list_item(&app_state, &network_state, &mut state, &chat_id)
            .await
            .expect("open temporary group");

        assert_eq!(state.app.active_chat_id.as_deref(), Some(chat_id.as_str()));
        assert_eq!(state.app.messages.len(), 2);
        assert_eq!(state.app.messages[0].text, "first");
        assert_eq!(state.app.messages[1].text, "second");
        assert!(!state.app.status.contains("not implemented"));

        let stored = network_state
            .temporary_state
            .lock()
            .await
            .messages
            .get(&chat_id)
            .cloned()
            .unwrap_or_default();
        assert_eq!(stored.len(), 2);
        assert!(stored.iter().all(|message| message.status == "read"));
    }

    #[tokio::test]
    async fn temporary_group_composer_routes_through_group_send_path() {
        let chat_id = rchat_core::chat_kind::generate_temp_group_chat_id();
        let (_temp, app_state, network_state, mut rx, mut state) =
            temp_group_tui_state(&chat_id, vec![]).await;
        open_chat_list_item(&app_state, &network_state, &mut state, &chat_id)
            .await
            .expect("open temporary group");
        state.app.composer = "hello group".to_string();

        send_composer(&app_state, &network_state, &mut state)
            .await
            .expect("send composer");

        let messages = network_state
            .temporary_state
            .lock()
            .await
            .messages
            .get(&chat_id)
            .cloned()
            .unwrap_or_default();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].peer_id, "Me");
        assert_eq!(messages[0].text_content.as_deref(), Some("hello group"));
        assert_eq!(messages[0].status, "delivered");

        match rx.recv().await.expect("command") {
            NetworkCommand::PublishGroup { envelope } => {
                assert_eq!(envelope.group_id, chat_id);
                assert_eq!(envelope.sender_id, "Me");
                assert_eq!(
                    envelope.content_type,
                    rchat_core::network::gossip::GroupContentType::Text
                );
                assert_eq!(envelope.text_content.as_deref(), Some("hello group"));
            }
            other => panic!("unexpected command: {other:?}"),
        }

        assert!(state.app.composer.is_empty());
        assert_eq!(state.app.messages.len(), 1);
        assert_eq!(state.app.messages[0].text, "hello group");
        assert_eq!(state.app.messages[0].status, "delivered");
        assert_eq!(state.app.status, "message sent");
    }

    #[tokio::test]
    async fn temporary_group_media_send_via_tui_path() {
        let chat_id = rchat_core::chat_kind::generate_temp_group_chat_id();
        let (_temp, app_state, network_state, mut rx, mut state) =
            temp_group_tui_state(&chat_id, vec![]).await;
        open_chat_list_item(&app_state, &network_state, &mut state, &chat_id)
            .await
            .expect("open temporary group");
        let image_path = std::env::temp_dir().join("rchat-tui-media-test.png");
        std::fs::write(&image_path, b"fake png bytes").expect("write image");

        send_attachment_from_path(
            &app_state,
            &network_state,
            &mut state,
            chat_media::MediaKind::Image,
            image_path.to_str().expect("utf8 path"),
        )
        .await
        .expect("send media");
        let _ = std::fs::remove_file(&image_path);

        let messages = network_state
            .temporary_state
            .lock()
            .await
            .messages
            .get(&chat_id)
            .cloned()
            .unwrap_or_default();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content_type, "image");
        assert!(messages[0].file_hash.is_some());

        match rx.recv().await.expect("command") {
            NetworkCommand::PublishGroup { envelope } => {
                assert_eq!(
                    envelope.content_type,
                    rchat_core::network::gossip::GroupContentType::Image
                );
                assert!(envelope.file_hash.is_some());
            }
            other => panic!("unexpected command: {other:?}"),
        }
        assert_eq!(state.app.status, "sent image");
    }

    #[tokio::test]
    async fn temporary_group_send_failure_retains_composer_draft() {
        let chat_id = rchat_core::chat_kind::generate_temp_group_chat_id();
        let (_temp, app_state, network_state, rx, mut state) =
            temp_group_tui_state(&chat_id, vec![]).await;
        open_chat_list_item(&app_state, &network_state, &mut state, &chat_id)
            .await
            .expect("open temporary group");
        drop(rx);
        state.app.composer = "important draft".to_string();

        send_composer(&app_state, &network_state, &mut state)
            .await
            .expect("send composer returns");

        assert_eq!(state.app.composer, "important draft");
        assert_eq!(state.app.status, "send failed");
        assert!(state.app.last_error.is_some());
        assert!(state
            .app
            .last_error
            .as_deref()
            .is_some_and(|error| error.contains("channel")));

        let messages = network_state
            .temporary_state
            .lock()
            .await
            .messages
            .get(&chat_id)
            .cloned()
            .unwrap_or_default();
        assert!(
            messages.is_empty(),
            "failed send must not leave a phantom delivered message"
        );
    }
}

pub fn run() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(io::stderr)
        .try_init()
        .ok();

    let cli = Cli::parse();
    let runtime = tokio::runtime::Runtime::new().context("failed to start tokio runtime")?;
    runtime.block_on(async move {
        match cli.command {
            Some(Command::MediaSmoke { fps, seconds }) => run_media_smoke(fps, seconds).await,
            Some(Command::LocalScreenSmoke {
                profile,
                path,
                seconds,
            }) => run_local_screen_smoke(profile, path, seconds).await,
            None => run_interactive().await,
        }
    })
}

async fn run_interactive() -> Result<()> {
    let app_dir = runtime::default_app_data_dir()?;
    let app_state = runtime::create_app_state(app_dir)?;
    let tui_log_path = runtime::default_app_data_dir()?.join("tui.log");
    let mut terminal = TerminalSession::enter()?;

    if !run_auth_screen(&app_state, &mut terminal).await? {
        return Ok(());
    }

    let picker = ratatui_image::picker::Picker::from_query_stdio()
        .unwrap_or_else(|_| ratatui_image::picker::Picker::halfblocks());
    let protocol_type = picker.protocol_type();
    let kitty_available = protocol_type == ProtocolType::Kitty;
    let protocol_worker = kitty_available.then(|| ProtocolWorker::spawn(picker));
    let inline_loader = kitty_available.then(|| InlineMediaLoader::spawn(app_state.clone()));
    let _output_redirect = OutputRedirect::redirect(&tui_log_path)?;
    let (event_sink, mut event_rx) = TuiEventSink::channel(TUI_EVENT_BUFFER);
    let network_state = rchat_core::network::start(app_state.clone(), Arc::new(event_sink.clone()))
        .await
        .context("failed to start rchat-core network")?;

    let mut state = UiState::new(protocol_type, event_sink);
    state.app.session_phase = AppSessionPhase::Unlocked;
    state.app.app_ready = true;
    state.app.status = "network running".to_string();
    state.status = "logs redirected to tui.log".to_string();
    refresh_direct_chats(&app_state, &network_state, &mut state).await?;
    if let Some(chat_id) = state.app.selected_chat_id().map(ToOwned::to_owned) {
        open_direct_chat(&app_state, &network_state, &mut state, &chat_id).await?;
    }
    let mut pending_frames = LatestFrameSlot::<BroadcastFrameEvent>::default();
    let mut pending_remote_video_frames =
        LatestFrameSlot::<VideoEncodedRemoteFrameEvent>::default();
    let mut decoder = ScreenFrameDecoder::default();
    let mut remote_video_decoder = RemoteVideoFrameDecoder::default();
    let mut last_graphics_clear_generation = graphics_clear_generation(&state);

    loop {
        let mut refresh_requested = false;
        let mut mark_read_chat_ids = Vec::new();
        drain_core_events(
            &mut event_rx,
            &mut state,
            &mut pending_frames,
            &mut pending_remote_video_frames,
            &mut decoder,
            &mut remote_video_decoder,
            &mut refresh_requested,
            &mut mark_read_chat_ids,
        );

        if refresh_requested {
            let active_chat_id = state.app.active_chat_id.clone();
            refresh_direct_chats(&app_state, &network_state, &mut state).await?;
            if let Some(chat_id) = active_chat_id {
                let _ = open_direct_chat(&app_state, &network_state, &mut state, &chat_id).await;
            }
        }
        for chat_id in mark_read_chat_ids {
            let _ = direct::mark_direct_messages_read(&app_state, &network_state, &chat_id).await;
        }

        state.pending_frame_drops = pending_frames.dropped_frames();
        state.decoder_dropped_delta = decoder.dropped_delta_before_keyframe();
        state.decoder_errors = decoder.decode_errors();
        state.remote_video_pending_drops = pending_remote_video_frames.dropped_frames();
        state.remote_video_decoder_errors = remote_video_decoder.decode_errors();
        state.remote_video_delta_drops = remote_video_decoder.dropped_delta_before_keyframe();

        if let Some(worker) = protocol_worker.as_ref() {
            if let Some(event) = pending_frames.take() {
                if let Some(frame) = decoder.decode_event(&event) {
                    state.decoded_frames = state.decoded_frames.saturating_add(1);
                    match ProtocolRequest::screen(frame, state.media_size) {
                        Ok(request) => worker.request(request),
                        Err(error) => state.media_error = Some(error.to_string()),
                    }
                }
            }
            if let Some(event) = pending_remote_video_frames.take() {
                if let Some(frame) = remote_video_decoder.decode_event(&event) {
                    state.remote_video_decoded_frames =
                        state.remote_video_decoded_frames.saturating_add(1);
                    match ProtocolRequest::remote_video(frame, state.remote_video_size) {
                        Ok(request) => worker.request(request),
                        Err(error) => state.remote_video_error = Some(error.to_string()),
                    }
                }
            }
        }

        if let Some(loader) = inline_loader.as_ref() {
            for response in loader.try_recv_all() {
                match response.result {
                    Ok(image) => {
                        if let Some(worker) = protocol_worker.as_ref() {
                            worker.request(ProtocolRequest::inline(
                                response.key.clone(),
                                image,
                                response.key.size,
                            ));
                        } else {
                            state
                                .inline_media_cache
                                .insert_error(response.key, "inline preview unavailable");
                        }
                    }
                    Err(error) => state.inline_media_cache.insert_error(response.key, error),
                }
            }
        }

        if let Some(worker) = protocol_worker.as_ref() {
            request_new_person_qr_protocol(&mut state, worker, kitty_available);
            for response in worker.try_recv_all() {
                match response {
                    Ok(protocol) => match protocol.id.clone() {
                        ProtocolRequestId::Screen(seq) => {
                            state.last_protocol_seq = Some(seq);
                            state.protocol = Some(protocol);
                            state.media_error = None;
                        }
                        ProtocolRequestId::RemoteVideo { call_id, .. } => {
                            if state.voice_call_state.call_id.as_deref() == Some(call_id.as_str()) {
                                state.remote_video_protocol = Some(protocol);
                                state.remote_video_error = None;
                            }
                        }
                        ProtocolRequestId::Inline(key) => {
                            state
                                .inline_media_cache
                                .insert_ready(key, protocol.protocol);
                        }
                        ProtocolRequestId::Viewer(key) => {
                            state.viewer_protocol_key = Some(key);
                            state.viewer_protocol = Some(protocol);
                            if let Some(viewer) = state.app.media_viewer.as_mut() {
                                viewer.error = None;
                            }
                        }
                    },
                    Err(error) => match error.id {
                        ProtocolRequestId::Screen(_) => state.media_error = Some(error.message),
                        ProtocolRequestId::RemoteVideo { .. } => {
                            state.remote_video_protocol = None;
                            state.remote_video_error = Some(error.message);
                        }
                        ProtocolRequestId::Inline(key) => {
                            state.inline_media_cache.insert_error(key, error.message);
                        }
                        ProtocolRequestId::Viewer(_) => {
                            if let Some(viewer) = state.app.media_viewer.as_mut() {
                                viewer.error = Some(error.message);
                            }
                        }
                    },
                }
            }
        }

        while event::poll(Duration::from_millis(1))? {
            match event::read()? {
                CrosstermEvent::Key(key) => {
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }
                    if handle_interactive_key(&app_state, &network_state, &mut state, key.code)
                        .await?
                    {
                        return Ok(());
                    }
                }
                CrosstermEvent::Mouse(mouse) => {
                    let size = terminal.size()?;
                    handle_mouse_event(&app_state, &network_state, &mut state, mouse, size).await?;
                }
                _ => {}
            }
        }

        let graphics_clear_generation = graphics_clear_generation(&state);
        if graphics_clear_generation != last_graphics_clear_generation {
            let transition = graphics_transition_action(
                last_graphics_clear_generation,
                graphics_clear_generation,
                kitty_available,
            );
            if transition.clear_kitty_graphics {
                terminal.clear_terminal_graphics()?;
            }
            if transition.invalidate_protocols {
                invalidate_terminal_graphics_protocols(&mut state);
            }
            if transition.clear_terminal {
                terminal.clear()?;
            }
            last_graphics_clear_generation = graphics_clear_generation;
        }

        terminal.draw(|frame| {
            render_app_shell(
                frame,
                &mut state,
                inline_loader.as_ref(),
                protocol_worker.as_ref(),
                kitty_available,
            )
        })?;
        tokio::time::sleep(Duration::from_millis(16)).await;
    }
}

async fn run_media_smoke(fps: u32, seconds: u64) -> Result<()> {
    if fps == 0 {
        return Err(anyhow!("fps must be greater than zero"));
    }

    let mut terminal = TerminalSession::enter()?;
    let picker = ratatui_image::picker::Picker::from_query_stdio()
        .unwrap_or_else(|_| ratatui_image::picker::Picker::halfblocks());
    let protocol_type = picker.protocol_type();
    let kitty_available = protocol_type == ProtocolType::Kitty;
    let protocol_worker = kitty_available.then(|| ProtocolWorker::spawn(picker));
    let mut state = UiState::new(protocol_type, TuiEventSink::channel(1).0);
    state.status = "media smoke".to_string();

    let mut generator = SmokeFrameGenerator::new(SMOKE_WIDTH, SMOKE_HEIGHT);
    let frame_interval = Duration::from_millis(1_000 / u64::from(fps));
    let deadline = std::time::Instant::now() + Duration::from_secs(seconds);
    let mut next_frame_at = std::time::Instant::now();

    while std::time::Instant::now() < deadline {
        if let Some(worker) = protocol_worker.as_ref() {
            if std::time::Instant::now() >= next_frame_at {
                worker.request(ProtocolRequest::screen(
                    generator.next_frame(),
                    state.media_size,
                )?);
                next_frame_at += frame_interval;
            }

            if let Some(response) = worker.try_recv_latest() {
                match response {
                    Ok(protocol) => {
                        state.decoded_frames = state.decoded_frames.saturating_add(1);
                        state.last_protocol_seq = screen_protocol_seq(&protocol);
                        state.protocol = Some(protocol);
                        state.media_error = None;
                    }
                    Err(error) => state.media_error = Some(error.message),
                }
            }
        }

        while event::poll(Duration::from_millis(1))? {
            let CrosstermEvent::Key(key) = event::read()? else {
                continue;
            };
            if key.kind == KeyEventKind::Press && key.code == KeyCode::Char('q') {
                return Ok(());
            }
        }

        terminal.draw(|frame| render_media_shell(frame, &mut state, kitty_available))?;
        tokio::time::sleep(Duration::from_millis(16)).await;
    }

    Ok(())
}

async fn run_local_screen_smoke(
    profile: ScreenCaptureProfile,
    path: LocalScreenPath,
    seconds: u64,
) -> Result<()> {
    let config = ScreenCaptureConfig::primary_display_for_profile(profile);
    let mut capture_session = ScreenCaptureSession::start(config)
        .await
        .context("failed to start local screen capture")?;
    let capture_info = capture_session.info().clone();

    let mut terminal = TerminalSession::enter()?;
    let picker = ratatui_image::picker::Picker::from_query_stdio()
        .unwrap_or_else(|_| ratatui_image::picker::Picker::halfblocks());
    let protocol_type = picker.protocol_type();
    let kitty_available = protocol_type == ProtocolType::Kitty;
    let protocol_worker = kitty_available.then(|| ProtocolWorker::spawn(picker));
    let mut state = UiState::new(protocol_type, TuiEventSink::channel(1).0);
    state.status = format!("local screen smoke {} {}", profile.label(), path.label());
    state.active_session_id = Some(format!(
        "{} {}x{}@{}",
        capture_info.backend.label(),
        capture_info.format.width,
        capture_info.format.height,
        capture_info.format.fps
    ));

    let mut encoder = (path == LocalScreenPath::Vp8)
        .then(|| {
            let (width, height) = profile.dimensions();
            Vp8VideoEncoder::new_with_dimensions(
                video_profile_for_screen_profile(profile),
                width,
                height,
            )
        })
        .transpose()
        .map_err(|error| anyhow!("failed to start local VP8 encoder: {error}"))?;
    let mut decoder = (path == LocalScreenPath::Vp8)
        .then(Vp8VideoDecoder::new)
        .transpose()
        .map_err(|error| anyhow!("failed to start local VP8 decoder: {error}"))?;

    let deadline = std::time::Instant::now() + Duration::from_secs(seconds);
    let mut next_seq = 0_u32;
    let mut encoded_frames = 0_u64;

    while std::time::Instant::now() < deadline {
        if let Some(worker) = protocol_worker.as_ref() {
            if let Some(frame) = capture_session.try_recv_latest_i420() {
                state.received_frames = state.received_frames.saturating_add(1);
                match local_screen_frame_to_rgba(
                    path,
                    profile,
                    &mut encoder,
                    &mut decoder,
                    next_seq,
                    frame,
                ) {
                    Ok(Some((frame, encoded_count))) => {
                        encoded_frames = encoded_frames.saturating_add(encoded_count);
                        state.decoded_frames = state.decoded_frames.saturating_add(1);
                        worker.request(ProtocolRequest::screen(frame, state.media_size)?);
                        next_seq = next_seq.wrapping_add(1);
                    }
                    Ok(None) => {}
                    Err(error) => {
                        state.decoder_errors = state.decoder_errors.saturating_add(1);
                        state.media_error = Some(error.to_string());
                    }
                }
            }

            if let Some(response) = worker.try_recv_latest() {
                match response {
                    Ok(protocol) => {
                        state.last_protocol_seq = screen_protocol_seq(&protocol);
                        state.protocol = Some(protocol);
                        state.media_error = None;
                    }
                    Err(error) => state.media_error = Some(error.message),
                }
            }
        }

        state.pending_frame_drops = capture_session.stats().dropped_i420_frames;
        state.encoded_frames = encoded_frames;

        while event::poll(Duration::from_millis(1))? {
            let CrosstermEvent::Key(key) = event::read()? else {
                continue;
            };
            if key.kind == KeyEventKind::Press && key.code == KeyCode::Char('q') {
                return Ok(());
            }
        }

        terminal.draw(|frame| render_media_shell(frame, &mut state, kitty_available))?;
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    Ok(())
}

fn local_screen_frame_to_rgba(
    path: LocalScreenPath,
    profile: ScreenCaptureProfile,
    encoder: &mut Option<Vp8VideoEncoder>,
    decoder: &mut Option<Vp8VideoDecoder>,
    seq: u32,
    frame: rchat_screen_capture::I420ScreenFrame,
) -> Result<Option<(DecodedRgbaFrame, u64)>> {
    match path {
        LocalScreenPath::Direct => {
            let rgba = i420_to_rgba(&frame.data, frame.width, frame.height)
                .map_err(|error| anyhow!("local I420 to RGBA failed: {error}"))?;
            Ok(Some((
                DecodedRgbaFrame {
                    session_id: "local-screen-smoke".to_string(),
                    seq,
                    timestamp_us: frame.timestamp_us,
                    width: frame.width,
                    height: frame.height,
                    rgba,
                },
                0,
            )))
        }
        LocalScreenPath::Vp8 => {
            let encoder = encoder
                .as_mut()
                .ok_or_else(|| anyhow!("local VP8 encoder is unavailable"))?;
            let decoder = decoder
                .as_mut()
                .ok_or_else(|| anyhow!("local VP8 decoder is unavailable"))?;
            let force_keyframe = seq == 0 || seq % profile.keyframe_interval_frames() == 0;
            let packets = encoder
                .encode_i420(
                    frame.timestamp_us,
                    frame.width,
                    frame.height,
                    &frame.data,
                    force_keyframe,
                )
                .map_err(|error| anyhow!("local VP8 encode failed: {error}"))?;
            let encoded_count = packets.len() as u64;
            let mut decoded = None;
            for packet in packets {
                let rgba = decoder
                    .decode_rgba(&packet.payload)
                    .map_err(|error| anyhow!("local VP8 decode failed: {error}"))?;
                decoded = Some(DecodedRgbaFrame {
                    session_id: "local-screen-smoke".to_string(),
                    seq,
                    timestamp_us: frame.timestamp_us,
                    width: rgba.width,
                    height: rgba.height,
                    rgba: rgba.rgba,
                });
            }
            Ok(decoded.map(|frame| (frame, encoded_count)))
        }
    }
}

struct InlineLoadRequest {
    key: InlineMediaKey,
    message: TuiMessage,
}

struct InlineLoadResponse {
    key: InlineMediaKey,
    result: std::result::Result<DynamicImage, String>,
}

struct InlineMediaLoader {
    tx: std_mpsc::Sender<InlineLoadRequest>,
    rx: std_mpsc::Receiver<InlineLoadResponse>,
}

impl InlineMediaLoader {
    fn spawn(app_state: AppState) -> Self {
        let (request_tx, request_rx) = std_mpsc::channel::<InlineLoadRequest>();
        let (response_tx, response_rx) = std_mpsc::channel::<InlineLoadResponse>();

        thread::spawn(move || {
            while let Ok(request) = request_rx.recv() {
                let result = load_inline_media_preview(&app_state, &request.message)
                    .map_err(|error| error.to_string());
                if response_tx
                    .send(InlineLoadResponse {
                        key: request.key,
                        result,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });

        Self {
            tx: request_tx,
            rx: response_rx,
        }
    }

    fn request(&self, key: InlineMediaKey, message: &TuiMessage) {
        let _ = self.tx.send(InlineLoadRequest {
            key,
            message: message.clone(),
        });
    }

    fn try_recv_all(&self) -> Vec<InlineLoadResponse> {
        let mut responses = Vec::new();
        while let Ok(response) = self.rx.try_recv() {
            responses.push(response);
        }
        responses
    }
}

fn load_inline_media_preview(app_state: &AppState, message: &TuiMessage) -> Result<DynamicImage> {
    let file_hash = message
        .file_hash
        .as_deref()
        .ok_or_else(|| anyhow!("inline media message has no file hash"))?;
    let conn = app_state
        .db_conn
        .lock()
        .map_err(|error| anyhow!("database lock failed: {error}"))?;
    let data = storage::object::load(&conn, file_hash, None)
        .with_context(|| format!("failed to load inline media object {file_hash}"))?;
    drop(conn);
    decode_inline_media_preview(&data)
}

async fn run_auth_screen(app_state: &AppState, terminal: &mut TerminalSession) -> Result<bool> {
    let mut config_manager = app_state.config_manager.lock().await;
    if config_manager.try_restore_session() {
        drop(config_manager);
        match needs_identity_choice(app_state).await {
            Ok(false) => return Ok(true),
            Ok(true) => {
                let mut auth = AuthUiState::new(AuthMode::GitHubLogin);
                return run_auth_form(app_state, terminal, &mut auth).await;
            }
            Err(_) => {
                let mut config_manager = app_state.config_manager.lock().await;
                config_manager.clear_restored_session();
            }
        }
        config_manager = app_state.config_manager.lock().await;
    }
    let mode = if config_manager.exists() {
        AuthMode::Unlock
    } else {
        AuthMode::CreateVault
    };
    drop(config_manager);

    let mut auth = AuthUiState::new(mode);
    run_auth_form(app_state, terminal, &mut auth).await
}

async fn run_auth_form(
    app_state: &AppState,
    terminal: &mut TerminalSession,
    auth: &mut AuthUiState,
) -> Result<bool> {
    loop {
        terminal.draw(|frame| render_auth_shell(frame, auth))?;

        while event::poll(Duration::from_millis(16))? {
            let CrosstermEvent::Key(key) = event::read()? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match handle_auth_key(app_state, auth, key).await? {
                AuthKeyOutcome::Continue => {}
                AuthKeyOutcome::Authenticated => return Ok(true),
                AuthKeyOutcome::Quit => return Ok(false),
            }
        }

        match tick_auth(app_state, auth).await? {
            AuthKeyOutcome::Continue => {}
            AuthKeyOutcome::Authenticated => return Ok(true),
            AuthKeyOutcome::Quit => return Ok(false),
        }

        tokio::time::sleep(Duration::from_millis(16)).await;
    }
}

async fn needs_identity_choice(app_state: &AppState) -> Result<bool> {
    let config_manager = app_state.config_manager.lock().await;
    let config = config_manager.load().await?;
    Ok(config.system.github_token.is_none()
        && config
            .user
            .profile
            .alias
            .as_deref()
            .is_none_or(|alias| alias.trim().is_empty()))
}

async fn send_network_command(network_state: &NetworkState, command: NetworkCommand) -> Result<()> {
    let sender = network_state.sender.lock().await.clone();
    sender
        .send(command)
        .await
        .map_err(|_| anyhow!("network command channel is closed"))
}

async fn refresh_direct_chats(
    app_state: &AppState,
    network_state: &NetworkState,
    state: &mut UiState,
) -> Result<()> {
    let chats = direct::list_direct_chats(app_state, network_state).await?;
    let group_chats = list_group_chats(app_state, network_state).await?;
    let envelope_rows = envelopes::list_envelopes(app_state)?;
    let assignment_rows = envelopes::list_assignments(app_state)?;
    state.app.pinned_chat_keys = settings_peers::get_pinned_peers(app_state)
        .await?
        .into_iter()
        .collect();
    state.app.replace_chats(
        chats
            .into_iter()
            .map(|chat| TuiChat {
                id: chat.id,
                name: chat.name,
                latest_timestamp: chat.latest_timestamp,
                unread_count: chat.unread_count,
            })
            .chain(group_chats)
            .collect(),
    );
    state.app.replace_envelopes(
        envelope_rows
            .into_iter()
            .map(|envelope| TuiEnvelope {
                id: envelope.id,
                name: envelope.name,
                icon: envelope.icon,
            })
            .collect(),
    );
    state.app.replace_envelope_assignments(
        assignment_rows
            .into_iter()
            .map(|assignment| (assignment.chat_id, assignment.envelope_id))
            .collect(),
    );
    state.app.status = format!("{} chats", state.app.chats.len());
    Ok(())
}

async fn list_group_chats(
    app_state: &AppState,
    network_state: &NetworkState,
) -> Result<Vec<TuiChat>> {
    let (items, latest_times, unread_counts) = {
        let conn = app_state
            .db_conn
            .lock()
            .map_err(|error| anyhow!("database lock failed: {error}"))?;
        (
            storage::db::get_chat_list(&conn)?,
            storage::db::get_chat_latest_times(&conn)?,
            storage::db::get_unread_counts(&conn, "Me")?,
        )
    };

    let mut summaries = items
        .into_iter()
        .filter_map(|item| group_summary_from_item(item, &latest_times, &unread_counts))
        .collect::<Vec<_>>();

    let now = now_unix_timestamp();
    let temp_state = network_state.temporary_state.lock().await;
    for (chat_id, session) in &temp_state.chats {
        if session.archived || !matches!(session.kind, TemporaryChatKind::Group) {
            continue;
        }
        if summaries.iter().any(|summary| summary.id == *chat_id) {
            continue;
        }
        let latest_timestamp = temp_state
            .messages
            .get(chat_id)
            .and_then(|messages| messages.last())
            .map(|message| message.timestamp)
            .unwrap_or(now);
        summaries.push(TuiChat {
            id: chat_id.clone(),
            name: session.name.clone(),
            latest_timestamp,
            unread_count: 0,
        });
    }

    Ok(summaries)
}

fn group_summary_from_item(
    item: storage::db::ChatListItem,
    latest_times: &HashMap<String, i64>,
    unread_counts: &HashMap<String, i64>,
) -> Option<TuiChat> {
    if !item.is_group {
        return None;
    }

    Some(TuiChat {
        latest_timestamp: latest_times.get(&item.id).copied().unwrap_or_default(),
        unread_count: unread_counts.get(&item.id).copied().unwrap_or_default(),
        id: item.id,
        name: item.name,
    })
}

async fn open_direct_chat(
    app_state: &AppState,
    network_state: &NetworkState,
    state: &mut UiState,
    chat_id: &str,
) -> Result<()> {
    let history = if chat_kind::is_temp_group_chat_id(chat_id) {
        temporary::get_temporary_group_history(network_state, chat_id).await?
    } else {
        direct::get_direct_history(app_state, network_state, chat_id).await?
    };
    state.app.select_chat_with_history(chat_id, history);
    direct::mark_direct_messages_read(app_state, network_state, chat_id).await?;
    Ok(())
}

async fn open_chat_list_item(
    app_state: &AppState,
    network_state: &NetworkState,
    state: &mut UiState,
    chat_id: &str,
) -> Result<()> {
    if chat_kind::is_temp_group_chat_id(chat_id) {
        return open_direct_chat(app_state, network_state, state, chat_id).await;
    }
    if is_group_chat_list_item(chat_id) {
        state.app.active_chat_id = Some(chat_id.to_string());
        state.app.messages.clear();
        state.app.history_scroll_offset = 0;
        state.app.status = "durable group chat history is not implemented in rchat-tui yet"
            .to_string();
        return Ok(());
    }

    open_direct_chat(app_state, network_state, state, chat_id).await
}

async fn open_chat_details(
    app_state: &AppState,
    network_state: &NetworkState,
    state: &mut UiState,
    chat_id: &str,
) -> Result<()> {
    let chat_id = db_chat_id(chat_id);
    let overview = details::overview(app_state, network_state, &chat_id).await?;
    let stats = details::stats(app_state, &chat_id)?;
    let recent_files = details::files(app_state, &chat_id, Some("all"), Some(8), Some(0))?
        .into_iter()
        .map(Into::into)
        .collect();

    state.app.chat_details = Some(TuiChatDetails {
        chat_id: overview.chat_id,
        peer_id: overview.peer_id,
        peer_name: overview.peer_name,
        peer_alias: overview.peer_alias,
        avatar_url: overview.avatar_url,
        connected: overview.connection.connected,
        remote_addr: overview.connection.remote_addr,
        reconnect_count: stats.reconnect_count,
        sent_total: stats.sent_total,
        received_total: stats.received_total,
        recent_files,
    });
    state.app.status = "chat details".to_string();
    Ok(())
}

async fn send_composer(
    app_state: &AppState,
    network_state: &NetworkState,
    state: &mut UiState,
) -> Result<()> {
    let Some(draft) = state.app.prepare_composer_send() else {
        return Ok(());
    };
    let result = if chat_kind::is_temp_group_chat_id(&draft.chat_id) {
        temporary::send_temporary_group_text(
            app_state,
            network_state,
            &draft.chat_id,
            &draft.text,
        )
        .await
    } else {
        direct::send_direct_text(app_state, network_state, &draft.chat_id, &draft.text).await
    };
    match result {
        Ok(msg_id) => {
            state
                .app
                .mark_composer_send_succeeded(draft, msg_id, now_unix_timestamp());
            state.app.status = "message sent".to_string();
        }
        Err(error) => {
            state.app.mark_composer_send_failed(error.to_string());
            state.app.status = "send failed".to_string();
        }
    }
    Ok(())
}

fn active_chat_id(state: &UiState) -> Result<String> {
    state
        .app
        .active_chat_id
        .clone()
        .or_else(|| state.app.selected_chat_id().map(ToOwned::to_owned))
        .ok_or_else(|| anyhow!("select a chat first"))
}

async fn send_attachment_from_path(
    app_state: &AppState,
    network_state: &NetworkState,
    state: &mut UiState,
    kind: chat_media::MediaKind,
    path: &str,
) -> Result<()> {
    let path = path.trim();
    if path.is_empty() {
        return Err(anyhow!("attachment path is empty"));
    }
    let chat_id = active_chat_id(state)?;
    let result =
        chat_media::send_file_from_path(app_state, network_state, &chat_id, kind, path).await?;
    refresh_direct_chats(app_state, network_state, state).await?;
    open_direct_chat(app_state, network_state, state, &chat_id).await?;
    state.app.selected_attachment_message_id = Some(result.msg_id);
    state.app.status = format!("sent {}", attachment_kind_label(kind));
    Ok(())
}

async fn send_sticker_hash(
    app_state: &AppState,
    network_state: &NetworkState,
    state: &mut UiState,
    file_hash: &str,
) -> Result<()> {
    let file_hash = file_hash.trim();
    if file_hash.is_empty() {
        return Err(anyhow!("sticker hash is empty"));
    }
    let chat_id = active_chat_id(state)?;
    let result = chat_media::send_sticker(app_state, network_state, &chat_id, file_hash).await?;
    refresh_direct_chats(app_state, network_state, state).await?;
    open_direct_chat(app_state, network_state, state, &chat_id).await?;
    state.app.selected_attachment_message_id = Some(result.msg_id);
    state.app.status = "sticker sent".to_string();
    Ok(())
}

fn save_attachment_to_path(app_state: &AppState, file_hash: &str, target_path: &str) -> Result<()> {
    let target_path = target_path.trim();
    if target_path.is_empty() {
        return Err(anyhow!("target path is empty"));
    }
    chat_media::save_attachment_to_path(app_state, file_hash, target_path)?;
    Ok(())
}

fn copy_hash_to_clipboard(file_hash: &str) -> Result<()> {
    let mut clipboard = arboard::Clipboard::new().context("clipboard unavailable")?;
    clipboard
        .set_text(file_hash.to_string())
        .context("failed to copy hash")
}

fn open_attachment_external(app_state: &AppState, file_hash: &str) -> Result<PathBuf> {
    let loaded = chat_media::load_attachment_bytes(app_state, file_hash)?;
    let file_name = safe_attachment_file_name(
        loaded
            .file_name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or(file_hash),
    );
    let dir = std::env::temp_dir().join("rchat-tui-open");
    fs::create_dir_all(&dir).context("failed to create temp attachment directory")?;
    let path = dir.join(format!("{}-{}", short_hash(file_hash), file_name));
    fs::write(&path, loaded.bytes).context("failed to write attachment temp file")?;
    launch_path(&path)?;
    Ok(path)
}

fn open_media_viewer_for_selected(app_state: &AppState, state: &mut UiState) -> Result<()> {
    if !state.app.open_media_viewer_for_selected() {
        let message = state
            .app
            .last_error
            .clone()
            .unwrap_or_else(|| "no attachment selected".to_string());
        return Err(anyhow!(message));
    }
    load_media_viewer_image(app_state, state);
    Ok(())
}

fn open_media_viewer_for_hash(
    app_state: &AppState,
    state: &mut UiState,
    file_hash: &str,
) -> Result<()> {
    if !state.app.open_media_viewer_for_hash(file_hash) {
        let message = state
            .app
            .last_error
            .clone()
            .unwrap_or_else(|| "attachment is not in the active chat".to_string());
        return Err(anyhow!(message));
    }
    load_media_viewer_image(app_state, state);
    Ok(())
}

fn load_media_viewer_image(app_state: &AppState, state: &mut UiState) {
    reset_viewer_protocol(state);
    let Some(snapshot) = state.app.media_viewer.clone() else {
        state.viewer_image = None;
        return;
    };
    if snapshot.kind != MediaViewerKind::Image {
        state.viewer_image = None;
        return;
    }

    match chat_media::load_attachment_bytes(app_state, &snapshot.file_hash)
        .and_then(|loaded| decode_inline_media_preview(&loaded.bytes))
    {
        Ok(image) => {
            state.viewer_image = Some(ViewerLoadedImage {
                file_hash: snapshot.file_hash,
                image: Arc::new(image),
            });
            if let Some(viewer) = state.app.media_viewer.as_mut() {
                viewer.status = Some("image loaded".to_string());
                viewer.error = None;
            }
        }
        Err(error) => {
            state.viewer_image = None;
            if let Some(viewer) = state.app.media_viewer.as_mut() {
                viewer.error = Some(error.to_string());
                viewer.status = None;
            }
        }
    }
}

fn reset_viewer_protocol(state: &mut UiState) {
    state.viewer_protocol = None;
    state.viewer_protocol_key = None;
}

fn request_viewer_protocol_key(state: &mut UiState, key: MediaViewerKey) {
    state.viewer_protocol_key = Some(key);
}

fn close_media_viewer(state: &mut UiState) {
    state.app.media_viewer = None;
    state.viewer_image = None;
    reset_viewer_protocol(state);
}

fn launch_path(path: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    let status = ProcessCommand::new("open").arg(path).status();

    #[cfg(target_os = "linux")]
    let status = ProcessCommand::new("xdg-open").arg(path).status();

    #[cfg(target_os = "windows")]
    let status = ProcessCommand::new("cmd")
        .args(["/C", "start", "", &path.display().to_string()])
        .status();

    let status = status.context("failed to launch default application")?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("default application exited with {status}"))
    }
}

async fn retry_attachment_fetch(
    app_state: &AppState,
    network_state: &NetworkState,
    state: &UiState,
    file_hash: &str,
) -> Result<()> {
    let chat_id = active_chat_id(state)?;
    chat_media::retry_direct_attachment_fetch(app_state, network_state, &chat_id, file_hash).await
}

fn safe_attachment_file_name(name: &str) -> String {
    let cleaned = name
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' | '\0' => '_',
            other => other,
        })
        .collect::<String>();
    if cleaned.trim().is_empty() {
        "attachment.bin".to_string()
    } else {
        cleaned
    }
}

fn attachment_kind_label(kind: chat_media::MediaKind) -> &'static str {
    match kind {
        chat_media::MediaKind::Image => "image",
        chat_media::MediaKind::Document => "document",
        chat_media::MediaKind::Video => "video",
        chat_media::MediaKind::Audio => "audio",
    }
}

fn request_new_person_qr_protocol(
    state: &mut UiState,
    worker: &ProtocolWorker,
    kitty_available: bool,
) {
    let size = Size::new(NEW_PERSON_QR_WIDTH, NEW_PERSON_QR_HEIGHT);
    let payload = state
        .app
        .new_person
        .as_ref()
        .and_then(|modal| modal.qr_payload.clone());
    let Some(payload) = payload else {
        return;
    };
    if !should_request_qr_protocol(kitty_available, &state.inline_media_cache, &payload, size) {
        return;
    }

    let key = qr_inline_key(&payload, size);
    match qr_dynamic_image(&payload) {
        Ok(image) => {
            state.inline_media_cache.insert_loading(key.clone());
            worker.request(ProtocolRequest::inline(key, image, size));
        }
        Err(error) => {
            state
                .inline_media_cache
                .insert_error(key, error.to_string());
            if let Some(modal) = state.app.new_person.as_mut() {
                modal.qr_error = Some(error.to_string());
            }
        }
    }
}

async fn close_new_person_modal(network_state: &NetworkState, state: &mut UiState) -> Result<()> {
    if state
        .app
        .new_person
        .as_ref()
        .is_some_and(|modal| modal.step == NewPersonStep::LocalScan)
    {
        mdns::disable_fast_discovery();
    }
    let _ = network_state;
    state.app.close_new_person();
    Ok(())
}

async fn set_new_person_step(
    network_state: &NetworkState,
    state: &mut UiState,
    step: NewPersonStep,
) -> Result<()> {
    let was_local_scan = state
        .app
        .new_person
        .as_ref()
        .is_some_and(|modal| modal.step == NewPersonStep::LocalScan);

    if was_local_scan && step != NewPersonStep::LocalScan {
        mdns::disable_fast_discovery();
    }
    if !was_local_scan && step == NewPersonStep::LocalScan {
        mdns::enable_fast_discovery();
    }

    if let Some(modal) = state.app.new_person.as_mut() {
        modal.set_step(step);
        match step {
            NewPersonStep::CreateInviteCode if !modal.create_invite_password.is_empty() => {
                modal.qr_payload = Some(modal.create_invite_password.clone());
            }
            NewPersonStep::TemporaryChat => {}
            _ => {
                modal.qr_payload = None;
            }
        }
    }

    if step == NewPersonStep::TemporaryChat {
        refresh_active_temporary_invite(network_state, state).await?;
    }

    Ok(())
}

async fn refresh_active_temporary_invite(
    network_state: &NetworkState,
    state: &mut UiState,
) -> Result<()> {
    let active = temporary::get_active_temporary_invite(network_state).await?;
    if let Some(modal) = state.app.new_person.as_mut() {
        if let Some(invite) = active {
            modal.active_temporary_link = Some(invite.deep_link.clone());
            modal.active_temporary_remaining_seconds = Some(invite.remaining_seconds);
            modal.qr_payload = Some(invite.deep_link);
        } else {
            modal.active_temporary_link = None;
            modal.active_temporary_remaining_seconds = None;
            modal.qr_payload = None;
        }
    }
    Ok(())
}

async fn handle_new_person_key(
    app_state: &AppState,
    network_state: &NetworkState,
    state: &mut UiState,
    code: KeyCode,
) -> Result<()> {
    match code {
        KeyCode::Esc => {
            let previous = state
                .app
                .new_person
                .as_ref()
                .and_then(|modal| previous_new_person_step(modal.step));
            if let Some(step) = previous {
                set_new_person_step(network_state, state, step).await?;
            } else {
                close_new_person_modal(network_state, state).await?;
            }
        }
        KeyCode::Up => {
            let local_count = state.app.local_peers.len();
            if let Some(modal) = state.app.new_person.as_mut() {
                modal.move_focus(-1, local_count);
            }
        }
        KeyCode::Down => {
            let local_count = state.app.local_peers.len();
            if let Some(modal) = state.app.new_person.as_mut() {
                modal.move_focus(1, local_count);
            }
        }
        KeyCode::Enter => {
            activate_new_person_focus(app_state, network_state, state).await?;
        }
        KeyCode::Backspace => {
            if let Some(modal) = state.app.new_person.as_mut() {
                modal.pop_char();
            }
        }
        KeyCode::Char(ch) => {
            if let Some(modal) = state.app.new_person.as_mut() {
                modal.push_char(ch);
            }
        }
        _ => {}
    }
    Ok(())
}

async fn activate_new_person_focus(
    app_state: &AppState,
    network_state: &NetworkState,
    state: &mut UiState,
) -> Result<()> {
    let Some(focus) = state
        .app
        .new_person
        .as_ref()
        .map(|modal| modal.focus.clone())
    else {
        return Ok(());
    };

    match focus {
        NewPersonField::LocalNetwork => {
            set_new_person_step(network_state, state, NewPersonStep::LocalScan).await?;
        }
        NewPersonField::OnlineNetwork => {
            set_new_person_step(network_state, state, NewPersonStep::Online).await?;
        }
        NewPersonField::LocalPeer(index) => {
            let Some(peer) = state.app.local_peers.get(index).cloned() else {
                return Ok(());
            };
            send_network_command(
                network_state,
                NetworkCommand::RequestConnection {
                    peer_id: peer.peer_id.clone(),
                },
            )
            .await?;
            if let Some(modal) = state.app.new_person.as_mut() {
                modal.waiting_peer_id = Some(peer.peer_id.clone());
            }
            state.app.status = format!("connecting to {}", peer.peer_id);
        }
        NewPersonField::CreateInvite => {
            set_new_person_step(network_state, state, NewPersonStep::CreateInviteUser).await?;
        }
        NewPersonField::AcceptInvite => {
            set_new_person_step(network_state, state, NewPersonStep::AcceptInviteUser).await?;
        }
        NewPersonField::TemporaryChat => {
            set_new_person_step(network_state, state, NewPersonStep::TemporaryChat).await?;
        }
        NewPersonField::CreateInviteNext => {
            let Some(invitee) = state
                .app
                .new_person
                .as_ref()
                .map(|modal| modal.invitee_username.trim().to_string())
            else {
                return Ok(());
            };
            if invitee.is_empty() {
                set_new_person_error(state, "enter the invitee GitHub username");
                return Ok(());
            }
            if let Some(modal) = state.app.new_person.as_mut() {
                modal.create_invite_password = direct::generate_invite_password();
                modal.qr_payload = Some(modal.create_invite_password.clone());
            }
            set_new_person_step(network_state, state, NewPersonStep::CreateInviteCode).await?;
        }
        NewPersonField::CreateInviteConfirm => {
            let Some((invitee, password)) = state.app.new_person.as_ref().map(|modal| {
                (
                    modal.invitee_username.trim().to_string(),
                    modal.create_invite_password.clone(),
                )
            }) else {
                return Ok(());
            };
            direct::create_github_invite(app_state, network_state, &invitee, &password).await?;
            state.app.status = format!("invite published for {invitee}");
            close_new_person_modal(network_state, state).await?;
        }
        NewPersonField::AcceptInviteNext => {
            let inviter = state
                .app
                .new_person
                .as_ref()
                .map(|modal| modal.inviter_username.trim().to_string())
                .unwrap_or_default();
            if inviter.is_empty() {
                set_new_person_error(state, "enter the inviter GitHub username");
                return Ok(());
            }
            set_new_person_step(network_state, state, NewPersonStep::AcceptInviteCode).await?;
        }
        NewPersonField::DecodeInviteQr => {
            let path = state
                .app
                .new_person
                .as_ref()
                .map(|modal| modal.invite_qr_path.clone())
                .unwrap_or_default();
            match decode_qr_payload_from_image_path(&path) {
                Ok(payload) => {
                    if let Some(modal) = state.app.new_person.as_mut() {
                        modal.invite_password = payload;
                        modal.qr_error = None;
                    }
                }
                Err(error) => set_new_person_qr_error(state, error.to_string()),
            }
        }
        NewPersonField::RedeemInvite => {
            let Some((inviter, password)) = state.app.new_person.as_ref().map(|modal| {
                (
                    modal.inviter_username.trim().to_string(),
                    modal.invite_password.trim().to_string(),
                )
            }) else {
                return Ok(());
            };
            if inviter.is_empty() || password.is_empty() {
                set_new_person_error(state, "enter inviter and password");
                return Ok(());
            }
            let chat_id =
                direct::redeem_github_invite(app_state, network_state, &inviter, &password).await?;
            refresh_direct_chats(app_state, network_state, state).await?;
            open_direct_chat(app_state, network_state, state, &chat_id).await?;
            state.app.status = format!("connected invite from {inviter}");
            close_new_person_modal(network_state, state).await?;
        }
        NewPersonField::CreateTemporary => {
            let name = state
                .app
                .new_person
                .as_ref()
                .map(|modal| modal.temporary_name.trim().to_string())
                .filter(|name| !name.is_empty());
            let invite = temporary::create_temporary_invite(
                app_state,
                network_state,
                TemporaryChatKind::Dm,
                name.as_deref(),
            )
            .await?;
            if let Some(modal) = state.app.new_person.as_mut() {
                modal.active_temporary_link = Some(invite.deep_link.clone());
                modal.active_temporary_remaining_seconds = Some(invite.remaining_seconds);
                modal.qr_payload = Some(invite.deep_link);
                modal.error = None;
            }
            state.app.status = "temporary invite created".to_string();
        }
        NewPersonField::DecodeTemporaryQr => {
            let path = state
                .app
                .new_person
                .as_ref()
                .map(|modal| modal.temporary_qr_path.clone())
                .unwrap_or_default();
            match decode_qr_payload_from_image_path(&path) {
                Ok(payload) => {
                    if let Some(modal) = state.app.new_person.as_mut() {
                        modal.temporary_link = payload;
                        modal.qr_error = None;
                    }
                }
                Err(error) => set_new_person_qr_error(state, error.to_string()),
            }
        }
        NewPersonField::RedeemTemporary => {
            let link = state
                .app
                .new_person
                .as_ref()
                .map(|modal| modal.temporary_link.trim().to_string())
                .unwrap_or_default();
            if link.is_empty() {
                set_new_person_error(state, "paste a temporary invite link");
                return Ok(());
            }
            let result = temporary::redeem_temporary_invite(network_state, &link).await?;
            refresh_direct_chats(app_state, network_state, state).await?;
            open_direct_chat(app_state, network_state, state, &result.chat_id).await?;
            state.app.status = format!("temporary chat connected {}", result.chat_id);
            close_new_person_modal(network_state, state).await?;
        }
        NewPersonField::CancelTemporary => {
            temporary::cancel_temporary_invite(network_state).await?;
            if let Some(modal) = state.app.new_person.as_mut() {
                modal.active_temporary_link = None;
                modal.active_temporary_remaining_seconds = None;
                modal.qr_payload = None;
            }
            state.app.status = "temporary invite cancelled".to_string();
        }
        NewPersonField::InviteeUsername
        | NewPersonField::InviterUsername
        | NewPersonField::InvitePassword
        | NewPersonField::InviteQrPath
        | NewPersonField::TemporaryLink
        | NewPersonField::TemporaryQrPath => {}
    }
    Ok(())
}

fn set_new_person_error(state: &mut UiState, message: impl Into<String>) {
    if let Some(modal) = state.app.new_person.as_mut() {
        modal.error = Some(message.into());
    }
}

fn set_new_person_qr_error(state: &mut UiState, message: impl Into<String>) {
    if let Some(modal) = state.app.new_person.as_mut() {
        modal.qr_error = Some(message.into());
    }
}

async fn handle_attachment_modal_key(
    app_state: &AppState,
    network_state: &NetworkState,
    state: &mut UiState,
    code: KeyCode,
) -> Result<()> {
    match code {
        KeyCode::Esc => state.app.attachment_modal = None,
        KeyCode::Tab => {
            if let Some(modal) = state.app.attachment_modal.as_mut() {
                modal.cycle_focus();
            }
        }
        KeyCode::Up => {
            if let Some(modal) = state.app.attachment_modal.as_mut() {
                if modal.focus == AttachmentModalField::Picker {
                    modal.move_entry_selection(-1);
                } else {
                    modal.cycle_kind(-1);
                    refresh_attachment_picker_entries(modal);
                }
            }
        }
        KeyCode::Down => {
            if let Some(modal) = state.app.attachment_modal.as_mut() {
                if modal.focus == AttachmentModalField::Picker {
                    modal.move_entry_selection(1);
                } else {
                    modal.cycle_kind(1);
                    refresh_attachment_picker_entries(modal);
                }
            }
        }
        KeyCode::Left => {
            if let Some(modal) = state.app.attachment_modal.as_mut() {
                modal.go_parent();
                refresh_attachment_picker_entries(modal);
            }
        }
        KeyCode::Char('o') => {
            if let Some(modal) = state.app.attachment_modal.as_mut() {
                if let Some(path) = pick_attachment_file(modal.kind) {
                    modal.set_path(path);
                    modal.focus = AttachmentModalField::Send;
                } else {
                    modal.status = Some("native picker closed; use search below".to_string());
                }
            }
        }
        KeyCode::Enter => {
            let action = state.app.attachment_modal.as_ref().map(|modal| {
                (
                    modal.focus,
                    modal.kind,
                    modal.selected_path_or_entry(),
                    modal.selected_entry().cloned(),
                )
            });
            let Some((focus, kind, selected_path, selected_entry)) = action else {
                return Ok(());
            };
            if focus == AttachmentModalField::Picker {
                if let Some(entry) = selected_entry {
                    if entry.is_dir {
                        if let Some(modal) = state.app.attachment_modal.as_mut() {
                            modal.enter_directory(entry.path);
                            refresh_attachment_picker_entries(modal);
                        }
                        return Ok(());
                    }
                    if let Some(modal) = state.app.attachment_modal.as_mut() {
                        modal.set_path(entry.path);
                        modal.focus = AttachmentModalField::Send;
                    }
                    return Ok(());
                }
            }

            let Some(path) = selected_path else {
                if let Some(modal) = state.app.attachment_modal.as_mut() {
                    modal.error = Some("choose a file first".to_string());
                    modal.status = None;
                }
                return Ok(());
            };
            let path = path.display().to_string();
            send_attachment_from_path(app_state, network_state, state, kind, &path).await?;
            state.app.attachment_modal = None;
        }
        KeyCode::Backspace => {
            if let Some(modal) = state.app.attachment_modal.as_mut() {
                modal.pop_char();
            }
        }
        KeyCode::Char(ch) => {
            if let Some(modal) = state.app.attachment_modal.as_mut() {
                modal.push_char(ch);
            }
        }
        _ => {}
    }
    Ok(())
}

fn pick_attachment_file(kind: MediaKind) -> Option<PathBuf> {
    let mut dialog = FileDialog::new();
    dialog = match kind {
        MediaKind::Image => dialog.add_filter("Images", &["png", "jpg", "jpeg", "gif", "webp"]),
        MediaKind::Video => dialog.add_filter("Videos", &["mp4", "mov", "mkv", "webm", "avi"]),
        MediaKind::Audio => dialog.add_filter("Audio", &["mp3", "wav", "ogg", "flac", "m4a"]),
        MediaKind::Document => dialog.add_filter(
            "Documents",
            &["pdf", "txt", "md", "doc", "docx", "xls", "xlsx", "ppt", "pptx"],
        ),
    };
    dialog.pick_file()
}

fn refresh_attachment_picker_entries(modal: &mut crate::state::AttachmentModalState) {
    let entries = match list_attachment_picker_entries(&modal.picker_root, modal.kind) {
        Ok(entries) => entries,
        Err(error) => {
            modal.error = Some(error.to_string());
            modal.status = None;
            Vec::new()
        }
    };
    modal.set_picker_entries(entries);
}

fn list_attachment_picker_entries(root: &Path, kind: MediaKind) -> Result<Vec<AttachmentFileEntry>> {
    let mut entries = Vec::new();
    if let Some(parent) = root.parent() {
        entries.push(AttachmentFileEntry {
            path: parent.to_path_buf(),
            name: "../".to_string(),
            is_dir: true,
            size_bytes: None,
        });
    }
    for entry in fs::read_dir(root).with_context(|| format!("failed to read {}", root.display()))? {
        let entry = entry?;
        let path = entry.path();
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        let is_dir = metadata.is_dir();
        if !is_dir && !attachment_kind_accepts_path(kind, &path) {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        let name = name.to_string();
        entries.push(AttachmentFileEntry {
            path,
            name,
            is_dir,
            size_bytes: (!is_dir).then_some(metadata.len()),
        });
    }
    entries.sort_by_key(|entry| (!entry.is_dir, entry.name.to_lowercase()));
    Ok(entries)
}

fn attachment_kind_accepts_path(kind: MediaKind, path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
        return kind == MediaKind::Document;
    };
    let ext = ext.to_lowercase();
    match kind {
        MediaKind::Image => matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp"),
        MediaKind::Video => matches!(ext.as_str(), "mp4" | "mov" | "mkv" | "webm" | "avi"),
        MediaKind::Audio => matches!(ext.as_str(), "mp3" | "wav" | "ogg" | "flac" | "m4a"),
        MediaKind::Document => true,
    }
}

async fn handle_sticker_picker_key(
    app_state: &AppState,
    network_state: &NetworkState,
    state: &mut UiState,
    code: KeyCode,
) -> Result<()> {
    if state
        .app
        .sticker_picker
        .as_ref()
        .is_some_and(|picker| picker.mode == StickerPickerMode::AddPath)
    {
        match code {
            KeyCode::Esc => {
                if let Some(picker) = state.app.sticker_picker.as_mut() {
                    picker.exit_add_path_mode();
                }
            }
            KeyCode::Enter => import_sticker_from_picker(app_state, state)?,
            KeyCode::Backspace => {
                if let Some(picker) = state.app.sticker_picker.as_mut() {
                    picker.pop_char();
                }
            }
            KeyCode::Char(ch) => {
                if let Some(picker) = state.app.sticker_picker.as_mut() {
                    picker.push_char(ch);
                }
            }
            _ => {}
        }
        return Ok(());
    }

    match code {
        KeyCode::Esc => state.app.sticker_picker = None,
        KeyCode::Char('a') => {
            if let Some(path) = pick_sticker_file() {
                import_sticker_path_from_picker(app_state, state, path)?;
            } else if let Some(picker) = state.app.sticker_picker.as_mut() {
                picker.enter_add_path_mode();
            }
        }
        KeyCode::Up => {
            if let Some(picker) = state.app.sticker_picker.as_mut() {
                picker.move_selection(-1);
            }
        }
        KeyCode::Down | KeyCode::Tab => {
            if let Some(picker) = state.app.sticker_picker.as_mut() {
                picker.move_selection(1);
            }
        }
        KeyCode::Enter => {
            let Some(hash) = state
                .app
                .sticker_picker
                .as_ref()
                .and_then(|picker| picker.selected_hash().map(ToOwned::to_owned))
            else {
                return Err(anyhow!("no sticker selected"));
            };
            send_sticker_hash(app_state, network_state, state, &hash).await?;
            state.app.sticker_picker = None;
        }
        _ => {}
    }
    Ok(())
}

fn import_sticker_from_picker(app_state: &AppState, state: &mut UiState) -> Result<()> {
    let path = state
        .app
        .sticker_picker
        .as_ref()
        .map(|picker| picker.add_path.trim().to_string())
        .unwrap_or_default();
    if path.is_empty() {
        if let Some(picker) = state.app.sticker_picker.as_mut() {
            picker.error = Some("enter a sticker image path".to_string());
            picker.status = None;
        }
        return Ok(());
    }

    import_sticker_path_from_picker(app_state, state, PathBuf::from(path))
}

fn pick_sticker_file() -> Option<PathBuf> {
    FileDialog::new()
        .add_filter("Images", &["png", "jpg", "jpeg", "gif", "webp"])
        .pick_file()
}

fn import_sticker_path_from_picker(
    app_state: &AppState,
    state: &mut UiState,
    path: PathBuf,
) -> Result<()> {
    let path = path.display().to_string();
    let imported = settings_stickers::add_sticker(app_state, &path)?;
    let stickers = settings_stickers::list_stickers(app_state)?
        .into_iter()
        .map(|sticker| TuiSticker {
            file_hash: sticker.file_hash,
            name: sticker.name,
            size_bytes: sticker.size_bytes,
        })
        .collect::<Vec<_>>();
    let mut picker = StickerPickerState::new(stickers);
    picker.select_hash(&imported.file_hash);
    picker.status = Some(if imported.already_exists {
        "sticker already saved".to_string()
    } else {
        "sticker imported".to_string()
    });
    state.app.sticker_picker = Some(picker);
    Ok(())
}

async fn handle_context_menu_key(
    app_state: &AppState,
    network_state: &NetworkState,
    state: &mut UiState,
    code: KeyCode,
) -> Result<()> {
    let action_count = state
        .app
        .context_menu
        .as_ref()
        .map(context_menu_actions)
        .map(|actions| actions.len())
        .unwrap_or_default();
    match code {
        KeyCode::Esc => state.app.context_menu = None,
        KeyCode::Up => {
            if let Some(menu) = state.app.context_menu.as_mut() {
                menu.move_selection(-1, action_count);
            }
        }
        KeyCode::Down | KeyCode::Tab => {
            if let Some(menu) = state.app.context_menu.as_mut() {
                menu.move_selection(1, action_count);
            }
        }
        KeyCode::Enter => activate_context_menu_action(app_state, network_state, state).await?,
        _ => {}
    }
    Ok(())
}

fn open_context_menu_for_focus(state: &mut UiState) {
    let target = match state.app.focus {
        FocusPane::Chats => Some(ContextMenuTarget::Chat(state.app.selected_chat_index)),
        FocusPane::History => state
            .app
            .selected_message_id
            .clone()
            .map(ContextMenuTarget::Message),
        _ => None,
    };
    if let Some(target) = target {
        state.app.context_menu = Some(ContextMenuState::new(target));
    }
}

fn context_menu_actions(menu: &ContextMenuState) -> Vec<ContextMenuAction> {
    match &menu.target {
        ContextMenuTarget::Chat(_) => vec![
            ContextMenuAction::Open,
            ContextMenuAction::Details,
            ContextMenuAction::MoveToRoot,
            ContextMenuAction::Close,
        ],
        ContextMenuTarget::Envelope(_) => {
            vec![ContextMenuAction::DeleteEnvelope, ContextMenuAction::Close]
        }
        ContextMenuTarget::Message(_) => vec![
            ContextMenuAction::AttachmentActions,
            ContextMenuAction::Close,
        ],
    }
}

fn context_menu_action_label(action: ContextMenuAction) -> &'static str {
    match action {
        ContextMenuAction::Open => "Open",
        ContextMenuAction::Details => "Details",
        ContextMenuAction::MoveToRoot => "Remove from envelope",
        ContextMenuAction::DeleteEnvelope => "Delete envelope",
        ContextMenuAction::AttachmentActions => "Message actions",
        ContextMenuAction::Close => "Close",
    }
}

async fn activate_context_menu_action(
    app_state: &AppState,
    network_state: &NetworkState,
    state: &mut UiState,
) -> Result<()> {
    let Some(menu) = state.app.context_menu.clone() else {
        return Ok(());
    };
    let actions = context_menu_actions(&menu);
    let action = actions
        .get(menu.selected_index)
        .copied()
        .unwrap_or(ContextMenuAction::Close);
    state.app.context_menu = None;

    match (menu.target, action) {
        (ContextMenuTarget::Chat(index), ContextMenuAction::Open) => {
            if let Some(chat_id) = state.app.chats.get(index).map(|chat| chat.id.clone()) {
                open_chat_list_item(app_state, network_state, state, &chat_id).await?;
            }
        }
        (ContextMenuTarget::Chat(index), ContextMenuAction::Details) => {
            if let Some(chat_id) = state.app.chats.get(index).map(|chat| chat.id.clone()) {
                open_chat_details(app_state, network_state, state, &chat_id).await?;
            }
        }
        (ContextMenuTarget::Chat(index), ContextMenuAction::MoveToRoot) => {
            if let Some(chat_id) = state.app.chats.get(index).map(|chat| chat.id.clone()) {
                envelopes::move_chat_to_envelope(app_state, &chat_id, None)?;
                refresh_direct_chats(app_state, network_state, state).await?;
                state.app.status = format!("removed {chat_id} from envelope");
            }
        }
        (ContextMenuTarget::Envelope(id), ContextMenuAction::DeleteEnvelope) => {
            envelopes::delete_envelope(app_state, &id)?;
            refresh_direct_chats(app_state, network_state, state).await?;
            state.app.status = format!("deleted envelope {id}");
        }
        (ContextMenuTarget::Message(_), ContextMenuAction::AttachmentActions) => {
            open_attachment_actions_for_selected(app_state, state)?;
        }
        (_, ContextMenuAction::Close) => {}
        _ => {}
    }
    Ok(())
}

async fn handle_attachment_action_key(
    app_state: &AppState,
    network_state: &NetworkState,
    state: &mut UiState,
    code: KeyCode,
) -> Result<()> {
    match code {
        KeyCode::Esc => state.app.attachment_actions = None,
        KeyCode::Tab => {
            if let Some(modal) = state.app.attachment_actions.as_mut() {
                modal.cycle_focus();
            }
        }
        KeyCode::Backspace => {
            if let Some(modal) = state.app.attachment_actions.as_mut() {
                modal.pop_char();
            }
        }
        KeyCode::Char(ch) => {
            if let Some(modal) = state.app.attachment_actions.as_mut() {
                modal.push_char(ch);
            }
        }
        KeyCode::Enter => activate_attachment_action(app_state, network_state, state).await?,
        _ => {}
    }
    Ok(())
}

async fn handle_media_viewer_key(
    app_state: &AppState,
    network_state: &NetworkState,
    state: &mut UiState,
    code: KeyCode,
) -> Result<()> {
    match code {
        KeyCode::Esc => close_media_viewer(state),
        KeyCode::Tab => {
            if let Some(viewer) = state.app.media_viewer.as_mut() {
                viewer.cycle_focus();
            }
        }
        KeyCode::Backspace => {
            if let Some(viewer) = state.app.media_viewer.as_mut() {
                viewer.pop_char();
            }
        }
        KeyCode::Char('+') | KeyCode::Char('=') => {
            if let Some(viewer) = state.app.media_viewer.as_mut() {
                viewer.zoom_in();
            }
        }
        KeyCode::Char('-') => {
            if let Some(viewer) = state.app.media_viewer.as_mut() {
                viewer.zoom_out();
            }
        }
        KeyCode::Char('0') | KeyCode::Char('R') => {
            if let Some(viewer) = state.app.media_viewer.as_mut() {
                viewer.reset_view();
            }
        }
        KeyCode::Left => {
            if let Some(viewer) = state.app.media_viewer.as_mut() {
                viewer.pan_by(-64, 0);
            }
        }
        KeyCode::Right => {
            if let Some(viewer) = state.app.media_viewer.as_mut() {
                viewer.pan_by(64, 0);
            }
        }
        KeyCode::Up => {
            if let Some(viewer) = state.app.media_viewer.as_mut() {
                viewer.pan_by(0, -64);
            }
        }
        KeyCode::Down => {
            if let Some(viewer) = state.app.media_viewer.as_mut() {
                viewer.pan_by(0, 64);
            }
        }
        KeyCode::Char('s') => {
            activate_media_viewer_action(app_state, network_state, state, MediaViewerAction::Save)
                .await?
        }
        KeyCode::Char('o') => {
            activate_media_viewer_action(app_state, network_state, state, MediaViewerAction::Open)
                .await?
        }
        KeyCode::Char('c') => {
            activate_media_viewer_action(
                app_state,
                network_state,
                state,
                MediaViewerAction::CopyHash,
            )
            .await?
        }
        KeyCode::Char('r') => {
            activate_media_viewer_action(app_state, network_state, state, MediaViewerAction::Retry)
                .await?
        }
        KeyCode::Char(ch) => {
            if let Some(viewer) = state.app.media_viewer.as_mut() {
                viewer.push_char(ch);
            }
        }
        KeyCode::Enter => {
            let Some(viewer) = state.app.media_viewer.as_ref() else {
                return Ok(());
            };
            activate_media_viewer_action(app_state, network_state, state, viewer.focus).await?;
        }
        _ => {}
    }
    Ok(())
}

async fn activate_media_viewer_action(
    app_state: &AppState,
    network_state: &NetworkState,
    state: &mut UiState,
    action: MediaViewerAction,
) -> Result<()> {
    let Some(snapshot) = state.app.media_viewer.clone() else {
        return Ok(());
    };
    match action {
        MediaViewerAction::SavePath => {}
        MediaViewerAction::Save => {
            if snapshot.target_path.trim().is_empty() {
                if let Some(viewer) = state.app.media_viewer.as_mut() {
                    viewer.focus = MediaViewerAction::SavePath;
                    viewer.error = Some("enter a save path".to_string());
                    viewer.status = None;
                }
                return Ok(());
            }
            save_attachment_to_path(app_state, &snapshot.file_hash, &snapshot.target_path)?;
            if let Some(viewer) = state.app.media_viewer.as_mut() {
                viewer.status = Some("saved".to_string());
                viewer.error = None;
            }
        }
        MediaViewerAction::Open => {
            let path = open_attachment_external(app_state, &snapshot.file_hash)?;
            state.app.status = format!("opened {}", path.display());
            close_media_viewer(state);
        }
        MediaViewerAction::CopyHash => {
            match copy_hash_to_clipboard(&snapshot.file_hash) {
                Ok(()) => state.app.status = "hash copied".to_string(),
                Err(_) => state.app.status = format!("hash {}", snapshot.file_hash),
            }
            close_media_viewer(state);
        }
        MediaViewerAction::Retry => {
            retry_attachment_fetch(app_state, network_state, state, &snapshot.file_hash).await?;
            state.app.status = "attachment retry requested".to_string();
            close_media_viewer(state);
        }
        MediaViewerAction::Close => close_media_viewer(state),
    }
    Ok(())
}

async fn activate_attachment_action(
    app_state: &AppState,
    network_state: &NetworkState,
    state: &mut UiState,
) -> Result<()> {
    let Some(snapshot) = state.app.attachment_actions.clone() else {
        return Ok(());
    };
    match snapshot.focus {
        AttachmentActionField::View => {
            state.app.attachment_actions = None;
            open_media_viewer_for_hash(app_state, state, &snapshot.file_hash)?;
        }
        AttachmentActionField::SavePath => {}
        AttachmentActionField::Save => {
            save_attachment_to_path(app_state, &snapshot.file_hash, &snapshot.target_path)?;
            if let Some(modal) = state.app.attachment_actions.as_mut() {
                modal.status = Some("saved".to_string());
                modal.error = None;
            }
        }
        AttachmentActionField::Open => {
            let path = open_attachment_external(app_state, &snapshot.file_hash)?;
            state.app.status = format!("opened {}", path.display());
            state.app.attachment_actions = None;
        }
        AttachmentActionField::CopyHash => {
            match copy_hash_to_clipboard(&snapshot.file_hash) {
                Ok(()) => state.app.status = "hash copied".to_string(),
                Err(_) => state.app.status = format!("hash {}", snapshot.file_hash),
            }
            state.app.attachment_actions = None;
        }
        AttachmentActionField::Retry => {
            retry_attachment_fetch(app_state, network_state, state, &snapshot.file_hash).await?;
            state.app.status = "attachment retry requested".to_string();
            state.app.attachment_actions = None;
        }
        AttachmentActionField::SaveSticker => {
            let result = settings_stickers::save_sticker_from_message(app_state, &snapshot.file_hash)?;
            if let Some(modal) = state.app.attachment_actions.as_mut() {
                modal.sticker_saved = true;
                modal.status = Some(if result.already_exists {
                    "sticker already saved".to_string()
                } else {
                    "sticker saved".to_string()
                });
                modal.error = None;
                modal.focus = AttachmentActionField::Close;
            }
        }
        AttachmentActionField::Close => state.app.attachment_actions = None,
    }
    Ok(())
}

fn open_attachment_actions_for_selected(app_state: &AppState, state: &mut UiState) -> Result<()> {
    let Some(message) = state.app.selected_attachment_message().cloned() else {
        state.app.last_error = Some("no attachment selected".to_string());
        return Ok(());
    };
    let sticker_saved = if message.content_type == "sticker" {
        message
            .file_hash
            .as_deref()
            .map(|file_hash| sticker_exists(app_state, file_hash))
            .transpose()?
            .unwrap_or(false)
    } else {
        false
    };
    let Some(modal) =
        crate::state::AttachmentActionModalState::from_message_with_sticker_saved(
            &message,
            sticker_saved,
        )
    else {
        state.app.last_error = Some("selected message has no attachment".to_string());
        return Ok(());
    };

    state.app.show_command_palette = false;
    state.app.chat_details = None;
    state.app.show_help = false;
    state.app.new_person = None;
    state.app.settings = None;
    state.app.attachment_modal = None;
    state.app.sticker_picker = None;
    state.app.media_viewer = None;
    state.app.attachment_actions = Some(modal);
    Ok(())
}

fn sticker_exists(app_state: &AppState, file_hash: &str) -> Result<bool> {
    let conn = app_state
        .db_conn
        .lock()
        .map_err(|error| anyhow!("database lock failed: {error}"))?;
    Ok(storage::db::sticker_exists(&conn, file_hash))
}

fn previous_new_person_step(step: NewPersonStep) -> Option<NewPersonStep> {
    match step {
        NewPersonStep::SelectNetwork => None,
        NewPersonStep::LocalScan | NewPersonStep::Online => Some(NewPersonStep::SelectNetwork),
        NewPersonStep::TemporaryChat => Some(NewPersonStep::Online),
        NewPersonStep::CreateInviteUser | NewPersonStep::AcceptInviteUser => {
            Some(NewPersonStep::Online)
        }
        NewPersonStep::CreateInviteCode => Some(NewPersonStep::CreateInviteUser),
        NewPersonStep::AcceptInviteCode => Some(NewPersonStep::AcceptInviteUser),
    }
}

async fn open_settings_modal(app_state: &AppState, state: &mut UiState) -> Result<()> {
    state.app.open_settings();
    refresh_settings_modal(app_state, state).await
}

fn open_sticker_picker(app_state: &AppState, state: &mut UiState) -> Result<()> {
    let stickers = settings_stickers::list_stickers(app_state)?
        .into_iter()
        .map(|sticker| TuiSticker {
            file_hash: sticker.file_hash,
            name: sticker.name,
            size_bytes: sticker.size_bytes,
        })
        .collect::<Vec<_>>();
    state.app.open_sticker_picker(stickers);
    state.app.status = "sticker picker".to_string();
    Ok(())
}

fn open_attachment_modal(state: &mut UiState) {
    state.app.open_attachment_modal();
    if let Some(modal) = state.app.attachment_modal.as_mut() {
        if let Some(path) = pick_attachment_file(modal.kind) {
            modal.set_path(path);
            modal.focus = AttachmentModalField::Send;
        } else {
            modal.status = Some("choose a file below, or press o to open file picker".to_string());
        }
        refresh_attachment_picker_entries(modal);
    }
    state.app.status = "attachment picker".to_string();
}

async fn refresh_settings_modal(app_state: &AppState, state: &mut UiState) -> Result<()> {
    let profile = settings_profile::get_user_profile(app_state).await?;
    let trusted_peers = settings_peers::get_trusted_peers(app_state)?;
    let friends = settings_peers::get_friends(app_state)
        .await?
        .into_iter()
        .map(|friend| friend.username)
        .collect::<Vec<_>>();
    let pinned_peers = settings_peers::get_pinned_peers(app_state).await?;
    let connectivity = settings_connectivity::get_connectivity_settings(app_state).await?;
    let theme_presets = settings_theme::list_theme_presets(app_state)
        .await?
        .into_iter()
        .map(|preset| TuiThemePreset {
            key: preset.key,
            name: preset.name,
            source: preset.source,
        })
        .collect::<Vec<_>>();
    let selected_preset = settings_theme::get_selected_preset(app_state).await?;
    let stickers = settings_stickers::list_stickers(app_state)?
        .into_iter()
        .map(|sticker| TuiSticker {
            file_hash: sticker.file_hash,
            name: sticker.name,
            size_bytes: sticker.size_bytes,
        })
        .collect::<Vec<_>>();

    if let Some(modal) = state.app.settings.as_mut() {
        modal.profile_alias = profile.alias.unwrap_or_default();
        modal.profile_avatar_path = profile.avatar_path.unwrap_or_default();
        modal.trusted_peers = trusted_peers;
        modal.friends = friends;
        modal.pinned_peers = pinned_peers;
        modal.connectivity = connectivity;
        modal.theme_presets = theme_presets;
        modal.selected_preset = selected_preset;
        modal.stickers = stickers;
        if modal.selected_sticker_hash().is_none() {
            modal.selected_sticker_hash = None;
        }
    }
    Ok(())
}

async fn handle_settings_key(
    app_state: &AppState,
    network_state: &NetworkState,
    state: &mut UiState,
    code: KeyCode,
) -> Result<()> {
    match code {
        KeyCode::Esc => state.app.close_settings(),
        KeyCode::Tab => {
            if let Some(modal) = state.app.settings.as_mut() {
                modal.cycle_focus();
            }
        }
        KeyCode::Up => {
            if let Some(modal) = state.app.settings.as_mut() {
                match modal.pane {
                    SettingsPane::Menu => modal.move_section(-1),
                    SettingsPane::Content => modal.move_content(-1),
                }
            }
        }
        KeyCode::Down => {
            if let Some(modal) = state.app.settings.as_mut() {
                match modal.pane {
                    SettingsPane::Menu => modal.move_section(1),
                    SettingsPane::Content => modal.move_content(1),
                }
            }
        }
        KeyCode::Enter => {
            activate_settings_focus(app_state, network_state, state).await?;
        }
        KeyCode::Backspace => {
            if let Some(modal) = state.app.settings.as_mut() {
                modal.pop_char();
            }
        }
        KeyCode::Char(ch) => {
            if let Some(modal) = state.app.settings.as_mut() {
                modal.push_char(ch);
            }
        }
        _ => {}
    }
    Ok(())
}

async fn activate_settings_focus(
    app_state: &AppState,
    network_state: &NetworkState,
    state: &mut UiState,
) -> Result<()> {
    let Some(focus) = state.app.settings.as_ref().map(|modal| modal.focus.clone()) else {
        return Ok(());
    };

    match focus {
        SettingsField::Section(index) => {
            if let Some(section) = SettingsSection::ALL.get(index).copied() {
                if let Some(modal) = state.app.settings.as_mut() {
                    modal.activate_section(section);
                }
            }
        }
        SettingsField::ProfileSave => {
            let Some((alias, avatar_path)) = state.app.settings.as_ref().map(|modal| {
                (
                    modal.profile_alias.clone(),
                    modal.profile_avatar_path.clone(),
                )
            }) else {
                return Ok(());
            };
            settings_profile::update_user_profile(app_state, Some(alias), Some(avatar_path))
                .await?;
            set_settings_status(state, "profile saved");
        }
        SettingsField::ConnectivityMode(mode) => {
            if let Some(modal) = state.app.settings.as_mut() {
                modal.connectivity =
                    rchat_core::storage::config::ConnectivitySettings::from_mode(mode);
                modal.status = Some(format!("selected {}", connectivity_mode_label(mode)));
                modal.error = None;
            }
        }
        SettingsField::ConnectivitySave => {
            let mode = state
                .app
                .settings
                .as_ref()
                .map(|modal| modal.connectivity.mode)
                .unwrap_or(ConnectivityMode::Reachable);
            settings_connectivity::set_connectivity_mode(app_state, Some(network_state), mode)
                .await?;
            refresh_settings_modal(app_state, state).await?;
            set_settings_status(state, "connectivity saved");
        }
        SettingsField::ThemePreset(index) => {
            if let Some(modal) = state.app.settings.as_mut() {
                if let Some(preset) = modal.theme_presets.get(index) {
                    modal.selected_preset = Some(preset.key.clone());
                    modal.status = Some(format!("selected {}", preset.name));
                    modal.error = None;
                }
            }
        }
        SettingsField::ThemeApply => {
            let key = state
                .app
                .settings
                .as_ref()
                .and_then(|modal| modal.focused_theme_preset_key().map(ToOwned::to_owned))
                .ok_or_else(|| anyhow!("no theme preset selected"))?;
            settings_theme::apply_preset(app_state, &key).await?;
            refresh_settings_modal(app_state, state).await?;
            set_settings_status(state, "theme applied");
        }
        SettingsField::ThemeCreateCustom => {
            let Some((name, primary, secondary, text)) = state.app.settings.as_ref().map(|modal| {
                (
                    modal.theme_custom_name.clone(),
                    modal.theme_primary.clone(),
                    modal.theme_secondary.clone(),
                    modal.theme_text.clone(),
                )
            }) else {
                return Ok(());
            };
            if name.trim().is_empty() {
                set_settings_error(state, "enter a custom theme name");
                return Ok(());
            }
            let theme = settings_theme::generate_simple_theme(&primary, &secondary, &text)?;
            settings_theme::create_custom_theme(app_state, name, None, theme).await?;
            refresh_settings_modal(app_state, state).await?;
            set_settings_status(state, "custom theme created");
        }
        SettingsField::StickerImport => {
            let path = state
                .app
                .settings
                .as_ref()
                .map(|modal| modal.sticker_path.clone())
                .unwrap_or_default();
            if path.trim().is_empty() {
                set_settings_error(state, "enter a sticker image path");
                return Ok(());
            }
            settings_stickers::add_sticker(app_state, &path)?;
            refresh_settings_modal(app_state, state).await?;
            if let Some(modal) = state.app.settings.as_mut() {
                modal.sticker_path.clear();
            }
            set_settings_status(state, "sticker imported");
        }
        SettingsField::StickerDelete => {
            let hash = state
                .app
                .settings
                .as_ref()
                .and_then(|modal| modal.selected_sticker_hash().map(ToOwned::to_owned))
                .ok_or_else(|| anyhow!("no sticker selected"))?;
            settings_stickers::delete_sticker(app_state, &hash)?;
            refresh_settings_modal(app_state, state).await?;
            set_settings_status(state, "sticker deleted");
        }
        SettingsField::ProfileAlias
        | SettingsField::ProfileAvatar
        | SettingsField::Peer(_)
        | SettingsField::Friend(_)
        | SettingsField::ThemeName
        | SettingsField::ThemePrimary
        | SettingsField::ThemeSecondary
        | SettingsField::ThemeText
        | SettingsField::StickerPath => {}
        SettingsField::Sticker(index) => {
            if let Some(modal) = state.app.settings.as_mut() {
                modal.select_sticker(index);
                modal.status = Some("sticker selected".to_string());
                modal.error = None;
            }
        }
    }
    Ok(())
}

fn set_settings_status(state: &mut UiState, message: impl Into<String>) {
    if let Some(modal) = state.app.settings.as_mut() {
        modal.status = Some(message.into());
        modal.error = None;
    }
}

fn set_settings_error(state: &mut UiState, message: impl Into<String>) {
    if let Some(modal) = state.app.settings.as_mut() {
        modal.error = Some(message.into());
        modal.status = None;
    }
}

async fn handle_interactive_key(
    app_state: &AppState,
    network_state: &NetworkState,
    state: &mut UiState,
    code: KeyCode,
) -> Result<bool> {
    if state.app.media_viewer.is_some() {
        if let Err(error) = handle_media_viewer_key(app_state, network_state, state, code).await {
            if let Some(viewer) = state.app.media_viewer.as_mut() {
                viewer.error = Some(error.to_string());
                viewer.status = None;
            }
        }
        return Ok(false);
    }

    if state.app.attachment_actions.is_some() {
        if let Err(error) =
            handle_attachment_action_key(app_state, network_state, state, code).await
        {
            if let Some(modal) = state.app.attachment_actions.as_mut() {
                modal.error = Some(error.to_string());
                modal.status = None;
            }
        }
        return Ok(false);
    }

    if state.app.attachment_modal.is_some() {
        if let Err(error) = handle_attachment_modal_key(app_state, network_state, state, code).await
        {
            if let Some(modal) = state.app.attachment_modal.as_mut() {
                modal.error = Some(error.to_string());
                modal.status = None;
            }
        }
        return Ok(false);
    }

    if state.app.sticker_picker.is_some() {
        if let Err(error) = handle_sticker_picker_key(app_state, network_state, state, code).await {
            if let Some(modal) = state.app.sticker_picker.as_mut() {
                modal.error = Some(error.to_string());
                modal.status = None;
            }
        }
        return Ok(false);
    }

    if state.app.settings.is_some() {
        if let Err(error) = handle_settings_key(app_state, network_state, state, code).await {
            set_settings_error(state, error.to_string());
        }
        return Ok(false);
    }

    if state.app.new_person.is_some() {
        handle_new_person_key(app_state, network_state, state, code).await?;
        return Ok(false);
    }

    if state.app.context_menu.is_some() {
        handle_context_menu_key(app_state, network_state, state, code).await?;
        return Ok(false);
    }

    if state.app.show_command_palette {
        match code {
            KeyCode::Esc => state.app.close_command_palette(),
            KeyCode::Enter => {
                let input = state.app.command_input.clone();
                if let Err(error) =
                    execute_palette_command(app_state, network_state, state, &input).await
                {
                    state.app.last_error = Some(error.to_string());
                    state.app.status = "command failed".to_string();
                }
                state.app.close_command_palette();
            }
            KeyCode::Backspace => {
                state.app.command_input.pop();
            }
            KeyCode::Char(ch) => state.app.command_input.push(ch),
            _ => {}
        }
        return Ok(false);
    }

    if state.app.sidebar_search_active && state.app.focus == FocusPane::Chats {
        match code {
            KeyCode::Esc => state.app.close_sidebar_search(),
            KeyCode::Backspace => state.app.pop_sidebar_search_char(),
            KeyCode::Char(ch) => state.app.push_sidebar_search_char(ch),
            KeyCode::Enter | KeyCode::Up | KeyCode::Down | KeyCode::Tab => {}
            _ => {}
        }
        if !matches!(code, KeyCode::Enter | KeyCode::Up | KeyCode::Down | KeyCode::Tab) {
            return Ok(false);
        }
    }

    if let Some(ch) = composer_printable_char(state.app.focus, code) {
        state.app.composer.push(ch);
        return Ok(false);
    }

    match code {
        KeyCode::Esc if state.app.chat_details.is_some() || state.app.show_help => {
            state.app.close_modal();
        }
        KeyCode::Char('a') if should_render_incoming_screen_share_prompt(state) => {
            accept_incoming_screen_share(network_state, state).await?;
        }
        KeyCode::Char('r') if should_render_incoming_screen_share_prompt(state) => {
            reject_incoming_screen_share(network_state, state).await?;
        }
        KeyCode::Char('e') if state.broadcast_state.phase != BroadcastPhase::Idle => {
            end_current_screen_share(network_state, state).await?;
        }
        KeyCode::Char('q') => return Ok(true),
        KeyCode::Char('n') => {
            state.app.open_new_person();
        }
        KeyCode::Char('s') => {
            if let Err(error) = open_settings_modal(app_state, state).await {
                state.app.last_error = Some(error.to_string());
            }
        }
        KeyCode::Char('m')
            if matches!(state.app.focus, FocusPane::Chats | FocusPane::History) =>
        {
            open_context_menu_for_focus(state);
        }
        KeyCode::Char('/') if state.app.focus != FocusPane::Composer => {
            state.app.open_sidebar_search();
        }
        KeyCode::Char('?') => state.app.show_help = !state.app.show_help,
        KeyCode::Tab => state.app.cycle_focus(),
        KeyCode::PageUp => state.app.scroll_history(5),
        KeyCode::PageDown => state.app.scroll_history(-5),
        KeyCode::Left if state.app.focus == FocusPane::ComposerActions => {
            state.app.move_composer_action(-1);
        }
        KeyCode::Right if state.app.focus == FocusPane::ComposerActions => {
            state.app.move_composer_action(1);
        }
        KeyCode::Left if state.app.focus == FocusPane::History => {
            state.app.move_attachment_selection(-1);
        }
        KeyCode::Right if state.app.focus == FocusPane::History => {
            state.app.move_attachment_selection(1);
        }
        KeyCode::Char('v') if state.app.focus == FocusPane::History => {
            open_media_viewer_for_selected(app_state, state)?;
        }
        KeyCode::Up => {
            let was_chats = state.app.focus == FocusPane::Chats;
            state.app.move_selection(-1);
            if was_chats {
                state.sidebar_follow_selection = true;
            }
        }
        KeyCode::Down => {
            let was_chats = state.app.focus == FocusPane::Chats;
            state.app.move_selection(1);
            if was_chats {
                state.sidebar_follow_selection = true;
            }
        }
        KeyCode::Enter => match state.app.focus {
            FocusPane::Chats => {
                if let Some(chat_id) = state.app.selected_chat_id().map(ToOwned::to_owned) {
                    open_chat_list_item(app_state, network_state, state, &chat_id).await?;
                }
            }
            FocusPane::History => {
                open_attachment_actions_for_selected(app_state, state)?;
            }
            FocusPane::ComposerActions => {
                activate_composer_action(app_state, network_state, state).await?;
            }
            FocusPane::Composer => send_composer(app_state, network_state, state).await?,
            FocusPane::CommandPalette => {}
        },
        KeyCode::Backspace if state.app.focus == FocusPane::Composer => {
            state.app.composer.pop();
        }
        _ => {}
    }

    Ok(false)
}

fn composer_printable_char(focus: FocusPane, code: KeyCode) -> Option<char> {
    match code {
        KeyCode::Char(ch) if focus == FocusPane::Composer => Some(ch),
        _ => None,
    }
}

fn composer_action_click_index(area: Rect, column: u16, row: u16) -> Option<usize> {
    let inner = inset_rect(area, 1);
    if !rect_contains(inner, column, row) || inner.width == 0 {
        return None;
    }
    let relative = column.saturating_sub(inner.x) as usize;
    let width = inner.width.max(1) as usize;
    let index = relative
        .saturating_mul(ComposerAction::ALL.len())
        .checked_div(width)
        .unwrap_or_default()
        .min(ComposerAction::ALL.len().saturating_sub(1));
    Some(index)
}

async fn activate_composer_action(
    app_state: &AppState,
    network_state: &NetworkState,
    state: &mut UiState,
) -> Result<()> {
    match state.app.selected_composer_action() {
        ComposerAction::Attach => {
            open_attachment_modal(state);
            Ok(())
        }
        ComposerAction::Stickers => {
            open_sticker_picker(app_state, state)?;
            Ok(())
        }
        ComposerAction::Voice => toggle_voice_call(network_state, state).await,
        ComposerAction::Video => toggle_video_call(network_state, state).await,
        ComposerAction::Screen => toggle_screen_share(network_state, state).await,
        ComposerAction::Details => {
            let chat_id = state
                .app
                .active_chat_id
                .clone()
                .or_else(|| state.app.selected_chat_id().map(ToOwned::to_owned))
                .ok_or_else(|| anyhow!("select a chat first"))?;
            open_chat_details(app_state, network_state, state, &chat_id).await
        }
    }
}

async fn toggle_voice_call(network_state: &NetworkState, state: &mut UiState) -> Result<()> {
    if state.voice_call_state.call_kind == Some(CallKind::Voice)
        && state.voice_call_state.phase != VoiceCallPhase::Idle
    {
        let call_id = voice_call_id(state, None)?;
        send_network_command(
            network_state,
            NetworkCommand::EndVoiceCall {
                call_id: call_id.clone(),
            },
        )
        .await?;
        state.app.status = format!("voice call ending {call_id}");
        return Ok(());
    }

    let peer_id = voice_call_target_chat_id(state, None)?;
    send_network_command(
        network_state,
        NetworkCommand::StartVoiceCall {
            peer_id: peer_id.clone(),
        },
    )
    .await?;
    state.app.status = format!("voice call requested {peer_id}");
    Ok(())
}

async fn toggle_video_call(network_state: &NetworkState, state: &mut UiState) -> Result<()> {
    if state.voice_call_state.call_kind == Some(CallKind::Video)
        && state.voice_call_state.phase != VoiceCallPhase::Idle
    {
        let call_id = voice_call_id(state, None)?;
        send_network_command(
            network_state,
            NetworkCommand::EndVideoCall {
                call_id: call_id.clone(),
            },
        )
        .await?;
        state.app.status = format!("video call ending {call_id}");
        return Ok(());
    }

    let peer_id = live_call_target_chat_id(state, None, "video")?;
    send_network_command(
        network_state,
        NetworkCommand::StartVideoCall {
            peer_id: peer_id.clone(),
        },
    )
    .await?;
    state.app.status = format!("video call requested {peer_id}");
    Ok(())
}

async fn toggle_screen_share(network_state: &NetworkState, state: &mut UiState) -> Result<()> {
    if state.broadcast_state.phase != BroadcastPhase::Idle {
        return end_current_screen_share(network_state, state).await;
    }

    let peer_id = screen_share_target_chat_id(state, None)?;
    let profile = ScreenCaptureProfile::P720F15;
    send_network_command(
        network_state,
        NetworkCommand::StartScreenBroadcast {
            peer_id: peer_id.clone(),
            profile,
        },
    )
    .await?;
    state.app.status = format!("screen share requested {peer_id} {}", profile.label());
    Ok(())
}

async fn accept_incoming_screen_share(
    network_state: &NetworkState,
    state: &mut UiState,
) -> Result<()> {
    let session_id = screen_share_session_id(state, None)?;
    send_network_command(
        network_state,
        NetworkCommand::AcceptScreenBroadcast {
            session_id: session_id.clone(),
        },
    )
    .await?;
    state.app.status = format!("screen share accepted {session_id}");
    Ok(())
}

async fn reject_incoming_screen_share(
    network_state: &NetworkState,
    state: &mut UiState,
) -> Result<()> {
    let session_id = screen_share_session_id(state, None)?;
    send_network_command(
        network_state,
        NetworkCommand::RejectScreenBroadcast {
            session_id: session_id.clone(),
        },
    )
    .await?;
    state.app.status = format!("screen share rejected {session_id}");
    Ok(())
}

async fn end_current_screen_share(network_state: &NetworkState, state: &mut UiState) -> Result<()> {
    let session_id = screen_share_session_id(state, None)?;
    send_network_command(
        network_state,
        NetworkCommand::EndScreenBroadcast {
            session_id: session_id.clone(),
        },
    )
    .await?;
    state.app.status = format!("screen share ending {session_id}");
    Ok(())
}

async fn handle_mouse_event(
    app_state: &AppState,
    network_state: &NetworkState,
    state: &mut UiState,
    mouse: MouseEvent,
    size: Size,
) -> Result<()> {
    if state.app.new_person.is_some() {
        return handle_new_person_mouse(app_state, network_state, state, mouse, size).await;
    }

    if state.app.settings.is_some() {
        return Ok(());
    }

    if state.app.attachment_modal.is_some()
        || state.app.sticker_picker.is_some()
        || state.app.attachment_actions.is_some()
    {
        return Ok(());
    }

    if state.app.context_menu.is_some() {
        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left | MouseButton::Right)) {
            state.app.context_menu = None;
        }
        return Ok(());
    }

    if state.app.show_command_palette {
        return Ok(());
    }

    let layout = app_layout(Rect {
        x: 0,
        y: 0,
        width: size.width,
        height: size.height,
    });

    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if rect_contains(layout.composer_actions, mouse.column, mouse.row) {
                state.app.focus = FocusPane::ComposerActions;
                if let Some(index) =
                    composer_action_click_index(layout.composer_actions, mouse.column, mouse.row)
                {
                    state.app.selected_composer_action_index = index;
                    activate_composer_action(app_state, network_state, state).await?;
                }
                return Ok(());
            }

            if rect_contains(layout.composer, mouse.column, mouse.row) {
                state.app.focus = FocusPane::Composer;
                return Ok(());
            }

            if rect_contains(layout.sidebar, mouse.column, mouse.row) {
                match sidebar_click_target(state, layout.sidebar, mouse.column, mouse.row) {
                    Some(MouseHitTarget::Chat(index)) => {
                        state.app.focus = FocusPane::Chats;
                        state.app.selected_chat_index = index;
                        state.sidebar_follow_selection = true;
                        if let Some(chat_id) = state.app.selected_chat_id().map(ToOwned::to_owned) {
                            open_chat_list_item(app_state, network_state, state, &chat_id).await?;
                        }
                    }
                    Some(MouseHitTarget::Envelope(_)) => {
                        state.app.focus = FocusPane::Chats;
                    }
                    None => {
                        state.app.focus = FocusPane::Chats;
                    }
                }
                return Ok(());
            }

            if rect_contains(layout.chat_history, mouse.column, mouse.row) {
                state.app.focus = FocusPane::History;
            }
        }
        MouseEventKind::Down(MouseButton::Right) => {
            if rect_contains(layout.sidebar, mouse.column, mouse.row) {
                state.app.focus = FocusPane::Chats;
                match sidebar_click_target(state, layout.sidebar, mouse.column, mouse.row) {
                    Some(MouseHitTarget::Chat(index)) => {
                        state.app.selected_chat_index = index;
                        state.app.context_menu =
                            Some(ContextMenuState::new(ContextMenuTarget::Chat(index)));
                    }
                    Some(MouseHitTarget::Envelope(id)) => {
                        state.app.context_menu =
                            Some(ContextMenuState::new(ContextMenuTarget::Envelope(id)));
                    }
                    None => {}
                }
            } else if rect_contains(layout.chat_history, mouse.column, mouse.row) {
                state.app.focus = FocusPane::History;
                if let Some(message_id) = state.app.selected_message_id.clone() {
                    state.app.context_menu =
                        Some(ContextMenuState::new(ContextMenuTarget::Message(message_id)));
                }
            }
        }
        MouseEventKind::ScrollUp => {
            if rect_contains(layout.chat_history, mouse.column, mouse.row) {
                state.app.focus = FocusPane::History;
                state.app.scroll_history(3);
            } else if rect_contains(layout.sidebar, mouse.column, mouse.row) {
                state.app.focus = FocusPane::Chats;
                state.sidebar_follow_selection = false;
                scroll_sidebar_rows(state, -3, sidebar_visible_row_capacity(layout.sidebar));
            }
        }
        MouseEventKind::ScrollDown => {
            if rect_contains(layout.chat_history, mouse.column, mouse.row) {
                state.app.focus = FocusPane::History;
                state.app.scroll_history(-3);
            } else if rect_contains(layout.sidebar, mouse.column, mouse.row) {
                state.app.focus = FocusPane::Chats;
                state.sidebar_follow_selection = false;
                scroll_sidebar_rows(state, 3, sidebar_visible_row_capacity(layout.sidebar));
            }
        }
        _ => {}
    }

    Ok(())
}

async fn handle_new_person_mouse(
    app_state: &AppState,
    network_state: &NetworkState,
    state: &mut UiState,
    mouse: MouseEvent,
    size: Size,
) -> Result<()> {
    let MouseEventKind::Down(MouseButton::Left) = mouse.kind else {
        return Ok(());
    };
    let area = Rect {
        x: 0,
        y: 0,
        width: size.width,
        height: size.height,
    };
    let Some(field) = new_person_click_target(state, area, mouse.column, mouse.row) else {
        return Ok(());
    };
    let is_text = new_person_field_is_text(&field);
    if let Some(modal) = state.app.new_person.as_mut() {
        modal.focus = field;
    }
    if !is_text {
        activate_new_person_focus(app_state, network_state, state).await?;
    }
    Ok(())
}

fn new_person_field_is_text(field: &NewPersonField) -> bool {
    matches!(
        field,
        NewPersonField::InviteeUsername
            | NewPersonField::InviterUsername
            | NewPersonField::InvitePassword
            | NewPersonField::InviteQrPath
            | NewPersonField::TemporaryLink
            | NewPersonField::TemporaryQrPath
    )
}

fn new_person_click_target(
    state: &UiState,
    area: Rect,
    column: u16,
    row: u16,
) -> Option<NewPersonField> {
    let modal = state.app.new_person.as_ref()?;
    let popup = centered_rect(92, 28, area);
    let inner = inset_rect(popup, 1);
    let left = Rect {
        x: inner.x,
        y: inner.y,
        width: inner
            .width
            .saturating_sub(NEW_PERSON_QR_WIDTH.saturating_add(2)),
        height: inner.height,
    };
    if !rect_contains(left, column, row) {
        return None;
    }
    let line = row.saturating_sub(left.y) as usize;
    match modal.step {
        NewPersonStep::SelectNetwork => match line {
            4 => Some(NewPersonField::LocalNetwork),
            5 => Some(NewPersonField::OnlineNetwork),
            _ => None,
        },
        NewPersonStep::LocalScan => {
            if line >= 4 {
                let index = line - 4;
                if index < state.app.local_peers.len() {
                    Some(NewPersonField::LocalPeer(index))
                } else {
                    None
                }
            } else {
                None
            }
        }
        NewPersonStep::Online => match line {
            4 => Some(NewPersonField::CreateInvite),
            5 => Some(NewPersonField::AcceptInvite),
            6 => Some(NewPersonField::TemporaryChat),
            _ => None,
        },
        NewPersonStep::CreateInviteUser => match line {
            5 => Some(NewPersonField::InviteeUsername),
            7 => Some(NewPersonField::CreateInviteNext),
            _ => None,
        },
        NewPersonStep::CreateInviteCode => match line {
            8 => Some(NewPersonField::CreateInviteConfirm),
            _ => None,
        },
        NewPersonStep::AcceptInviteUser => match line {
            5 => Some(NewPersonField::InviterUsername),
            7 => Some(NewPersonField::AcceptInviteNext),
            _ => None,
        },
        NewPersonStep::AcceptInviteCode => match line {
            5 => Some(NewPersonField::InvitePassword),
            6 => Some(NewPersonField::InviteQrPath),
            7 => Some(NewPersonField::DecodeInviteQr),
            8 => Some(NewPersonField::RedeemInvite),
            _ => None,
        },
        NewPersonStep::TemporaryChat => temporary_chat_click_target(modal, line),
    }
}

fn temporary_chat_click_target(
    modal: &crate::state::NewPersonModalState,
    line: usize,
) -> Option<NewPersonField> {
    let mut current = 4;
    if line == current {
        return Some(NewPersonField::CreateTemporary);
    }
    current += if modal.active_temporary_link.is_some() {
        5
    } else {
        2
    };
    match line {
        value if value == current => Some(NewPersonField::TemporaryLink),
        value if value == current + 1 => Some(NewPersonField::TemporaryQrPath),
        value if value == current + 2 => Some(NewPersonField::DecodeTemporaryQr),
        value if value == current + 3 => Some(NewPersonField::RedeemTemporary),
        value if value == current + 4 => Some(NewPersonField::CancelTemporary),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum MouseHitTarget {
    Chat(usize),
    Envelope(String),
}

fn sidebar_click_target(
    state: &UiState,
    sidebar: Rect,
    column: u16,
    row: u16,
) -> Option<MouseHitTarget> {
    let inner = inset_rect(sidebar, 1);
    if !rect_contains(inner, column, row) {
        return None;
    }

    let relative_row = row.saturating_sub(inner.y);
    if relative_row == 0 {
        return None;
    }

    let row_index = state
        .app
        .sidebar_scroll_offset
        .saturating_add(relative_row.saturating_sub(1) as usize);
    match sidebar_rows(state).get(row_index) {
        Some(SidebarRow::Chat(index)) => Some(MouseHitTarget::Chat(*index)),
        Some(SidebarRow::Envelope { id, .. }) => Some(MouseHitTarget::Envelope(id.clone())),
        None => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PaletteCommand {
    Refresh,
    Open {
        chat_id: String,
    },
    Details {
        chat_id: Option<String>,
    },
    Connect {
        peer_id: String,
    },
    EnvelopeCreate {
        id: String,
        name: String,
    },
    EnvelopeRename {
        id: String,
        name: String,
    },
    EnvelopeDelete {
        id: String,
    },
    MoveChat {
        chat_id: String,
        envelope_id: Option<String>,
    },
    InviteCreate {
        invitee: String,
        password: String,
    },
    InviteRedeem {
        inviter: String,
        password: String,
    },
    AttachModal,
    Attach {
        kind: chat_media::MediaKind,
        path: String,
    },
    StickerPicker,
    Sticker {
        file_hash: String,
    },
    Save {
        file_hash: String,
        target_path: String,
    },
    View {
        file_hash: String,
    },
    OpenAttachment {
        file_hash: String,
    },
    CopyHash {
        file_hash: String,
    },
    Retry {
        file_hash: String,
    },
    VoiceCallStart {
        peer_id: Option<String>,
    },
    VoiceCallAccept {
        call_id: Option<String>,
    },
    VoiceCallReject {
        call_id: Option<String>,
    },
    VoiceCallEnd {
        call_id: Option<String>,
    },
    VoiceCallMute {
        call_id: Option<String>,
        muted: bool,
    },
    VideoCallStart {
        peer_id: Option<String>,
    },
    VideoCallAccept {
        call_id: Option<String>,
    },
    VideoCallReject {
        call_id: Option<String>,
    },
    VideoCallEnd {
        call_id: Option<String>,
    },
    VideoCallMute {
        call_id: Option<String>,
        muted: bool,
    },
    VideoCallCamera {
        call_id: Option<String>,
        enabled: bool,
    },
    ScreenShareStart {
        chat_id: Option<String>,
        profile: ScreenCaptureProfile,
    },
    ScreenShareAccept {
        session_id: Option<String>,
    },
    ScreenShareReject {
        session_id: Option<String>,
    },
    ScreenShareEnd {
        session_id: Option<String>,
    },
    GroupInviteAccept {
        invite_id: String,
    },
    GroupInviteReject {
        invite_id: String,
    },
    Help,
}

fn parse_palette_command(input: &str) -> std::result::Result<PaletteCommand, String> {
    let mut parts = input.split_whitespace();
    let Some(command) = parts.next() else {
        return Err("enter a command".to_string());
    };
    match command {
        "refresh" | "r" => Ok(PaletteCommand::Refresh),
        "open" => {
            let chat_id = parts.collect::<Vec<_>>().join(" ");
            if chat_id.is_empty() {
                Err("usage: open <chat-id>".to_string())
            } else if looks_like_file_hash(&chat_id) {
                Ok(PaletteCommand::OpenAttachment { file_hash: chat_id })
            } else {
                Ok(PaletteCommand::Open { chat_id })
            }
        }
        "chat" => {
            let chat_id = parts.collect::<Vec<_>>().join(" ");
            if chat_id.is_empty() {
                Err("usage: chat <chat-id>".to_string())
            } else {
                Ok(PaletteCommand::Open { chat_id })
            }
        }
        "connect" => parts
            .next()
            .map(|peer_id| PaletteCommand::Connect {
                peer_id: peer_id.to_string(),
            })
            .ok_or_else(|| "usage: connect <peer-id>".to_string()),
        "details" | "info" => {
            let chat_id = parts.collect::<Vec<_>>().join(" ");
            Ok(PaletteCommand::Details {
                chat_id: if chat_id.trim().is_empty() {
                    None
                } else {
                    Some(chat_id)
                },
            })
        }
        "envelope" | "env" => parse_envelope_palette_command(parts.collect()),
        "move" => {
            let chat_id = parts
                .next()
                .ok_or_else(|| "usage: move <chat-id> <envelope-id|root>".to_string())?;
            let envelope = parts
                .next()
                .ok_or_else(|| "usage: move <chat-id> <envelope-id|root>".to_string())?;
            let envelope_id = match envelope {
                "root" | "none" | "-" => None,
                other => Some(other.to_string()),
            };
            Ok(PaletteCommand::MoveChat {
                chat_id: chat_id.to_string(),
                envelope_id,
            })
        }
        "invite" => match (parts.next(), parts.next(), parts.next()) {
            (Some("create"), Some(invitee), Some(password)) => Ok(PaletteCommand::InviteCreate {
                invitee: invitee.to_string(),
                password: password.to_string(),
            }),
            (Some("redeem"), Some(inviter), Some(password)) => Ok(PaletteCommand::InviteRedeem {
                inviter: inviter.to_string(),
                password: password.to_string(),
            }),
            _ => Err("usage: invite create|redeem <github-user> <password>".to_string()),
        },
        "attach" => parse_attach_palette_command(parts.collect()),
        "sticker" => {
            let file_hash = parts.collect::<Vec<_>>().join(" ");
            if file_hash.trim().is_empty() {
                Ok(PaletteCommand::StickerPicker)
            } else {
                Ok(PaletteCommand::Sticker { file_hash })
            }
        }
        "save" => {
            let file_hash = parts
                .next()
                .ok_or_else(|| "usage: save <file-hash> <target-path>".to_string())?;
            let target_path = parts.collect::<Vec<_>>().join(" ");
            if target_path.trim().is_empty() {
                Err("usage: save <file-hash> <target-path>".to_string())
            } else {
                Ok(PaletteCommand::Save {
                    file_hash: file_hash.to_string(),
                    target_path,
                })
            }
        }
        "view" => {
            let file_hash = parts.collect::<Vec<_>>().join(" ");
            if file_hash.trim().is_empty() {
                Err("usage: view <file-hash>".to_string())
            } else {
                Ok(PaletteCommand::View { file_hash })
            }
        }
        "open-attachment" | "open-file" => {
            let file_hash = parts.collect::<Vec<_>>().join(" ");
            if file_hash.trim().is_empty() {
                Err("usage: open-attachment <file-hash>".to_string())
            } else {
                Ok(PaletteCommand::OpenAttachment { file_hash })
            }
        }
        "copy-hash" => {
            let file_hash = parts.collect::<Vec<_>>().join(" ");
            if file_hash.trim().is_empty() {
                Err("usage: copy-hash <file-hash>".to_string())
            } else {
                Ok(PaletteCommand::CopyHash { file_hash })
            }
        }
        "retry" => {
            let file_hash = parts.collect::<Vec<_>>().join(" ");
            if file_hash.trim().is_empty() {
                Err("usage: retry <file-hash>".to_string())
            } else {
                Ok(PaletteCommand::Retry { file_hash })
            }
        }
        "voice" | "call" => parse_voice_palette_command(parts.collect()),
        "video" => parse_video_palette_command(parts.collect()),
        "screen" | "screen-share" | "share" => parse_screen_share_palette_command(parts.collect()),
        "group-invite" | "group-inv" => parse_group_invite_palette_command(parts.collect()),
        "help" | "?" => Ok(PaletteCommand::Help),
        other => Err(format!("unknown command: {other}")),
    }
}

fn parse_group_invite_palette_command(
    parts: Vec<&str>,
) -> std::result::Result<PaletteCommand, String> {
    let Some(action) = parts.first().copied() else {
        return Err("usage: group-invite accept|reject <invite-id>".to_string());
    };
    let invite_id = joined_arg(&parts[1..])
        .ok_or_else(|| "usage: group-invite accept|reject <invite-id>".to_string())?;
    match action {
        "accept" => Ok(PaletteCommand::GroupInviteAccept { invite_id }),
        "reject" => Ok(PaletteCommand::GroupInviteReject { invite_id }),
        _ => Err("usage: group-invite accept|reject <invite-id>".to_string()),
    }
}

fn parse_voice_palette_command(parts: Vec<&str>) -> std::result::Result<PaletteCommand, String> {
    let Some(action) = parts.first().copied() else {
        return Err("usage: voice start|accept|reject|end|mute".to_string());
    };
    match action {
        "start" => Ok(PaletteCommand::VoiceCallStart {
            peer_id: joined_arg(&parts[1..]),
        }),
        "accept" => Ok(PaletteCommand::VoiceCallAccept {
            call_id: joined_arg(&parts[1..]),
        }),
        "reject" => Ok(PaletteCommand::VoiceCallReject {
            call_id: joined_arg(&parts[1..]),
        }),
        "end" => Ok(PaletteCommand::VoiceCallEnd {
            call_id: joined_arg(&parts[1..]),
        }),
        "mute" => parse_voice_mute_palette_command(&parts[1..]),
        _ => Err("usage: voice start|accept|reject|end|mute".to_string()),
    }
}

fn parse_voice_mute_palette_command(parts: &[&str]) -> std::result::Result<PaletteCommand, String> {
    let Some(value) = parts.first().copied() else {
        return Ok(PaletteCommand::VoiceCallMute {
            call_id: None,
            muted: true,
        });
    };
    let muted = match value {
        "on" | "true" | "yes" | "1" => true,
        "off" | "false" | "no" | "0" => false,
        _ => return Err("usage: voice mute [on|off] [call-id]".to_string()),
    };
    Ok(PaletteCommand::VoiceCallMute {
        call_id: joined_arg(&parts[1..]),
        muted,
    })
}

fn parse_video_palette_command(parts: Vec<&str>) -> std::result::Result<PaletteCommand, String> {
    let Some(action) = parts.first().copied() else {
        return Err("usage: video start|accept|reject|end|mute|camera".to_string());
    };
    match action {
        "start" => Ok(PaletteCommand::VideoCallStart {
            peer_id: joined_arg(&parts[1..]),
        }),
        "accept" => Ok(PaletteCommand::VideoCallAccept {
            call_id: joined_arg(&parts[1..]),
        }),
        "reject" => Ok(PaletteCommand::VideoCallReject {
            call_id: joined_arg(&parts[1..]),
        }),
        "end" => Ok(PaletteCommand::VideoCallEnd {
            call_id: joined_arg(&parts[1..]),
        }),
        "mute" => parse_video_mute_palette_command(&parts[1..]),
        "camera" => parse_video_camera_palette_command(&parts[1..]),
        _ => Err("usage: video start|accept|reject|end|mute|camera".to_string()),
    }
}

fn parse_video_mute_palette_command(parts: &[&str]) -> std::result::Result<PaletteCommand, String> {
    let Some(value) = parts.first().copied() else {
        return Ok(PaletteCommand::VideoCallMute {
            call_id: None,
            muted: true,
        });
    };
    let muted = match value {
        "on" | "true" | "yes" | "1" => true,
        "off" | "false" | "no" | "0" => false,
        _ => return Err("usage: video mute [on|off] [call-id]".to_string()),
    };
    Ok(PaletteCommand::VideoCallMute {
        call_id: joined_arg(&parts[1..]),
        muted,
    })
}

fn parse_video_camera_palette_command(
    parts: &[&str],
) -> std::result::Result<PaletteCommand, String> {
    let Some(value) = parts.first().copied() else {
        return Err("usage: video camera <on|off> [call-id]".to_string());
    };
    let enabled = match value {
        "on" | "true" | "yes" | "1" => true,
        "off" | "false" | "no" | "0" => false,
        _ => return Err("usage: video camera <on|off> [call-id]".to_string()),
    };
    Ok(PaletteCommand::VideoCallCamera {
        call_id: joined_arg(&parts[1..]),
        enabled,
    })
}

fn parse_screen_share_palette_command(
    parts: Vec<&str>,
) -> std::result::Result<PaletteCommand, String> {
    let Some(action) = parts.first().copied() else {
        return Err(
            "usage: screen start [profile] [chat-id] | accept|reject|end [session-id]".to_string(),
        );
    };
    match action {
        "start" => parse_screen_share_start_palette_command(&parts[1..]),
        "accept" => Ok(PaletteCommand::ScreenShareAccept {
            session_id: joined_arg(&parts[1..]),
        }),
        "reject" => Ok(PaletteCommand::ScreenShareReject {
            session_id: joined_arg(&parts[1..]),
        }),
        "end" => Ok(PaletteCommand::ScreenShareEnd {
            session_id: joined_arg(&parts[1..]),
        }),
        _ => Err(
            "usage: screen start [profile] [chat-id] | accept|reject|end [session-id]".to_string(),
        ),
    }
}

fn parse_screen_share_start_palette_command(
    parts: &[&str],
) -> std::result::Result<PaletteCommand, String> {
    let default_profile = ScreenCaptureProfile::P720F15;
    let Some(first) = parts.first().copied() else {
        return Ok(PaletteCommand::ScreenShareStart {
            chat_id: None,
            profile: default_profile,
        });
    };

    if let Some(profile) = ScreenCaptureProfile::from_label(first) {
        return Ok(PaletteCommand::ScreenShareStart {
            chat_id: joined_arg(&parts[1..]),
            profile,
        });
    }

    Ok(PaletteCommand::ScreenShareStart {
        chat_id: joined_arg(parts),
        profile: default_profile,
    })
}

fn joined_arg(parts: &[&str]) -> Option<String> {
    let value = parts.join(" ");
    (!value.trim().is_empty()).then_some(value)
}

fn parse_attach_palette_command(parts: Vec<&str>) -> std::result::Result<PaletteCommand, String> {
    if parts.is_empty() {
        return Ok(PaletteCommand::AttachModal);
    }
    let kind = match parts[0] {
        "image" => chat_media::MediaKind::Image,
        "document" | "doc" | "file" => chat_media::MediaKind::Document,
        "video" => chat_media::MediaKind::Video,
        "audio" => chat_media::MediaKind::Audio,
        _ => return Err("usage: attach [image|document|video|audio] <path>".to_string()),
    };
    let path = parts[1..].join(" ");
    if path.trim().is_empty() {
        return Err("usage: attach [image|document|video|audio] <path>".to_string());
    }
    Ok(PaletteCommand::Attach { kind, path })
}

fn looks_like_file_hash(value: &str) -> bool {
    let value = value.trim();
    value.len() >= 32 && value.chars().all(|ch| ch.is_ascii_hexdigit())
}

fn parse_envelope_palette_command(parts: Vec<&str>) -> std::result::Result<PaletteCommand, String> {
    match parts.as_slice() {
        ["create", id, name @ ..] if !name.is_empty() => Ok(PaletteCommand::EnvelopeCreate {
            id: (*id).to_string(),
            name: name.join(" "),
        }),
        ["rename", id, name @ ..] if !name.is_empty() => Ok(PaletteCommand::EnvelopeRename {
            id: (*id).to_string(),
            name: name.join(" "),
        }),
        ["delete", id] => Ok(PaletteCommand::EnvelopeDelete {
            id: (*id).to_string(),
        }),
        _ => {
            Err("usage: envelope create <id> <name> | rename <id> <name> | delete <id>".to_string())
        }
    }
}

async fn execute_palette_command(
    app_state: &AppState,
    network_state: &NetworkState,
    state: &mut UiState,
    input: &str,
) -> Result<()> {
    match parse_palette_command(input) {
        Ok(PaletteCommand::Refresh) => refresh_direct_chats(app_state, network_state, state).await,
        Ok(PaletteCommand::Open { chat_id }) => {
            open_chat_list_item(app_state, network_state, state, &chat_id).await
        }
        Ok(PaletteCommand::Details { chat_id }) => {
            let chat_id = chat_id
                .or_else(|| state.app.active_chat_id.clone())
                .or_else(|| state.app.selected_chat_id().map(ToOwned::to_owned))
                .ok_or_else(|| anyhow!("select a chat first"))?;
            open_chat_details(app_state, network_state, state, &chat_id).await
        }
        Ok(PaletteCommand::Connect { peer_id }) => {
            send_network_command(network_state, NetworkCommand::RequestConnection { peer_id })
                .await?;
            state.app.status = "connection requested".to_string();
            Ok(())
        }
        Ok(PaletteCommand::EnvelopeCreate { id, name }) => {
            envelopes::create_envelope(app_state, &id, &name, None)?;
            refresh_direct_chats(app_state, network_state, state).await?;
            state.app.status = format!("created envelope {name}");
            Ok(())
        }
        Ok(PaletteCommand::EnvelopeRename { id, name }) => {
            envelopes::update_envelope(app_state, &id, &name, None)?;
            refresh_direct_chats(app_state, network_state, state).await?;
            state.app.status = format!("renamed envelope {name}");
            Ok(())
        }
        Ok(PaletteCommand::EnvelopeDelete { id }) => {
            envelopes::delete_envelope(app_state, &id)?;
            refresh_direct_chats(app_state, network_state, state).await?;
            state.app.status = format!("deleted envelope {id}");
            Ok(())
        }
        Ok(PaletteCommand::MoveChat {
            chat_id,
            envelope_id,
        }) => {
            envelopes::move_chat_to_envelope(app_state, &chat_id, envelope_id.as_deref())?;
            refresh_direct_chats(app_state, network_state, state).await?;
            state.app.status = match envelope_id {
                Some(envelope_id) => format!("moved {chat_id} to {envelope_id}"),
                None => format!("moved {chat_id} to root"),
            };
            Ok(())
        }
        Ok(PaletteCommand::InviteCreate { invitee, password }) => {
            direct::create_github_invite(app_state, network_state, &invitee, &password).await?;
            state.app.status = format!("invite published for {invitee}");
            Ok(())
        }
        Ok(PaletteCommand::InviteRedeem { inviter, password }) => {
            let chat_id =
                direct::redeem_github_invite(app_state, network_state, &inviter, &password).await?;
            refresh_direct_chats(app_state, network_state, state).await?;
            open_direct_chat(app_state, network_state, state, &chat_id).await?;
            state.app.status = format!("connected invite from {inviter}");
            Ok(())
        }
        Ok(PaletteCommand::AttachModal) => {
            open_attachment_modal(state);
            Ok(())
        }
        Ok(PaletteCommand::Attach { kind, path }) => {
            send_attachment_from_path(app_state, network_state, state, kind, &path).await
        }
        Ok(PaletteCommand::StickerPicker) => {
            open_sticker_picker(app_state, state)?;
            Ok(())
        }
        Ok(PaletteCommand::Sticker { file_hash }) => {
            send_sticker_hash(app_state, network_state, state, &file_hash).await
        }
        Ok(PaletteCommand::Save {
            file_hash,
            target_path,
        }) => {
            save_attachment_to_path(app_state, &file_hash, &target_path)?;
            state.app.status = "attachment saved".to_string();
            Ok(())
        }
        Ok(PaletteCommand::View { file_hash }) => {
            open_media_viewer_for_hash(app_state, state, &file_hash)?;
            state.app.status = "viewer opened".to_string();
            Ok(())
        }
        Ok(PaletteCommand::OpenAttachment { file_hash }) => {
            let path = open_attachment_external(app_state, &file_hash)?;
            state.app.status = format!("opened {}", path.display());
            Ok(())
        }
        Ok(PaletteCommand::CopyHash { file_hash }) => {
            match copy_hash_to_clipboard(&file_hash) {
                Ok(()) => state.app.status = "hash copied".to_string(),
                Err(_) => state.app.status = format!("hash {file_hash}"),
            }
            Ok(())
        }
        Ok(PaletteCommand::Retry { file_hash }) => {
            retry_attachment_fetch(app_state, network_state, state, &file_hash).await?;
            state.app.status = "attachment retry requested".to_string();
            Ok(())
        }
        Ok(PaletteCommand::VoiceCallStart { peer_id }) => {
            let peer_id = voice_call_target_chat_id(state, peer_id.as_deref())?;
            send_network_command(
                network_state,
                NetworkCommand::StartVoiceCall {
                    peer_id: peer_id.clone(),
                },
            )
            .await?;
            state.app.status = format!("voice call requested {peer_id}");
            Ok(())
        }
        Ok(PaletteCommand::VoiceCallAccept { call_id }) => {
            let call_id = voice_call_id(state, call_id.as_deref())?;
            send_network_command(
                network_state,
                NetworkCommand::AcceptVoiceCall {
                    call_id: call_id.clone(),
                },
            )
            .await?;
            state.app.status = format!("voice call accepted {call_id}");
            Ok(())
        }
        Ok(PaletteCommand::VoiceCallReject { call_id }) => {
            let call_id = voice_call_id(state, call_id.as_deref())?;
            send_network_command(
                network_state,
                NetworkCommand::RejectVoiceCall {
                    call_id: call_id.clone(),
                },
            )
            .await?;
            state.app.status = format!("voice call rejected {call_id}");
            Ok(())
        }
        Ok(PaletteCommand::VoiceCallEnd { call_id }) => {
            let call_id = voice_call_id(state, call_id.as_deref())?;
            send_network_command(
                network_state,
                NetworkCommand::EndVoiceCall {
                    call_id: call_id.clone(),
                },
            )
            .await?;
            state.app.status = format!("voice call ending {call_id}");
            Ok(())
        }
        Ok(PaletteCommand::VoiceCallMute { call_id, muted }) => {
            let call_id = voice_call_id(state, call_id.as_deref())?;
            send_network_command(
                network_state,
                NetworkCommand::SetVoiceCallMuted {
                    call_id: call_id.clone(),
                    muted,
                },
            )
            .await?;
            state.app.status = if muted {
                format!("voice call muted {call_id}")
            } else {
                format!("voice call unmuted {call_id}")
            };
            Ok(())
        }
        Ok(PaletteCommand::VideoCallStart { peer_id }) => {
            let peer_id = live_call_target_chat_id(state, peer_id.as_deref(), "video")?;
            send_network_command(
                network_state,
                NetworkCommand::StartVideoCall {
                    peer_id: peer_id.clone(),
                },
            )
            .await?;
            state.app.status = format!("video call requested {peer_id}");
            Ok(())
        }
        Ok(PaletteCommand::VideoCallAccept { call_id }) => {
            let call_id = voice_call_id(state, call_id.as_deref())?;
            send_network_command(
                network_state,
                NetworkCommand::AcceptVideoCall {
                    call_id: call_id.clone(),
                },
            )
            .await?;
            state.app.status = format!("video call accepted {call_id}");
            Ok(())
        }
        Ok(PaletteCommand::VideoCallReject { call_id }) => {
            let call_id = voice_call_id(state, call_id.as_deref())?;
            send_network_command(
                network_state,
                NetworkCommand::RejectVideoCall {
                    call_id: call_id.clone(),
                },
            )
            .await?;
            state.app.status = format!("video call rejected {call_id}");
            Ok(())
        }
        Ok(PaletteCommand::VideoCallEnd { call_id }) => {
            let call_id = voice_call_id(state, call_id.as_deref())?;
            send_network_command(
                network_state,
                NetworkCommand::EndVideoCall {
                    call_id: call_id.clone(),
                },
            )
            .await?;
            state.app.status = format!("video call ending {call_id}");
            Ok(())
        }
        Ok(PaletteCommand::VideoCallMute { call_id, muted }) => {
            let call_id = voice_call_id(state, call_id.as_deref())?;
            send_network_command(
                network_state,
                NetworkCommand::SetVideoCallMuted {
                    call_id: call_id.clone(),
                    muted,
                },
            )
            .await?;
            state.app.status = if muted {
                format!("video call muted {call_id}")
            } else {
                format!("video call unmuted {call_id}")
            };
            Ok(())
        }
        Ok(PaletteCommand::VideoCallCamera { call_id, enabled }) => {
            let call_id = voice_call_id(state, call_id.as_deref())?;
            send_network_command(
                network_state,
                NetworkCommand::SetVideoCallCameraEnabled {
                    call_id: call_id.clone(),
                    enabled,
                },
            )
            .await?;
            state.app.status = if enabled {
                format!("video camera on {call_id}")
            } else {
                format!("video camera off {call_id}")
            };
            Ok(())
        }
        Ok(PaletteCommand::ScreenShareStart { chat_id, profile }) => {
            let peer_id = screen_share_target_chat_id(state, chat_id.as_deref())?;
            send_network_command(
                network_state,
                NetworkCommand::StartScreenBroadcast {
                    peer_id: peer_id.clone(),
                    profile,
                },
            )
            .await?;
            state.app.status = format!("screen share requested {peer_id} {}", profile.label());
            Ok(())
        }
        Ok(PaletteCommand::ScreenShareAccept { session_id }) => {
            let session_id = screen_share_session_id(state, session_id.as_deref())?;
            send_network_command(
                network_state,
                NetworkCommand::AcceptScreenBroadcast {
                    session_id: session_id.clone(),
                },
            )
            .await?;
            state.app.status = format!("screen share accepted {session_id}");
            Ok(())
        }
        Ok(PaletteCommand::ScreenShareReject { session_id }) => {
            let session_id = screen_share_session_id(state, session_id.as_deref())?;
            send_network_command(
                network_state,
                NetworkCommand::RejectScreenBroadcast {
                    session_id: session_id.clone(),
                },
            )
            .await?;
            state.app.status = format!("screen share rejected {session_id}");
            Ok(())
        }
        Ok(PaletteCommand::ScreenShareEnd { session_id }) => {
            let session_id = screen_share_session_id(state, session_id.as_deref())?;
            send_network_command(
                network_state,
                NetworkCommand::EndScreenBroadcast {
                    session_id: session_id.clone(),
                },
            )
            .await?;
            state.app.status = format!("screen share ending {session_id}");
            Ok(())
        }
        Ok(PaletteCommand::GroupInviteAccept { invite_id }) => {
            let group_id =
                group::accept_invite(app_state, network_state, invite_id.clone()).await?;
            refresh_direct_chats(app_state, network_state, state).await?;
            state.app.status = format!("accepted group invite {group_id}");
            Ok(())
        }
        Ok(PaletteCommand::GroupInviteReject { invite_id }) => {
            group::reject_invite(app_state, &invite_id)?;
            state.app.status = format!("rejected group invite {invite_id}");
            Ok(())
        }
        Ok(PaletteCommand::Help) => {
            state.app.show_help = true;
            Ok(())
        }
        Err(error) => {
            state.app.last_error = Some(error);
            Ok(())
        }
    }
}

fn voice_call_target_chat_id(state: &UiState, explicit: Option<&str>) -> Result<String> {
    live_call_target_chat_id(state, explicit, "voice")
}

fn live_call_target_chat_id(
    state: &UiState,
    explicit: Option<&str>,
    media_label: &str,
) -> Result<String> {
    let chat_id = explicit
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| state.app.active_chat_id.clone())
        .or_else(|| state.app.selected_chat_id().map(ToOwned::to_owned))
        .ok_or_else(|| anyhow!("select a direct chat first"))?;
    if chat_kind::parse_chat_kind(&chat_id) != ChatKind::Direct {
        return Err(anyhow!(
            "{media_label} calls are only available for regular DM chats"
        ));
    }
    if !state.app.is_chat_connected(&chat_id) {
        return Err(anyhow!("peer is not currently connected"));
    }
    Ok(chat_id)
}

fn voice_call_id(state: &UiState, explicit: Option<&str>) -> Result<String> {
    explicit
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| state.voice_call_state.call_id.clone())
        .ok_or_else(|| anyhow!("no voice call is active"))
}

fn screen_share_target_chat_id(state: &UiState, explicit: Option<&str>) -> Result<String> {
    let chat_id = explicit
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| state.app.active_chat_id.clone())
        .or_else(|| state.app.selected_chat_id().map(ToOwned::to_owned))
        .ok_or_else(|| anyhow!("select a direct chat first"))?;
    if chat_kind::parse_chat_kind(&chat_id) != ChatKind::Direct {
        return Err(anyhow!(
            "screen share is only available for regular DM chats"
        ));
    }
    if !state.app.is_chat_connected(&chat_id) {
        return Err(anyhow!("peer is not currently connected"));
    }
    Ok(chat_id)
}

fn screen_share_session_id(state: &UiState, explicit: Option<&str>) -> Result<String> {
    explicit
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| state.broadcast_state.session_id.clone())
        .ok_or_else(|| anyhow!("no screen share session is active"))
}

fn drain_core_events(
    event_rx: &mut mpsc::Receiver<TuiEvent>,
    state: &mut UiState,
    pending_frames: &mut LatestFrameSlot<BroadcastFrameEvent>,
    pending_remote_video_frames: &mut LatestFrameSlot<VideoEncodedRemoteFrameEvent>,
    decoder: &mut ScreenFrameDecoder,
    remote_video_decoder: &mut RemoteVideoFrameDecoder,
    refresh_requested: &mut bool,
    mark_read_chat_ids: &mut Vec<String>,
) {
    while let Ok(event) = event_rx.try_recv() {
        match event {
            TuiEvent::Core(CoreEvent::ConnectedChatIdsUpdated(ids)) => {
                state.connected_chat_ids = ids.clone();
                state.app.apply_connected_chat_ids(ids);
            }
            TuiEvent::Core(CoreEvent::BroadcastStateUpdated(next)) => {
                apply_broadcast_state(state, decoder, next);
            }
            TuiEvent::Core(CoreEvent::VoiceCallStateUpdated(next)) => {
                apply_voice_call_state_with_decoder(state, remote_video_decoder, next);
            }
            TuiEvent::Core(CoreEvent::BroadcastFrame(frame)) => {
                state.received_frames = state.received_frames.saturating_add(1);
                pending_frames.push(frame);
            }
            TuiEvent::Core(CoreEvent::VideoCallEncodedRemoteFrame(frame)) => {
                state.remote_video_received_frames =
                    state.remote_video_received_frames.saturating_add(1);
                pending_remote_video_frames.push(frame);
            }
            TuiEvent::Core(CoreEvent::VideoCallCameraState(event)) => {
                if state.voice_call_state.call_id.as_deref() == Some(event.call_id.as_str()) {
                    state.remote_video_camera_enabled = Some(event.enabled);
                }
            }
            TuiEvent::Core(CoreEvent::ScreenBroadcastCaptureError(error)) => {
                state.media_error = Some(error.message);
            }
            TuiEvent::Core(CoreEvent::LocalPeerDiscovered(peer)) => {
                state.app.apply_local_peer_discovered(peer.clone());
                state.last_peer_event = Some(format!("discovered {}", peer.peer_id));
            }
            TuiEvent::Core(CoreEvent::LocalPeerExpired(peer_id)) => {
                state.app.apply_local_peer_expired(&peer_id);
                state.last_peer_event = Some(format!("expired {peer_id}"));
            }
            TuiEvent::Core(CoreEvent::MessageReceived(message)) => {
                let chat_id = message.chat_id.clone();
                let peer_id = message.peer_id.clone();
                let effect = state.app.apply_incoming_message(message);
                if effect == crate::state::IncomingMessageEffect::AppendedToActive
                    && peer_id != "Me"
                {
                    mark_read_chat_ids.push(chat_id);
                }
            }
            TuiEvent::Core(CoreEvent::MessageStatusUpdated(update)) => {
                state
                    .app
                    .apply_message_status(&update.msg_id, &update.status);
            }
            TuiEvent::Core(CoreEvent::NewGithubChat(event)) => {
                state.app.status = format!("new GitHub chat {}", event.chat_id);
                *refresh_requested = true;
            }
            TuiEvent::Core(CoreEvent::TemporaryChatConnected(event)) => {
                state.app.status = format!("temporary chat connected {}", event.chat_id);
                *refresh_requested = true;
            }
            TuiEvent::Core(CoreEvent::TemporaryChatEnded(event)) => {
                state.app.status = format!("temporary chat ended {}", event.chat_id);
                *refresh_requested = true;
            }
            TuiEvent::Core(CoreEvent::FileTransferComplete(event)) => {
                state.app.status = format!("transfer complete {}", event.file_hash);
                *refresh_requested = true;
            }
            TuiEvent::Core(CoreEvent::GroupInviteReceived(event)) => {
                state.app.status = group_invite_received_status(&event);
            }
            TuiEvent::Core(CoreEvent::ConnectionWaiting(peer_id)) => {
                state.app.status = format!("connecting to {peer_id}");
            }
            TuiEvent::Core(CoreEvent::ConnectionRequestReceived(peer_id)) => {
                state.app.status = format!("connection request from {peer_id}");
            }
            TuiEvent::Core(CoreEvent::PeerConnected(peer_id)) => {
                state.app.status = format!("connected to {peer_id}");
                if state
                    .app
                    .new_person
                    .as_ref()
                    .and_then(|modal| modal.waiting_peer_id.as_deref())
                    == Some(peer_id.as_str())
                {
                    mdns::disable_fast_discovery();
                    state.app.close_new_person();
                }
                *refresh_requested = true;
            }
            _ => {}
        }
    }
}

fn group_invite_received_status(event: &rchat_core::events::GroupInviteReceivedEvent) -> String {
    format!(
        "group invite {} from {} - open New Person to accept or reject",
        short_identifier(&event.group_name, 28),
        short_identifier(&event.inviter_peer_id, 24)
    )
}

#[cfg(test)]
fn apply_voice_call_state(state: &mut UiState, next: VoiceCallState) {
    let mut remote_video_decoder = RemoteVideoFrameDecoder::default();
    apply_voice_call_state_with_decoder(state, &mut remote_video_decoder, next);
}

fn apply_voice_call_state_with_decoder(
    state: &mut UiState,
    remote_video_decoder: &mut RemoteVideoFrameDecoder,
    next: VoiceCallState,
) {
    let previous_call_id = state.voice_call_state.call_id.clone();
    let next_call_id = next.call_id.clone();
    let reset_remote_video = next.call_kind != Some(CallKind::Video)
        || next.phase == VoiceCallPhase::Idle
        || previous_call_id != next_call_id;
    state.voice_call_state = next;
    if reset_remote_video {
        state.remote_video_protocol = None;
        state.remote_video_error = None;
        state.remote_video_camera_enabled = None;
        remote_video_decoder.clear();
    }
}

fn apply_broadcast_state(
    state: &mut UiState,
    decoder: &mut ScreenFrameDecoder,
    next: BroadcastState,
) {
    state.broadcast_state = next.clone();
    state.incoming_session_id = if next.phase == BroadcastPhase::IncomingRinging {
        next.session_id.clone()
    } else {
        None
    };
    state.active_session_id = if next.phase == BroadcastPhase::Active {
        next.session_id.clone()
    } else {
        None
    };
    if next.phase == BroadcastPhase::Idle {
        decoder.clear();
        state.protocol = None;
        state.last_protocol_seq = None;
    }
}

struct OutputRedirect {
    #[cfg(unix)]
    stdout_fd: RawFd,
    #[cfg(unix)]
    stderr_fd: RawFd,
}

impl OutputRedirect {
    fn redirect(path: &Path) -> Result<Self> {
        redirect_output_to_log(path)
    }
}

#[cfg(unix)]
fn redirect_output_to_log(path: &Path) -> Result<OutputRedirect> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create TUI log directory {}", parent.display()))?;
    }
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("failed to open TUI log {}", path.display()))?;
    let log_fd = log.as_raw_fd();

    // Keep the TUI screen clean while legacy core modules still use println!/eprintln!.
    let stdout_fd = unsafe { libc::dup(libc::STDOUT_FILENO) };
    if stdout_fd < 0 {
        return Err(io::Error::last_os_error()).context("failed to save stdout");
    }
    let stderr_fd = unsafe { libc::dup(libc::STDERR_FILENO) };
    if stderr_fd < 0 {
        unsafe {
            libc::close(stdout_fd);
        }
        return Err(io::Error::last_os_error()).context("failed to save stderr");
    }
    if unsafe { libc::dup2(log_fd, libc::STDOUT_FILENO) } < 0 {
        unsafe {
            libc::close(stdout_fd);
            libc::close(stderr_fd);
        }
        return Err(io::Error::last_os_error()).context("failed to redirect stdout");
    }
    if unsafe { libc::dup2(log_fd, libc::STDERR_FILENO) } < 0 {
        unsafe {
            libc::dup2(stdout_fd, libc::STDOUT_FILENO);
            libc::close(stdout_fd);
            libc::close(stderr_fd);
        }
        return Err(io::Error::last_os_error()).context("failed to redirect stderr");
    }

    Ok(OutputRedirect {
        stdout_fd,
        stderr_fd,
    })
}

#[cfg(not(unix))]
fn redirect_output_to_log(_path: &Path) -> Result<OutputRedirect> {
    Ok(OutputRedirect {})
}

impl Drop for OutputRedirect {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            let _ = libc::dup2(self.stdout_fd, libc::STDOUT_FILENO);
            let _ = libc::dup2(self.stderr_fd, libc::STDERR_FILENO);
            let _ = libc::close(self.stdout_fd);
            let _ = libc::close(self.stderr_fd);
        }
    }
}

struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Box<dyn Write>>>,
}

impl TerminalSession {
    fn enter() -> Result<Self> {
        enable_raw_mode().context("failed to enable raw mode")?;
        let mut output = terminal_output()?;
        execute!(&mut output, EnterAlternateScreen, EnableMouseCapture)
            .context("failed to enter alternate screen")?;
        let backend = CrosstermBackend::new(output);
        let terminal = Terminal::new(backend).context("failed to start terminal")?;
        let mut session = Self { terminal };
        session.clear()?;
        Ok(session)
    }

    fn draw<F>(&mut self, f: F) -> Result<()>
    where
        F: FnOnce(&mut Frame<'_>),
    {
        self.terminal.draw(f)?;
        Ok(())
    }

    fn size(&self) -> Result<Size> {
        self.terminal.size().context("failed to read terminal size")
    }

    fn clear(&mut self) -> Result<()> {
        execute!(
            self.terminal.backend_mut(),
            TerminalClear(ClearType::All),
            MoveTo(0, 0)
        )
        .context("failed to clear terminal")?;
        force_full_redraw_after_external_clear(&mut self.terminal);
        Ok(())
    }

    fn clear_terminal_graphics(&mut self) -> Result<()> {
        self.terminal
            .backend_mut()
            .write_all(kitty_graphics_delete_visible_placements_sequence())
            .context("failed to clear terminal graphics")?;
        std::io::Write::flush(self.terminal.backend_mut())
            .context("failed to flush terminal graphics clear")
    }
}

fn force_full_redraw_after_external_clear<B: Backend>(terminal: &mut Terminal<B>) {
    terminal.swap_buffers();
    terminal.swap_buffers();
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        if let Ok(mut output) = terminal_output() {
            let _ = execute!(&mut output, DisableMouseCapture, LeaveAlternateScreen);
        }
    }
}

fn terminal_output() -> Result<Box<dyn Write>> {
    #[cfg(unix)]
    {
        let tty = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .context("failed to open /dev/tty for terminal output")?;
        Ok(Box::new(tty))
    }
    #[cfg(not(unix))]
    {
        Ok(Box::new(io::stdout()))
    }
}

struct UiState {
    app: TuiAppState,
    status: String,
    protocol_type: ProtocolType,
    event_sink: TuiEventSink,
    voice_call_state: VoiceCallState,
    broadcast_state: BroadcastState,
    incoming_session_id: Option<String>,
    active_session_id: Option<String>,
    connected_chat_ids: Vec<String>,
    last_peer_event: Option<String>,
    received_frames: u64,
    encoded_frames: u64,
    decoded_frames: u64,
    pending_frame_drops: u64,
    decoder_dropped_delta: u64,
    decoder_errors: u64,
    remote_video_received_frames: u64,
    remote_video_decoded_frames: u64,
    remote_video_pending_drops: u64,
    remote_video_delta_drops: u64,
    remote_video_decoder_errors: u64,
    remote_video_camera_enabled: Option<bool>,
    remote_video_error: Option<String>,
    remote_video_size: Size,
    remote_video_protocol: Option<ProtocolResponse>,
    media_error: Option<String>,
    media_size: Size,
    protocol: Option<ProtocolResponse>,
    last_protocol_seq: Option<u32>,
    inline_media_cache: InlineMediaCache,
    viewer_image: Option<ViewerLoadedImage>,
    viewer_protocol: Option<ProtocolResponse>,
    viewer_protocol_key: Option<MediaViewerKey>,
    sidebar_follow_selection: bool,
    show_help: bool,
}

#[derive(Clone)]
struct ViewerLoadedImage {
    file_hash: String,
    image: Arc<DynamicImage>,
}

impl UiState {
    fn new(protocol_type: ProtocolType, event_sink: TuiEventSink) -> Self {
        Self {
            app: TuiAppState::default(),
            status: "network running".to_string(),
            protocol_type,
            event_sink,
            voice_call_state: VoiceCallState::default(),
            broadcast_state: BroadcastState::default(),
            incoming_session_id: None,
            active_session_id: None,
            connected_chat_ids: Vec::new(),
            last_peer_event: None,
            received_frames: 0,
            encoded_frames: 0,
            decoded_frames: 0,
            pending_frame_drops: 0,
            decoder_dropped_delta: 0,
            decoder_errors: 0,
            remote_video_received_frames: 0,
            remote_video_decoded_frames: 0,
            remote_video_pending_drops: 0,
            remote_video_delta_drops: 0,
            remote_video_decoder_errors: 0,
            remote_video_camera_enabled: None,
            remote_video_error: None,
            remote_video_size: Size::new(48, 10),
            remote_video_protocol: None,
            media_error: None,
            media_size: Size::new(80, 24),
            protocol: None,
            last_protocol_seq: None,
            inline_media_cache: InlineMediaCache::new(INLINE_MEDIA_CACHE_CAPACITY),
            viewer_image: None,
            viewer_protocol: None,
            viewer_protocol_key: None,
            sidebar_follow_selection: true,
            show_help: true,
        }
    }

    fn protocol_type_is_kitty(&self) -> bool {
        self.protocol_type == ProtocolType::Kitty
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuthMode {
    Unlock,
    CreateVault,
    GitHubLogin,
    LocalUsername,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuthField {
    Password,
    Confirm,
    Token,
    Username,
}

#[derive(Debug)]
struct AuthGitHubDeviceState {
    device_code: String,
    user_code: String,
    verification_uri: String,
    poll_interval: Duration,
    next_poll_at: Instant,
}

#[derive(Debug)]
struct AuthUiState {
    mode: AuthMode,
    field: AuthField,
    password: String,
    confirm_password: String,
    token: String,
    username: String,
    github_device: Option<AuthGitHubDeviceState>,
    status: String,
    error: Option<String>,
}

impl AuthUiState {
    fn new(mode: AuthMode) -> Self {
        let status = match mode {
            AuthMode::Unlock => "Vault locked".to_string(),
            AuthMode::CreateVault => "No vault found".to_string(),
            AuthMode::GitHubLogin => "Connect GitHub".to_string(),
            AuthMode::LocalUsername => "Choose username".to_string(),
        };
        Self {
            mode,
            field: match mode {
                AuthMode::GitHubLogin => AuthField::Token,
                AuthMode::LocalUsername => AuthField::Username,
                AuthMode::Unlock | AuthMode::CreateVault => AuthField::Password,
            },
            password: String::new(),
            confirm_password: String::new(),
            token: String::new(),
            username: String::new(),
            github_device: None,
            status,
            error: None,
        }
    }

    fn active_input_mut(&mut self) -> &mut String {
        match self.field {
            AuthField::Password => &mut self.password,
            AuthField::Confirm => &mut self.confirm_password,
            AuthField::Token => &mut self.token,
            AuthField::Username => &mut self.username,
        }
    }

    fn cycle_field(&mut self) {
        if self.mode == AuthMode::CreateVault {
            self.field = match self.field {
                AuthField::Password => AuthField::Confirm,
                AuthField::Confirm => AuthField::Password,
                AuthField::Token | AuthField::Username => self.field,
            };
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuthKeyOutcome {
    Continue,
    Authenticated,
    Quit,
}

async fn handle_auth_key(
    app_state: &AppState,
    auth: &mut AuthUiState,
    key: KeyEvent,
) -> Result<AuthKeyOutcome> {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return Ok(AuthKeyOutcome::Quit);
    }
    if auth.mode == AuthMode::GitHubLogin
        && key.modifiers.contains(KeyModifiers::CONTROL)
        && key.code == KeyCode::Char('o')
    {
        return start_github_device_login(auth).await;
    }
    if auth.mode == AuthMode::GitHubLogin
        && key.modifiers.contains(KeyModifiers::CONTROL)
        && key.code == KeyCode::Char('l')
    {
        *auth = AuthUiState::new(AuthMode::LocalUsername);
        return Ok(AuthKeyOutcome::Continue);
    }

    match key.code {
        KeyCode::Esc => Ok(AuthKeyOutcome::Quit),
        KeyCode::Tab | KeyCode::BackTab => {
            auth.cycle_field();
            Ok(AuthKeyOutcome::Continue)
        }
        KeyCode::Enter => submit_auth(app_state, auth).await,
        KeyCode::Backspace => {
            auth.active_input_mut().pop();
            auth.error = None;
            Ok(AuthKeyOutcome::Continue)
        }
        KeyCode::Char(ch)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            auth.active_input_mut().push(ch);
            auth.error = None;
            Ok(AuthKeyOutcome::Continue)
        }
        _ => Ok(AuthKeyOutcome::Continue),
    }
}

async fn submit_auth(app_state: &AppState, auth: &mut AuthUiState) -> Result<AuthKeyOutcome> {
    match auth.mode {
        AuthMode::GitHubLogin => return submit_github_token(app_state, auth).await,
        AuthMode::LocalUsername => return submit_local_username(app_state, auth).await,
        AuthMode::Unlock | AuthMode::CreateVault => {}
    }

    let password = auth.password.trim().to_string();
    if password.is_empty() {
        auth.error = Some("Password cannot be empty".to_string());
        return Ok(AuthKeyOutcome::Continue);
    }
    if auth.mode == AuthMode::CreateVault && password.len() < 8 {
        auth.error = Some("Password must be at least 8 characters".to_string());
        return Ok(AuthKeyOutcome::Continue);
    }

    if auth.mode == AuthMode::CreateVault && password != auth.confirm_password.trim() {
        auth.error = Some("Passwords do not match".to_string());
        auth.field = AuthField::Confirm;
        return Ok(AuthKeyOutcome::Continue);
    }

    auth.error = None;

    let mut config_manager = app_state.config_manager.lock().await;
    let result = match auth.mode {
        AuthMode::Unlock => config_manager
            .unlock_with_password(&password)
            .await
            .map(|_| ()),
        AuthMode::CreateVault => config_manager.init(&password).await.map(|_| ()),
        AuthMode::GitHubLogin | AuthMode::LocalUsername => unreachable!("handled above"),
    };
    drop(config_manager);

    match result {
        Ok(()) => {
            if needs_identity_choice(app_state).await? {
                *auth = AuthUiState::new(AuthMode::GitHubLogin);
                Ok(AuthKeyOutcome::Continue)
            } else {
                Ok(AuthKeyOutcome::Authenticated)
            }
        }
        Err(error) => {
            auth.status = match auth.mode {
                AuthMode::Unlock => "Vault locked".to_string(),
                AuthMode::CreateVault => "No vault found".to_string(),
                AuthMode::GitHubLogin | AuthMode::LocalUsername => unreachable!("handled above"),
            };
            auth.error = Some(error.to_string());
            Ok(AuthKeyOutcome::Continue)
        }
    }
}

async fn start_github_device_login(auth: &mut AuthUiState) -> Result<AuthKeyOutcome> {
    auth.status = "Requesting GitHub device code...".to_string();
    auth.error = None;
    match oauth::start_device_flow().await {
        Ok(state) => {
            let poll_interval = Duration::from_secs((state.interval.max(1) + 1) as u64);
            auth.status = "Authorize RChat in GitHub".to_string();
            auth.github_device = Some(AuthGitHubDeviceState {
                device_code: state.device_code,
                user_code: state.user_code,
                verification_uri: state.verification_uri,
                poll_interval,
                next_poll_at: Instant::now() + poll_interval,
            });
        }
        Err(error) => {
            auth.status = "Connect GitHub".to_string();
            auth.error = Some(format!("GitHub auth failed: {error}"));
        }
    }
    Ok(AuthKeyOutcome::Continue)
}

async fn tick_auth(app_state: &AppState, auth: &mut AuthUiState) -> Result<AuthKeyOutcome> {
    if auth.mode != AuthMode::GitHubLogin {
        return Ok(AuthKeyOutcome::Continue);
    }

    let Some((device_code, poll_interval)) = auth.github_device.as_mut().and_then(|device| {
        let now = Instant::now();
        if now < device.next_poll_at {
            None
        } else {
            device.next_poll_at = now + device.poll_interval;
            Some((device.device_code.clone(), device.poll_interval))
        }
    }) else {
        return Ok(AuthKeyOutcome::Continue);
    };

    match oauth::poll_for_token(&device_code).await {
        Ok(token) => save_github_token(app_state, auth, token).await,
        Err(error) => {
            let message = error.to_string();
            if message.contains("authorization_pending") {
                auth.status = "Waiting for GitHub authorization...".to_string();
                Ok(AuthKeyOutcome::Continue)
            } else if message.contains("slow_down") {
                if let Some(device) = auth.github_device.as_mut() {
                    device.next_poll_at = Instant::now() + poll_interval + Duration::from_secs(5);
                }
                auth.status = "GitHub asked us to slow down...".to_string();
                Ok(AuthKeyOutcome::Continue)
            } else if message.contains("expired_token") {
                auth.status = "Connect GitHub".to_string();
                auth.github_device = None;
                auth.error = Some("GitHub login timed out. Start device flow again.".to_string());
                Ok(AuthKeyOutcome::Continue)
            } else {
                auth.status = "Connect GitHub".to_string();
                auth.github_device = None;
                auth.error = Some(format!("GitHub polling failed: {message}"));
                Ok(AuthKeyOutcome::Continue)
            }
        }
    }
}

async fn submit_github_token(
    app_state: &AppState,
    auth: &mut AuthUiState,
) -> Result<AuthKeyOutcome> {
    let token = auth.token.trim().to_string();
    if token.is_empty() {
        auth.error = Some(
            "Paste a token, press Ctrl+O for device login, or Ctrl+L for local only.".to_string(),
        );
        return Ok(AuthKeyOutcome::Continue);
    }

    save_github_token(app_state, auth, token).await
}

async fn save_github_token(
    app_state: &AppState,
    auth: &mut AuthUiState,
    token: String,
) -> Result<AuthKeyOutcome> {
    auth.status = "Saving GitHub login...".to_string();
    auth.error = None;

    match oauth::fetch_github_username(&token).await {
        Ok(username) => {
            let config_manager = app_state.config_manager.lock().await;
            let result = async {
                let mut config = config_manager.load().await?;
                config.system.github_token = Some(token);
                config.system.github_username = Some(username);
                config_manager.save(&config).await
            }
            .await;
            drop(config_manager);

            match result {
                Ok(()) => Ok(AuthKeyOutcome::Authenticated),
                Err(error) => {
                    auth.status = "Connect GitHub".to_string();
                    auth.error = Some(error.to_string());
                    Ok(AuthKeyOutcome::Continue)
                }
            }
        }
        Err(error) => {
            auth.status = "Connect GitHub".to_string();
            auth.error = Some(format!("Could not verify GitHub token: {error}"));
            Ok(AuthKeyOutcome::Continue)
        }
    }
}

async fn submit_local_username(
    app_state: &AppState,
    auth: &mut AuthUiState,
) -> Result<AuthKeyOutcome> {
    let username = auth.username.trim().to_string();
    if username.is_empty() {
        auth.error = Some("Username is required".to_string());
        return Ok(AuthKeyOutcome::Continue);
    }

    auth.status = "Saving username...".to_string();
    auth.error = None;

    let config_manager = app_state.config_manager.lock().await;
    let result = async {
        let mut config = config_manager.load().await?;
        config.user.profile.alias = Some(username);
        config_manager.save(&config).await
    }
    .await;
    drop(config_manager);

    match result {
        Ok(()) => Ok(AuthKeyOutcome::Authenticated),
        Err(error) => {
            auth.status = "Choose username".to_string();
            auth.error = Some(error.to_string());
            Ok(AuthKeyOutcome::Continue)
        }
    }
}

fn render_auth_shell(frame: &mut Frame<'_>, auth: &AuthUiState) {
    let theme = Theme::rchat();
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.bg)),
        frame.area(),
    );

    let popup = centered_rect(68, 14, frame.area());
    draw_shadow(frame, popup);
    frame.render_widget(Clear, popup);

    let password_focused = auth.field == AuthField::Password;
    let confirm_focused = auth.field == AuthField::Confirm;
    let token_focused = auth.field == AuthField::Token;
    let username_focused = auth.field == AuthField::Username;
    let mut lines = vec![
        Line::from(Span::styled(
            auth.status.as_str(),
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(auth_mode_description(auth.mode)),
        Line::from(""),
    ];

    match auth.mode {
        AuthMode::Unlock => {
            lines.push(auth_input_line(
                "Password",
                &auth.password,
                password_focused,
                true,
                &theme,
            ));
        }
        AuthMode::CreateVault => {
            lines.push(auth_input_line(
                "Password",
                &auth.password,
                password_focused,
                true,
                &theme,
            ));
            lines.push(auth_input_line(
                "Confirm",
                &auth.confirm_password,
                confirm_focused,
                true,
                &theme,
            ));
        }
        AuthMode::GitHubLogin => {
            if let Some(device) = &auth.github_device {
                lines.push(Line::from(vec![
                    Span::styled("Code: ", Style::default().fg(theme.muted)),
                    Span::styled(
                        device.user_code.clone(),
                        Style::default()
                            .fg(theme.accent)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]));
                lines.push(Line::from(vec![
                    Span::styled("URL:  ", Style::default().fg(theme.muted)),
                    Span::styled(
                        device.verification_uri.clone(),
                        Style::default().fg(theme.text),
                    ),
                ]));
            } else {
                lines.push(auth_input_line(
                    "Token",
                    &auth.token,
                    token_focused,
                    auth_field_is_secret(auth.mode, AuthField::Token),
                    &theme,
                ));
            }
        }
        AuthMode::LocalUsername => {
            lines.push(auth_input_line(
                "Username",
                &auth.username,
                username_focused,
                false,
                &theme,
            ));
        }
    }

    lines.push(Line::from(""));
    if let Some(error) = &auth.error {
        lines.push(Line::from(Span::styled(
            error.clone(),
            Style::default().fg(theme.error),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            auth_help_text(auth.mode),
            Style::default().fg(theme.muted),
        )));
    }

    let paragraph = Paragraph::new(lines)
        .block(themed_block(auth_title(auth.mode), &theme))
        .style(Style::default().bg(theme.surface).fg(theme.text))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, popup);
}

fn auth_input_line<'a>(
    label: &'a str,
    value: &str,
    focused: bool,
    secret: bool,
    theme: &Theme,
) -> Line<'a> {
    let style = if focused {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.text)
    };
    Line::from(vec![
        Span::styled(if focused { "> " } else { "  " }, style),
        Span::styled(format!("{label}: "), style),
        Span::styled(
            auth_display_value(value, secret),
            Style::default().fg(theme.text),
        ),
    ])
}

fn auth_title(mode: AuthMode) -> &'static str {
    match mode {
        AuthMode::Unlock => " Unlock RChat ",
        AuthMode::CreateVault => " Create RChat Vault ",
        AuthMode::GitHubLogin => " Connect GitHub ",
        AuthMode::LocalUsername => " Choose Username ",
    }
}

fn auth_mode_description(mode: AuthMode) -> &'static str {
    match mode {
        AuthMode::Unlock => "Enter your RChat vault password to continue.",
        AuthMode::CreateVault => "Create a local encrypted vault for this RChat identity.",
        AuthMode::GitHubLogin => "Sync through GitHub/Gist, paste a token, or continue locally.",
        AuthMode::LocalUsername => "Pick a local display name before entering RChat.",
    }
}

fn auth_help_text(mode: AuthMode) -> &'static str {
    match mode {
        AuthMode::Unlock => "Enter unlocks | Esc quits",
        AuthMode::CreateVault => "Tab switches fields | Enter creates | Esc quits",
        AuthMode::GitHubLogin => {
            "Enter saves token | Ctrl+O device login | Ctrl+L local only | Esc quits"
        }
        AuthMode::LocalUsername => "Enter saves | Esc quits",
    }
}

fn auth_display_value(value: &str, secret: bool) -> String {
    if secret {
        mask_secret(value)
    } else {
        value.to_string()
    }
}

fn auth_field_is_secret(mode: AuthMode, field: AuthField) -> bool {
    matches!(field, AuthField::Password | AuthField::Confirm)
        || matches!((mode, field), (AuthMode::GitHubLogin, AuthField::Token))
}

fn mask_secret(value: &str) -> String {
    PASSWORD_MASK_SYMBOL.repeat(value.chars().count())
}

fn render_media_shell(frame: &mut Frame<'_>, state: &mut UiState, kitty_available: bool) {
    let root = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(34), Constraint::Min(20)])
        .split(frame.area());
    render_status(frame, root[0], state, kitty_available);
    render_media(frame, root[1], state, kitty_available);
}

fn render_app_shell(
    frame: &mut Frame<'_>,
    state: &mut UiState,
    inline_loader: Option<&InlineMediaLoader>,
    protocol_worker: Option<&ProtocolWorker>,
    kitty_available: bool,
) {
    let theme = app_background_theme(state);
    let modal_theme = modal_overlay_theme();
    let background_kitty_available = background_kitty_media_enabled(state, kitty_available);
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.bg)),
        frame.area(),
    );

    let layout = app_layout(frame.area());
    let chat_history_area = if should_render_remote_video_panel(state) {
        let split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(12), Constraint::Min(4)])
            .split(layout.chat_history);
        render_remote_video_panel(frame, split[0], state, background_kitty_available, &theme);
        split[1]
    } else {
        layout.chat_history
    };

    render_top_bar(frame, layout.top_bar, state, &theme);
    render_app_sidebar(frame, layout.sidebar, state, &theme);
    render_chat_history(
        frame,
        chat_history_area,
        state,
        &theme,
        inline_loader,
        background_kitty_available,
    );
    render_composer_actions(frame, layout.composer_actions, state, &theme);
    render_composer(frame, layout.composer, state, &theme);
    render_help_line(frame, layout.help_line, state, &theme);

    if state.app.show_help {
        render_help_overlay(frame, frame.area(), &modal_theme);
    }
    if state.app.chat_details.is_some() {
        render_chat_details_overlay(frame, frame.area(), state, &modal_theme);
    }
    if state.app.new_person.is_some() {
        render_new_person_overlay(frame, frame.area(), state, kitty_available, &modal_theme);
    }
    if state.app.settings.is_some() {
        render_settings_overlay(frame, frame.area(), state, &modal_theme);
    }
    if state.app.attachment_modal.is_some() {
        render_attachment_overlay(
            frame,
            frame.area(),
            state,
            protocol_worker,
            kitty_available,
            &modal_theme,
        );
    }
    if state.app.sticker_picker.is_some() {
        render_sticker_picker_overlay(
            frame,
            frame.area(),
            state,
            inline_loader,
            kitty_available,
            &modal_theme,
        );
    }
    if state.app.attachment_actions.is_some() {
        render_attachment_actions_overlay(frame, frame.area(), state, &modal_theme);
    }
    if state.app.context_menu.is_some() {
        render_context_menu_overlay(frame, frame.area(), state, &modal_theme);
    }
    if state.app.media_viewer.is_some() {
        render_media_viewer_overlay(
            frame,
            frame.area(),
            state,
            protocol_worker,
            kitty_available,
            &modal_theme,
        );
    }
    if should_render_incoming_screen_share_prompt(state) {
        render_incoming_screen_share_prompt(frame, frame.area(), state, &modal_theme);
    }
    if state.app.show_command_palette {
        render_command_palette(frame, frame.area(), state, &modal_theme);
    }
}

fn background_kitty_media_enabled(state: &UiState, kitty_available: bool) -> bool {
    kitty_available && !graphics_obscuring_overlay_active(state)
}

fn graphics_clear_generation(state: &UiState) -> bool {
    graphics_obscuring_overlay_active(state)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GraphicsTransitionAction {
    clear_kitty_graphics: bool,
    invalidate_protocols: bool,
    clear_terminal: bool,
}

fn graphics_transition_action(
    previous_overlay_active: bool,
    current_overlay_active: bool,
    kitty_available: bool,
) -> GraphicsTransitionAction {
    let changed = previous_overlay_active != current_overlay_active;
    GraphicsTransitionAction {
        clear_kitty_graphics: changed && kitty_available,
        invalidate_protocols: changed && kitty_available,
        clear_terminal: false,
    }
}

fn kitty_graphics_delete_visible_placements_sequence() -> &'static [u8] {
    KITTY_DELETE_VISIBLE_PLACEMENTS
}

fn invalidate_terminal_graphics_protocols(state: &mut UiState) {
    state.protocol = None;
    state.remote_video_protocol = None;
    state.viewer_protocol = None;
    state.viewer_protocol_key = None;
    state.last_protocol_seq = None;
    state.inline_media_cache.clear();
}

fn graphics_obscuring_overlay_active(state: &UiState) -> bool {
    state.app.show_help
        || state.app.chat_details.is_some()
        || state.app.new_person.is_some()
        || state.app.settings.is_some()
        || state.app.attachment_modal.is_some()
        || state.app.sticker_picker.is_some()
        || state.app.attachment_actions.is_some()
        || state.app.context_menu.is_some()
        || state.app.media_viewer.is_some()
        || state.app.show_command_palette
        || should_render_incoming_screen_share_prompt(state)
}

#[derive(Debug, Clone, Copy)]
struct AppLayout {
    top_bar: Rect,
    sidebar: Rect,
    chat_history: Rect,
    composer_actions: Rect,
    composer: Rect,
    help_line: Rect,
}

fn app_layout(area: Rect) -> AppLayout {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(3),
            Constraint::Length(4),
            Constraint::Length(1),
        ])
        .split(area);
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(34), Constraint::Min(40)])
        .split(root[1]);

    AppLayout {
        top_bar: root[0],
        sidebar: body[0],
        chat_history: body[1],
        composer_actions: root[2],
        composer: root[3],
        help_line: root[4],
    }
}

fn render_top_bar(frame: &mut Frame<'_>, area: Rect, state: &UiState, theme: &Theme) {
    let mut spans = vec![
        Span::styled(
            " RChat ",
            Style::default()
                .fg(theme.bg)
                .bg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            state.app.status.as_str(),
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            format!(
                "{} connected  {} local",
                state.app.connected_chat_ids.len(),
                state.app.local_peers.len()
            ),
            Style::default().fg(theme.muted),
        ),
    ];
    if let Some(call_status) = voice_call_status_label(state) {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            call_status,
            Style::default()
                .fg(theme.warning)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if let Some(screen_status) = screen_share_status_label(state) {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            screen_status,
            Style::default()
                .fg(theme.warning)
                .add_modifier(Modifier::BOLD),
        ));
    }
    let title = Line::from(spans);

    let block = themed_block(" Status ", theme);
    let paragraph = Paragraph::new(title)
        .block(block)
        .alignment(Alignment::Left);
    frame.render_widget(paragraph, area);
}

fn should_render_remote_video_panel(state: &UiState) -> bool {
    state.voice_call_state.call_kind == Some(CallKind::Video)
        && state.voice_call_state.phase == VoiceCallPhase::Active
}

fn render_remote_video_panel(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &mut UiState,
    kitty_available: bool,
    theme: &Theme,
) {
    let block = themed_block(" Remote video ", theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    state.remote_video_size = Size::new(inner.width, inner.height);

    if !kitty_available {
        let message = Paragraph::new("Remote video requires Kitty image protocol.")
            .style(Style::default().bg(theme.bg).fg(theme.muted))
            .wrap(Wrap { trim: true });
        frame.render_widget(message, inner);
        return;
    }

    if state.remote_video_camera_enabled == Some(false) {
        let message = Paragraph::new("Remote camera off")
            .style(Style::default().bg(theme.bg).fg(theme.muted))
            .alignment(Alignment::Center);
        frame.render_widget(message, inner);
        return;
    }

    if let Some(error) = state.remote_video_error.as_deref() {
        let message = Paragraph::new(error)
            .style(Style::default().bg(theme.bg).fg(theme.error))
            .wrap(Wrap { trim: true });
        frame.render_widget(message, inner);
        return;
    }

    if let Some(protocol) = state.remote_video_protocol.as_ref() {
        let image = Image::new(&protocol.protocol);
        frame.render_widget(image, inner);
        return;
    }

    let message = Paragraph::new(format!(
        "Waiting for remote video frames...\nreceived {} decoded {} drops {} errors {}",
        state.remote_video_received_frames,
        state.remote_video_decoded_frames,
        state
            .remote_video_pending_drops
            .saturating_add(state.remote_video_delta_drops),
        state.remote_video_decoder_errors,
    ))
    .style(Style::default().bg(theme.bg).fg(theme.muted))
    .wrap(Wrap { trim: true });
    frame.render_widget(message, inner);
}

fn voice_call_status_label(state: &UiState) -> Option<String> {
    let call = &state.voice_call_state;
    let kind = call.call_kind.as_ref()?;
    if call.phase == VoiceCallPhase::Idle {
        return None;
    }
    let peer = call.peer_id.as_deref().unwrap_or("unknown");
    let label = match (kind, &call.phase) {
        (CallKind::Voice, VoiceCallPhase::OutgoingRinging) => {
            format!("voice calling {peer} | Actions: Voice ends")
        }
        (CallKind::Voice, VoiceCallPhase::IncomingRinging) => {
            format!("voice incoming {peer} | use incoming call controls")
        }
        (CallKind::Voice, VoiceCallPhase::Active) if call.muted => {
            format!("voice active {peer} muted | Actions: Voice ends")
        }
        (CallKind::Voice, VoiceCallPhase::Active) => {
            format!("voice active {peer} | Actions: Voice ends")
        }
        (CallKind::Voice, VoiceCallPhase::Ending) => format!("voice ending {peer}"),
        (CallKind::Video, VoiceCallPhase::OutgoingRinging) => {
            format!("video calling {peer} | Actions: Video ends")
        }
        (CallKind::Video, VoiceCallPhase::IncomingRinging) => {
            format!("video incoming {peer} | use incoming call controls")
        }
        (CallKind::Video, VoiceCallPhase::Active) => video_call_active_status_label(call, peer),
        (CallKind::Video, VoiceCallPhase::Ending) => format!("video ending {peer}"),
        (_, VoiceCallPhase::Idle) => return None,
    };
    Some(label)
}

fn video_call_active_status_label(call: &VoiceCallState, peer: &str) -> String {
    let mute = if call.muted { "muted" } else { "unmuted" };
    let camera = if call.camera_enabled {
        "camera on"
    } else {
        "camera off"
    };
    format!("video active {peer} {mute} {camera} | Actions: Video ends")
}

fn screen_share_status_label(state: &UiState) -> Option<String> {
    let broadcast = &state.broadcast_state;
    if broadcast.phase == BroadcastPhase::Idle {
        return None;
    }
    let peer = broadcast.peer_id.as_deref().unwrap_or("unknown");
    let label = match broadcast.phase {
        BroadcastPhase::OutgoingRinging => format!("starting screen share with {peer} | e end"),
        BroadcastPhase::IncomingRinging => {
            format!("incoming screen share from {peer} | a accept | r reject")
        }
        BroadcastPhase::Active if broadcast.is_host => {
            format!("sharing screen with {peer} | e end")
        }
        BroadcastPhase::Active => format!("watching screen share from {peer} | e end"),
        BroadcastPhase::Ending => format!("ending screen share with {peer}"),
        BroadcastPhase::Idle => return None,
    };
    Some(label)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct IncomingScreenSharePrompt {
    title: String,
    body: String,
    actions: String,
}

fn incoming_screen_share_prompt_summary(state: &UiState) -> Option<IncomingScreenSharePrompt> {
    let broadcast = &state.broadcast_state;
    if broadcast.phase != BroadcastPhase::IncomingRinging {
        return None;
    }
    let peer = broadcast.peer_id.as_deref().unwrap_or("unknown");
    let session = broadcast.session_id.as_deref().unwrap_or("unknown session");
    Some(IncomingScreenSharePrompt {
        title: "Incoming screen share".to_string(),
        body: format!("{peer} wants to share their screen.\nSession {session}"),
        actions: "a accept    r reject".to_string(),
    })
}

fn should_render_incoming_screen_share_prompt(state: &UiState) -> bool {
    state.broadcast_state.phase == BroadcastPhase::IncomingRinging
        && !state.app.show_command_palette
        && state.app.chat_details.is_none()
        && state.app.new_person.is_none()
        && state.app.settings.is_none()
        && state.app.attachment_modal.is_none()
        && state.app.sticker_picker.is_none()
        && state.app.attachment_actions.is_none()
        && state.app.context_menu.is_none()
        && state.app.media_viewer.is_none()
}

fn render_incoming_screen_share_prompt(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &UiState,
    theme: &Theme,
) {
    let Some(prompt) = incoming_screen_share_prompt_summary(state) else {
        return;
    };
    let popup = centered_rect(64, 9, area);
    draw_shadow(frame, popup);
    frame.render_widget(Clear, popup);
    let lines = vec![
        Line::from(Span::styled(
            prompt.title,
            Style::default()
                .fg(theme.warning)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(prompt.body),
        Line::from(""),
        Line::from(Span::styled(
            prompt.actions,
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "Use the direct keys above. Esc keeps the share pending.",
            Style::default().fg(theme.muted),
        )),
    ];
    let paragraph = Paragraph::new(lines)
        .block(themed_block(" Screen share ", theme))
        .style(Style::default().bg(theme.surface).fg(theme.text))
        .wrap(Wrap { trim: true });
    frame.render_widget(paragraph, popup);
}

fn render_app_sidebar(frame: &mut Frame<'_>, area: Rect, state: &mut UiState, theme: &Theme) {
    let mut lines = vec![Line::from(Span::styled(
        if state.app.sidebar_search_active {
            format!("Search: {}", state.app.sidebar_search)
        } else {
            "Chats".to_string()
        },
        Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
    ))];

    if state.app.chats.is_empty() {
        lines.push(Line::from(Span::styled(
            "No chats yet",
            Style::default().fg(theme.muted),
        )));
    }

    let inner = inset_rect(area, 1);
    let visible_capacity = inner.height.saturating_sub(1) as usize;
    for row in visible_sidebar_rows(state, visible_capacity) {
        match row {
            SidebarRow::Envelope { label, .. } => {
                lines.push(Line::from(Span::styled(
                    label,
                    Style::default()
                        .fg(theme.muted)
                        .add_modifier(Modifier::BOLD),
                )));
            }
            SidebarRow::Chat(index) => lines.push(sidebar_chat_line(state, index, theme)),
        }
    }

    if let Some(error) = &state.app.last_error {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            error.clone(),
            Style::default().fg(theme.error),
        )));
    }

    let paragraph = Paragraph::new(lines)
        .block(themed_block(" Conversations ", theme))
        .style(Style::default().bg(theme.bg).fg(theme.text))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SidebarRow {
    Envelope { id: String, label: String },
    Chat(usize),
}

fn sidebar_rows(state: &UiState) -> Vec<SidebarRow> {
    let query = state.app.sidebar_search.trim().to_ascii_lowercase();
    let chat_matches = |chat: &TuiChat| {
        query.is_empty()
            || chat.name.to_ascii_lowercase().contains(&query)
            || chat.id.to_ascii_lowercase().contains(&query)
    };
    let envelope_matches = |envelope: &TuiEnvelope| {
        query.is_empty()
            || envelope.name.to_ascii_lowercase().contains(&query)
            || envelope.id.to_ascii_lowercase().contains(&query)
    };
    let known_envelopes = state
        .app
        .envelopes
        .iter()
        .map(|envelope| envelope.id.as_str())
        .collect::<HashSet<_>>();
    let mut rows = Vec::new();
    let mut grouped_chat_indices = HashSet::new();

    for envelope in &state.app.envelopes {
        let members = state
            .app
            .chats
            .iter()
            .enumerate()
            .filter(|(_, chat)| {
                state.app.chat_envelope_id(&chat.id) == Some(envelope.id.as_str())
                    && (chat_matches(chat) || envelope_matches(envelope))
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if members.is_empty() {
            continue;
        }

        let icon = envelope.icon.as_deref().unwrap_or("folder");
        rows.push(SidebarRow::Envelope {
            id: envelope.id.clone(),
            label: format!("{icon} {}", envelope.name),
        });
        for index in members {
            grouped_chat_indices.insert(index);
            rows.push(SidebarRow::Chat(index));
        }
    }

    let mut root_rows = state
        .app
        .chats
        .iter()
        .enumerate()
        .filter(|(index, chat)| {
            if grouped_chat_indices.contains(index) {
                return false;
            }
            state
                .app
                .chat_envelope_id(&chat.id)
                .is_none_or(|envelope_id| !known_envelopes.contains(envelope_id))
                && chat_matches(chat)
        })
        .map(|(index, _)| SidebarRow::Chat(index))
        .collect::<Vec<_>>();
    root_rows.extend(rows);
    root_rows
}

fn sidebar_visible_row_capacity(sidebar: Rect) -> usize {
    inset_rect(sidebar, 1).height.saturating_sub(1) as usize
}

fn visible_sidebar_rows(state: &mut UiState, visible_capacity: usize) -> Vec<SidebarRow> {
    let rows = sidebar_rows(state);
    if visible_capacity == 0 || rows.is_empty() {
        state.app.sidebar_scroll_offset = 0;
        return Vec::new();
    }

    let max_offset = rows.len().saturating_sub(visible_capacity);
    state.app.sidebar_scroll_offset = state.app.sidebar_scroll_offset.min(max_offset);

    if state.sidebar_follow_selection {
        if let Some(selected_row) = selected_sidebar_row_index(&rows, state.app.selected_chat_index)
        {
            if selected_row < state.app.sidebar_scroll_offset {
                state.app.sidebar_scroll_offset = selected_row;
            } else {
                let bottom = state
                    .app
                    .sidebar_scroll_offset
                    .saturating_add(visible_capacity);
                if selected_row >= bottom {
                    state.app.sidebar_scroll_offset = selected_row
                        .saturating_add(1)
                        .saturating_sub(visible_capacity);
                }
            }
        }
    }
    state.sidebar_follow_selection = false;

    let start = state.app.sidebar_scroll_offset.min(max_offset);
    rows.into_iter().skip(start).take(visible_capacity).collect()
}

fn scroll_sidebar_rows(state: &mut UiState, delta: isize, visible_capacity: usize) {
    let rows = sidebar_rows(state);
    if visible_capacity == 0 || rows.len() <= visible_capacity {
        state.app.sidebar_scroll_offset = 0;
        return;
    }
    let max_offset = rows.len().saturating_sub(visible_capacity);
    if delta < 0 {
        state.app.sidebar_scroll_offset = state
            .app
            .sidebar_scroll_offset
            .saturating_sub(delta.unsigned_abs());
    } else {
        state.app.sidebar_scroll_offset = state
            .app
            .sidebar_scroll_offset
            .saturating_add(delta as usize)
            .min(max_offset);
    }
}

fn selected_sidebar_row_index(rows: &[SidebarRow], selected_chat_index: usize) -> Option<usize> {
    rows.iter()
        .position(|row| *row == SidebarRow::Chat(selected_chat_index))
}

fn sidebar_chat_line<'a>(state: &'a UiState, index: usize, theme: &Theme) -> Line<'a> {
    let Some(chat) = state.app.chats.get(index) else {
        return Line::from("");
    };
    let selected = state.app.focus == FocusPane::Chats && index == state.app.selected_chat_index;
    let active = state.app.active_chat_id.as_deref() == Some(chat.id.as_str());
    let kind_label = if is_group_chat_list_item(&chat.id) {
        "group"
    } else if state.app.is_chat_connected(&chat.id) {
        "online"
    } else {
        "offline"
    };
    let parsed_kind = chat_kind::parse_chat_kind(&chat.id);
    let mut badges = Vec::new();
    if state.app.is_chat_pinned(chat) {
        badges.push("pin");
    }
    if matches!(
        parsed_kind,
        ChatKind::TemporaryDirect | ChatKind::TemporaryGroup
    ) {
        badges.push("temp");
    }
    if state.voice_call_state.peer_id.as_deref() == Some(chat.id.as_str())
        && state.voice_call_state.phase != VoiceCallPhase::Idle
    {
        badges.push("live");
    }
    if state.broadcast_state.peer_id.as_deref() == Some(chat.id.as_str())
        && state.broadcast_state.phase != BroadcastPhase::Idle
    {
        badges.push("share");
    }
    let badges = if badges.is_empty() {
        String::new()
    } else {
        format!(" {}", badges.join(" "))
    };
    let unread = if chat.unread_count > 0 {
        format!(" ({})", chat.unread_count)
    } else {
        String::new()
    };
    let marker = if selected {
        ">"
    } else if active {
        "*"
    } else {
        " "
    };
    let style = if selected {
        Style::default()
            .fg(theme.bg)
            .bg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else if active {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.text)
    };
    let name = display_chat_name(&chat.name, &chat.id);
    Line::from(Span::styled(
        format!("{marker} {} [{}]{}{}", name, kind_label, unread, badges),
        style,
    ))
}

fn is_group_chat_list_item(chat_id: &str) -> bool {
    matches!(
        chat_kind::parse_chat_kind(chat_id),
        ChatKind::Group | ChatKind::TemporaryGroup
    )
}

fn render_chat_history(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &mut UiState,
    theme: &Theme,
    inline_loader: Option<&InlineMediaLoader>,
    kitty_available: bool,
) {
    let title = active_chat_title(state);
    let scroll_suffix = if state.app.history_scroll_offset > 0 {
        format!(" - {} messages up", state.app.history_scroll_offset)
    } else {
        String::new()
    };
    let block = themed_block(format!(" {title}{scroll_suffix} "), theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if state.app.messages.is_empty() {
        let paragraph =
            Paragraph::new("No messages yet").style(Style::default().bg(theme.bg).fg(theme.muted));
        frame.render_widget(paragraph, inner);
        return;
    }

    let plans = visible_message_plans(
        &state.app.messages,
        inner.width,
        inner.height,
        kitty_available,
        state.app.history_scroll_offset,
    );
    let peer_label = active_chat_peer_label(state);
    for plan in plans {
        let row_area = Rect {
            x: inner.x,
            y: inner.y.saturating_add(plan.y_offset),
            width: inner.width,
            height: plan.visible_height,
        };
        render_message(
            frame,
            row_area,
            state,
            &plan.message,
            theme,
            peer_label.as_deref(),
            inline_loader,
            kitty_available && plan.inline_preview,
            plan.clip_top,
        );
    }
}

#[derive(Clone)]
struct MessageRenderPlan {
    message: TuiMessage,
    inline_preview: bool,
    y_offset: u16,
    visible_height: u16,
    clip_top: u16,
}

fn visible_message_plans(
    messages: &[TuiMessage],
    width: u16,
    available_height: u16,
    kitty_available: bool,
    scroll_offset: usize,
) -> Vec<MessageRenderPlan> {
    if messages.is_empty() || available_height == 0 {
        return Vec::new();
    }

    let mut inline_flags = vec![false; messages.len()];
    let mut inline_previews = 0_usize;
    for (index, message) in messages.iter().enumerate().rev() {
        if kitty_available
            && inline_previews < INLINE_MEDIA_VISIBLE_CAP
            && inline_preview_capable(message)
        {
            inline_flags[index] = true;
            inline_previews += 1;
        }
    }

    let heights = messages
        .iter()
        .enumerate()
        .map(|(index, message)| {
            message_render_height(message, width, inline_flags[index], kitty_available)
        })
        .collect::<Vec<_>>();
    let total_height = heights.iter().fold(0_usize, |sum, height| {
        sum.saturating_add(usize::from(*height))
    });
    let viewport_height = usize::from(available_height);
    if total_height <= viewport_height {
        let mut y = viewport_height.saturating_sub(total_height) as u16;
        return messages
            .iter()
            .zip(heights)
            .zip(inline_flags)
            .map(|((message, height), inline_preview)| {
                let plan = MessageRenderPlan {
                    message: message.clone(),
                    inline_preview,
                    y_offset: y,
                    visible_height: height,
                    clip_top: 0,
                };
                y = y.saturating_add(height);
                plan
            })
            .collect();
    }

    let max_scroll = messages.len().saturating_sub(1);
    let scroll_offset = scroll_offset.min(max_scroll);
    let end_exclusive = messages.len().saturating_sub(scroll_offset);
    let mut selected = Vec::new();
    let mut selected_height = 0_usize;

    for index in (0..end_exclusive).rev() {
        let height = usize::from(heights[index]);
        let render_height = height.min(viewport_height);
        let remaining_height = viewport_height.saturating_sub(selected_height);
        if remaining_height == 0 {
            break;
        }

        if !selected.is_empty() && selected_height.saturating_add(render_height) > viewport_height {
            selected.push((
                index,
                remaining_height,
                height.saturating_sub(remaining_height),
            ));
            selected_height = selected_height.saturating_add(remaining_height);
            break;
        }

        selected.push((index, render_height, 0));
        selected_height = selected_height.saturating_add(render_height);

        if selected_height >= viewport_height {
            break;
        }
    }

    selected.reverse();
    let mut y = viewport_height.saturating_sub(selected_height) as u16;
    let mut plans = Vec::new();

    for (index, render_height, clip_top) in selected {
        if render_height == 0 {
            continue;
        }
        plans.push(MessageRenderPlan {
            message: messages[index].clone(),
            inline_preview: inline_flags[index],
            y_offset: y,
            visible_height: render_height as u16,
            clip_top: clip_top as u16,
        });
        y = y.saturating_add(render_height as u16);
    }

    plans
}

fn message_render_height(
    message: &TuiMessage,
    width: u16,
    inline_preview: bool,
    kitty_available: bool,
) -> u16 {
    if inline_preview {
        return 1 + INLINE_MEDIA_PREVIEW_HEIGHT + 1;
    }

    if is_media_content_type(&message.content_type) {
        let fallback_line = (!kitty_available && inline_preview_capable(message)) as u16;
        return 1
            + media_message_body_lines(message, &Theme::rchat()).len() as u16
            + fallback_line
            + 1;
    }

    1 + wrapped_line_count(&message.text, width.saturating_sub(2)) + 1
}

fn wrapped_line_count(text: &str, width: u16) -> u16 {
    let width = usize::from(width.max(1));
    let lines = text.lines().count().max(1);
    text.lines()
        .map(|line| line.chars().count().max(1).div_ceil(width))
        .sum::<usize>()
        .max(lines) as u16
}

fn render_message(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &mut UiState,
    message: &TuiMessage,
    theme: &Theme,
    peer_label: Option<&str>,
    inline_loader: Option<&InlineMediaLoader>,
    render_inline_preview: bool,
    clip_top: u16,
) {
    if clip_top > 0 {
        render_message_clipped(
            frame,
            area,
            state,
            message,
            theme,
            inline_loader,
            render_inline_preview,
            clip_top,
        );
        return;
    }

    let mut y = area.y;
    let is_me = message.sender == "Me";
    let sender_label = if is_me {
        "You".to_string()
    } else {
        display_sender_label(&message.sender, peer_label)
    };
    let message_selected = message_is_selected(state, message);
    let attachment_selected = message_selected && message_is_attachment(message);
    let message_bg = if message_selected {
        theme.surface
    } else {
        theme.bg
    };
    let mut header_spans = vec![
        Span::styled(
            sender_label,
            if is_me {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(theme.warning)
                    .add_modifier(Modifier::BOLD)
            },
        ),
        Span::raw("  "),
        Span::styled(
            format!("[{}]", message.status),
            Style::default().fg(theme.muted),
        ),
    ];
    if message_selected {
        header_spans.push(Span::raw("  "));
        header_spans.push(Span::styled(
            if attachment_selected {
                "selected | v view | Enter actions"
            } else {
                "selected"
            },
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ));
    }
    let header = Paragraph::new(Line::from(header_spans))
        .style(Style::default().bg(message_bg).fg(theme.text));
    frame.render_widget(
        header,
        Rect {
            x: area.x,
            y,
            width: area.width,
            height: 1,
        },
    );
    y = y.saturating_add(1);

    if render_inline_preview {
        let preview_area = Rect {
            x: area.x.saturating_add(2),
            y,
            width: area.width.saturating_sub(4).max(1),
            height: INLINE_MEDIA_PREVIEW_HEIGHT.min(area.height.saturating_sub(1)),
        };
        render_inline_preview_box(frame, preview_area, state, message, theme, inline_loader);
        return;
    }

    let mut lines = message_body_lines(message, theme);
    if !state.protocol_type_is_kitty() && inline_preview_capable(message) {
        lines.push(Line::from(Span::styled(
            "  inline preview unavailable",
            Style::default().fg(theme.muted),
        )));
    }
    let paragraph = Paragraph::new(lines)
        .style(Style::default().bg(message_bg).fg(theme.text))
        .wrap(Wrap { trim: true });
    frame.render_widget(
        paragraph,
        Rect {
            x: area.x,
            y,
            width: area.width,
            height: area.height.saturating_sub(1),
        },
    );
}

fn message_is_selected(state: &UiState, message: &TuiMessage) -> bool {
    state.app.focus == FocusPane::History
        && state
            .app
            .selected_message()
            .is_some_and(|selected| selected.id == message.id)
}

fn render_message_clipped(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &mut UiState,
    message: &TuiMessage,
    theme: &Theme,
    inline_loader: Option<&InlineMediaLoader>,
    render_inline_preview: bool,
    clip_top: u16,
) {
    if render_inline_preview {
        let skipped_preview_rows = clip_top.saturating_sub(1);
        if skipped_preview_rows < INLINE_MEDIA_PREVIEW_HEIGHT {
            render_inline_preview_box(frame, area, state, message, theme, inline_loader);
        }
        return;
    }

    let body_scroll = clip_top.saturating_sub(1);
    let paragraph = Paragraph::new(message_body_lines(message, theme))
        .style(Style::default().bg(theme.bg).fg(theme.text))
        .wrap(Wrap { trim: true })
        .scroll((body_scroll, 0));
    frame.render_widget(paragraph, area);
}

fn render_inline_preview_box(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &mut UiState,
    message: &TuiMessage,
    theme: &Theme,
    inline_loader: Option<&InlineMediaLoader>,
) {
    let block = themed_block(" preview ", theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width < INLINE_MEDIA_MIN_WIDTH || inner.height == 0 {
        let paragraph = Paragraph::new("terminal pane too small")
            .style(Style::default().bg(theme.bg).fg(theme.muted));
        frame.render_widget(paragraph, inner);
        return;
    }

    let key = match inline_media_key(message, Size::new(inner.width, inner.height)) {
        Some(key) => key,
        None => {
            let paragraph = Paragraph::new("missing file hash")
                .style(Style::default().bg(theme.bg).fg(theme.error));
            frame.render_widget(paragraph, inner);
            return;
        }
    };

    if state.inline_media_cache.get(&key).is_none() {
        if let Some(loader) = inline_loader {
            state.inline_media_cache.insert_loading(key.clone());
            loader.request(key.clone(), message);
        } else {
            state
                .inline_media_cache
                .insert_error(key.clone(), "inline preview unavailable");
        }
    }

    match state.inline_media_cache.get(&key) {
        Some(InlineMediaState::Ready(protocol)) => {
            let image = Image::new(protocol);
            frame.render_widget(image, inner);
        }
        Some(InlineMediaState::Error(error)) => {
            let paragraph = Paragraph::new(error.as_str())
                .style(Style::default().bg(theme.bg).fg(theme.error))
                .wrap(Wrap { trim: true });
            frame.render_widget(paragraph, inner);
        }
        Some(InlineMediaState::Loading) | None => {
            let paragraph = Paragraph::new("loading preview...")
                .style(Style::default().bg(theme.bg).fg(theme.muted));
            frame.render_widget(paragraph, inner);
        }
    }
}

fn message_body_lines<'a>(message: &'a TuiMessage, theme: &Theme) -> Vec<Line<'a>> {
    if is_media_content_type(&message.content_type) {
        media_message_body_lines(message, theme)
    } else {
        vec![Line::from(Span::styled(
            format!("  {}", message.text),
            Style::default().fg(theme.text),
        ))]
    }
}

fn media_message_body_lines<'a>(message: &'a TuiMessage, theme: &Theme) -> Vec<Line<'a>> {
    let label = media_content_label(&message.content_type);
    let name = media_display_name(message);
    let hash = message
        .file_hash
        .as_deref()
        .map(short_hash)
        .unwrap_or_else(|| "missing hash".to_string());
    let size = message
        .content_metadata
        .as_deref()
        .and_then(media_size_label)
        .map(|value| format!("  {value}"))
        .unwrap_or_default();

    let mut lines = vec![Line::from(vec![
        Span::styled("  ", Style::default().fg(theme.text)),
        Span::styled(
            label,
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {name}"), Style::default().fg(theme.text)),
        Span::styled(size, Style::default().fg(theme.muted)),
    ])];

    lines.push(Line::from(Span::styled(
        format!("  hash {hash}"),
        Style::default().fg(theme.muted),
    )));
    lines
}

fn is_media_content_type(content_type: &str) -> bool {
    matches!(
        content_type,
        "image" | "photo" | "sticker" | "video" | "audio" | "document"
    )
}

fn inline_preview_capable(message: &TuiMessage) -> bool {
    matches!(message.content_type.as_str(), "image" | "photo" | "sticker")
        && message
            .file_hash
            .as_deref()
            .is_some_and(|hash| !hash.is_empty())
}

#[cfg(test)]
fn inline_preview_renders_metadata(message: &TuiMessage, render_inline_preview: bool) -> bool {
    is_media_content_type(&message.content_type) && !render_inline_preview
}

fn inline_media_key(message: &TuiMessage, size: Size) -> Option<InlineMediaKey> {
    Some(InlineMediaKey::new(
        message.id.clone(),
        message.file_hash.as_ref()?.clone(),
        size,
    ))
}

fn media_content_label(content_type: &str) -> &'static str {
    match content_type {
        "image" | "photo" => "[image]",
        "sticker" => "[sticker]",
        "video" => "[video]",
        "audio" => "[audio]",
        "document" => "[file]",
        _ => "[media]",
    }
}

fn media_display_name(message: &TuiMessage) -> String {
    let trimmed = message.text.trim();
    if !trimmed.is_empty() {
        return trimmed.to_string();
    }
    match message.content_type.as_str() {
        "image" | "photo" => "image attachment".to_string(),
        "sticker" => "sticker".to_string(),
        "video" => "video attachment".to_string(),
        "audio" => "audio attachment".to_string(),
        "document" => "file attachment".to_string(),
        _ => "media attachment".to_string(),
    }
}

fn media_size_label(metadata: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(metadata).ok()?;
    let size = value.get("size_bytes")?.as_u64()?;
    Some(format_bytes(size))
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn short_hash(hash: &str) -> String {
    const PREFIX_LEN: usize = 12;
    if hash.len() <= PREFIX_LEN {
        hash.to_string()
    } else {
        format!("{}...", &hash[..PREFIX_LEN])
    }
}

fn render_composer_actions(frame: &mut Frame<'_>, area: Rect, state: &UiState, theme: &Theme) {
    let focused = state.app.focus == FocusPane::ComposerActions;
    let border_style = if focused {
        Style::default().fg(theme.accent)
    } else {
        Style::default().fg(theme.muted)
    };
    let spans = ComposerAction::ALL
        .iter()
        .enumerate()
        .flat_map(|(index, action)| {
            let selected = focused && index == state.app.selected_composer_action_index;
            let style = if selected {
                Style::default()
                    .fg(theme.bg)
                    .bg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text)
            };
            [
                Span::styled(format!(" {} ", action.label()), style),
                Span::raw(" "),
            ]
        })
        .collect::<Vec<_>>();
    let line = Line::from(spans);
    let paragraph = Paragraph::new(line)
        .block(
            themed_block(" Actions ", theme)
                .border_style(border_style)
                .style(Style::default().bg(theme.surface).fg(theme.text)),
        )
        .style(Style::default().bg(theme.surface).fg(theme.text))
        .wrap(Wrap { trim: true });
    frame.render_widget(paragraph, area);
}

fn render_composer(frame: &mut Frame<'_>, area: Rect, state: &UiState, theme: &Theme) {
    let focused = state.app.focus == FocusPane::Composer;
    let title = if state.app.active_chat_id.is_some() {
        " Message "
    } else {
        " Select a chat "
    };
    let border_style = if focused {
        Style::default().fg(theme.accent)
    } else {
        Style::default().fg(theme.muted)
    };
    let text = format!("> {}", state.app.composer);
    let paragraph = Paragraph::new(text)
        .block(
            themed_block(title, theme)
                .border_style(border_style)
                .style(Style::default().bg(theme.surface).fg(theme.text)),
        )
        .style(Style::default().bg(theme.surface).fg(theme.text))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn render_help_line(frame: &mut Frame<'_>, area: Rect, state: &UiState, theme: &Theme) {
    let line = Paragraph::new(help_line_text(state, area.width as usize))
        .style(Style::default().bg(theme.bg).fg(theme.muted));
    frame.render_widget(line, area);
}

fn help_line_text(state: &UiState, width: usize) -> String {
    if state.app.new_person.is_some() {
        return fit_segments(
            &["New Person", "Up/Down move", "Enter activate", "Esc back"],
            width,
        );
    }
    if state.app.settings.is_some() {
        return fit_segments(
            &[
                "Settings",
                "Tab panes",
                "Up/Down move",
                "Enter activate",
                "Esc close",
            ],
            width,
        );
    }
    if state.app.media_viewer.is_some() {
        return fit_segments(
            &["Viewer", "+/- zoom", "arrows move", "Enter action", "Esc close"],
            width,
        );
    }
    if state.app.attachment_actions.is_some() {
        return fit_segments(
            &["Attachment", "Up/Down move", "Enter action", "Esc close"],
            width,
        );
    }
    if state.app.attachment_modal.is_some() {
        return fit_segments(
            &[
                "Attach",
                "type search",
                "o file picker",
                "Enter select/send",
                "Esc close",
            ],
            width,
        );
    }
    if state.app.sticker_picker.is_some() {
        return fit_segments(
            &["Stickers", "preview first", "a add", "Enter send", "Esc close"],
            width,
        );
    }
    if state.app.show_command_palette {
        return fit_segments(&["Commands", "Enter run", "Esc close"], width);
    }

    let focus = match state.app.focus {
        FocusPane::Chats => "Chats",
        FocusPane::History => "Chat",
        FocusPane::ComposerActions => "Actions",
        FocusPane::Composer => "Message",
        FocusPane::CommandPalette => "Commands",
    };
    let context_hints: &[&str] = match state.app.focus {
        FocusPane::Chats if state.app.sidebar_search_active => {
            &["type to filter", "Esc clear", "Enter open"]
        }
        FocusPane::Chats => &["Up/Down move", "/ search", "Enter open", "m menu"],
        FocusPane::History => &["Up/Down msgs", "Enter actions", "v view"],
        FocusPane::ComposerActions => &["Left/Right choose", "Enter activate"],
        FocusPane::Composer => &["Enter send", "Tab leave"],
        FocusPane::CommandPalette => &["Enter run", "Esc close"],
    };
    let mut parts = if state.app.focus == FocusPane::Composer {
        vec![focus, "text types normally", "? is text"]
    } else {
        vec![focus, "n new", "s settings", "? help", "q quit"]
    };
    parts.extend_from_slice(context_hints);
    fit_segments(&parts, width)
}

fn fit_segments(parts: &[&str], width: usize) -> String {
    if width == 0 {
        return String::new();
    }

    let mut output = String::new();
    for part in parts {
        let candidate = if output.is_empty() {
            (*part).to_string()
        } else {
            format!("{output} | {part}")
        };
        if candidate.chars().count() <= width {
            output = candidate;
        }
    }

    if output.is_empty() {
        truncate_chars(parts.first().copied().unwrap_or_default(), width)
    } else {
        truncate_chars(&output, width)
    }
}

fn truncate_chars(value: &str, width: usize) -> String {
    value.chars().take(width).collect()
}

fn render_help_overlay(frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
    let popup = centered_rect(76, 24, area);
    draw_shadow(frame, popup);
    frame.render_widget(Clear, popup);
    let lines = help_overlay_text_lines()
        .iter()
        .copied()
        .map(Line::from)
        .collect::<Vec<_>>();
    let paragraph = Paragraph::new(lines)
        .block(themed_block(" Help ", theme))
        .style(Style::default().bg(theme.surface).fg(theme.text))
        .wrap(Wrap { trim: true });
    frame.render_widget(paragraph, popup);
}

fn help_overlay_text_lines() -> &'static [&'static str] {
    &[
        "Keyboard",
        "Tab: Conversations -> Chat -> Actions -> Message -> Conversations",
        "Up/Down: move the selected conversation or message",
        "PageUp/PageDown: scroll chat history",
        "Enter: open chat, activate action, open attachment actions, or send message",
        "Left/Right: choose composer action when Actions is focused",
        "/: search conversations outside Message focus",
        "n: New Person",
        "s: Settings",
        "v: View selected attachment from Chat focus",
        "?: Help",
        "q: quit outside Message focus",
        "",
        "Message",
        "When Message is focused, letters and punctuation are typed normally.",
        "Use Tab or mouse click to leave the message box before using app shortcuts.",
        "",
        "Mouse",
        "Click a pane to focus it. Click action buttons to activate them.",
        "Use the wheel over Conversations or Chat to scroll.",
        "",
        "Incoming Screen Share",
        "a: accept",
        "r: reject",
        "e: end active screen share",
    ]
}

fn render_chat_details_overlay(frame: &mut Frame<'_>, area: Rect, state: &UiState, theme: &Theme) {
    let Some(details) = state.app.chat_details.as_ref() else {
        return;
    };

    let popup = centered_rect(76, 20, area);
    draw_shadow(frame, popup);
    frame.render_widget(Clear, popup);

    let connection_label = if details.connected {
        details
            .remote_addr
            .as_deref()
            .map(|addr| format!("online via {addr}"))
            .unwrap_or_else(|| "online".to_string())
    } else {
        "offline".to_string()
    };

    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                display_chat_name(&details.peer_name, &details.chat_id),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(connection_label, Style::default().fg(theme.muted)),
        ]),
        Line::from(""),
        Line::from(format!(
            "Peer id: {}",
            short_identifier(&details.peer_id, 42)
        )),
        Line::from(format!(
            "Alias: {}",
            details.peer_alias.as_deref().unwrap_or("-")
        )),
        Line::from(format!(
            "Avatar: {}",
            details.avatar_url.as_deref().unwrap_or("-")
        )),
        Line::from(format!("Reconnects: {}", details.reconnect_count)),
        Line::from(format!(
            "Messages: {} sent, {} received",
            details.sent_total, details.received_total
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Recent files",
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        )),
    ];

    if details.recent_files.is_empty() {
        lines.push(Line::from(Span::styled(
            "No shared files yet",
            Style::default().fg(theme.muted),
        )));
    } else {
        for file in &details.recent_files {
            let size = file
                .size_bytes
                .filter(|size| *size >= 0)
                .map(|size| format_bytes(size as u64))
                .unwrap_or_else(|| "unknown size".to_string());
            lines.push(Line::from(format!(
                "{}  {}  {}  {}",
                file.content_type,
                short_identifier(&file.display_name, 26),
                size,
                short_hash(&file.file_hash)
            )));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Esc closes",
        Style::default().fg(theme.muted),
    )));

    let paragraph = Paragraph::new(lines)
        .block(themed_block(" Chat details ", theme))
        .style(Style::default().bg(theme.surface).fg(theme.text))
        .wrap(Wrap { trim: true });
    frame.render_widget(paragraph, popup);
}

fn render_attachment_overlay(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &mut UiState,
    protocol_worker: Option<&ProtocolWorker>,
    kitty_available: bool,
    theme: &Theme,
) {
    let Some(modal) = state.app.attachment_modal.clone() else {
        return;
    };
    let popup = centered_rect(82, 26, area);
    draw_shadow(frame, popup);
    frame.render_widget(Clear, popup);

    let body = themed_block(" Attach ", theme);
    frame.render_widget(
        body.style(Style::default().bg(theme.surface).fg(theme.text)),
        popup,
    );
    let inner = popup.inner(ratatui::layout::Margin {
        horizontal: 2,
        vertical: 1,
    });
    let split = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(inner);
    let left = split[0];
    let right = split[1];

    let visible_entries = modal.visible_entries();
    let mut lines = vec![
        Line::from(Span::styled(
            "Send Attachment",
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        attachment_modal_line(
            modal.focus == AttachmentModalField::Kind,
            "Kind",
            attachment_kind_label(modal.kind),
            theme,
        ),
        attachment_modal_line(
            modal.focus == AttachmentModalField::Picker,
            "Search",
            &modal.picker_query,
            theme,
        ),
        Line::from(Span::styled(
            format!("Folder {}", modal.picker_root.display()),
            Style::default().fg(theme.muted),
        )),
        Line::from(""),
    ];

    let entry_window_start = modal.selected_entry_index.saturating_sub(7);
    for (offset, entry) in visible_entries
        .iter()
        .skip(entry_window_start)
        .take(8)
        .enumerate()
    {
        let index = entry_window_start + offset;
        let selected =
            modal.focus == AttachmentModalField::Picker && index == modal.selected_entry_index;
        let style = if selected {
            Style::default()
                .fg(theme.bg)
                .bg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text)
        };
        let marker = if selected { ">" } else { " " };
        let suffix = if entry.is_dir {
            "/".to_string()
        } else {
            entry
                .size_bytes
                .map(format_bytes)
                .unwrap_or_else(|| "file".to_string())
        };
        lines.push(Line::from(Span::styled(
            format!("{marker} {} {suffix}", entry.name),
            style,
        )));
    }

    if visible_entries.is_empty() {
        lines.push(Line::from(Span::styled(
            "No matching files",
            Style::default().fg(theme.muted),
        )));
    }

    lines.extend([
        Line::from(""),
        attachment_modal_line(
            modal.focus == AttachmentModalField::Send,
            "Selected",
            modal.selected_preview_path().unwrap_or("none"),
            theme,
        ),
        attachment_button_line(
            modal.focus == AttachmentModalField::Send,
            "Send",
            "Enter",
            theme,
        ),
        Line::from(""),
        Line::from(Span::styled(
            "o opens file picker, type filters, Enter selects/sends, Left parent",
            Style::default().fg(theme.muted),
        )),
        modal_status_line(modal.status.as_deref(), modal.error.as_deref(), theme),
    ]);

    let paragraph = Paragraph::new(lines)
        .style(Style::default().bg(theme.surface).fg(theme.text))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, left);
    render_attachment_preview_box(
        frame,
        right,
        state,
        &modal,
        protocol_worker,
        kitty_available,
        theme,
    );
}

fn render_sticker_picker_overlay(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &mut UiState,
    inline_loader: Option<&InlineMediaLoader>,
    kitty_available: bool,
    theme: &Theme,
) {
    let Some(picker) = state.app.sticker_picker.clone() else {
        return;
    };
    let popup = centered_rect(82, 24, area);
    draw_shadow(frame, popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        themed_block(" Stickers ", theme).style(Style::default().bg(theme.surface).fg(theme.text)),
        popup,
    );
    let inner = popup.inner(ratatui::layout::Margin {
        horizontal: 2,
        vertical: 1,
    });
    let split = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
        .split(inner);
    let left = split[0];
    let right = split[1];

    let mut lines = vec![
        Line::from(Span::styled(
            "Sticker Picker",
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];
    if picker.mode == StickerPickerMode::AddPath {
        lines.push(attachment_modal_line(
            true,
            "Path",
            &picker.add_path,
            theme,
        ));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Enter imports, Esc returns to picker",
            Style::default().fg(theme.muted),
        )));
    } else if picker.stickers.is_empty() {
        lines.push(Line::from(Span::styled(
            "No stickers in library",
            Style::default().fg(theme.muted),
        )));
        lines.push(Line::from(""));
        lines.push(attachment_button_line(true, "Add", "press a", theme));
    } else {
        let sticker_window_start = picker.selected_index.saturating_sub(7);
        for (offset, _sticker) in picker
            .stickers
            .iter()
            .skip(sticker_window_start)
            .take(8)
            .enumerate()
        {
            let index = sticker_window_start + offset;
            let selected = index == picker.selected_index;
            let marker = if selected { ">" } else { " " };
            let style = if selected {
                Style::default()
                    .fg(theme.bg)
                    .bg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text)
            };
            lines.push(Line::from(Span::styled(
                format!("{marker} Sticker {}", index + 1),
                style,
            )));
        }
        lines.push(Line::from(""));
        lines.push(attachment_button_line(false, "Add", "press a", theme));
    }
    lines.push(Line::from(""));
    let hint = if picker.mode == StickerPickerMode::AddPath {
        "Enter imports sticker, Esc goes back"
    } else {
        "Enter sends selected sticker, a adds from path, Esc closes"
    };
    lines.push(Line::from(Span::styled(
        hint,
        Style::default().fg(theme.muted),
    )));
    lines.push(modal_status_line(
        picker.status.as_deref(),
        picker.error.as_deref(),
        theme,
    ));
    let paragraph = Paragraph::new(lines)
        .style(Style::default().bg(theme.surface).fg(theme.text))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, left);
    render_sticker_picker_preview_box(
        frame,
        right,
        state,
        &picker,
        inline_loader,
        kitty_available,
        theme,
    );
}

fn render_attachment_preview_box(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &mut UiState,
    modal: &crate::state::AttachmentModalState,
    protocol_worker: Option<&ProtocolWorker>,
    kitty_available: bool,
    theme: &Theme,
) {
    let block = themed_block(" Preview ", theme);
    frame.render_widget(
        block.style(Style::default().bg(theme.bg).fg(theme.text)),
        area,
    );
    let inner = area.inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 1,
    });
    let Some(path) = modal.selected_preview_path() else {
        frame.render_widget(
            Paragraph::new("Choose a file to preview")
                .style(Style::default().bg(theme.bg).fg(theme.muted)),
            inner,
        );
        return;
    };

    match modal.kind {
        MediaKind::Image if kitty_available && state.protocol_type_is_kitty() => {
            render_local_image_file_preview(frame, inner, state, path, protocol_worker, theme);
        }
        MediaKind::Image => {
            render_preview_card(
                frame,
                inner,
                theme,
                "image preview unavailable",
                path,
                Some("Kitty/Ratty image support is not active"),
            );
        }
        MediaKind::Video => {
            render_preview_card(frame, inner, theme, "video", path, Some("opens externally"));
        }
        MediaKind::Audio => {
            render_preview_card(frame, inner, theme, "audio", path, Some("opens externally"));
        }
        MediaKind::Document => {
            render_preview_card(frame, inner, theme, "document", path, Some("opens externally"));
        }
    }
}

fn render_local_image_file_preview(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &mut UiState,
    path: &str,
    protocol_worker: Option<&ProtocolWorker>,
    theme: &Theme,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let key = InlineMediaKey::new(
        "attachment-preview",
        path.to_string(),
        Size::new(area.width, area.height),
    );
    if state.inline_media_cache.get(&key).is_none() {
        if let Some(worker) = protocol_worker {
            match image::open(path) {
                Ok(image) => {
                    state.inline_media_cache.insert_loading(key.clone());
                    worker.request(ProtocolRequest::inline(
                        key.clone(),
                        image,
                        Size::new(area.width, area.height),
                    ));
                }
                Err(error) => state
                    .inline_media_cache
                    .insert_error(key.clone(), format!("failed to preview image: {error}")),
            }
        } else {
            state
                .inline_media_cache
                .insert_error(key.clone(), "image preview unavailable");
        }
    }

    match state.inline_media_cache.get(&key) {
        Some(InlineMediaState::Ready(protocol)) => {
            frame.render_widget(Image::new(protocol), area);
        }
        Some(InlineMediaState::Error(error)) => {
            render_preview_card(frame, area, theme, "image", path, Some(error.as_str()));
        }
        Some(InlineMediaState::Loading) | None => {
            frame.render_widget(
                Paragraph::new("loading preview...")
                    .style(Style::default().bg(theme.bg).fg(theme.muted)),
                area,
            );
        }
    }
}

fn render_sticker_picker_preview_box(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &mut UiState,
    picker: &StickerPickerState,
    inline_loader: Option<&InlineMediaLoader>,
    kitty_available: bool,
    theme: &Theme,
) {
    let block = themed_block(" Preview ", theme);
    frame.render_widget(
        block.style(Style::default().bg(theme.bg).fg(theme.text)),
        area,
    );
    let inner = area.inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 1,
    });

    let Some(sticker) = picker.selected_sticker() else {
        frame.render_widget(
            Paragraph::new("Add a sticker to preview it here")
                .style(Style::default().bg(theme.bg).fg(theme.muted)),
            inner,
        );
        return;
    };

    let preview_height = inner.height.saturating_sub(3).max(1);
    let preview_area = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: preview_height,
    };
    let metadata_area = Rect {
        x: inner.x,
        y: inner.y.saturating_add(preview_height),
        width: inner.width,
        height: inner.height.saturating_sub(preview_height),
    };

    if kitty_available && state.protocol_type_is_kitty() {
        if let Some(message) = picker.selected_preview_message() {
            render_inline_preview_box(frame, preview_area, state, &message, theme, inline_loader);
        }
    } else {
        frame.render_widget(
            Paragraph::new("sticker preview unavailable")
                .style(Style::default().bg(theme.bg).fg(theme.muted)),
            preview_area,
        );
    }

    let metadata = vec![
        Line::from(Span::styled(
            sticker.name.as_deref().unwrap_or("sticker"),
            Style::default().fg(theme.muted),
        )),
        Line::from(Span::styled(
            format!(
                "{}  {}",
                format_bytes(sticker.size_bytes.max(0) as u64),
                short_hash(&sticker.file_hash)
            ),
            Style::default().fg(theme.muted),
        )),
    ];
    frame.render_widget(
        Paragraph::new(metadata)
            .style(Style::default().bg(theme.bg).fg(theme.text))
            .wrap(Wrap { trim: true }),
        metadata_area,
    );
}

fn render_preview_card(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &Theme,
    title: &str,
    path: &str,
    note: Option<&str>,
) {
    let name = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path);
    let mut lines = vec![
        Line::from(Span::styled(
            title,
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(name.to_string(), Style::default().fg(theme.text))),
    ];
    if let Ok(metadata) = fs::metadata(path) {
        lines.push(Line::from(Span::styled(
            format_bytes(metadata.len()),
            Style::default().fg(theme.muted),
        )));
    }
    if let Some(note) = note {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            note.to_string(),
            Style::default().fg(theme.muted),
        )));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().bg(theme.bg).fg(theme.text))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn render_attachment_actions_overlay(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &UiState,
    theme: &Theme,
) {
    let Some(modal) = state.app.attachment_actions.as_ref() else {
        return;
    };
    let popup = centered_rect(72, 18, area);
    draw_shadow(frame, popup);
    frame.render_widget(Clear, popup);
    let lines = vec![
        Line::from(Span::styled(
            modal.file_name.clone(),
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            format!("{}  {}", modal.content_type, short_hash(&modal.file_hash)),
            Style::default().fg(theme.muted),
        )),
        Line::from(""),
        attachment_button_line(
            modal.focus == AttachmentActionField::View,
            "View",
            "open viewer",
            theme,
        ),
        attachment_modal_line(
            modal.focus == AttachmentActionField::SavePath,
            "Save to",
            &modal.target_path,
            theme,
        ),
        attachment_button_line(
            modal.focus == AttachmentActionField::Save,
            "Save",
            "write to path",
            theme,
        ),
        attachment_button_line(
            modal.focus == AttachmentActionField::Open,
            "Open",
            "default app",
            theme,
        ),
        attachment_button_line(
            modal.focus == AttachmentActionField::CopyHash,
            "Copy hash",
            "clipboard",
            theme,
        ),
        attachment_button_line(
            modal.focus == AttachmentActionField::Retry,
            "Retry fetch",
            "direct chat",
            theme,
        ),
        if modal.content_type == "sticker" {
            if modal.sticker_saved {
                attachment_button_line(false, "Saved", "in sticker library", theme)
            } else {
                attachment_button_line(
                    modal.focus == AttachmentActionField::SaveSticker,
                    "Save sticker",
                    "add to library",
                    theme,
                )
            }
        } else {
            Line::from("")
        },
        attachment_button_line(
            modal.focus == AttachmentActionField::Close,
            "Close",
            "dismiss",
            theme,
        ),
        Line::from(""),
        Line::from(Span::styled(
            "Tab moves focus, Enter activates, Esc closes",
            Style::default().fg(theme.muted),
        )),
        modal_status_line(modal.status.as_deref(), modal.error.as_deref(), theme),
    ];
    let paragraph = Paragraph::new(lines)
        .block(themed_block(" Attachment ", theme))
        .style(Style::default().bg(theme.surface).fg(theme.text))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, popup);
}

fn render_context_menu_overlay(frame: &mut Frame<'_>, area: Rect, state: &UiState, theme: &Theme) {
    let Some(menu) = state.app.context_menu.as_ref() else {
        return;
    };
    let actions = context_menu_actions(menu);
    let height = (actions.len() as u16).saturating_add(3).max(5);
    let popup = centered_rect(38, height, area);
    draw_shadow(frame, popup);
    frame.render_widget(Clear, popup);
    let lines = actions
        .iter()
        .enumerate()
        .map(|(index, action)| {
            let selected = index == menu.selected_index;
            let marker = if selected { ">" } else { " " };
            let style = if selected {
                Style::default()
                    .fg(theme.bg)
                    .bg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text)
            };
            Line::from(Span::styled(
                format!("{marker} {}", context_menu_action_label(*action)),
                style,
            ))
        })
        .collect::<Vec<_>>();
    let paragraph = Paragraph::new(lines)
        .block(themed_block(" Menu ", theme))
        .style(Style::default().bg(theme.surface).fg(theme.text))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, popup);
}

fn render_media_viewer_overlay(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &mut UiState,
    protocol_worker: Option<&ProtocolWorker>,
    kitty_available: bool,
    theme: &Theme,
) {
    let Some(viewer) = state.app.media_viewer.clone() else {
        return;
    };
    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.bg).fg(theme.text)),
        area,
    );

    let block = themed_block(format!(" Media Viewer - {} ", viewer.file_name), theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(30), Constraint::Length(38)])
        .split(inner);

    match viewer.kind {
        MediaViewerKind::Image => render_media_viewer_image(
            frame,
            layout[0],
            state,
            &viewer,
            protocol_worker,
            kitty_available,
            theme,
        ),
        MediaViewerKind::Video => render_media_viewer_placeholder(
            frame,
            layout[0],
            "Terminal video playback is not enabled. Open externally to play.",
            theme,
        ),
        MediaViewerKind::Audio => render_media_viewer_placeholder(
            frame,
            layout[0],
            "Audio playback uses your external default app.",
            theme,
        ),
        MediaViewerKind::Document => render_media_viewer_placeholder(
            frame,
            layout[0],
            "Documents open in your external default app.",
            theme,
        ),
    }

    render_media_viewer_details(frame, layout[1], &viewer, theme);
}

fn render_media_viewer_image(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &mut UiState,
    viewer: &crate::state::MediaViewerState,
    protocol_worker: Option<&ProtocolWorker>,
    kitty_available: bool,
    theme: &Theme,
) {
    let block = themed_block(" Image ", theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if !kitty_available {
        render_media_viewer_placeholder(
            frame,
            inner,
            "Kitty image protocol unavailable. Metadata and file actions are still available.",
            theme,
        );
        return;
    }

    let Some(loaded) = state
        .viewer_image
        .as_ref()
        .filter(|loaded| loaded.file_hash == viewer.file_hash)
    else {
        render_media_viewer_placeholder(frame, inner, "Loading image...", theme);
        return;
    };

    let size = Size::new(inner.width.max(1), inner.height.max(1));
    let key = MediaViewerKey::new(
        viewer.file_hash.clone(),
        size,
        viewer.zoom_percent,
        viewer.pan_x,
        viewer.pan_y,
    );

    let request_image = if state.viewer_protocol_key.as_ref() != Some(&key) {
        Some(Arc::clone(&loaded.image))
    } else {
        None
    };

    if let Some(request_image) = request_image {
        request_viewer_protocol_key(state, key.clone());
        if let Some(worker) = protocol_worker {
            worker.request(ProtocolRequest::viewer(key.clone(), request_image, size));
        }
    }

    match state.viewer_protocol.as_ref() {
        Some(protocol) if protocol.id == ProtocolRequestId::Viewer(key) => {
            let image = Image::new(&protocol.protocol);
            frame.render_widget(image, inner);
        }
        Some(protocol) => {
            let image = Image::new(&protocol.protocol);
            frame.render_widget(image, inner);
        }
        None => render_media_viewer_placeholder(frame, inner, "Preparing image...", theme),
    }
}

fn render_media_viewer_placeholder(frame: &mut Frame<'_>, area: Rect, text: &str, theme: &Theme) {
    let paragraph = Paragraph::new(text)
        .style(Style::default().bg(theme.bg).fg(theme.muted))
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true });
    frame.render_widget(paragraph, area);
}

fn render_media_viewer_details(
    frame: &mut Frame<'_>,
    area: Rect,
    viewer: &crate::state::MediaViewerState,
    theme: &Theme,
) {
    let mut lines = vec![
        Line::from(Span::styled(
            viewer.kind.label().to_string(),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(viewer.file_name.clone()),
        Line::from(Span::styled(
            format!("{}  {}", viewer.content_type, short_hash(&viewer.file_hash)),
            Style::default().fg(theme.muted),
        )),
    ];

    if let Some(size) = viewer.metadata.as_deref().and_then(media_size_label) {
        lines.push(Line::from(format!("size {size}")));
    }
    if let Some(metadata) = viewer
        .metadata
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        lines.push(Line::from(Span::styled(
            short_identifier(metadata, 80),
            Style::default().fg(theme.muted),
        )));
    }

    lines.extend([
        Line::from(""),
        Line::from(Span::styled(
            media_viewer_view_label(viewer),
            Style::default().fg(theme.muted),
        )),
        viewer_input_line(
            viewer,
            MediaViewerAction::SavePath,
            "Save to",
            &viewer.target_path,
            theme,
        ),
        viewer_button_line(
            viewer,
            MediaViewerAction::Save,
            "Save",
            "write to path",
            theme,
        ),
        viewer_button_line(
            viewer,
            MediaViewerAction::Open,
            "Open",
            "default app",
            theme,
        ),
        viewer_button_line(
            viewer,
            MediaViewerAction::CopyHash,
            "Copy hash",
            "clipboard",
            theme,
        ),
        viewer_button_line(
            viewer,
            MediaViewerAction::Retry,
            "Retry fetch",
            "direct chat",
            theme,
        ),
        viewer_button_line(viewer, MediaViewerAction::Close, "Close", "dismiss", theme),
        Line::from(""),
        Line::from(Span::styled(
            media_viewer_shortcuts_label(),
            Style::default().fg(theme.muted),
        )),
        modal_status_line(viewer.status.as_deref(), viewer.error.as_deref(), theme),
    ]);

    let paragraph = Paragraph::new(lines)
        .block(themed_block(" Details ", theme))
        .style(Style::default().bg(theme.surface).fg(theme.text))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn media_viewer_view_label(viewer: &crate::state::MediaViewerState) -> String {
    if viewer.zoom_percent <= 100 || (viewer.pan_x == 0 && viewer.pan_y == 0) {
        format!("zoom {}%  centered", viewer.zoom_percent)
    } else {
        format!(
            "zoom {}%  moved {:+},{:+}",
            viewer.zoom_percent, viewer.pan_x, viewer.pan_y
        )
    }
}

fn media_viewer_shortcuts_label() -> &'static str {
    "+/- zoom  arrows move image when zoomed  0/R reset  s/o/c/r actions  Esc close"
}

fn viewer_input_line(
    viewer: &crate::state::MediaViewerState,
    field: MediaViewerAction,
    label: &str,
    value: &str,
    theme: &Theme,
) -> Line<'static> {
    let focused = viewer.focus == field;
    Line::from(vec![
        Span::styled(
            if focused { "> " } else { "  " },
            if focused {
                Style::default().fg(theme.accent)
            } else {
                Style::default().fg(theme.muted)
            },
        ),
        Span::styled(format!("{label}: "), Style::default().fg(theme.muted)),
        Span::styled(
            if value.is_empty() {
                "<path>".to_string()
            } else {
                value.to_string()
            },
            Style::default().fg(theme.text),
        ),
    ])
}

fn viewer_button_line(
    viewer: &crate::state::MediaViewerState,
    field: MediaViewerAction,
    label: &str,
    hint: &str,
    theme: &Theme,
) -> Line<'static> {
    let focused = viewer.focus == field;
    let style = if focused {
        Style::default()
            .fg(theme.bg)
            .bg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.text)
    };
    Line::from(vec![
        Span::styled(
            format!("{} {label}", if focused { ">" } else { " " }),
            style,
        ),
        Span::styled(format!("  {hint}"), Style::default().fg(theme.muted)),
    ])
}

fn attachment_modal_line<'a>(
    focused: bool,
    label: &'a str,
    value: &'a str,
    theme: &Theme,
) -> Line<'a> {
    let label_style = if focused {
        Style::default()
            .fg(theme.bg)
            .bg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.muted)
    };
    Line::from(vec![
        Span::styled(format!("{label:<9}"), label_style),
        Span::raw(" "),
        Span::styled(value.to_string(), Style::default().fg(theme.text)),
    ])
}

fn attachment_button_line<'a>(
    focused: bool,
    label: &'a str,
    detail: &'a str,
    theme: &Theme,
) -> Line<'a> {
    let marker = if focused { ">" } else { " " };
    let style = if focused {
        Style::default()
            .fg(theme.bg)
            .bg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.text)
    };
    Line::from(vec![
        Span::styled(format!("{marker} {label:<12}"), style),
        Span::raw(" "),
        Span::styled(detail.to_string(), Style::default().fg(theme.muted)),
    ])
}

fn modal_status_line<'a>(
    status: Option<&'a str>,
    error: Option<&'a str>,
    theme: &Theme,
) -> Line<'a> {
    if let Some(error) = error {
        Line::from(Span::styled(
            error.to_string(),
            Style::default().fg(theme.error),
        ))
    } else if let Some(status) = status {
        Line::from(Span::styled(
            status.to_string(),
            Style::default().fg(theme.accent),
        ))
    } else {
        Line::from("")
    }
}

fn render_settings_overlay(frame: &mut Frame<'_>, area: Rect, state: &UiState, theme: &Theme) {
    let Some(modal) = state.app.settings.as_ref() else {
        return;
    };

    let popup = centered_rect(92, 30, area);
    draw_shadow(frame, popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        themed_block(" Settings ", theme)
            .border_style(Style::default().fg(theme.accent))
            .style(Style::default().bg(theme.surface)),
        popup,
    );

    let inner = inset_rect(popup, 2);
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(22), Constraint::Min(20)])
        .split(inner);

    let menu_lines = SettingsSection::ALL
        .iter()
        .enumerate()
        .map(|(index, section)| {
            let focused = modal.pane == SettingsPane::Menu
                && modal.focus == SettingsField::Section(index);
            let active = modal.section == *section;
            let marker = if active { ">" } else { " " };
            Line::from(Span::styled(
                format!("{marker} {}", section.label()),
                if focused {
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD)
                } else if active {
                    Style::default().fg(theme.warning)
                } else {
                    Style::default().fg(theme.text)
                },
            ))
        })
        .collect::<Vec<_>>();

    frame.render_widget(
        Paragraph::new(menu_lines)
            .block(themed_block(" Sections ", theme))
            .style(Style::default().bg(theme.surface).fg(theme.text)),
        columns[0],
    );

    let mut lines = match modal.section {
        SettingsSection::Profile => settings_profile_lines(modal, theme),
        SettingsSection::Peers => settings_peer_lines(modal, theme),
        SettingsSection::Connectivity => settings_connectivity_lines(modal, theme),
        SettingsSection::Theme => settings_theme_lines(modal, theme),
        SettingsSection::Stickers => settings_sticker_lines(modal, theme),
        SettingsSection::Media => settings_media_lines(state, theme),
        SettingsSection::About => settings_about_lines(theme),
    };

    lines.push(Line::from(""));
    if let Some(error) = modal.error.as_ref() {
        lines.push(Line::from(Span::styled(
            error.clone(),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )));
    } else if let Some(status) = modal.status.as_ref() {
        lines.push(Line::from(Span::styled(
            status.clone(),
            Style::default().fg(theme.accent),
        )));
    }
    lines.push(Line::from(Span::styled(
        "Esc close | Tab menu/content | Up/Down move focus | Enter activate",
        Style::default().fg(theme.muted),
    )));

    frame.render_widget(
        Paragraph::new(lines)
            .block(themed_block(modal.section.label(), theme))
            .style(Style::default().bg(theme.surface).fg(theme.text))
            .wrap(Wrap { trim: false }),
        columns[1],
    );
}

fn settings_profile_lines(
    modal: &crate::state::SettingsModalState,
    theme: &Theme,
) -> Vec<Line<'static>> {
    vec![
        Line::from("Profile"),
        settings_input_line(
            modal,
            SettingsField::ProfileAlias,
            "Alias",
            &modal.profile_alias,
            theme,
        ),
        settings_input_line(
            modal,
            SettingsField::ProfileAvatar,
            "Avatar path",
            &modal.profile_avatar_path,
            theme,
        ),
        settings_button_line(modal, SettingsField::ProfileSave, "Save profile", theme),
    ]
}

fn settings_peer_lines(
    modal: &crate::state::SettingsModalState,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from("Trusted peers")];
    if modal.trusted_peers.is_empty() {
        lines.push(Line::from(Span::styled(
            "No trusted peers",
            Style::default().fg(theme.muted),
        )));
    } else {
        for (index, peer) in modal.trusted_peers.iter().take(6).enumerate() {
            lines.push(settings_button_line(
                modal,
                SettingsField::Peer(index),
                &short_identifier(peer, 48),
                theme,
            ));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from("Friends"));
    if modal.friends.is_empty() {
        lines.push(Line::from(Span::styled(
            "No friends yet",
            Style::default().fg(theme.muted),
        )));
    } else {
        for (index, friend) in modal.friends.iter().take(6).enumerate() {
            let pinned = if modal.pinned_peers.contains(friend) {
                " pinned"
            } else {
                ""
            };
            lines.push(settings_button_line(
                modal,
                SettingsField::Friend(index),
                &format!("{friend}{pinned}"),
                theme,
            ));
        }
    }
    lines
}

fn settings_connectivity_lines(
    modal: &crate::state::SettingsModalState,
    theme: &Theme,
) -> Vec<Line<'static>> {
    vec![
        Line::from("Connectivity mode"),
        settings_button_line(
            modal,
            SettingsField::ConnectivityMode(ConnectivityMode::Invisible),
            &format!(
                "{} Invisible",
                selected_marker(modal.connectivity.mode == ConnectivityMode::Invisible)
            ),
            theme,
        ),
        settings_button_line(
            modal,
            SettingsField::ConnectivityMode(ConnectivityMode::Lan),
            &format!(
                "{} LAN",
                selected_marker(modal.connectivity.mode == ConnectivityMode::Lan)
            ),
            theme,
        ),
        settings_button_line(
            modal,
            SettingsField::ConnectivityMode(ConnectivityMode::Reachable),
            &format!(
                "{} Reachable",
                selected_marker(modal.connectivity.mode == ConnectivityMode::Reachable)
            ),
            theme,
        ),
        Line::from(format!(
            "mDNS={} GitHub={} NAT={} punch={}",
            modal.connectivity.mdns_enabled,
            modal.connectivity.github_sync_enabled,
            modal.connectivity.nat_keepalive_enabled,
            modal.connectivity.punch_assist_enabled
        )),
        settings_button_line(
            modal,
            SettingsField::ConnectivitySave,
            "Save connectivity",
            theme,
        ),
    ]
}

fn settings_theme_lines(
    modal: &crate::state::SettingsModalState,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from("Presets")];
    for (index, preset) in modal.theme_presets.iter().take(8).enumerate() {
        let selected = modal.selected_preset.as_deref() == Some(preset.key.as_str());
        lines.push(settings_button_line(
            modal,
            SettingsField::ThemePreset(index),
            &format!(
                "{} {} ({})",
                selected_marker(selected),
                preset.name,
                preset.source
            ),
            theme,
        ));
    }
    lines.push(settings_button_line(
        modal,
        SettingsField::ThemeApply,
        "Apply selected preset",
        theme,
    ));
    lines.push(Line::from(""));
    lines.push(Line::from("Custom theme"));
    lines.push(settings_input_line(
        modal,
        SettingsField::ThemeName,
        "Name",
        &modal.theme_custom_name,
        theme,
    ));
    lines.push(settings_input_line(
        modal,
        SettingsField::ThemePrimary,
        "Primary",
        &modal.theme_primary,
        theme,
    ));
    lines.push(settings_input_line(
        modal,
        SettingsField::ThemeSecondary,
        "Secondary",
        &modal.theme_secondary,
        theme,
    ));
    lines.push(settings_input_line(
        modal,
        SettingsField::ThemeText,
        "Text",
        &modal.theme_text,
        theme,
    ));
    lines.push(settings_button_line(
        modal,
        SettingsField::ThemeCreateCustom,
        "Create custom theme",
        theme,
    ));
    lines
}

fn settings_sticker_lines(
    modal: &crate::state::SettingsModalState,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from("Sticker library")];
    if modal.stickers.is_empty() {
        lines.push(Line::from(Span::styled(
            "No stickers yet",
            Style::default().fg(theme.muted),
        )));
    } else {
        for (index, sticker) in modal.stickers.iter().take(8).enumerate() {
            let name = sticker.name.as_deref().unwrap_or("sticker");
            let selection = if modal.sticker_is_selected(index) {
                "[selected] "
            } else {
                ""
            };
            lines.push(settings_button_line(
                modal,
                SettingsField::Sticker(index),
                &format!(
                    "{}{}  {}  {}",
                    selection,
                    short_identifier(name, 24),
                    format_bytes(sticker.size_bytes.max(0) as u64),
                    short_hash(&sticker.file_hash)
                ),
                theme,
            ));
        }
    }
    lines.push(Line::from(""));
    lines.push(settings_input_line(
        modal,
        SettingsField::StickerPath,
        "Import path",
        &modal.sticker_path,
        theme,
    ));
    lines.push(settings_button_line(
        modal,
        SettingsField::StickerImport,
        "Import sticker",
        theme,
    ));
    lines.push(settings_button_line(
        modal,
        SettingsField::StickerDelete,
        "Delete selected sticker",
        theme,
    ));
    lines
}

fn settings_media_lines(state: &UiState, theme: &Theme) -> Vec<Line<'static>> {
    vec![
        Line::from("Media diagnostics"),
        Line::from(format!("Protocol: {:?}", state.protocol_type)),
        Line::from(format!(
            "Kitty media: {}",
            if state.protocol_type_is_kitty() {
                "yes"
            } else {
                "no"
            }
        )),
        Line::from(format!("Screen frames received: {}", state.received_frames)),
        Line::from(format!("Decoded frames: {}", state.decoded_frames)),
        Line::from(format!(
            "Remote video frames: {} received, {} decoded",
            state.remote_video_received_frames, state.remote_video_decoded_frames
        )),
        Line::from(format!(
            "Pending frame drops: {}",
            state.pending_frame_drops
        )),
        Line::from(format!("Decoder errors: {}", state.decoder_errors)),
        Line::from(Span::styled(
            "Read-only for this iteration",
            Style::default().fg(theme.muted),
        )),
    ]
}

fn settings_about_lines(theme: &Theme) -> Vec<Line<'static>> {
    vec![
        Line::from("RChat TUI"),
        Line::from("Terminal client for the existing RChat identity and 1:1 chat flow."),
        Line::from("Settings Foundation slice: profile, peers, connectivity, theme, stickers."),
        Line::from(Span::styled(
            "Group chat, live media, and polish work are tracked in tui-checklist.md.",
            Style::default().fg(theme.muted),
        )),
    ]
}

fn settings_input_line(
    modal: &crate::state::SettingsModalState,
    field: SettingsField,
    label: &str,
    value: &str,
    theme: &Theme,
) -> Line<'static> {
    let focused = modal.focus == field;
    Line::from(vec![
        Span::styled(
            if focused { "> " } else { "  " },
            Style::default().fg(theme.accent),
        ),
        Span::styled(format!("{label}: "), Style::default().fg(theme.muted)),
        Span::styled(
            value.to_string(),
            if focused {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text)
            },
        ),
    ])
}

fn settings_button_line(
    modal: &crate::state::SettingsModalState,
    field: SettingsField,
    label: &str,
    theme: &Theme,
) -> Line<'static> {
    let focused = modal.focus == field;
    Line::from(Span::styled(
        format!("{}{}", if focused { "> " } else { "  " }, label),
        if focused {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text)
        },
    ))
}

fn selected_marker(selected: bool) -> &'static str {
    if selected {
        "[x]"
    } else {
        "[ ]"
    }
}

fn connectivity_mode_label(mode: ConnectivityMode) -> &'static str {
    match mode {
        ConnectivityMode::Invisible => "Invisible",
        ConnectivityMode::Lan => "LAN",
        ConnectivityMode::Reachable => "Reachable",
        ConnectivityMode::Custom => "Custom",
    }
}

fn render_new_person_overlay(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &mut UiState,
    kitty_available: bool,
    theme: &Theme,
) {
    let Some(modal) = state.app.new_person.as_ref() else {
        return;
    };

    let popup = centered_rect(92, 28, area);
    draw_shadow(frame, popup);
    frame.render_widget(Clear, popup);

    let block = themed_block(" New Person ", theme);
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(48),
            Constraint::Length(NEW_PERSON_QR_WIDTH.saturating_add(2)),
        ])
        .split(inner);

    let lines = new_person_lines(modal, state, theme);
    let paragraph = Paragraph::new(lines)
        .style(Style::default().bg(theme.bg).fg(theme.text))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, columns[0]);

    render_new_person_qr(frame, columns[1], state, kitty_available, theme);
}

fn new_person_lines(
    modal: &crate::state::NewPersonModalState,
    state: &UiState,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(
            new_person_step_title(modal.step),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];

    match modal.step {
        NewPersonStep::SelectNetwork => {
            lines.push(Line::from("How do you want to connect?"));
            lines.push(Line::from(""));
            lines.push(new_person_button_line(
                modal,
                NewPersonField::LocalNetwork,
                "Local network scan",
                "Find nearby RChat peers via mDNS",
                theme,
            ));
            lines.push(new_person_button_line(
                modal,
                NewPersonField::OnlineNetwork,
                "Internet invite",
                "GitHub/Gist or temporary invite link",
                theme,
            ));
        }
        NewPersonStep::LocalScan => {
            lines.push(Line::from("Nearby peers"));
            lines.push(Line::from(""));
            if state.app.local_peers.is_empty() {
                lines.push(Line::from(Span::styled(
                    "Scanning... no peers seen yet",
                    Style::default().fg(theme.muted),
                )));
            } else {
                for (index, peer) in state.app.local_peers.iter().enumerate() {
                    let first_addr = peer.addresses.first().map(String::as_str).unwrap_or("-");
                    let waiting = modal.waiting_peer_id.as_deref() == Some(peer.peer_id.as_str());
                    let label = if waiting { "Connecting" } else { "Connect" };
                    lines.push(new_person_button_line(
                        modal,
                        NewPersonField::LocalPeer(index),
                        label,
                        format!("{}  {}", short_identifier(&peer.peer_id, 24), first_addr),
                        theme,
                    ));
                }
            }
        }
        NewPersonStep::Online => {
            lines.push(Line::from("Choose an invite flow"));
            lines.push(Line::from(""));
            lines.push(new_person_button_line(
                modal,
                NewPersonField::CreateInvite,
                "Create GitHub/Gist invite",
                "Generate password and publish an invite",
                theme,
            ));
            lines.push(new_person_button_line(
                modal,
                NewPersonField::AcceptInvite,
                "Accept GitHub/Gist invite",
                "Enter inviter username and password",
                theme,
            ));
            lines.push(new_person_button_line(
                modal,
                NewPersonField::TemporaryChat,
                "Temporary DM invite",
                "Create, redeem, or cancel a short-lived DM link",
                theme,
            ));
        }
        NewPersonStep::CreateInviteUser => {
            lines.push(Line::from("Who is this invite for?"));
            lines.push(Line::from(""));
            lines.push(new_person_input_line(
                modal,
                NewPersonField::InviteeUsername,
                "Invitee GitHub username",
                &modal.invitee_username,
                false,
                theme,
            ));
            lines.push(Line::from(""));
            lines.push(new_person_button_line(
                modal,
                NewPersonField::CreateInviteNext,
                "Next",
                "Generate password and QR",
                theme,
            ));
        }
        NewPersonStep::CreateInviteCode => {
            lines.push(Line::from(format!(
                "Invitee: {}",
                modal.invitee_username.trim()
            )));
            lines.push(Line::from(""));
            lines.push(Line::from("Share this password:"));
            lines.push(Line::from(Span::styled(
                modal.create_invite_password.clone(),
                Style::default()
                    .fg(theme.warning)
                    .add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            lines.push(new_person_button_line(
                modal,
                NewPersonField::CreateInviteConfirm,
                "Publish invite",
                "Create the GitHub/Gist invite",
                theme,
            ));
        }
        NewPersonStep::AcceptInviteUser => {
            lines.push(Line::from("Who invited you?"));
            lines.push(Line::from(""));
            lines.push(new_person_input_line(
                modal,
                NewPersonField::InviterUsername,
                "Inviter GitHub username",
                &modal.inviter_username,
                false,
                theme,
            ));
            lines.push(Line::from(""));
            lines.push(new_person_button_line(
                modal,
                NewPersonField::AcceptInviteNext,
                "Next",
                "Enter or decode the invite password",
                theme,
            ));
        }
        NewPersonStep::AcceptInviteCode => {
            lines.push(Line::from(format!(
                "Inviter: {}",
                modal.inviter_username.trim()
            )));
            lines.push(Line::from(""));
            lines.push(new_person_input_line(
                modal,
                NewPersonField::InvitePassword,
                "Password",
                &modal.invite_password,
                false,
                theme,
            ));
            lines.push(new_person_input_line(
                modal,
                NewPersonField::InviteQrPath,
                "QR image path",
                &modal.invite_qr_path,
                false,
                theme,
            ));
            lines.push(new_person_button_line(
                modal,
                NewPersonField::DecodeInviteQr,
                "Decode QR file",
                "Load password from image",
                theme,
            ));
            lines.push(new_person_button_line(
                modal,
                NewPersonField::RedeemInvite,
                "Redeem invite",
                "Create/open the 1:1 chat",
                theme,
            ));
        }
        NewPersonStep::TemporaryChat => {
            lines.push(Line::from("DM-only temporary invite"));
            lines.push(Line::from(""));
            lines.push(new_person_button_line(
                modal,
                NewPersonField::CreateTemporary,
                "Create temporary invite",
                "Valid for about two minutes",
                theme,
            ));
            if let Some(link) = modal.active_temporary_link.as_deref() {
                lines.push(Line::from(""));
                lines.push(Line::from(format!(
                    "Active link ({}s):",
                    modal.active_temporary_remaining_seconds.unwrap_or_default()
                )));
                lines.push(Line::from(Span::styled(
                    short_identifier(link, 64),
                    Style::default().fg(theme.warning),
                )));
            }
            lines.push(Line::from(""));
            lines.push(new_person_input_line(
                modal,
                NewPersonField::TemporaryLink,
                "Redeem link",
                &modal.temporary_link,
                false,
                theme,
            ));
            lines.push(new_person_input_line(
                modal,
                NewPersonField::TemporaryQrPath,
                "QR image path",
                &modal.temporary_qr_path,
                false,
                theme,
            ));
            lines.push(new_person_button_line(
                modal,
                NewPersonField::DecodeTemporaryQr,
                "Decode QR file",
                "Load temporary link from image",
                theme,
            ));
            lines.push(new_person_button_line(
                modal,
                NewPersonField::RedeemTemporary,
                "Redeem temporary link",
                "Connect this temporary DM",
                theme,
            ));
            lines.push(new_person_button_line(
                modal,
                NewPersonField::CancelTemporary,
                "Cancel active invite",
                "Remove the local temporary invite",
                theme,
            ));
        }
    }

    if let Some(error) = modal.error.as_deref() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            error.to_string(),
            Style::default().fg(theme.error),
        )));
    }
    if let Some(error) = modal.qr_error.as_deref() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            error.to_string(),
            Style::default().fg(theme.error),
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        new_person_shortcuts_label(),
        Style::default().fg(theme.muted),
    )));
    lines
}

fn new_person_shortcuts_label() -> &'static str {
    "Up/Down move | Enter activate | Esc back/close"
}

fn new_person_step_title(step: NewPersonStep) -> &'static str {
    match step {
        NewPersonStep::SelectNetwork => "Select Network",
        NewPersonStep::LocalScan => "Local Scan",
        NewPersonStep::Online => "Internet Invites",
        NewPersonStep::TemporaryChat => "Temporary Chat",
        NewPersonStep::CreateInviteUser => "Create Invite",
        NewPersonStep::CreateInviteCode => "Share Password",
        NewPersonStep::AcceptInviteUser => "Accept Invite",
        NewPersonStep::AcceptInviteCode => "Redeem Invite",
    }
}

fn new_person_button_line(
    modal: &crate::state::NewPersonModalState,
    field: NewPersonField,
    label: impl Into<String>,
    hint: impl Into<String>,
    theme: &Theme,
) -> Line<'static> {
    let focused = modal.focus == field;
    let marker = if focused { ">" } else { " " };
    let label = label.into();
    let hint = hint.into();
    let style = if focused {
        Style::default()
            .fg(theme.bg)
            .bg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.text)
    };
    Line::from(vec![
        Span::styled(format!("{marker} {label}"), style),
        Span::raw("  "),
        Span::styled(hint, Style::default().fg(theme.muted)),
    ])
}

fn new_person_input_line(
    modal: &crate::state::NewPersonModalState,
    field: NewPersonField,
    label: impl Into<String>,
    value: &str,
    secret: bool,
    theme: &Theme,
) -> Line<'static> {
    let focused = modal.focus == field;
    let marker = if focused { ">" } else { " " };
    let label = label.into();
    let display = if secret {
        mask_secret(value)
    } else if value.is_empty() {
        "-".to_string()
    } else {
        value.to_string()
    };
    let label_style = if focused {
        Style::default()
            .fg(theme.bg)
            .bg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.text)
    };
    Line::from(vec![
        Span::styled(format!("{marker} {label}: "), label_style),
        Span::styled(display, Style::default().fg(theme.warning)),
    ])
}

fn render_new_person_qr(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &mut UiState,
    kitty_available: bool,
    theme: &Theme,
) {
    let block = themed_block(" QR ", theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let payload = state
        .app
        .new_person
        .as_ref()
        .and_then(|modal| modal.qr_payload.as_deref());
    let Some(payload) = payload else {
        let paragraph = Paragraph::new("QR appears here")
            .style(Style::default().bg(theme.bg).fg(theme.muted))
            .wrap(Wrap { trim: true });
        frame.render_widget(paragraph, inner);
        return;
    };

    if !kitty_available {
        let paragraph = Paragraph::new("QR preview unavailable\n\nUse the text on the left.")
            .style(Style::default().bg(theme.bg).fg(theme.muted))
            .wrap(Wrap { trim: true });
        frame.render_widget(paragraph, inner);
        return;
    }

    let key = qr_inline_key(
        payload,
        Size::new(NEW_PERSON_QR_WIDTH, NEW_PERSON_QR_HEIGHT),
    );
    match state.inline_media_cache.get(&key) {
        Some(InlineMediaState::Ready(protocol)) => {
            let image = Image::new(protocol);
            frame.render_widget(image, inner);
        }
        Some(InlineMediaState::Error(error)) => {
            let paragraph = Paragraph::new(error.as_str())
                .style(Style::default().bg(theme.bg).fg(theme.error))
                .wrap(Wrap { trim: true });
            frame.render_widget(paragraph, inner);
        }
        Some(InlineMediaState::Loading) | None => {
            let paragraph = Paragraph::new("rendering QR...")
                .style(Style::default().bg(theme.bg).fg(theme.muted));
            frame.render_widget(paragraph, inner);
        }
    }
}

fn render_command_palette(frame: &mut Frame<'_>, area: Rect, state: &UiState, theme: &Theme) {
    let popup = centered_rect(70, 3, area);
    draw_shadow(frame, popup);
    frame.render_widget(Clear, popup);
    let paragraph = Paragraph::new(format!("/{}", state.app.command_input))
        .block(themed_block(" Command ", theme))
        .style(Style::default().bg(theme.surface).fg(theme.text));
    frame.render_widget(paragraph, popup);
}

fn active_chat_title(state: &UiState) -> String {
    state
        .app
        .active_chat_id
        .as_deref()
        .map(|chat_id| {
            state
                .app
                .chats
                .iter()
                .find(|chat| chat.id == chat_id)
                .map(|chat| display_chat_name(&chat.name, &chat.id))
                .unwrap_or_else(|| display_chat_name(chat_id, chat_id))
        })
        .unwrap_or_else(|| "No active chat".to_string())
}

fn active_chat_peer_label(state: &UiState) -> Option<String> {
    state.app.active_chat_id.as_deref().map(|chat_id| {
        state
            .app
            .chats
            .iter()
            .find(|chat| chat.id == chat_id)
            .map(|chat| display_chat_name(&chat.name, &chat.id))
            .unwrap_or_else(|| display_chat_name(chat_id, chat_id))
    })
}

fn display_sender_label(sender: &str, active_peer_label: Option<&str>) -> String {
    if sender.trim().is_empty() || looks_like_raw_id(sender) {
        active_peer_label
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| short_identifier(sender, 18))
    } else {
        short_identifier(sender, 24)
    }
}

fn display_chat_name(name: &str, id: &str) -> String {
    let trimmed = name.trim();
    if !trimmed.is_empty() && trimmed != id && !looks_like_raw_id(trimmed) {
        return short_identifier(trimmed, 28);
    }
    chat_identity::extract_name_from_chat_id(id)
        .filter(|candidate| !candidate.trim().is_empty() && !looks_like_raw_id(candidate))
        .map(|candidate| short_identifier(&candidate, 28))
        .unwrap_or_else(|| short_identifier(id, 28))
}

fn looks_like_raw_id(value: &str) -> bool {
    let value = value.trim();
    value.len() >= 32 && (value.starts_with("12D") || value.starts_with("Qm"))
}

fn short_identifier(value: &str, max_chars: usize) -> String {
    let value = value.trim();
    let char_count = value.chars().count();
    if char_count <= max_chars || max_chars < 8 {
        return value.to_string();
    }

    let head_len = (max_chars - 3) / 2;
    let tail_len = max_chars - 3 - head_len;
    let head: String = value.chars().take(head_len).collect();
    let tail: String = value
        .chars()
        .rev()
        .take(tail_len)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{head}...{tail}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Theme {
    bg: Color,
    surface: Color,
    accent: Color,
    warning: Color,
    text: Color,
    muted: Color,
    error: Color,
}

impl Theme {
    fn rchat() -> Self {
        Self {
            bg: Color::Rgb(18, 22, 29),
            surface: Color::Rgb(28, 34, 44),
            accent: Color::Rgb(82, 183, 136),
            warning: Color::Rgb(235, 203, 139),
            text: Color::Rgb(229, 234, 242),
            muted: Color::Rgb(132, 145, 166),
            error: Color::Rgb(239, 111, 108),
        }
    }

    fn dimmed(self) -> Self {
        Self {
            bg: Color::Rgb(8, 10, 14),
            surface: Color::Rgb(13, 17, 23),
            accent: Color::Rgb(43, 105, 82),
            warning: Color::Rgb(117, 97, 58),
            text: Color::Rgb(94, 105, 122),
            muted: Color::Rgb(58, 66, 80),
            error: Color::Rgb(120, 55, 54),
        }
    }
}

fn app_background_theme(state: &UiState) -> Theme {
    let theme = Theme::rchat();
    if graphics_obscuring_overlay_active(state) {
        theme.dimmed()
    } else {
        theme
    }
}

fn modal_overlay_theme() -> Theme {
    Theme::rchat()
}

fn themed_block(title: impl Into<Line<'static>>, theme: &Theme) -> Block<'static> {
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.muted))
        .style(Style::default().bg(theme.bg).fg(theme.text))
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

fn inset_rect(area: Rect, inset: u16) -> Rect {
    Rect {
        x: area.x.saturating_add(inset),
        y: area.y.saturating_add(inset),
        width: area.width.saturating_sub(inset.saturating_mul(2)),
        height: area.height.saturating_sub(inset.saturating_mul(2)),
    }
}

fn rect_contains(area: Rect, column: u16, row: u16) -> bool {
    column >= area.x
        && column < area.x.saturating_add(area.width)
        && row >= area.y
        && row < area.y.saturating_add(area.height)
}

fn draw_shadow(frame: &mut Frame<'_>, area: Rect) {
    let shadow = Rect {
        x: area.x.saturating_add(1),
        y: area.y.saturating_add(1),
        width: area.width,
        height: area.height,
    };
    if shadow.x < frame.area().width && shadow.y < frame.area().height {
        frame.render_widget(
            Block::default().style(Style::default().bg(Color::Black)),
            shadow,
        );
    }
}

fn render_status(frame: &mut Frame<'_>, area: Rect, state: &UiState, kitty_available: bool) {
    let mut lines = vec![
        Line::from(Span::styled(
            "RChat TUI",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(format!("Status: {}", state.status)),
        Line::from(format!("Protocol: {:?}", state.protocol_type)),
        Line::from(format!(
            "Kitty media: {}",
            if kitty_available { "yes" } else { "no" }
        )),
        Line::from(format!(
            "Connected chats: {}",
            state.connected_chat_ids.len()
        )),
        Line::from(format!("Broadcast: {:?}", state.broadcast_state.phase)),
        Line::from(format!(
            "Incoming: {}",
            state.incoming_session_id.as_deref().unwrap_or("-")
        )),
        Line::from(format!(
            "Active: {}",
            state.active_session_id.as_deref().unwrap_or("-")
        )),
        Line::from(""),
        Line::from(format!("Received frames: {}", state.received_frames)),
        Line::from(format!("Encoded frames: {}", state.encoded_frames)),
        Line::from(format!("Decoded frames: {}", state.decoded_frames)),
        Line::from(format!("Pending drops: {}", state.pending_frame_drops)),
        Line::from(format!("Pre-key deltas: {}", state.decoder_dropped_delta)),
        Line::from(format!("Decode errors: {}", state.decoder_errors)),
        Line::from(format!(
            "Event drops: {}",
            state.event_sink.dropped_events()
        )),
        Line::from(format!(
            "Last seq: {}",
            state
                .last_protocol_seq
                .map(|seq| seq.to_string())
                .unwrap_or_else(|| "-".to_string())
        )),
    ];

    if let Some(peer_event) = &state.last_peer_event {
        lines.push(Line::from(""));
        lines.push(Line::from(peer_event.clone()));
    }
    if let Some(error) = &state.media_error {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            error.clone(),
            Style::default().fg(Color::Red),
        )));
    }
    if state.show_help {
        lines.push(Line::from(""));
        lines.push(Line::from("q quit"));
        lines.push(Line::from("a accept screen"));
        lines.push(Line::from("r reject screen"));
        lines.push(Line::from("e end screen"));
        lines.push(Line::from("? help"));
    }

    let paragraph = Paragraph::new(lines)
        .block(Block::default().title("Control").borders(Borders::ALL))
        .wrap(Wrap { trim: true });
    frame.render_widget(paragraph, area);
}

fn render_media(frame: &mut Frame<'_>, area: Rect, state: &mut UiState, kitty_available: bool) {
    let block = Block::default().title("Screen").borders(Borders::ALL);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    state.media_size = Size::new(inner.width, inner.height);

    if !kitty_available {
        let message = Paragraph::new("Kitty image protocol unavailable; run inside Ratty.")
            .style(Style::default().fg(Color::Yellow))
            .wrap(Wrap { trim: true });
        frame.render_widget(message, inner);
        return;
    }

    if let Some(protocol) = &state.protocol {
        let image = Image::new(&protocol.protocol);
        frame.render_widget(image, inner);
    } else {
        let message = Paragraph::new("Waiting for screen-share frames...")
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(message, inner);
    }
}

fn screen_protocol_seq(response: &ProtocolResponse) -> Option<u32> {
    match &response.id {
        ProtocolRequestId::Screen(seq) => Some(*seq),
        ProtocolRequestId::RemoteVideo { .. }
        | ProtocolRequestId::Inline(_)
        | ProtocolRequestId::Viewer(_) => None,
    }
}
