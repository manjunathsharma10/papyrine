use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use papyrine_cos::{Document, OpenOptions, Secret};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::import::{ImportOptions, import_pages};
use super::outlines::OutlineMode;
use crate::changeset::ChangeSet;
use crate::command::Command;
use crate::context::EditContext;
use crate::error::{Error, Result};
use crate::registry::CommandRegistry;
use crate::text::LocalizedText;

/// Content-address of a blob: the BLAKE3 hash in hex, the key the host stores it under.
pub fn blob_id(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// Where replayed commands find the bytes of files they were given as input.
pub trait BlobSource: Send + Sync {
    fn get(&self, id: &str) -> Option<Arc<Vec<u8>>>;
}

/// A simple in-memory [`BlobSource`] (tests, and hosts that keep inputs in memory).
#[derive(Default)]
pub struct MemoryBlobs(Mutex<HashMap<String, Arc<Vec<u8>>>>);

impl MemoryBlobs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Store `bytes` and return the id commands use to refer to them.
    pub fn put(&self, bytes: Vec<u8>) -> String {
        let id = blob_id(&bytes);
        self.0
            .lock()
            .expect("blob map poisoned")
            .insert(id.clone(), Arc::new(bytes));
        id
    }
}

impl BlobSource for MemoryBlobs {
    fn get(&self, id: &str) -> Option<Arc<Vec<u8>>> {
        self.0.lock().expect("blob map poisoned").get(id).cloned()
    }
}

struct Bytes(Arc<Vec<u8>>);

impl AsRef<[u8]> for Bytes {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

/// Insert pages of another PDF so the first becomes page `at` (`0..=page_count`).
///
/// The other file is an external input: the host stores its bytes under [`blob_id`] and
/// `params()` carries only that id, so replay never depends on a file that may have changed.
/// Pages come with their annotations, form fields (renamed on a name collision), the named
/// destinations they use and the bookmarks that lead to them; links to pages that were not
/// inserted are dropped. A password for an encrypted source is never part of `params()`.
pub struct InsertPagesFromPdf {
    blob: String,
    bytes: Option<Arc<Vec<u8>>>,
    password: Option<Secret>,
    at: usize,
    /// Zero-based source pages in the order to insert them; `None` means all, in order.
    pages: Option<Vec<usize>>,
    outline: OutlineMode,
    /// Bookmark title for [`OutlineMode::PerFile`]; also names the step.
    name: String,
    named_dests: bool,
}

#[derive(Serialize, Deserialize)]
struct Params {
    blob: String,
    at: usize,
    #[serde(default)]
    pages: Option<Vec<usize>>,
    #[serde(default)]
    outline: OutlineMode,
    #[serde(default)]
    name: String,
    #[serde(default = "yes")]
    named_dests: bool,
}

fn yes() -> bool {
    true
}

impl InsertPagesFromPdf {
    pub const NAME: &'static str = "insert_pages";

    /// Insert every page of the PDF in `bytes` at position `at`.
    pub fn new(bytes: Vec<u8>, at: usize) -> Self {
        InsertPagesFromPdf {
            blob: blob_id(&bytes),
            bytes: Some(Arc::new(bytes)),
            password: None,
            at,
            pages: None,
            outline: OutlineMode::Merged,
            name: String::new(),
            named_dests: true,
        }
    }

    /// Insert only these zero-based source pages (in this order).
    pub fn pages(mut self, pages: Vec<usize>) -> Self {
        self.pages = Some(pages);
        self
    }

    pub fn password(mut self, password: impl Into<Secret>) -> Self {
        self.password = Some(password.into());
        self
    }

    pub fn outline(mut self, mode: OutlineMode, title: impl Into<String>) -> Self {
        self.outline = mode;
        self.name = title.into();
        self
    }

    pub fn named_dests(mut self, on: bool) -> Self {
        self.named_dests = on;
        self
    }

    /// The input the host must store under [`Self::blob_id`].
    pub fn blob_bytes(&self) -> Option<&Arc<Vec<u8>>> {
        self.bytes.as_ref()
    }

    pub fn blob_id(&self) -> &str {
        &self.blob
    }

    fn from_params(p: &Value, blobs: &dyn BlobSource) -> Result<Box<dyn Command>> {
        let p: Params = serde_json::from_value(p.clone())
            .map_err(|e| Error::invalid(format!("{}: {e}", Self::NAME)))?;
        let bytes = blobs
            .get(&p.blob)
            .ok_or_else(|| Error::invalid(format!("input blob {} is not available", p.blob)))?;
        Ok(Box::new(InsertPagesFromPdf {
            blob: p.blob,
            bytes: Some(bytes),
            password: None,
            at: p.at,
            pages: p.pages,
            outline: p.outline,
            name: p.name,
            named_dests: p.named_dests,
        }))
    }

    /// Register this command so replay finds its input in `blobs`.
    pub fn register(reg: &mut CommandRegistry, blobs: Arc<dyn BlobSource>) {
        reg.register(Self::NAME, move |_, p| Self::from_params(p, blobs.as_ref()));
    }
}

impl Command for InsertPagesFromPdf {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn describe(&self) -> LocalizedText {
        let mut t = LocalizedText::new("cmd.insert-pages").arg("at", self.at + 1);
        if let Some(p) = &self.pages {
            t = t.arg("count", p.len());
        }
        if !self.name.is_empty() {
            t = t.arg("name", &self.name);
        }
        t
    }

    fn params(&self) -> Value {
        json!(Params {
            blob: self.blob.clone(),
            at: self.at,
            pages: self.pages.clone(),
            outline: self.outline,
            name: self.name.clone(),
            named_dests: self.named_dests,
        })
    }

    fn apply(&mut self, cx: &mut EditContext<'_>) -> Result<ChangeSet> {
        let bytes = self
            .bytes
            .clone()
            .ok_or_else(|| Error::invalid("input blob is not loaded"))?;
        let opts = OpenOptions {
            password: self.password.clone(),
            ..OpenOptions::default()
        };
        let src = Document::open_bytes(Bytes(bytes), &opts)?;
        let n = src.page_count()?;
        let indices: Vec<usize> = match &self.pages {
            Some(p) => p.clone(),
            None => (0..n).collect(),
        };
        let io = ImportOptions {
            outline: self.outline,
            outline_title: self.name.clone(),
            named_dests: self.named_dests,
            place_by_page: true,
        };
        if let Err(e) = import_pages(cx, &src, &indices, self.at, &io) {
            // A failed import must leave the document as it was.
            cx.rollback()?;
            return Err(e);
        }
        cx.changeset()
    }
}
