use crate::{Connection, Error, Result};
use sha2::{Digest, Sha256};
use zova::{ObjectChunkId, ObjectId, ObjectManifestChunk};

fn digest(hash: &str) -> Result<[u8; 32]> {
    let mut bytes = [0; 32];
    hex::decode_to_slice(hash, &mut bytes)
        .map_err(|_| Error::InvalidValue("invalid SHA-256 identifier".into()))?;
    Ok(bytes)
}

impl Connection {
    pub fn has_chunk(&self, hash: &str) -> Result<bool> {
        Ok(self
            .db
            .has_object_chunk(ObjectChunkId::from(digest(hash)?))?)
    }
    pub fn put_chunk(&self, hash: &str, bytes: &[u8]) -> Result<()> {
        Ok(self
            .db
            .put_object_chunk(ObjectChunkId::from(digest(hash)?), bytes)?)
    }

    pub fn get_chunk(&self, hash: &str) -> Result<Vec<u8>> {
        let expected = digest(hash)?;
        let bytes = self.db.get_object_chunk(ObjectChunkId::from(expected))?;
        if Sha256::digest(&bytes).as_slice() != expected {
            return Err(Error::InvalidValue("stored chunk hash mismatch".into()));
        }
        Ok(bytes)
    }

    pub fn has_object(&self, hash: &str) -> Result<bool> {
        Ok(self.db.has_object(ObjectId::from(digest(hash)?))?)
    }

    /// Verify each transport chunk and the full file before making an object visible.
    /// Chunk bytes are read one at a time, preserving RChat's wire manifest.
    pub fn assemble_object(&self, hash: &str, chunks: &[(String, u64)]) -> Result<()> {
        let expected = digest(hash)?;
        let mut hasher = Sha256::new();
        let mut offset = 0u64;
        let mut manifest = Vec::with_capacity(chunks.len());
        for (index, (chunk_hash, size)) in chunks.iter().enumerate() {
            let bytes = self.get_chunk(chunk_hash)?;
            if bytes.len() as u64 != *size {
                return Err(Error::InvalidValue("chunk size mismatch".into()));
            }
            hasher.update(&bytes);
            manifest.push(ObjectManifestChunk {
                index: index as u64,
                hash: ObjectChunkId::from(digest(chunk_hash)?),
                offset,
                size_bytes: *size,
            });
            offset = offset
                .checked_add(*size)
                .ok_or_else(|| Error::InvalidValue("object size overflow".into()))?;
        }
        if hasher.finalize().as_slice() != expected {
            return Err(Error::InvalidValue("assembled file hash mismatch".into()));
        }
        if self.db.has_object(ObjectId::from(expected))? {
            return self.verify_object(hash);
        }
        Ok(self
            .db
            .assemble_object_from_chunks(ObjectId::from(expected), offset, &manifest)?)
    }

    pub fn verify_object(&self, hash: &str) -> Result<()> {
        let expected = digest(hash)?;
        let id = ObjectId::from(expected);
        let size = self.db.object_size(id)?;
        let mut offset = 0;
        let mut buffer = [0; 64 * 1024];
        let mut hasher = Sha256::new();
        while offset < size {
            let read = self.db.read_object_range(id, offset, &mut buffer)?;
            if read == 0 {
                return Err(Error::InvalidValue("truncated object".into()));
            }
            hasher.update(&buffer[..read]);
            offset += read as u64;
        }
        if offset != size || hasher.finalize().as_slice() != expected {
            return Err(Error::InvalidValue("stored object hash mismatch".into()));
        }
        Ok(())
    }

    pub fn get_object(&self, hash: &str) -> Result<Vec<u8>> {
        let expected = digest(hash)?;
        let bytes = self.db.get_object(ObjectId::from(expected))?;
        if Sha256::digest(&bytes).as_slice() != expected {
            return Err(Error::InvalidValue("stored object hash mismatch".into()));
        }
        Ok(bytes)
    }
}
