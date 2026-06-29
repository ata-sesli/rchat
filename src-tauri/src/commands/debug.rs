use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::Manager;

fn sanitize_frontend_log(message: &str) -> String {
    message
        .replace(['\r', '\n'], " ")
        .chars()
        .take(2_000)
        .collect::<String>()
}

fn append_frontend_log_line(path: &Path, message: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let sanitized = sanitize_frontend_log(message);
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "[{}] {}", timestamp_ms, sanitized)
}

#[tauri::command]
pub fn frontend_log(app_handle: tauri::AppHandle, message: String) -> Result<(), String> {
    let sanitized = sanitize_frontend_log(&message);
    println!("{}", sanitized);
    let log_path = app_handle
        .path()
        .app_data_dir()
        .map_err(|e| format!("failed to resolve app data dir: {}", e))?
        .join("frontend.log");
    append_frontend_log_line(&log_path, &message)
        .map_err(|e| format!("failed to append frontend log: {}", e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontend_log_file_entry_is_sanitized_and_appended() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("frontend.log");

        append_frontend_log_line(&path, "first\nline").expect("writes first line");
        append_frontend_log_line(&path, "second\r\nline").expect("writes second line");

        let contents = std::fs::read_to_string(path).expect("log contents");
        assert!(contents.contains("first line"));
        assert!(contents.contains("second  line"));
        assert_eq!(contents.lines().count(), 2);
    }
}
