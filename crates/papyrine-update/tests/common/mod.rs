#![allow(dead_code)]
use ed25519_dalek::SigningKey;
use papyrine_update::download::Opener;
use papyrine_update::envelope::{DOMAIN_KEYRING, DOMAIN_UPDATES, Envelope, signing};
use papyrine_update::keyring::{KeyPurpose, Keyring, OnlineKey, Revocation};
use papyrine_update::roots::{self, Root};
use papyrine_update::transport::{Request, Response, Transport};
use papyrine_update::updates::{Channel, Download, Severity, Updates};
use papyrine_update::{Error, Result};
use std::collections::{BTreeMap, HashMap};
use std::io::Cursor;
use std::path::Path;
use std::sync::Mutex;

pub const NOW: u64 = 1_800_000_000;

pub fn root_key() -> SigningKey {
    signing::signing_key_from_seed(&roots::dev_root_seed())
}
pub fn root() -> Root {
    Root {
        id: roots::DEV_ROOT_ID.into(),
        key: root_key().verifying_key(),
    }
}
pub fn release_key() -> SigningKey {
    signing::signing_key_from_seed(&[7u8; 32])
}
pub fn other_key() -> SigningKey {
    signing::signing_key_from_seed(&[9u8; 32])
}

pub fn online(id: &str, key: &SigningKey, purpose: KeyPurpose, nb: u64, na: u64) -> OnlineKey {
    OnlineKey {
        id: id.into(),
        purpose,
        public_key: signing::public_key_b64(key),
        not_before: nb,
        not_after: na,
    }
}

pub fn good_keyring(serial: u64) -> Keyring {
    Keyring {
        schema: 1,
        serial,
        issued: NOW - 10,
        keys: vec![online("rel-1", &release_key(), KeyPurpose::Release, NOW - 1000, NOW + 1_000_000)],
        revoked: vec![],
    }
}

pub fn sign_keyring(k: &Keyring) -> Vec<u8> {
    let env = signing::sign(DOMAIN_KEYRING, serde_json::to_string(k).unwrap(), roots::DEV_ROOT_ID, &root_key());
    serde_json::to_vec(&env).unwrap()
}

pub fn dl(sha: &str, size: u64) -> Download {
    Download {
        os: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        url: "https://github.com/manjunathsharma10/papyrine/releases/download/v0.1.3/Papyrine-0.1.3.dmg".into(),
        sha256: sha.into(),
        size,
    }
}

pub fn updates(serial: u64, version: &str, sev: Severity, affected: &[&str]) -> Updates {
    let mut channels = BTreeMap::new();
    channels.insert(
        "stable".to_string(),
        Channel {
            version: version.into(),
            severity: sev,
            advisory: "fixes a crash when opening crafted PDFs".into(),
            affected: affected.iter().map(|s| s.to_string()).collect(),
            downloads: vec![dl(&"0".repeat(64), 10)],
        },
    );
    Updates { schema: 1, serial, issued: NOW - 5, expires: NOW + 86_400, channels }
}

pub fn sign_updates_with(u: &Updates, id: &str, key: &SigningKey) -> Vec<u8> {
    let env = signing::sign(DOMAIN_UPDATES, serde_json::to_string(u).unwrap(), id, key);
    serde_json::to_vec(&env).unwrap()
}
pub fn sign_updates(u: &Updates) -> Vec<u8> {
    sign_updates_with(u, "rel-1", &release_key())
}

pub fn envelope(bytes: &[u8]) -> Envelope {
    Envelope::parse(bytes).unwrap()
}

pub fn revoked(mut k: Keyring, id: &str) -> Keyring {
    k.revoked.push(Revocation { id: id.into(), reason: "test".into() });
    k
}

/// Records every request; serves canned bodies by URL suffix.
#[derive(Default)]
pub struct Recording {
    pub requests: Mutex<Vec<Request>>,
    pub files: Mutex<HashMap<String, Vec<u8>>>,
}
impl Recording {
    pub fn with(files: &[(&str, Vec<u8>)]) -> Self {
        let r = Recording::default();
        for (n, b) in files {
            r.files.lock().unwrap().insert(n.to_string(), b.clone());
        }
        r
    }
    pub fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}
impl Transport for Recording {
    fn get(&self, req: &Request) -> Result<Response> {
        self.requests.lock().unwrap().push(req.clone());
        let files = self.files.lock().unwrap();
        let hit = files.iter().find(|(n, _)| req.url.ends_with(n.as_str()));
        match hit {
            Some((_, b)) => Ok(Response { status: 200, body: Box::new(Cursor::new(b.clone())) }),
            None => Ok(Response { status: 404, body: Box::new(Cursor::new(Vec::new())) }),
        }
    }
}

#[derive(Default)]
pub struct RecordingOpener(pub Mutex<Vec<std::path::PathBuf>>);
impl Opener for RecordingOpener {
    fn open(&self, p: &Path) -> std::result::Result<(), Error> {
        self.0.lock().unwrap().push(p.to_path_buf());
        Ok(())
    }
}
