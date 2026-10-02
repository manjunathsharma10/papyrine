//! Encrypted variants. Encryption is delegated to the `qpdf` CLI at test-data generation time
//! only (never linked into shipped code). Output is not byte-reproducible: qpdf draws random
//! salts and file keys; passwords and security-handler parameters are fixed and recorded.

use crate::doc::*;
use crate::pdf::Out;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::process::Command;

pub struct Variant {
    pub name: &'static str,
    pub desc: &'static str,
    pub user: &'static str,
    pub owner: &'static str,
    /// Arguments placed between the passwords and the closing `--`.
    pub spec: &'static [&'static str],
    pub extra: &'static [&'static str],
}

pub fn variants() -> Vec<Variant> {
    let v = |name, desc, user, owner, spec, extra| Variant {
        name,
        desc,
        user,
        owner,
        spec,
        extra,
    };
    vec![
        v(
            "enc-rc4-40-user",
            "RC4 40-bit (V1/R2), user password required",
            "user",
            "owner",
            &["40"],
            &["--allow-weak-crypto"],
        ),
        v(
            "enc-rc4-128-user",
            "RC4 128-bit (V2/R3), user password required",
            "user",
            "owner",
            &["128", "--use-aes=n"],
            &["--allow-weak-crypto"],
        ),
        v(
            "enc-aes-128-user",
            "AES-128 (V4/R4), user password required",
            "user",
            "owner",
            &["128", "--use-aes=y"],
            &[],
        ),
        v(
            "enc-aes-256-r5-user",
            "AES-256 (V5/R5, Adobe extension level 3), user password required",
            "user",
            "owner",
            &["256", "--force-R5"],
            &[],
        ),
        v(
            "enc-aes-256-r6-user",
            "AES-256 (V5/R6, ISO 32000-2), user password required",
            "user",
            "owner",
            &["256"],
            &[],
        ),
        v(
            "enc-rc4-40-owner-only",
            "RC4 40-bit, empty user password, owner password restricts print/modify/extract",
            "",
            "owner",
            &[
                "40",
                "--print=n",
                "--modify=n",
                "--extract=n",
                "--annotate=n",
            ],
            &["--allow-weak-crypto"],
        ),
        v(
            "enc-rc4-128-owner-only",
            "RC4 128-bit, empty user password, owner restrictions",
            "",
            "owner",
            &[
                "128",
                "--use-aes=n",
                "--print=none",
                "--modify=none",
                "--extract=n",
            ],
            &["--allow-weak-crypto"],
        ),
        v(
            "enc-aes-128-owner-only",
            "AES-128, empty user password, owner restrictions",
            "",
            "owner",
            &[
                "128",
                "--use-aes=y",
                "--print=none",
                "--modify=none",
                "--extract=n",
            ],
            &[],
        ),
        v(
            "enc-aes-256-r5-owner-only",
            "AES-256 R5, empty user password, owner restrictions",
            "",
            "owner",
            &[
                "256",
                "--force-R5",
                "--print=none",
                "--modify=none",
                "--extract=n",
            ],
            &[],
        ),
        v(
            "enc-aes-256-r6-owner-only",
            "AES-256 R6, empty user password, owner restrictions",
            "",
            "owner",
            &["256", "--print=none", "--modify=none", "--extract=n"],
            &[],
        ),
        v(
            "enc-aes-128-cleartext-metadata",
            "AES-128 with /EncryptMetadata false (XMP stays readable)",
            "user",
            "owner",
            &["128", "--use-aes=y", "--cleartext-metadata"],
            &[],
        ),
        v(
            "enc-aes-256-r6-cleartext-metadata",
            "AES-256 R6 with unencrypted XMP metadata",
            "user",
            "owner",
            &["256", "--cleartext-metadata"],
            &[],
        ),
        v(
            "enc-aes-256-r6-unicode-password",
            "AES-256 R6, user password is the NFC string 'p\u{e4}ssw\u{f6}rd'",
            "p\u{e4}ssw\u{f6}rd",
            "\u{f6}wner",
            &["256"],
            &[],
        ),
        v(
            "enc-aes-256-r6-objstm",
            "AES-256 R6 with generated object streams and xref stream",
            "user",
            "owner",
            &["256"],
            &["--object-streams=generate"],
        ),
        v(
            "enc-rc4-128-same-passwords",
            "RC4 128-bit, user and owner passwords identical",
            "same",
            "same",
            &["128", "--use-aes=n"],
            &["--allow-weak-crypto"],
        ),
    ]
}

const XMP: &str = "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?><x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description rdf:about=\"\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\"><dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">Encryption source</rdf:li></rdf:Alt></dc:title></rdf:Description></rdf:RDF></x:xmpmeta><?xpacket end=\"w\"?>";

/// Five text pages with an XMP metadata stream: the plaintext source for every encrypted variant.
pub fn plain_source(path: &Path) -> io::Result<()> {
    let mut out = Out::new(BufWriter::new(std::fs::File::create(path)?));
    out.header("1.7")?;
    let kids: String = (0..5)
        .map(|i| format!("{} 0 R", 10 + i * 3))
        .collect::<Vec<_>>()
        .join(" ");
    out.obj(1, b"<< /Type /Catalog /Pages 2 0 R /Metadata 7 0 R >>")?;
    out.obj(
        2,
        format!("<< /Type /Pages /Kids [{kids}] /Count 5 >>").as_bytes(),
    )?;
    out.obj(
        3,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
    )?;
    out.obj(
        4,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>",
    )?;
    out.obj(
        5,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Courier /Encoding /WinAnsiEncoding >>",
    )?;
    out.obj(
        6,
        format!("<< /Title (Encryption source) /Author (Papyrine) /CreationDate ({INFO_DATE}) >>")
            .as_bytes(),
    )?;
    out.stream(7, "/Type /Metadata /Subtype /XML", XMP.as_bytes())?;
    for i in 0..5 {
        let c = format!(
            "BT /F2 20 Tf 72 720 Td (Encrypted document, page {}) Tj ET\nBT /F1 12 Tf 72 690 Td (The quick brown fox jumps over the lazy dog.) Tj ET\n",
            i + 1
        );
        write_page(
            &mut out,
            i,
            "0 0 612 792",
            &PageData {
                content: c.into_bytes(),
                image: None,
            },
            6,
        )?;
    }
    finish_classic(&mut out, 7)?;
    out.into_inner().flush()
}

pub fn qpdf_available() -> bool {
    Command::new("qpdf")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn encrypt(v: &Variant, src: &Path, dst: &Path) -> io::Result<()> {
    let mut cmd = Command::new("qpdf");
    cmd.args(v.extra)
        .arg("--static-id")
        .arg("--static-aes-iv")
        .arg("--encrypt")
        .arg(v.user)
        .arg(v.owner)
        .args(v.spec)
        .arg("--")
        .arg(src)
        .arg(dst);
    let o = cmd.output()?;
    // exit 3 = completed with warnings, which is acceptable
    if !matches!(o.status.code(), Some(0) | Some(3)) {
        return Err(io::Error::other(format!(
            "qpdf failed for {}: {}",
            v.name,
            String::from_utf8_lossy(&o.stderr)
        )));
    }
    Ok(())
}
