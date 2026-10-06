//! The check itself: gate on opt-in/policy, fetch, verify, decide.

use crate::envelope::Envelope;
use crate::keyring::Keyring;
use crate::policy::Policy;
use crate::roots::Root;
use crate::settings::{CheckState, UpdateSettings};
use crate::transport::{Request, Transport, read_limited};
use crate::updates::{Banner, Updates};
use crate::{Error, envelope::MAX_ENVELOPE_BYTES};
use std::path::PathBuf;

pub const GITHUB_BASE: &str =
    "https://github.com/manjunathsharma10/papyrine/releases/latest/download";

pub type CheckError = Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// Daily background check: needs opt-in and the interval to have passed.
    Scheduled,
    /// Explicit user action (About -> "Check now", `papyrine update check`):
    /// the request is the consent for this one fetch, but policy still wins.
    UserRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
    /// First run: not asked yet.
    Unanswered,
    OptedOut,
    DisabledByPolicy,
    NotDue,
}

#[derive(Debug)]
pub enum CheckOutcome {
    /// No request was made.
    Skipped(Skip),
    Done {
        banner: Banner,
        /// Non-fatal problems (an ignored keyring, a stale cache...).
        warnings: Vec<String>,
    },
    /// `updates.json` was fetched but rejected: ignored, nothing shown.
    Ignored {
        reason: Error,
        warnings: Vec<String>,
    },
}

pub struct Checker<T: Transport> {
    pub transport: T,
    pub roots: Vec<Root>,
    pub policy: Policy,
    /// Installed version, e.g. `env!("CARGO_PKG_VERSION")`.
    pub installed: String,
    pub channel: String,
    /// Where the verified keyring is cached.
    pub cache_dir: PathBuf,
    /// Keyring shipped in the app bundle, used offline and as a floor.
    pub bundled_keyring: Option<Vec<u8>>,
}

impl<T: Transport> Checker<T> {
    pub fn new(
        transport: T,
        roots: Vec<Root>,
        policy: Policy,
        installed: &str,
        cache_dir: PathBuf,
    ) -> Self {
        Self {
            transport,
            roots,
            policy,
            installed: installed.to_string(),
            channel: "stable".into(),
            cache_dir,
            bundled_keyring: None,
        }
    }

    fn fetch(&self, base: &str, name: &str) -> crate::Result<Vec<u8>> {
        let resp = self.transport.get(&Request {
            url: format!("{base}/{name}"),
        })?;
        if resp.status != 200 {
            return Err(Error::Io(format!("{name}: HTTP {}", resp.status)));
        }
        read_limited(resp.body, MAX_ENVELOPE_BYTES)
    }

    fn verified_keyring(&self, bytes: &[u8]) -> crate::Result<Keyring> {
        Keyring::verify(&Envelope::parse(bytes)?, &self.roots)
    }

    /// Run a check. Updates `settings.last_check` and the serial counters;
    /// the caller persists `settings`.
    pub fn check(&self, settings: &mut UpdateSettings, trigger: Trigger, now: u64) -> CheckOutcome {
        match settings.state(&self.policy) {
            CheckState::DisabledByPolicy => return CheckOutcome::Skipped(Skip::DisabledByPolicy),
            CheckState::Unanswered if trigger == Trigger::Scheduled => {
                return CheckOutcome::Skipped(Skip::Unanswered);
            }
            CheckState::Disabled if trigger == Trigger::Scheduled => {
                return CheckOutcome::Skipped(Skip::OptedOut);
            }
            _ => {}
        }
        if trigger == Trigger::Scheduled && !settings.due(now) {
            return CheckOutcome::Skipped(Skip::NotDue);
        }
        // `mirror()` was validated by `state()` above.
        let base = match self.policy.mirror() {
            Ok(Some(m)) => m,
            _ => GITHUB_BASE.to_string(),
        };
        settings.last_check = now;
        let mut warnings = Vec::new();

        // Candidates: fetched, cached, bundled. The highest verified serial wins,
        // so a replayed old keyring cannot un-revoke a key.
        let mut best: Option<(Keyring, Option<Vec<u8>>)> = None;
        let mut consider = |ring: Keyring, raw: Option<Vec<u8>>| {
            if best.as_ref().is_none_or(|(b, _)| ring.serial > b.serial) {
                best = Some((ring, raw));
            }
        };
        match self.fetch(&base, "keyring.json") {
            Ok(raw) => match self.verified_keyring(&raw) {
                Ok(r) => consider(r, Some(raw)),
                Err(e) => warnings.push(format!("fetched keyring ignored: {e}")),
            },
            Err(e) => warnings.push(format!("keyring fetch failed: {e}")),
        }
        if let Ok(raw) = std::fs::read(self.cache_dir.join("keyring.json"))
            && let Ok(r) = self.verified_keyring(&raw)
        {
            consider(r, None);
        }
        if let Some(raw) = &self.bundled_keyring
            && let Ok(r) = self.verified_keyring(raw)
        {
            consider(r, None);
        }
        let Some((ring, raw)) = best else {
            return CheckOutcome::Ignored {
                reason: Error::BadSignature("no verified keyring available".into()),
                warnings,
            };
        };
        if ring.serial < settings.keyring_serial {
            return CheckOutcome::Ignored {
                reason: Error::Stale("keyring serial is older than one already accepted".into()),
                warnings,
            };
        }
        settings.keyring_serial = ring.serial;
        if let Some(raw) = raw {
            let _ = std::fs::create_dir_all(&self.cache_dir)
                .and_then(|_| std::fs::write(self.cache_dir.join("keyring.json"), raw));
        }

        let env = match self
            .fetch(&base, "updates.json")
            .and_then(|b| Envelope::parse(&b))
        {
            Ok(e) => e,
            Err(reason) => return CheckOutcome::Ignored { reason, warnings },
        };
        let updates = match Updates::verify(&env, &ring, now) {
            Ok(u) => u,
            Err(reason) => return CheckOutcome::Ignored { reason, warnings },
        };
        if updates.serial < settings.updates_serial {
            return CheckOutcome::Ignored {
                reason: Error::Stale(
                    "updates.json serial is older than one already accepted".into(),
                ),
                warnings,
            };
        }
        settings.updates_serial = updates.serial;
        match updates.decide(
            &self.installed,
            &self.channel,
            std::env::consts::OS,
            std::env::consts::ARCH,
        ) {
            Ok(banner) => CheckOutcome::Done { banner, warnings },
            Err(reason) => CheckOutcome::Ignored { reason, warnings },
        }
    }
}
