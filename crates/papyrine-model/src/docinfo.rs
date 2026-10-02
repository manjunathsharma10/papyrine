//! Document-level views: Info dictionary, catalog summary, XMP, encryption and permissions.

use std::collections::BTreeMap;
use std::rc::Rc;

use papyrine_cos::{DecodeLevel, EncryptionInfo, ObjectKind, Result};

use crate::dest::{Action, Destination};
use crate::model::{Key, Model};
use crate::text::PdfDate;

/// The `/Info` dictionary.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DocumentInfo {
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub keywords: Option<String>,
    pub creator: Option<String>,
    pub producer: Option<String>,
    pub creation_date: Option<PdfDate>,
    pub creation_date_raw: Option<String>,
    pub mod_date: Option<PdfDate>,
    pub mod_date_raw: Option<String>,
    /// `True`, `False` or `Unknown`.
    pub trapped: Option<String>,
    /// Every other key with a text value.
    pub custom: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OpenAction {
    Destination(Destination),
    Action(Action),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ViewerPreferences {
    pub hide_toolbar: bool,
    pub hide_menubar: bool,
    pub hide_window_ui: bool,
    pub fit_window: bool,
    pub center_window: bool,
    pub display_doc_title: bool,
    /// `/Direction`: `L2R` or `R2L`.
    pub direction: Option<String>,
    pub print_scaling: Option<String>,
    pub duplex: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CatalogInfo {
    /// `%PDF-x.y` header version.
    pub header_version: String,
    /// Catalog `/Version`, which may override the header upwards.
    pub catalog_version: Option<String>,
    /// The greater of the two.
    pub version: String,
    pub is_linearized: bool,
    /// `/Extensions /ADBE /ExtensionLevel`.
    pub adobe_extension_level: Option<i64>,
    pub page_layout: Option<String>,
    pub page_mode: Option<String>,
    pub language: Option<String>,
    /// `/MarkInfo /Marked`.
    pub is_marked: bool,
    pub has_struct_tree: bool,
    pub has_xmp: bool,
    pub has_outlines: bool,
    pub has_page_labels: bool,
    pub has_acroform: bool,
    pub has_embedded_files: bool,
    pub has_optional_content: bool,
    pub has_names_dests: bool,
    pub open_action: Option<OpenAction>,
    /// Document-level triggers: `WC WS DS WP DP`.
    pub catalog_actions: Vec<(String, Action)>,
    pub viewer_preferences: ViewerPreferences,
}

/// The eight user-facing permission classes of the standard security handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PermissionSummary {
    pub print: bool,
    pub print_high_quality: bool,
    /// Copy / extract text and graphics.
    pub copy: bool,
    /// Extract for accessibility.
    pub accessibility: bool,
    pub modify_contents: bool,
    pub annotate: bool,
    pub fill_forms: bool,
    /// Insert, delete and rotate pages; bookmarks and thumbnails.
    pub assemble: bool,
}

impl PermissionSummary {
    pub const ALL: PermissionSummary = PermissionSummary {
        print: true,
        print_high_quality: true,
        copy: true,
        accessibility: true,
        modify_contents: true,
        annotate: true,
        fill_forms: true,
        assemble: true,
    };

    /// Permissions as declared by the `/P` bits for a standard security handler of this
    /// revision, regardless of who opened the file. Revision 2 has no separate bits for page
    /// assembly, form filling or high-quality printing; they fall back to bits 4, 6 and 3.
    pub fn from_p(p: i32, revision: u8) -> PermissionSummary {
        let bit = |n: u32| p & (1 << (n - 1)) != 0;
        let r3 = revision >= 3;
        PermissionSummary {
            print: bit(3),
            print_high_quality: bit(3) && (!r3 || bit(12)),
            copy: bit(5),
            accessibility: if r3 { bit(10) } else { bit(5) },
            modify_contents: bit(4),
            annotate: bit(6),
            fill_forms: if r3 { bit(9) } else { bit(6) },
            assemble: if r3 { bit(11) } else { bit(4) },
        }
    }
}

#[derive(Debug, Clone)]
pub struct SecuritySummary {
    pub encrypted: bool,
    pub info: Option<EncryptionInfo>,
    /// Human-readable algorithm, e.g. `AES-256 (R6)`.
    pub algorithm: Option<String>,
    /// What the permissions grant to this session (everything when the owner password matched).
    pub effective: PermissionSummary,
    /// What `/P` declares for non-owners.
    pub declared: PermissionSummary,
    pub owner_authenticated: bool,
}

impl Model {
    /// The `/Info` dictionary (empty when absent).
    pub fn info(&self) -> Result<Rc<DocumentInfo>> {
        self.cached(Key::Info, |m| {
            let mut info = DocumentInfo::default();
            let Some(dict) = m.trailer().and_then(|t| m.get(&t, "Info")) else {
                return Ok(info);
            };
            for key in m.dict_keys(&dict) {
                let Some(v) = m.get(&dict, &key) else {
                    continue;
                };
                let text = match m.kind(&v) {
                    ObjectKind::String => m.text(&v),
                    ObjectKind::Name => m.name(&v),
                    _ => None,
                };
                let Some(text) = text else { continue };
                match key.as_str() {
                    "Title" => info.title = Some(text),
                    "Author" => info.author = Some(text),
                    "Subject" => info.subject = Some(text),
                    "Keywords" => info.keywords = Some(text),
                    "Creator" => info.creator = Some(text),
                    "Producer" => info.producer = Some(text),
                    "CreationDate" => {
                        info.creation_date = PdfDate::parse(&text);
                        info.creation_date_raw = Some(text);
                    }
                    "ModDate" => {
                        info.mod_date = PdfDate::parse(&text);
                        info.mod_date_raw = Some(text);
                    }
                    "Trapped" => info.trapped = Some(text),
                    _ => {
                        info.custom.insert(key, text);
                    }
                }
            }
            Ok(info)
        })
    }

    pub fn catalog_info(&self) -> Result<Rc<CatalogInfo>> {
        self.cached(Key::Catalog, |m| {
            let header_version = m.document().pdf_version().unwrap_or_default();
            let is_linearized = m.document().is_linearized().unwrap_or(false);
            let mut c = CatalogInfo {
                version: header_version.clone(),
                header_version,
                catalog_version: None,
                is_linearized,
                adobe_extension_level: None,
                page_layout: None,
                page_mode: None,
                language: None,
                is_marked: false,
                has_struct_tree: false,
                has_xmp: false,
                has_outlines: false,
                has_page_labels: false,
                has_acroform: false,
                has_embedded_files: false,
                has_optional_content: false,
                has_names_dests: false,
                open_action: None,
                catalog_actions: Vec::new(),
                viewer_preferences: ViewerPreferences::default(),
            };
            let Some(root) = m.root() else {
                return Ok(c);
            };
            c.catalog_version = m.name_of(&root, "Version");
            if let Some(cv) = &c.catalog_version
                && version_key(cv) > version_key(&c.header_version)
            {
                c.version = cv.clone();
            }
            c.adobe_extension_level = m
                .get(&root, "Extensions")
                .and_then(|e| m.get(&e, "ADBE"))
                .and_then(|a| m.int_of(&a, "ExtensionLevel"));
            c.page_layout = m.name_of(&root, "PageLayout");
            c.page_mode = m.name_of(&root, "PageMode");
            c.language = m.text_of(&root, "Lang");
            c.is_marked = m
                .get(&root, "MarkInfo")
                .and_then(|mi| m.bool_of(&mi, "Marked"))
                .unwrap_or(false);
            c.has_struct_tree = m.get(&root, "StructTreeRoot").is_some();
            c.has_xmp = m
                .get(&root, "Metadata")
                .is_some_and(|x| m.kind(&x) == ObjectKind::Stream);
            c.has_outlines = m
                .get(&root, "Outlines")
                .is_some_and(|o| m.get(&o, "First").is_some());
            c.has_page_labels = m.get(&root, "PageLabels").is_some();
            c.has_acroform = m.get(&root, "AcroForm").is_some_and(|a| m.is_dict(&a));
            c.has_optional_content = m.get(&root, "OCProperties").is_some();
            if let Some(names) = m.get(&root, "Names") {
                c.has_embedded_files = m.get(&names, "EmbeddedFiles").is_some();
                c.has_names_dests = m.get(&names, "Dests").is_some();
            }
            c.has_names_dests |= m.get(&root, "Dests").is_some();
            if let Some(oa) = m.get(&root, "OpenAction") {
                c.open_action = match m.kind(&oa) {
                    ObjectKind::Array => m.parse_destination(&oa).map(OpenAction::Destination),
                    ObjectKind::Dictionary => m.parse_action(&oa).map(OpenAction::Action),
                    _ => None,
                };
            }
            if let Some(aa) = m.get(&root, "AA") {
                for t in ["WC", "WS", "DS", "WP", "DP"] {
                    if let Some(a) = m.get(&aa, t).and_then(|a| m.parse_action(&a)) {
                        c.catalog_actions.push((t.to_string(), a));
                    }
                }
            }
            if let Some(vp) = m.get(&root, "ViewerPreferences") {
                let flag = |k: &str| m.bool_of(&vp, k).unwrap_or(false);
                c.viewer_preferences = ViewerPreferences {
                    hide_toolbar: flag("HideToolbar"),
                    hide_menubar: flag("HideMenubar"),
                    hide_window_ui: flag("HideWindowUI"),
                    fit_window: flag("FitWindow"),
                    center_window: flag("CenterWindow"),
                    display_doc_title: flag("DisplayDocTitle"),
                    direction: m.name_of(&vp, "Direction"),
                    print_scaling: m.name_of(&vp, "PrintScaling"),
                    duplex: m.name_of(&vp, "Duplex"),
                };
            }
            Ok(c)
        })
    }

    /// The decoded XMP packet from the catalog's `/Metadata`, if any.
    pub fn xmp_packet(&self) -> Result<Option<Vec<u8>>> {
        let Some(root) = self.root() else {
            return Ok(None);
        };
        let Some(md) = self.get(&root, "Metadata") else {
            return Ok(None);
        };
        if self.kind(&md) != ObjectKind::Stream {
            return Ok(None);
        }
        Ok(md
            .stream_decoded(DecodeLevel::Generalized)
            .ok()
            .map(|b| b.to_vec()))
    }

    /// Encryption algorithm and permissions. Not cached: it is a single FFI call.
    pub fn security(&self) -> Result<SecuritySummary> {
        let info = self.document().encryption()?;
        Ok(match info {
            None => SecuritySummary {
                encrypted: false,
                info: None,
                algorithm: None,
                effective: PermissionSummary::ALL,
                declared: PermissionSummary::ALL,
                owner_authenticated: true,
            },
            Some(e) => {
                let algorithm = describe_algorithm(&e);
                SecuritySummary {
                    encrypted: true,
                    algorithm: Some(algorithm),
                    effective: if e.owner_password_matched {
                        PermissionSummary::ALL
                    } else {
                        PermissionSummary {
                            print: e.allow_print_low_res,
                            print_high_quality: e.allow_print_high_res,
                            copy: e.allow_extract_all,
                            accessibility: e.allow_accessibility,
                            modify_contents: e.allow_modify_other,
                            annotate: e.allow_modify_annotation,
                            fill_forms: e.allow_modify_form,
                            assemble: e.allow_modify_assembly,
                        }
                    },
                    declared: PermissionSummary::from_p(e.permissions, e.revision),
                    owner_authenticated: e.owner_password_matched,
                    info: Some(e),
                }
            }
        })
    }
}

fn describe_algorithm(e: &EncryptionInfo) -> String {
    use papyrine_cos::CryptMethod::*;
    let m = match e.stream_method {
        Rc4 => format!("RC4-{}", if e.version <= 1 { 40 } else { 128 }),
        Aes128 => "AES-128".into(),
        Aes256 => "AES-256".into(),
        None => "none".into(),
        Unknown => "unknown".into(),
    };
    format!("{m} (R{})", e.revision)
}

fn version_key(v: &str) -> (u32, u32) {
    let mut it = v.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    (it.next().unwrap_or(0), it.next().unwrap_or(0))
}
