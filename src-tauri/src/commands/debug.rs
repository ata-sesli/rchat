#[tauri::command]
pub fn frontend_log(message: String) -> Result<(), String> {
    let sanitized = message.replace(['\r', '\n'], " ");
    println!("{}", sanitized.chars().take(2_000).collect::<String>());
    Ok(())
}
