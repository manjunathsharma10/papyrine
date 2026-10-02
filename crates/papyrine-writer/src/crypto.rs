//! Per-object encryption for the standard security handler (R2-R6).
//!
//! The file key comes from qpdf (`Document::encryption_key`), so no password handling happens
//! here. R2-R4 derive a key per object (ISO 32000-1, 7.6.2 algorithm 1); R5/R6 use the 32-byte
//! file key directly with AES-256-CBC (ISO 32000-2, 7.6.3).

use aes::cipher::{BlockEncryptMut, KeyIvInit, block_padding::Pkcs7};
use md5::{Digest, Md5};
use papyrine_cos::{CryptMethod, Document, ObjId, ObjectKind, Secret};
use zeroize::Zeroizing;

use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Identity,
    Rc4,
    AesV2,
    AesV3,
}

impl Method {
    fn from_cos(m: CryptMethod) -> Result<Method> {
        Ok(match m {
            CryptMethod::None => Method::Identity,
            CryptMethod::Rc4 => Method::Rc4,
            CryptMethod::Aes128 => Method::AesV2,
            CryptMethod::Aes256 => Method::AesV3,
            CryptMethod::Unknown => return Err(Error::unsupported("unknown crypt filter method")),
        })
    }
}

/// Everything needed to encrypt objects the way the document's `/Encrypt` dictionary says.
pub struct Crypto {
    key: Zeroizing<Vec<u8>>,
    string: Method,
    stream: Method,
    encrypt_metadata: bool,
    has_eff: bool,
    /// The `/Encrypt` dictionary's own object is never encrypted.
    encrypt_obj: Option<ObjId>,
}

impl Crypto {
    /// `None` for unencrypted documents.
    pub fn from_document(doc: &Document) -> Result<Option<Crypto>> {
        let Some(info) = doc.encryption()? else {
            return Ok(None);
        };
        let key: Secret = doc.encryption_key()?.ok_or_else(|| {
            Error::unsupported("encrypted document without an available file key")
        })?;
        let enc = doc.trailer()?.dict_get("Encrypt")?;
        let (encrypt_metadata, has_eff) = if enc.kind()? == ObjectKind::Dictionary {
            let em = enc.dict_get("EncryptMetadata")?;
            (
                em.kind()? != ObjectKind::Bool || em.as_bool()?,
                enc.dict_has("EFF")?,
            )
        } else {
            (true, false)
        };
        Ok(Some(Crypto {
            key: Zeroizing::new(key.expose().to_vec()),
            string: Method::from_cos(info.string_method)?,
            stream: Method::from_cos(info.stream_method)?,
            encrypt_metadata,
            has_eff,
            encrypt_obj: enc.id(),
        }))
    }

    pub(crate) fn is_encrypt_dict(&self, id: ObjId) -> bool {
        self.encrypt_obj == Some(id)
    }

    pub(crate) fn has_eff(&self) -> bool {
        self.has_eff
    }

    pub(crate) fn encrypt_metadata(&self) -> bool {
        self.encrypt_metadata
    }

    pub(crate) fn string_method(&self) -> Method {
        self.string
    }

    pub(crate) fn stream_method(&self) -> Method {
        self.stream
    }

    /// Encrypt `data` for object `id` with `method`.
    pub fn encrypt(&self, method: Method, id: ObjId, data: &[u8]) -> Result<Vec<u8>> {
        match method {
            Method::Identity => Ok(data.to_vec()),
            Method::Rc4 => {
                let k = self.object_key(id, false);
                let mut v = data.to_vec();
                rc4(&k, &mut v);
                Ok(v)
            }
            Method::AesV2 => {
                let k = self.object_key(id, true);
                aes_cbc(&k, data)
            }
            Method::AesV3 => aes_cbc(&self.key, data),
        }
    }

    /// Algorithm 1: MD5(key || num[0..3] || gen[0..2] [|| "sAlT"]) truncated to min(n + 5, 16).
    fn object_key(&self, id: ObjId, aes: bool) -> Zeroizing<Vec<u8>> {
        let mut h = Md5::new();
        h.update(&self.key[..]);
        h.update(&id.num.to_le_bytes()[..3]);
        h.update(id.generation.to_le_bytes());
        if aes {
            h.update(b"sAlT");
        }
        let digest = h.finalize();
        let n = (self.key.len() + 5).min(16);
        Zeroizing::new(digest[..n].to_vec())
    }
}

/// RC4 (a 20-line cipher; the RustCrypto crate is generic over key length at compile time, which
/// does not fit 5..16-byte PDF keys).
fn rc4(key: &[u8], data: &mut [u8]) {
    let mut s: [u8; 256] = std::array::from_fn(|i| i as u8);
    let mut j = 0u8;
    for i in 0..256 {
        j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len()]);
        s.swap(i, j as usize);
    }
    let (mut i, mut j) = (0u8, 0u8);
    for b in data {
        i = i.wrapping_add(1);
        j = j.wrapping_add(s[i as usize]);
        s.swap(i as usize, j as usize);
        *b ^= s[s[i as usize].wrapping_add(s[j as usize]) as usize];
    }
}

/// AES-CBC with a random IV prepended and PKCS#7 padding, as PDF requires.
fn aes_cbc(key: &[u8], data: &[u8]) -> Result<Vec<u8>> {
    let mut iv = [0u8; 16];
    getrandom::fill(&mut iv).map_err(|e| Error::Io(std::io::Error::other(e.to_string())))?;
    let ct = match key.len() {
        16 => cbc::Encryptor::<aes::Aes128>::new(key.into(), (&iv).into())
            .encrypt_padded_vec_mut::<Pkcs7>(data),
        32 => cbc::Encryptor::<aes::Aes256>::new(key.into(), (&iv).into())
            .encrypt_padded_vec_mut::<Pkcs7>(data),
        n => return Err(Error::unsupported(format!("AES key of {n} bytes"))),
    };
    let mut out = Vec::with_capacity(16 + ct.len());
    out.extend_from_slice(&iv);
    out.extend_from_slice(&ct);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rc4_known_vector() {
        // RFC 6229-style classic vector: key "Key", plaintext "Plaintext".
        let mut d = *b"Plaintext";
        rc4(b"Key", &mut d);
        assert_eq!(d, [0xBB, 0xF3, 0x16, 0xE8, 0xD9, 0x40, 0xAF, 0x0A, 0xD3]);
    }

    #[test]
    fn aes_output_shape() {
        let out = aes_cbc(&[7u8; 16], b"hello").unwrap();
        assert_eq!(out.len(), 32);
        let out = aes_cbc(&[7u8; 32], &[0u8; 16]).unwrap();
        assert_eq!(out.len(), 48);
        assert!(aes_cbc(&[7u8; 10], b"x").is_err());
    }
}
