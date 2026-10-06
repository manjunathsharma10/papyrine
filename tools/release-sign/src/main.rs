//! `release-sign`: key generation and signing for the update channel.
//!
//! Keys are read from `PAPYRINE_SIGN_KEY` (hex seed) or `--key-file`; they are
//! never accepted on the command line. `--dev` uses the INSECURE deterministic
//! dev keys and is refused for anything but dev-root verification.
//!
//! ```text
//! release-sign gen-key --out seed.hex
//! release-sign dev-keys
//! release-sign sign-keyring --in keyring.payload.json --id root-primary --out keyring.json [--key-file F | --dev]
//! release-sign sign-updates --in updates.payload.json --id release-2026 --out updates.json [--key-file F | --dev]
//! release-sign verify-keyring keyring.json --root-id ID --root-pub B64 | --dev
//! release-sign verify-updates updates.json --keyring keyring.json (--root-id ID --root-pub B64 | --dev) [--now UNIX]
//! ```

use ed25519_dalek::SigningKey;
use papyrine_update::envelope::{DOMAIN_KEYRING, DOMAIN_UPDATES, Envelope, decode_public_key, signing};
use papyrine_update::keyring::Keyring;
use papyrine_update::roots::{self, Root};
use papyrine_update::updates::Updates;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Write;
use std::process::ExitCode;

const DEV_RELEASE_PHRASE: &str = "papyrine-dev-release-INSECURE-do-not-trust";

type Res<T> = Result<T, String>;

struct Args {
    pos: Vec<String>,
    opt: HashMap<String, String>,
    flags: Vec<String>,
}

fn parse_args(raw: Vec<String>) -> Args {
    let mut a = Args {
        pos: vec![],
        opt: HashMap::new(),
        flags: vec![],
    };
    let mut it = raw.into_iter().peekable();
    while let Some(x) = it.next() {
        if let Some(name) = x.strip_prefix("--") {
            if name == "dev" {
                a.flags.push(name.into());
            } else if let Some(v) = it.next() {
                a.opt.insert(name.into(), v);
            }
        } else {
            a.pos.push(x);
        }
    }
    a
}

impl Args {
    fn need(&self, k: &str) -> Res<&str> {
        self.opt
            .get(k)
            .map(String::as_str)
            .ok_or_else(|| format!("missing --{k}"))
    }
    fn dev(&self) -> bool {
        self.flags.iter().any(|f| f == "dev")
    }
}

fn seed_from_hex(s: &str) -> Res<[u8; 32]> {
    let s = s.trim();
    if s.len() != 64 || !s.is_ascii() {
        return Err("seed must be 64 hex characters".into());
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).map_err(|_| "seed is not hex")?;
    }
    Ok(out)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn load_key(a: &Args, dev_phrase: &str) -> Res<SigningKey> {
    if a.dev() {
        let seed: [u8; 32] = Sha256::digest(dev_phrase.as_bytes()).into();
        return Ok(signing::signing_key_from_seed(&seed));
    }
    let text = match a.opt.get("key-file") {
        Some(f) => std::fs::read_to_string(f).map_err(|e| format!("{f}: {e}"))?,
        None => std::env::var("PAPYRINE_SIGN_KEY")
            .map_err(|_| "set PAPYRINE_SIGN_KEY or pass --key-file (or --dev)".to_string())?,
    };
    Ok(signing::signing_key_from_seed(&seed_from_hex(&text)?))
}

fn write_out(path: &str, env: &Envelope) -> Res<()> {
    let text = serde_json::to_string_pretty(env).map_err(|e| e.to_string())?;
    std::fs::write(path, text + "\n").map_err(|e| format!("{path}: {e}"))
}

fn roots_from(a: &Args) -> Res<Vec<Root>> {
    if a.dev() {
        let key = load_key(a, roots::DEV_ROOT_SEED_PHRASE)?;
        return Ok(vec![Root {
            id: roots::DEV_ROOT_ID.into(),
            key: key.verifying_key(),
        }]);
    }
    Ok(vec![Root {
        id: a.need("root-id")?.into(),
        key: decode_public_key(a.need("root-pub")?).map_err(|e| e.to_string())?,
    }])
}

fn run(raw: Vec<String>) -> Res<()> {
    let a = parse_args(raw);
    let cmd = a.pos.first().map(String::as_str).unwrap_or("");
    match cmd {
        "gen-key" => {
            let out = a.need("out")?;
            let mut seed = [0u8; 32];
            getrandom::fill(&mut seed).map_err(|e| format!("no OS randomness: {e}"))?;
            let key = SigningKey::from_bytes(&seed);
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
            let mut f = opts.open(out).map_err(|e| format!("{out}: {e}"))?;
            writeln!(f, "{}", hex(&seed)).map_err(|e| e.to_string())?;
            println!("public_key: {}", signing::public_key_b64(&key));
            Ok(())
        }
        "dev-keys" => {
            let root = load_key(
                &Args { pos: vec![], opt: HashMap::new(), flags: vec!["dev".into()] },
                roots::DEV_ROOT_SEED_PHRASE,
            )?;
            let rel = load_key(
                &Args { pos: vec![], opt: HashMap::new(), flags: vec!["dev".into()] },
                DEV_RELEASE_PHRASE,
            )?;
            println!("dev-root public_key: {}", signing::public_key_b64(&root));
            println!("dev-release public_key: {}", signing::public_key_b64(&rel));
            Ok(())
        }
        "sign-keyring" => {
            let payload = std::fs::read_to_string(a.need("in")?).map_err(|e| e.to_string())?;
            let ring: Keyring = serde_json::from_str(&payload).map_err(|e| format!("payload: {e}"))?;
            let signed = serde_json::to_string(&ring).map_err(|e| e.to_string())?;
            let key = load_key(&a, roots::DEV_ROOT_SEED_PHRASE)?;
            let id = if a.dev() { roots::DEV_ROOT_ID } else { a.need("id")? };
            write_out(a.need("out")?, &signing::sign(DOMAIN_KEYRING, signed, id, &key))
        }
        "sign-updates" => {
            let payload = std::fs::read_to_string(a.need("in")?).map_err(|e| e.to_string())?;
            let u: Updates = serde_json::from_str(&payload).map_err(|e| format!("payload: {e}"))?;
            let signed = serde_json::to_string(&u).map_err(|e| e.to_string())?;
            let key = load_key(&a, DEV_RELEASE_PHRASE)?;
            write_out(
                a.need("out")?,
                &signing::sign(DOMAIN_UPDATES, signed, a.need("id")?, &key),
            )
        }
        "verify-keyring" => {
            let file = a.pos.get(1).ok_or("missing file")?;
            let env = Envelope::parse(&std::fs::read(file).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            let ring = Keyring::verify(&env, &roots_from(&a)?).map_err(|e| e.to_string())?;
            println!("ok: serial {}, {} keys, {} revoked", ring.serial, ring.keys.len(), ring.revoked.len());
            Ok(())
        }
        "verify-updates" => {
            let file = a.pos.get(1).ok_or("missing file")?;
            let env = Envelope::parse(&std::fs::read(file).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            let kr = Envelope::parse(&std::fs::read(a.need("keyring")?).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            let ring = Keyring::verify(&kr, &roots_from(&a)?).map_err(|e| e.to_string())?;
            let now = match a.opt.get("now") {
                Some(n) => n.parse().map_err(|_| "bad --now")?,
                None => papyrine_update::unix_now(),
            };
            let u = Updates::verify(&env, &ring, now).map_err(|e| e.to_string())?;
            println!("ok: serial {}, channels: {}", u.serial, u.channels.keys().cloned().collect::<Vec<_>>().join(","));
            Ok(())
        }
        _ => Err("commands: gen-key | dev-keys | sign-keyring | sign-updates | verify-keyring | verify-updates".into()),
    }
}

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("release-sign: {e}");
            ExitCode::FAILURE
        }
    }
}
