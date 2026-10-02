use qpdf_sys::EncryptionInfo as Raw;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CryptMethod {
    None,
    Unknown,
    Rc4,
    Aes128,
    Aes256,
}

impl CryptMethod {
    fn from_code(c: i32) -> Self {
        match c {
            0 => CryptMethod::None,
            2 => CryptMethod::Rc4,
            3 => CryptMethod::Aes128,
            4 => CryptMethod::Aes256,
            _ => CryptMethod::Unknown,
        }
    }
}

/// What the document's encryption dictionary says, and what the supplied password grants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptionInfo {
    /// Standard security handler revision `/R` (2..=6).
    pub revision: u8,
    /// Algorithm version `/V`.
    pub version: u8,
    /// Raw `/P` permission bits.
    pub permissions: i32,
    pub stream_method: CryptMethod,
    pub string_method: CryptMethod,
    pub file_method: CryptMethod,
    pub user_password_matched: bool,
    pub owner_password_matched: bool,
    pub allow_accessibility: bool,
    pub allow_extract_all: bool,
    pub allow_print_low_res: bool,
    pub allow_print_high_res: bool,
    pub allow_modify_assembly: bool,
    pub allow_modify_form: bool,
    pub allow_modify_annotation: bool,
    pub allow_modify_other: bool,
    pub allow_modify_all: bool,
}

impl EncryptionInfo {
    pub(crate) fn from_raw(r: &Raw) -> Option<Self> {
        // qpdf leaves the crypt methods unset for V1-V3, which are always RC4.
        let method = |c: i32| match CryptMethod::from_code(c) {
            CryptMethod::None if r.v <= 3 => CryptMethod::Rc4,
            m => m,
        };
        r.encrypted.then(|| EncryptionInfo {
            revision: r.r as u8,
            version: r.v as u8,
            permissions: r.p,
            stream_method: method(r.stream_method),
            string_method: method(r.string_method),
            file_method: method(r.file_method),
            user_password_matched: r.user_password_matched,
            owner_password_matched: r.owner_password_matched,
            allow_accessibility: r.allow_accessibility,
            allow_extract_all: r.allow_extract_all,
            allow_print_low_res: r.allow_print_low_res,
            allow_print_high_res: r.allow_print_high_res,
            allow_modify_assembly: r.allow_modify_assembly,
            allow_modify_form: r.allow_modify_form,
            allow_modify_annotation: r.allow_modify_annotation,
            allow_modify_other: r.allow_modify_other,
            allow_modify_all: r.allow_modify_all,
        })
    }
}
