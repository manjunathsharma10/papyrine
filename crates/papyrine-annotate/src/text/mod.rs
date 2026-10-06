//! Text layout and font embedding for text boxes.

pub mod embed;
pub mod fonts;
pub mod layout;

pub use fonts::{
    FontReport, FontUse, add_font_data, add_font_dir, set_system_fonts_enabled, use_only_fonts,
};
pub use layout::{Layout, Line, PlacedGlyph, layout, layout_with};

/// Which fonts a text box with this `text` and `style` would use: lets the UI warn about
/// characters that need a system font, or that no embeddable font covers, before committing.
/// Deterministic; equals the report `AddAnnotation::font_report` gives after it runs.
pub fn analyze(text: &str, style: &crate::TextStyle) -> FontReport {
    layout(text, style, f64::MAX / 4.0).report
}
