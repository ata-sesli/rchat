use std::env;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use rand::rng;
use rand::{Rng};
use rand::distr::Alphanumeric;
use directories::ProjectDirs;
use std::time::Duration;
use std::time::SystemTime;
use crate::config::Config;

/// Returns the path to the secure directory used for session files.
fn get_session_dir() -> Result<PathBuf, std::io::Error> {
    if let Some(proj_dirs) = ProjectDirs::from("io.github", "ata-sesli", "RVault") {
        // Use a runtime-specific directory if available, otherwise cache.
        let runtime_dir = proj_dirs.runtime_dir().unwrap_or_else(|| proj_dirs.cache_dir());
        let session_dir = runtime_dir.join("sessions");

        fs::create_dir_all(&session_dir)?;
        Ok(session_dir)
    } else {
        Err(std::io::Error::new(std::io::ErrorKind::NotFound, "Could not find project directories"))
    }
}

/// Creates a new session by caching the encryption key in a secure temp file.
/// Returns the new session token, which is the filename.
pub fn start_session(encryption_key: &[u8]) -> Result<String, std::io::Error> {
    let _ = end_session();
    let session_dir = get_session_dir()?;
    let session_token: String = rng()
        .sample_iter(&Alphanumeric)
        .take(48) // A long, random string for the token
        .map(char::from)
        .collect();

    let session_file_path = session_dir.join(&session_token);

    // Create the file and write the key to it.
    let mut file = fs::File::create(&session_file_path)?;

    // On Unix-like systems, set file permissions to be readable/writable only by the owner.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = file.metadata()?.permissions();
        perms.set_mode(0o600); // Read/Write for owner, no access for others.
        fs::set_permissions(&session_file_path, perms)?;
    }

    file.write_all(encryption_key)?;
    Ok(session_token)
}

/// Validates the current session token (from the env var) and returns the encryption key.
pub fn get_key_from_session() -> Result<Vec<u8>, String> {
    // 1. Load the configuration at runtime.
    let config = Config::new().map_err(|e| format!("Failed to load config: {}", e))?;

    // 2. Create the timeout duration from the config value.
    let session_timeout = Duration::from_secs(config.session_timeout.parse::<u64>().unwrap_or(15) * 60);
    let token = read_current()?;

    let session_dir = get_session_dir()
        .map_err(|e| format!("Error accessing session directory: {}", e))?;

    let session_file_path = session_dir.join(&token);

    if session_file_path.exists() {
        // Get the file's metadata to check its timestamp
        let metadata = fs::metadata(&session_file_path)
            .map_err(|e| format!("Failed to read session metadata: {}", e))?;

        let modified_time = metadata.modified()
            .map_err(|e| format!("Failed to get session timestamp: {}", e))?;

        // Calculate how long ago the session file was created/modified
        let age = SystemTime::now().duration_since(modified_time)
            .map_err(|_| "System clock error.".to_string())?;

        // Check if the session has expired
        if age > session_timeout {
            // If it's too old, delete the file and reject the request. ⏰
            fs::remove_file(&session_file_path)
                .map_err(|e| format!("Failed to clean up expired session: {}", e))?;
            return Err("Vault is locked. Your session has expired.".to_string());
        }

        // If not expired, read the key from the file
        fs::read(session_file_path)
            .map_err(|e| format!("Failed to read session key: {}", e))
    } else {
        Err("Vault is locked. Invalid or expired session.".to_string())
    }
}

/// Ends the current session by deleting the session file.
pub fn end_session() -> Result<(), String> {
    let token = read_current()?;
    let session_dir = get_session_dir()
        .map_err(|e| format!("Error accessing session directory: {}", e))?;

    let session_file_path = session_dir.join(token);

    if session_file_path.exists() {
        fs::remove_file(session_file_path)
            .map_err(|e| format!("Failed to remove session file: {}", e))?;
    }
    let current_file_path = session_dir.join("current");
    if current_file_path.exists() {
        let _ = fs::remove_file(current_file_path);
    }

    Ok(())
}
pub fn write_current(token: &str) -> Result<(), String> {
    let p = get_session_dir().map_err(|e| e.to_string())?.join("current");
    std::fs::write(p, token).map_err(|e| format!("Failed to write current token: {e}"))
}
pub fn read_current() -> Result<String, String> {
    let p = get_session_dir().map_err(|e| e.to_string())?.join("current");
    Ok(std::fs::read_to_string(p)
        .map_err(|e| format!("No active session to lock: {e}"))?
        .trim()
        .to_string())
}
