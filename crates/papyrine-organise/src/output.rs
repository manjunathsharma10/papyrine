//! Writing new documents to disk: atomically, validated, with collision-free names.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use papyrine_cos::{Document, WriteOptions};
use papyrine_writer::{ReplaceOptions, Validation, atomic_replace};

use crate::name::{NameVars, render_name};
use crate::split::Part;
use crate::{Error, Result};

/// What to do when a target file already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Exists {
    /// Pick `name (2).pdf`, `name (3).pdf`, ... (default; nothing is overwritten).
    #[default]
    Rename,
    /// Replace the file (atomically).
    Replace,
}

/// Write `doc` to `path` through a temporary file in the same directory; the file is re-opened
/// and its page count checked before it replaces anything. Returns the page count.
pub fn write_document(doc: &Document, path: &Path) -> Result<usize> {
    let pages = doc.page_count()?;
    atomic_replace(
        path,
        |tmp| {
            doc.write_to_path(tmp, &WriteOptions::default())
                .map(|_| ())
                .map_err(papyrine_writer::Error::from)
        },
        ReplaceOptions {
            validation: Validation {
                expect_pages: Some(pages),
                ..Validation::default()
            },
            faults: None,
        },
    )?;
    Ok(pages)
}

/// Where and how a split writes its parts.
#[derive(Debug, Clone)]
pub struct WriteSet {
    pub dir: PathBuf,
    /// Source file stem, for `{name}`.
    pub name: String,
    /// See [`render_name`]; `.pdf` is appended.
    pub template: String,
    pub exists: Exists,
}

impl WriteSet {
    pub fn new(dir: impl Into<PathBuf>, name: impl Into<String>) -> Self {
        WriteSet {
            dir: dir.into(),
            name: name.into(),
            template: "{name}_{pages}".into(),
            exists: Exists::Rename,
        }
    }

    pub fn template(mut self, t: impl Into<String>) -> Self {
        self.template = t.into();
        self
    }
}

fn title_of(doc: &Document) -> Option<String> {
    doc.outlines()
        .ok()?
        .into_iter()
        .find(|o| o.depth == 0 && !o.title.trim().is_empty())
        .map(|o| o.title)
}

fn free_name(dir: &Path, stem: &str, used: &HashSet<PathBuf>, exists: Exists) -> PathBuf {
    let candidate = |k: usize| {
        let f = if k == 1 {
            format!("{stem}.pdf")
        } else {
            format!("{stem} ({k}).pdf")
        };
        dir.join(f)
    };
    (1..)
        .map(candidate)
        .find(|p| !used.contains(p) && (exists == Exists::Replace || !p.exists()))
        .expect("unbounded search")
}

/// Write every part to `set.dir`, named by the template, and return the paths in order. If a
/// part fails (or `cancel` returns `true` between parts) the files this call created are
/// removed again (files it replaced stay replaced).
pub fn write_parts(
    parts: impl Iterator<Item = Result<Part>>,
    set: &WriteSet,
    mut cancel: Option<&mut dyn FnMut(usize) -> bool>,
) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(&set.dir)?;
    // Validate the template before doing any work.
    render_name(
        &set.template,
        &NameVars {
            name: set.name.clone(),
            n: 1,
            first: 1,
            last: 1,
            title: None,
        },
    )?;
    let mut used: HashSet<PathBuf> = HashSet::new();
    let mut written: Vec<PathBuf> = vec![];
    // Files that replaced something stay (their old content is gone either way); only files
    // this call created are removed on failure.
    let mut created: Vec<PathBuf> = vec![];
    let mut result: Result<()> = Ok(());
    for part in parts {
        if let Some(c) = cancel.as_deref_mut()
            && c(written.len())
        {
            result = Err(Error::Cancelled);
            break;
        }
        let step = (|| -> Result<(PathBuf, bool)> {
            let part = part?;
            let (first, last) = part.page_span();
            let stem = render_name(
                &set.template,
                &NameVars {
                    name: set.name.clone(),
                    n: part.index + 1,
                    first,
                    last,
                    title: title_of(&part.doc),
                },
            )?;
            let path = free_name(&set.dir, &stem, &used, set.exists);
            let existed = path.exists();
            write_document(&part.doc, &path)?;
            used.insert(path.clone());
            Ok((path, existed))
        })();
        match step {
            Ok((p, existed)) => {
                if !existed {
                    created.push(p.clone());
                }
                written.push(p);
            }
            Err(e) => {
                result = Err(e);
                break;
            }
        }
    }
    if let Err(e) = result {
        for p in &created {
            let _ = std::fs::remove_file(p);
        }
        return Err(e);
    }
    Ok(written)
}
