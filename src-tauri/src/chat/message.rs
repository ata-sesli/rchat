// THIS FILE IS EXPERIMENTAL, DO NOT USE IT ANYWHERE ELSE!
pub struct MessageMetadata {
    pub id: String,
    pub chat_id: String,
    pub peer_id: String,
    pub timestamp: i64,
    pub status: String, // 'pending', 'delivered', 'read'
}
pub enum MessageContent {
    Text(String),
    Photo {
        file_hash: String,
        caption: Option<String>
    }
    Video {
        file_hash: String,
        caption: Option<String>
    }
}