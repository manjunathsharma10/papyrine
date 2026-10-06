//! `keyring.json`: online keys authorised by a root (ARCHITECTURE 9.3).

use crate::envelope::{DOMAIN_KEYRING, Envelope, decode_public_key};
use crate::roots::Root;
use crate::{Error, Result};
use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyPurpose {
    Release,
    Component,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnlineKey {
    pub id: String,
    pub purpose: KeyPurpose,
    /// Base64 Ed25519 public key.
    pub public_key: String,
    /// Unix seconds, inclusive.
    pub not_before: u64,
    /// Unix seconds, exclusive.
    pub not_after: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Revocation {
    pub id: String,
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Keyring {
    pub schema: u32,
    /// Strictly increasing; a lower serial than one already accepted is a rollback.
    pub serial: u64,
    pub issued: u64,
    pub keys: Vec<OnlineKey>,
    #[serde(default)]
    pub revoked: Vec<Revocation>,
}

impl Keyring {
    /// Verify `env` was signed by one of `roots` and parse the payload.
    pub fn verify(env: &Envelope, roots: &[Root]) -> Result<Keyring> {
        let mut last = Error::BadSignature("no signature by a trusted root".into());
        for s in &env.signatures {
            let Some(root) = roots.iter().find(|r| r.id == s.key_id) else {
                continue;
            };
            match env.verify_with(DOMAIN_KEYRING, &root.id, &root.key) {
                Ok(()) => {
                    let ring: Keyring = serde_json::from_str(&env.signed)
                        .map_err(|e| Error::Malformed(format!("keyring: {e}")))?;
                    if ring.schema != 1 {
                        return Err(Error::Malformed(format!(
                            "unsupported keyring schema {}",
                            ring.schema
                        )));
                    }
                    return Ok(ring);
                }
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    pub fn is_revoked(&self, id: &str) -> bool {
        self.revoked.iter().any(|r| r.id == id)
    }

    /// Resolve an online key for `purpose` at time `now`, rejecting revoked,
    /// not-yet-valid, expired and wrong-purpose keys.
    pub fn key_for(&self, id: &str, purpose: KeyPurpose, now: u64) -> Result<VerifyingKey> {
        if self.is_revoked(id) {
            return Err(Error::KeyRevoked(id.to_string()));
        }
        let k = self
            .keys
            .iter()
            .find(|k| k.id == id)
            .ok_or_else(|| Error::BadSignature(format!("key {id} is not in the keyring")))?;
        if k.purpose != purpose {
            return Err(Error::WrongPurpose(id.to_string()));
        }
        if now < k.not_before {
            return Err(Error::KeyNotValid(format!("{id} is not valid yet")));
        }
        if now >= k.not_after {
            return Err(Error::KeyNotValid(format!("{id} has expired")));
        }
        decode_public_key(&k.public_key)
    }
}
