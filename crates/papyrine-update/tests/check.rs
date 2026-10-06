mod common;
use common::*;
use papyrine_update::Error;
use papyrine_update::check::{CheckOutcome, Checker, Skip, Trigger};
use papyrine_update::keyring::KeyPurpose;
use papyrine_update::policy::Policy;
use papyrine_update::settings::UpdateSettings;
use papyrine_update::updates::{Banner, Severity};

fn enabled() -> UpdateSettings {
    let mut s = UpdateSettings::default();
    s.set_choice(true);
    s
}

fn checker(t: Recording, policy: Policy, dir: &std::path::Path) -> Checker<Recording> {
    Checker::new(t, vec![root()], policy, "0.1.0", dir.to_path_buf())
}

fn files(ring: &papyrine_update::Keyring, u: &papyrine_update::Updates) -> Recording {
    Recording::with(&[
        ("keyring.json", sign_keyring(ring)),
        ("updates.json", sign_updates(u)),
    ])
}

fn sec_updates(serial: u64) -> papyrine_update::Updates {
    updates(serial, "0.1.3", Severity::Security, &[">=0.1.0, <0.1.3"])
}

#[test]
fn nothing_is_fetched_unless_opted_in() {
    let dir = tempfile::tempdir().unwrap();
    let ring = good_keyring(1);
    let u = sec_updates(1);

    // First run: unanswered.
    let c = checker(files(&ring, &u), Policy::default(), dir.path());
    let mut s = UpdateSettings::default();
    assert!(matches!(
        c.check(&mut s, Trigger::Scheduled, NOW),
        CheckOutcome::Skipped(Skip::Unanswered)
    ));
    assert_eq!(c.transport.count(), 0);

    // Opted out.
    let mut s = UpdateSettings::default();
    s.set_choice(false);
    assert!(matches!(
        c.check(&mut s, Trigger::Scheduled, NOW),
        CheckOutcome::Skipped(Skip::OptedOut)
    ));
    assert_eq!(c.transport.count(), 0);

    // Opted in but policy disables: even an explicit request is refused.
    let pol = Policy {
        update_check_disabled: true,
        update_mirror_url: None,
    };
    let c = checker(files(&ring, &u), pol, dir.path());
    let mut s = enabled();
    for t in [Trigger::Scheduled, Trigger::UserRequest] {
        assert!(matches!(
            c.check(&mut s, t, NOW),
            CheckOutcome::Skipped(Skip::DisabledByPolicy)
        ));
    }
    // A set-but-invalid mirror must not fall back to GitHub.
    let pol = Policy {
        update_check_disabled: false,
        update_mirror_url: Some("http://evil".into()),
    };
    let c = checker(files(&ring, &u), pol, dir.path());
    assert!(matches!(
        c.check(&mut s, Trigger::UserRequest, NOW),
        CheckOutcome::Skipped(Skip::DisabledByPolicy)
    ));
    assert_eq!(c.transport.count(), 0);

    // Opted in but checked less than a day ago.
    let c = checker(files(&ring, &u), Policy::default(), dir.path());
    let mut s = enabled();
    s.last_check = NOW - 60;
    assert!(matches!(
        c.check(&mut s, Trigger::Scheduled, NOW),
        CheckOutcome::Skipped(Skip::NotDue)
    ));
    assert_eq!(c.transport.count(), 0);
}

#[test]
fn opted_in_check_produces_security_banner_and_sends_only_two_bare_gets() {
    let dir = tempfile::tempdir().unwrap();
    let c = checker(
        files(&good_keyring(1), &sec_updates(1)),
        Policy::default(),
        dir.path(),
    );
    let mut s = enabled();
    let out = c.check(&mut s, Trigger::Scheduled, NOW);
    match out {
        CheckOutcome::Done {
            banner: Banner::Security {
                version, download, ..
            },
            ..
        } => {
            assert_eq!(version, "0.1.3");
            assert!(download.is_some());
        }
        other => panic!("unexpected {other:?}"),
    }
    let reqs = c.transport.requests.lock().unwrap();
    let urls: Vec<_> = reqs.iter().map(|r| r.url.as_str()).collect();
    assert_eq!(
        urls,
        [
            "https://github.com/manjunathsharma10/papyrine/releases/latest/download/keyring.json",
            "https://github.com/manjunathsharma10/papyrine/releases/latest/download/updates.json",
        ]
    );
    for u in urls {
        assert!(
            !u.contains(['?', '#', '@']),
            "no query, fragment or userinfo"
        );
        assert!(!u.contains("0.1.0"), "installed version must not leak");
    }
    assert_eq!(s.last_check, NOW);
    // The keyring is cached for offline use.
    assert!(dir.path().join("keyring.json").exists());
}

#[test]
fn mirror_policy_redirects_the_request() {
    let dir = tempfile::tempdir().unwrap();
    let pol = Policy {
        update_check_disabled: false,
        update_mirror_url: Some("https://mirror.corp.example/papyrine/".into()),
    };
    let c = checker(files(&good_keyring(1), &sec_updates(1)), pol, dir.path());
    let mut s = enabled();
    c.check(&mut s, Trigger::Scheduled, NOW);
    let reqs = c.transport.requests.lock().unwrap();
    assert_eq!(
        reqs[0].url,
        "https://mirror.corp.example/papyrine/keyring.json"
    );
    assert_eq!(
        reqs[1].url,
        "https://mirror.corp.example/papyrine/updates.json"
    );
}

fn ignored(out: CheckOutcome) -> Error {
    match out {
        CheckOutcome::Ignored { reason, .. } => reason,
        other => panic!("expected Ignored, got {other:?}"),
    }
}

fn run_with_updates_bytes(bytes: Vec<u8>, ring: &papyrine_update::Keyring) -> CheckOutcome {
    let dir = tempfile::tempdir().unwrap();
    let t = Recording::with(&[
        ("keyring.json", sign_keyring(ring)),
        ("updates.json", bytes),
    ]);
    let c = checker(t, Policy::default(), dir.path());
    c.check(&mut enabled(), Trigger::Scheduled, NOW)
}

#[test]
fn tampered_updates_are_ignored() {
    let ring = good_keyring(1);
    let good = sign_updates(&sec_updates(1));
    let mut env = envelope(&good);
    env.signed = env.signed.replace("0.1.3", "9.9.9");
    let r = ignored(run_with_updates_bytes(
        serde_json::to_vec(&env).unwrap(),
        &ring,
    ));
    assert!(matches!(r, Error::BadSignature(_)), "{r}");
}

#[test]
fn unsigned_updates_are_ignored() {
    let ring = good_keyring(1);
    let mut env = envelope(&sign_updates(&sec_updates(1)));
    env.signatures.clear();
    assert!(matches!(
        ignored(run_with_updates_bytes(
            serde_json::to_vec(&env).unwrap(),
            &ring
        )),
        Error::BadSignature(_)
    ));
    // Plain unwrapped JSON and garbage are ignored too.
    let plain = serde_json::to_vec(&sec_updates(1)).unwrap();
    assert!(matches!(
        ignored(run_with_updates_bytes(plain, &ring)),
        Error::Malformed(_)
    ));
    assert!(matches!(
        ignored(run_with_updates_bytes(b"not json".to_vec(), &ring)),
        Error::Malformed(_)
    ));
}

#[test]
fn updates_signed_by_unlisted_or_wrong_key_are_ignored() {
    let ring = good_keyring(1);
    // Right id, wrong private key.
    let b = sign_updates_with(&sec_updates(1), "rel-1", &other_key());
    assert!(matches!(
        ignored(run_with_updates_bytes(b, &ring)),
        Error::BadSignature(_)
    ));
    // Key not in the keyring at all.
    let b = sign_updates_with(&sec_updates(1), "rel-x", &other_key());
    assert!(matches!(
        ignored(run_with_updates_bytes(b, &ring)),
        Error::BadSignature(_)
    ));
}

#[test]
fn signature_does_not_transfer_between_file_types() {
    // A signature over a keyring payload's text must not validate as updates.json.
    let ring = good_keyring(1);
    let u = sec_updates(1);
    let env = papyrine_update::envelope::signing::sign(
        papyrine_update::envelope::DOMAIN_KEYRING,
        serde_json::to_string(&u).unwrap(),
        "rel-1",
        &release_key(),
    );
    let r = ignored(run_with_updates_bytes(
        serde_json::to_vec(&env).unwrap(),
        &ring,
    ));
    assert!(matches!(r, Error::BadSignature(_)));
}

#[test]
fn revoked_key_is_rejected() {
    let ring = revoked(good_keyring(2), "rel-1");
    let r = ignored(run_with_updates_bytes(sign_updates(&sec_updates(1)), &ring));
    assert!(matches!(r, Error::KeyRevoked(_)), "{r}");
}

#[test]
fn expired_not_yet_valid_and_wrong_purpose_keys_are_rejected() {
    let mut ring = good_keyring(1);
    ring.keys[0].not_after = NOW; // exclusive end: expired exactly now
    let r = ignored(run_with_updates_bytes(sign_updates(&sec_updates(1)), &ring));
    assert!(
        matches!(r, Error::KeyNotValid(ref m) if m.contains("expired")),
        "{r}"
    );

    let mut ring = good_keyring(1);
    ring.keys[0].not_before = NOW + 1;
    let r = ignored(run_with_updates_bytes(sign_updates(&sec_updates(1)), &ring));
    assert!(
        matches!(r, Error::KeyNotValid(ref m) if m.contains("not valid yet")),
        "{r}"
    );

    let mut ring = good_keyring(1);
    ring.keys[0].purpose = KeyPurpose::Component;
    let r = ignored(run_with_updates_bytes(sign_updates(&sec_updates(1)), &ring));
    assert!(matches!(r, Error::WrongPurpose(_)), "{r}");
}

#[test]
fn expired_updates_file_is_ignored() {
    let mut u = sec_updates(1);
    u.expires = NOW - 1;
    let r = ignored(run_with_updates_bytes(sign_updates(&u), &good_keyring(1)));
    assert!(matches!(r, Error::Stale(_)));
}

#[test]
fn keyring_not_signed_by_a_root_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let ring = good_keyring(1);
    let forged = papyrine_update::envelope::signing::sign(
        papyrine_update::envelope::DOMAIN_KEYRING,
        serde_json::to_string(&ring).unwrap(),
        "dev-root",
        &other_key(),
    );
    let t = Recording::with(&[
        ("keyring.json", serde_json::to_vec(&forged).unwrap()),
        ("updates.json", sign_updates(&sec_updates(1))),
    ]);
    let c = checker(t, Policy::default(), dir.path());
    let out = c.check(&mut enabled(), Trigger::Scheduled, NOW);
    let CheckOutcome::Ignored { warnings, .. } = out else {
        panic!("{out:?}")
    };
    assert!(warnings.iter().any(|w| w.contains("keyring")));
}

#[test]
fn replayed_old_keyring_cannot_unrevoke_a_key() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = enabled();
    // First check sees the revoking keyring (serial 2) and caches it.
    let t = Recording::with(&[
        (
            "keyring.json",
            sign_keyring(&revoked(good_keyring(2), "rel-1")),
        ),
        ("updates.json", sign_updates(&sec_updates(1))),
    ]);
    checker(t, Policy::default(), dir.path()).check(&mut s, Trigger::UserRequest, NOW);
    // A mirror now replays the old keyring (serial 1) that still trusts rel-1.
    let t = files(&good_keyring(1), &sec_updates(1));
    let out = checker(t, Policy::default(), dir.path()).check(&mut s, Trigger::UserRequest, NOW);
    assert!(matches!(ignored(out), Error::KeyRevoked(_)));
}

#[test]
fn bundled_keyring_works_offline_and_updates_serial_rollback_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = enabled();
    // No keyring.json on the server (404): fall back to the bundled one.
    let t = Recording::with(&[("updates.json", sign_updates(&sec_updates(5)))]);
    let mut c = checker(t, Policy::default(), dir.path());
    c.bundled_keyring = Some(sign_keyring(&good_keyring(1)));
    assert!(matches!(
        c.check(&mut s, Trigger::UserRequest, NOW),
        CheckOutcome::Done { .. }
    ));
    assert_eq!(s.updates_serial, 5);
    // An older (but validly signed) updates.json is a rollback.
    let t = Recording::with(&[("updates.json", sign_updates(&sec_updates(4)))]);
    let mut c = checker(t, Policy::default(), dir.path());
    c.bundled_keyring = Some(sign_keyring(&good_keyring(1)));
    assert!(matches!(
        ignored(c.check(&mut s, Trigger::UserRequest, NOW)),
        Error::Stale(_)
    ));
}

#[test]
fn banner_decisions() {
    let d =
        |u: &papyrine_update::Updates, v: &str| u.decide(v, "stable", "macos", "aarch64").unwrap();
    let sec = sec_updates(1);
    assert!(matches!(d(&sec, "0.1.0"), Banner::Security { .. }));
    assert!(matches!(d(&sec, "0.1.2"), Banner::Security { .. }));
    assert_eq!(d(&sec, "0.1.3"), Banner::UpToDate);
    assert_eq!(d(&sec, "0.2.0"), Banner::UpToDate);
    assert_eq!(d(&sec, "1.0.0-beta.1"), Banner::UpToDate);
    // Newer security release that does not affect this version: About only.
    let narrow = updates(1, "0.3.1", Severity::Security, &[">=0.3.0, <0.3.1"]);
    assert!(matches!(
        d(&narrow, "0.2.5"),
        Banner::NormalAvailable { .. }
    ));
    assert!(matches!(d(&narrow, "0.3.0"), Banner::Security { .. }));
    // Normal severity never banners.
    let normal = updates(1, "0.2.0", Severity::Normal, &[]);
    assert!(matches!(
        d(&normal, "0.1.0"),
        Banner::NormalAvailable { .. }
    ));
    // Empty affected list on a security release means everything older.
    let all = updates(1, "0.2.0", Severity::Security, &[]);
    assert!(matches!(d(&all, "0.1.9"), Banner::Security { .. }));
    // Other channel or bad version strings.
    assert_eq!(
        sec.decide("0.1.0", "beta", "macos", "aarch64").unwrap(),
        Banner::UpToDate
    );
    assert!(
        sec.decide("not-a-version", "stable", "macos", "aarch64")
            .is_err()
    );
    // No installer for this platform: banner without a download.
    let Banner::Security { download, .. } = sec.decide("0.1.0", "stable", "plan9", "mips").unwrap()
    else {
        panic!()
    };
    assert!(download.is_none());
}
