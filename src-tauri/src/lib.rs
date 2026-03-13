mod app_state;
mod chat;
mod chat_identity;
mod chat_kind;
mod commands;
mod live;
mod network;
mod oauth;
mod storage;

pub use app_state::{AppState, NetworkState};

use crate::commands::auth::{
    check_auth_status, get_connectivity_settings, init_vault, poll_github_auth, reset_vault,
    save_api_token, set_connectivity_mode, start_github_auth, start_network,
    toggle_online_status, unlock_vault, update_connectivity_settings,
};
use crate::commands::call::{
    accept_video_call, accept_voice_call, end_video_call, end_voice_call, get_connected_chat_ids,
    get_voice_call_state, reject_video_call, reject_voice_call, send_video_call_chunk,
    set_video_call_camera_enabled, set_video_call_muted, set_voice_call_muted, start_video_call,
    start_voice_call,
};
use crate::commands::chat::{
    create_group_chat, get_chat_history, get_chat_latest_times, get_chat_list, get_unread_counts,
    join_group_chat, leave_group_chat, mark_messages_read, save_temporary_chat_to_archive,
    send_message, send_message_to_self,
};
use crate::commands::chat_details::{
    drop_chat_connection, force_chat_reconnect, get_chat_details_overview, get_chat_stats,
    list_chat_files,
};
use crate::commands::envelopes::{
    create_envelope, delete_envelope, get_envelope_assignments, get_envelopes,
    move_chat_to_envelope, update_envelope,
};
use crate::commands::invite::{
    cancel_temporary_invite, create_invite, create_temporary_invite, generate_invite_password,
    get_active_temporary_invite, redeem_and_connect, redeem_temporary_invite,
};
use crate::commands::media::{
    add_sticker, add_stickers_batch, delete_sticker, get_audio_data, get_image_data,
    get_image_from_path, get_video_data, list_stickers, save_audio_to_file, save_document_to_file,
    save_image_to_file, save_sticker_from_message, send_audio_message, send_document_message,
    send_image_message, send_sticker_message, send_video_message,
};
use crate::commands::network_control::{request_connection, set_fast_discovery};
use crate::commands::peer_profile::{
    add_friend, apply_preset, create_custom_theme, delete_custom_theme, delete_peer,
    generate_simple_theme, get_friends, get_peer_aliases, get_pinned_peers, get_selected_preset,
    get_theme, get_trusted_peers, get_user_profile, list_theme_presets, remove_friend,
    toggle_pin_peer, update_custom_theme, update_theme, update_user_profile,
};
use crate::storage::config::ConfigManager;
use tauri::{Emitter, Manager};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            // Bring existing window to front when a second instance is invoked.
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }

            // Forward deep-link URLs from second-launch args to the running instance.
            let urls: Vec<String> = args
                .into_iter()
                .filter(|arg| arg.starts_with("rchat://"))
                .collect();
            if !urls.is_empty() {
                let _ = app.emit("deep-link://new-url", urls);
            }
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            println!("RChat is initializing...");

            let app_dir = app
                .path()
                .app_data_dir()
                .expect("failed to get app data dir");
            std::fs::create_dir_all(&app_dir).expect("failed to create app data dir");
            let mut config_manager = ConfigManager::new(app_dir.clone());

            if config_manager.try_restore_session() {
                println!("Session restored successfully. Vault unlocked.");
            } else {
                println!("Session not restored. Vault locked.");
            }

            let db_connection =
                storage::db::connect_to_db().expect("Failed to initialize database");

            app.manage(AppState {
                config_manager: tokio::sync::Mutex::new(config_manager),
                db_conn: std::sync::Mutex::new(db_connection),
                app_dir: app_dir.clone(),
            });

            println!("[Backend] Setup hook returning Ok");
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            save_api_token,
            check_auth_status,
            get_connectivity_settings,
            set_connectivity_mode,
            update_connectivity_settings,
            toggle_online_status,
            init_vault,
            unlock_vault,
            start_network,
            start_github_auth,
            poll_github_auth,
            reset_vault,
            get_friends,
            get_peer_aliases,
            get_trusted_peers,
            add_friend,
            delete_peer,
            remove_friend,
            get_user_profile,
            get_theme,
            update_theme,
            list_theme_presets,
            apply_preset,
            get_selected_preset,
            generate_simple_theme,
            create_custom_theme,
            update_custom_theme,
            delete_custom_theme,
            update_user_profile,
            get_pinned_peers,
            toggle_pin_peer,
            send_message_to_self,
            send_message,
            get_chat_history,
            create_envelope,
            update_envelope,
            delete_envelope,
            get_envelopes,
            move_chat_to_envelope,
            get_envelope_assignments,
            request_connection,
            set_fast_discovery,
            get_chat_latest_times,
            get_chat_list,
            get_chat_details_overview,
            get_chat_stats,
            list_chat_files,
            drop_chat_connection,
            force_chat_reconnect,
            send_image_message,
            get_image_data,
            get_image_from_path,
            save_image_to_file,
            mark_messages_read,
            get_unread_counts,
            send_document_message,
            save_document_to_file,
            send_video_message,
            get_video_data,
            send_audio_message,
            get_audio_data,
            save_audio_to_file,
            list_stickers,
            add_sticker,
            add_stickers_batch,
            delete_sticker,
            send_sticker_message,
            save_sticker_from_message,
            generate_invite_password,
            create_invite,
            redeem_and_connect,
            create_temporary_invite,
            redeem_temporary_invite,
            get_active_temporary_invite,
            cancel_temporary_invite,
            create_group_chat,
            join_group_chat,
            leave_group_chat,
            save_temporary_chat_to_archive,
            start_voice_call,
            accept_voice_call,
            reject_voice_call,
            end_voice_call,
            set_voice_call_muted,
            start_video_call,
            accept_video_call,
            reject_video_call,
            end_video_call,
            set_video_call_muted,
            set_video_call_camera_enabled,
            send_video_call_chunk,
            get_voice_call_state,
            get_connected_chat_ids,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
