//! Low-level bindings to the vendored qpdf (native crypto only) through a cxx-bridged C++ shim.
//!
//! This crate is deliberately thin and unsafe-adjacent: use `papyrine-cos` for the safe API.
//! Every fallible function returns `Result<_, cxx::Exception>` whose message has the form
//! `"<code>|<text>"`: `code` is a `qpdf_error_code_e` (1 internal, 2 system, 3 unsupported,
//! 4 password, 5 damaged pdf, 6 pages, 7 object, 8 json) or 100/101/102 for non-qpdf failures.

#![allow(clippy::too_many_arguments, clippy::missing_safety_doc)]

#[cxx::bridge(namespace = "papyrine")]
pub mod ffi {
    /// Byte string wrapper so we can return `Vec<Bytes>` (names and strings are not always UTF-8).
    pub struct Bytes {
        pub v: Vec<u8>,
    }

    pub struct ObjGenPair {
        pub id: i32,
        pub gen_: i32,
    }

    /// One repair or warning emitted by qpdf (the raw material of the RepairLog).
    pub struct RepairEntry {
        pub message: String,
        pub filename: String,
        /// qpdf's textual object locator ("object 12 0", "trailer", "xref table", ...), may be empty.
        pub object: String,
        pub object_id: i32,
        pub object_gen: i32,
        pub offset: i64,
        pub code: i32,
    }

    pub struct OpenOptions {
        pub password: Vec<u8>,
        pub attempt_recovery: bool,
        pub ignore_xref_streams: bool,
        pub max_warnings: usize,
    }

    pub struct EncryptionInfo {
        pub encrypted: bool,
        pub r: i32,
        pub p: i32,
        pub v: i32,
        /// 0 none, 1 unknown, 2 RC4, 3 AES-128, 4 AES-256
        pub stream_method: i32,
        pub string_method: i32,
        pub file_method: i32,
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

    pub struct EncryptionParams {
        /// 0 = none (unless `preserve_encryption`), 2..=6 = encryption revision to write.
        pub r: i32,
        pub aes: bool,
        pub encrypt_metadata: bool,
        pub user_password: Vec<u8>,
        pub owner_password: Vec<u8>,
        pub allow_accessibility: bool,
        pub allow_extract: bool,
        pub allow_assemble: bool,
        pub allow_annotate_and_form: bool,
        pub allow_form_filling: bool,
        pub allow_modify_other: bool,
        /// 0 none, 1 low resolution, 2 full
        pub print: i32,
    }

    pub struct WriteOptions {
        /// 0 disable, 1 preserve, 2 generate
        pub object_streams: i32,
        /// 0 uncompress, 1 preserve, 2 compress
        pub stream_data: i32,
        pub recompress_flate: bool,
        pub linearize: bool,
        pub static_id: bool,
        pub deterministic_id: bool,
        pub qdf: bool,
        pub normalize_content: bool,
        pub preserve_unreferenced: bool,
        pub newline_before_endstream: bool,
        pub preserve_encryption: bool,
        pub min_version: String,
        pub encryption: EncryptionParams,
    }

    pub struct Renumber {
        pub old_id: i32,
        pub old_gen: i32,
        pub new_id: i32,
        pub new_gen: i32,
    }

    pub struct Fingerprint {
        /// Direct representation of the object (dictionary or scalar), references left as "n g R".
        pub repr: Vec<u8>,
        pub is_stream: bool,
        pub stream_raw_len: u64,
        /// SHA-256 of the raw (still encoded) stream bytes; empty for non-streams.
        pub stream_sha256: Vec<u8>,
    }

    pub struct WidgetInfo {
        /// Object number of the widget annotation (0 when the annotation is a direct object).
        pub id: i32,
        pub gen_: i32,
        /// Zero-based page index holding the widget, or -1 when the widget is on no page.
        pub page: i32,
        pub x0: f64,
        pub y0: f64,
        pub x1: f64,
        pub y1: f64,
    }

    pub struct FieldInfo {
        pub id: i32,
        pub gen_: i32,
        /// 0 other, 1 text, 2 checkbox, 3 radio button, 4 push button, 5 choice, 6 signature
        pub kind: i32,
        pub flags: i32,
        pub quadding: i32,
        /// Inherited `/MaxLen`, or -1.
        pub max_len: i32,
        pub checked: bool,
        /// `/Tx`, `/Btn`, `/Ch`, `/Sig` (inherited), empty if absent.
        pub field_type: Vec<u8>,
        /// Fully qualified name, UTF-8.
        pub name: Vec<u8>,
        pub partial_name: Vec<u8>,
        pub alt_name: Vec<u8>,
        /// Current value as UTF-8 (for buttons: the state name without its slash).
        pub value: Vec<u8>,
        pub default_value: Vec<u8>,
        pub default_appearance: Vec<u8>,
        /// Display values of a choice field.
        pub choices: Vec<Bytes>,
        /// Appearance states other than `Off` found in the widgets' `/AP /N` (buttons).
        pub states: Vec<Bytes>,
        pub widgets: Vec<WidgetInfo>,
    }

    pub struct OutlineItem {
        pub id: i32,
        pub gen_: i32,
        /// 0 for top-level entries.
        pub depth: i32,
        /// Destination page index, or -1 (no destination, or it is not a page of this document).
        pub page: i32,
        /// `/Count`: negative when closed.
        pub count: i32,
        pub title: Vec<u8>,
        /// `/A << /S /URI /URI (..) >>` target, if any.
        pub uri: Vec<u8>,
        /// Named destination (name or string) when the destination is given by name.
        pub dest_name: Vec<u8>,
    }

    pub struct LabelRange {
        pub start_page: i32,
        /// 0 none, 1 decimal, 2 lower alpha, 3 upper alpha, 4 lower roman, 5 upper roman
        pub style: i32,
        pub prefix: Vec<u8>,
        pub first_value: i32,
    }

    pub struct EmbeddedFileInfo {
        /// Key in the `/EmbeddedFiles` name tree.
        pub name: Vec<u8>,
        pub filename: Vec<u8>,
        pub description: Vec<u8>,
        /// MIME type from the stream `/Subtype` (without the leading slash), if any.
        pub mime: Vec<u8>,
        /// `/Params /Size`, or -1.
        pub size: i64,
        pub created: Vec<u8>,
        pub modified: Vec<u8>,
        /// Raw MD5 checksum bytes from `/Params /CheckSum`, if any.
        pub checksum: Vec<u8>,
        pub filespec_id: i32,
        pub filespec_gen: i32,
        /// Embedded file stream (0 when the file specification has none).
        pub stream_id: i32,
        pub stream_gen: i32,
    }

    extern "Rust" {
        /// Lazy stream data supplier (see `Obj` stream providers).
        type ProviderBox;
        fn provider_produce(p: &ProviderBox) -> Result<Vec<u8>>;
        /// Write progress sink; returns false to cancel the write.
        type ProgressBox;
        fn progress_report(p: &mut ProgressBox, percent: i32) -> bool;
    }

    unsafe extern "C++" {
        include!("papyrine_shim.h");

        type Doc;
        type Obj;
        type Buf;
        type WriteOut;

        fn doc_new() -> UniquePtr<Doc>;
        /// # Safety
        /// `data..data+len` must stay valid and unmodified until the `Doc` is dropped.
        unsafe fn doc_open_slice(
            d: &Doc,
            description: &str,
            data: *const u8,
            len: usize,
            opts: &OpenOptions,
        ) -> Result<()>;
        fn doc_open_file(d: &Doc, path: &[u8], opts: &OpenOptions) -> Result<()>;
        fn doc_new_empty(d: &Doc) -> Result<()>;
        fn doc_take_warnings(d: &Doc) -> Result<Vec<RepairEntry>>;
        fn doc_version(d: &Doc) -> Result<String>;
        fn qpdf_version() -> String;
        fn doc_is_linearized(d: &Doc) -> Result<bool>;
        fn doc_encryption_info(d: &Doc) -> Result<EncryptionInfo>;
        fn doc_encryption_key(d: &Doc) -> Result<UniquePtr<Buf>>;
        fn doc_trailer(d: &Doc) -> Result<UniquePtr<Obj>>;
        fn doc_root(d: &Doc) -> Result<UniquePtr<Obj>>;
        fn doc_object_count(d: &Doc) -> Result<usize>;
        fn doc_all_objects(d: &Doc) -> Result<Vec<ObjGenPair>>;
        fn doc_get_object(d: &Doc, id: i32, gen_: i32) -> Result<UniquePtr<Obj>>;
        fn doc_make_indirect(d: &Doc, o: &Obj) -> Result<UniquePtr<Obj>>;
        fn doc_replace_object(d: &Doc, id: i32, gen_: i32, o: &Obj) -> Result<()>;
        fn doc_copy_foreign(d: &Doc, foreign: &Obj) -> Result<UniquePtr<Obj>>;
        fn doc_new_stream(d: &Doc, data: &[u8]) -> Result<UniquePtr<Obj>>;
        fn doc_pages_count(d: &Doc) -> Result<usize>;
        fn doc_page(d: &Doc, index: usize) -> Result<UniquePtr<Obj>>;
        fn doc_add_page(d: &Doc, page: &Obj, first: bool) -> Result<()>;
        fn doc_add_page_at(d: &Doc, page: &Obj, before: bool, reference: &Obj) -> Result<()>;
        fn doc_remove_page(d: &Doc, page: &Obj) -> Result<()>;
        fn doc_find_page(d: &Doc, page: &Obj) -> Result<i32>;
        fn doc_push_inherited(d: &Doc) -> Result<()>;
        /// Empty `path` writes to memory, otherwise to the file at `path`.
        fn doc_write(
            d: &Doc,
            opts: &WriteOptions,
            path: &[u8],
            progress: &mut ProgressBox,
        ) -> Result<UniquePtr<WriteOut>>;
        fn write_out_data(w: &WriteOut) -> &[u8];
        fn write_out_renumber(w: &WriteOut) -> Result<Vec<Renumber>>;

        fn crypto_impls() -> Result<Vec<String>>;
        fn crypto_default() -> Result<String>;

        fn obj_clone(o: &Obj) -> UniquePtr<Obj>;
        fn obj_type(o: &Obj) -> Result<i32>;
        fn obj_is_indirect(o: &Obj) -> bool;
        fn obj_id(o: &Obj) -> i32;
        fn obj_gen(o: &Obj) -> i32;
        fn obj_get_bool(o: &Obj) -> Result<bool>;
        fn obj_get_int(o: &Obj) -> Result<i64>;
        fn obj_get_numeric(o: &Obj) -> Result<f64>;
        fn obj_get_real_text(o: &Obj) -> Result<String>;
        fn obj_get_name(o: &Obj) -> Result<Vec<u8>>;
        fn obj_get_string(o: &Obj) -> Result<Vec<u8>>;
        fn obj_unparse(o: &Obj, resolved: bool) -> Result<Vec<u8>>;
        fn obj_fingerprint(o: &Obj) -> Result<Fingerprint>;

        fn obj_new_null() -> UniquePtr<Obj>;
        fn obj_new_bool(v: bool) -> UniquePtr<Obj>;
        fn obj_new_int(v: i64) -> UniquePtr<Obj>;
        fn obj_new_real(v: f64) -> Result<UniquePtr<Obj>>;
        fn obj_new_name(v: &[u8]) -> Result<UniquePtr<Obj>>;
        fn obj_new_string(v: &[u8]) -> Result<UniquePtr<Obj>>;
        fn obj_new_array() -> UniquePtr<Obj>;
        fn obj_new_dict() -> UniquePtr<Obj>;
        fn obj_parse(text: &[u8]) -> Result<UniquePtr<Obj>>;

        fn array_len(o: &Obj) -> Result<i32>;
        fn array_get(o: &Obj, i: i32) -> Result<UniquePtr<Obj>>;
        fn array_set(o: &Obj, i: i32, v: &Obj) -> Result<()>;
        fn array_append(o: &Obj, v: &Obj) -> Result<()>;
        fn array_insert(o: &Obj, i: i32, v: &Obj) -> Result<()>;
        fn array_erase(o: &Obj, i: i32) -> Result<()>;

        fn dict_keys(o: &Obj) -> Result<Vec<Bytes>>;
        fn dict_has(o: &Obj, key: &[u8]) -> Result<bool>;
        fn dict_get(o: &Obj, key: &[u8]) -> Result<UniquePtr<Obj>>;
        fn dict_replace(o: &Obj, key: &[u8], v: &Obj) -> Result<()>;
        fn dict_remove(o: &Obj, key: &[u8]) -> Result<()>;

        fn stream_dict(o: &Obj) -> Result<UniquePtr<Obj>>;
        fn stream_raw_data(o: &Obj) -> Result<UniquePtr<Buf>>;
        /// `decode_level`: 0 none, 1 generalized, 2 specialized, 3 all.
        fn stream_data(o: &Obj, decode_level: i32) -> Result<UniquePtr<Buf>>;
        fn stream_replace_data(
            o: &Obj,
            data: &[u8],
            filter: &Obj,
            decode_parms: &Obj,
        ) -> Result<()>;

        /// Replace the stream data with lazily produced bytes. The provider must return the same
        /// bytes on every call (qpdf may call it several times, e.g. to learn the length).
        fn stream_replace_provider(
            o: &Obj,
            provider: Box<ProviderBox>,
            filter: &Obj,
            decode_parms: &Obj,
        ) -> Result<()>;

        fn buf_data(b: &Buf) -> &[u8];

        // ---- AcroForm ----
        fn form_has_acroform(d: &Doc) -> Result<bool>;
        fn form_fields(d: &Doc) -> Result<Vec<FieldInfo>>;
        /// Set a field value. Text and choice fields take UTF-8 text and get regenerated
        /// appearances when `appearance` is set; checkbox and radio fields take a state name.
        fn form_set_value(
            d: &Doc,
            id: i32,
            gen_: i32,
            value: &[u8],
            appearance: bool,
        ) -> Result<()>;
        fn form_need_appearances(d: &Doc) -> Result<bool>;
        fn form_set_need_appearances(d: &Doc, v: bool) -> Result<()>;
        /// Generate appearance streams for every text and choice widget.
        fn form_generate_appearances(d: &Doc) -> Result<()>;

        // ---- Page-level helpers ----
        fn flatten_annotations(d: &Doc, required_flags: i32, forbidden_flags: i32) -> Result<()>;
        fn remove_unreferenced_resources(d: &Doc) -> Result<()>;
        fn page_remove_unreferenced_resources(d: &Doc, page: &Obj) -> Result<()>;
        /// Copy pages `indices` of `src` into `d` starting at page index `at` (clamped to the end),
        /// carrying annotations and form fields over. Returns the new page objects.
        fn copy_pages(d: &Doc, src: &Doc, indices: &[u32], at: usize) -> Result<Vec<ObjGenPair>>;

        // ---- Document structure ----
        fn outlines_read(d: &Doc) -> Result<Vec<OutlineItem>>;
        fn page_labels_read(d: &Doc) -> Result<Vec<LabelRange>>;
        /// An empty list removes `/PageLabels`.
        fn page_labels_write(d: &Doc, ranges: &[LabelRange]) -> Result<()>;
        fn embedded_files_list(d: &Doc) -> Result<Vec<EmbeddedFileInfo>>;

        // ---- Name and number trees ----
        fn name_tree_new(d: &Doc) -> Result<UniquePtr<Obj>>;
        fn name_tree_keys(d: &Doc, root: &Obj) -> Result<Vec<Bytes>>;
        /// Null pointer when the key is absent.
        fn name_tree_get(d: &Doc, root: &Obj, key: &[u8]) -> Result<UniquePtr<Obj>>;
        fn name_tree_set(d: &Doc, root: &Obj, key: &[u8], value: &Obj) -> Result<()>;
        fn name_tree_remove(d: &Doc, root: &Obj, key: &[u8]) -> Result<bool>;
        fn number_tree_new(d: &Doc) -> Result<UniquePtr<Obj>>;
        fn number_tree_keys(d: &Doc, root: &Obj) -> Result<Vec<i64>>;
        fn number_tree_get(d: &Doc, root: &Obj, key: i64) -> Result<UniquePtr<Obj>>;
        fn number_tree_set(d: &Doc, root: &Obj, key: i64, value: &Obj) -> Result<()>;
        fn number_tree_remove(d: &Doc, root: &Obj, key: i64) -> Result<bool>;
    }
}

pub use ffi::*;

/// The CMake cache entries and crypto object files recorded when qpdf was built; checked by the
/// license gate and by `papyrine-cos` tests (ADR-021).
pub const CRYPTO_BUILD_CONFIG: &str =
    include_str!(concat!(env!("OUT_DIR"), "/qpdf-crypto-config.txt"));

/// Lazy stream data supplier handed to C++ (`stream_replace_provider`).
pub struct ProviderBox {
    f: Box<dyn Fn() -> Result<Vec<u8>, String>>,
}

impl ProviderBox {
    pub fn new(f: impl Fn() -> Result<Vec<u8>, String> + 'static) -> Box<ProviderBox> {
        Box::new(ProviderBox { f: Box::new(f) })
    }
}

fn provider_produce(p: &ProviderBox) -> Result<Vec<u8>, String> {
    // A panic must not unwind through C++ frames.
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (p.f)())) {
        Ok(r) => r,
        Err(_) => Err("stream provider panicked".into()),
    }
}

/// Progress sink for `doc_write`. Wraps a closure that is only called during the write call.
pub struct ProgressBox {
    f: Option<*mut dyn FnMut(i32) -> bool>,
}

impl ProgressBox {
    pub fn none() -> ProgressBox {
        ProgressBox { f: None }
    }

    /// # Safety
    /// The returned value must not outlive `f`, and must only be used for one `doc_write` call.
    pub unsafe fn new(f: &mut dyn FnMut(i32) -> bool) -> ProgressBox {
        // SAFETY: lifetime erasure only; upheld by the caller as documented above.
        let raw: *mut (dyn FnMut(i32) -> bool + '_) = f;
        let raw: *mut (dyn FnMut(i32) -> bool + 'static) = unsafe { std::mem::transmute(raw) };
        ProgressBox { f: Some(raw) }
    }
}

fn progress_report(p: &mut ProgressBox, percent: i32) -> bool {
    let Some(f) = p.f else { return true };
    // SAFETY: valid for the duration of the write call (contract of `ProgressBox::new`).
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe { (*f)(percent) }));
    r.unwrap_or(false)
}
