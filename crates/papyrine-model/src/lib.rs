//! Typed, lazily materialized read-side views over `papyrine-cos` (ARCHITECTURE section 4.3).
//!
//! A [`Model`] owns a [`papyrine_cos::Document`] and builds views on demand: page tree with
//! inheritance, boxes and rotation, page labels, outlines, destinations and actions, AcroForm
//! fields, annotations, the Info dictionary, catalog summary, encryption and permissions,
//! signature fields and JavaScript inventory.
//!
//! Views are cached against per-object generation counters. Every indirect object a view read is
//! recorded as a dependency; [`Model::invalidate`] bumps the counters of the objects an edit
//! touched and drops exactly the views that depended on them.
//!
//! All accessors are lenient: malformed values fall back to spec defaults and are noted in
//! [`Model::diagnostics`]; only failures of the COS layer itself surface as errors. Tree walks
//! are cycle- and size-guarded.

mod annot;
mod dest;
mod docinfo;
mod form;
mod labels;
mod model;
mod nametree;
mod outline;
mod pages;
mod script;
mod signature;
pub mod text;

pub use annot::{AnnotSubtype, Annotation, AnnotationSet, BorderKind, BorderStyle, annot_flags};
pub use dest::{Action, ActionKind, DestPage, Destination, ExternalKind, ExternalTarget, Fit};
pub use docinfo::{
    CatalogInfo, DocumentInfo, OpenAction, PermissionSummary, SecuritySummary, ViewerPreferences,
};
pub use form::{
    AnnotPageMap, ChoiceOption, Field, FieldAction, FieldKind, FieldValue, Form, Widget, XfaKind,
    flags as field_flags,
};
pub use labels::{LabelRange, LabelStyle, PageLabels, format_number};
pub use model::Model;
pub use outline::{OutlineItem, Outlines};
pub use pages::{DEFAULT_MEDIA_BOX, PageAction, PageBoxes, PageInfo, PageList, normalize_rotation};
pub use script::{Script, ScriptInventory, ScriptLocation, is_javascript};
pub use signature::{SignatureInfo, SignatureSummary};
pub use text::PdfDate;

pub use papyrine_cos::{ObjId, Object};
