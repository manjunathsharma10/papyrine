//! Text layout and font embedding for text boxes.

pub mod embed;
pub mod fonts;
pub mod layout;

pub use fonts::{FontReport, FontUse, add_font_data, add_font_dir, set_system_fonts_enabled};
pub use layout::{Layout, Line, PlacedGlyph, layout, layout_with};
