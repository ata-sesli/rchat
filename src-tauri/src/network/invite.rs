//! Invitation System for RChat
//!
//! Secure, Gist-based invitation system using:
//! - Interleaved Harvester logic (14-char → 18-char deterministic key)
//! - Argon2 KDF via `rvault_core::crypto::derive_key`
//! - XChaCha20-Poly1305 encryption via `rvault_core::crypto`

use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use rvault_core::crypto;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, ZeroizeOnDrop};

// ============================================================================
// Data Structures
// ============================================================================

/// Public Gist Entry - Safe to store publicly (no IDs, no names, no hashes)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedInvite {
    /// Base64: 16-byte Argon2 salt
    pub salt: String,
    /// Base64: XChaCha20 nonce (from encrypt_with_key)
    pub nonce: String,
    /// Base64: Encrypted payload + Poly1305 tag
    pub ciphertext: String,
}

/// Secret Payload - Must be wiped from RAM when out of scope
#[derive(Debug, Clone, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct InvitePayload {
    /// Critical: Prevents "Wrong User" attacks
    pub target_username: String,
    /// The secret data (IP address or multiaddr)
    pub ip_address: String,
    /// Unix timestamp for expiration check
    pub ttl_timestamp: u64,
}

// ============================================================================
// Interleaved Harvester Logic
// ============================================================================

/// Generates an 18-character key from a 14-character passphrase and usernames.
///
/// # Algorithm
/// 1. Normalize usernames (trim, lowercase)
/// 2. Generate pool: SHA-256(inviter + invitee) → 64-char hex string
/// 3. Harvest 4 chars by summing chunks of the password and indexing into pool
/// 4. Interleave: password chars + harvested chars = 18-char output
///
/// # Arguments
/// * `password` - Exactly 14 characters
/// * `inviter` - Username of the person sending the invite
/// * `invitee` - Username of the person receiving the invite
///
/// # Returns
/// An 18-character secret string
pub fn harvest_key(password: &str, inviter: &str, invitee: &str) -> Result<HarvestedKey> {
    // 1. Input Validation
    if password.len() != 14 {
        return Err(anyhow!(
            "Password must be exactly 14 characters, got {}",
            password.len()
        ));
    }

    // 2. Normalization
    let raw_seed = format!(
        "{}{}",
        inviter.trim().to_lowercase(),
        invitee.trim().to_lowercase()
    );

    // 3. Pool Generation: SHA-256 → Hex String (64 chars)
    let mut hasher = Sha256::new();
    hasher.update(raw_seed.as_bytes());
    let hash_result = hasher.finalize();
    let pool = hex::encode(hash_result); // 64-char hex string

    // 4. The Harvest
    let password_bytes = password.as_bytes();

    // Helper: Sum chunk bytes, index into pool
    let get_harvest_char = |chunk: &[u8]| -> char {
        let sum: u32 = chunk.iter().map(|&b| b as u32).sum();
        let index = (sum % 64) as usize;
        pool.chars().nth(index).unwrap_or('0')
    };

    // Split password into 4 chunks (14 chars: 4+3+4+3)
    let chunks = [
        &password_bytes[0..4],   // 4 chars
        &password_bytes[4..7],   // 3 chars
        &password_bytes[7..11],  // 4 chars
        &password_bytes[11..14], // 3 chars
    ];

    let h1 = get_harvest_char(chunks[0]);
    let h2 = get_harvest_char(chunks[1]);
    let h3 = get_harvest_char(chunks[2]);
    let h4 = get_harvest_char(chunks[3]);

    // 5. Interleaving: p1(4) + h1 + p2(3) + h2 + p3(4) + h3 + p4(3) + h4 = 18 chars
    let result = format!(
        "{}{}{}{}{}{}{}{}",
        &password[0..4],
        h1,
        &password[4..7],
        h2,
        &password[7..11],
        h3,
        &password[11..14],
        h4
    );

    Ok(HarvestedKey(result))
}

/// A zeroize-on-drop wrapper for the harvested key
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct HarvestedKey(String);

impl HarvestedKey {
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    #[allow(dead_code)]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

// ============================================================================
// Cryptography Layer (rvault_core)
// ============================================================================

/// Encrypts an InvitePayload using Argon2 + XChaCha20-Poly1305.
///
/// # Returns
/// An `EncryptedInvite` containing salt, nonce, and ciphertext (all Base64).
pub fn encrypt_invite(
    payload: &InvitePayload,
    harvested_key: &HarvestedKey,
) -> Result<EncryptedInvite> {
    use rand::RngCore;

    // 1. Generate random salt (16 bytes) for Argon2
    let mut salt = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut salt);

    // 2. Derive 32-byte key using Argon2 (via rvault_core)
    let key = crypto::derive_key(harvested_key.as_bytes(), &salt)
        .map_err(|e| anyhow!("Key derivation failed: {}", e))?;

    // 3. Serialize payload to JSON
    let payload_json = serde_json::to_string(payload)?;

    // 4. Encrypt using rvault_core (XChaCha20-Poly1305)
    // Returns (ciphertext_b64, nonce_b64)
    let (ciphertext_b64, nonce_b64) = crypto::encrypt_with_key(&key, payload_json.as_bytes())
        .map_err(|e| anyhow!("Encryption failed: {}", e))?;

    Ok(EncryptedInvite {
        salt: BASE64.encode(salt),
        nonce: nonce_b64,
        ciphertext: ciphertext_b64,
    })
}

/// Attempts to decrypt an EncryptedInvite using the derived key.
///
/// # Returns
/// - `Ok(Some(payload))` if decryption and validation succeed
/// - `Ok(None)` if decryption fails (wrong key)
/// - `Err(...)` for other errors
pub fn decrypt_invite(
    invite: &EncryptedInvite,
    harvested_key: &HarvestedKey,
) -> Result<Option<InvitePayload>> {
    // 1. Decode salt
    let salt_bytes = BASE64
        .decode(&invite.salt)
        .map_err(|e| anyhow!("Invalid salt: {}", e))?;

    let salt: [u8; 16] = salt_bytes
        .try_into()
        .map_err(|_| anyhow!("Salt must be 16 bytes"))?;

    // 2. Derive key using Argon2
    let key = crypto::derive_key(harvested_key.as_bytes(), &salt)
        .map_err(|e| anyhow!("Key derivation failed: {}", e))?;

    // 3. Attempt decryption
    match crypto::decrypt_with_key(&key, &invite.ciphertext, &invite.nonce) {
        Ok(plaintext_json) => {
            // 4. Parse payload
            let payload: InvitePayload = serde_json::from_str(&plaintext_json)?;
            Ok(Some(payload))
        }
        Err(_) => {
            // Decryption failed - wrong key (this is expected during scanning)
            Ok(None)
        }
    }
}

// ============================================================================
// Storage Protocol (Write Path)
// ============================================================================

/// Generates a complete invite ready for Gist upload.
///
/// # Arguments
/// * `password` - 14-char passphrase (shared out-of-band)
/// * `inviter` - Sender's username
/// * `invitee` - Receiver's username
/// * `ip_address` - The secret data to share
/// * `ttl_secs` - How long the invite is valid (in seconds from now)
pub fn generate_invite(
    password: &str,
    inviter: &str,
    invitee: &str,
    ip_address: &str,
    ttl_secs: u64,
) -> Result<EncryptedInvite> {
    // 1. Generate Harvester Key
    let harvested_key = harvest_key(password, inviter, invitee)?;

    // 2. Create Payload
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let payload = InvitePayload {
        target_username: invitee.trim().to_lowercase(),
        ip_address: ip_address.to_string(),
        ttl_timestamp: now + ttl_secs,
    };

    // 3. Encrypt
    encrypt_invite(&payload, &harvested_key)
}

// ============================================================================
// Discovery Protocol (Read Path)
// ============================================================================

/// Scans a list of invites and attempts to find one meant for the receiver.
///
/// # Arguments
/// * `invites` - List of encrypted invites from the Gist
/// * `password` - 14-char passphrase
/// * `inviter` - Sender's username
/// * `my_username` - Receiver's own username
///
/// # Returns
/// - `Ok(Some((payload, index)))` if a valid invite is found
/// - `Ok(None)` if no matching invite exists
pub fn process_invites(
    invites: &[EncryptedInvite],
    password: &str,
    inviter: &str,
    my_username: &str,
) -> Result<Option<(InvitePayload, usize)>> {
    // 1. Generate Harvester Key
    let harvested_key = harvest_key(password, inviter, my_username)?;

    let my_username_normalized = my_username.trim().to_lowercase();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // 2. Scan all invites
    for (index, invite) in invites.iter().enumerate() {
        match decrypt_invite(invite, &harvested_key)? {
            Some(payload) => {
                // 3. Validate: target_username matches and not expired
                if payload.target_username == my_username_normalized
                    && payload.ttl_timestamp > now
                {
                    return Ok(Some((payload, index)));
                }
                // Wrong target or expired - continue scanning
            }
            None => {
                // Decryption failed - wrong key, continue
            }
        }
    }

    Ok(None)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_harvest_key_determinism() {
        let key1 = harvest_key("12345678901234", "Alice", "Bob").unwrap();
        let key2 = harvest_key("12345678901234", "Alice", "Bob").unwrap();
        assert_eq!(key1.as_str(), key2.as_str());
    }

    #[test]
    fn test_harvest_key_length() {
        let key = harvest_key("12345678901234", "Alice", "Bob").unwrap();
        assert_eq!(key.as_str().len(), 18);
    }

    #[test]
    fn test_harvest_key_different_users() {
        let key1 = harvest_key("12345678901234", "Alice", "Bob").unwrap();
        let key2 = harvest_key("12345678901234", "Charlie", "Bob").unwrap();
        assert_ne!(key1.as_str(), key2.as_str());
    }

    #[test]
    fn test_harvest_key_case_insensitive() {
        let key1 = harvest_key("12345678901234", "Alice", "Bob").unwrap();
        let key2 = harvest_key("12345678901234", "ALICE", "BOB").unwrap();
        assert_eq!(key1.as_str(), key2.as_str());
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let password = "12345678901234";
        let inviter = "Alice";
        let invitee = "Bob";

        let invite = generate_invite(password, inviter, invitee, "192.168.1.100", 3600).unwrap();

        let result = process_invites(&[invite], password, inviter, invitee).unwrap();

        assert!(result.is_some());
        let (payload, _) = result.unwrap();
        assert_eq!(payload.ip_address, "192.168.1.100");
    }

    #[test]
    fn test_wrong_password_fails() {
        let invite =
            generate_invite("12345678901234", "Alice", "Bob", "192.168.1.100", 3600).unwrap();

        let result = process_invites(&[invite], "wrongpassword1", "Alice", "Bob").unwrap();

        // Wrong password means key derivation produces different key, decrypt fails
        assert!(result.is_none());
    }

    #[test]
    fn test_wrong_username_fails() {
        let invite =
            generate_invite("12345678901234", "Alice", "Bob", "192.168.1.100", 3600).unwrap();

        let result = process_invites(&[invite], "12345678901234", "Alice", "Charlie").unwrap();

        // Should return None (no matching invite found)
        assert!(result.is_none());
    }
}
