//! Embedded root public keys (ARCHITECTURE 9.3).
//!
//! PENDING OWNER ACTION: the two production roots are generated offline on
//! hardware security keys (docs/KEYS.md). Until then `PRODUCTION_ROOTS` is
//! empty and a release build verifies nothing (fail closed). The dev root is
//! a deterministic, publicly known, INSECURE key for tests and dev builds.

use crate::envelope::decode_public_key;
use ed25519_dalek::VerifyingKey;

#[derive(Debug, Clone)]
pub struct Root {
    pub id: String,
    pub key: VerifyingKey,
}

/// (id, base64 public key). Filled in after the key ceremony.
pub const PRODUCTION_ROOTS: &[(&str, &str)] = &[
    // ("root-primary", "<base64 Ed25519 public key>"),
    // ("root-backup", "<base64 Ed25519 public key>"),
];

pub const DEV_ROOT_ID: &str = "dev-root";
/// Public half of the dev root. Its seed is SHA-256 of [`DEV_ROOT_SEED_PHRASE`].
pub const DEV_ROOT_PUBLIC_B64: &str = "C1T6pxPnw2ad8CiC6S1Y84Wq1YW8U6G493TzP3bPGHo=";
pub const DEV_ROOT_SEED_PHRASE: &str = "papyrine-dev-root-INSECURE-do-not-trust";

/// Roots this build trusts: production roots, plus the dev root only when
/// the `dev-root` feature is on (which release builds cannot compile).
pub fn embedded_roots() -> Vec<Root> {
    #[allow(unused_mut)] // mutated only with the dev-root feature
    let mut out: Vec<Root> = PRODUCTION_ROOTS
        .iter()
        .filter_map(|(id, k)| {
            decode_public_key(k).ok().map(|key| Root {
                id: (*id).to_string(),
                key,
            })
        })
        .collect();
    #[cfg(feature = "dev-root")]
    if let Ok(key) = decode_public_key(DEV_ROOT_PUBLIC_B64) {
        out.push(Root {
            id: DEV_ROOT_ID.to_string(),
            key,
        });
    }
    out
}

/// Deterministic dev-root seed.
#[cfg(feature = "signing")]
pub fn dev_root_seed() -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(DEV_ROOT_SEED_PHRASE.as_bytes()).into()
}
