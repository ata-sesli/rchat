mod network;
use tokio::sync::mpsc;
use tokio::sync::Mutex;
use tauri::{Manager, State, Runtime};

// This struct holds the Sender channel.
// We wrap it in Mutex so multiple UI threads can use it safely.
pub struct NetworkState {
    pub sender: Mutex<mpsc::Sender<String>>,
}
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
                
                if let Err(e) = network::init(app_handle).await {
                    eprintln!("Failed to start network: {}", e);
                }
                
                println!("Background Services Ready!");
            });

            Ok(())
        })
        // --- End Setup Hook ---
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![greet,send_chat_message])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[tauri::command]
async fn send_chat_message(
    message: String, 
    state: State<'_, NetworkState>
) -> Result<(), String> {
    // 1. Lock the sender
    let tx = state.sender.lock().await;
    
    // 2. Send the message to the background Network Manager
    tx.send(message)
        .await
        .map_err(|e| e.to_string())?;
        
    Ok(())
}