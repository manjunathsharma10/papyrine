//! papyrine-text: search over page text (docs/ARCHITECTURE.md §4.7).
//!
//! The renderer supplies per-page chars with boxes ([`PageChars`]); this crate
//! normalizes (NFKC-equivalent compatibility folding, ligatures, hyphenation,
//! whitespace, case, diacritics, optional bidi reordering), matches, and maps
//! hits back to char ranges, merged per-line [`Quad`]s and snippets. It never
//! touches PDFium, so it stays light and fully unit-testable.
//!
//! Known limits (v0.1): matches never span pages; a genuinely hyphenated
//! compound broken at a line end ("well-\nknown") is indexed joined
//! ("wellknown"); whole-word on CJK uses UAX #29 boundaries, which treat each
//! ideograph as a word (no dictionary segmentation).

mod normalize;
mod quads;
mod search;

pub use quads::Quad;
pub use search::{
    BidiOrder, ExtraText, PageChars, PageInput, SearchEvent, SearchHit, SearchOptions,
    SearchSummary, Searcher, Snippet, TextSource, search_document,
};
