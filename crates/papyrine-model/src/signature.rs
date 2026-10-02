//! Signature fields and certification, read-only. Drives the save policy: a signed or certified
//! document must be saved incrementally or flagged before a full rewrite.

use std::rc::Rc;

use papyrine_core::geom::Rect;
use papyrine_cos::{ObjId, Object, Result};

use crate::form::FieldKind;
use crate::model::{Key, Model};
use crate::text::PdfDate;

#[derive(Debug, Clone)]
pub struct SignatureInfo {
    pub field: ObjId,
    pub field_name: String,
    /// The field has a signature value (`/V` with `/Contents` or `/ByteRange`).
    pub signed: bool,
    /// `/DocTimeStamp` rather than `/Sig`.
    pub is_timestamp: bool,
    pub filter: Option<String>,
    pub sub_filter: Option<String>,
    pub byte_range: Vec<i64>,
    /// Length in bytes of the (hex-decoded) `/Contents` placeholder.
    pub contents_len: Option<usize>,
    pub signer_name: Option<String>,
    pub reason: Option<String>,
    pub location: Option<String>,
    pub contact_info: Option<String>,
    pub signing_time: Option<PdfDate>,
    pub signing_time_raw: Option<String>,
    /// DocMDP level declared in this signature's `/Reference` (1, 2 or 3).
    pub docmdp_level: Option<u8>,
    pub page: Option<usize>,
    pub rect: Option<Rect>,
}

#[derive(Debug, Clone, Default)]
pub struct SignatureSummary {
    pub fields: Vec<SignatureInfo>,
    /// Catalog `/Perms /DocMDP` present: the document is certified.
    pub certified: bool,
    /// Permitted changes of the certifying signature: 1 none, 2 form fill and sign, 3 plus annotations.
    pub certification_level: Option<u8>,
    /// Catalog `/Perms /UR3` (usage rights, Reader extensions).
    pub has_usage_rights: bool,
    /// AcroForm `/SigFlags`.
    pub sig_flags: u32,
}

impl SignatureSummary {
    pub fn signed_count(&self) -> usize {
        self.fields.iter().filter(|f| f.signed).count()
    }
    pub fn unsigned_count(&self) -> usize {
        self.fields.len() - self.signed_count()
    }
    /// Is there at least one applied signature or a certification?
    pub fn is_signed(&self) -> bool {
        self.signed_count() > 0 || self.certified
    }
    /// A full rewrite (renumbering, object streams, recompression) breaks signature byte ranges,
    /// so saves of such documents must be incremental, or the user must be warned.
    pub fn requires_incremental_save(&self) -> bool {
        self.is_signed() || self.sig_flags & 2 != 0
    }
    /// Certified with level 1: no changes are permitted at all.
    pub fn forbids_changes(&self) -> bool {
        self.certification_level == Some(1)
    }
}

impl Model {
    pub fn signatures(&self) -> Result<Rc<SignatureSummary>> {
        self.cached(Key::Signatures, |m| {
            let form = m.form()?;
            let mut s = SignatureSummary {
                sig_flags: form.sig_flags,
                ..Default::default()
            };
            for f in form.fields.iter() {
                if f.kind != FieldKind::Signature || !f.is_terminal {
                    continue;
                }
                let Some(fobj) = m.document().object(f.id).ok() else {
                    continue;
                };
                let value = m.get(&fobj, "V").filter(|v| m.is_dict(v));
                let mut info = SignatureInfo {
                    field: f.id,
                    field_name: f.qualified_name.clone(),
                    signed: false,
                    is_timestamp: false,
                    filter: None,
                    sub_filter: None,
                    byte_range: Vec::new(),
                    contents_len: None,
                    signer_name: None,
                    reason: None,
                    location: None,
                    contact_info: None,
                    signing_time: None,
                    signing_time_raw: None,
                    docmdp_level: None,
                    page: f.widgets.first().and_then(|w| w.page),
                    rect: f.widgets.first().map(|w| w.rect),
                };
                if let Some(v) = value {
                    m.fill_signature(&v, &mut info);
                }
                s.fields.push(info);
            }
            if let Some(root) = m.root()
                && let Some(perms) = m.get(&root, "Perms")
            {
                s.has_usage_rights =
                    m.get(&perms, "UR3").is_some() || m.get(&perms, "UR").is_some();
                if let Some(dm) = m.get(&perms, "DocMDP") {
                    s.certified = true;
                    s.certification_level = Some(m.docmdp_level(&dm).unwrap_or(2));
                }
            }
            Ok(s)
        })
    }

    fn fill_signature(&self, v: &Object, info: &mut SignatureInfo) {
        info.is_timestamp = self.name_of(v, "Type").as_deref() == Some("DocTimeStamp");
        info.filter = self.name_of(v, "Filter");
        info.sub_filter = self.name_of(v, "SubFilter");
        info.byte_range = self
            .items_of(v, "ByteRange")
            .iter()
            .filter_map(|e| self.int(e))
            .collect();
        let contents = self.get(v, "Contents").and_then(|c| self.bytes(&c));
        info.contents_len = contents.as_ref().map(Vec::len);
        info.signed = contents.is_some() || !info.byte_range.is_empty();
        info.signer_name = self.text_of(v, "Name");
        info.reason = self.text_of(v, "Reason");
        info.location = self.text_of(v, "Location");
        info.contact_info = self.text_of(v, "ContactInfo");
        info.signing_time_raw = self.text_of(v, "M");
        info.signing_time = info.signing_time_raw.as_deref().and_then(PdfDate::parse);
        info.docmdp_level = self.docmdp_level(v).map(|l| l.clamp(1, 3));
    }

    /// `P` of the DocMDP transform in a signature's `/Reference` array.
    fn docmdp_level(&self, sig: &Object) -> Option<u8> {
        for r in self.items_of(sig, "Reference") {
            if self.name_of(&r, "TransformMethod").as_deref() == Some("DocMDP") {
                let p = self
                    .get(&r, "TransformParams")
                    .and_then(|tp| self.int_of(&tp, "P"))
                    .unwrap_or(2);
                return Some(p.clamp(1, 3) as u8);
            }
        }
        None
    }
}
