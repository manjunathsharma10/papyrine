//! Safe Rust API over qpdf (ARCHITECTURE section 4.1).
//!
//! * [`Document`] owns one qpdf instance and is `!Send`: each open document lives on a single
//!   document-actor thread. [`Object`] handles keep their document alive.
//! * Every C++ exception becomes a typed [`Error`]; nothing panics on hostile input.
//! * Warnings and repairs are captured structurally in a [`RepairLog`].
//! * Passwords and keys are [`Secret`]s, zeroed on drop.
//!
//! ```compile_fail
//! fn assert_send<T: Send>() {}
//! assert_send::<papyrine_cos::Document>();
//! ```
//!
//! ```compile_fail
//! fn assert_send<T: Send>() {}
//! assert_send::<papyrine_cos::Object>();
//! ```

mod document;
mod embedded;
mod encryption;
mod error;
mod forms;
mod object;
mod outline;
mod page_labels;
mod pages;
mod repair;
mod secret;
mod trees;
mod write;

pub use document::{Document, Fingerprint, ObjId, OpenOptions};
pub use embedded::EmbeddedFile;
pub use encryption::{CryptMethod, EncryptionInfo};
pub use error::{Error, Result};
pub use forms::{FieldKind, FormField, Widget};
pub use object::{DecodeLevel, Object, ObjectKind, StreamBytes};
pub use outline::OutlineItem;
pub use page_labels::{LabelStyle, PageLabelRange, PageLabels};
pub use pages::{FlattenOptions, annotation_flags};
pub use repair::{RepairEntry, RepairLog};
pub use secret::Secret;
pub use write::{
    EncryptionMode, EncryptionRevision, EncryptionSpec, ObjectStreams, Permissions,
    PrintPermission, StreamMode, WriteOptions, WriteOutput,
};

/// qpdf version string of the vendored library.
pub fn qpdf_version() -> String {
    qpdf_sys::qpdf_version()
}

/// Names of the crypto implementations compiled into qpdf. The license gate (ADR-021) requires
/// exactly `["native"]`.
pub fn registered_crypto_impls() -> Result<Vec<String>> {
    Ok(qpdf_sys::crypto_impls()?)
}

/// Name of qpdf's default crypto provider (`"native"`).
pub fn default_crypto_impl() -> Result<String> {
    Ok(qpdf_sys::crypto_default()?)
}
