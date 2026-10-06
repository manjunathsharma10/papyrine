mod common;
use common::*;
use papyrine_update::download::{download_and_open, download_verified};
use papyrine_update::policy::{self, Policy, parse_reg_query, validate_mirror};
use papyrine_update::roots;
use papyrine_update::settings::{CheckState, UpdateSettings};
use papyrine_update::{Error, envelope};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::net::TcpListener;

#[test]
fn dev_root_constant_matches_its_seed() {
    let k = envelope::signing::signing_key_from_seed(&roots::dev_root_seed());
    assert_eq!(
        envelope::signing::public_key_b64(&k),
        roots::DEV_ROOT_PUBLIC_B64
    );
    assert!(
        roots::embedded_roots()
            .iter()
            .any(|r| r.id == roots::DEV_ROOT_ID)
    );
}

#[test]
fn settings_have_no_default_and_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("sub/update.json");
    let mut s = UpdateSettings::load(&p);
    assert_eq!(s.state(&Policy::default()), CheckState::Unanswered);
    s.set_choice(true);
    s.last_check = 42;
    s.save(&p).unwrap();
    let s2 = UpdateSettings::load(&p);
    assert_eq!(s2, s);
    assert_eq!(s2.state(&Policy::default()), CheckState::Enabled);
    let pol = Policy {
        update_check_disabled: true,
        update_mirror_url: None,
    };
    assert_eq!(s2.state(&pol), CheckState::DisabledByPolicy);
    // Corrupt file means "unanswered", never "enabled".
    std::fs::write(&p, b"{{{").unwrap();
    assert_eq!(
        UpdateSettings::load(&p).state(&Policy::default()),
        CheckState::Unanswered
    );
}

#[test]
fn policy_sources_parse() {
    let p = Policy::from_json(r#"{"update_check_disabled": true}"#);
    assert!(p.update_check_disabled);
    let p = Policy::from_json(r#"{"UpdateMirrorUrl": "https://m.example/x/"}"#);
    assert_eq!(p.mirror(), Ok(Some("https://m.example/x".into())));
    // Unparseable policy fails closed.
    assert!(Policy::from_json("garbage").update_check_disabled);
    assert!(validate_mirror("http://m.example").is_none());
    assert!(validate_mirror("https://u:p@m.example").is_none());
    assert!(validate_mirror("https://m.example/a?x=1").is_none());
    assert!(validate_mirror("https://").is_none());
    let reg = "HKEY_LOCAL_MACHINE\\Software\\Policies\\Papyrine\n    UpdateCheckDisabled    REG_DWORD    0x1\n    UpdateMirrorUrl    REG_SZ    https://m.example/p\n";
    let p = parse_reg_query(reg);
    assert!(p.update_check_disabled);
    assert_eq!(p.update_mirror_url.as_deref(), Some("https://m.example/p"));
    assert!(!parse_reg_query("    UpdateCheckDisabled    REG_DWORD    0x0").update_check_disabled);
    // Merge: disable wins.
    assert!(Policy::default().merge(p).update_check_disabled);
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("policy.json");
    std::fs::write(&f, r#"{"update_check_disabled": true}"#).unwrap();
    assert!(Policy::load_from(&f).update_check_disabled);
    assert_eq!(policy::MAC_DOMAIN, "io.github.manjunathsharma10.papyrine");
}

fn sha(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

#[test]
fn verified_download_then_handoff() {
    let data = vec![0xABu8; 200_000];
    let d = dl(&sha(&data), data.len() as u64);
    let t = Recording::with(&[("Papyrine-0.1.3.dmg", data.clone())]);
    let dir = tempfile::tempdir().unwrap();
    let op = RecordingOpener::default();
    let v = download_and_open(&t, &d, dir.path(), &op).unwrap();
    assert_eq!(std::fs::read(&v.path).unwrap(), data);
    assert_eq!(
        op.0.lock().unwrap().as_slice(),
        std::slice::from_ref(&v.path)
    );
    assert!(v.path.ends_with("Papyrine-0.1.3.dmg"));
    // The request is a bare GET of the signed URL.
    assert_eq!(t.requests.lock().unwrap()[0].url, d.url);
}

#[test]
fn bad_downloads_are_deleted_and_never_opened() {
    let data = vec![1u8; 5000];
    let dir = tempfile::tempdir().unwrap();
    let op = RecordingOpener::default();
    let t = Recording::with(&[("Papyrine-0.1.3.dmg", data.clone())]);

    // Wrong hash.
    let d = dl(&"0".repeat(64), data.len() as u64);
    assert!(matches!(
        download_and_open(&t, &d, dir.path(), &op),
        Err(Error::Integrity(_))
    ));
    // Declared size smaller than the body.
    let d = dl(&sha(&data), 100);
    assert!(matches!(
        download_and_open(&t, &d, dir.path(), &op),
        Err(Error::Integrity(_))
    ));
    // Declared size larger than the body.
    let d = dl(&sha(&data), 6000);
    assert!(matches!(
        download_and_open(&t, &d, dir.path(), &op),
        Err(Error::Integrity(_))
    ));
    // Non-https URL.
    let mut d = dl(&sha(&data), 5000);
    d.url = d.url.replace("https://", "http://");
    assert!(matches!(
        download_verified(&t, &d, dir.path()),
        Err(Error::Integrity(_))
    ));
    assert!(op.0.lock().unwrap().is_empty());
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        0,
        "no partial files left"
    );
}

/// Wire-level: what ureq really sends. A loopback server captures the raw request.
#[test]
fn real_request_carries_no_identifiers() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let mut raw = Vec::new();
        let mut buf = [0u8; 1024];
        while !raw.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = s.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            raw.extend_from_slice(&buf[..n]);
        }
        s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
            .unwrap();
        String::from_utf8(raw).unwrap()
    });
    let t = papyrine_update::UreqTransport::plain_http_for_tests();
    let url = format!("http://127.0.0.1:{port}/updates.json");
    let mut resp = papyrine_update::Transport::get(&t, &papyrine_update::Request { url }).unwrap();
    assert_eq!(resp.status, 200);
    let mut body = String::new();
    resp.body.read_to_string(&mut body).unwrap();
    assert_eq!(body, "{}");
    let raw = server.join().unwrap();
    let mut lines = raw.split("\r\n");
    assert_eq!(lines.next().unwrap(), "GET /updates.json HTTP/1.1");
    let mut names: Vec<String> = lines
        .take_while(|l| !l.is_empty())
        .map(|l| l.split(':').next().unwrap().to_ascii_lowercase())
        .collect();
    names.sort();
    // Only the standard transport headers; nothing that can carry an identifier.
    let allowed = ["accept", "host", "user-agent"];
    for n in &names {
        assert!(allowed.contains(&n.as_str()), "unexpected header {n}");
    }
    for forbidden in ["cookie", "authorization", "referer", "x-", "origin"] {
        assert!(!names.iter().any(|n| n.starts_with(forbidden)));
    }
    assert!(raw.contains("User-Agent: Papyrine\r\n") || raw.contains("user-agent: Papyrine\r\n"));
}

#[test]
fn https_only_transport_refuses_plain_http() {
    let t = papyrine_update::UreqTransport::new();
    let r = papyrine_update::Transport::get(
        &t,
        &papyrine_update::Request {
            url: "http://127.0.0.1:1/updates.json".into(),
        },
    );
    assert!(r.is_err());
}
