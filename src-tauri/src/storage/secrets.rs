//! Secrets live in the OS credential store (macOS Keychain, Windows
//! Credential Manager), never in SQLite or config files.
//!
//! Stored items:
//! - integration credentials (Notion token, Linear API key)
//! - the AES-256 key that encrypts voice embeddings at rest

use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use keyring::{Entry, Error as KeyringError};

pub const SERVICE: &str = "app.minutes.desktop";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretKey {
    NotionToken,
    LinearApiKey,
    VoiceprintKey,
}

impl SecretKey {
    fn account(self) -> &'static str {
        match self {
            SecretKey::NotionToken => "notion-token",
            SecretKey::LinearApiKey => "linear-api-key",
            SecretKey::VoiceprintKey => "voiceprint-encryption-key",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error("the system credential store is unavailable: {0}")]
    Store(String),
    #[error("stored voice profile key is corrupt")]
    CorruptKey,
    #[error("could not decrypt voice profile data")]
    Decrypt,
}

impl From<KeyringError> for SecretError {
    fn from(e: KeyringError) -> Self {
        SecretError::Store(e.to_string())
    }
}

/// Abstraction so tests (and future stores) don't touch the real keychain.
pub trait SecretStore: Send + Sync {
    fn get(&self, key: SecretKey) -> Result<Option<String>, SecretError>;
    fn set(&self, key: SecretKey, value: &str) -> Result<(), SecretError>;
    fn delete(&self, key: SecretKey) -> Result<(), SecretError>;
}

pub struct OsSecretStore;

impl SecretStore for OsSecretStore {
    fn get(&self, key: SecretKey) -> Result<Option<String>, SecretError> {
        match Entry::new(SERVICE, key.account())?.get_password() {
            Ok(s) => Ok(Some(s)),
            Err(KeyringError::NoEntry) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn set(&self, key: SecretKey, value: &str) -> Result<(), SecretError> {
        Ok(Entry::new(SERVICE, key.account())?.set_password(value)?)
    }

    fn delete(&self, key: SecretKey) -> Result<(), SecretError> {
        match Entry::new(SERVICE, key.account())?.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

/// In-memory store for tests and explicit development fixtures.
#[derive(Default)]
pub struct MemorySecretStore(parking_lot::Mutex<std::collections::HashMap<&'static str, String>>);

impl SecretStore for MemorySecretStore {
    fn get(&self, key: SecretKey) -> Result<Option<String>, SecretError> {
        Ok(self.0.lock().get(key.account()).cloned())
    }
    fn set(&self, key: SecretKey, value: &str) -> Result<(), SecretError> {
        self.0.lock().insert(key.account(), value.to_string());
        Ok(())
    }
    fn delete(&self, key: SecretKey) -> Result<(), SecretError> {
        self.0.lock().remove(key.account());
        Ok(())
    }
}

/// Encrypts voice embeddings with a key held in the credential store.
pub struct VoiceprintCipher {
    cipher: Aes256Gcm,
}

pub struct Sealed {
    pub ciphertext: Vec<u8>,
    pub nonce: Vec<u8>,
}

impl VoiceprintCipher {
    /// Load the key, creating it on first use.
    pub fn load_or_create(store: &dyn SecretStore) -> Result<Self, SecretError> {
        let key_bytes = match store.get(SecretKey::VoiceprintKey)? {
            Some(hex_key) => hex::decode(hex_key.trim()).map_err(|_| SecretError::CorruptKey)?,
            None => {
                let key = Aes256Gcm::generate_key(OsRng);
                store.set(SecretKey::VoiceprintKey, &hex::encode(key))?;
                key.to_vec()
            }
        };
        if key_bytes.len() != 32 {
            return Err(SecretError::CorruptKey);
        }
        let key = Key::<Aes256Gcm>::from_slice(&key_bytes);
        Ok(VoiceprintCipher { cipher: Aes256Gcm::new(key) })
    }

    pub fn seal_embedding(&self, embedding: &[f32]) -> Sealed {
        let plaintext: Vec<u8> = embedding.iter().flat_map(|v| v.to_le_bytes()).collect();
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ciphertext = self.cipher.encrypt(&nonce, plaintext.as_slice()).expect("AES-GCM encryption cannot fail");
        Sealed { ciphertext, nonce: nonce.to_vec() }
    }

    pub fn open_embedding(&self, ciphertext: &[u8], nonce: &[u8]) -> Result<Vec<f32>, SecretError> {
        if nonce.len() != 12 {
            return Err(SecretError::Decrypt);
        }
        let plain = self
            .cipher
            .decrypt(Nonce::from_slice(nonce), ciphertext)
            .map_err(|_| SecretError::Decrypt)?;
        if plain.len() % 4 != 0 {
            return Err(SecretError::Decrypt);
        }
        Ok(plain.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedding_round_trip() {
        let store = MemorySecretStore::default();
        let c = VoiceprintCipher::load_or_create(&store).unwrap();
        let emb = vec![0.25f32, -1.5, 3.0];
        let sealed = c.seal_embedding(&emb);
        assert_ne!(sealed.ciphertext, emb.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>());
        // Reloading uses the same persisted key.
        let c2 = VoiceprintCipher::load_or_create(&store).unwrap();
        assert_eq!(c2.open_embedding(&sealed.ciphertext, &sealed.nonce).unwrap(), emb);
    }

    #[test]
    fn tampering_is_detected() {
        let store = MemorySecretStore::default();
        let c = VoiceprintCipher::load_or_create(&store).unwrap();
        let mut sealed = c.seal_embedding(&[1.0, 2.0]);
        sealed.ciphertext[0] ^= 0xff;
        assert!(c.open_embedding(&sealed.ciphertext, &sealed.nonce).is_err());
    }

    #[test]
    fn deleting_key_makes_old_data_unreadable() {
        let store = MemorySecretStore::default();
        let sealed = VoiceprintCipher::load_or_create(&store).unwrap().seal_embedding(&[1.0]);
        store.delete(SecretKey::VoiceprintKey).unwrap();
        let fresh = VoiceprintCipher::load_or_create(&store).unwrap();
        assert!(fresh.open_embedding(&sealed.ciphertext, &sealed.nonce).is_err());
    }
}
