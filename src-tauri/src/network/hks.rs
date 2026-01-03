 use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use ed25519_dalek::Verifier;
use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use rand::rngs::OsRng;
use rvault_core::crypto;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::prelude::*;
use x25519_dalek::{PublicKey as X25519PublicKey, StaticSecret};

const TREE_DEPTH: u32 = 12;
const MAX_NODES: usize = (1 << (TREE_DEPTH + 1)) - 1; // 8191 for depth 12
const LEAF_START_IDX: usize = (1 << TREE_DEPTH) - 1; // 4095
const MAX_FRIENDS: usize = 15000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FriendEntry {
    pub name: String,
    pub x25519_pubkey: String,      // Base64
    pub encrypted_leaf_key: String, // Encrypted with Shared Secret
    pub nonce: String,
    pub leaf_index: usize,
}

/// Invitation blob with TTL tracking (2-minute lifetime)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackedInvite {
    /// Salt for Argon2 key derivation (Base64)
    pub salt: String,
    /// XChaCha20 nonce (Base64)
    pub nonce: String,
    /// Encrypted payload (Base64)
    pub ciphertext: String,
    /// Unix timestamp when invite was created
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HksTree {
    // We persist the raw keys for all nodes.
    // Index 0 is root.
    // Index i children: 2*i + 1, 2*i + 2.
    // Index i parent: (i-1) / 2.
    pub nodes: Vec<[u8; 32]>,
    pub roster: HashMap<String, FriendEntry>,
    pub next_friend_idx: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishedBlob {
    pub payload: String, // Encrypted IP info
    pub payload_nonce: String,
    // Up-Links: Map of NodeIndex -> (Nonce, Ciphertext of ParentKey encrypted by NodeKey)
    pub tree_links: HashMap<usize, (String, String)>,
    pub roster: HashMap<String, FriendEntry>,
    pub signature: String, // Signed by Ed25519
    pub sender_x25519_pubkey: String,
    /// Encrypted invitations with 2-minute TTL
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub invitations: Vec<TrackedInvite>,
}

impl HksTree {
    pub fn new() -> Self {
        let mut nodes = Vec::with_capacity(MAX_NODES);
        // Initialize all nodes with random keys
        for _ in 0..MAX_NODES {
            nodes.push(rvault_core::crypto::generate_raw_key());
        }

        Self {
            nodes,
            roster: HashMap::new(),
            next_friend_idx: 0,
        }
    }

    pub fn restore(nodes: Vec<[u8; 32]>, roster: HashMap<String, FriendEntry>) -> Self {
        let next_friend_idx = roster.len(); // Approximate, assumes no deletions/gaps for MVP
        Self {
            nodes,
            roster,
            next_friend_idx,
        }
    }

    pub fn root_key(&self) -> &[u8; 32] {
        &self.nodes[0]
    }

    /// Generate a fresh Identity (Ed25519) and Encryption Key (X25519)
    pub fn generate_identity() -> (SigningKey, StaticSecret) {
        let mut csprng = OsRng;
        let mut bytes = [0u8; 32];
        use rand::RngCore;
        csprng.fill_bytes(&mut bytes);
        let signing_key = SigningKey::from_bytes(&bytes);
        let encryption_key = StaticSecret::random_from_rng(OsRng);
        (signing_key, encryption_key)
    }

    /// Add a friend to the roster
    pub fn add_friend(
        &mut self,
        name: &str,
        friend_pubkey_b64: &str,
        my_secret: &StaticSecret,
    ) -> Result<()> {
        if self.next_friend_idx >= MAX_FRIENDS {
            return Err(anyhow!("Friend limit reached (15000)"));
        }

        // 1. Assign Leaf
        // 4 friends per leaf.
        let leaf_offset = self.next_friend_idx / 4;
        let leaf_index = LEAF_START_IDX + leaf_offset;

        if leaf_index >= self.nodes.len() {
            return Err(anyhow!("Tree capacity exceeded"));
        }

        let leaf_key = self.nodes[leaf_index];

        // 2. Encrypt Leaf Key for Friend
        let friend_pubkey_bytes = BASE64.decode(friend_pubkey_b64)?;
        let friend_pubkey_array: [u8; 32] = friend_pubkey_bytes
            .try_into()
            .map_err(|_| anyhow!("Invalid public key length"))?;
        let friend_public = X25519PublicKey::from(friend_pubkey_array);

        let shared_secret = my_secret.diffie_hellman(&friend_public);
        let shared_secret_bytes = shared_secret.to_bytes();

        let leaf_key_b64 = BASE64.encode(leaf_key);
        let (ciphertext, nonce) =
            crypto::encrypt_with_key(&shared_secret_bytes, leaf_key_b64.as_bytes())
                .map_err(|e| anyhow!("Encryption failed: {}", e))?;

        let entry = FriendEntry {
            name: name.to_string(),
            x25519_pubkey: friend_pubkey_b64.to_string(),
            encrypted_leaf_key: ciphertext,
            nonce,
            leaf_index,
        };

        self.roster.insert(friend_pubkey_b64.to_string(), entry);
        self.next_friend_idx += 1;
        Ok(())
    }

    /// Export the tree and payload
    pub fn export(
        &self,
        payload_data: &str,
        signing_key: &SigningKey,
        encryption_pubkey: &X25519PublicKey,
    ) -> Result<String> {
        // 1. Encrypt Payload with Root Key
        let root_key = self.root_key();
        let (payload_cipher, payload_nonce) =
            crypto::encrypt_with_key(root_key, payload_data.as_bytes())
                .map_err(|e| anyhow!("Payload encryption failed: {}", e))?;

        // 2. Build Tree Links (Up-Links)
        // Child Encrypts Parent.
        // We only need links for nodes that are part of active paths.
        // For MVP/Robustness, let's export ALL links?
        // 8192 links.
        let mut tree_links = HashMap::new();
        // Skip Root (Index 0). Start from 1.
        for i in 1..self.nodes.len() {
            let parent_idx = (i - 1) / 2;
            let child_key = &self.nodes[i];
            let parent_key = &self.nodes[parent_idx];

            let parent_key_b64 = BASE64.encode(parent_key);
            if let Ok((cipher, nonce)) =
                crypto::encrypt_with_key(child_key, parent_key_b64.as_bytes())
            {
                tree_links.insert(i, (nonce, cipher)); // Store as (Nonce, Ciphertext) per struct comment?
                                                       // My struct comment said: "Up-Links: Map of NodeIndex -> (Nonce, Ciphertext ...)"
                                                       // So I need to verify what tree_links expects.
                                                       // Struct definition: pub tree_links: HashMap<usize, (String, String)>,
                                                       // Let's stick to (nonce, cipher) order in the map for consistency with struct comment?
                                                       // No, wait. encrypt_with_key logic I just fixed returns (cipher, nonce).
                                                       // So "cipher" is the first element, "nonce" is second.
                                                       // If struct expects (nonce, cipher), I need to construct tuple carefully.
                                                       // Struct: tree_links: HashMap<usize, (String, String)>
                                                       // Let's check struct usage in import.
                                                       // import says: let (nonce, cipher) = blob.tree_links.get(...)
                                                       // So import expects key (tuple.0) to be nonce, and value (tuple.1) to be cipher.
                                                       // So I must insert (nonce, cipher).
                                                       // My encrypt returns (cipher, nonce).
                                                       // So: tree_links.insert(i, (nonce, cipher)); Is correct if (cipher, nonce) is the output of encrypt.
            }
        }

        // 3. Create Blob
        let blob = PublishedBlob {
            payload: payload_cipher,
            payload_nonce,
            tree_links,
            roster: self.roster.clone(),
            signature: String::new(),
            sender_x25519_pubkey: BASE64.encode(encryption_pubkey.as_bytes()),
            invitations: vec![],
        };

        // 4. Serialize & Sign
        let json = serde_json::to_string(&blob)?;
        let signature = signing_key.sign(json.as_bytes());
        let mut final_blob = blob;
        final_blob.signature = BASE64.encode(signature.to_bytes());

        let final_json = serde_json::to_string(&final_blob)?;

        // 5. Compress & Encode
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(final_json.as_bytes())?;
        let compressed = encoder.finish()?;
        Ok(BASE64.encode(compressed))
    }

    /// Import a blob
    pub fn import(
        blob_b64: &str,
        my_pubkey_b64: &str,
        my_secret: &StaticSecret,
        friend_identity_pubkey: &VerifyingKey,
    ) -> Result<String> {
        // Decode & Decompress
        let compressed = BASE64.decode(blob_b64)?;
        let mut decoder = ZlibDecoder::new(&compressed[..]);
        let mut json = String::new();
        decoder.read_to_string(&mut json)?;

        let blob: PublishedBlob = serde_json::from_str(&json)?;

        // Verify Signature
        let mut unsigned_blob = blob.clone();
        unsigned_blob.signature = String::new();
        let unsigned_json = serde_json::to_string(&unsigned_blob)?;
        let signature_bytes = BASE64.decode(&blob.signature)?;
        let signature = ed25519_dalek::Signature::from_slice(&signature_bytes)?;
        friend_identity_pubkey
            .verify(unsigned_json.as_bytes(), &signature)
            .map_err(|_| anyhow!("Invalid signature"))?;

        // Find my entry
        let entry = blob
            .roster
            .get(my_pubkey_b64)
            .ok_or_else(|| anyhow!("Not in roster"))?;

        // Decrypt Leaf Key
        let sender_pubkey_bytes = BASE64.decode(&blob.sender_x25519_pubkey)?;
        let sender_public =
            X25519PublicKey::from(<[u8; 32]>::try_from(sender_pubkey_bytes).unwrap());
        let shared_secret = my_secret.diffie_hellman(&sender_public);

        let leaf_key_json = crypto::decrypt_with_key(
            &shared_secret.to_bytes(),
            &entry.encrypted_leaf_key,
            &entry.nonce,
        )
        .map_err(|e| anyhow!("Decrypt leaf failed: {}", e))?;
        let mut current_key = BASE64.decode(leaf_key_json)?;

        // Traverse Up: Leaf -> Root
        let mut current_idx = entry.leaf_index;
        while current_idx > 0 {
            let (nonce, cipher) = blob
                .tree_links
                .get(&current_idx)
                .ok_or_else(|| anyhow!("Broken link at {}", current_idx))?;

            let parent_key_json = crypto::decrypt_with_key(&current_key, cipher, nonce)
                .map_err(|e| anyhow!("Decrypt link {} failed: {}", current_idx, e))?;

            current_key = BASE64.decode(parent_key_json)?;
            current_idx = (current_idx - 1) / 2;
        }

        // Decrypt Payload with Root Key (current_key)
        let payload = crypto::decrypt_with_key(&current_key, &blob.payload, &blob.payload_nonce)
            .map_err(|e| anyhow!("Payload decrypt failed: {}", e))?;

        Ok(payload)
    }
}
