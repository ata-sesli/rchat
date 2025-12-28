use anyhow::Result;
use rvault_core;
use rvault_core::session;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::fs;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;
use x25519_dalek::StaticSecret;

// System Configuration, can be modified only internally.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct SystemConfig {
    pub github_username: Option<String>,
    pub github_token: Option<String>,
    pub public_key: Option<String>,
    pub private_key: Option<String>,
    pub master_hash: Option<String>, // New: For password verification
}

// User Configuration, can be modified via UI.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FriendConfig {
    pub username: String,                   // Gist ID / Username
    pub x25519_pubkey: Option<String>,      // Base64
    pub ed25519_pubkey: Option<String>,     // Base64
    pub leaf_index: usize,                  // HKS Leaf Index
    pub encrypted_leaf_key: Option<String>, // Base64
    pub nonce: Option<String>,              // Base64
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct UserProfile {
    pub alias: Option<String>,
    pub avatar_path: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UserConfig {
    pub dark_mode: bool,
    pub timeout: u16,
    pub identity_private_key: Option<String>, // Ed25519 Secret (Base64)
    pub identity_public_key: Option<String>,  // Ed25519 Public (Base64)
    pub encryption_private_key: Option<String>, // X25519 Secret (Base64)
    pub friends: Vec<FriendConfig>,
    pub hks_nodes: Vec<String>, // Base64 encoded keys of the tree (Depth 12 = 8191 nodes)

    // New Features
    pub profile: UserProfile,
    #[serde(default)]
    pub pinned_peers: Vec<String>,
    #[serde(default)]
    pub is_online: bool, // Offline/Online switch
    #[serde(default)]
    pub libp2p_keypair: Option<String>, // Base64-encoded protobuf keypair for persistent peer ID
}

impl Default for UserConfig {
    fn default() -> Self {
        Self {
            dark_mode: true,
            timeout: 0, // 0 = disabled (manual lock only)
            identity_private_key: None,
            identity_public_key: None,
            encryption_private_key: None,
            friends: vec![],
            hks_nodes: vec![],
            profile: UserProfile::default(),
            pinned_peers: vec![],
            is_online: false,
            libp2p_keypair: None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct Config {
    pub system: SystemConfig,
    pub user: UserConfig,
}

// Manager
pub struct ConfigManager {
    file_path: PathBuf,
    key: Option<[u8; 32]>, // Session Key
}

// Helper to get rchat's keystore path (NOT rvault's path)
fn rchat_keystore_path(app_dir: &PathBuf) -> PathBuf {
    app_dir.join("rchat.keystore")
}

impl ConfigManager {
    pub fn new(app_dir: PathBuf) -> Self {
        Self {
            file_path: app_dir.join("rchat.config"),
            key: None,
        }
    }

    pub fn unlock(&mut self, key: [u8; 32]) {
        self.key = Some(key);
    }

    pub fn is_unlocked(&self) -> bool {
        self.key.is_some()
    }

    pub fn lock(&mut self) {
        self.key = None;
    }

    pub fn exists(&self) -> bool {
        self.file_path.exists()
    }

    /// Initialize new config with password
    pub async fn init(&mut self, password: &str) -> Result<Config> {
        if self.file_path.exists() {
            return Err(anyhow::anyhow!("Config already exists"));
        }

        // Hash the password for storage
        let hashed = rvault_core::crypto::hash_data(password.as_bytes())
            .map_err(|e| anyhow::anyhow!("Hashing failed: {}", e))?;

        // Create rchat's own keystore (not rvault's!)
        let keystore_path = rchat_keystore_path(&self.file_path.parent().unwrap().to_path_buf());
        rvault_core::keystore::create_key_vault(password, &keystore_path)
            .map_err(|e| anyhow::anyhow!("Keystore creation failed: {}", e))?;

        // Load the MEK from our keystore
        let key = rvault_core::keystore::load_key_from_vault(password, &keystore_path)
            .map_err(|e| anyhow::anyhow!("Key derivation failed: {}", e))?;

        // Generate Keys
        let mut csprng = OsRng;

        // 1. Identity Key (Ed25519)
        let identity_sk = SigningKey::generate(&mut csprng);
        let identity_pk = identity_sk.verifying_key();

        // 2. Encryption Key (X25519)
        let encryption_sk = StaticSecret::random_from_rng(&mut csprng);

        // Encode to Base64
        let identity_sk_b64 = BASE64.encode(identity_sk.to_bytes());
        let identity_pk_b64 = BASE64.encode(identity_pk.to_bytes());
        let encryption_sk_b64 = BASE64.encode(encryption_sk.to_bytes());

        let config = Config {
            system: SystemConfig {
                master_hash: Some(hashed.hash),
                ..Default::default()
            },
            user: UserConfig {
                identity_private_key: Some(identity_sk_b64),
                identity_public_key: Some(identity_pk_b64),
                encryption_private_key: Some(encryption_sk_b64),
                ..UserConfig::default()
            },
        };

        // Update state
        self.key = Some(key);

        // Save using the derived key
        Self::save_internal(&config, &key, &self.file_path).await?;

        // Start Session
        if let Ok(token) = session::start_session(&key) {
            let _ = session::write_current(&token);
        }

        Ok(config)
    }

    /// Unlock existing config with password
    pub async fn unlock_with_password(&mut self, password: &str) -> Result<Config> {
        if !self.file_path.exists() {
            return Err(anyhow::anyhow!("Config file not found"));
        }

        let data = fs::read(&self.file_path).await?;
        let wrapper: ConfigWrapper = serde_json::from_slice(&data)?;

        println!(
            "Unlock attempt: Password len={}, Stored Hash len={}",
            password.len(),
            wrapper.master_hash.len()
        );

        // Verify password against stored hash first (for better UX/error messages)
        if !rvault_core::crypto::verify_password(password.as_bytes(), &wrapper.master_hash) {
            return Err(anyhow::anyhow!("Invalid password"));
        }

        // Load MEK from rchat's keystore
        let keystore_path = rchat_keystore_path(&self.file_path.parent().unwrap().to_path_buf());
        let key = rvault_core::keystore::load_key_from_vault(password, &keystore_path)
            .map_err(|e| anyhow::anyhow!("Keystore unlock failed: {}", e))?;

        let decrypted_json =
            rvault_core::crypto::decrypt_with_key(&key, &wrapper.ciphertext, &wrapper.nonce)
                .map_err(|e| anyhow::anyhow!("Decryption failed: {}", e))?;

        let config: Config = serde_json::from_str(&decrypted_json)?;

        // Update state
        self.key = Some(key);

        // Start Session
        if let Ok(token) = session::start_session(&key) {
            let _ = session::write_current(&token);
        }

        Ok(config)
    }

    pub async fn load(&self) -> Result<Config> {
        let key = self.key.ok_or_else(|| anyhow::anyhow!("Vault is locked"))?;

        if !self.file_path.exists() {
            return Err(anyhow::anyhow!("Config file not found"));
        }

        let data = fs::read(&self.file_path).await?;
        let wrapper: ConfigWrapper = serde_json::from_slice(&data)?;

        let decrypted_json =
            rvault_core::crypto::decrypt_with_key(&key, &wrapper.ciphertext, &wrapper.nonce)
                .map_err(|e| anyhow::anyhow!("Decryption failed: {}", e))?;

        let config: Config = serde_json::from_str(&decrypted_json)?;
        Ok(config)
    }

    /// Synchronous version of load for use in sync contexts
    pub fn load_sync(&self) -> Result<Config> {
        let key = self.key.ok_or_else(|| anyhow::anyhow!("Vault is locked"))?;

        if !self.file_path.exists() {
            return Err(anyhow::anyhow!("Config file not found"));
        }

        let data = std::fs::read(&self.file_path)?;
        let wrapper: ConfigWrapper = serde_json::from_slice(&data)?;

        let decrypted_json =
            rvault_core::crypto::decrypt_with_key(&key, &wrapper.ciphertext, &wrapper.nonce)
                .map_err(|e| anyhow::anyhow!("Decryption failed: {}", e))?;

        let config: Config = serde_json::from_str(&decrypted_json)?;
        Ok(config)
    }

    pub async fn save(&self, config: &Config) -> Result<()> {
        let key = self.key.ok_or_else(|| anyhow::anyhow!("Vault is locked"))?;
        Self::save_internal(config, &key, &self.file_path).await
    }

    // Internal static save to avoid borrowing issues or for use in init
    async fn save_internal(config: &Config, key: &[u8], path: &PathBuf) -> Result<()> {
        let plain_json = serde_json::to_string(config)?;
        let (ciphertext, nonce) = rvault_core::crypto::encrypt_with_key(key, plain_json.as_bytes())
            .map_err(|e| anyhow::anyhow!("Encryption failed: {}", e))?;

        // Ensure master_hash is present
        let master_hash = config
            .system
            .master_hash
            .clone()
            .ok_or_else(|| anyhow::anyhow!("System config missing master_hash"))?;

        let wrapper = ConfigWrapper {
            master_hash,
            ciphertext,
            nonce,
        };

        let file_data = serde_json::to_vec_pretty(&wrapper)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(path, file_data).await?;
        Ok(())
    }
    pub async fn has_token(&self) -> bool {
        if let Some(key) = self.key {
            if let Ok(data) = fs::read(&self.file_path).await {
                if let Ok(wrapper) = serde_json::from_slice::<ConfigWrapper>(&data) {
                    if let Ok(decrypted) = rvault_core::crypto::decrypt_with_key(
                        &key,
                        &wrapper.ciphertext,
                        &wrapper.nonce,
                    ) {
                        if let Ok(config) = serde_json::from_str::<Config>(&decrypted) {
                            return config.system.github_token.is_some();
                        }
                    }
                }
            }
        }
        false
    }

    pub async fn reset(&mut self) -> Result<()> {
        if self.file_path.exists() {
            fs::remove_file(&self.file_path).await?;
        }
        self.key = None;
        let _ = session::end_session();
        Ok(())
    }

    pub fn try_restore_session(&mut self) -> bool {
        if let Ok(key_vec) = session::get_key_from_session() {
            if let Ok(key) = key_vec.try_into() {
                self.key = Some(key);
                return true;
            }
        }
        false
    }
}

#[derive(Serialize, Deserialize)]
struct ConfigWrapper {
    master_hash: String,
    ciphertext: String,
    nonce: String,
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crypto_verification() {
        let password = "test_password";
        let hashed = rvault_core::crypto::hash_data(password.as_bytes()).expect("Hashing failed");
        println!("Hash: {}", hashed.hash);
        assert!(
            rvault_core::crypto::verify_password(password.as_bytes(), &hashed.hash),
            "Verification failed"
        );

        // This step verifies if get_encryption_key works with the password.
        // It will fail if keystore.rvault is missing or password doesn't match the one in keystore.
        // We expect it to fail in CI/clean env, but we want to see the error message.
        match rvault_core::vault::Vault::get_encryption_key(password, &hashed.hash) {
            Ok(_) => println!("get_encryption_key success"),
            Err(e) => println!(
                "get_encryption_key failed as expected (if no keystore): {}",
                e
            ),
        }
    }
}
