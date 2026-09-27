use clap::ValueEnum;
use rand::seq::{IndexedRandom, SliceRandom};
use chacha20poly1305::{aead::{Aead,AeadCore,KeyInit,OsRng},ChaCha20Poly1305, Nonce};
use argon2::{password_hash::{self, PasswordHash, PasswordHasher, SaltString, PasswordVerifier}, Argon2};
use base64::prelude::*;
use base64::engine::general_purpose::STANDARD as Base64;
#[derive(Debug,Clone,ValueEnum)]
// Multiple encryption methods in future implementations
pub enum Encryption {
    Raw,

}
#[derive(Debug,Clone,ValueEnum)]
// Multiple hashing methods in future implementations
pub enum Hash {
    Raw,
}

pub struct EncryptedData{
    pub key: String,
    pub nonce: String,
    pub ciphertext: String
}
pub struct DerivedEncryptedData {
    pub ciphertext: String,
    pub nonce: String,
    pub salt: String,
}
pub struct HashedData{
    pub hash: String
}
pub fn generate_key() -> String{
    let ek = ChaCha20Poly1305::generate_key(&mut OsRng); // A 32-byte random key
    let ek_b64 = Base64.encode(&ek);
    ek_b64
}
pub fn generate_raw_key() -> [u8; 32] {
    let key = ChaCha20Poly1305::generate_key(&mut OsRng);
    key.into()
}

// &[u8] means binary sequence.
pub fn hash_data(data:&[u8]) -> Result<HashedData,argon2::password_hash::Error>{
    let salt = SaltString::generate(&mut argon2::password_hash::rand_core::OsRng);
    let argon2 = Argon2::default();
    let hash = argon2.hash_password(data, &salt)?.to_string();
    let hashed_data = HashedData{
        hash
    };
    Ok(hashed_data)
}

/// Derive a raw 32-byte encryption key from a password and salt using Argon2
/// Unlike `hash_data`, this returns the raw output bytes suitable for encryption keys
pub fn derive_key(password: &[u8], salt: &[u8]) -> Result<[u8; 32], argon2::Error> {
    let argon2 = Argon2::default();
    let mut output_key = [0u8; 32];
    argon2.hash_password_into(password, salt, &mut output_key)?;
    Ok(output_key)
}

pub fn encrypt_data(data:&[u8]) -> Result<EncryptedData,chacha20poly1305::Error> {
    let key = ChaCha20Poly1305::generate_key(&mut OsRng);
    let cipher = ChaCha20Poly1305::new(&key);
    let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
    let ciphertext_bytes = cipher.encrypt(&nonce, data)?;
    let encrypted_data = EncryptedData{
        key: Base64.encode(&key),
        nonce: Base64.encode(&nonce),
        ciphertext: Base64.encode(&ciphertext_bytes)
    };
    Ok(encrypted_data)
}
pub fn generate_password(length: u8,special_characters: bool) -> String{
    let lowercase = b"abcdefghijklmnopqrstuvwxyz";
    let uppercase = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let numbers = b"0123456789";
    let symbols = b"!@#$%^&*()_+-=[]{}|;:'\",.<>/?";

    let mut rng = rand::rng();
    let mut password_chars = Vec::with_capacity(length as usize);
    let mut master_pool = Vec::new();

    master_pool.extend_from_slice(lowercase);
    password_chars.push(*lowercase.choose(&mut rng).unwrap() as char);

    master_pool.extend_from_slice(uppercase);
    password_chars.push(*uppercase.choose(&mut rng).unwrap() as char);

    master_pool.extend_from_slice(numbers);
    password_chars.push(*numbers.choose(&mut rng).unwrap() as char);

    if special_characters {
        master_pool.extend_from_slice(symbols);
        password_chars.push(*symbols.choose(&mut rng).unwrap() as char);
    }

    let remaining_len = length as usize - password_chars.len();
    for _ in 0..remaining_len {
        password_chars.push(*master_pool.choose(&mut rng).unwrap() as char);
    }
    password_chars.shuffle(&mut rng);
    password_chars.into_iter().collect::<String>()
}
pub fn encrypt_with_key(key: &[u8], data: &[u8]) -> Result<(String, String), String> {
    let cipher = ChaCha20Poly1305::new_from_slice(key).map_err(|e| e.to_string())?;
    let nonce = ChaCha20Poly1305::generate_nonce(&mut chacha20poly1305::aead::OsRng);
    let ciphertext = cipher.encrypt(&nonce, data).map_err(|e| e.to_string())?;

    Ok((
        Base64.encode(&ciphertext),
        Base64.encode(&nonce),
    ))
}
pub fn decrypt_with_key(key: &[u8], ciphertext_b64: &str, nonce_b64: &str) -> Result<String, String> {
    let ciphertext = Base64.decode(ciphertext_b64).map_err(|e| e.to_string())?;
    let nonce_bytes = Base64.decode(nonce_b64).map_err(|e| e.to_string())?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    let cipher = ChaCha20Poly1305::new_from_slice(key).map_err(|e| e.to_string())?;
    let plaintext = cipher.decrypt(nonce, ciphertext.as_ref()).map_err(|e| e.to_string())?;
    String::from_utf8(plaintext).map_err(|e| e.to_string())
}
pub fn verify_password(password: &[u8], stored_hash: &str) -> bool {
    // Attempt to parse the stored hash string
    if let Ok(parsed_hash) = PasswordHash::new(stored_hash) {
        // Verify the plaintext password against the parsed hash
        Argon2::default()
            .verify_password(password, &parsed_hash)
            .is_ok() // Returns true if verification succeeds, false otherwise
    } else {
        // If the stored hash is invalid, verification fails
        false
    }
}
