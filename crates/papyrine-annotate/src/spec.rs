//! A complete, defaulted description of one annotation: geometry plus properties.

use papyrine_ops::{Error, Result};

use crate::geometry::{Geometry, MarkupKind};
use crate::props::{AnnotProps, Color};

/// Geometry plus user properties; the single input of appearance generation.
#[derive(Clone, Debug, PartialEq)]
pub struct Spec {
    pub geometry: Geometry,
    pub props: AnnotProps,
}

impl Spec {
    pub fn new(geometry: Geometry, props: AnnotProps) -> Spec {
        Spec { geometry, props }
    }

    pub fn validate(&self) -> Result<()> {
        self.geometry.validate()?;
        self.props.validate()?;
        if let Geometry::Note { icon, .. } = &self.geometry
            && (icon.is_empty() || icon.len() > 64 || icon.contains(['\0', '\n']))
        {
            return Err(Error::invalid("invalid note icon name"));
        }
        Ok(())
    }

    /// `/C` after defaults.
    pub fn color(&self) -> Color {
        self.props
            .color
            .clone()
            .unwrap_or_else(|| default_color(&self.geometry))
    }

    pub fn opacity(&self) -> f64 {
        self.props.opacity.unwrap_or(1.0)
    }

    pub fn width(&self) -> f64 {
        self.props
            .width
            .unwrap_or_else(|| default_width(&self.geometry))
    }
}

pub fn default_color(g: &Geometry) -> Color {
    match g {
        Geometry::TextMarkup { kind, .. } => match kind {
            MarkupKind::Highlight => Color::rgb(1.0, 1.0, 0.0),
            MarkupKind::Underline => Color::rgb(0.0, 0.55, 0.0),
            MarkupKind::StrikeOut | MarkupKind::Squiggly => Color::rgb(1.0, 0.0, 0.0),
        },
        Geometry::Note { .. } => Color::rgb(1.0, 0.85, 0.0),
        Geometry::TextBox { .. } => Color::gray(0.0),
        Geometry::Ink { .. }
        | Geometry::Square { .. }
        | Geometry::Circle { .. }
        | Geometry::Line { .. } => Color::rgb(1.0, 0.0, 0.0),
    }
}

pub fn default_width(g: &Geometry) -> f64 {
    match g {
        Geometry::TextMarkup { .. } => 1.0,
        Geometry::Note { .. } | Geometry::TextBox { .. } => 0.0,
        Geometry::Ink { .. } => 2.0,
        Geometry::Square { .. } | Geometry::Circle { .. } | Geometry::Line { .. } => 1.0,
    }
}
