async fn bounded_media_loop() {
    let _ = tokio::sync::mpsc::channel::<Vec<i16>>(4);
    tokio::time::sleep(std::time::Duration::from_millis(1)).await;
}

fn dedicated_capture_thread() {
    std::thread::sleep(std::time::Duration::from_millis(1));
}

fn other_bounded_channels() {
    let _ = std::sync::mpsc::sync_channel::<Vec<i16>>(4);
    let _ = tokio::sync::mpsc::channel::<Vec<i16>>(4);
    let _ = mpsc::channel::<Vec<i16>>(4);
    let _ = custom::channel();
    let _ = channel();
}

async fn offloaded_work() {
    tokio::task::spawn_blocking(|| {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }).await;
}

fn text_is_not_code() {
    let _ = "todo!(); mpsc::unbounded(); std::thread::sleep(duration);";
    // unimplemented!();
}
