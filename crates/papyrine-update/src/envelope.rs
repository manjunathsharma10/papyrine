//! Signed envelope: `{"signed": "<exact JSON text>", "signatures": [{key_id, sig}]}`.
//!
//! The signed bytes are the `signed` string verbatim, so there is no
//! canonicalisation step to get wrong. A domain tag stops a signature for
//! one file type being replayed as another.

use crate::{Error, Result};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

pub const DOMAIN_KEYRING: &str = "papyrine-keyring-v1";
pub const DOMAIN_UPDATES: &str = "papyrine-updates-v1";

/// Largest envelope accepted from the network.
pub const MAX_ENVELOPE_BYTES: usize = 1 << 20;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SigEntry {
    pub key_id: String,
    /// Base64 Ed25519 signature.
    pub sig: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub signed: String,
    pub signatures: Vec<SigEntry>,
}

impl Envelope {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_ENVELOPE_BYTES {
            return Err(Error::Malformed("envelope too large".into()));
        }
        serde_json::from_slice(bytes).map_err(|e| Error::Malformed(e.to_string()))
    }

    /// The exact bytes a signature covers.
    pub fn message(domain: &str, signed: &str) -> Vec<u8> {
        let mut m = Vec::with_capacity(domain.len() + 1 + signed.len());
        m.extend_from_slice(domain.as_bytes());
        m.push(b'\n');
        m.extend_from_slice(signed.as_bytes());
        m
    }

    /// Verify the signature made by `key_id` with `key`.
    pub fn verify_with(&self, domain: &str, key_id: &str, key: &VerifyingKey) -> Result<()> {
        let entry = self
            .signatures
            .iter()
            .find(|s| s.key_id == key_id)
            .ok_or_else(|| Error::BadSignature(format!("no signature by {key_id}")))?;
        let raw = B64
            .decode(&entry.sig)
            .map_err(|_| Error::BadSignature("signature is not base64".into()))?;
        let sig = Signature::from_slice(&raw)
            .map_err(|_| Error::BadSignature("signature has the wrong length".into()))?;
        key.verify(&Self::message(domain, &self.signed), &sig)
            .map_err(|_| Error::BadSignature(format!("signature by {key_id} does not verify")))
    }
}

pub fn decode_public_key(b64: &str) -> Result<VerifyingKey> {
    let raw = B64
        .decode(b64)
        .map_err(|_| Error::Malformed("public key is not base64".into()))?;
    let arr: [u8; 32] = raw
        .try_into()
        .map_err(|_| Error::Malformed("public key must be 32 bytes".into()))?;
    VerifyingKey::from_bytes(&arr).map_err(|_| Error::Malformed("invalid public key".into()))
}

#[cfg(feature = "signing")]
pub mod signing {
    //! Signing side, compiled only for the release tool and tests.
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    pub fn signing_key_from_seed(seed: &[u8; 32]) -> SigningKey {
        SigningKey::from_bytes(seed)
    }

    pub fn public_key_b64(key: &SigningKey) -> String {
        B64.encode(key.verifying_key().to_bytes())
    }

    /// Sign `signed` (the exact payload text) and wrap it in an envelope.
    pub fn sign(domain: &str, signed: String, key_id: &str, key: &SigningKey) -> Envelope {
        let sig = key.sign(&Envelope::message(domain, &signed));
        Envelope {
            signed,
            signatures: vec![SigEntry {
                key_id: key_id.to_string(),
                sig: B64.encode(sig.to_bytes()),
            }],
        }
    }

    /// Add a further signature (two roots may both sign a keyring).
    pub fn add_signature(env: &mut Envelope, domain: &str, key_id: &str, key: &SigningKey) {
        let sig = key.sign(&Envelope::message(domain, &env.signed));
        env.signatures.push(SigEntry {
            key_id: key_id.to_string(),
            sig: B64.encode(sig.to_bytes()),
        });
    }
}
