use std::path::PathBuf;
use std::{fs, path::Path};
use crate::crypto::{decrypt_with_key, encrypt_with_key, generate_key};
use argon2::{Algorithm, Params, Version};
use argon2::Argon2;
use base64::prelude::*;
use base64::engine::general_purpose::STANDARD as Base64;
use directories::ProjectDirs;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use chacha20poly1305::Key;
use zeroize::Zeroize;

const MAGIC: &str = "RVAULT";
const AAD: &[u8] = b"rvault-keystore-v1";
const KEYSTORE_NAME: &str = "keystore.rvault"; // file name
const EK_LEN: usize = 32;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;

#[derive(Serialize, Deserialize)]
struct KdfParams {
    t: u32,       // iterations
    m: u32,       // memory (KiB)
    p: u32,       // parallelism
}

#[derive(Serialize, Deserialize)]
struct KeystoreFile {
    magic: String,
    version: u32,
    kdf: String,           // "argon2id"
    kdf_params: KdfParams, // t,m,p
    salt_b64: String,
    nonce_b64: String,
    wrapped_ek_b64: String,
}

pub fn keystore_path() -> Result<PathBuf, String> {
    if let Some(pd) = ProjectDirs::from("io.github", "ata-sesli", "RVault") {
        let dir = pd.config_dir();
        fs::create_dir_all(dir).map_err(|e| format!("mkdir: {e}"))?;
        Ok(dir.join(KEYSTORE_NAME))
    } else {
        Err("Could not find project directories".to_string())
    }
}

fn derive_kek(
    master_password: &[u8],
    salt: &[u8],
    k: &KdfParams,
) -> Result<Key, String> {
    // Output length = 32 bytes (for ChaCha20Poly1305 key)
    let params = Params::new(k.m, k.t, k.p, Some(EK_LEN))
        .map_err(|e| format!("Argon2 params: {e}"))?;
    let a2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);

    let mut out = [0u8; EK_LEN];
    a2.hash_password_into(master_password, salt, &mut out)
        .map_err(|e| format!("Argon2 derive: {e}"))?;
    let key = Key::from_slice(&out).to_owned();
    out.zeroize();
    Ok(key)
}
/// Creates a new, encrypted vault file containing a newly generated Master Encryption Key (MEK).
pub fn create_key_vault(master_password: &str, path: &Path) -> Result<(), String> {
    // 1. Generate a new, random 32-byte Master Encryption Key (MEK). This is the key we will protect.
    let mek = generate_key();
    // 2) Derive KEK from master + raw 16-byte salt
    let mut salt = [0u8; SALT_LEN];
    rand::rng().fill_bytes(&mut salt);

    let mut kek = [0u8; EK_LEN];
    let argon2 = Argon2::default();
    let _ = argon2.hash_password_into(master_password.as_bytes(), &salt, &mut kek)
        .map_err(|_| "Failed to derive key.".to_string())?;

    // 3. Encrypt the MEK using the KEK.
    let (ciphertext_b64, nonce_b64) = encrypt_with_key(&kek, &mek.as_bytes())?;

    // 4) Write raw: [salt][nonce][ct]
    let nonce = Base64.decode(&nonce_b64).map_err(|e| e.to_string())?;
    if nonce.len() != NONCE_LEN { return Err("Unexpected nonce length".into()); }
    let ct = Base64.decode(&ciphertext_b64).map_err(|e| e.to_string())?;

    let mut out = Vec::with_capacity(SALT_LEN + NONCE_LEN + ct.len());
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);

    // ensure parent exists (avoids ENOENT on first run)
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
    }

    // actually write the keystore
    std::fs::write(path, &out).map_err(|e| format!("write keystore: {e}"))
}

/// Loads and decrypts the Master Encryption Key (MEK) from the vault file.
pub fn load_key_from_vault(master_password: &str, path: &Path) -> Result<[u8; EK_LEN], String> {
    let file_bytes = fs::read(path).map_err(|e| format!("Failed to read vault file: {}", e))?;

    // 1. Parse the file: [32-byte salt][12-byte nonce][encrypted MEK]
    if file_bytes.len() < SALT_LEN + NONCE_LEN {
        return Err("Invalid or corrupt vault file.".to_string());
    }
    let salt = &file_bytes[0..SALT_LEN];
    let nonce_b64 = Base64.encode(&file_bytes[SALT_LEN..SALT_LEN + NONCE_LEN]);
    let encrypted_mek_b64 = Base64.encode(&file_bytes[SALT_LEN + NONCE_LEN..]);

    // 2. Re-derive the Key Encryption Key (KEK) from the password and salt.
    let mut kek = [0u8; EK_LEN];
    let argon2 = Argon2::default();
    argon2.hash_password_into(master_password.as_bytes(), salt, &mut kek)
        .map_err(|_| "Failed to derive key.".to_string())?;

    // 3. Decrypt the MEK using the KEK.
    let mek_json = decrypt_with_key(&kek, &encrypted_mek_b64, &nonce_b64)?;
    let mek_bytes = Base64.decode(mek_json).map_err(|e| e.to_string())?;

    mek_bytes.try_into().map_err(|_| "Decrypted key has incorrect length.".to_string())
}