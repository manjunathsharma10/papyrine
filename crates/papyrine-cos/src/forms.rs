//! AcroForm helpers: list fields, set values (regenerating appearances), NeedAppearances.

use qpdf_sys as ffi;

use crate::{Document, ObjId, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Other,
    Text,
    Checkbox,
    Radio,
    PushButton,
    Choice,
    Signature,
}

/// A widget annotation that displays a field.
#[derive(Debug, Clone, PartialEq)]
pub struct Widget {
    /// `None` when the annotation is a direct object.
    pub id: Option<ObjId>,
    /// Zero-based page index, `None` when the widget is not on any page.
    pub page: Option<usize>,
    /// `[x0, y0, x1, y1]` as stored in `/Rect`.
    pub rect: [f64; 4],
}

/// A terminal form field with its inherited attributes resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct FormField {
    pub id: ObjId,
    pub kind: FieldKind,
    /// `/Ff` flag bits.
    pub flags: u32,
    pub quadding: i32,
    pub max_len: Option<u32>,
    /// Checkbox state (false for other kinds).
    pub checked: bool,
    /// `/Tx`, `/Btn`, `/Ch` or `/Sig`; empty when missing.
    pub field_type: String,
    /// Fully qualified name (`parent.child`).
    pub name: String,
    pub partial_name: String,
    pub alt_name: String,
    /// Text for text/choice fields, state name (no slash) for buttons.
    pub value: String,
    pub default_value: String,
    pub default_appearance: String,
    pub choices: Vec<String>,
    /// Non-`Off` appearance states of a checkbox or radio group.
    pub states: Vec<String>,
    pub widgets: Vec<Widget>,
}

impl FormField {
    pub const READ_ONLY: u32 = 1;
    pub const REQUIRED: u32 = 1 << 1;
    pub const MULTILINE: u32 = 1 << 12;
    pub const PASSWORD: u32 = 1 << 13;
}

fn text(b: Vec<u8>) -> String {
    String::from_utf8_lossy(&b).into_owned()
}

fn objid(id: i32, gen_: i32) -> Option<ObjId> {
    (id > 0).then(|| ObjId::new(id as u32, gen_ as u16))
}

impl Document {
    pub fn has_acroform(&self) -> Result<bool> {
        Ok(ffi::form_has_acroform(self.ffi())?)
    }

    /// All terminal form fields in tree order.
    pub fn form_fields(&self) -> Result<Vec<FormField>> {
        Ok(ffi::form_fields(self.ffi())?
            .into_iter()
            .map(|f| FormField {
                id: ObjId::new(f.id as u32, f.gen_ as u16),
                kind: match f.kind {
                    1 => FieldKind::Text,
                    2 => FieldKind::Checkbox,
                    3 => FieldKind::Radio,
                    4 => FieldKind::PushButton,
                    5 => FieldKind::Choice,
                    6 => FieldKind::Signature,
                    _ => FieldKind::Other,
                },
                flags: f.flags as u32,
                quadding: f.quadding,
                max_len: u32::try_from(f.max_len).ok(),
                checked: f.checked,
                field_type: text(f.field_type),
                name: text(f.name),
                partial_name: text(f.partial_name),
                alt_name: text(f.alt_name),
                value: text(f.value),
                default_value: text(f.default_value),
                default_appearance: text(f.default_appearance),
                choices: f.choices.into_iter().map(|b| text(b.v)).collect(),
                states: f.states.into_iter().map(|b| text(b.v)).collect(),
                widgets: f
                    .widgets
                    .into_iter()
                    .map(|w| Widget {
                        id: objid(w.id, w.gen_),
                        page: usize::try_from(w.page).ok(),
                        rect: [w.x0, w.y0, w.x1, w.y1],
                    })
                    .collect(),
            })
            .collect())
    }

    /// Find a field by fully qualified name.
    pub fn form_field(&self, qualified_name: &str) -> Result<Option<FormField>> {
        Ok(self
            .form_fields()?
            .into_iter()
            .find(|f| f.name == qualified_name))
    }

    /// Set a field value.
    ///
    /// * text and choice fields: `value` is the text; with `generate_appearance` the widgets get
    ///   fresh appearance streams (qpdf's generator handles ASCII text with the field's `/DA`
    ///   font; anything fancier should rely on the viewer re-rendering, see
    ///   [`Document::set_need_appearances`]);
    /// * checkbox and radio fields: `value` is a state name (`"Yes"`, `"Off"`, ...). Any value but
    ///   `Off` selects the on state, and the widgets' `/AS` entries are updated.
    ///
    /// `/NeedAppearances` is never turned on by this call.
    pub fn set_form_field_value(
        &self,
        field: ObjId,
        value: &str,
        generate_appearance: bool,
    ) -> Result<()> {
        ffi::form_set_value(
            self.ffi(),
            field.num as i32,
            i32::from(field.generation),
            value.as_bytes(),
            generate_appearance,
        )?;
        Ok(())
    }

    /// Tick or untick a checkbox.
    pub fn set_form_checkbox(&self, field: ObjId, checked: bool) -> Result<()> {
        self.set_form_field_value(field, if checked { "Yes" } else { "Off" }, false)
    }

    pub fn need_appearances(&self) -> Result<bool> {
        Ok(ffi::form_need_appearances(self.ffi())?)
    }

    /// Set `/AcroForm /NeedAppearances`. A no-op for documents without an AcroForm.
    pub fn set_need_appearances(&self, v: bool) -> Result<()> {
        Ok(ffi::form_set_need_appearances(self.ffi(), v)?)
    }

    /// Regenerate appearance streams of every text and choice widget, then clear
    /// `/NeedAppearances`.
    pub fn generate_form_appearances(&self) -> Result<()> {
        Ok(ffi::form_generate_appearances(self.ffi())?)
    }
}
