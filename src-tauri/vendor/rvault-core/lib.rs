// --- RVAULT CORE LIBRARY ENTRY POINT ---
// This file declares all the modules containing the shared security and data logic.

// Configuration and Error Handling
pub mod config;
pub mod error;

// Cryptography and Key Management
pub mod crypto;
pub mod keystore;
// If keystore_bin.rs doesn't exist, remove this line
// and fix the imports in your other files.

// Session, Data, and Vault Management
pub mod session;
#[cfg(feature = "storage")]
pub mod storage;
pub mod vault;

// Utilities
pub mod clipboard;
pub mod watcher; // Based on your file list, this seems to be included
// If watcher.rs doesn't exist, remove this line.

// Re-export common types for easier access in other crates (CLI/GUI)
pub use error::{ConfigError, DatabaseError};
pub use vault::VaultEntry;
