use std::path::Path;

use papyrine_cos::{Document, ObjectKind, OpenOptions, Secret};
use papyrine_ops::{
    EditContext, FieldReport, ImportOptions, OutlineMode, check_source_permissions, import_pages,
};

use crate::labels::LabelCarry;
use crate::{Error, Result};

/// Which document-information entries (`/Info`: title, author, subject, keywords) and catalog
/// defaults (`/Lang`) a new document takes from its first source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InfoPolicy {
    #[default]
    None,
    First,
}

/// One file in a merge.
pub struct MergeInput {
    /// Shown as the file's top-level bookmark in [`OutlineMode::PerFile`] (usually the file
    /// name without extension).
    pub name: String,
    pub doc: Document,
    /// Zero-based source pages in the order to use them; `None` is every page.
    pub pages: Option<Vec<usize>>,
}

impl MergeInput {
    pub fn new(name: impl Into<String>, doc: Document) -> Self {
        MergeInput {
            name: name.into(),
            doc,
            pages: None,
        }
    }

    pub fn pages(mut self, pages: Vec<usize>) -> Self {
        self.pages = Some(pages);
        self
    }

    /// Open `path` (the file name stem becomes the input's name). Sources stay open until the
    /// merged document is written, because their stream data is copied lazily.
    pub fn open(path: &Path, password: Option<Secret>) -> Result<Self> {
        let doc = Document::open_path(
            path,
            &OpenOptions {
                password,
                ..OpenOptions::default()
            },
        )?;
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(MergeInput::new(name, doc))
    }
}

#[derive(Debug, Clone)]
pub struct MergeOptions {
    /// Default: one top-level bookmark per file.
    pub outline: OutlineMode,
    pub named_dests: bool,
    /// Carry each page's label from its source.
    pub page_labels: bool,
    pub info: InfoPolicy,
    /// `/Info /Title` of the result.
    pub title: Option<String>,
}

impl Default for MergeOptions {
    fn default() -> Self {
        MergeOptions {
            outline: OutlineMode::PerFile,
            named_dests: true,
            page_labels: true,
            info: InfoPolicy::None,
            title: None,
        }
    }
}

/// Progress of a long operation; the callback returns `false` to cancel.
#[derive(Debug, Clone)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    pub current: String,
}

/// What happened to one input.
#[derive(Debug)]
pub struct InputReport {
    pub name: String,
    /// Index of the file's first page in the merged document.
    pub first_page: usize,
    pub pages: usize,
    pub fields: FieldReport,
    pub links_dropped: usize,
    pub bookmarks: usize,
    pub dests_renamed: Vec<(String, String)>,
}

impl std::fmt::Debug for MergeResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MergeResult")
            .field("reports", &self.reports)
            .finish_non_exhaustive()
    }
}

pub struct MergeResult {
    pub doc: Document,
    pub reports: Vec<InputReport>,
}

/// Copy the source's `/Info` text entries and `/Lang` into a new document.
pub(crate) fn carry_info(out: &Document, src: &Document, policy: InfoPolicy) -> Result<()> {
    if policy == InfoPolicy::None {
        return Ok(());
    }
    let src_info = src.trailer()?.dict_get("Info")?;
    if src_info.kind()? == ObjectKind::Dictionary {
        let info = out.make_indirect(&out.new_dict())?;
        let mut any = false;
        for k in ["Title", "Author", "Subject", "Keywords", "Creator"] {
            let v = src_info.dict_get(k)?;
            if v.kind()? == ObjectKind::String {
                info.dict_set(k, &out.new_string(v.string()?)?)?;
                any = true;
            }
        }
        if any {
            out.trailer()?.dict_set("Info", &info)?;
        }
    }
    let lang = src.root()?.dict_get("Lang")?;
    if lang.kind()? == ObjectKind::String {
        out.root()?
            .dict_set("Lang", &out.new_string(lang.string()?)?)?;
    }
    Ok(())
}

/// Open the bookmarks panel when the result has bookmarks.
pub(crate) fn finish_catalog(out: &Document) -> Result<()> {
    let root = out.root()?;
    if root.dict_get("Outlines")?.kind()? == ObjectKind::Dictionary && !root.dict_has("PageMode")? {
        root.dict_set("PageMode", &out.new_name("UseOutlines")?)?;
    }
    Ok(())
}

pub(crate) fn set_title(out: &Document, title: &str) -> Result<()> {
    let trailer = out.trailer()?;
    let mut info = trailer.dict_get("Info")?;
    if info.kind()? != ObjectKind::Dictionary {
        info = out.make_indirect(&out.new_dict())?;
        trailer.dict_set("Info", &info)?;
    }
    info.dict_set(
        "Title",
        &out.new_string(papyrine_ops::encode_text_string(title))?,
    )?;
    Ok(())
}

/// Merge `inputs`, in order, into one new document.
///
/// Every input must allow page extraction or assembly (or have been opened with its owner
/// password). The inputs must stay alive until the result is written.
pub fn merge(
    inputs: Vec<MergeInput>,
    opts: &MergeOptions,
    mut progress: Option<&mut dyn FnMut(&Progress) -> bool>,
) -> Result<MergeResult> {
    if inputs.is_empty() {
        return Err(Error::invalid("nothing to merge"));
    }
    for i in &inputs {
        check_source_permissions(&i.doc)?;
    }
    let out = Document::new_empty()?;
    let mut cx = EditContext::new(&out)?;
    let mut carry = LabelCarry::default();
    let mut reports = Vec::with_capacity(inputs.len());
    let total = inputs.len();
    for (n, input) in inputs.iter().enumerate() {
        if let Some(p) = progress.as_deref_mut()
            && !p(&Progress {
                done: n,
                total,
                current: input.name.clone(),
            })
        {
            return Err(Error::Cancelled);
        }
        let count = input.doc.page_count()?;
        let indices: Vec<usize> = input.pages.clone().unwrap_or_else(|| (0..count).collect());
        let at = out.page_count()?;
        let io = ImportOptions {
            outline: opts.outline,
            outline_title: input.name.clone(),
            named_dests: opts.named_dests,
            place_by_page: false,
        };
        let imp = import_pages(&mut cx, &input.doc, &indices, at, &io)
            .map_err(|e| Error::Invalid(format!("{}: {e}", input.name)))?;
        if opts.page_labels {
            carry.add(&input.doc.page_labels()?, &indices);
        }
        reports.push(InputReport {
            name: input.name.clone(),
            first_page: at,
            pages: indices.len(),
            fields: imp.fields,
            links_dropped: imp.links_dropped,
            bookmarks: imp.bookmarks,
            dests_renamed: imp.dests_renamed,
        });
    }
    carry.finish(&out)?;
    carry_info(&out, &inputs[0].doc, opts.info)?;
    if let Some(t) = &opts.title {
        set_title(&out, t)?;
    }
    finish_catalog(&out)?;
    if let Some(p) = progress {
        p(&Progress {
            done: total,
            total,
            current: String::new(),
        });
    }
    Ok(MergeResult { doc: out, reports })
}
