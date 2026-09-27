fn unbounded_audio() {
    let _ = tokio::sync::mpsc::unbounded_channel::<Vec<i16>>();
}

fn unbounded_network() {
    let _ = mpsc::unbounded();
}

fn unbounded_std_playback() {
    let _ = std::sync::mpsc::channel::<Vec<i16>>();
}

fn unbounded_imported_std_playback() {
    let _ = mpsc::channel::<Vec<i16>>();
}

async fn blocking_media_loop() {
    std::thread::sleep(std::time::Duration::from_secs(1));
    receive_frame().await;
}

fn missing_send() {
    todo!("group media");
}

fn missing_decode() {
    unimplemented!();
}
