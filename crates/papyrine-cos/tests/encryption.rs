mod common;

use common::*;
use papyrine_cos::*;

struct Case {
    name: &'static str,
    rev: EncryptionRevision,
    r: u8,
    v: u8,
    method: CryptMethod,
    key_len: usize,
}

fn cases() -> Vec<Case> {
    use CryptMethod::*;
    use EncryptionRevision::*;
    vec![
        Case {
            name: "R2",
            rev: R2,
            r: 2,
            v: 1,
            method: Rc4,
            key_len: 5,
        },
        Case {
            name: "R3",
            rev: R3,
            r: 3,
            v: 2,
            method: Rc4,
            key_len: 16,
        },
        Case {
            name: "R4-RC4",
            rev: R4 { aes: false },
            r: 4,
            v: 4,
            method: Rc4,
            key_len: 16,
        },
        Case {
            name: "R4-AES",
            rev: R4 { aes: true },
            r: 4,
            v: 4,
            method: Aes128,
            key_len: 16,
        },
        Case {
            name: "R5",
            rev: R5,
            r: 5,
            v: 5,
            method: Aes256,
            key_len: 32,
        },
        Case {
            name: "R6",
            rev: R6,
            r: 6,
            v: 5,
            method: Aes256,
            key_len: 32,
        },
    ]
}

#[allow(clippy::unnecessary_to_owned)]
fn open_with(data: &[u8], pw: Option<&str>) -> Result<Document> {
    let opts = match pw {
        Some(p) => OpenOptions::with_password(p),
        None => OpenOptions::default(),
    };
    Document::open_bytes(data.to_vec(), &opts)
}

#[test]
fn every_revision_opens_with_right_password_and_refuses_wrong() {
    for c in cases() {
        let spec = EncryptionSpec::new(c.rev, "user-pw", "owner-pw");
        let data = encrypted_pdf(spec);
        assert!(
            find(&data, b"/Encrypt").is_some(),
            "{}: not encrypted",
            c.name
        );
        assert!(
            find(&data, b"Page 1").is_none(),
            "{}: plaintext leaked",
            c.name
        );

        // Correct passwords.
        for (pw, owner) in [("user-pw", false), ("owner-pw", true)] {
            let doc = open_with(&data, Some(pw)).unwrap_or_else(|e| panic!("{} {pw}: {e}", c.name));
            let info = doc.encryption().unwrap().expect("encrypted");
            assert_eq!(info.revision, c.r, "{}", c.name);
            assert_eq!(info.version, c.v, "{}", c.name);
            assert_eq!(info.stream_method, c.method, "{}", c.name);
            assert_eq!(info.string_method, c.method, "{}", c.name);
            assert_eq!(info.owner_password_matched, owner, "{} {pw}", c.name);
            assert_eq!(info.user_password_matched, !owner, "{} {pw}", c.name);
            let key = doc.encryption_key().unwrap().expect("key");
            assert_eq!(key.len(), c.key_len, "{}", c.name);
            assert_eq!(doc.page_count().unwrap(), 2, "{}", c.name);
            assert_eq!(
                decoded_content(&doc, 0),
                content_for(1).into_bytes(),
                "{}",
                c.name
            );
            assert_eq!(
                decoded_content(&doc, 1),
                content_for(2).into_bytes(),
                "{}",
                c.name
            );
            // Strings are decrypted too.
            let title = doc
                .trailer()
                .unwrap()
                .dict_get("Info")
                .unwrap()
                .dict_get("Title")
                .unwrap();
            assert_eq!(title.string().unwrap(), b"Test document", "{}", c.name);
        }

        // Wrong / missing passwords are refused with a typed error.
        for pw in [
            Some("nope"),
            Some("user-pw "),
            Some("OWNER-PW"),
            Some(""),
            None,
        ] {
            match open_with(&data, pw) {
                Err(Error::InvalidPassword) => {}
                Err(e) => panic!("{} {pw:?}: wrong error {e:?}", c.name),
                Ok(_) => panic!("{} {pw:?}: opened with wrong password", c.name),
            }
        }
    }
}

#[test]
fn empty_user_password_opens_without_one() {
    for c in cases() {
        let data = encrypted_pdf(EncryptionSpec::new(c.rev, "", "owner-pw"));
        let doc = open_with(&data, None).unwrap_or_else(|e| panic!("{}: {e}", c.name));
        assert!(
            doc.encryption().unwrap().unwrap().user_password_matched,
            "{}",
            c.name
        );
        assert_eq!(doc.page_count().unwrap(), 2);
        assert!(open_with(&data, Some("wrong")).is_err());
        let owner = open_with(&data, Some("owner-pw")).unwrap();
        assert!(owner.encryption().unwrap().unwrap().owner_password_matched);
    }
}

#[test]
fn permissions_are_reported() {
    let mut spec = EncryptionSpec::new(EncryptionRevision::R6, "u", "o");
    spec.permissions = Permissions {
        accessibility: false,
        extract: false,
        assemble: false,
        annotate_and_form: false,
        form_filling: false,
        modify_other: false,
        print: PrintPermission::None,
    };
    let data = encrypted_pdf(spec);
    let info = open_with(&data, Some("u"))
        .unwrap()
        .encryption()
        .unwrap()
        .unwrap();
    assert!(!info.allow_print_low_res && !info.allow_print_high_res);
    assert!(!info.allow_extract_all && !info.allow_modify_all && !info.allow_modify_other);
    assert!(
        !info.allow_modify_annotation && !info.allow_modify_form && !info.allow_modify_assembly
    );
    // Permissions describe /P; the owner password is reported separately (qpdf does not enforce).
    let owner = open_with(&data, Some("o"))
        .unwrap()
        .encryption()
        .unwrap()
        .unwrap();
    assert!(owner.owner_password_matched);
    assert_eq!(owner.permissions, info.permissions);

    let mut low = EncryptionSpec::new(EncryptionRevision::R3, "u", "o");
    low.permissions.print = PrintPermission::LowRes;
    let info = open_with(&encrypted_pdf(low), Some("u"))
        .unwrap()
        .encryption()
        .unwrap()
        .unwrap();
    assert!(info.allow_print_low_res && !info.allow_print_high_res);
}

#[test]
fn preserve_and_decrypt_on_rewrite() {
    let data = encrypted_pdf(EncryptionSpec::new(EncryptionRevision::R6, "u", "o"));
    let doc = open_with(&data, Some("u")).unwrap();

    let kept = doc
        .write(&WriteOptions {
            static_id: true,
            ..Default::default()
        })
        .unwrap()
        .into_vec();
    assert!(find(&kept, b"/Encrypt").is_some());
    assert!(open_with(&kept, None).is_err());
    let again = open_with(&kept, Some("u")).unwrap();
    assert_eq!(again.encryption().unwrap().unwrap().revision, 6);
    assert_eq!(decoded_content(&again, 0), content_for(1).into_bytes());

    let plain = doc
        .write(&WriteOptions {
            encryption: EncryptionMode::Decrypt,
            ..Default::default()
        })
        .unwrap()
        .into_vec();
    assert!(find(&plain, b"/Encrypt").is_none());
    let d = open_with(&plain, None).unwrap();
    assert!(d.encryption().unwrap().is_none());
    assert_eq!(decoded_content(&d, 1), content_for(2).into_bytes());

    // Re-encrypt with a different revision.
    let re = doc
        .write(&WriteOptions {
            encryption: EncryptionMode::Encrypt(EncryptionSpec::new(
                EncryptionRevision::R4 { aes: true },
                "x",
                "y",
            )),
            ..Default::default()
        })
        .unwrap()
        .into_vec();
    assert!(open_with(&re, Some("u")).is_err());
    assert_eq!(
        open_with(&re, Some("x"))
            .unwrap()
            .encryption()
            .unwrap()
            .unwrap()
            .revision,
        4
    );
}

#[test]
fn unencrypted_metadata_option() {
    let mut spec = EncryptionSpec::new(EncryptionRevision::R4 { aes: true }, "u", "o");
    spec.encrypt_metadata = false;
    let data = encrypted_pdf(spec);
    assert_eq!(
        open_with(&data, Some("u")).unwrap().page_count().unwrap(),
        2
    );
    assert!(find(&data, b"/EncryptMetadata false").is_some());
}

#[test]
fn non_utf8_password_bytes() {
    let spec = EncryptionSpec::new(
        EncryptionRevision::R6,
        Secret::new(vec![0xE9, 0xFF, 0x80]),
        "o",
    );
    let data = encrypted_pdf(spec);
    let ok = Document::open_bytes(
        data.clone(),
        &OpenOptions {
            password: Some(Secret::new(vec![0xE9, 0xFF, 0x80])),
            ..Default::default()
        },
    );
    assert!(ok.is_ok(), "{:?}", ok.err());
    assert!(open_with(&data, Some("é")).is_err());
}

#[test]
fn secret_is_redacted() {
    let s = Secret::from("hunter2");
    assert_eq!(format!("{s:?}"), "Secret(***)");
    let opts = OpenOptions::with_password("hunter2");
    assert!(!format!("{opts:?}").contains("hunter2"));
    let spec = EncryptionSpec::new(EncryptionRevision::R6, "hunter2", "o");
    assert!(!format!("{spec:?}").contains("hunter2"));
}
