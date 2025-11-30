use tauri::Manager; // Import Manager to access app.handle()

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // --- 1. The Setup Hook ---
        .setup(|app| {
            // This runs BEFORE the window appears
            println!("RChat is initializing...");

            // Get a handle to the app if you need it for events/windows later
            let app_handle = app.handle().clone();

            // --- 2. Run Heavy Background Tasks ---
            // We spawn a separate async task so we don't freeze the UI startup
            tauri::async_runtime::spawn(async move {
                println!("Starting Background Services...");
                
                // Example: Initialize your Database here
                // let db = storage::db::init().await;
                
                // Example: Initialize P2P Network
                // let swarm = network::init().await;
                
                println!("Background Services Ready!");
            });

            Ok(())
        })
        // --- End Setup Hook ---
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![greet])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}