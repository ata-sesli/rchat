use thiserror::Error;

#[derive(Error, Debug)]
pub enum DatabaseError {
    #[error("Path Error")]
    Path,
    #[error("I/O Error: {0}")]
    Io(#[from] std::io::Error),
    #[cfg(feature = "storage")]
    #[error("SQLite Error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}
#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("Path Error")]
    Path,
    #[error("I/O Error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON Error: {0}")]
    Json(#[from] serde_json::Error),
}
