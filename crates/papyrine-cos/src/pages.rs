//! Page-level helpers: copying pages with their annotations and fields, annotation flattening,
//! unused resource removal.

use qpdf_sys as ffi;

use crate::{Document, Error, Object, Result};

/// Annotation `/F` flag bits (PDF 32000 table 167) for [`FlattenOptions`].
pub mod annotation_flags {
    pub const INVISIBLE: u32 = 1;
    pub const HIDDEN: u32 = 1 << 1;
    pub const PRINT: u32 = 1 << 2;
    pub const NO_VIEW: u32 = 1 << 5;
}

/// Which annotations to flatten into page content. The default flattens everything that is
/// visible (not `Invisible` or `Hidden`), like qpdf's `--flatten-annotations=all`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlattenOptions {
    /// Only annotations with all of these `/F` bits.
    pub required_flags: u32,
    /// Skip annotations with any of these `/F` bits.
    pub forbidden_flags: u32,
}

impl Default for FlattenOptions {
    fn default() -> Self {
        FlattenOptions {
            required_flags: 0,
            forbidden_flags: annotation_flags::INVISIBLE | annotation_flags::HIDDEN,
        }
    }
}

impl FlattenOptions {
    /// Annotations that print (what "print to PDF" would bake in).
    pub fn printable() -> Self {
        FlattenOptions {
            required_flags: annotation_flags::PRINT,
            ..Self::default()
        }
    }
}

impl Document {
    /// Copy pages of `src` (zero-based `indices`, in the order given, repeats allowed) into this
    /// document at page index `at` (clamped to the end; `usize::MAX` appends). Annotations are
    /// duplicated per copied page and form fields are copied with their appearance state and
    /// renamed when they would collide with existing field names. `src` may be this document.
    /// Returns the new page objects in order.
    pub fn copy_pages(&self, src: &Document, indices: &[usize], at: usize) -> Result<Vec<Object>> {
        let idx = indices
            .iter()
            .map(|&i| {
                u32::try_from(i).map_err(|_| Error::Range("source page index out of range".into()))
            })
            .collect::<Result<Vec<u32>>>()?;
        if !std::rc::Rc::ptr_eq(&self.inner, &src.inner) {
            self.keep_alive(src);
        }
        let pages = ffi::copy_pages(self.ffi(), src.ffi(), &idx, at)?;
        pages
            .into_iter()
            .map(|p| self.object(crate::ObjId::new(p.id as u32, p.gen_ as u16)))
            .collect()
    }

    /// Replace the appearance of visible annotations (including filled form fields) by static
    /// page content. This removes `/AcroForm` unless `/NeedAppearances` is set, in which case
    /// widgets are left alone (call [`Document::generate_form_appearances`] first).
    pub fn flatten_annotations(&self, opts: FlattenOptions) -> Result<()> {
        Ok(ffi::flatten_annotations(
            self.ffi(),
            opts.required_flags as i32,
            opts.forbidden_flags as i32,
        )?)
    }

    /// Drop resources (fonts, images, XObjects, ...) that the page content never uses, on every
    /// page.
    pub fn remove_unreferenced_resources(&self) -> Result<()> {
        Ok(ffi::remove_unreferenced_resources(self.ffi())?)
    }

    pub fn remove_unreferenced_page_resources(&self, page: &Object) -> Result<()> {
        Ok(ffi::page_remove_unreferenced_resources(
            self.ffi(),
            page.raw(),
        )?)
    }
}
