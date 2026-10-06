//! Font selection: bundled Noto subsets first, then system fonts (lazy fontdb scan) for
//! characters the bundled font lacks, embedded only when the font's OS/2 `fsType` allows it.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use crate::geometry::FontFamily;

static SANS_R: &[u8] = include_bytes!("../../assets/NotoSans-Regular.ttf");
static SANS_B: &[u8] = include_bytes!("../../assets/NotoSans-Bold.ttf");
static SERIF_R: &[u8] = include_bytes!("../../assets/NotoSerif-Regular.ttf");
static SERIF_B: &[u8] = include_bytes!("../../assets/NotoSerif-Bold.ttf");
static MONO_R: &[u8] = include_bytes!("../../assets/NotoSansMono-Regular.ttf");
static MONO_B: &[u8] = include_bytes!("../../assets/NotoSansMono-Bold.ttf");

#[derive(Clone)]
enum Bytes {
    Static(&'static [u8]),
    Shared(Arc<Vec<u8>>),
}

/// One face of a font file (a `.ttc` has several).
#[derive(Clone)]
pub struct FontFace {
    /// Stable identity used to deduplicate faces.
    pub key: String,
    /// Human-readable name (PostScript name when known).
    pub name: String,
    pub system: bool,
    pub index: u32,
    bytes: Bytes,
}

impl std::fmt::Debug for FontFace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FontFace({})", self.key)
    }
}

impl FontFace {
    pub fn bytes(&self) -> &[u8] {
        match &self.bytes {
            Bytes::Static(b) => b,
            Bytes::Shared(b) => b,
        }
    }

    pub fn ttf(&self) -> Option<rustybuzz::ttf_parser::Face<'_>> {
        rustybuzz::ttf_parser::Face::parse(self.bytes(), self.index).ok()
    }

    pub fn covers(&self, c: char) -> bool {
        self.ttf().is_some_and(|f| f.glyph_index(c).is_some())
    }
}

fn bundled_entry(family: FontFamily, bold: bool) -> (&'static [u8], &'static str) {
    match (family, bold) {
        (FontFamily::Sans, false) => (SANS_R, "NotoSans-Regular"),
        (FontFamily::Sans, true) => (SANS_B, "NotoSans-Bold"),
        (FontFamily::Serif, false) => (SERIF_R, "NotoSerif-Regular"),
        (FontFamily::Serif, true) => (SERIF_B, "NotoSerif-Bold"),
        (FontFamily::Mono, false) => (MONO_R, "NotoSansMono-Regular"),
        (FontFamily::Mono, true) => (MONO_B, "NotoSansMono-Bold"),
    }
}

pub(crate) fn bundled_static(family: FontFamily, bold: bool) -> &'static [u8] {
    bundled_entry(family, bold).0
}

/// The bundled face for `family`/`bold`.
pub fn bundled(family: FontFamily, bold: bool) -> FontFace {
    let (data, name) = bundled_entry(family, bold);
    FontFace {
        key: format!("bundled:{name}"),
        name: name.into(),
        system: false,
        index: 0,
        bytes: Bytes::Static(data),
    }
}

/// Whether the font's OS/2 `fsType` allows embedding a *subset* in a PDF that stays editable.
///
/// Allowed: installable (0) and editable (8) embedding. Refused: restricted license (2),
/// preview-and-print only (4, the file would have to stay read-only), "no subsetting" (0x100)
/// and "bitmap embedding only" (0x200). A font without an OS/2 table states no restriction.
pub fn embedding_allowed(face: &rustybuzz::ttf_parser::Face<'_>) -> bool {
    let Some(os2) = face
        .raw_face()
        .table(rustybuzz::ttf_parser::Tag::from_bytes(b"OS/2"))
    else {
        return true;
    };
    let Some(fs) = os2.get(8..10).map(|b| u16::from_be_bytes([b[0], b[1]])) else {
        return true;
    };
    matches!(fs & 0x000F, 0 | 8) && fs & 0x0300 == 0
}

/// What the text layout did about fonts; shown to the user when it matters.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FontReport {
    /// System fonts embedded (as subsets) for characters the bundled font lacks.
    pub fallbacks: Vec<FontUse>,
    /// System fonts that cover some characters but forbid embedding (OS/2 `fsType`); the
    /// characters fell back to the next candidate or to the missing-glyph box.
    pub restricted: Vec<FontUse>,
    /// Characters no usable font covers; drawn with the font's `.notdef` glyph.
    pub missing: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FontUse {
    pub font: String,
    pub chars: String,
}

impl FontReport {
    pub fn is_clean(&self) -> bool {
        self.fallbacks.is_empty() && self.restricted.is_empty() && self.missing.is_empty()
    }
    fn note(list: &mut Vec<FontUse>, font: &str, c: char) {
        match list.iter_mut().find(|u| u.font == font) {
            Some(u) => {
                if !u.chars.contains(c) {
                    u.chars.push(c)
                }
            }
            None => list.push(FontUse {
                font: font.into(),
                chars: c.to_string(),
            }),
        }
    }
    pub(crate) fn note_fallback(&mut self, font: &str, c: char) {
        Self::note(&mut self.fallbacks, font, c)
    }
    pub(crate) fn note_restricted(&mut self, font: &str, c: char) {
        Self::note(&mut self.restricted, font, c)
    }
    pub(crate) fn note_missing(&mut self, c: char) {
        if !self.missing.contains(c) {
            self.missing.push(c)
        }
    }
}

struct Sys {
    enabled: bool,
    db: Option<fontdb::Database>,
    /// Faces and directories registered before the first scan (tests, user font folders).
    extra_data: Vec<Vec<u8>>,
    extra_dirs: Vec<std::path::PathBuf>,
    /// char -> decision (None: nothing found).
    decided: HashMap<(char, bool), Option<Decision>>,
    loaded: HashMap<fontdb::ID, Arc<Vec<u8>>>,
}

#[derive(Clone)]
enum Decision {
    Use(FontFace),
    Restricted(String),
}

fn sys() -> &'static Mutex<Sys> {
    static S: OnceLock<Mutex<Sys>> = OnceLock::new();
    S.get_or_init(|| {
        Mutex::new(Sys {
            enabled: true,
            db: None,
            extra_data: Vec::new(),
            extra_dirs: Vec::new(),
            decided: HashMap::new(),
            loaded: HashMap::new(),
        })
    })
}

fn lock() -> std::sync::MutexGuard<'static, Sys> {
    sys().lock().unwrap_or_else(|e| e.into_inner())
}

/// Turn the system-font fallback on or off (on by default). The scan itself only happens the
/// first time a character is missing from the bundled fonts.
pub fn set_system_fonts_enabled(on: bool) {
    let mut s = lock();
    s.enabled = on;
    s.decided.clear();
}

/// Replace the fallback database with exactly `fonts` (no system scan). For tests and for
/// hosts that must not read the user's font folders.
pub fn use_only_fonts(fonts: Vec<Vec<u8>>) {
    let mut s = lock();
    let mut db = fontdb::Database::new();
    for f in fonts {
        db.load_font_data(f);
    }
    s.db = Some(db);
    s.extra_data.clear();
    s.extra_dirs.clear();
    s.loaded.clear();
    s.decided.clear();
}

/// Make an additional font available to the fallback search (before or after the first scan).
pub fn add_font_data(data: Vec<u8>) {
    let mut s = lock();
    match s.db.as_mut() {
        Some(db) => db.load_font_data(data),
        None => s.extra_data.push(data),
    }
    s.decided.clear();
}

/// Make every font in `dir` available to the fallback search.
pub fn add_font_dir(dir: &Path) {
    let mut s = lock();
    match s.db.as_mut() {
        Some(db) => db.load_fonts_dir(dir),
        None => s.extra_dirs.push(dir.to_owned()),
    }
    s.decided.clear();
}

impl Sys {
    fn db(&mut self) -> &mut fontdb::Database {
        if self.db.is_none() {
            let mut db = fontdb::Database::new();
            db.load_system_fonts();
            for d in self.extra_data.drain(..) {
                db.load_font_data(d);
            }
            for d in self.extra_dirs.drain(..) {
                db.load_fonts_dir(d);
            }
            self.db = Some(db);
        }
        self.db.as_mut().expect("just set")
    }
}

/// The best system font for `c`: one that has the glyph, has outlines, is not variable, can be
/// subset by `subsetter` and allows embedding. Result is cached per character.
pub(crate) fn system_font_for(c: char, bold: bool) -> SystemChoice {
    let mut s = lock();
    if !s.enabled {
        return SystemChoice::None;
    }
    if let Some(d) = s.decided.get(&(c, bold)) {
        return match d {
            None => SystemChoice::None,
            Some(Decision::Use(f)) => SystemChoice::Use(f.clone()),
            Some(Decision::Restricted(n)) => SystemChoice::Restricted(n.clone()),
        };
    }
    let decision = pick(&mut s, c, bold);
    s.decided.insert((c, bold), decision.clone());
    match decision {
        None => SystemChoice::None,
        Some(Decision::Use(f)) => SystemChoice::Use(f),
        Some(Decision::Restricted(n)) => SystemChoice::Restricted(n),
    }
}

pub(crate) enum SystemChoice {
    None,
    Use(FontFace),
    Restricted(String),
}

fn pick(s: &mut Sys, c: char, bold: bool) -> Option<Decision> {
    let db = s.db();
    let mut cands: Vec<(i32, fontdb::ID, u32, String)> = Vec::new();
    for f in db.faces() {
        if f.style != fontdb::Style::Normal {
            continue;
        }
        let want = if bold { 700i32 } else { 400 };
        let dist = (i32::from(f.weight.0) - want).abs();
        // Prefer sans faces and well-known broad-coverage families on ties.
        let name = f.post_script_name.clone();
        let fam = f
            .families
            .first()
            .map(|(n, _)| n.to_lowercase())
            .unwrap_or_default();
        let bonus = if fam.contains("noto") || fam.contains("arial unicode") {
            0
        } else if fam.contains("sans") || fam.contains("gothic") || fam.contains("hei") {
            50
        } else {
            100
        };
        cands.push((dist + bonus, f.id, f.index, name));
    }
    cands.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.3.cmp(&b.3)));
    let mut restricted: Option<String> = None;
    for (_, id, index, name) in cands {
        // Cheap checks on the mapped data first.
        let verdict = db.with_face_data(id, |data, idx| {
            let face = rustybuzz::ttf_parser::Face::parse(data, idx).ok()?;
            let gid = face.glyph_index(c)?;
            let t = face.tables();
            if (t.glyf.is_none() && t.cff.is_none()) || face.is_variable() {
                return None;
            }
            Some((embedding_allowed(&face), gid.0))
        });
        let Some(Some((allowed, gid))) = verdict else {
            continue;
        };
        if !allowed {
            restricted.get_or_insert(name);
            continue;
        }
        // The subsetter must accept it (rare unsupported layouts are skipped).
        let ok = db
            .with_face_data(id, |data, idx| {
                let mut r = subsetter::GlyphRemapper::new();
                r.remap(0);
                r.remap(gid);
                subsetter::subset(data, idx, &r).is_ok()
            })
            .unwrap_or(false);
        if !ok {
            continue;
        }
        let data = match s.loaded.get(&id) {
            Some(d) => d.clone(),
            None => {
                let d = Arc::new(
                    s.db.as_ref()
                        .and_then(|db| db.with_face_data(id, |d, _| d.to_vec()))?,
                );
                if s.loaded.len() >= 6 {
                    s.loaded.clear();
                }
                s.loaded.insert(id, d.clone());
                d
            }
        };
        return Some(Decision::Use(FontFace {
            key: format!("system:{name}:{index}"),
            name,
            system: true,
            index,
            bytes: Bytes::Shared(data),
        }));
    }
    restricted.map(Decision::Restricted)
}

/// Fonts used by one layout: index 0 is the primary (bundled) font.
pub(crate) struct FontSet {
    pub faces: Vec<FontFace>,
    pub report: FontReport,
    by_char: HashMap<char, usize>,
    bold: bool,
    system: bool,
}

impl FontSet {
    pub fn new(family: FontFamily, bold: bool, system: bool) -> FontSet {
        FontSet {
            faces: vec![bundled(family, bold)],
            report: FontReport::default(),
            by_char: HashMap::new(),
            bold,
            system,
        }
    }

    /// Index of the face to draw `c` with.
    pub fn face_for(&mut self, c: char) -> usize {
        if let Some(&i) = self.by_char.get(&c) {
            return i;
        }
        let i = self.choose(c);
        self.by_char.insert(c, i);
        i
    }

    fn choose(&mut self, c: char) -> usize {
        for (i, f) in self.faces.iter().enumerate() {
            if f.covers(c) {
                if i > 0 {
                    self.report.note_fallback(&f.name, c);
                }
                return i;
            }
        }
        let choice = if self.system {
            system_font_for(c, self.bold)
        } else {
            SystemChoice::None
        };
        match choice {
            SystemChoice::Use(f) => {
                self.report.note_fallback(&f.name, c);
                match self.faces.iter().position(|x| x.key == f.key) {
                    Some(i) => i,
                    None => {
                        self.faces.push(f);
                        self.faces.len() - 1
                    }
                }
            }
            SystemChoice::Restricted(name) => {
                self.report.note_restricted(&name, c);
                self.report.note_missing(c);
                0
            }
            SystemChoice::None => {
                self.report.note_missing(c);
                0
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_fonts_cover_latin_greek_cyrillic() {
        for fam in [FontFamily::Sans, FontFamily::Serif, FontFamily::Mono] {
            for bold in [false, true] {
                let f = bundled(fam, bold);
                for c in "AaZz09 .,éñßŁžΩαβжЯ€\u{301}".chars() {
                    assert!(f.covers(c), "{} lacks {c:?}", f.name);
                }
                assert!(!f.covers('日'));
                let face = f.ttf().unwrap();
                assert!(embedding_allowed(&face), "{} fsType", f.name);
            }
        }
    }

    #[test]
    fn fs_type_rules() {
        // Patch fsType (offset 8 of the OS/2 table) in a copy of a bundled font.
        let base = bundled(FontFamily::Sans, false);
        let data = base.bytes().to_vec();
        let face = rustybuzz::ttf_parser::Face::parse(&data, 0).unwrap();
        let rec = face
            .raw_face()
            .table(rustybuzz::ttf_parser::Tag::from_bytes(b"OS/2"))
            .unwrap();
        let off = rec.as_ptr() as usize - data.as_ptr() as usize + 8;
        for (fs, ok) in [
            (0u16, true),
            (8, true),
            (2, false),
            (4, false),
            (0x100, false),
            (0x200, false),
            (8 | 0x100, false),
        ] {
            let mut d = data.clone();
            d[off..off + 2].copy_from_slice(&fs.to_be_bytes());
            let f = rustybuzz::ttf_parser::Face::parse(&d, 0).unwrap();
            assert_eq!(embedding_allowed(&f), ok, "fsType {fs:#x}");
        }
    }

    #[test]
    fn bundled_size_budget() {
        let total: usize = [SANS_R, SANS_B, SERIF_R, SERIF_B, MONO_R, MONO_B]
            .iter()
            .map(|b| b.len())
            .sum();
        assert!(total < 2_000_000, "bundled fonts {total} bytes");
    }
}
