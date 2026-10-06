//! PDFium document wrapper: open over a [`MultiBuf`], tile/preview rendering,
//! text layer, page cache, and a form-fill environment used **only to draw**
//! widget appearances (`FPDF_FFLDraw`); field state never lives in PDFium.

use crate::error::{Error, Result};
use crate::library::Library;
use crate::source::{Bytes, MultiBuf};
use crate::tiles::{self, TILE_SIZE, TileCoord};
use pdfium_render::prelude::*;
use std::ffi::c_void;
use std::os::raw::{c_int, c_uchar, c_ulong};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::sync::Arc;

/// PDFium page cache cap (ARCHITECTURE §4.6 / ADR-009).
pub const PAGE_CACHE_CAP: usize = 8;

const FPDFBITMAP_BGRA: c_int = 4;
// fpdfview.h: FPDF_ANNOT = 0x01, FPDF_REVERSE_BYTE_ORDER = 0x10, FPDF_RENDER_LIMITEDIMAGECACHE = 0x200.
const RENDER_FLAGS: c_int = 0x01 | 0x10 | 0x200;
const RENDER_TOBECONTINUED: c_int = 1;
const RENDER_DONE: c_int = 2;

/// A rendered bitmap.
///
/// Pixels are **RGBA, 8 bits per channel, row-major, stride `width * 4`,
/// opaque**: the page is composited over a white background before rendering,
/// so alpha is always 255 and premultiplied == straight alpha.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tile {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Tile {
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [
            self.rgba[i],
            self.rgba[i + 1],
            self.rgba[i + 2],
            self.rgba[i + 3],
        ]
    }
}

/// One character of the text layer. Boxes are in **page user space**: points,
/// origin bottom-left, before page rotation (PDFium's convention).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CharBox {
    pub ch: char,
    /// Tight glyph box.
    pub left: f64,
    pub right: f64,
    pub bottom: f64,
    pub top: f64,
    /// Loose (font-metrics) box, better for selection rectangles.
    pub loose_left: f64,
    pub loose_right: f64,
    pub loose_bottom: f64,
    pub loose_top: f64,
}

/// Page text: `chars[i]` is PDFium char index `i`, the same index
/// [`Document::char_at`] returns. `text` is those chars concatenated.
#[derive(Debug, Clone, Default)]
pub struct PageText {
    pub text: String,
    pub chars: Vec<CharBox>,
}

/// Page text in compact, cache-friendly form (see [`Document::page_glyphs`]).
#[derive(Debug, Clone, Default)]
pub struct Glyphs {
    pub chars: Vec<char>,
    /// Tight glyph boxes `[left, bottom, right, top]`, page user space.
    pub tight: Vec<[f32; 4]>,
    /// Loose (font metrics) boxes, same layout.
    pub loose: Vec<[f32; 4]>,
}

impl Glyphs {
    /// Approximate heap bytes, for cache accounting.
    pub fn heap_bytes(&self) -> usize {
        self.chars.len() * (4 + 16 + 16)
    }
}

/// Keeps the file-access struct and the buffer it points at alive and pinned.
struct Inner {
    lib: Arc<Library>,
    handle: FPDF_DOCUMENT,
    access: Box<Access>,
    /// Form-fill environment, null for documents without an AcroForm. Used
    /// for `FPDF_FFLDraw` only: no event, focus or script call is ever made.
    form: FPDF_FORMHANDLE,
    /// Must stay at a fixed address while `form` is alive.
    form_info: Box<FPDF_FORMFILLINFO>,
}

#[repr(C)]
struct Access {
    raw: FPDF_FILEACCESS,
    buf: MultiBuf,
}

unsafe extern "C" fn get_block(
    param: *mut c_void,
    position: c_ulong,
    out: *mut c_uchar,
    size: c_ulong,
) -> c_int {
    // SAFETY: `param` is `&Access::buf`, valid while the document is open; PDFium
    // guarantees `out` has `size` writable bytes.
    let r = catch_unwind(AssertUnwindSafe(|| unsafe {
        let buf = &*(param as *const MultiBuf);
        let slice = std::slice::from_raw_parts_mut(out, size as usize);
        #[allow(clippy::useless_conversion)] // c_ulong is u32 on Windows
        buf.read_at(u64::from(position), slice)
    }));
    c_int::from(r.unwrap_or(false))
}

impl Inner {
    fn open(lib: &Arc<Library>, buf: MultiBuf, password: Option<&str>) -> Result<Inner> {
        let len = c_ulong::try_from(buf.len()).map_err(|_| Error::TooLarge)?;
        let mut access = Box::new(Access {
            raw: FPDF_FILEACCESS {
                m_FileLen: len,
                m_GetBlock: Some(get_block),
                m_Param: ptr::null_mut(),
            },
            buf,
        });
        access.raw.m_Param = &access.buf as *const MultiBuf as *mut c_void;
        // SAFETY: `access` is heap-pinned and outlives the handle (see Drop).
        let handle = unsafe {
            lib.bindings
                .FPDF_LoadCustomDocument(&mut access.raw, password)
        };
        if handle.is_null() {
            // SAFETY: plain getter.
            let code = unsafe { lib.bindings.FPDF_GetLastError() };
            return Err(match code {
                2 => Error::File,
                3 => Error::Format,
                4 if password.is_some() => Error::PasswordIncorrect,
                4 => Error::PasswordRequired,
                5 => Error::Security,
                c => Error::Other(format!("PDFium error {c}")),
            });
        }
        // SAFETY: an all-zero FORMFILLINFO is valid (null callbacks); version 1 uses
        // only the stable interface, and every callback PDFium may invoke is optional.
        let mut form_info: Box<FPDF_FORMFILLINFO> = Box::new(unsafe { std::mem::zeroed() });
        form_info.version = 1;
        let mut form = ptr::null_mut();
        // SAFETY: valid doc handle; `form_info` is boxed and outlives `form` (see Drop).
        unsafe {
            if lib.bindings.FPDF_GetFormType(handle) != 0 {
                form = lib
                    .bindings
                    .FPDFDOC_InitFormFillEnvironment(handle, &mut *form_info);
                if !form.is_null() {
                    // No tint over fields: appearances only.
                    lib.bindings.FPDF_SetFormFieldHighlightAlpha(form, 0);
                }
            }
        }
        Ok(Inner {
            lib: lib.clone(),
            handle,
            access,
            form,
            form_info,
        })
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        // SAFETY: handle came from FPDF_LoadCustomDocument; the form environment
        // is exited first; `access` and `form_info` drop afterwards.
        unsafe {
            if !self.form.is_null() {
                self.lib.bindings.FPDFDOC_ExitFormFillEnvironment(self.form);
            }
            self.lib.bindings.FPDF_CloseDocument(self.handle);
        }
        let _ = (&self.access, &self.form_info);
    }
}

struct CachedPage {
    index: usize,
    page: FPDF_PAGE,
    text: FPDF_TEXTPAGE,
}

/// An open document. Not `Sync`; drive it from one thread at a time.
pub struct Document {
    inner: Inner,
    base: Bytes,
    password: Option<String>,
    page_count: usize,
    /// Most recently used first.
    cache: Vec<CachedPage>,
    /// Pages kept loaded at once (<= [`PAGE_CACHE_CAP`] by default).
    cap: usize,
    /// Pages parsed since the document was (re)opened.
    loads: u64,
}

// SAFETY: PDFium handles have no thread affinity; every call goes through the
// serialising binding layer, and `Document` is only used via `&mut self`.
unsafe impl Send for Document {}

#[repr(C)]
struct Pause<'a> {
    /// Must stay first: PDFium hands this struct's address back as `*mut IFSDK_PAUSE`.
    #[allow(dead_code)]
    raw: IFSDK_PAUSE,
    cancel: &'a mut dyn FnMut() -> bool,
    cancelled: bool,
}

unsafe extern "C" fn need_to_pause(this: *mut IFSDK_PAUSE) -> c_int {
    // SAFETY: `raw` is the first field of the repr(C) `Pause` we passed in.
    let p = unsafe { &mut *(this as *mut Pause<'_>) };
    if !p.cancelled {
        let cancel = &mut *p.cancel;
        p.cancelled = catch_unwind(AssertUnwindSafe(cancel)).unwrap_or(true);
    }
    c_int::from(p.cancelled)
}

impl Document {
    /// Open `base` followed by `sections` (appended incremental updates).
    pub fn open(
        lib: &Arc<Library>,
        base: Bytes,
        sections: &[Bytes],
        password: Option<&str>,
    ) -> Result<Document> {
        let inner = Inner::open(lib, Self::buffer(&base, sections), password)?;
        // SAFETY: valid handle.
        let page_count = unsafe { lib.bindings.FPDF_GetPageCount(inner.handle) }.max(0) as usize;
        Ok(Document {
            inner,
            base,
            password: password.map(String::from),
            page_count,
            cache: Vec::new(),
            cap: PAGE_CACHE_CAP,
            loads: 0,
        })
    }

    fn buffer(base: &Bytes, sections: &[Bytes]) -> MultiBuf {
        let mut b = MultiBuf::new(base.clone());
        for s in sections {
            b.push(s.clone());
        }
        b
    }

    /// Re-open over the same base with a new set of sections. The new document
    /// is opened first; on error the current one is left untouched. The page
    /// cache is dropped.
    pub fn reopen(&mut self, sections: &[Bytes]) -> Result<()> {
        let lib = self.inner.lib.clone();
        let inner = Inner::open(
            &lib,
            Self::buffer(&self.base, sections),
            self.password.as_deref(),
        )?;
        self.clear_cache();
        // SAFETY: valid handle.
        self.page_count = unsafe { lib.bindings.FPDF_GetPageCount(inner.handle) }.max(0) as usize;
        self.inner = inner; // drops (closes) the old document
        self.loads = 0;
        Ok(())
    }

    fn b(&self) -> &dyn PdfiumLibraryBindings {
        self.inner.lib.bindings.as_ref()
    }

    pub fn page_count(&self) -> usize {
        self.page_count
    }

    /// Total bytes visible to PDFium (base + sections).
    pub fn byte_len(&self) -> u64 {
        self.inner.access.buf.len()
    }

    /// Number of pages currently held in PDFium's page cache (<= [`PAGE_CACHE_CAP`]).
    pub fn cached_pages(&self) -> Vec<usize> {
        self.cache.iter().map(|c| c.index).collect()
    }

    fn check(&self, index: usize) -> Result<()> {
        if index < self.page_count {
            Ok(())
        } else {
            Err(Error::PageOutOfRange(index))
        }
    }

    /// Page size in points, rotation applied (what the user sees).
    pub fn page_size(&self, index: usize) -> Result<(f32, f32)> {
        self.check(index)?;
        let mut s = FS_SIZEF {
            width: 0.0,
            height: 0.0,
        };
        // SAFETY: valid doc handle, in-range index, valid out pointer.
        let ok = unsafe {
            self.b()
                .FPDF_GetPageSizeByIndexF(self.inner.handle, index as c_int, &mut s)
        };
        if ok == 0 {
            Err(Error::PageLoad(index))
        } else {
            Ok((s.width, s.height))
        }
    }

    pub fn page_sizes(&self) -> Result<Vec<(f32, f32)>> {
        (0..self.page_count).map(|i| self.page_size(i)).collect()
    }

    /// Page /Rotate in degrees (0, 90, 180, 270).
    pub fn page_rotation(&mut self, index: usize) -> Result<u16> {
        let page = self.load_page(index)?.page;
        // SAFETY: page is in the cache and valid.
        let r = unsafe { self.b().FPDFPage_GetRotation(page) };
        Ok(((r.rem_euclid(4)) * 90) as u16)
    }

    /// Page /Rotate without keeping the page loaded (PDFium does not parse the
    /// content stream until the first render, so this is cheap).
    pub fn page_rotation_uncached(&self, index: usize) -> Result<u16> {
        self.check(index)?;
        // SAFETY: valid doc handle, in-range index; the page is closed before returning.
        unsafe {
            let page = self.b().FPDF_LoadPage(self.inner.handle, index as c_int);
            if page.is_null() {
                return Err(Error::PageLoad(index));
            }
            let r = self.b().FPDFPage_GetRotation(page);
            self.b().FPDF_ClosePage(page);
            Ok(((r.rem_euclid(4)) * 90) as u16)
        }
    }

    fn clear_cache(&mut self) {
        let lib = self.inner.lib.clone();
        let form = self.inner.form;
        for c in self.cache.drain(..) {
            close_cached(lib.bindings.as_ref(), form, &c);
        }
    }

    /// Close every cached page (PDFium frees the parsed page and its decoded
    /// images; the document-level object store stays until [`Self::reopen`]).
    pub fn close_pages(&mut self) {
        self.clear_cache();
    }

    /// Close all cached pages except `keep`.
    pub fn close_pages_except(&mut self, keep: usize) {
        let lib = self.inner.lib.clone();
        let form = self.inner.form;
        let (kept, dropped): (Vec<_>, Vec<_>) = self.cache.drain(..).partition(|c| c.index == keep);
        self.cache = kept;
        for c in dropped {
            close_cached(lib.bindings.as_ref(), form, &c);
        }
    }

    /// Close one cached page (no-op when it is not cached).
    pub fn close_page(&mut self, index: usize) {
        if let Some(pos) = self.cache.iter().position(|c| c.index == index) {
            let c = self.cache.remove(pos);
            close_cached(self.b(), self.inner.form, &c);
        }
    }

    /// Limit how many pages stay loaded (1 ..= [`PAGE_CACHE_CAP`]); extra
    /// pages are closed now.
    pub fn set_page_cache_cap(&mut self, cap: usize) {
        self.cap = cap.clamp(1, PAGE_CACHE_CAP);
        while self.cache.len() > self.cap {
            if let Some(old) = self.cache.pop() {
                close_cached(self.b(), self.inner.form, &old);
            }
        }
    }

    pub fn page_cache_cap(&self) -> usize {
        self.cap
    }

    /// Pages parsed since the document was opened or re-opened.
    pub fn pages_loaded(&self) -> u64 {
        self.loads
    }

    /// True when widget appearances are drawn through a form-fill environment.
    pub fn has_form_env(&self) -> bool {
        !self.inner.form.is_null()
    }

    /// Cheap check used before touching a page: is it already parsed?
    pub fn is_page_cached(&self, index: usize) -> bool {
        self.cache.iter().any(|c| c.index == index)
    }

    /// Make `index` the most-recently-used cached page (loading it if needed).
    fn load_page(&mut self, index: usize) -> Result<&mut CachedPage> {
        self.check(index)?;
        if let Some(pos) = self.cache.iter().position(|c| c.index == index) {
            let c = self.cache.remove(pos);
            self.cache.insert(0, c);
        } else {
            // SAFETY: valid doc handle, in-range index.
            let page = unsafe { self.b().FPDF_LoadPage(self.inner.handle, index as c_int) };
            if page.is_null() {
                return Err(Error::PageLoad(index));
            }
            self.loads += 1;
            if !self.inner.form.is_null() {
                // SAFETY: valid page and form handles; creates the page view FFLDraw needs.
                unsafe { self.b().FORM_OnAfterLoadPage(page, self.inner.form) };
            }
            while self.cache.len() >= self.cap
                && let Some(old) = self.cache.pop()
            {
                close_cached(self.b(), self.inner.form, &old);
            }
            self.cache.insert(
                0,
                CachedPage {
                    index,
                    page,
                    text: ptr::null_mut(),
                },
            );
        }
        Ok(&mut self.cache[0])
    }

    fn text_page(&mut self, index: usize) -> Result<FPDF_TEXTPAGE> {
        let page = self.load_page(index)?.page;
        if self.cache[0].text.is_null() {
            // SAFETY: valid page.
            let t = unsafe { self.b().FPDFText_LoadPage(page) };
            if t.is_null() {
                return Err(Error::Render("text page".into()));
            }
            self.cache[0].text = t;
        }
        Ok(self.cache[0].text)
    }

    /// Render the 512-px tile `coord` of page `index` at zoom `bucket`.
    /// Edge tiles are clipped to the page, so they can be smaller than 512.
    ///
    /// `cancel` is polled by PDFium between page objects (not inside a single
    /// image decode or shading); return true to abort with [`Error::Cancelled`].
    pub fn render_tile(
        &mut self,
        index: usize,
        bucket: i32,
        coord: TileCoord,
        cancel: &mut dyn FnMut() -> bool,
    ) -> Result<Tile> {
        let mut rgba = Vec::new();
        let (width, height) = self.render_tile_into(index, bucket, coord, &mut rgba, cancel)?;
        Ok(Tile {
            width,
            height,
            rgba,
        })
    }

    /// Like [`Self::render_tile`], writing RGBA into `out` (resized, reusable
    /// scratch so the steady state allocates nothing). Returns `(width, height)`.
    pub fn render_tile_into(
        &mut self,
        index: usize,
        bucket: i32,
        coord: TileCoord,
        out: &mut Vec<u8>,
        cancel: &mut dyn FnMut() -> bool,
    ) -> Result<(u32, u32)> {
        let (wp, hp) = self.page_size(index)?;
        let bucket = tiles::clamp_bucket_for_page(bucket, wp, hp);
        let grid = tiles::page_grid(wp, hp, bucket);
        let r = tiles::tile_rect(&grid, coord)
            .ok_or_else(|| Error::Render(format!("tile {coord:?} outside {grid:?}")))?;
        debug_assert!(r.w <= TILE_SIZE && r.h <= TILE_SIZE);
        self.render_region(
            index,
            r.w,
            r.h,
            -(r.x as i32),
            -(r.y as i32),
            grid.width_px as i32,
            grid.height_px as i32,
            out,
            cancel,
        )?;
        Ok((r.w, r.h))
    }

    /// Low-resolution whole-page preview whose longest side is at most `max_edge` px.
    pub fn render_preview(
        &mut self,
        index: usize,
        max_edge: u32,
        cancel: &mut dyn FnMut() -> bool,
    ) -> Result<Tile> {
        let mut rgba = Vec::new();
        let (width, height) = self.render_preview_into(index, max_edge, &mut rgba, cancel)?;
        Ok(Tile {
            width,
            height,
            rgba,
        })
    }

    pub fn render_preview_into(
        &mut self,
        index: usize,
        max_edge: u32,
        out: &mut Vec<u8>,
        cancel: &mut dyn FnMut() -> bool,
    ) -> Result<(u32, u32)> {
        let (wp, hp) = self.page_size(index)?;
        let k = max_edge.max(1) as f32 / wp.max(hp).max(1.0);
        let w = ((wp * k).round() as u32).max(1);
        let h = ((hp * k).round() as u32).max(1);
        self.render_region(index, w, h, 0, 0, w as i32, h as i32, out, cancel)?;
        Ok((w, h))
    }

    #[allow(clippy::too_many_arguments)]
    fn render_region(
        &mut self,
        index: usize,
        w: u32,
        h: u32,
        sx: i32,
        sy: i32,
        size_x: i32,
        size_y: i32,
        out: &mut Vec<u8>,
        cancel: &mut dyn FnMut() -> bool,
    ) -> Result<()> {
        let page = self.load_page(index)?.page;
        let form = self.inner.form;
        out.clear();
        out.resize(w as usize * h as usize * 4, 0);
        let b = self.inner.lib.bindings.as_ref();
        // SAFETY: `out` outlives the bitmap (destroyed below); stride matches.
        let bitmap = unsafe {
            b.FPDFBitmap_CreateEx(
                w as c_int,
                h as c_int,
                FPDFBITMAP_BGRA,
                out.as_mut_ptr() as *mut c_void,
                (w * 4) as c_int,
            )
        };
        if bitmap.is_null() {
            return Err(Error::Render("bitmap".into()));
        }
        let mut pause = Pause {
            raw: IFSDK_PAUSE {
                version: 1,
                NeedToPauseNow: Some(need_to_pause),
                user: ptr::null_mut(),
            },
            cancel,
            cancelled: false,
        };
        // SAFETY: valid bitmap/page; `pause` is repr-compatible (raw first) and lives across the loop.
        let status = unsafe {
            b.FPDFBitmap_FillRect(bitmap, 0, 0, w as c_int, h as c_int, 0xFFFF_FFFF);
            let praw = &mut pause as *mut Pause<'_> as *mut IFSDK_PAUSE;
            let mut st = b.FPDF_RenderPageBitmap_Start(
                bitmap,
                page,
                sx,
                sy,
                size_x,
                size_y,
                0,
                RENDER_FLAGS,
                praw,
            );
            while st == RENDER_TOBECONTINUED && !pause.cancelled {
                st = b.FPDF_RenderPage_Continue(page, praw);
            }
            if st == RENDER_DONE && !form.is_null() {
                // Widgets are skipped by FPDF_RenderPageBitmap; draw their appearances.
                b.FPDF_FFLDraw(form, bitmap, page, sx, sy, size_x, size_y, 0, RENDER_FLAGS);
            }
            b.FPDF_RenderPage_Close(page);
            b.FPDFBitmap_Destroy(bitmap);
            st
        };
        if status == RENDER_TOBECONTINUED {
            return Err(Error::Cancelled);
        }
        if status != RENDER_DONE {
            return Err(Error::Render(format!("status {status}")));
        }
        Ok(())
    }

    /// Page text as flat vectors (one entry per PDFium char index): the compact
    /// form used by search and the text cache. Boxes are `[left, bottom, right, top]`.
    pub fn page_glyphs(&mut self, index: usize) -> Result<Glyphs> {
        let tp = self.text_page(index)?;
        let b = self.b();
        // SAFETY: valid text page for the whole block.
        let n = unsafe { b.FPDFText_CountChars(tp) }.max(0);
        let mut g = Glyphs {
            chars: Vec::with_capacity(n as usize),
            tight: Vec::with_capacity(n as usize),
            loose: Vec::with_capacity(n as usize),
        };
        for i in 0..n {
            let (mut l, mut r, mut bo, mut t) = (0.0, 0.0, 0.0, 0.0);
            let mut rect = FS_RECTF {
                left: 0.0,
                top: 0.0,
                right: 0.0,
                bottom: 0.0,
            };
            // SAFETY: valid text page, in-range index, valid out pointers.
            let code = unsafe {
                b.FPDFText_GetCharBox(tp, i, &mut l, &mut r, &mut bo, &mut t);
                if b.FPDFText_GetLooseCharBox(tp, i, &mut rect) == 0 {
                    rect = FS_RECTF {
                        left: l as f32,
                        top: t as f32,
                        right: r as f32,
                        bottom: bo as f32,
                    };
                }
                b.FPDFText_GetUnicode(tp, i)
            };
            g.chars.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
            g.tight.push([l as f32, bo as f32, r as f32, t as f32]);
            g.loose.push([rect.left, rect.bottom, rect.right, rect.top]);
        }
        Ok(g)
    }

    /// Extract the page text layer with per-char boxes.
    pub fn page_text(&mut self, index: usize) -> Result<PageText> {
        let tp = self.text_page(index)?;
        let b = self.b();
        // SAFETY: valid text page for the whole block.
        let n = unsafe { b.FPDFText_CountChars(tp) }.max(0);
        let mut out = PageText {
            text: String::new(),
            chars: Vec::with_capacity(n as usize),
        };
        for i in 0..n {
            let (mut l, mut r, mut bo, mut t) = (0.0, 0.0, 0.0, 0.0);
            let (mut ll, mut lr, mut lb, mut lt) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
            // SAFETY: valid text page, in-range index, valid out pointers.
            let code = unsafe {
                b.FPDFText_GetCharBox(tp, i, &mut l, &mut r, &mut bo, &mut t);
                let mut rect = FS_RECTF {
                    left: 0.0,
                    top: 0.0,
                    right: 0.0,
                    bottom: 0.0,
                };
                if b.FPDFText_GetLooseCharBox(tp, i, &mut rect) != 0 {
                    (ll, lr, lb, lt) = (rect.left, rect.right, rect.bottom, rect.top);
                }
                b.FPDFText_GetUnicode(tp, i)
            };
            let ch = char::from_u32(code).unwrap_or('\u{FFFD}');
            out.text.push(ch);
            out.chars.push(CharBox {
                ch,
                left: l,
                right: r,
                bottom: bo,
                top: t,
                loose_left: ll as f64,
                loose_right: lr as f64,
                loose_bottom: lb as f64,
                loose_top: lt as f64,
            });
        }
        Ok(out)
    }

    /// Index of the char under page-space point `(x, y)` (points, origin
    /// bottom-left, unrotated user space), within the given tolerances.
    pub fn char_at(
        &mut self,
        index: usize,
        x: f64,
        y: f64,
        tol_x: f64,
        tol_y: f64,
    ) -> Result<Option<usize>> {
        let tp = self.text_page(index)?;
        // SAFETY: valid text page.
        let i = unsafe { self.b().FPDFText_GetCharIndexAtPos(tp, x, y, tol_x, tol_y) };
        Ok((i >= 0).then_some(i as usize))
    }
}

fn close_cached(b: &dyn PdfiumLibraryBindings, form: FPDF_FORMHANDLE, c: &CachedPage) {
    // SAFETY: handles came from FPDF_LoadPage / FPDFText_LoadPage and are closed once;
    // the page view is released before the page.
    unsafe {
        if !c.text.is_null() {
            b.FPDFText_ClosePage(c.text);
        }
        if !form.is_null() {
            b.FORM_OnBeforeClosePage(c.page, form);
        }
        b.FPDF_ClosePage(c.page);
    }
}

impl Drop for Document {
    fn drop(&mut self) {
        self.clear_cache();
    }
}
