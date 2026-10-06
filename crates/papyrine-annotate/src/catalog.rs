//! Static metadata for the UI: which tools exist, which properties each supports, and how the
//! view-only "show or hide all annotations" switch interacts with annotation flags.

use serde::Serialize;

use crate::props::flags;

#[derive(Clone, Copy, Debug, Serialize)]
pub struct ToolInfo {
    /// Tool identifier used by the UI.
    pub id: &'static str,
    /// PDF annotation subtype it creates.
    pub subtype: &'static str,
    /// Property keys of [`crate::AnnotProps`] the tool exposes.
    pub props: &'static [&'static str],
    /// Needs text quads from the text layer rather than a drag rectangle.
    pub from_text_selection: bool,
}

const MARKUP_PROPS: &[&str] = &["color", "opacity", "author", "subject", "contents"];
const SHAPE_PROPS: &[&str] = &[
    "color", "opacity", "width", "dash", "fill", "author", "subject", "contents",
];

/// The MVP tool set (ROADMAP 1.12, no stamps).
pub const TOOLS: &[ToolInfo] = &[
    ToolInfo {
        id: "highlight",
        subtype: "Highlight",
        props: MARKUP_PROPS,
        from_text_selection: true,
    },
    ToolInfo {
        id: "underline",
        subtype: "Underline",
        props: &["color", "opacity", "width", "author", "subject", "contents"],
        from_text_selection: true,
    },
    ToolInfo {
        id: "strikeout",
        subtype: "StrikeOut",
        props: &["color", "opacity", "width", "author", "subject", "contents"],
        from_text_selection: true,
    },
    ToolInfo {
        id: "squiggly",
        subtype: "Squiggly",
        props: &["color", "opacity", "width", "author", "subject", "contents"],
        from_text_selection: true,
    },
    ToolInfo {
        id: "sticky_note",
        subtype: "Text",
        props: &["color", "opacity", "author", "subject", "contents"],
        from_text_selection: false,
    },
    ToolInfo {
        id: "text_box",
        subtype: "FreeText",
        props: &[
            "color", "opacity", "width", "dash", "fill", "author", "subject", "contents",
        ],
        from_text_selection: false,
    },
    ToolInfo {
        id: "pen",
        subtype: "Ink",
        props: &["color", "opacity", "width", "author", "subject", "contents"],
        from_text_selection: false,
    },
    ToolInfo {
        id: "eraser",
        subtype: "Ink",
        props: &[],
        from_text_selection: false,
    },
    ToolInfo {
        id: "rectangle",
        subtype: "Square",
        props: SHAPE_PROPS,
        from_text_selection: false,
    },
    ToolInfo {
        id: "oval",
        subtype: "Circle",
        props: SHAPE_PROPS,
        from_text_selection: false,
    },
    ToolInfo {
        id: "line",
        subtype: "Line",
        props: SHAPE_PROPS,
        from_text_selection: false,
    },
    ToolInfo {
        id: "arrow",
        subtype: "Line",
        props: SHAPE_PROPS,
        from_text_selection: false,
    },
];

/// True for the subtypes whose appearance Papyrine can regenerate when properties change.
pub fn is_editable_subtype(subtype: &str) -> bool {
    matches!(
        subtype,
        "Highlight"
            | "Underline"
            | "StrikeOut"
            | "Squiggly"
            | "Text"
            | "FreeText"
            | "Ink"
            | "Square"
            | "Circle"
            | "Line"
    )
}

/// Whether an annotation with `/F` flags is drawn in the viewer. "Show all annotations" is a
/// view setting (never saved in the document): when off, every annotation except form widgets
/// and links is hidden; when on, only the annotation's own Hidden/NoView flags apply.
pub fn visible_in_view(show_annotations: bool, subtype: &str, f: u32) -> bool {
    if f & (flags::HIDDEN | flags::NO_VIEW) != 0 {
        return false;
    }
    show_annotations || matches!(subtype, "Widget" | "Link" | "Popup")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visibility_rules() {
        assert!(visible_in_view(true, "Highlight", flags::PRINT));
        assert!(!visible_in_view(false, "Highlight", flags::PRINT));
        assert!(visible_in_view(false, "Widget", 4));
        assert!(!visible_in_view(true, "Text", flags::HIDDEN));
    }

    #[test]
    fn tools_are_unique() {
        let mut ids: Vec<_> = TOOLS.iter().map(|t| t.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), TOOLS.len());
    }
}
