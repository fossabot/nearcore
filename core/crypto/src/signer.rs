use std::path::Path;
use std::sync::Arc;

use crate::bls::{BlsPublicKey, BlsSecretKey, BlsSignature};
use crate::key_file::{BlsKeyFile, KeyFile};
use crate::{KeyType, PublicKey, SecretKey, Signature};

/// Generic signer trait, that can sign with some subset of supported curves.
pub trait Signer: Sync + Send {
    fn public_key(&self) -> PublicKey;
    fn sign(&self, data: &[u8]) -> Signature;

    fn verify(&self, data: &[u8], signature: &Signature) -> bool {
        signature.verify(data, &self.public_key())
    }

    /// Used by test infrastructure, only implement if make sense for testing otherwise raise `unimplemented`.
    fn write_to_file(&self, path: &Path);
}

// Signer that returns empty signature. Used for transaction testing.
pub struct EmptySigner {}

impl Signer for EmptySigner {
    fn public_key(&self) -> PublicKey {
        PublicKey::empty(KeyType::ED25519)
    }

    fn sign(&self, _data: &[u8]) -> Signature {
        Signature::empty(KeyType::ED25519)
    }

    fn write_to_file(&self, _path: &Path) {
        unimplemented!()
    }
}

/// Signer that keeps secret key in memory.
#[derive(Clone)]
pub struct InMemorySigner {
    pub account_id: String,
    pub public_key: PublicKey,
    pub secret_key: SecretKey,
}

impl InMemorySigner {
    pub fn from_seed(account_id: &str, key_type: KeyType, seed: &str) -> Self {
        let secret_key = SecretKey::from_seed(key_type, seed);
        Self { account_id: account_id.to_string(), public_key: secret_key.public_key(), secret_key }
    }

    pub fn from_secret_key(account_id: String, secret_key: SecretKey) -> Self {
        Self { account_id, public_key: secret_key.public_key(), secret_key }
    }

    pub fn from_file(path: &Path) -> Self {
        KeyFile::from_file(path).into()
    }
}

impl Signer for InMemorySigner {
    fn public_key(&self) -> PublicKey {
        self.public_key.clone()
    }

    fn sign(&self, data: &[u8]) -> Signature {
        self.secret_key.sign(data)
    }

    fn write_to_file(&self, path: &Path) {
        KeyFile::from(self).write_to_file(path);
    }
}

impl From<KeyFile> for InMemorySigner {
    fn from(key_file: KeyFile) -> Self {
        Self {
            account_id: key_file.account_id,
            public_key: key_file.public_key,
            secret_key: key_file.secret_key,
        }
    }
}

impl From<&InMemorySigner> for KeyFile {
    fn from(signer: &InMemorySigner) -> KeyFile {
        KeyFile {
            account_id: signer.account_id.clone(),
            public_key: signer.public_key.clone(),
            secret_key: signer.secret_key.clone(),
        }
    }
}

impl From<Arc<InMemorySigner>> for KeyFile {
    fn from(signer: Arc<InMemorySigner>) -> KeyFile {
        KeyFile {
            account_id: signer.account_id.clone(),
            public_key: signer.public_key.clone(),
            secret_key: signer.secret_key.clone(),
        }
    }
}

// Specifically signer for BLS curve.
pub trait BlsSigner: Sync + Send {
    fn public_key(&self) -> BlsPublicKey;
    fn sign(&self, data: &[u8]) -> BlsSignature;

    fn verify(&self, data: &[u8], signature: &BlsSignature) -> bool {
        signature.verify_single(data, &self.public_key())
    }

    /// Used by test infrastructure, only implement if make sense for testing otherwise raise `unimplemented`.
    fn write_to_file(&self, path: &Path);
}

/// Signer that returns empty signature. Used for genesis block and testing.
pub struct EmptyBlsSigner {}

impl BlsSigner for EmptyBlsSigner {
    fn public_key(&self) -> BlsPublicKey {
        BlsPublicKey::empty()
    }

    fn sign(&self, _data: &[u8]) -> BlsSignature {
        BlsSignature::empty()
    }

    fn write_to_file(&self, _path: &Path) {
        unimplemented!()
    }
}

/// Signer that keeps secret key in memory.
#[derive(Clone)]
pub struct InMemoryBlsSigner {
    pub account_id: String,
    pub public_key: BlsPublicKey,
    pub secret_key: BlsSecretKey,
}

impl InMemoryBlsSigner {
    pub fn from_seed(account_id: &str, seed: &str) -> Self {
        let secret_key = BlsSecretKey::from_seed(seed);
        Self { account_id: account_id.to_string(), public_key: secret_key.public_key(), secret_key }
    }

    pub fn from_file(path: &Path) -> Self {
        BlsKeyFile::from_file(path).into()
    }

    pub fn from_secret_key(account_id: String, secret_key: BlsSecretKey) -> Self {
        Self { account_id, public_key: secret_key.public_key(), secret_key }
    }
}

impl BlsSigner for InMemoryBlsSigner {
    fn public_key(&self) -> BlsPublicKey {
        self.public_key.clone()
    }

    fn sign(&self, data: &[u8]) -> BlsSignature {
        self.secret_key.sign(data)
    }

    fn write_to_file(&self, path: &Path) {
        BlsKeyFile::from(self).write_to_file(path);
    }
}

impl From<BlsKeyFile> for InMemoryBlsSigner {
    fn from(key_file: BlsKeyFile) -> Self {
        Self {
            account_id: key_file.account_id,
            public_key: key_file.public_key,
            secret_key: key_file.secret_key,
        }
    }
}

impl From<&InMemoryBlsSigner> for BlsKeyFile {
    fn from(signer: &InMemoryBlsSigner) -> BlsKeyFile {
        BlsKeyFile {
            account_id: signer.account_id.clone(),
            public_key: signer.public_key.clone(),
            secret_key: signer.secret_key.clone(),
        }
    }
}

impl From<Arc<InMemoryBlsSigner>> for BlsKeyFile {
    fn from(signer: Arc<InMemoryBlsSigner>) -> BlsKeyFile {
        BlsKeyFile {
            account_id: signer.account_id.clone(),
            public_key: signer.public_key.clone(),
            secret_key: signer.secret_key.clone(),
        }
    }
}